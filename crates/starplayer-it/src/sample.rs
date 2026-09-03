//! The 80-byte `IMPS` sample header, and the PCM decoding that turns a sample's bytes into
//! the `i16` frames [`ModuleBuilder::add_sample`](starplayer_model::ModuleBuilder) wants.

use alloc::vec::Vec;
use starplayer_core::Error;
use starplayer_model::AutoVibratoWaveform;

/// Bytes in one IT sample header.
pub const HEADER_LENGTH: usize = 80;

/// The signature at offset `0x00` of a sample header.
pub const MAGIC: [u8; 4] = *b"IMPS";

/// `Flg` bit 0 — a sample is associated with this header.
pub const FLAG_HAS_DATA: u8 = 1 << 0;
/// `Flg` bit 1 — 16-bit rather than 8-bit.
pub const FLAG_SIXTEEN_BIT: u8 = 1 << 1;
/// `Flg` bit 2 — stereo. The right channel follows the left as a second block.
pub const FLAG_STEREO: u8 = 1 << 2;
/// `Flg` bit 3 — IT 2.14 compressed.
pub const FLAG_COMPRESSED: u8 = 1 << 3;
/// `Flg` bit 4 — use the loop.
pub const FLAG_LOOP: u8 = 1 << 4;
/// `Flg` bit 5 — use the sustain loop.
pub const FLAG_SUSTAIN_LOOP: u8 = 1 << 5;
/// `Flg` bit 6 — the loop is ping-pong rather than forward.
pub const FLAG_PING_PONG_LOOP: u8 = 1 << 6;
/// `Flg` bit 7 — the sustain loop is ping-pong rather than forward.
pub const FLAG_PING_PONG_SUSTAIN: u8 = 1 << 7;

/// `Cvt` bit 0 — sample data is signed. IT 2.02 and above write this; IT 2.01 and below
/// wrote unsigned data with the bit clear.
pub const CONVERT_SIGNED: u8 = 1 << 0;
/// `Cvt` bit 1 — 16-bit data is big-endian. ITTECH.TXT calls this safe to ignore; IT does
/// not ignore it, so neither does this loader.
pub const CONVERT_BIG_ENDIAN: u8 = 1 << 1;
/// `Cvt` bit 2 — delta values.
///
/// With [`FLAG_COMPRESSED`] this selects the **IT 2.15** double-delta compression variant;
/// without it, it means ordinary delta-coded PCM.
pub const CONVERT_DELTA: u8 = 1 << 2;

/// `DfP` bit 7 — the sample's default pan is enabled. The low seven bits are the position.
pub const PAN_ENABLED: u8 = 0x80;

/// The `Cwt/v` at or above which a sample's stereo flag is believed.
///
/// Some old Impulse Tracker versions failed to clear the flag when importing a sample, and
/// every tracker that really writes stereo samples identifies as IT 2.14 or later
/// (`ITSample::GetSampleFormat`).
pub const STEREO_FLAG_MINIMUM_TRACKER_VERSION: u16 = 0x0214;

/// Reference rate a sample with a zero `C5Speed` is given: the format default.
pub const FALLBACK_C5_SPEED: u32 = starplayer_model::DEFAULT_REFERENCE_RATE_HZ;

/// Lowest reference rate accepted, matching `ITSample::ConvertToMPT`'s own floor.
pub const MINIMUM_C5_SPEED: u32 = 256;

