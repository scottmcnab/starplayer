//! The 80-byte S3M sample header, and the PCM decoding that turns a sample's bytes into
//! the `i16` frames [`ModuleBuilder::add_sample`](starplayer_model::ModuleBuilder) wants.

use alloc::vec::Vec;
use starplayer_core::Error;

/// Bytes in one S3M sample header.
pub const HEADER_LENGTH: usize = 80;

/// The signature at offset `0x4C` of a sample header.
pub const MAGIC: [u8; 4] = *b"SCRS";

/// Sample header `type` for an ordinary PCM sample. `0` is an empty slot; `2`..=`7` are
/// Adlib instruments, which this engine has no synthesiser for.
pub const TYPE_PCM: u8 = 1;

/// Sample header `type` for an empty slot — a message-only instrument, of which the
/// owner's `REFLEX.S3M` has three.
pub const TYPE_EMPTY: u8 = 0;

/// `flags` bit 0: the sample loops.
pub const FLAG_LOOP: u8 = 1 << 0;

/// `flags` bit 1: the sample is stereo — the right channel's frames follow the left
/// channel's, as a second block of the same length.
pub const FLAG_STEREO: u8 = 1 << 1;

/// `flags` bit 2: the sample's frames are signed 16-bit little-endian rather than
/// unsigned 8-bit.
pub const FLAG_SIXTEEN_BIT: u8 = 1 << 2;

/// The 80-byte S3M sample header, field for field.
///
/// `name` is not here — it is a string and this type is `Copy`; the loader reads it out of
/// bytes `0x30..0x4C` itself.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct S3mSampleHeader {
    /// `type` (`0x00`): [`TYPE_PCM`], [`TYPE_EMPTY`], or an Adlib instrument.
    pub kind: u8,
    /// The 24-bit `memseg` parapointer (`0x0D` high byte, `0x0E` low word), already
    /// multiplied by 16, so this is a byte offset into the file.
    ///
    /// The original read only the low word (`S3MLIB.ASM:5740`,
    /// `movzx edx,word ptr [esi+0eh]`), capping sample data at the first megabyte of the
    /// file. The high byte is part of the format, costs nothing, and is read here; every
    /// file in the owner's collection has it zero, so no module the original could play
    /// loads differently. This is deviation **D8** in
    /// `plans/product/03-accuracy-policy.md`.
    pub data_offset: usize,
    /// `length` (`0x10`) in **frames**.
    pub length: u32,
    /// `loopbeg` (`0x14`) in frames.
    pub loop_start: u32,
    /// `loopend` (`0x18`) in frames, one past the last frame of the loop.
    pub loop_end: u32,
    /// `vol` (`0x1C`), 0..=64.
    pub volume: u8,
    /// `pack` (`0x1E`): 0 for raw PCM. Anything else is a packing scheme Scream Tracker 3
    /// never wrote and this loader rejects.
    pub packing: u8,
    /// `flags` (`0x1F`): [`FLAG_LOOP`], [`FLAG_STEREO`], [`FLAG_SIXTEEN_BIT`].
    pub flags: u8,
    /// `C2Spd` (`0x20`), the rate at which the sample sounds C-4.
    ///
    /// **The full 32 bits.** The original read only the low word, which is deviation
    /// **D7** in `plans/product/03-accuracy-policy.md`; see [`S3mSampleHeader::parse`].
    pub c2spd: u32,
    /// Whether `SCRS` was present at `0x4C`. Not fatal — the loader treats a slot without
    /// it as empty rather than rejecting the file.
    pub has_magic: bool,
}

