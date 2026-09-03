//! A minimal RIFF/WAVE reader and writer for the offline renderer's output.
//!
//! [`write_wav`] covers exactly the four sample types the renderer produces —
//! [`i16`], [`I24`] (packed as 24-in-32, written as three little-endian bytes), [`i32`]
//! and IEEE-float `f32` — and nothing else: no `LIST`, no `fact` chunk, no extensible
//! format tag. [`read_wav`] is the mirror, kept only so this module's own tests can round
//! -trip a file without a second WAV library in the tree; it accepts exactly what
//! [`write_wav`] produces plus reasonable chunk ordering, and rejects anything else with
//! [`WavError::Malformed`] rather than guessing.
//!
//! # 4 GiB
//!
//! A `RIFF` chunk size and a `data` chunk size are both 32-bit fields. A render whose
//! payload would not fit is rejected with [`WavError::TooLarge`] rather than writing a
//! header whose size has silently wrapped — a wrapped header is a file every reader
//! disagrees about, and a byte count that large is almost certainly the wrong render
//! length rather than an intentional file.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;

use starplayer::mixer::I24;

/// PCM format tag (`WAVE_FORMAT_PCM`) — used for [`i16`], [`I24`] and [`i32`].
const WAVE_FORMAT_PCM: u16 = 1;
/// IEEE-float format tag (`WAVE_FORMAT_IEEE_FLOAT`) — used for `f32`.
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

/// A failure to write or read a WAV file.
#[derive(Debug)]
pub enum WavError {
    /// The payload does not fit a 32-bit RIFF or `data` chunk size (task D5 deliverable
    /// 1: rejected outright rather than written with a wrapped header).
    TooLarge,
    /// Opening, writing or reading the file failed.
    Io(io::Error),
    /// The bytes are not a WAV file [`read_wav`] understands.
    Malformed(&'static str),
}

impl std::fmt::Display for WavError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WavError::TooLarge => formatter.write_str("render is too large for a 32-bit WAV chunk size (over ~4 GiB)"),
            WavError::Io(error) => write!(formatter, "I/O error: {error}"),
            WavError::Malformed(reason) => write!(formatter, "not a WAV file this reader understands: {reason}"),
        }
    }
}

impl std::error::Error for WavError {}

impl From<io::Error> for WavError {
    fn from(error: io::Error) -> WavError { WavError::Io(error) }
}

/// One interleaved sample this writer can encode and this reader can decode.
///
/// `BYTES_PER_SAMPLE` is spelled out separately from `BITS_PER_SAMPLE` because [`I24`]'s
/// three file bytes are not `BITS_PER_SAMPLE / 8` of its four-byte in-memory container.
pub trait WavSample: Copy + Default {
    /// Value written to the `fmt ` chunk's `wBitsPerSample`.
    const BITS_PER_SAMPLE: u16;
    /// Value written to the `fmt ` chunk's `wFormatTag`.
    const FORMAT_TAG: u16;
    /// Bytes one sample occupies in the file — three for [`I24`], otherwise
    /// `BITS_PER_SAMPLE / 8`.
    const BYTES_PER_SAMPLE: u16;

    /// Append this sample's little-endian file bytes to `destination`.
    fn write_le(self, destination: &mut Vec<u8>);

    /// Decode one sample from exactly [`WavSample::BYTES_PER_SAMPLE`] little-endian bytes.
    fn read_le(bytes: &[u8]) -> Self;
}

impl WavSample for i16 {
    const BITS_PER_SAMPLE: u16 = 16;
    const FORMAT_TAG: u16 = WAVE_FORMAT_PCM;
    const BYTES_PER_SAMPLE: u16 = 2;

    fn write_le(self, destination: &mut Vec<u8>) { destination.extend_from_slice(&self.to_le_bytes()); }

    fn read_le(bytes: &[u8]) -> i16 { i16::from_le_bytes([bytes[0], bytes[1]]) }
}