/// The 80-byte IT sample header, field for field. Raw: nothing here is clamped.
///
/// `name` is not here — it is a string and this type is `Copy`; the loader reads it out of
/// bytes `0x14..0x2E` itself.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItSampleHeader {
    /// Whether `IMPS` was present at offset zero.
    ///
    /// Not fatal, and deliberately not checked by the loader: Impulse Tracker does not
    /// check it either, and there is a bad XM-to-IT converter that leaves it off empty
    /// sample slots (`ITSample::ConvertToMPT`).
    pub has_magic: bool,
    /// `GvL` (`0x11`), the sample's global volume, 0..=64.
    pub global_volume: u8,
    /// `Flg` (`0x12`).
    pub flags: u8,
    /// `Vol` (`0x13`), the default volume a note starts the channel at, 0..=64.
    pub volume: u8,
    /// `Cvt` (`0x2E`).
    pub convert: u8,
    /// `DfP` (`0x2F`), the default pan: bits 0..=6 the position, bit 7 to use it.
    pub default_pan: u8,
    /// `Length` (`0x30`), in frames.
    pub length: u32,
    /// `Loop Begin` (`0x34`), in frames.
    pub loop_start: u32,
    /// `Loop End` (`0x38`), one past the last frame of the loop.
    pub loop_end: u32,
    /// `C5Speed` (`0x3C`), the rate at which the sample sounds C-5.
    pub c5_speed: u32,
    /// `SusLoop Begin` (`0x40`), in frames.
    pub sustain_start: u32,
    /// `SusLoop End` (`0x44`), one past the last frame of the sustain loop.
    pub sustain_end: u32,
    /// `SamplePointer` (`0x48`), a byte offset into the file.
    pub data_offset: u32,
    /// `ViS` (`0x4C`) — auto-vibrato **speed** in ITTECH.TXT's naming, which OpenMPT calls
    /// the rate.
    pub vibrato_speed: u8,
    /// `ViD` (`0x4D`), auto-vibrato depth.
    pub vibrato_depth: u8,
    /// `ViR` (`0x4E`) — auto-vibrato **rate** in ITTECH.TXT's naming, which OpenMPT calls
    /// the sweep: how quickly the depth ramps in.
    pub vibrato_sweep: u8,
    /// `ViT` (`0x4F`), the auto-vibrato waveform: 0 sine, 1 ramp down, 2 square, 3 random.
    pub vibrato_waveform: u8,
}

impl ItSampleHeader {
    /// Parse an 80-byte sample header.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if fewer than [`HEADER_LENGTH`] bytes were supplied.
    pub fn parse(bytes: &[u8]) -> Result<ItSampleHeader, Error> {
        let bytes = bytes.get(..HEADER_LENGTH).ok_or(Error::Truncated { offset: 0, needed: HEADER_LENGTH })?;
        Ok(ItSampleHeader {
            has_magic: bytes.get(..4) == Some(&MAGIC[..]),
            global_volume: read_u8(bytes, 0x11)?,
            flags: read_u8(bytes, 0x12)?,
            volume: read_u8(bytes, 0x13)?,
            convert: read_u8(bytes, 0x2E)?,
            default_pan: read_u8(bytes, 0x2F)?,
            length: read_u32(bytes, 0x30)?,
            loop_start: read_u32(bytes, 0x34)?,
            loop_end: read_u32(bytes, 0x38)?,
            c5_speed: read_u32(bytes, 0x3C)?,
            sustain_start: read_u32(bytes, 0x40)?,
            sustain_end: read_u32(bytes, 0x44)?,
            data_offset: read_u32(bytes, 0x48)?,
            vibrato_speed: read_u8(bytes, 0x4C)?,
            vibrato_depth: read_u8(bytes, 0x4D)?,
            vibrato_sweep: read_u8(bytes, 0x4E)?,
            vibrato_waveform: read_u8(bytes, 0x4F)?,
        })
    }

    /// Whether the header claims to have sample data behind it.
    pub const fn has_data(&self) -> bool { self.flags & FLAG_HAS_DATA != 0 }

    /// Whether the stored data is 16-bit.
    pub const fn is_sixteen_bit(&self) -> bool { self.flags & FLAG_SIXTEEN_BIT != 0 }

    /// Whether the stored data is compressed.
    pub const fn is_compressed(&self) -> bool { self.flags & FLAG_COMPRESSED != 0 }

