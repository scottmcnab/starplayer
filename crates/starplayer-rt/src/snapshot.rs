//! [`SnapshotPublisher`] / [`SnapshotReader`] — how the audio thread hands a UI a
//! *coherent* copy of its scalar state (architecture §9(a)).
//!
//! The payload is one `Copy` value of a couple of kilobytes. The audio thread must never
//! block on the reader, and the reader must never see half of one snapshot and half of
//! another: a row number from one tick paired with a note from the next renders wrong.
//!
//! # Why this is a bounded channel and not a triple buffer (M1-B6, research point 1)
//!
//! Architecture §11 lists a triple buffer here, and a triple buffer is the textbook
//! answer — three slots, an atomic swap of the "most recent" index, wait-free on both
//! sides. It is not implementable in this crate, and the reason is the same one
//! architecture §8.1 already gives for the SPSC ring:
//!
//! > A triple buffer's writer holds `&mut` to one slot while the reader holds `&` to
//! > another, disjoint by an invariant that lives in an index atomic and that the borrow
//! > checker cannot see. `core` has no safe primitive granting interior mutability over
//! > an arbitrary `T` shared between threads, so the three slots must be
//! > `UnsafeCell<T>` — and this crate is `#![forbid(unsafe_code)]`.
//!
//! The escape used for the SPSC ring was to take an audited dependency. There is no
//! equivalent for the triple buffer: the `triple_buffer` crate is `std`-only (it names
//! `std::sync::atomic` and `std::cell`, and has no `no_std` feature), so it cannot serve
//! the bare-metal target CI checks on every commit. Architecture §11.2 already flags it
//! "std hosts only". Rejected.
//!
//! `ringbuf`'s own `push_overwrite` would give the "newest wins" behaviour directly, but
//! its documentation is explicit that it *"requires exclusive access to the ring buffer,
//! so to perform it concurrently you need to guard the ring buffer with a mutex"* — it
//! takes `&mut Rb`, not the producer half, and a lock on the audio thread is exactly what
//! this module exists to avoid.
//!
//! So: **a bounded SPSC channel of whole snapshots**, [`DEFAULT_SNAPSHOT_DEPTH`] deep —
//! the same three copies a triple buffer would have allocated. The writer pushes one
//! complete value per tick and the reader drains to the newest, so a snapshot is
//! *atomic by construction*: it is moved into the ring in one piece and taken out in one
//! piece, and no reader can observe a partially written one. Two consequences, both
//! deliberate:
//!
//! * **A publish is dropped when the ring is full**, rather than blocking or overwriting.
//!   A reader that has stalled for three ticks (~60 ms at 50 Hz) loses a frame it was
//!   never going to draw; the next tick publishes again.
//!   [`SnapshotPublisher::publishes_dropped`] counts them so the loss is visible rather
//!   than silent.
//! * **The oldest is kept, not the newest.** An SPSC producer cannot pop, so the value
//!   that loses is the one being written. The reader then drains at most
//!   [`SnapshotPublisher::depth`] stale snapshots on its next poll and lands on the
//!   newest of them, which is at worst one depth of ticks old.
//!
//! It also needs *less* than a triple buffer would: an SPSC ring only loads and stores
//! its two index atomics, so — unlike the triple buffer's `AtomicUsize::swap` — there is
//! no compare-and-swap anywhere on this path, and `riscv32imc-unknown-none-elf` does not
//! have to reach for `portable-atomic/critical-section` to publish telemetry.
//!
//! The scope rings of architecture §9(b) arrived in M3-D6 and are a different shape
//! entirely, as expected: lossy per-channel taps with a `Relaxed` write index and no
//! coherence guarantee at all. See [`crate::tap`].

use crate::spsc::{Consumer, Producer, channel};

/// Snapshots in flight before the writer starts dropping them.
///
/// Three, matching what a triple buffer would have allocated. The writer publishes once
/// per tracker tick — roughly 50 Hz — and a UI polls at frame rate, so the ring is
/// normally empty or holds one value.
pub const DEFAULT_SNAPSHOT_DEPTH: usize = 3;

/// Create a snapshot channel and split it into its two halves.
///
/// `initial` is what [`SnapshotReader::latest`] answers before anything has been
/// published. **The buffer is allocated exactly once, here**; neither half allocates
/// again, so [`SnapshotPublisher::publish`] is safe to call from the audio thread.
pub fn snapshot_channel<T: Copy>(depth: usize, initial: T) -> (SnapshotPublisher<T>, SnapshotReader<T>) {
    let (producer, consumer) = channel::<T>(depth);
    (SnapshotPublisher { producer, publishes_dropped: 0 }, SnapshotReader { consumer, latest: initial })
}

/// The writing half. Exactly one thread may hold it, and that thread is the audio thread.
///
/// `T: Copy` is not a convenience bound — it is the guarantee that a snapshot left
/// unread in the ring has no destructor, so nothing this half touches can call `free()`
/// inside the audio callback.
pub struct SnapshotPublisher<T: Copy> {
    producer: Producer<T>,
    publishes_dropped: u32,
}

impl<T: Copy> SnapshotPublisher<T> {
    /// Publish `snapshot`, or count a drop if the reader has not kept up.
    ///
    /// Wait-free and allocation-free. Returns whether the snapshot was accepted.
    pub fn publish(&mut self, snapshot: T) -> bool {
        if self.producer.push(snapshot).is_err() {
            self.publishes_dropped = self.publishes_dropped.saturating_add(1);
            return false;
        }
        true
    }

