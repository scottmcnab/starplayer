//! Host-testable decisions for a circular DMA refill which stages a descriptor before offering.
//!
//! The board owns the async DMA transfer, rendering and staged bytes. This module only decides
//! whether an observed availability result admits the already-staged descriptor, whether a run of
//! failures has proved the ring needs a descriptor handed back before anything else can happen,
//! and whether a completed handoff may stage the next descriptor.

/// Consecutive `Late` results which prove a self-perpetuating wedge rather than a transient.
///
/// A `Late` is self-perpetuating by construction — `TxCircularState::update` returns it whenever
/// its walk finds every descriptor owned by the CPU, and only the refill can make that untrue —
/// so in principle one is proof enough. The threshold exists because the *cost of being wrong* is
/// asymmetric. Recovery perturbs esp-hal's free-space accounting; a run of this length cannot be
/// anything else, so normal playback keeps exactly the behaviour a 2026-09-15 hardware run soaked
/// clean. At the 48 kHz descriptor rate of 187.5/s this bounds the wedge to about 43 ms.
pub const LATE_RECOVERY_THRESHOLD: u32 = 8;

/// What to do after an explicit availability check.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AvailabilityDecision {
    /// A run of `Late` results has proved the DMA owns nothing. Hand exactly one descriptor back
    /// with a zero-byte handoff, then check availability again.
    RecoverOwnership,
    /// Wait for another availability result without rendering.
    Retry,
    /// A whole-descriptor offer is open. Hand the already-staged descriptor over immediately.
    HandOffStaged { underrun: bool },
}

/// Why an availability check failed, as much of it as the board can tell.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AvailabilityError {
    /// `DmaError::Late`: `update` walked the chain and found every descriptor CPU-owned.
    Late,
    /// Any other HAL error. Transient, and a retry is the whole of the response.
    Transient,
}

/// Classify a circular TX availability result for a fixed whole-descriptor handoff.
///
/// `consecutive_late` counts the unbroken run of [`AvailabilityError::Late`] results ending with
/// this one, so a single `Late` is 1. Only a run of [`LATE_RECOVERY_THRESHOLD`] asks for recovery.
///
/// The caller has already rendered and packed the descriptor before the check that reaches here.
/// A reversed order — reserving an offer first and rendering against it — was tried and twice
/// failed on hardware; see [`crate::dma_refill`] callers and the A1S `audio.rs` record.
///
/// Every other failure retries, which is what the transport did through a clean two-minute
/// hardware soak: that run logged exactly one startup error and then none, so the error path a
/// healthy transport actually takes must stay a plain retry.
///
/// # Why recovery hands back one descriptor and not a ring
///
/// In the `Late` state all three descriptors are free in hardware, but `state.available` reads 0,
/// because `update` returns before crediting anything. Only `update` can credit those bytes, and
/// it refuses while its walk finds every descriptor CPU-owned. One returned descriptor is exactly
/// what breaks that walk, after which the next check credits the free bytes and the steady loop
/// resumes.
///
/// Handing back a whole ring instead is worse, and a 2026-09-16 hardware run proved it. A
/// zero-byte handoff marks a descriptor DMA-owned without consuming anything from
/// `state.available`, so three of them leave the HAL believing a whole ring is writable while the
/// DMA owns all of it. Every later handoff then writes into a buffer the DMA is reading. That
/// build turned a clean `REFLEX.S3M` into about four ring-empty events a second and audibly
/// degraded playback, from a single harmless startup transient the previous build had simply
/// retried.
pub const fn decide_availability(
    available: Result<usize, AvailabilityError>, consecutive_late: u32, descriptor_bytes: usize, ring_bytes: usize,
) -> AvailabilityDecision {
    match available {
        Ok(bytes) if bytes >= descriptor_bytes && bytes.is_multiple_of(descriptor_bytes) => {
            AvailabilityDecision::HandOffStaged { underrun: bytes >= ring_bytes }
        }
        Err(AvailabilityError::Late) if consecutive_late >= LATE_RECOVERY_THRESHOLD => AvailabilityDecision::RecoverOwnership,
        Ok(_) | Err(_) => AvailabilityDecision::Retry,
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

    fn decide(available: Result<usize, AvailabilityError>, consecutive_late: u32) -> AvailabilityDecision {
        decide_availability(available, consecutive_late, DESCRIPTOR_BYTES, RING_BYTES)
    }

    #[test]
    fn a_transient_error_only_ever_retries() {
        for consecutive_late in [0, 1, LATE_RECOVERY_THRESHOLD, 10 * LATE_RECOVERY_THRESHOLD] {
            assert_eq!(decide(Err(AvailabilityError::Transient), consecutive_late), AvailabilityDecision::Retry);
        }
    }

    #[test]
    fn a_short_run_of_late_retries_without_perturbing_the_ring() {
        for consecutive_late in 1..LATE_RECOVERY_THRESHOLD {
            assert_eq!(decide(Err(AvailabilityError::Late), consecutive_late), AvailabilityDecision::Retry);
        }
    }

    #[test]
    fn a_proven_run_of_late_hands_one_descriptor_back() {
        assert_eq!(decide(Err(AvailabilityError::Late), LATE_RECOVERY_THRESHOLD), AvailabilityDecision::RecoverOwnership);
        assert_eq!(decide(Err(AvailabilityError::Late), LATE_RECOVERY_THRESHOLD + 1), AvailabilityDecision::RecoverOwnership);
    }

    #[test]
    fn recovery_never_renders_or_stages() {
        // The board reads this off the variant: `RecoverOwnership` carries no descriptor to write,
        // so the engine cannot advance to manufacture recovery audio.
        assert_eq!(decide(Err(AvailabilityError::Late), LATE_RECOVERY_THRESHOLD), AvailabilityDecision::RecoverOwnership);
    }

    #[test]
    fn a_whole_offer_hands_the_staged_descriptor_over() {
        assert_eq!(decide(Ok(DESCRIPTOR_BYTES), 0), AvailabilityDecision::HandOffStaged { underrun: false });
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
    fn only_aligned_whole_descriptor_offers_admit_a_handoff() {
        assert_eq!(decide(Ok(DESCRIPTOR_BYTES), 0), AvailabilityDecision::HandOffStaged { underrun: false });
        assert_eq!(decide(Ok(2 * DESCRIPTOR_BYTES), 0), AvailabilityDecision::HandOffStaged { underrun: false });
        assert_eq!(decide(Ok(0), 0), AvailabilityDecision::Retry);
        assert_eq!(decide(Ok(DESCRIPTOR_BYTES + 1), 0), AvailabilityDecision::Retry);
    }

    #[test]
    fn a_whole_ring_is_reported_as_an_underrun() {
        assert_eq!(decide(Ok(RING_BYTES), 0), AvailabilityDecision::HandOffStaged { underrun: true });
    }
}