    /// Whether the sample loops.
    pub const fn loops(&self) -> bool { self.flags & FLAG_LOOP != 0 }

    /// Whether the sample has a sustain loop.
    pub const fn sustain_loops(&self) -> bool { self.flags & FLAG_SUSTAIN_LOOP != 0 }

    /// Whether the ordinary loop is ping-pong.
    pub const fn loop_is_ping_pong(&self) -> bool { self.flags & FLAG_PING_PONG_LOOP != 0 }

    /// Whether the sustain loop is ping-pong.
    pub const fn sustain_is_ping_pong(&self) -> bool { self.flags & FLAG_PING_PONG_SUSTAIN != 0 }

    /// Whether the stored data has a second channel, given the file's `Cwt/v`.
    ///
    /// See [`STEREO_FLAG_MINIMUM_TRACKER_VERSION`] for why the version matters.
    pub const fn is_stereo(&self, tracker_version: u16) -> bool {
        self.flags & FLAG_STEREO != 0 && tracker_version >= STEREO_FLAG_MINIMUM_TRACKER_VERSION
    }

    /// Channels the stored data holds: two for a stereo sample, one otherwise.
    pub const fn stored_channels(&self, tracker_version: u16) -> usize {
        match self.is_stereo(tracker_version) {
            true => 2,
            false => 1,
        }
    }

    /// Whether uncompressed 8-bit or 16-bit data is signed.
    pub const fn is_signed(&self) -> bool { self.convert & CONVERT_SIGNED != 0 }

    /// Whether uncompressed 16-bit data is big-endian.
    pub const fn is_big_endian(&self) -> bool { self.convert & CONVERT_BIG_ENDIAN != 0 }

    /// Whether uncompressed data is delta-coded, or — with [`ItSampleHeader::is_compressed`]
    /// — whether the IT 2.15 compression variant applies.
    pub const fn is_delta(&self) -> bool { self.convert & CONVERT_DELTA != 0 }

    /// Bytes one *stored* frame occupies, both channels of a stereo sample included.
    /// Meaningless for a compressed sample, whose frames are not byte-aligned.
    pub const fn bytes_per_frame(&self, tracker_version: u16) -> usize {
        let width = if self.is_sixteen_bit() { 2 } else { 1 };
        width * self.stored_channels(tracker_version)
    }

    /// The sample's reference rate, with the format's fallback and floor applied.
    pub const fn reference_rate_hz(&self) -> u32 {
        match self.c5_speed {
            0 => FALLBACK_C5_SPEED,
            rate if rate < MINIMUM_C5_SPEED => MINIMUM_C5_SPEED,
            rate => rate,
        }
    }

    /// The sample's default pan, `None` when the "use it" bit is clear.
    pub const fn pan_position(&self) -> Option<u8> {
        match self.default_pan & PAN_ENABLED != 0 {
            true => Some(self.default_pan & 0x7F),
            false => None,
        }
    }

    /// The auto-vibrato waveform, from `ViT`'s low two bits.
    pub const fn auto_vibrato_waveform(&self) -> AutoVibratoWaveform {
        match self.vibrato_waveform & 3 {
            0 => AutoVibratoWaveform::Sine,
            1 => AutoVibratoWaveform::RampDown,
            2 => AutoVibratoWaveform::Square,
            _ => AutoVibratoWaveform::Random,
        }
    }
}

/// Widen one **unsigned** 8-bit IT sample byte to the `i16` the mixer plays.
///
/// The same arithmetic as S3M's, and for the same reason: exclusive-or moves the origin
/// from 128 to 0, the `i8` cast reinterprets, and the multiply widens with the low byte
/// zero. `0x00` becomes `-32768`, `0x80` becomes `0`, `0xFF` becomes `32512`.
pub const fn unsigned8_to_i16(byte: u8) -> i16 { ((byte ^ 0x80) as i8 as i16) * 256 }

