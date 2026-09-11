//! Turning interleaved 16-bit PCM into something a Cast receiver will play.
//!
//! Google's supported-media list for Google Home, Nest Audio and Chromecast Audio includes
//! FLAC (to 96 kHz/24-bit) and WAV/LPCM, and the audio-device guidance caps a stream at
//! 2 Mbit/s. WAV at 44.1 kHz stereo is 1.4 Mbit/s, which fits but leaves no margin; FLAC
//! roughly halves it with a pure-Rust encoder and no C toolchain, so FLAC is the default
//! and WAV is the zero-surprises fallback.
//!
//! Both encoders answer the same two shapes:
//!
//! - [`CastEncoder::encode_all`] for the pre-render path, where the whole song is in hand
//!   and the header can be honest about its length;
//! - [`CastEncoder::push`]/[`CastEncoder::finish`] for `--live`, where it cannot: FLAC
//!   emits a STREAMINFO with *unknown* total samples and then whole fixed-size frames, and
//!   WAV emits a header declaring the largest size a 32-bit RIFF field can hold, because a
//!   chunked reader never reaches the end of the stream to notice.

use flacenc::bitsink::ByteSink;
use flacenc::component::{BitRepr, Stream, StreamInfo};
use flacenc::config::Encoder as EncoderConfig;
use flacenc::error::{Verified, Verify};
use flacenc::source::{Fill, FrameBuf, MemSource};
use starplayer_offline::wav::{WavHeader, WavSample};

use crate::CastError;

/// The FLAC block size both the pre-render and the live path encode at.
///
/// 4096 is `flacenc`'s own default and the reference encoder's: about 93 ms at 44.1 kHz,
/// which is small enough that a live listener is not waiting on the encoder and large
/// enough that per-frame overhead does not matter.
pub const FLAC_BLOCK_SIZE: usize = 4_096;

/// The `data` chunk size a live WAV header declares: the largest a RIFF header can hold.
///
/// `write_wav` refuses to write a size it would have to wrap (see `wav.rs`'s "4 GiB"
/// section) — a file whose header lies is a file every reader disagrees about. A *live*
/// stream is the one deliberate exception: its length is unknown when the header goes out,
/// the response is chunked so no reader ever sees the end, and a receiver that trusts the
/// declared length will simply never reach it. Sized so that the outer `RIFF` size fits
/// too, which is 36 bytes more than the `data` payload.
const LIVE_WAV_DECLARED_SAMPLES: usize = ((u32::MAX as usize) - 36) / 2;

/// Encodes interleaved 16-bit PCM into bytes a Cast receiver will play.
pub trait CastEncoder {
    /// The MIME type to put in the LOAD payload and the HTTP response.
    fn content_type(&self) -> &'static str;

    /// The extension for the URL path. Some receivers sniff it.
    fn extension(&self) -> &'static str;

    /// Encode a complete interleaved buffer in one call.
    fn encode_all(&mut self, samples: &[i16], sample_rate_hz: u32, channels: u16) -> Result<Vec<u8>, CastError>;

    /// Encode as much of `samples` as makes whole output units, holding the rest back.
    ///
    /// May legitimately return an empty `Vec`: the first call emits a header, and later
    /// calls emit nothing until enough samples have arrived to fill a frame.
    fn push(&mut self, samples: &[i16]) -> Result<Vec<u8>, CastError>;

    /// Flush whatever `push` held back, and end the stream.
    fn finish(&mut self) -> Result<Vec<u8>, CastError>;
}

/// FLAC at 16 bits, through `flacenc`.
pub struct FlacEncoder {
    sample_rate_hz: u32,
    channels: u16,
    config: Verified<EncoderConfig>,
    /// Streaming state, built on the first [`CastEncoder::push`].
    stream_info: Option<StreamInfo>,
    /// Interleaved samples that did not fill a frame.
    remainder: Vec<i32>,
    frame_number: usize,
}