impl S3mSampleHeader {
    /// Parse an 80-byte sample header.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if fewer than [`HEADER_LENGTH`] bytes were supplied.
    pub fn parse(bytes: &[u8]) -> Result<S3mSampleHeader, Error> {
        let bytes = bytes.get(..HEADER_LENGTH).ok_or(Error::Truncated { offset: 0, needed: HEADER_LENGTH })?;

        let parapointer = (read_u8(bytes, 0x0D)? as usize) << 16 | read_u16(bytes, 0x0E)? as usize;

        Ok(S3mSampleHeader {
            kind: read_u8(bytes, 0x00)?,
            data_offset: parapointer * 16,
            length: read_u32(bytes, 0x10)?,
            loop_start: read_u32(bytes, 0x14)?,
            loop_end: read_u32(bytes, 0x18)?,
            volume: read_u8(bytes, 0x1C)?,
            packing: read_u8(bytes, 0x1E)?,
            flags: read_u8(bytes, 0x1F)?,
            // Accuracy policy D7: the original's `__UpdateTracker` read only the low 16
            // bits of this field, so any sample above 65535 Hz played at the wrong pitch.
            // The full 32 bits are read here and carried to `SampleSpec::reference_rate_hz`
            // at full width.
            c2spd: read_u32(bytes, 0x20)?,
            has_magic: bytes.get(0x4C..0x50) == Some(&MAGIC[..]),
        })
    }

    /// Whether this slot holds playable PCM.
    pub const fn is_pcm(&self) -> bool { self.kind == TYPE_PCM }

    /// Whether this slot holds an Adlib instrument (`type` 2..=7), which this engine
    /// cannot sound.
    pub const fn is_adlib(&self) -> bool { self.kind > TYPE_PCM }

    /// Whether the sample loops.
    pub const fn loops(&self) -> bool { self.flags & FLAG_LOOP != 0 }

    /// Bytes one frame of this sample's *stored* data occupies, both channels of a stereo
    /// sample included.
    pub const fn bytes_per_frame(&self) -> usize {
        let width = if self.flags & FLAG_SIXTEEN_BIT != 0 { 2 } else { 1 };
        let channels = if self.flags & FLAG_STEREO != 0 { 2 } else { 1 };
        width * channels
    }
}

/// Widen one **unsigned** 8-bit S3M sample byte to the `i16` the mixer plays.
///
/// S3M sample data is unsigned — silence is `0x80`
/// (`plans/reference/original-s3mlib-analysis.md` §5) — so the conversion is
/// `(byte ^ 0x80) as i8 as i16 * 256`: exclusive-or moves the origin from 128 to 0, the
/// `i8` cast reinterprets the result as signed, and the multiply widens 8 bits to 16 with
/// the low byte zero.
///
/// The endpoints, which the tests pin: `0x00` → `-32768` (full negative), `0x80` → `0`,
/// `0xFF` → `32512`. The positive end stops one 8-bit step short of `32767` because the
/// source has 255 codes, not 256; scaling to reach exactly full scale would need a
/// multiply-and-round and would stop the conversion being exact and reversible.
pub const fn unsigned8_to_i16(byte: u8) -> i16 { ((byte ^ 0x80) as i8 as i16) * 256 }

/// The `ffi` header byte (`0x2A`) value meaning "sample data is signed".
///
/// Recorded for completeness and **not acted on**: Scream Tracker 3 always writes `2`
/// (unsigned) — every file in the owner's 1994–96 collection does — and the original DOS
/// player never looked at the field. Honouring it would change how a file loads on the
/// strength of a byte no reference implementation of this engine's specification reads,
/// so the loader treats 8-bit data as unsigned unconditionally, exactly as
/// `plans/reference/original-s3mlib-analysis.md` §5 states. If a file that genuinely
/// carries signed 8-bit data ever turns up, that is an accuracy-policy decision, not a
/// loader bug fix.
pub const FILE_FORMAT_SIGNED: u16 = 1;

