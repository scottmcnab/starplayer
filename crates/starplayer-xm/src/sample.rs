//! The 40-byte XM sample header and the PCM decoding that turns a sample's bytes into the
//! `i16` frames [`ModuleBuilder::add_sample`](starplayer_model::ModuleBuilder) wants.
//!
//! # The three encodings a `.xm` sample can be in
//!
//! | Encoding | How it is spotted | Bytes per frame |
//! |---|---|---|
//! | 8-bit delta | the default | 1 |
//! | 16-bit delta | `flags` bit 4 | 2 |
//! | ModPlug 4-bit ADPCM | `reserved` == [`RESERVED_ADPCM`] and neither 16-bit nor stereo | ½, after a 16-byte table |
//!
//! Any of them may additionally be **stereo** (`flags` bit 5, a ModPlug extension): the
//! left channel's whole delta stream, then the right channel's, each decoded from its own
//! running accumulator. The engine's samples are mono, so the two are averaged — see
//! [`decode_frames`].

use alloc::vec::Vec;

/// Bytes in one XM sample header.
pub const HEADER_LENGTH: usize = 40;

/// Offset of the 22-byte sample name inside the header.
pub const NAME_OFFSET: usize = 18;

/// Length of the sample-name field.
pub const NAME_LENGTH: usize = 22;

/// `type` bit 0: the sample loops forwards.
pub const FLAG_FORWARD_LOOP: u8 = 1 << 0;

/// `type` bit 1: the sample loops back and forth.
pub const FLAG_PING_PONG_LOOP: u8 = 1 << 1;

/// `type` bits 0–1 together: the loop kind. `0` none, `1` forward, `2` ping-pong, and `3`
/// — which ModPlug up to 1.11 wrote for a plain forward loop — is read as ping-pong, the
/// way OpenMPT's `XMSample::ConvertToMPT` reads it.
pub const FLAG_LOOP_MASK: u8 = FLAG_FORWARD_LOOP | FLAG_PING_PONG_LOOP;

/// `type` bit 4: frames are signed 16-bit little-endian rather than signed 8-bit.
pub const FLAG_SIXTEEN_BIT: u8 = 1 << 4;

/// `type` bit 5: the sample is stereo — a ModPlug extension. The right channel's delta
/// stream follows the left channel's as a second block of the same length.
pub const FLAG_STEREO: u8 = 1 << 5;

/// `reserved` value ModPlug writes for a 4-bit ADPCM-compressed sample. It is the whole
/// of the detection: OpenMPT's `XMSample::GetSampleFormat` reads this byte and nothing
/// else, and the compressed data carries no magic of its own.
pub const RESERVED_ADPCM: u8 = 0xAD;

/// Bytes of compression table an ADPCM sample carries before its nybbles.
pub const ADPCM_TABLE_BYTES: usize = 16;

/// The 40-byte XM sample header, field for field.
///
/// `name` is not here — it is a string and this type is `Copy`; the loader reads it out of
/// bytes [`NAME_OFFSET`]`..`[`NAME_OFFSET`]`+`[`NAME_LENGTH`] itself.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct XmSampleHeader {
    /// `length` (`0x00`), in **bytes** of encoded data, not frames.
    pub length_bytes: u32,
    /// `loopStart` (`0x04`), in bytes.
    pub loop_start_bytes: u32,
    /// `loopLength` (`0x08`), in bytes.
    pub loop_length_bytes: u32,
    /// `vol` (`0x0C`), 0..=64.
    pub volume: u8,
    /// `finetune` (`0x0D`), signed, in 1/128 of a semitone.
    pub finetune: i8,
    /// `type` (`0x0E`): the loop kind in bits 0–1, [`FLAG_SIXTEEN_BIT`], [`FLAG_STEREO`].
    pub flags: u8,
    /// `pan` (`0x0F`), 0..=255 with 128 centred.
    pub pan: u8,
    /// `relnote` (`0x10`), the signed semitone offset added to the played note.
    pub relative_note: i8,
    /// `reserved` (`0x11`). [`RESERVED_ADPCM`] marks a ModPlug ADPCM sample; FastTracker 2
    /// writes the sample name's length here instead.
    pub reserved: u8,
}

impl XmSampleHeader {
    /// Parse a [`HEADER_LENGTH`]-byte sample header.
    ///
    /// A short slice is not an error: everything past its end reads as zero, which is a
    /// silent, non-looping, 8-bit sample. Early Sk@le Tracker writes a
    /// `sample_header_size` of `0` and `cybernostra weekend` writes `0x12`, and
    /// FastTracker 2 reads both, so a loader that rejected a short header would reject
    /// files the tracker plays.
    pub fn parse(bytes: &[u8]) -> XmSampleHeader {
        XmSampleHeader {
            length_bytes: read_u32(bytes, 0x00),
            loop_start_bytes: read_u32(bytes, 0x04),
            loop_length_bytes: read_u32(bytes, 0x08),
            volume: read_u8(bytes, 0x0C),
            finetune: read_u8(bytes, 0x0D) as i8,
            flags: read_u8(bytes, 0x0E),
            pan: read_u8(bytes, 0x0F),
            relative_note: read_u8(bytes, 0x10) as i8,
            reserved: read_u8(bytes, 0x11),
        }
    }

