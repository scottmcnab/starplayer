//! [`SeqlockU64`] — one 64-bit value published across a thread boundary using nothing but
//! 32-bit atomics.
//!
//! # Why this exists at all
//!
//! Both halves of this host carry 64-bit frame counters across the boundary between the
//! DMA refill and the control task: the taps' two clocks, and the seek mailbox's target
//! and stamp. On a desktop those are an `AtomicU64` and the matter ends there. On the two
//! targets M8 is about it does not:
//!
//! * `riscv32imc-unknown-none-elf` — the target CI checks on every commit — has no atomic
//!   instructions at all, and the ESP32's LX6 has 32-bit compare-and-swap but no 64-bit
//!   one. `portable_atomic::AtomicU64` is therefore unavailable on both unless
//!   `portable-atomic`'s `fallback` feature is turned on, and that fallback is a
//!   **critical section** — a lock, taken inside `render()`, which design goal 5 forbids.
//! * Truncating the counters to `u32` would wrap after 2^32 frames, which is 27 hours at
//!   44.1 kHz. For a device that is left switched on that is a real cliff, not a
//!   theoretical one, and what it breaks is the clock a freshly loaded sequencer is
//!   stamped against.
//!
//! So: the seqlock architecture §9(a) names, narrowed to one value. The **writer never
//! waits, never retries and never masks an interrupt** — four stores and no branch — which
//! is what makes it legal in the audio path. A reader that catches a write in progress
//! reads again, and since publication happens once per render call, milliseconds apart, it
//! essentially never does.

use starplayer_rt::atomic::{AtomicU32, Ordering};

/// Reads a [`SeqlockU64`] retries before it gives up and answers with what it has.
///
/// Two would do. Eight is there so that the give-up arm is unreachable by anything short
/// of a writer that never stops writing — which would mean one side is publishing faster
/// than the other can perform six relaxed loads, and if that ever happens, spinning here
/// is not the answer either. Bounded rather than a `loop`, because one of the two readers
/// is the DMA refill.
const READ_ATTEMPTS: usize = 8;

/// A 64-bit value published by one writer to one reader, over 32-bit atomics.
#[derive(Debug, Default)]
pub struct SeqlockU64 {
    /// Even when the two halves agree, odd while they are being written.
    version: AtomicU32,
    low: AtomicU32,
    high: AtomicU32,
}

impl SeqlockU64 {
    /// Publish `value`. Four stores, no branch, no retry — safe in `render()`.
    pub fn store(&self, value: u64) {
        let version = self.version.load(Ordering::Relaxed);
        self.version.store(version.wrapping_add(1), Ordering::Release);
        self.low.store(value as u32, Ordering::Release);
        self.high.store((value >> 32) as u32, Ordering::Release);
        self.version.store(version.wrapping_add(2), Ordering::Release);
    }

    /// The most recent completely-written value.
    pub fn load(&self) -> u64 {
        let mut halves = 0u64;
        for _ in 0..READ_ATTEMPTS {
            let before = self.version.load(Ordering::Acquire);
            let low = self.low.load(Ordering::Acquire);
            let high = self.high.load(Ordering::Acquire);
            halves = (high as u64) << 32 | low as u64;
            if before.is_multiple_of(2) && self.version.load(Ordering::Acquire) == before {
                return halves;
            }
        }
        // Unreachable short of a writer that never stops; see [`READ_ATTEMPTS`]. The worst
        // this can be wrong by is one 2^32-frame carry, in a value whose whole contract is
        // that it is a lossy tap.
        halves
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_round_trips_values_on_both_sides_of_the_word_boundary() {
        let slot = SeqlockU64::default();
        assert_eq!(slot.load(), 0, "an unwritten slot reads as zero");
        for value in [1, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX, 44_100 * 60 * 60 * 40] {
            slot.store(value);
            assert_eq!(slot.load(), value);
        }
    }

    #[test]
    fn a_read_that_catches_a_write_in_progress_sees_the_version_odd() {
        let slot = SeqlockU64::default();
        slot.store(7);
        // Exactly what `store` does up to the point the halves have been written but the
        // version has not been closed, which is the window a reader has to detect.
        slot.version.store(1, Ordering::Release);
        slot.low.store(9, Ordering::Release);
        assert_eq!(slot.version.load(Ordering::Acquire) % 2, 1, "the slot advertises itself as mid-write");
        slot.version.store(2, Ordering::Release);
        assert_eq!(slot.load(), 9, "and the closed write reads back whole");
    }
}