impl FlacEncoder {
    /// A FLAC encoder for `channels` interleaved channels at `sample_rate_hz`.
    pub fn new(sample_rate_hz: u32, channels: u16) -> Result<FlacEncoder, CastError> {
        let config = EncoderConfig::default()
            .into_verified()
            .map_err(|(_, error)| CastError::Io(format!("the FLAC encoder's configuration is invalid: {error}")))?;
        Ok(FlacEncoder { sample_rate_hz, channels: channels.max(1), config, stream_info: None, remainder: Vec::new(), frame_number: 0 })
    }

    /// The `fLaC` magic and the STREAMINFO block, with the total sample count left
    /// **unknown** — a zero total, which is what the format reserves for "the encoder did
    /// not know".
    fn streaming_header(&mut self) -> Result<Vec<u8>, CastError> {
        let mut stream = Stream::new(self.sample_rate_hz as usize, self.channels as usize, 16)
            .map_err(|error| CastError::Io(format!("FLAC stream setup failed: {error}")))?;
        stream
            .stream_info_mut()
            .set_block_sizes(FLAC_BLOCK_SIZE, FLAC_BLOCK_SIZE)
            .map_err(|error| CastError::Io(format!("FLAC block size {FLAC_BLOCK_SIZE} rejected: {error}")))?;
        // Zero in all four "unknown" fields is what the format reserves for an encoder
        // that does not know yet. `StreamInfo::new`'s defaults are the *opposite* — a
        // minimum of `u32::MAX` and a maximum of zero, so that the first frame narrows
        // them — and writing those into a header no frame will ever update produces a
        // STREAMINFO that says the smallest frame is bigger than the largest.
        stream
            .stream_info_mut()
            .set_frame_sizes(0, 0)
            .map_err(|error| CastError::Io(format!("FLAC frame sizes rejected: {error}")))?;
        stream.stream_info_mut().set_total_samples(0);
        let info = stream.stream_info().clone();
        // A `Stream` with no frames writes exactly the magic and the metadata blocks,
        // which is the whole of a FLAC stream's header.
        let mut sink = ByteSink::new();
        stream.write(&mut sink).map_err(|error| CastError::Io(format!("writing the FLAC header failed: {error}")))?;
        self.stream_info = Some(info);
        Ok(sink.into_inner())
    }

    /// Encode every whole frame sitting in `remainder`, leaving the tail behind.
    fn drain_whole_frames(&mut self, output: &mut Vec<u8>) -> Result<(), CastError> {
        let channels = self.channels as usize;
        let frame_samples = FLAC_BLOCK_SIZE * channels;
        let Some(stream_info) = self.stream_info.clone() else { return Ok(()) };

        while self.remainder.len() >= frame_samples {
            let block: Vec<i32> = self.remainder.drain(..frame_samples).collect();
            self.encode_one_frame(&block, FLAC_BLOCK_SIZE, &stream_info, output)?;
        }
        Ok(())
    }

    /// Encode one frame of `block_frames` frames from `block` and append its bytes.
    fn encode_one_frame(&mut self, block: &[i32], block_frames: usize, stream_info: &StreamInfo, output: &mut Vec<u8>) -> Result<(), CastError> {
        let channels = self.channels as usize;
        let mut frame_buffer = FrameBuf::with_size(channels, block_frames).map_err(|error| CastError::Io(format!("FLAC frame buffer setup failed: {error}")))?;
        frame_buffer.fill_interleaved(block).map_err(|error| CastError::Io(format!("filling a FLAC frame failed: {error}")))?;
        let frame = flacenc::encode_fixed_size_frame(&self.config, &frame_buffer, self.frame_number, stream_info)
            .map_err(|error| CastError::Io(format!("FLAC frame {} failed to encode: {error}", self.frame_number)))?;
        self.frame_number += 1;
        let mut sink = ByteSink::new();
        frame.write(&mut sink).map_err(|error| CastError::Io(format!("writing a FLAC frame failed: {error}")))?;
        output.extend_from_slice(sink.as_slice());
        Ok(())
    }
}