/// Widen one **signed** 8-bit sample byte.
pub const fn signed8_to_i16(byte: u8) -> i16 { (byte as i8 as i16) * 256 }

/// Decode `frames` frames of one channel of an uncompressed sample.
///
/// `raw` is that channel's own block: a stereo sample stores its right channel as a second
/// block after the left, so the caller slices before calling. Reading past the end of
/// `raw` yields silence rather than panicking, which is what makes a sample that runs off
/// the end of the file a shorter sample.
/// Delta coding replaces the signed/unsigned choice rather than composing with it —
/// OpenMPT's `SampleIO` keeps one *encoding* field, and `sampleIO |= SampleIO::deltaPCM`
/// overwrites whatever `cvtSignedSample` put there — so a delta-coded sample's deltas are
/// always signed and its running sum wraps at the sample's own width.
pub fn decode_frames(raw: &[u8], frames: usize, header: &ItSampleHeader) -> Vec<i16> {
    let mut pcm = Vec::with_capacity(frames);
    let signed = header.is_signed();
    let delta = header.is_delta();
    if header.is_sixteen_bit() {
        let mut accumulator = 0i16;
        for frame in 0..frames {
            let low = raw.get(frame * 2).copied().unwrap_or(0);
            let high = raw.get(frame * 2 + 1).copied().unwrap_or(0);
            let bytes = match header.is_big_endian() {
                true => [high, low],
                false => [low, high],
            };
            let value = u16::from_le_bytes(bytes);
            pcm.push(match (delta, signed) {
                (true, _) => {
                    accumulator = accumulator.wrapping_add(value as i16);
                    accumulator
                }
                (false, true) => value as i16,
                (false, false) => (value ^ 0x8000) as i16,
            });
        }
    } else {
        let mut accumulator = 0i8;
        for frame in 0..frames {
            let silence = if signed || delta { 0 } else { 0x80 };
            let byte = raw.get(frame).copied().unwrap_or(silence);
            pcm.push(match (delta, signed) {
                (true, _) => {
                    accumulator = accumulator.wrapping_add(byte as i8);
                    (accumulator as i16) * 256
                }
                (false, true) => signed8_to_i16(byte),
                (false, false) => unsigned8_to_i16(byte),
            });
        }
    }
    pcm
}