impl WavSample for I24 {
    const BITS_PER_SAMPLE: u16 = 24;
    const FORMAT_TAG: u16 = WAVE_FORMAT_PCM;
    const BYTES_PER_SAMPLE: u16 = 3;

    fn write_le(self, destination: &mut Vec<u8>) { destination.extend_from_slice(&self.to_le_bytes()); }

    /// Sign-extends the packed 24-bit value into `I24`'s `i32` container by shifting it to
    /// the top of the word and back — the same convention [`I24::to_le_bytes`] packs.
    fn read_le(bytes: &[u8]) -> I24 {
        let unsigned = i32::from(bytes[0]) | (i32::from(bytes[1]) << 8) | (i32::from(bytes[2]) << 16);
        I24((unsigned << 8) >> 8)
    }
}

impl WavSample for i32 {
    const BITS_PER_SAMPLE: u16 = 32;
    const FORMAT_TAG: u16 = WAVE_FORMAT_PCM;
    const BYTES_PER_SAMPLE: u16 = 4;

    fn write_le(self, destination: &mut Vec<u8>) { destination.extend_from_slice(&self.to_le_bytes()); }

    fn read_le(bytes: &[u8]) -> i32 { i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) }
}

impl WavSample for f32 {
    const BITS_PER_SAMPLE: u16 = 32;
    const FORMAT_TAG: u16 = WAVE_FORMAT_IEEE_FLOAT;
    const BYTES_PER_SAMPLE: u16 = 4;

    fn write_le(self, destination: &mut Vec<u8>) { destination.extend_from_slice(&self.to_le_bytes()); }

    fn read_le(bytes: &[u8]) -> f32 { f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) }
}

/// The `fmt ` chunk's fields, and the arithmetic that turns a sample count into the RIFF
/// and `data` chunk sizes both [`write_wav`] and [`read_wav`] agree on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WavHeader {
    /// Output rate in Hz.
    pub sample_rate_hz: u32,
    /// Interleaved channel count. At least one.
    pub channels: u16,
    /// `wBitsPerSample`.
    pub bits_per_sample: u16,
    /// `wFormatTag` — [`WAVE_FORMAT_PCM`] or [`WAVE_FORMAT_IEEE_FLOAT`].
    pub format_tag: u16,
}

impl WavHeader {
    /// A header for `channels` interleaved channels of `S` at `sample_rate_hz`.
    pub fn for_format<S: WavSample>(sample_rate_hz: u32, channels: u16) -> WavHeader {
        WavHeader { sample_rate_hz, channels: channels.max(1), bits_per_sample: S::BITS_PER_SAMPLE, format_tag: S::FORMAT_TAG }
    }

    fn bytes_per_sample(&self) -> u32 { self.bits_per_sample as u32 / 8 }

    /// `nBlockAlign` — bytes per interleaved frame.
    fn block_align(&self) -> u32 { self.bytes_per_sample() * self.channels as u32 }

    /// `nAvgBytesPerSec`.
    fn byte_rate(&self) -> u64 { self.sample_rate_hz as u64 * self.block_align() as u64 }

    /// The `data` chunk's declared size and the outer `RIFF` chunk's declared size, for
    /// `sample_count` interleaved samples (frames × channels) of this header's sample
    /// type. `Err` when either would not fit the format's 32-bit size fields.
    fn chunk_sizes(&self, sample_count: usize) -> Result<(u32, u32), WavError> {
        let data_bytes = (sample_count as u64).checked_mul(self.bytes_per_sample() as u64).ok_or(WavError::TooLarge)?;
        if data_bytes > u32::MAX as u64 {
            return Err(WavError::TooLarge);
        }
        // `RIFF` size excludes the `RIFF`/size fields themselves but includes everything
        // after: the `WAVE` tag, the whole `fmt ` chunk (8-byte header + 16 bytes of
        // fields) and the whole `data` chunk (8-byte header + payload, plus one pad byte
        // if the payload is odd-length — RIFF chunks are word-aligned).
        let data_chunk_bytes = data_bytes + (data_bytes % 2);
        let riff_size = 4u64 + (8 + 16) + (8 + data_chunk_bytes);
        if riff_size > u32::MAX as u64 {
            return Err(WavError::TooLarge);
        }
        Ok((riff_size as u32, data_bytes as u32))
    }