    /// Whether the data is ModPlug's 4-bit ADPCM. Reproduces
    /// `XMSample::GetSampleFormat`: the marker only counts on an 8-bit mono sample.
    pub const fn is_adpcm(&self) -> bool {
        self.reserved == RESERVED_ADPCM && self.flags & (FLAG_SIXTEEN_BIT | FLAG_STEREO) == 0
    }

    /// Whether frames are 16-bit.
    pub const fn is_sixteen_bit(&self) -> bool { self.flags & FLAG_SIXTEEN_BIT != 0 }

    /// Whether the sample carries two channels.
    pub const fn is_stereo(&self) -> bool { self.flags & FLAG_STEREO != 0 }

    /// How many encoded bytes one frame of one channel takes.
    pub const fn bytes_per_channel_frame(&self) -> usize {
        match self.is_sixteen_bit() {
            true => 2,
            false => 1,
        }
    }

    /// Channels interleaved as separate blocks: 2 for a stereo sample, 1 otherwise.
    pub const fn channels(&self) -> usize {
        match self.is_stereo() {
            true => 2,
            false => 1,
        }
    }

    /// Frames the header says the sample has.
    ///
    /// `length` is a **byte** count, so it is divided by the width of a frame — and for an
    /// ADPCM sample it is the frame count already, because two frames share one byte and
    /// OpenMPT treats `length` as `nLength` unchanged.
    pub const fn frames(&self) -> usize {
        match self.is_adpcm() {
            true => self.length_bytes as usize,
            false => self.length_bytes as usize / (self.bytes_per_channel_frame() * self.channels()),
        }
    }

    /// Bytes of file the encoded sample occupies, which is what the loader steps over to
    /// reach the next sample.
    pub const fn encoded_bytes(&self) -> usize {
        match self.is_adpcm() {
            // Two frames share a byte, so an odd frame count still costs a whole one.
            true => ADPCM_TABLE_BYTES + (self.length_bytes as usize).div_ceil(2),
            false => self.length_bytes as usize,
        }
    }

    /// Whether the loop flags say the sample repeats at all.
    pub const fn loops(&self) -> bool { self.flags & FLAG_LOOP_MASK != 0 }

    /// Whether the loop is a ping-pong one.
    pub const fn is_ping_pong(&self) -> bool { self.flags & FLAG_PING_PONG_LOOP != 0 }

    /// First frame of the loop, converted from the header's byte offset.
    pub const fn loop_start_frames(&self) -> usize {
        match self.is_adpcm() {
            true => self.loop_start_bytes as usize,
            false => self.loop_start_bytes as usize / (self.bytes_per_channel_frame() * self.channels()),
        }
    }

    /// One past the last frame of the loop, converted from the header's byte offsets.
    ///
    /// `loop_start + loop_length`, both converted, with the addition saturating rather
    /// than wrapping — the loader clamps the result to the sample's real length anyway.
    pub const fn loop_end_frames(&self) -> usize {
        let length = match self.is_adpcm() {
            true => self.loop_length_bytes as usize,
            false => self.loop_length_bytes as usize / (self.bytes_per_channel_frame() * self.channels()),
        };
        self.loop_start_frames().saturating_add(length)
    }
}

/// Decode `frames` frames of a sample's raw bytes into `i16`.
///
/// * **8-bit** data is a signed delta stream: each byte is added to a running `i8`
///   accumulator (wrapping, as FastTracker 2's own `add al,bl` does) and the accumulator
///   is widened by 256.
/// * **16-bit** data is the same with a running `i16` over little-endian pairs.
/// * **Stereo** data is two such streams, one after the other, each with its own
///   accumulator; the two are averaged into the mono frame the mixer plays, because the
///   engine's sample blob is mono and pan comes from the channel.
/// * **ADPCM** data is a 16-entry signed table followed by two 4-bit table indices per
///   byte, low nybble first, each index added to a running `i8`. This is ModPlug's
///   scheme, reproduced from OpenMPT's `SampleIO::ReadSample`.
///
/// `raw` shorter than the frames asked for is not an error: the missing bytes read as
/// zero, which continues the last value rather than jumping to silence.
pub fn decode_frames(raw: &[u8], frames: usize, header: &XmSampleHeader) -> Vec<i16> {
    if header.is_adpcm() {
        return decode_adpcm(raw, frames);
    }

    let stride = header.bytes_per_channel_frame();
    let left = decode_delta_channel(raw, 0, frames, stride, header.is_sixteen_bit());
    if !header.is_stereo() {
        return left;
    }

    let right = decode_delta_channel(raw, frames * stride, frames, stride, header.is_sixteen_bit());
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| ((*left as i32 + *right as i32) / 2) as i16)
        .collect()
}