    /// How many publishes have been dropped because the reader was behind.
    ///
    /// Saturating, and never reset: it is a diagnostic, and a host that wants a rate
    /// takes differences.
    pub const fn publishes_dropped(&self) -> u32 { self.publishes_dropped }

    /// How many snapshots the ring holds.
    pub fn depth(&self) -> usize { self.producer.capacity() }

    /// Whether the next [`SnapshotPublisher::publish`] will be dropped.
    pub fn is_full(&self) -> bool { self.producer.is_full() }
}

/// The reading half. Exactly one thread may hold it, and that thread is the UI's.
pub struct SnapshotReader<T: Copy> {
    consumer: Consumer<T>,
    latest: T,
}

impl<T: Copy> SnapshotReader<T> {
    /// Drain to the newest published snapshot and return it.
    ///
    /// Bounded by the ring's depth, so this cannot spin however fast the writer runs.
    /// With nothing new published it returns the previous one unchanged, which is what a
    /// UI polling faster than the tick rate wants.
    ///
    /// `&mut self` rather than `&self` because draining an SPSC consumer moves values out
    /// of it. The half is single-consumer by construction, so this costs a caller nothing
    /// it did not already have.
    pub fn read(&mut self) -> &T {
        while let Some(snapshot) = self.consumer.pop() {
            self.latest = snapshot;
        }
        &self.latest
    }

    /// The last snapshot [`SnapshotReader::read`] returned, without draining.
    pub const fn latest(&self) -> &T { &self.latest }

    /// Whether a newer snapshot is waiting.
    pub fn has_pending(&self) -> bool { !self.consumer.is_empty() }

    /// How many snapshots the ring holds.
    pub fn depth(&self) -> usize { self.consumer.capacity() }
}

impl<T: Copy> core::fmt::Debug for SnapshotPublisher<T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("SnapshotPublisher")
            .field("depth", &self.depth())
            .field("publishes_dropped", &self.publishes_dropped)
            .finish()
    }
}

impl<T: Copy> core::fmt::Debug for SnapshotReader<T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("SnapshotReader")
            .field("depth", &self.depth())
            .field("has_pending", &self.has_pending())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload wide enough that a torn read would be obvious, with every field derived
    /// from the first one.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
    struct Coherent {
        sequence: u64,
        derived: [u64; 16],
    }

    impl Coherent {
        fn of(sequence: u64) -> Coherent { Coherent { sequence, derived: [sequence; 16] } }
        fn is_coherent(&self) -> bool { self.derived.iter().all(|value| *value == self.sequence) }
    }

    #[test]
    fn a_reader_that_keeps_up_sees_every_snapshot() {
        let (mut publisher, mut reader) = snapshot_channel(DEFAULT_SNAPSHOT_DEPTH, Coherent::default());
        assert_eq!(publisher.depth(), 3);
        assert_eq!(reader.latest(), &Coherent::default(), "nothing published yet");

        for sequence in 1..=100u64 {
            assert!(publisher.publish(Coherent::of(sequence)), "the reader is draining every time");
            assert!(reader.has_pending());
            assert_eq!(reader.read(), &Coherent::of(sequence));
        }
        assert_eq!(publisher.publishes_dropped(), 0);
    }

    #[test]
    fn a_stalled_reader_costs_publishes_rather_than_the_writer() {
        let (mut publisher, mut reader) = snapshot_channel(3, Coherent::default());

        for sequence in 1..=3u64 {
            assert!(publisher.publish(Coherent::of(sequence)));
        }
        assert!(publisher.is_full());
        assert!(!publisher.publish(Coherent::of(4)), "the writer never blocks; it drops");
        assert!(!publisher.publish(Coherent::of(5)));
        assert_eq!(publisher.publishes_dropped(), 2);

        assert_eq!(reader.read(), &Coherent::of(3), "the reader drains to the newest it was given");
        assert!(publisher.publish(Coherent::of(6)), "and the ring is usable again");
        assert_eq!(reader.read(), &Coherent::of(6));
    }

    #[test]
    fn reading_with_nothing_pending_repeats_the_last_snapshot() {
        let (mut publisher, mut reader) = snapshot_channel(3, Coherent::of(7));
        assert_eq!(reader.read(), &Coherent::of(7));
        assert!(publisher.publish(Coherent::of(9)));
        assert_eq!(reader.read(), &Coherent::of(9));
        assert!(!reader.has_pending());
        assert_eq!(reader.read(), &Coherent::of(9), "a UI polling faster than the tick rate keeps drawing the same frame");
    }

    #[test]
    fn every_snapshot_that_arrives_is_internally_consistent() {
        let (mut publisher, mut reader) = snapshot_channel(DEFAULT_SNAPSHOT_DEPTH, Coherent::default());
        for sequence in 1..=1_000u64 {
            publisher.publish(Coherent::of(sequence));
            if sequence % 7 == 0 {
                assert!(reader.read().is_coherent(), "a snapshot is moved in and out whole");
            }
        }
    }

    #[test]
    fn a_zero_depth_channel_still_carries_one_snapshot() {
        let (mut publisher, mut reader) = snapshot_channel(0, Coherent::default());
        assert_eq!(publisher.depth(), 1, "the SPSC ring raises a zero capacity to one");
        assert!(publisher.publish(Coherent::of(1)));
        assert_eq!(reader.read(), &Coherent::of(1));
    }
}