/// Average a stereo sample's two channels into the mono frames the mixer plays.
///
/// Accuracy policy **D60**: this engine's voices are mono-source, so an IT stereo sample —
/// OpenMPT's extension, which Impulse Tracker itself never played — is downmixed at load
/// time rather than dropped or split. The sum is halved with a rounding-free integer
/// divide, which cannot clip: two `i16`s average back into `i16`.
pub fn downmix(left: &[i16], right: &[i16]) -> Vec<i16> {
    let frames = core::cmp::max(left.len(), right.len());
    let mut pcm = Vec::with_capacity(frames);
    for frame in 0..frames {
        let left_frame = left.get(frame).copied().unwrap_or(0) as i32;
        let right_frame = right.get(frame).copied().unwrap_or(0) as i32;
        pcm.push(((left_frame + right_frame) / 2) as i16);
    }
    pcm
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, Error> {
    bytes.get(offset).copied().ok_or(Error::Truncated { offset, needed: 1 })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    match bytes.get(offset..offset + 4) {
        Some([a, b, c, d]) => Ok(u32::from_le_bytes([*a, *b, *c, *d])),
        _ => Err(Error::Truncated { offset, needed: 4 }),
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use alloc::vec;

    fn synthetic_sample_header() -> [u8; HEADER_LENGTH] {
        let mut bytes = [0u8; HEADER_LENGTH];
        bytes[..4].copy_from_slice(&MAGIC);
        bytes[0x11] = 64;
        bytes[0x12] = FLAG_HAS_DATA | FLAG_LOOP | FLAG_SUSTAIN_LOOP | FLAG_PING_PONG_SUSTAIN;
        bytes[0x13] = 48;
        bytes[0x2E] = CONVERT_SIGNED;
        bytes[0x2F] = PAN_ENABLED | 16;
        bytes[0x30..0x34].copy_from_slice(&1000u32.to_le_bytes());
        bytes[0x34..0x38].copy_from_slice(&100u32.to_le_bytes());
        bytes[0x38..0x3C].copy_from_slice(&900u32.to_le_bytes());
        bytes[0x3C..0x40].copy_from_slice(&22050u32.to_le_bytes());
        bytes[0x40..0x44].copy_from_slice(&200u32.to_le_bytes());
        bytes[0x44..0x48].copy_from_slice(&400u32.to_le_bytes());
        bytes[0x48..0x4C].copy_from_slice(&0x1234u32.to_le_bytes());
        bytes[0x4C] = 8;
        bytes[0x4D] = 32;
        bytes[0x4E] = 4;
        bytes[0x4F] = 2;
        bytes
    }

    #[test]
    fn a_sample_header_reads_every_field_the_loader_uses() {
        let header = ItSampleHeader::parse(&synthetic_sample_header()).expect("a valid header");

        assert!(header.has_magic && header.has_data() && header.loops() && header.sustain_loops());
        assert!(header.sustain_is_ping_pong() && !header.loop_is_ping_pong());
        assert!(!header.is_sixteen_bit() && !header.is_compressed());
        assert_eq!(header.global_volume, 64);
        assert_eq!(header.volume, 48);
        assert_eq!(header.length, 1000);
        assert_eq!(header.loop_start, 100);
        assert_eq!(header.loop_end, 900);
        assert_eq!(header.sustain_start, 200);
        assert_eq!(header.sustain_end, 400);
        assert_eq!(header.data_offset, 0x1234);
        assert_eq!(header.reference_rate_hz(), 22050);
        assert_eq!(header.pan_position(), Some(16));
        assert_eq!(header.auto_vibrato_waveform(), AutoVibratoWaveform::Square);
        assert_eq!(header.vibrato_speed, 8);
        assert_eq!(header.vibrato_depth, 32);
        assert_eq!(header.vibrato_sweep, 4);
        assert_eq!(header.bytes_per_frame(0x0214), 1);
    }

    #[test]
    fn a_short_header_is_truncated_not_a_panic() {
        assert_eq!(ItSampleHeader::parse(&[0u8; HEADER_LENGTH - 1]), Err(Error::Truncated { offset: 0, needed: HEADER_LENGTH }));
    }

    #[test]
    fn a_stereo_flag_is_only_believed_from_it_two_fourteen_onwards() {
        let mut bytes = synthetic_sample_header();
        bytes[0x12] |= FLAG_STEREO;
        let header = ItSampleHeader::parse(&bytes).expect("a valid header");

        assert!(header.is_stereo(0x0214));
        assert_eq!(header.stored_channels(0x0214), 2);
        assert!(!header.is_stereo(0x0213), "an old IT set the flag by accident on import");
        assert_eq!(header.stored_channels(0x0213), 1);
        assert_eq!(header.bytes_per_frame(0x0214), 2);
    }

    #[test]
    fn a_zero_or_tiny_reference_rate_takes_the_formats_fallback_and_floor() {
        let mut bytes = synthetic_sample_header();
        bytes[0x3C..0x40].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(ItSampleHeader::parse(&bytes).expect("valid").reference_rate_hz(), FALLBACK_C5_SPEED);

        bytes[0x3C..0x40].copy_from_slice(&100u32.to_le_bytes());
        assert_eq!(ItSampleHeader::parse(&bytes).expect("valid").reference_rate_hz(), MINIMUM_C5_SPEED);
    }

    #[test]
    fn eight_bit_data_is_read_signed_or_unsigned_as_the_convert_byte_says() {
        let mut bytes = synthetic_sample_header();
        let signed = ItSampleHeader::parse(&bytes).expect("valid");
        assert_eq!(decode_frames(&[0x00, 0x7F, 0x80, 0xFF], 4, &signed), [0, 32512, i16::MIN, -256]);

        bytes[0x2E] = 0;
        let unsigned = ItSampleHeader::parse(&bytes).expect("valid");
        assert_eq!(decode_frames(&[0x00, 0x7F, 0x80, 0xFF], 4, &unsigned), [i16::MIN, -256, 0, 32512]);
    }

    #[test]
    fn sixteen_bit_data_honours_the_signed_and_endianness_bits() {
        let mut bytes = synthetic_sample_header();
        bytes[0x12] |= FLAG_SIXTEEN_BIT;
        let signed = ItSampleHeader::parse(&bytes).expect("valid");
        assert_eq!(decode_frames(&[0x00, 0x80, 0xFF, 0x7F, 0x00, 0x00], 3, &signed), [i16::MIN, i16::MAX, 0]);

        bytes[0x2E] = CONVERT_SIGNED | CONVERT_BIG_ENDIAN;
        let big_endian = ItSampleHeader::parse(&bytes).expect("valid");
        assert_eq!(decode_frames(&[0x80, 0x00, 0x7F, 0xFF], 2, &big_endian), [i16::MIN, i16::MAX]);

        bytes[0x2E] = 0;
        let unsigned = ItSampleHeader::parse(&bytes).expect("valid");
        assert_eq!(decode_frames(&[0x00, 0x00, 0x00, 0x80], 2, &unsigned), [i16::MIN, 0]);
    }

    #[test]
    fn delta_coded_data_integrates_in_its_own_width() {
        let mut bytes = synthetic_sample_header();
        bytes[0x2E] = CONVERT_SIGNED | CONVERT_DELTA;
        let narrow = ItSampleHeader::parse(&bytes).expect("valid");
        // 1, 1, -2 accumulates to 1, 2, 0 — the same shape the decompressor produces.
        assert_eq!(decode_frames(&[1, 1, 0xFE], 3, &narrow), [256, 512, 0]);
        // 127 + 1 wraps to -128 rather than saturating, which is what IT's byte-wide
        // accumulator does.
        assert_eq!(decode_frames(&[0x7F, 0x01], 2, &narrow), [32512, i16::MIN]);

        bytes[0x12] |= FLAG_SIXTEEN_BIT;
        let wide = ItSampleHeader::parse(&bytes).expect("valid");
        assert_eq!(decode_frames(&[0x01, 0x00, 0xFD, 0xFF, 0x02, 0x00], 3, &wide), [1, -2, 0]);
    }

    #[test]
    fn decoding_past_the_end_of_the_data_yields_silence_rather_than_a_panic() {
        let mut bytes = synthetic_sample_header();
        bytes[0x2E] = 0;
        let unsigned = ItSampleHeader::parse(&bytes).expect("valid");
        assert_eq!(decode_frames(&[], 3, &unsigned), [0, 0, 0], "unsigned silence is 0x80");

        bytes[0x2E] = CONVERT_SIGNED;
        bytes[0x12] |= FLAG_SIXTEEN_BIT;
        let wide = ItSampleHeader::parse(&bytes).expect("valid");
        assert_eq!(decode_frames(&[], 2, &wide), [0, 0]);
    }

    #[test]
    fn a_stereo_samples_channels_average_into_one() {
        assert_eq!(downmix(&[100, -100, i16::MAX], &[200, 100, i16::MAX]), vec![150, 0, i16::MAX]);
        assert_eq!(downmix(&[i16::MIN, i16::MIN], &[i16::MIN, i16::MIN]), vec![i16::MIN, i16::MIN], "the average of two extremes cannot clip");
        assert_eq!(downmix(&[100, 200], &[]), vec![50, 100], "a missing right channel averages against silence");
        assert_eq!(downmix(&[], &[]), Vec::<i16>::new());
    }
}