impl CastEncoder for FlacEncoder {
    fn content_type(&self) -> &'static str { "audio/flac" }

    fn extension(&self) -> &'static str { "flac" }

    fn encode_all(&mut self, samples: &[i16], sample_rate_hz: u32, channels: u16) -> Result<Vec<u8>, CastError> {
        let channels = channels.max(1);
        let widened: Vec<i32> = samples.iter().map(|&sample| i32::from(sample)).collect();
        let source = MemSource::from_samples(&widened, channels as usize, 16, sample_rate_hz as usize);
        let stream = flacenc::encode_with_fixed_block_size(&self.config, source, FLAC_BLOCK_SIZE)
            .map_err(|error| CastError::Io(format!("FLAC encoding failed: {error}")))?;
        let mut sink = ByteSink::new();
        stream.write(&mut sink).map_err(|error| CastError::Io(format!("writing the FLAC stream failed: {error}")))?;
        Ok(sink.into_inner())
    }

    fn push(&mut self, samples: &[i16]) -> Result<Vec<u8>, CastError> {
        let mut output = Vec::new();
        if self.stream_info.is_none() {
            output = self.streaming_header()?;
        }
        self.remainder.extend(samples.iter().map(|&sample| i32::from(sample)));
        self.drain_whole_frames(&mut output)?;
        Ok(output)
    }

    fn finish(&mut self) -> Result<Vec<u8>, CastError> {
        let mut output = Vec::new();
        // Nothing was ever pushed: the stream still needs its header to be a FLAC stream
        // at all.
        if self.stream_info.is_none() {
            output = self.streaming_header()?;
        }
        self.drain_whole_frames(&mut output)?;

        let channels = self.channels as usize;
        if !self.remainder.is_empty() {
            let stream_info = self.stream_info.clone().expect("the header was written above");
            let block: Vec<i32> = std::mem::take(&mut self.remainder);
            let block_frames = block.len() / channels;
            // A frame shorter than the FLAC minimum block size (16 frames) cannot be
            // encoded; a fraction of a millisecond of tail is not worth a malformed frame.
            if block_frames >= 16 {
                let usable = block_frames * channels;
                self.encode_one_frame(&block[..usable], block_frames, &stream_info, &mut output)?;
            }
        }
        Ok(output)
    }
}

/// WAV/LPCM at 16 bits: the fallback that needs no codec at all.
pub struct WavEncoder {
    sample_rate_hz: u32,
    channels: u16,
    header_written: bool,
}

impl WavEncoder {
    /// A WAV encoder for `channels` interleaved channels at `sample_rate_hz`.
    pub fn new(sample_rate_hz: u32, channels: u16) -> WavEncoder { WavEncoder { sample_rate_hz, channels: channels.max(1), header_written: false } }

    fn header_bytes(sample_rate_hz: u32, channels: u16, sample_count: usize) -> Result<Vec<u8>, CastError> {
        let header = WavHeader::for_format::<i16>(sample_rate_hz, channels);
        let mut bytes = Vec::with_capacity(44);
        header.write_to(&mut bytes, sample_count).map_err(|error| CastError::Io(format!("writing the WAV header failed: {error}")))?;
        Ok(bytes)
    }

    fn payload(samples: &[i16]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(samples.len() * 2);
        for &sample in samples {
            sample.write_le(&mut payload);
        }
        payload
    }
}

impl CastEncoder for WavEncoder {
    fn content_type(&self) -> &'static str { "audio/wav" }

    fn extension(&self) -> &'static str { "wav" }