    /// Write the `RIFF`/`WAVE`, `fmt ` and `data` chunk headers for `sample_count`
    /// interleaved samples, leaving the writer positioned to receive exactly that many
    /// bytes of sample data (plus, when `sample_count`'s bytes are odd, one pad byte the
    /// caller must also write — [`write_wav`] does both).
    fn write<W: Write>(&self, writer: &mut W, sample_count: usize) -> Result<u32, WavError> {
        let (riff_size, data_bytes) = self.chunk_sizes(sample_count)?;
        writer.write_all(b"RIFF")?;
        writer.write_all(&riff_size.to_le_bytes())?;
        writer.write_all(b"WAVE")?;
        writer.write_all(b"fmt ")?;
        writer.write_all(&16u32.to_le_bytes())?;
        writer.write_all(&self.format_tag.to_le_bytes())?;
        writer.write_all(&self.channels.to_le_bytes())?;
        writer.write_all(&self.sample_rate_hz.to_le_bytes())?;
        writer.write_all(&(self.byte_rate() as u32).to_le_bytes())?;
        writer.write_all(&(self.block_align() as u16).to_le_bytes())?;
        writer.write_all(&self.bits_per_sample.to_le_bytes())?;
        writer.write_all(b"data")?;
        writer.write_all(&data_bytes.to_le_bytes())?;
        Ok(data_bytes)
    }
}

/// Write `samples` — interleaved frames of `channels` channels — as a WAV file at `path`.
///
/// `S` selects the file's format tag and bit depth: [`i16`], [`I24`] and [`i32`] are
/// written as `WAVE_FORMAT_PCM`; `f32` as `WAVE_FORMAT_IEEE_FLOAT` (format tag 3).
pub fn write_wav<S: WavSample>(path: impl AsRef<Path>, sample_rate_hz: u32, channels: u16, samples: &[S]) -> Result<(), WavError> {
    let header = WavHeader::for_format::<S>(sample_rate_hz, channels);
    let mut writer = BufWriter::new(File::create(path)?);
    let data_bytes = header.write(&mut writer, samples.len())?;

    let mut payload = Vec::with_capacity(data_bytes as usize);
    for &sample in samples {
        sample.write_le(&mut payload);
    }
    writer.write_all(&payload)?;
    if data_bytes % 2 == 1 {
        writer.write_all(&[0u8])?;
    }
    writer.flush()?;
    Ok(())
}

