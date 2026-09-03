//! [`TapRing`] — the **lossy** per-channel audio tap of architecture §9(b) (M3-D6).
//!
//! Half (a) of telemetry is the coherent scalar [`snapshot`](crate::snapshot): one whole
//! `Copy` value per tracker tick, moved through a bounded SPSC ring so a reader can never
//! pair a row number from one tick with a note from the next. Half (b) is this, and it
//! wants the opposite of coherence.
//!
//! # Why tearing is the right answer here
//!
//! A scope draws a *picture of a waveform*. If the audio thread advances the ring while
//! the UI is copying it, the UI gets a window whose oldest few buckets are one quantum
//! newer than the rest — a seam of at most 2.7 ms in a trace the eye reads as a shape.
//! Nobody can see it, and nobody has ever been able to see it: this is what every
//! hardware scope's un-triggered sweep already does.
//!
//! Paying for coherence would cost the audio thread real work — a seqlock retry loop, or
//! double buffers per channel — to remove an artefact that is invisible. So both sides
//! use `Relaxed` loads and stores and nothing else. There is no lock, no compare-and-swap
//! (so `riscv32imc` does not reach for `portable-atomic/critical-section` to draw a
//! scope), and no ordering guarantee at all between the write index and the values it
//! nominally describes. **A reader may see a torn window. That is intended.**
//!
//! # Shape
//!
//! One ring per channel, [`TAP_RING_BUCKETS`] buckets long, each bucket the value the
//! audio thread sampled for [`TAP_BUCKET_FRAMES`] output frames. Four frames per bucket
//! is 32 buckets per 128-frame `RENDER_QUANTUM`, which is §9's "32 values per quantum,
//! not 4096" — the downsampling happens *in the audio thread*, so no transport ever
//! carries a frame-rate stream.
//!
//! 1024 buckets is 4096 frames: ~93 ms at 44.1 kHz, ~85 ms at 48 kHz. A UI drawing at
//! 60 Hz needs 16.7 ms of history per frame, so the ring holds five refreshes' worth of
//! headroom and a UI that misses a frame or two still draws a continuous trace.
//!
//! The storage is allocated exactly once, in [`TapRing::new`]. Neither half allocates
//! again, so [`TapWriter::write`] and [`TapWriter::commit`] are safe to call from
//! `render()`.
//!
//! # What the tap is *not*
//!
//! It is not the mix. The engine samples voice state, not the accumulator — see
//! `starplayer_engine::scope` for why (short version: there are no per-channel buses, and
//! introducing them would change the float path's summation order and break the goldens).

use alloc::vec::Vec;

use portable_atomic::{AtomicI16, AtomicU32, Ordering};
use portable_atomic_util::Arc;

/// Output frames one tap bucket covers.
///
/// Four, so a 128-frame `RENDER_QUANTUM` is exactly [`TAP_BUCKETS_PER_QUANTUM`] buckets —
/// architecture §9(b)'s "32 values per quantum".
pub const TAP_BUCKET_FRAMES: usize = 4;

/// Buckets one channel's ring holds: ~93 ms at 44.1 kHz, and a power of two so the
/// index wrap is a mask rather than a division.
pub const TAP_RING_BUCKETS: usize = 1024;

/// Buckets one 128-frame render quantum produces.
///
/// Spelled here rather than derived at each call site because the engine's
/// `RENDER_QUANTUM` lives in a crate this one may not depend on.
pub const TAP_BUCKETS_PER_QUANTUM: usize = 32;

const _: () = assert!(TAP_RING_BUCKETS.is_power_of_two(), "the bucket index wraps by mask");
const _: () = assert!(TAP_BUCKETS_PER_QUANTUM * TAP_BUCKET_FRAMES == 128, "128 frames is one RENDER_QUANTUM");

/// The shared storage of one channel's tap: the bucket values, and how far the writer has
/// got.
///
/// Both halves hold a clone of this. It is not itself the API — see [`TapRing::new`],
/// which is the only thing that constructs one and which hands out the two halves.
#[derive(Clone)]
pub struct TapRing {
    values: Arc<[AtomicI16]>,
    write_index: Arc<AtomicU32>,
}