/// Decode `frames` frames of a sample's raw bytes into `i16`.
///
/// A stereo sample (a second block of the same length after the first) contributes its
/// **left** channel only: the mixer is mono-source and the pan comes from the channel, so
/// the right block is skipped. Scream Tracker 3 never wrote one.
pub fn decode_frames(raw: &[u8], frames: usize, flags: u8) -> Vec<i16> {
    let mut pcm = Vec::with_capacity(frames);
    if flags & FLAG_SIXTEEN_BIT != 0 {
        for frame in 0..frames {
            let low = raw.get(frame * 2).copied().unwrap_or(0);
            let high = raw.get(frame * 2 + 1).copied().unwrap_or(0);
            pcm.push(i16::from_le_bytes([low, high]));
        }
    } else {
        for frame in 0..frames {
            pcm.push(unsigned8_to_i16(raw.get(frame).copied().unwrap_or(0x80)));
        }
    }
    pcm
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, Error> {
    bytes.get(offset).copied().ok_or(Error::Truncated { offset, needed: 1 })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    match bytes.get(offset..offset + 2) {
        Some([low, high]) => Ok(u16::from_le_bytes([*low, *high])),
        _ => Err(Error::Truncated { offset, needed: 2 }),
    }
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

    fn synthetic_sample_header() -> [u8; HEADER_LENGTH] {
        let mut bytes = [0u8; HEADER_LENGTH];
        bytes[0x00] = TYPE_PCM;
        bytes[0x0D] = 0x01; // high byte of the 24-bit parapointer
        bytes[0x0E..0x10].copy_from_slice(&0x0002u16.to_le_bytes());
        bytes[0x10..0x14].copy_from_slice(&1000u32.to_le_bytes());
        bytes[0x14..0x18].copy_from_slice(&100u32.to_le_bytes());
        bytes[0x18..0x1C].copy_from_slice(&900u32.to_le_bytes());
        bytes[0x1C] = 48;
        bytes[0x1F] = FLAG_LOOP;
        bytes[0x20..0x24].copy_from_slice(&70_000u32.to_le_bytes());
        bytes[0x4C..0x50].copy_from_slice(&MAGIC);
        bytes
    }

    #[test]
    fn a_sample_header_reads_every_field_the_loader_uses() {
        let header = S3mSampleHeader::parse(&synthetic_sample_header()).expect("a valid header");

        assert_eq!(header.kind, TYPE_PCM);
        assert_eq!(header.data_offset, 0x10002 * 16, "the 24-bit parapointer, times sixteen");
        assert_eq!(header.length, 1000);
        assert_eq!(header.loop_start, 100);
        assert_eq!(header.loop_end, 900);
        assert_eq!(header.volume, 48);
        assert!(header.loops());
        assert!(header.has_magic);
        assert_eq!(header.bytes_per_frame(), 1);
    }

    #[test]
    fn the_full_thirty_two_bit_c2spd_is_read_which_is_deviation_d7() {
        let header = S3mSampleHeader::parse(&synthetic_sample_header()).expect("a valid header");
        assert_eq!(header.c2spd, 70_000, "the original would have read 4464, the low 16 bits");
        assert_eq!(header.c2spd as u16, 4464, "which is what the low word alone says");
    }

    #[test]
    fn a_short_header_is_truncated_not_a_panic() {
        assert_eq!(S3mSampleHeader::parse(&[0u8; 79]), Err(Error::Truncated { offset: 0, needed: HEADER_LENGTH }));
    }

    #[test]
    fn unsigned_eight_bit_data_maps_zero_to_the_minimum_and_half_scale_to_silence() {
        assert_eq!(unsigned8_to_i16(0x00), i16::MIN);
        assert_eq!(unsigned8_to_i16(0x80), 0);
        assert_eq!(unsigned8_to_i16(0xFF), 32512);
        assert_eq!(unsigned8_to_i16(0x81), 256);
        assert_eq!(unsigned8_to_i16(0x7F), -256);
        assert!(unsigned8_to_i16(0xFF) > i16::MAX - 256, "0xFF is within one 8-bit step of full scale");
    }

    #[test]
    fn sixteen_bit_data_is_signed_little_endian() {
        let raw = [0x00, 0x80, 0xFF, 0x7F, 0x00, 0x00];
        assert_eq!(decode_frames(&raw, 3, FLAG_SIXTEEN_BIT), [i16::MIN, i16::MAX, 0]);
    }

    #[test]
    fn a_stereo_sample_contributes_its_left_channel_only() {
        // Four left frames, then four right frames; only the first four are decoded.
        let raw = [0x00, 0x40, 0xC0, 0xFF, 0x80, 0x80, 0x80, 0x80];
        assert_eq!(decode_frames(&raw, 4, FLAG_STEREO).len(), 4);
        assert_eq!(decode_frames(&raw, 4, FLAG_STEREO)[0], i16::MIN);
    }

    #[test]
    fn decoding_past_the_end_of_the_data_yields_silence_rather_than_a_panic() {
        assert_eq!(decode_frames(&[], 3, 0), [0, 0, 0]);
        assert_eq!(decode_frames(&[], 2, FLAG_SIXTEEN_BIT), [0, 0]);
    }
}