/// Read a WAV file [`write_wav`] could have produced: `RIFF`/`WAVE`, a `fmt ` chunk and a
/// `data` chunk in either order, no other chunk understood. `S` must match the file's
/// declared format tag and bit depth exactly, or this reports [`WavError::Malformed`].
///
/// Exists for this module's own round-trip tests; nothing in the CLI reads a WAV file
/// back.
pub fn read_wav<S: WavSample>(path: impl AsRef<Path>) -> Result<(WavHeader, Vec<S>), WavError> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;

    let riff = bytes.get(0..12).ok_or(WavError::Malformed("shorter than a RIFF header"))?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return Err(WavError::Malformed("missing RIFF/WAVE signature"));
    }

    let mut header = None;
    let mut data = None;
    let mut cursor = 12usize;
    while let Some(chunk) = bytes.get(cursor..cursor + 8) {
        let name = &chunk[0..4];
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as usize;
        let body_start = cursor + 8;
        let body = bytes.get(body_start..body_start + size).ok_or(WavError::Malformed("chunk runs past the end of the file"))?;

        if name == b"fmt " {
            if size < 16 {
                return Err(WavError::Malformed("fmt chunk is shorter than 16 bytes"));
            }
            let format_tag = u16::from_le_bytes([body[0], body[1]]);
            let channels = u16::from_le_bytes([body[2], body[3]]);
            let sample_rate_hz = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            let bits_per_sample = u16::from_le_bytes([body[14], body[15]]);
            header = Some(WavHeader { sample_rate_hz, channels, bits_per_sample, format_tag });
        } else if name == b"data" {
            data = Some(body.to_vec());
        }

        // Chunks are word-aligned: an odd-sized body is followed by one pad byte that is
        // not part of the chunk's declared size.
        cursor = body_start + size + (size % 2);
    }

    let header = header.ok_or(WavError::Malformed("no fmt chunk"))?;
    let data = data.ok_or(WavError::Malformed("no data chunk"))?;
    if header.format_tag != S::FORMAT_TAG || header.bits_per_sample != S::BITS_PER_SAMPLE {
        return Err(WavError::Malformed("file format does not match the requested sample type"));
    }

    let sample_bytes = S::BYTES_PER_SAMPLE as usize;
    let samples = data.chunks_exact(sample_bytes).map(S::read_le).collect();
    Ok((header, samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<S: WavSample + PartialEq + std::fmt::Debug>(samples: &[S], channels: u16) {
        let path = std::env::temp_dir().join(format!("starplayer-wav-test-{:?}-{}.wav", std::thread::current().id(), channels));
        write_wav(&path, 44_100, channels, samples).expect("the file writes");
        let (header, read_back) = read_wav::<S>(&path).expect("the file reads back");
        assert_eq!(header.sample_rate_hz, 44_100);
        assert_eq!(header.channels, channels);
        assert_eq!(header.bits_per_sample, S::BITS_PER_SAMPLE);
        assert_eq!(header.format_tag, S::FORMAT_TAG);
        assert_eq!(read_back, samples);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn i16_round_trips() { round_trip(&[0i16, 1, -1, i16::MIN, i16::MAX, 12_345], 1); }

    #[test]
    fn i24_round_trips() { round_trip(&[I24(0), I24(1), I24(-1), I24::MIN, I24::MAX, I24(-70_000)], 2); }

    #[test]
    fn i32_round_trips() { round_trip(&[0i32, 1, -1, i32::MIN, i32::MAX, 123_456_789], 1); }

    #[test]
    fn f32_round_trips() { round_trip(&[0.0f32, 1.0, -1.0, 0.5, -0.5, 0.123_456], 2); }

    #[test]
    fn an_odd_sample_count_is_word_aligned_and_still_round_trips() {
        // Three mono I24 samples is nine payload bytes: odd, so the writer's pad byte and
        // the reader's alignment skip both have to agree.
        round_trip(&[I24(1), I24(2), I24(3)], 1);
    }

    #[test]
    fn the_header_reports_the_correct_chunk_sizes() {
        let header = WavHeader::for_format::<i16>(44_100, 2);
        let (riff_size, data_bytes) = header.chunk_sizes(4).expect("four samples fit easily");
        assert_eq!(data_bytes, 8, "four i16 samples is eight bytes");
        assert_eq!(riff_size, 4 + (8 + 16) + (8 + 8), "RIFF size excludes only its own tag and size field");
    }

    #[test]
    fn a_payload_over_four_gibibytes_is_rejected_rather_than_wrapped() {
        let header = WavHeader::for_format::<i32>(44_100, 2);
        // One sample short of wrapping u32::MAX at four bytes each.
        let huge_sample_count = (u32::MAX as usize / 4) + 1;
        assert!(matches!(header.chunk_sizes(huge_sample_count), Err(WavError::TooLarge)));
    }

    #[test]
    fn reading_a_file_with_the_wrong_sample_type_is_reported_rather_than_misread() {
        let path = std::env::temp_dir().join("starplayer-wav-test-wrong-type.wav");
        write_wav(&path, 44_100, 1, &[1i16, 2, 3]).expect("the file writes");
        assert!(matches!(read_wav::<f32>(&path), Err(WavError::Malformed(_))));
        let _ = std::fs::remove_file(&path);
    }
}