impl TapRing {
    /// Allocate one channel's ring and split it into its writing and reading halves.
    ///
    /// **This is the only allocation the tap performs.** Both halves borrow the same
    /// storage from here on.
    #[allow(clippy::new_ret_no_self, reason = "a ring is only ever useful as its two halves, as with `snapshot_channel`")]
    pub fn new() -> (TapWriter, TapReader) {
        let mut values = Vec::with_capacity(TAP_RING_BUCKETS);
        for _ in 0..TAP_RING_BUCKETS {
            values.push(AtomicI16::new(0));
        }
        let ring = TapRing { values: Arc::from(values), write_index: Arc::new(AtomicU32::new(0)) };
        (TapWriter { ring: ring.clone() }, TapReader { ring })
    }

    fn value(&self, bucket: u32) -> Option<&AtomicI16> {
        self.values.get(bucket as usize % TAP_RING_BUCKETS)
    }
}

/// The writing half. Exactly one thread may hold it, and that thread is the audio thread.
///
/// Every method is a `Relaxed` load or store: no allocation, no lock, no
/// compare-and-swap, and nothing that can panic.
pub struct TapWriter {
    ring: TapRing,
}

impl TapWriter {
    /// Store `value` in `bucket`, replacing whatever was there.
    ///
    /// `bucket` is a monotonic bucket counter, not an index: it wraps into the ring here.
    pub fn write(&self, bucket: u32, value: i16) {
        if let Some(slot) = self.ring.value(bucket) {
            slot.store(value, Ordering::Relaxed);
        }
    }

    /// Add `value` into `bucket`, saturating.
    ///
    /// What several voices sharing one channel need — IT's background voices, and any
    /// format that lets a channel sound twice at once. The writer is single-threaded by
    /// construction, so a plain load-modify-store is sound; it is deliberately *not* an
    /// atomic read-modify-write, which would be a compare-and-swap on the targets that
    /// have no native one.
    pub fn accumulate(&self, bucket: u32, value: i16) {
        if let Some(slot) = self.ring.value(bucket) {
            let sum = slot.load(Ordering::Relaxed).saturating_add(value);
            slot.store(sum, Ordering::Relaxed);
        }
    }

    /// Publish everything written so far by moving the write index to `next_bucket`.
    ///
    /// A reader that races this sees a window one bucket-run out of step. See the module
    /// documentation: that is the whole point of the design.
    pub fn commit(&self, next_bucket: u32) {
        self.ring.write_index.store(next_bucket, Ordering::Relaxed);
    }

    /// The bucket counter the last [`TapWriter::commit`] published.
    pub fn write_index(&self) -> u32 { self.ring.write_index.load(Ordering::Relaxed) }
}

/// The reading half. Held by whoever draws — the UI thread, the worklet's telemetry pack,
/// the TUI's oscilloscope.
pub struct TapReader {
    ring: TapRing,
}

impl TapReader {
    /// Copy the newest `out.len()` buckets into `out`, **oldest first**, and return the
    /// write index that was seen.
    ///
    /// A request longer than [`TAP_RING_BUCKETS`] is served with the whole ring and the
    /// remainder of `out` zeroed, so a caller never reads the same bucket twice under a
    /// different name. Bounded, allocation-free, and it cannot panic — every access goes
    /// through `get`.
    ///
    /// The window may be torn: the writer is free to advance during the copy. The
    /// returned index is what the reader saw *before* copying, so a caller that wants to
    /// know whether anything moved compares it with a later call's.
    pub fn latest(&self, out: &mut [i16]) -> u32 {
        let write_index = self.ring.write_index.load(Ordering::Relaxed);
        let wanted = out.len().min(TAP_RING_BUCKETS);
        let oldest = write_index.wrapping_sub(wanted as u32);
        for (offset, slot) in out.iter_mut().enumerate() {
            *slot = if offset < wanted {
                let bucket = oldest.wrapping_add(offset as u32);
                self.ring.value(bucket).map_or(0, |value| value.load(Ordering::Relaxed))
            } else {
                0
            };
        }
        write_index
    }

    /// The bucket counter the writer has published, without copying anything.
    pub fn write_index(&self) -> u32 { self.ring.write_index.load(Ordering::Relaxed) }

    /// Buckets one ring holds.
    pub fn capacity(&self) -> usize { self.ring.values.len() }
}

impl core::fmt::Debug for TapWriter {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("TapWriter").field("write_index", &self.write_index()).finish()
    }
}

