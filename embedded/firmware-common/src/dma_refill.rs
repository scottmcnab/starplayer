//! Host-testable decisions for a circular DMA refill which reserves before rendering.
//!
//! The board owns the async DMA transfer, rendering and staged bytes. This module only decides
//! whether an observed availability result reserves enough space to render one descriptor, and
//! whether a completed handoff may advance to the next reservation. Reserving first is essential:
//! esp-hal's `push_with` can ignore a `Late` discovered during rendering only while the earlier
//! reservation remains in `TxCircularState::available`.

/// What to do after an explicit availability check.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AvailabilityDecision {
    /// Wait for another availability result without rendering.
    Retry,
    /// Exactly one descriptor may now be rendered and handed off.
    RenderReserved { underrun: bool },
}

/// Classify a circular TX availability result for a fixed whole-descriptor handoff.
///
/// `None` represents any HAL error. Calling `push_with` after such an error is not recovery:
/// esp-hal has already cleared EOF, and its inner `available()` can await forever before reaching
/// the closure. Only a successful aligned result reserves the state needed across rendering.
pub const fn decide_availability(available: Option<usize>, descriptor_bytes: usize, ring_bytes: usize) -> AvailabilityDecision {
    match available {
        Some(bytes) if bytes >= descriptor_bytes && bytes.is_multiple_of(descriptor_bytes) => {
            AvailabilityDecision::RenderReserved { underrun: bytes >= ring_bytes }
        }
        Some(_) | None => AvailabilityDecision::Retry,
    }
}

/// What to do after the constrained handoff completes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WriteDecision {
    /// DMA accepted the whole staged descriptor, so another availability may be reserved.
    ReserveNext,
    /// Preserve and retry the same staged descriptor after another availability check.
    RetryStaged { written_bytes: usize },
}

/// Classify a handoff result without permitting a short write to advance rendering.
///
/// `None` represents any HAL error. The board closure itself must return only zero or one whole
/// descriptor, never a partial descriptor. A failed or short result preserves the staged bytes
/// and retries without rendering again.
pub const fn decide_write(written: Option<usize>, descriptor_bytes: usize) -> WriteDecision {
    match written {
        Some(bytes) if bytes == descriptor_bytes => WriteDecision::ReserveNext,
        Some(bytes) => WriteDecision::RetryStaged { written_bytes: bytes },
        None => WriteDecision::RetryStaged { written_bytes: 0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESCRIPTOR_BYTES: usize = 2_048;
    const RING_BYTES: usize = 6_144;

    #[test]
    fn availability_error_retries_without_rendering_or_handoff() {
        assert_eq!(decide_availability(None, DESCRIPTOR_BYTES, RING_BYTES), AvailabilityDecision::Retry);
    }

    #[test]
    fn a_whole_offer_reserves_render_before_the_handoff() {
        assert_eq!(
            decide_availability(Some(DESCRIPTOR_BYTES), DESCRIPTOR_BYTES, RING_BYTES),
            AvailabilityDecision::RenderReserved { underrun: false }
        );
    }

    #[test]
    fn a_full_reserved_write_moves_to_the_next_reservation() {
        assert_eq!(decide_write(Some(DESCRIPTOR_BYTES), DESCRIPTOR_BYTES), WriteDecision::ReserveNext);
    }

    #[test]
    fn zero_short_and_error_writes_retry_the_staged_descriptor() {
        for written in [Some(0), Some(DESCRIPTOR_BYTES - 1), None] {
            assert!(matches!(decide_write(written, DESCRIPTOR_BYTES), WriteDecision::RetryStaged { .. }));
        }
    }

    #[test]
    fn only_aligned_whole_descriptor_offers_reserve_a_render() {
        assert_eq!(
            decide_availability(Some(DESCRIPTOR_BYTES), DESCRIPTOR_BYTES, RING_BYTES),
            AvailabilityDecision::RenderReserved { underrun: false }
        );
        assert_eq!(
            decide_availability(Some(2 * DESCRIPTOR_BYTES), DESCRIPTOR_BYTES, RING_BYTES),
            AvailabilityDecision::RenderReserved { underrun: false }
        );
        assert_eq!(decide_availability(Some(0), DESCRIPTOR_BYTES, RING_BYTES), AvailabilityDecision::Retry);
        assert_eq!(decide_availability(Some(DESCRIPTOR_BYTES + 1), DESCRIPTOR_BYTES, RING_BYTES), AvailabilityDecision::Retry);
    }

    #[test]
    fn a_whole_ring_is_reported_as_an_underrun() {
        let decision = decide_availability(Some(RING_BYTES), DESCRIPTOR_BYTES, RING_BYTES);
        assert_eq!(decision, AvailabilityDecision::RenderReserved { underrun: true });
    }
}
