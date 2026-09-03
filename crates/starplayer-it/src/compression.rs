//! The IT 2.14 sample decompressor, and its IT 2.15 double-delta variant.
//!
//! This is a port of OpenMPT's `ITDecompression` (`soundlib/ITCompression.cpp`) and the
//! least-significant-bit-first `BitReader` it reads through
//! (`soundlib/BitReader.h`), which is the only complete description of the scheme in
//! existence — `ITTECH.TXT` documents the sample *header's* compression flag and nothing
//! about the bit stream.
//!
//! # The stream
//!
//! A compressed sample is a run of **blocks**. Each block is a little-endian `u16` byte
//! count followed by that many bytes of bit stream; a count of zero is skipped. One block
//! decodes at most [`BLOCK_BYTES`] bytes' worth of samples — `0x8000` samples for 8-bit
//! data, `0x4000` for 16-bit — and the integrator memories are reset at every block
//! boundary. Bits are read from the least significant end of each byte, and a block that
//! runs out of bits mid-symbol simply stops: [`decompress`] then moves on to the next
//! block header rather than failing, which is what OpenMPT's `catch(BitReader::eof)` does.
//!
//! Within a block the decoder runs a bit-width state machine that starts at 9 bits (8-bit
//! data) or 17 (16-bit) and shrinks as the residuals do. Three modes share the loop:
//!
//! | Width | Mode | Width change spelled as |
//! |---|---|---|
//! | 1..=6 | A | the single value `1 << (width - 1)`, then [`FETCH_A_8`] / [`FETCH_A_16`] more bits |
//! | 7..=8 / 7..=16 | B | a value in a small window around the sign bit |
//! | 9 / 17 | C | the top bit set, the rest being the new width minus one |
//!
//! Everything else is a residual. The residual is sign-extended (except in mode C, where
//! the value is already the full width), added into the first integrator, and — for IT
//! 2.15, which delta-codes twice — into a second. Both integrators wrap at 32 bits and the
//! result is truncated to the sample's own width, exactly as OpenMPT's `unsigned int`
//! arithmetic and `static_cast` do.
//!
//! # Which variant a file uses
//!
//! `Cvt` bit 2 (delta) **together with** the compressed sample flag selects IT 2.15
//! (`ITSample::GetSampleFormat`: `(cvt & cvtDelta) ? SampleIO::IT215 : SampleIO::IT214`).
//! Without the compressed flag the same bit means ordinary delta-coded PCM, which is
//! [`crate::sample`]'s business, not this module's.

use alloc::vec::Vec;

/// Bytes one block decodes into, before the sample's own width is applied: `0x8000`
/// samples of 8-bit data, `0x4000` of 16-bit.
pub const BLOCK_BYTES: usize = 0x8000;

/// Bits fetched after a mode-A width change in 8-bit data.
pub const FETCH_A_8: u32 = 3;
/// Bits fetched after a mode-A width change in 16-bit data.
pub const FETCH_A_16: u32 = 4;

/// Starting — and maximum — bit width for 8-bit data.
pub const DEFAULT_WIDTH_8: u32 = 9;
/// Starting — and maximum — bit width for 16-bit data.
pub const DEFAULT_WIDTH_16: u32 = 17;

/// Low end of mode B's width-change window, 8-bit.
const LOWER_B_8: i32 = -4;
/// High end of mode B's width-change window, 8-bit.
const UPPER_B_8: i32 = 3;
/// Low end of mode B's width-change window, 16-bit.
const LOWER_B_16: i32 = -8;
/// High end of mode B's width-change window, 16-bit.
const UPPER_B_16: i32 = 7;

/// The per-width parameters `ITCompression.cpp` keeps in `IT8BitParams` / `IT16BitParams`.
/// Only the decompression half is here; the compressor's `lowerTab` / `upperTab` are not
/// needed to read a file.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Parameters {
    fetch_a: u32,
    lower_b: i32,
    upper_b: i32,
    default_width: u32,
    block_samples: usize,
    wide: bool,
}