/// One channel's delta stream, starting at `offset` bytes into `raw`.
fn decode_delta_channel(raw: &[u8], offset: usize, frames: usize, stride: usize, sixteen_bit: bool) -> Vec<i16> {
    let mut pcm = Vec::with_capacity(frames);
    if sixteen_bit {
        let mut accumulator: i16 = 0;
        for frame in 0..frames {
            let low = raw.get(offset + frame * stride).copied().unwrap_or(0);
            let high = raw.get(offset + frame * stride + 1).copied().unwrap_or(0);
            accumulator = accumulator.wrapping_add(i16::from_le_bytes([low, high]));
            pcm.push(accumulator);
        }
    } else {
        let mut accumulator: i8 = 0;
        for frame in 0..frames {
            let delta = raw.get(offset + frame * stride).copied().unwrap_or(0) as i8;
            accumulator = accumulator.wrapping_add(delta);
            pcm.push(accumulator as i16 * 256);
        }
    }
    pcm
}

/// ModPlug's 4-bit ADPCM: a 16-byte signed table, then two nybble indices per byte, low
/// nybble first, each added to a running `i8`.
fn decode_adpcm(raw: &[u8], frames: usize) -> Vec<i16> {
    let mut table = [0i8; ADPCM_TABLE_BYTES];
    for (index, entry) in table.iter_mut().enumerate() {
        *entry = raw.get(index).copied().unwrap_or(0) as i8;
    }

    let mut pcm = Vec::with_capacity(frames);
    let mut accumulator: i8 = 0;
    for index in 0..frames {
        let byte = raw.get(ADPCM_TABLE_BYTES + index / 2).copied().unwrap_or(0);
        let nybble = match index % 2 {
            0 => byte & 0x0F,
            _ => byte >> 4,
        };
        let delta = table.get(nybble as usize).copied().unwrap_or(0);
        accumulator = accumulator.wrapping_add(delta);
        pcm.push(accumulator as i16 * 256);
    }
    pcm
}

