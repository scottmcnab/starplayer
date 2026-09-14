//! Allocation-free PCM packing shared by embedded audio transports.

/// Pack signed `i16` samples into the high half of signed 32-bit little-endian I2S slots.
///
/// Samples stay in their input order, so an interleaved `[left, right]` pair becomes one left
/// slot followed by one right slot. A destination tail shorter than four bytes is untouched.
/// The return value is the number of complete output bytes written.
pub fn pack_i16_high_aligned_le(samples: &[i16], destination: &mut [u8]) -> usize {
    let mut written = 0usize;
    for (sample, slot) in samples.iter().zip(destination.chunks_exact_mut(4)) {
        let bytes = i32::from(*sample).wrapping_shl(16).to_le_bytes();
        for (destination_byte, sample_byte) in slot.iter_mut().zip(bytes) {
            *destination_byte = sample_byte;
        }
        written += 4;
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_and_unit_samples_are_sign_extended_into_the_high_half() {
        let mut packed = [0xa5; 12];
        assert_eq!(pack_i16_high_aligned_le(&[0, 1, -1], &mut packed), 12);
        assert_eq!(packed, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xff, 0xff]);
    }

    #[test]
    fn signed_extrema_keep_their_exact_twos_complement_bits() {
        let mut packed = [0; 8];
        assert_eq!(pack_i16_high_aligned_le(&[i16::MIN, i16::MAX], &mut packed), 8);
        assert_eq!(packed, [0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0xff, 0x7f]);
    }

    #[test]
    fn stereo_order_is_left_slot_then_right_slot_and_no_partial_slot_is_written() {
        let mut packed = [0xa5; 10];
        assert_eq!(pack_i16_high_aligned_le(&[0x1234, -0x1234, 7], &mut packed), 8);
        assert_eq!(packed, [0x00, 0x00, 0x34, 0x12, 0x00, 0x00, 0xcc, 0xed, 0xa5, 0xa5]);
    }
}
