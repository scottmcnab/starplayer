//! Host-testable decisions for a circular DMA refill which stages a descriptor before offering.
//!
//! The board owns the async DMA transfer, rendering and staged bytes. This module only decides
//! whether an observed availability result admits the already-staged descriptor, whether a run of
//! failures has proved the ring needs a descriptor handed back before anything else can happen,
//! and whether a completed handoff may stage the next descriptor.

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
/// A [`AvailabilityError::Late`] recovers on sight. It cannot be a transient: `update` returns it
/// whenever its walk finds every descriptor CPU-owned, and it reports success only once one is
/// DMA-owned again, which only this refill can arrange. Every other failure retries, which is what
/// the transport did through a clean two-minute hardware soak — that run logged exactly one
/// startup error and then none, so the path a healthy transport actually takes stays a plain
/// retry.
///
/// Recovering on sight also matters for *pace*, which is what a run of hardware measurements
/// settled. An earlier build waited for eight consecutive `Late` results before recovering, on
/// the theory that more evidence is safer. It is not free: the check clears EOF, so every failed
/// `available()` after the first of a run must wait for the next descriptor completion before it
/// can fail again. At 48 kHz that is 5.3 ms each, and an overloaded `unreal.s3m` logged 108
/// errors across 14 recoveries in one wall second — **501 ms of that second spent waiting to be
/// told something already known**, which halved the audio the refill could produce. Evidence
/// that costs a descriptor period per sample is the wrong kind of caution.
///
/// The caller has already rendered and packed the descriptor before the check that reaches here.
/// A reversed order — reserving an offer first and rendering against it — was tried and twice
/// failed on hardware; see the A1S `audio.rs` record.
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
pub const fn decide_availability(available: Result<usize, AvailabilityError>, descriptor_bytes: usize, ring_bytes: usize) -> AvailabilityDecision {
    match available {
        Ok(bytes) if bytes >= descriptor_bytes && bytes.is_multiple_of(descriptor_bytes) => {
            AvailabilityDecision::HandOffStaged { underrun: bytes >= ring_bytes }
        }
        Err(AvailabilityError::Late) => AvailabilityDecision::RecoverOwnership,
        Ok(_) | Err(AvailabilityError::Transient) => AvailabilityDecision::Retry,
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

    fn decide(available: Result<usize, AvailabilityError>) -> AvailabilityDecision {
        decide_availability(available, DESCRIPTOR_BYTES, RING_BYTES)
    }

    #[test]
    fn a_transient_error_only_ever_retries() {
        assert_eq!(decide(Err(AvailabilityError::Transient)), AvailabilityDecision::Retry);
    }

    #[test]
    fn the_first_late_recovers_on_sight() {
        // Waiting for a second opinion costs a descriptor period per check, because the failed
        // check cleared the EOF the next one needs. `Late` is self-perpetuating, so there is
        // nothing to wait for.
        assert_eq!(decide(Err(AvailabilityError::Late)), AvailabilityDecision::RecoverOwnership);
    }

    #[test]
    fn recovery_never_renders_or_stages() {
        // The board reads this off the variant: `RecoverOwnership` carries no descriptor to write,
        // so the engine cannot advance to manufacture recovery audio.
        assert_eq!(decide(Err(AvailabilityError::Late)), AvailabilityDecision::RecoverOwnership);
    }

    #[test]
    fn a_whole_offer_hands_the_staged_descriptor_over() {
        assert_eq!(decide(Ok(DESCRIPTOR_BYTES)), AvailabilityDecision::HandOffStaged { underrun: false });
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
        assert_eq!(decide(Ok(DESCRIPTOR_BYTES)), AvailabilityDecision::HandOffStaged { underrun: false });
        assert_eq!(decide(Ok(2 * DESCRIPTOR_BYTES)), AvailabilityDecision::HandOffStaged { underrun: false });
        assert_eq!(decide(Ok(0)), AvailabilityDecision::Retry);
        assert_eq!(decide(Ok(DESCRIPTOR_BYTES + 1)), AvailabilityDecision::Retry);
    }

    #[test]
    fn a_whole_ring_is_reported_as_an_underrun() {
        assert_eq!(decide(Ok(RING_BYTES)), AvailabilityDecision::HandOffStaged { underrun: true });
    }
}