impl core::fmt::Debug for TapReader {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("TapReader")
            .field("buckets", &self.capacity())
            .field("write_index", &self.write_index())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Write `count` buckets of `value_of(bucket)` starting at `first`, then commit.
    fn fill(writer: &TapWriter, first: u32, count: u32, value_of: impl Fn(u32) -> i16) {
        for offset in 0..count {
            writer.write(first + offset, value_of(first + offset));
        }
        writer.commit(first + count);
    }

    #[test]
    fn a_fresh_ring_reads_as_silence() {
        let (writer, reader) = TapRing::new();
        assert_eq!(reader.capacity(), TAP_RING_BUCKETS);
        assert_eq!(writer.write_index(), 0);

        let mut window = vec![7i16; 8];
        assert_eq!(reader.latest(&mut window), 0);
        assert_eq!(window, vec![0i16; 8], "an unwritten ring is zeroed, not stale");
    }

    #[test]
    fn latest_returns_the_newest_buckets_oldest_first() {
        let (writer, reader) = TapRing::new();
        fill(&writer, 0, 100, |bucket| bucket as i16);

        let mut window = vec![0i16; 4];
        assert_eq!(reader.latest(&mut window), 100);
        assert_eq!(window, vec![96, 97, 98, 99], "the newest four, in the order they were written");
    }

    #[test]
    fn a_window_longer_than_the_ring_is_zero_padded_rather_than_repeated() {
        let (writer, reader) = TapRing::new();
        fill(&writer, 0, TAP_RING_BUCKETS as u32, |bucket| (bucket % 1000) as i16);

        let mut window = vec![-1i16; TAP_RING_BUCKETS + 3];
        assert_eq!(reader.latest(&mut window), TAP_RING_BUCKETS as u32);
        assert_eq!(window.get(..TAP_RING_BUCKETS).map(|slice| slice.len()), Some(TAP_RING_BUCKETS));
        assert_eq!(window.get(TAP_RING_BUCKETS..), Some(&[0i16, 0, 0][..]), "no bucket is served twice");
    }

    #[test]
    fn the_bucket_counter_wraps_into_the_ring_without_a_gap() {
        let (writer, reader) = TapRing::new();
        // Two and a bit laps, so the counter is well past the ring length.
        let written = TAP_RING_BUCKETS as u32 * 2 + 37;
        fill(&writer, 0, written, |bucket| (bucket % 251) as i16);

        let mut window = vec![0i16; 8];
        assert_eq!(reader.latest(&mut window), written);
        let expected: Vec<i16> = (written - 8..written).map(|bucket| (bucket % 251) as i16).collect();
        assert_eq!(window, expected, "the newest eight, across the wrap");
    }

    #[test]
    fn the_counter_survives_wrapping_past_u32_max() {
        let (writer, reader) = TapRing::new();
        let first = u32::MAX - 3;
        for offset in 0..8u32 {
            writer.write(first.wrapping_add(offset), offset as i16 + 1);
        }
        writer.commit(first.wrapping_add(8));

        let mut window = vec![0i16; 8];
        assert_eq!(reader.latest(&mut window), first.wrapping_add(8));
        assert_eq!(window, vec![1, 2, 3, 4, 5, 6, 7, 8], "wrapping arithmetic, not a panic");
    }

    #[test]
    fn accumulate_sums_into_a_bucket_and_saturates() {
        let (writer, reader) = TapRing::new();
        writer.write(0, 0);
        writer.accumulate(0, 20_000);
        writer.accumulate(0, 20_000);
        writer.accumulate(0, -100);
        writer.commit(1);

        let mut window = vec![0i16; 1];
        reader.latest(&mut window);
        assert_eq!(window.first().copied(), Some(i16::MAX - 100), "two loud voices saturate rather than wrap");
    }

    #[test]
    fn a_reader_racing_the_writer_gets_a_torn_window_rather_than_a_panic() {
        let (writer, reader) = TapRing::new();
        fill(&writer, 0, TAP_RING_BUCKETS as u32, |_| 1);

        // Interleave a copy with writes the way two threads would, by advancing the
        // writer between the reader's own reads. Nothing here may panic, and the window
        // may legitimately contain values from either side of the advance.
        let mut window = vec![0i16; 64];
        let mut cursor = TAP_RING_BUCKETS as u32;
        for round in 0..64 {
            fill(&writer, cursor, 32, |_| 2);
            cursor += 32;
            let seen = reader.latest(&mut window);
            assert_eq!(seen, cursor, "the reader sees whatever index the writer last published");
            assert!(window.iter().all(|value| *value == 1 || *value == 2), "round {round}: only written values");
        }
    }
}