fn read_u8(bytes: &[u8], offset: usize) -> u8 { bytes.get(offset).copied().unwrap_or(0) }

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        read_u8(bytes, offset),
        read_u8(bytes, offset + 1),
        read_u8(bytes, offset + 2),
        read_u8(bytes, offset + 3),
    ])
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use alloc::vec;

    fn synthetic_sample_header(flags: u8, reserved: u8) -> [u8; HEADER_LENGTH] {
        let mut bytes = [0u8; HEADER_LENGTH];
        bytes[0x00..0x04].copy_from_slice(&1000u32.to_le_bytes());
        bytes[0x04..0x08].copy_from_slice(&200u32.to_le_bytes());
        bytes[0x08..0x0C].copy_from_slice(&400u32.to_le_bytes());
        bytes[0x0C] = 48;
        bytes[0x0D] = (-32i8) as u8;
        bytes[0x0E] = flags;
        bytes[0x0F] = 200;
        bytes[0x10] = (-12i8) as u8;
        bytes[0x11] = reserved;
        bytes[NAME_OFFSET..NAME_OFFSET + 5].copy_from_slice(b"kick ");
        bytes
    }

    #[test]
    fn a_sample_header_reads_every_field_the_loader_uses() {
        let header = XmSampleHeader::parse(&synthetic_sample_header(FLAG_FORWARD_LOOP, 0));

        assert_eq!(header.length_bytes, 1000);
        assert_eq!(header.volume, 48);
        assert_eq!(header.finetune, -32);
        assert_eq!(header.pan, 200);
        assert_eq!(header.relative_note, -12);
        assert!(header.loops());
        assert!(!header.is_ping_pong());
        assert_eq!(header.frames(), 1000);
        assert_eq!(header.loop_start_frames(), 200);
        assert_eq!(header.loop_end_frames(), 600);
        assert_eq!(header.encoded_bytes(), 1000);
    }

    #[test]
    fn a_sixteen_bit_header_halves_every_byte_count() {
        let header = XmSampleHeader::parse(&synthetic_sample_header(FLAG_FORWARD_LOOP | FLAG_SIXTEEN_BIT, 0));
        assert_eq!(header.frames(), 500);
        assert_eq!(header.loop_start_frames(), 100);
        assert_eq!(header.loop_end_frames(), 300);
        assert_eq!(header.encoded_bytes(), 1000, "the file still holds 1000 bytes");
    }

    #[test]
    fn a_stereo_sixteen_bit_header_quarters_them() {
        let header = XmSampleHeader::parse(&synthetic_sample_header(FLAG_SIXTEEN_BIT | FLAG_STEREO, 0));
        assert_eq!(header.frames(), 250);
        assert_eq!(header.channels(), 2);
        assert_eq!(header.encoded_bytes(), 1000);
    }

    #[test]
    fn the_adpcm_marker_only_counts_on_an_eight_bit_mono_sample() {
        assert!(XmSampleHeader::parse(&synthetic_sample_header(0, RESERVED_ADPCM)).is_adpcm());
        assert!(!XmSampleHeader::parse(&synthetic_sample_header(FLAG_SIXTEEN_BIT, RESERVED_ADPCM)).is_adpcm());
        assert!(!XmSampleHeader::parse(&synthetic_sample_header(FLAG_STEREO, RESERVED_ADPCM)).is_adpcm());
        assert!(!XmSampleHeader::parse(&synthetic_sample_header(0, 22)).is_adpcm(), "FT2 writes a name length here");

        let header = XmSampleHeader::parse(&synthetic_sample_header(0, RESERVED_ADPCM));
        assert_eq!(header.frames(), 1000, "the length field is already a frame count");
        assert_eq!(header.encoded_bytes(), ADPCM_TABLE_BYTES + 500);
    }

    #[test]
    fn a_short_header_reads_its_missing_fields_as_zero() {
        let header = XmSampleHeader::parse(&[0u8; 4]);
        assert_eq!(header, XmSampleHeader::parse(&[0u8; HEADER_LENGTH]));
        assert_eq!(header.frames(), 0);
        assert!(!header.loops());
    }

    #[test]
    fn eight_bit_data_is_a_wrapping_signed_delta_stream() {
        let header = XmSampleHeader::parse(&synthetic_sample_header(0, 0));
        let raw = [10u8, 10, (-20i8) as u8, 0x7F, 0x7F];
        assert_eq!(decode_frames(&raw, 5, &header), vec![10 * 256, 20 * 256, 0, 127 * 256, (-2i16) * 256]);
    }

    #[test]
    fn sixteen_bit_data_is_the_same_stream_a_word_at_a_time() {
        let header = XmSampleHeader::parse(&synthetic_sample_header(FLAG_SIXTEEN_BIT, 0));
        let mut raw = Vec::new();
        for delta in [1000i16, 1000, -3000] {
            raw.extend_from_slice(&delta.to_le_bytes());
        }
        assert_eq!(decode_frames(&raw, 3, &header), vec![1000, 2000, -1000]);
    }

    #[test]
    fn a_stereo_sample_is_two_streams_averaged_into_one() {
        let header = XmSampleHeader::parse(&synthetic_sample_header(FLAG_STEREO, 0));
        // Left deltas +10 +10, right deltas +30 +30: frames average to 20 and 40.
        let raw = [10u8, 10, 30, 30];
        assert_eq!(decode_frames(&raw, 2, &header), vec![20 * 256, 40 * 256]);
    }

    #[test]
    fn data_shorter_than_the_frames_asked_for_holds_its_last_value() {
        let header = XmSampleHeader::parse(&synthetic_sample_header(0, 0));
        assert_eq!(decode_frames(&[5u8], 4, &header), vec![5 * 256, 5 * 256, 5 * 256, 5 * 256]);
        assert_eq!(decode_frames(&[], 2, &header), vec![0, 0]);
    }

    #[test]
    fn adpcm_data_is_a_table_then_two_nybble_indices_per_byte() {
        let header = XmSampleHeader::parse(&synthetic_sample_header(0, RESERVED_ADPCM));
        let mut raw = vec![0i8 as u8; ADPCM_TABLE_BYTES];
        raw[1] = 4; // table[1] = +4
        raw[2] = (-8i8) as u8; // table[2] = -8
        // Nybbles, low first: 1, 2, 1, 1 -> +4, -8, +4, +4 -> 4, -4, 0, 4.
        raw.push(0x21);
        raw.push(0x11);
        assert_eq!(decode_frames(&raw, 4, &header), vec![4 * 256, -4 * 256, 0, 4 * 256]);
    }

    #[test]
    fn an_adpcm_stream_that_ends_early_reads_index_zero() {
        let header = XmSampleHeader::parse(&synthetic_sample_header(0, RESERVED_ADPCM));
        let raw = vec![0u8; ADPCM_TABLE_BYTES];
        assert_eq!(decode_frames(&raw, 3, &header), vec![0, 0, 0]);
        assert_eq!(decode_frames(&[], 2, &header), vec![0, 0]);
    }
}