    fn encode_all(&mut self, samples: &[i16], sample_rate_hz: u32, channels: u16) -> Result<Vec<u8>, CastError> {
        let mut bytes = WavEncoder::header_bytes(sample_rate_hz, channels.max(1), samples.len())?;
        bytes.extend_from_slice(&WavEncoder::payload(samples));
        // RIFF chunks are word-aligned; an odd-length payload needs its pad byte.
        if bytes.len() % 2 == 1 {
            bytes.push(0);
        }
        Ok(bytes)
    }

    fn push(&mut self, samples: &[i16]) -> Result<Vec<u8>, CastError> {
        let mut bytes = Vec::new();
        if !self.header_written {
            bytes = WavEncoder::header_bytes(self.sample_rate_hz, self.channels, LIVE_WAV_DECLARED_SAMPLES)?;
            self.header_written = true;
        }
        bytes.extend_from_slice(&WavEncoder::payload(samples));
        Ok(bytes)
    }

    /// Nothing is held back by a WAV stream: every sample went out in the `push` that
    /// produced it, and there is no trailer to write.
    fn finish(&mut self) -> Result<Vec<u8>, CastError> {
        if self.header_written {
            return Ok(Vec::new());
        }
        // A live session that ended before rendering anything still has to be a WAV file.
        self.header_written = true;
        WavEncoder::header_bytes(self.sample_rate_hz, self.channels, LIVE_WAV_DECLARED_SAMPLES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer_offline::wav::read_wav;

    /// A short interleaved stereo ramp, distinctive enough that a decode can be checked
    /// sample for sample.
    fn ramp(frames: usize) -> Vec<i16> {
        (0..frames).flat_map(|frame| [(frame as i16).wrapping_mul(37), (frame as i16).wrapping_mul(-53)]).collect()
    }

    /// `(stream_info, total decoded frames, interleaved samples)` of a FLAC stream this
    /// crate wrote.
    fn decode_flac(bytes: &[u8]) -> (StreamInfo, usize, Vec<i32>) {
        use flacenc::component::Decode;

        assert_eq!(&bytes[0..4], b"fLaC", "a FLAC stream starts with its magic");
        // Our writer emits exactly one metadata block — STREAMINFO — so its 34-byte body
        // starts after the 4-byte magic and the 4-byte block header.
        let (_, stream_info) = flacenc::component::parser::stream_info::<()>(&bytes[8..]).expect("STREAMINFO parses");

        let mut rest = &bytes[8 + 34..];
        let mut frames = 0usize;
        let mut samples = Vec::new();
        let mut parse_frame = flacenc::component::parser::frame::<()>(&stream_info, true);
        while !rest.is_empty() {
            let Ok((remaining, frame)) = parse_frame(rest) else { break };
            frames += frame.block_size();
            samples.extend(frame.decode());
            rest = remaining;
        }
        assert!(rest.is_empty(), "{} bytes were left unparsed", rest.len());
        (stream_info, frames, samples)
    }

    #[test]
    fn a_whole_flac_encode_reports_its_own_rate_channels_and_length() {
        let samples = ramp(10_000);
        let mut encoder = FlacEncoder::new(44_100, 2).unwrap();
        let bytes = encoder.encode_all(&samples, 44_100, 2).unwrap();

        let (stream_info, frames, decoded) = decode_flac(&bytes);
        assert_eq!(stream_info.sample_rate(), 44_100);
        assert_eq!(stream_info.channels(), 2);
        assert_eq!(stream_info.bits_per_sample(), 16);
        assert_eq!(stream_info.total_samples(), 10_000, "a pre-render knows its own length and says so");
        assert_eq!(frames, 10_000);
        let expected: Vec<i32> = samples.iter().map(|&sample| i32::from(sample)).collect();
        assert_eq!(decoded, expected, "FLAC is lossless, so the decode is the input");
    }

    #[test]
    fn a_live_flac_stream_declares_an_unknown_length_and_still_round_trips() {
        let mut encoder = FlacEncoder::new(22_050, 2).unwrap();
        let samples = ramp(FLAC_BLOCK_SIZE * 2 + 1_000);

        let mut bytes = Vec::new();
        // Pushed in ragged blocks, the way a driver thread would.
        for chunk in samples.chunks(1_234 * 2) {
            bytes.extend_from_slice(&encoder.push(chunk).unwrap());
        }
        bytes.extend_from_slice(&encoder.finish().unwrap());

        let (stream_info, frames, decoded) = decode_flac(&bytes);
        assert_eq!(stream_info.total_samples(), 0, "a live stream's length is unknown, which FLAC spells as zero");
        assert_eq!(stream_info.sample_rate(), 22_050);
        assert_eq!(frames, FLAC_BLOCK_SIZE * 2 + 1_000);
        let expected: Vec<i32> = samples.iter().map(|&sample| i32::from(sample)).collect();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn a_live_flac_stream_that_never_rendered_anything_is_still_a_flac_stream() {
        let mut encoder = FlacEncoder::new(44_100, 2).unwrap();
        let bytes = encoder.finish().unwrap();
        assert_eq!(&bytes[0..4], b"fLaC");
    }

    #[test]
    fn a_whole_wav_encode_round_trips_through_the_offline_reader() {
        let samples = ramp(1_000);
        let mut encoder = WavEncoder::new(44_100, 2);
        let bytes = encoder.encode_all(&samples, 44_100, 2).unwrap();

        let directory = std::env::temp_dir().join(format!("starplayer-cast-wav-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("round-trip.wav");
        std::fs::write(&path, &bytes).unwrap();

        let (header, read_back) = read_wav::<i16>(&path).unwrap();
        assert_eq!(header.sample_rate_hz, 44_100);
        assert_eq!(header.channels, 2);
        assert_eq!(header.bits_per_sample, 16);
        assert_eq!(read_back, samples);
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_live_wav_header_declares_the_largest_size_a_riff_field_can_hold() {
        let mut encoder = WavEncoder::new(48_000, 2);
        let first = encoder.push(&ramp(4)).unwrap();
        assert_eq!(&first[0..4], b"RIFF");
        let riff_size = u32::from_le_bytes([first[4], first[5], first[6], first[7]]);
        let data_size = u32::from_le_bytes([first[40], first[41], first[42], first[43]]);
        assert_eq!(data_size, (LIVE_WAV_DECLARED_SAMPLES * 2) as u32);
        assert_eq!(riff_size, data_size + 36, "the RIFF size covers the two chunk headers and the fmt body too");
        assert_eq!(first.len(), 44 + 4 * 2 * 2, "the first push carries the header and its own four stereo frames");

        // Later pushes are payload only.
        let second = encoder.push(&ramp(4)).unwrap();
        assert_eq!(second.len(), 4 * 2 * 2);
        assert!(encoder.finish().unwrap().is_empty(), "a WAV stream holds nothing back");
    }

    #[test]
    fn both_encoders_name_the_mime_type_and_extension_the_load_payload_needs() {
        let flac = FlacEncoder::new(44_100, 2).unwrap();
        assert_eq!(flac.content_type(), "audio/flac");
        assert_eq!(flac.extension(), "flac");
        let wav = WavEncoder::new(44_100, 2);
        assert_eq!(wav.content_type(), "audio/wav");
        assert_eq!(wav.extension(), "wav");
    }

    #[test]
    fn flac_is_smaller_than_wav_for_the_same_audio() {
        let samples = ramp(44_100);
        let flac = FlacEncoder::new(44_100, 2).unwrap().encode_all(&samples, 44_100, 2).unwrap();
        let wav = WavEncoder::new(44_100, 2).encode_all(&samples, 44_100, 2).unwrap();
        assert!(flac.len() < wav.len(), "FLAC {} bytes vs WAV {} bytes", flac.len(), wav.len());
    }
}
