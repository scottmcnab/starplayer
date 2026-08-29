//! The garbage channel: how a value the audio thread has finished with gets destroyed
//! somewhere else (architecture §8).
//!
//! Dropping the last `Arc<Module>` on the audio thread calls `free()`, and `free()` takes
//! a lock in every general-purpose allocator. That is an unbounded stall in the middle of
//! an audio callback, and it is the single easiest real-time-safety mistake to make,
//! because nothing about `self.module = new_module;` looks like a deallocation.
//!
//! So the audio thread never overwrites a retired handle: it [`retire`](GarbageChannel::retire)s
//! it, which is a wait-free push onto an SPSC ring, and the control thread
//! [`collect`](GarbageCollector::collect)s and drops it whenever it next looks.

use crate::spsc::{self, Consumer, Producer};

/// Create a garbage channel of `capacity` retired values.
///
/// The [`GarbageChannel`] goes to the audio thread and the [`GarbageCollector`] stays with
/// whoever created the values.
pub fn garbage_channel<T>(capacity: usize) -> (GarbageChannel<T>, GarbageCollector<T>) {
    let (producer, consumer) = spsc::channel(capacity);
    (GarbageChannel { producer }, GarbageCollector { consumer })
}

/// The audio thread's end: values go in, nothing is ever dropped here.
#[derive(Debug)]
pub struct GarbageChannel<T> {
    producer: Producer<T>,
}

impl<T> GarbageChannel<T> {
    /// Hand `value` to the control thread to be destroyed. Wait-free, allocation-free.
    ///
    /// Returns `Err(value)` if the channel is full. **The caller must handle that**, and
    /// the honest answer is usually "drop it and raise a warning": the alternative is an
    /// unbounded queue, which is an allocation on the audio thread. It cannot happen in
    /// normal use — the channel holds many more retired handles than a control thread
    /// could realistically fall behind by — so a breach is a diagnostic, not a design
    /// point. The value is handed back rather than dropped here so that the decision is
    /// made at the call site rather than silently by this function.
    pub fn retire(&mut self, value: T) -> Result<(), T> { self.producer.push(value) }

    /// How many retired values the channel can hold.
    pub fn capacity(&self) -> usize { self.producer.capacity() }

    /// Whether the next [`GarbageChannel::retire`] will fail.
    pub fn is_full(&self) -> bool { self.producer.is_full() }
}

/// The control thread's end: values come out and are dropped here.
#[derive(Debug)]
pub struct GarbageCollector<T> {
    consumer: Consumer<T>,
}

impl<T> GarbageCollector<T> {
    /// Take one retired value, so the caller can inspect it before it is dropped.
    pub fn collect(&mut self) -> Option<T> { self.consumer.pop() }

    /// Drop everything waiting, and report how much that was.
    ///
    /// This is the call a host makes on its control thread — from a UI frame, a worker
    /// task or a `setInterval` — and it is the only place the retired modules' memory is
    /// actually returned to the allocator.
    pub fn collect_all(&mut self) -> usize { self.consumer.drain_bounded(usize::MAX, drop) }

    /// How many retired values are waiting.
    pub fn pending(&self) -> usize { self.consumer.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use core::cell::Cell;

    /// Counts its own destruction, so a test can prove *where* a value was dropped rather
    /// than merely that it was.
    struct DropCounter<'counter> {
        drops: &'counter Cell<u32>,
    }

    impl Drop for DropCounter<'_> {
        fn drop(&mut self) { self.drops.set(self.drops.get() + 1); }
    }

    #[test]
    fn a_retired_value_is_not_dropped_by_the_thread_that_retired_it() {
        let drops = Cell::new(0u32);
        let (mut channel, mut collector) = garbage_channel::<DropCounter<'_>>(4);

        // The "audio thread" retires it.
        assert!(channel.retire(DropCounter { drops: &drops }).is_ok());
        assert_eq!(drops.get(), 0, "retiring must not run the destructor");
        assert_eq!(collector.pending(), 1);

        // The "control thread" collects it.
        assert_eq!(collector.collect_all(), 1);
        assert_eq!(drops.get(), 1);
        assert_eq!(collector.pending(), 0);
    }

    #[test]
    fn a_full_channel_hands_the_value_back_instead_of_freeing_it() {
        let drops = Cell::new(0u32);
        let (mut channel, mut collector) = garbage_channel::<DropCounter<'_>>(1);
        assert!(channel.retire(DropCounter { drops: &drops }).is_ok());
        assert!(channel.is_full());

        let rejected = channel.retire(DropCounter { drops: &drops });
        assert!(rejected.is_err(), "a full channel refuses rather than blocking");
        assert_eq!(drops.get(), 0, "and refusing has not dropped anything yet");
        drop(rejected);
        assert_eq!(drops.get(), 1, "the caller owns the rejected value and decides");

        assert_eq!(collector.collect_all(), 1);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn a_collected_value_can_be_inspected_before_it_dies() {
        let (mut channel, mut collector) = garbage_channel::<Box<u32>>(2);
        assert!(channel.retire(Box::new(41)).is_ok());
        assert_eq!(collector.collect().as_deref(), Some(&41));
        assert_eq!(collector.pending(), 0);
    }
}
