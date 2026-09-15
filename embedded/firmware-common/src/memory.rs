//! Checked partitioning for the A1S's claim-only PSRAM arena.
//!
//! This stays board-independent so exact capacity and undersized-region behaviour can be
//! tested on the host. The board still owns the mapped addresses and hands the resulting
//! byte counts to its arena; no allocator knows about PSRAM.

/// A checked division of PSRAM between network futures, decoder scratch and the three
/// reusable upload/image buffers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PsramLayout {
    pub network_bytes: usize,
    pub workspace_bytes: usize,
    pub buffer_bytes: usize,
    pub unused_bytes: usize,
}

impl PsramLayout {
    /// Reserve `network_bytes` and `workspace_bytes`, then divide everything left into
    /// three equal buffers whose starts and lengths remain four-byte aligned.
    pub fn calculate(total_bytes: usize, network_bytes: usize, workspace_bytes: usize) -> Option<PsramLayout> {
        let fixed = network_bytes.checked_add(workspace_bytes)?;
        let remaining = total_bytes.checked_sub(fixed)?;
        let buffer_bytes = (remaining / 3) & !3;
        if buffer_bytes == 0 {
            return None;
        }
        let used = fixed.checked_add(buffer_bytes.checked_mul(3)?)?;
        Some(PsramLayout { network_bytes, workspace_bytes, buffer_bytes, unused_bytes: total_bytes - used })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_mibibytes_has_the_approved_three_buffer_layout() {
        let layout = PsramLayout::calculate(4 * 1024 * 1024, 64 * 1024, 256 * 1024).unwrap();
        assert_eq!(layout.buffer_bytes, 1_288_872);
        assert_eq!(layout.unused_bytes, 8);
        assert_eq!(layout.buffer_bytes % 4, 0);
    }

    #[test]
    fn one_byte_below_the_smallest_usable_layout_is_refused() {
        assert_eq!(PsramLayout::calculate(64 + 256 + 11, 64, 256), None);
        assert_eq!(PsramLayout::calculate(64 + 256 + 12, 64, 256).unwrap().buffer_bytes, 4);
    }

    #[test]
    fn fixed_reservation_overflow_and_oversubscription_are_refused() {
        assert_eq!(PsramLayout::calculate(usize::MAX, usize::MAX, 1), None);
        assert_eq!(PsramLayout::calculate(100, 64, 64), None);
    }
}