impl Parameters {
    const EIGHT_BIT: Parameters = Parameters {
        fetch_a: FETCH_A_8,
        lower_b: LOWER_B_8,
        upper_b: UPPER_B_8,
        default_width: DEFAULT_WIDTH_8,
        block_samples: BLOCK_BYTES,
        wide: false,
    };

    const SIXTEEN_BIT: Parameters = Parameters {
        fetch_a: FETCH_A_16,
        lower_b: LOWER_B_16,
        upper_b: UPPER_B_16,
        default_width: DEFAULT_WIDTH_16,
        block_samples: BLOCK_BYTES / 2,
        wide: true,
    };

    const fn for_width(wide: bool) -> Parameters {
        match wide {
            true => Parameters::SIXTEEN_BIT,
            false => Parameters::EIGHT_BIT,
        }
    }
}

/// A least-significant-bit-first reader over one block's bytes.
///
/// [`BitStream::read`] answers `None` where OpenMPT's `BitReader` throws `eof`; the caller
/// abandons the block, which is what makes a truncated compressed sample a shorter sample
/// rather than a failure.
#[derive(Clone, Debug)]
struct BitStream<'data> {
    bytes: &'data [u8],
    position: usize,
    buffer: u32,
    available: u32,
}

impl<'data> BitStream<'data> {
    const fn new(bytes: &'data [u8]) -> BitStream<'data> {
        BitStream { bytes, position: 0, buffer: 0, available: 0 }
    }

    /// The next `count` bits, or `None` once the block's bytes are exhausted.
    ///
    /// `count` is never more than [`DEFAULT_WIDTH_16`], so the buffer holds at most 24
    /// pending bits and cannot overflow.
    fn read(&mut self, count: u32) -> Option<u32> {
        if count == 0 || count > 32 {
            return None;
        }
        while self.available < count {
            let byte = self.bytes.get(self.position).copied()?;
            self.position += 1;
            self.buffer |= (byte as u32) << self.available;
            self.available += 8;
        }
        let mask = if count >= 32 { u32::MAX } else { (1u32 << count) - 1 };
        let value = self.buffer & mask;
        self.buffer >>= count;
        self.available -= count;
        Some(value)
    }
}

/// The two integrators, reset at every block boundary.
#[derive(Copy, Clone, Debug, Default)]
struct Integrators {
    first: u32,
    second: u32,
}

/// Apply `ITDecompression::ChangeWidth`: the new width is one more than the fetched value,
/// and one more again if that would not actually change the width.
fn change_width(current: u32, fetched: u32) -> u32 {
    let mut width = fetched.saturating_add(1);
    if width >= current {
        width = width.saturating_add(1);
    }
    width
}

/// Decode one block into `output`, stopping after `wanted` samples or when the bit stream
/// runs out.
fn decode_block(bytes: &[u8], wanted: usize, parameters: Parameters, is_215: bool, output: &mut Vec<i16>) {
    let mut stream = BitStream::new(bytes);
    let mut integrators = Integrators::default();
    let mut width = parameters.default_width;
    let mut remaining = wanted;

    while remaining > 0 {
        if width > parameters.default_width {
            // A mode-C escape asked for a width the format cannot express. OpenMPT calls
            // this "Error!" and abandons the block.
            return;
        }
        let Some(value) = stream.read(width) else { return };
        let top_bit = 1i32 << (width - 1);
        let value = value as i32;

        if width <= 6 {
            // Mode A: the single value `topBit` escapes to a width change.
            if value == top_bit {
                let Some(fetched) = stream.read(parameters.fetch_a) else { return };
                width = change_width(width, fetched);
            } else {
                write_sample(value, top_bit, parameters, is_215, &mut integrators, output);
                remaining -= 1;
            }
        } else if width < parameters.default_width {
            // Mode B: a small window around the sign bit escapes to a width change.
            let low = top_bit + parameters.lower_b;
            let high = top_bit + parameters.upper_b;
            if value >= low && value <= high {
                width = change_width(width, (value - low) as u32);
            } else {
                write_sample(value, top_bit, parameters, is_215, &mut integrators, output);
                remaining -= 1;
            }
        } else {
            // Mode C: the top bit says "the rest is the new width, minus one".
            if value & top_bit != 0 {
                width = ((value & !top_bit) as u32).saturating_add(1);
            } else {
                write_sample(value & !top_bit, 0, parameters, is_215, &mut integrators, output);
                remaining -= 1;
            }
        }
    }
}

/// Sign-extend, integrate and store one residual.
fn write_sample(value: i32, top_bit: i32, parameters: Parameters, is_215: bool, integrators: &mut Integrators, output: &mut Vec<i16>) {
    let residual = match top_bit != 0 && value & top_bit != 0 {
        true => value - (top_bit << 1),
        false => value,
    };
    integrators.first = integrators.first.wrapping_add(residual as u32);
    integrators.second = integrators.second.wrapping_add(integrators.first);
    let integrated = match is_215 {
        true => integrators.second,
        false => integrators.first,
    };
    output.push(match parameters.wide {
        true => integrated as u16 as i16,
        // The decompressed 8-bit value is *signed*, unlike raw IT 8-bit PCM, so it widens
        // by a shift rather than through the unsigned-origin exclusive-or.
        false => (integrated as u8 as i8 as i16) * 256,
    });
}

/// Decode one channel of an IT 2.14 / 2.15 compressed sample.
///
/// `frames` is the sample length the header declares — the decoder stops there — and
/// `data` is everything from the sample pointer to the end of the file, because the
/// compressed length is not stored anywhere: it is the sum of the block headers walked.
///
/// Returns the decoded frames, which may be **shorter** than `frames` if the data ran out,
/// and how many bytes of `data` were consumed, so a stereo sample's second channel can be
/// decoded from where the first stopped.
///
/// Nothing here can fail and nothing allocates ahead of what it decodes: the output grows
/// one frame at a time, so a header that claims four gigaframes costs only what its bit
/// stream can actually produce.
pub fn decompress(data: &[u8], frames: usize, wide: bool, is_215: bool) -> (Vec<i16>, usize) {
    let parameters = Parameters::for_width(wide);
    let mut output = Vec::new();
    let mut cursor = 0usize;

    while output.len() < frames && cursor + 2 <= data.len() {
        let size = match data.get(cursor..cursor + 2) {
            Some([low, high]) => u16::from_le_bytes([*low, *high]) as usize,
            _ => break,
        };
        cursor += 2;
        if size == 0 {
            // "Malformed sample?" — OpenMPT skips the header and keeps going. The two
            // bytes are consumed either way, so this cannot spin.
            continue;
        }
        let end = core::cmp::min(cursor.saturating_add(size), data.len());
        let block = data.get(cursor..end).unwrap_or_default();
        cursor = end;

        let wanted = core::cmp::min(frames - output.len(), parameters.block_samples);
        decode_block(block, wanted, parameters, is_215, &mut output);
    }

    (output, cursor)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use alloc::vec;

    /// The compressor's bit writer, so a round trip can be tested without committing
    /// binary fixtures. It is the inverse of [`BitStream::read`] alone — it does **not**
    /// implement `ITCompression`'s width search, because these tests drive the width
    /// changes explicitly.
    #[derive(Default)]
    struct BitWriter {
        bytes: Vec<u8>,
        partial: u8,
        used: u32,
    }

    impl BitWriter {
        fn write(&mut self, mut width: u32, mut value: u32) {
            while width > 0 {
                let room = 8 - self.used;
                let taken = core::cmp::min(room, width);
                let chunk = value & ((1u32 << taken) - 1);
                self.partial |= (chunk as u8) << self.used;
                self.used += taken;
                value >>= taken;
                width -= taken;
                if self.used == 8 {
                    self.bytes.push(self.partial);
                    self.partial = 0;
                    self.used = 0;
                }
            }
        }

        fn finish(mut self) -> Vec<u8> {
            if self.used > 0 {
                self.bytes.push(self.partial);
            }
            self.bytes
        }
    }

    /// Wrap one block's bit stream in the two-byte length header [`decompress`] expects.
    fn block(bits: Vec<u8>) -> Vec<u8> {
        let mut bytes = (bits.len() as u16).to_le_bytes().to_vec();
        bytes.extend_from_slice(&bits);
        bytes
    }

    /// Eight-bit residuals written in mode C, the widest and simplest encoding: nine bits
    /// each, top bit clear, the low eight being the value.
    fn eight_bit_mode_c(residuals: &[u8]) -> Vec<u8> {
        let mut writer = BitWriter::default();
        for residual in residuals {
            writer.write(DEFAULT_WIDTH_8, *residual as u32);
        }
        block(writer.finish())
    }

    #[test]
    fn a_bit_stream_reads_least_significant_bits_first_and_ends_rather_than_panicking() {
        let mut stream = BitStream::new(&[0b1010_0101, 0b0000_0011]);
        assert_eq!(stream.read(4), Some(0b0101));
        assert_eq!(stream.read(4), Some(0b1010));
        assert_eq!(stream.read(8), Some(0b0000_0011), "a read may span bytes");
        assert_eq!(stream.read(1), None, "past the end is None, not a panic");

        let mut spanning = BitStream::new(&[0xFF, 0x01]);
        assert_eq!(spanning.read(9), Some(0b1_1111_1111), "nine bits span two bytes");
        assert_eq!(BitStream::new(&[]).read(1), None);
        assert_eq!(BitStream::new(&[0xFF]).read(0), None, "a zero-width read is refused");
    }

    #[test]
    fn eight_bit_mode_c_residuals_integrate_once_under_it_214() {
        // Residuals 1, 1, 254 (-2 as an unsigned byte added into a wrapping accumulator):
        // the running sum is 1, 2, 0.
        let data = eight_bit_mode_c(&[1, 1, 254]);
        let (pcm, consumed) = decompress(&data, 3, false, false);

        assert_eq!(pcm, vec![256, 512, 0]);
        assert_eq!(consumed, data.len());
    }

    #[test]
    fn the_it_215_variant_integrates_twice() {
        // The same residuals, double-integrated: 1, 3, 3.
        let data = eight_bit_mode_c(&[1, 1, 254]);
        let (pcm, _) = decompress(&data, 3, false, true);

        assert_eq!(pcm, vec![256, 768, 768]);
    }

    #[test]
    fn mode_c_escapes_to_a_narrower_width_and_mode_a_escapes_back() {
        let mut writer = BitWriter::default();
        // Mode C: top bit set, low bits = new width - 1, so `3` selects width 4.
        writer.write(DEFAULT_WIDTH_8, (1 << (DEFAULT_WIDTH_8 - 1)) | 3);
        // Width 4 is mode A. Two residuals: +1, then -1 as a 4-bit two's complement.
        writer.write(4, 1);
        writer.write(4, 0b1111);
        // The mode-A escape value at width 4 is `1 << 3`; the fetched three bits select
        // the new width through `change_width`: 7 + 1 = 8, which is >= 4, so 9.
        writer.write(4, 0b1000);
        writer.write(FETCH_A_8, 7);
        // Back at width 9 — mode C again — one more residual of +1.
        writer.write(DEFAULT_WIDTH_8, 1);
        let data = block(writer.finish());

        let (pcm, _) = decompress(&data, 3, false, false);
        assert_eq!(pcm, vec![256, 0, 256], "the running sum is 1, 0, 1");
    }

    #[test]
    fn mode_b_changes_width_from_inside_its_window() {
        let mut writer = BitWriter::default();
        // Enter width 8, which is mode B for 8-bit data.
        writer.write(DEFAULT_WIDTH_8, (1 << (DEFAULT_WIDTH_8 - 1)) | 7);
        // One residual of +1 at width 8.
        writer.write(8, 1);
        // The mode-B window is `topBit + lowerB ..= topBit + upperB` = 124..=131 at width
        // 8. The window's first value asks for width 1, `change_width(8, 0) = 1`.
        writer.write(8, 124);
        // Width 1 is mode A: `1 << 0` is the escape, so `0` is a residual of 0 and `1`
        // would be the escape. Two zero residuals hold the sum.
        writer.write(1, 0);
        writer.write(1, 0);
        let data = block(writer.finish());

        let (pcm, _) = decompress(&data, 3, false, false);
        assert_eq!(pcm, vec![256, 256, 256]);
    }

    #[test]
    fn sixteen_bit_data_uses_the_seventeen_bit_default_width() {
        let mut writer = BitWriter::default();
        for residual in [1u32, 0x1_0000 - 3, 2] {
            writer.write(DEFAULT_WIDTH_16, residual & 0xFFFF);
        }
        let data = block(writer.finish());

        let (pcm, _) = decompress(&data, 3, true, false);
        assert_eq!(pcm, vec![1, -2, 0], "the running sum wraps at sixteen bits");
    }

    #[test]
    fn a_truncated_block_yields_a_shorter_sample_rather_than_a_failure() {
        let full = eight_bit_mode_c(&[1, 1, 1]);
        // Keep the header's byte count but hand over only part of the bit stream.
        let truncated = full.get(..4).expect("the fixture is longer than four bytes");
        let (pcm, consumed) = decompress(truncated, 3, false, false);

        assert!(pcm.len() < 3, "the stream ran out: {} frames decoded", pcm.len());
        assert_eq!(consumed, truncated.len());
    }

    #[test]
    fn a_zero_length_block_is_skipped_without_spinning() {
        let mut data = vec![0u8, 0];
        data.extend_from_slice(&eight_bit_mode_c(&[5]));
        let (pcm, consumed) = decompress(&data, 1, false, false);

        assert_eq!(pcm, vec![5 * 256]);
        assert_eq!(consumed, data.len());
    }

    #[test]
    fn an_empty_or_headerless_stream_decodes_to_nothing() {
        assert_eq!(decompress(&[], 100, false, false), (Vec::new(), 0));
        assert_eq!(decompress(&[0x10], 100, false, false), (Vec::new(), 0), "a lone byte cannot even be a block header");
        assert_eq!(decompress(&[0xFF, 0xFF], 100, false, false).0.len(), 0, "a block header with no block behind it");
    }

    #[test]
    fn a_declared_length_far_beyond_the_data_costs_only_what_the_data_holds() {
        let data = eight_bit_mode_c(&[1; 16]);
        let (pcm, _) = decompress(&data, usize::MAX / 2, false, false);

        assert_eq!(pcm.len(), 16, "the four-gigaframe claim allocates sixteen frames");
    }

    #[test]
    fn a_second_channel_decodes_from_where_the_first_stopped() {
        let mut data = eight_bit_mode_c(&[1, 1]);
        let right = eight_bit_mode_c(&[2, 2]);
        data.extend_from_slice(&right);

        let (left_pcm, consumed) = decompress(&data, 2, false, false);
        let (right_pcm, _) = decompress(data.get(consumed..).expect("the left channel stopped inside the data"), 2, false, false);

        assert_eq!(left_pcm, vec![256, 512]);
        assert_eq!(right_pcm, vec![512, 1024]);
    }

    #[test]
    fn a_block_never_decodes_more_than_its_own_sample_budget() {
        assert_eq!(Parameters::EIGHT_BIT.block_samples, 0x8000);
        assert_eq!(Parameters::SIXTEEN_BIT.block_samples, 0x4000);
    }

    #[test]
    fn change_width_never_returns_the_width_it_was_given() {
        for current in 1..=DEFAULT_WIDTH_16 {
            for fetched in 0..16u32 {
                assert_ne!(change_width(current, fetched), current, "current {current}, fetched {fetched}");
            }
        }
    }
}
