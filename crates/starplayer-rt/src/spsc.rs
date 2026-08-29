//! The single-producer / single-consumer ring: how the control thread talks to the audio
//! thread, and how the audio thread talks back (architecture §8).

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer as _, Observer as _, Producer as _, Split as _};
use ringbuf::{HeapCons, HeapProd};

/// The smallest ring that can hold anything. A capacity of zero would be a ring that
/// silently drops everything, which is never what a caller meant.
const MINIMUM_CAPACITY: usize = 1;

/// Create a ring of `capacity` values and split it into its two halves.
///
/// **The buffer is allocated exactly once, here.** Neither half ever allocates again, so
/// [`Producer::push`] and [`Consumer::pop`] are safe to call from the audio thread.
///
/// A capacity of zero is raised to one rather than rejected: `render()` may not panic and
/// a `Result` on a constructor that cannot meaningfully fail would be noise at every call
/// site.
pub fn channel<T>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let (producer, consumer) = HeapRb::<T>::new(capacity.max(MINIMUM_CAPACITY)).split();
    (Producer { inner: producer }, Consumer { inner: consumer })
}

/// The writing half of an SPSC ring. Exactly one thread may hold it.
pub struct Producer<T> {
    inner: HeapProd<T>,
}

impl<T> Producer<T> {
    /// Publish `value`, or hand it back if the ring is full.
    ///
    /// Wait-free, and **never drops `value` itself**: a full ring returns it so the caller
    /// decides what happens next. That matters most on the audio side, where dropping a
    /// value may run a deallocator — see [`crate::garbage`].
    pub fn push(&mut self, value: T) -> Result<(), T> { self.inner.try_push(value) }

    /// How many values the ring can hold.
    pub fn capacity(&self) -> usize { self.inner.capacity().get() }

    /// How many values are waiting to be read.
    pub fn len(&self) -> usize { self.inner.occupied_len() }

    /// Whether nothing is waiting to be read.
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Whether the next [`Producer::push`] will fail.
    pub fn is_full(&self) -> bool { self.inner.is_full() }
}

/// The reading half of an SPSC ring. Exactly one thread may hold it.
pub struct Consumer<T> {
    inner: HeapCons<T>,
}

impl<T> Consumer<T> {
    /// Take the oldest value, or `None` if the ring is empty. Wait-free.
    pub fn pop(&mut self) -> Option<T> { self.inner.try_pop() }

    /// How many values the ring can hold.
    pub fn capacity(&self) -> usize { self.inner.capacity().get() }

    /// How many values are waiting to be read.
    pub fn len(&self) -> usize { self.inner.occupied_len() }

    /// Whether nothing is waiting to be read.
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// Take at most `limit` values, passing each to `consume`, and report how many were
    /// taken.
    ///
    /// The bound is the point: an unbounded drain inside `render()` would let a control
    /// thread that floods the ring stall the audio callback for as long as it kept
    /// writing.
    pub fn drain_bounded(&mut self, limit: usize, mut consume: impl FnMut(T)) -> usize {
        let mut taken = 0usize;
        while taken < limit {
            let Some(value) = self.pop() else { break };
            consume(value);
            taken = taken.saturating_add(1);
        }
        taken
    }
}

impl<T> core::fmt::Debug for Producer<T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Producer").field("len", &self.len()).field("capacity", &self.capacity()).finish()
    }
}

impl<T> core::fmt::Debug for Consumer<T> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("Consumer").field("len", &self.len()).field("capacity", &self.capacity()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn values_come_out_in_the_order_they_went_in() {
        let (mut producer, mut consumer) = channel::<u32>(4);
        assert_eq!(producer.capacity(), 4);
        assert!(consumer.is_empty());

        for value in 1..=4 {
            assert_eq!(producer.push(value), Ok(()));
        }
        assert!(producer.is_full());
        assert_eq!(consumer.len(), 4);

        let drained: Vec<u32> = core::iter::from_fn(|| consumer.pop()).collect();
        assert_eq!(drained, alloc::vec![1, 2, 3, 4]);
        assert!(consumer.is_empty());
        assert_eq!(consumer.pop(), None);
    }

    #[test]
    fn a_full_ring_hands_the_value_back_rather_than_dropping_it() {
        let (mut producer, mut consumer) = channel::<u32>(2);
        assert_eq!(producer.push(1), Ok(()));
        assert_eq!(producer.push(2), Ok(()));
        assert_eq!(producer.push(3), Err(3), "the rejected value comes back to its owner");

        assert_eq!(consumer.pop(), Some(1));
        assert_eq!(producer.push(3), Ok(()), "one slot freed, one slot usable");
    }

    #[test]
    fn a_zero_capacity_ring_still_holds_one_value() {
        let (mut producer, mut consumer) = channel::<u32>(0);
        assert_eq!(producer.capacity(), 1);
        assert_eq!(producer.push(9), Ok(()));
        assert_eq!(consumer.pop(), Some(9));
    }

    #[test]
    fn draining_is_bounded() {
        let (mut producer, mut consumer) = channel::<u32>(8);
        for value in 0..8 {
            assert_eq!(producer.push(value), Ok(()));
        }
        let mut seen = Vec::new();
        assert_eq!(consumer.drain_bounded(3, |value| seen.push(value)), 3);
        assert_eq!(seen, alloc::vec![0, 1, 2]);
        assert_eq!(consumer.len(), 5, "the rest waits for the next pass");
        assert_eq!(consumer.drain_bounded(100, |value| seen.push(value)), 5, "a limit past the end is not an error");
    }

    #[test]
    fn the_ring_carries_values_that_are_not_copy() {
        use alloc::boxed::Box;
        let (mut producer, mut consumer) = channel::<Box<u64>>(2);
        assert!(producer.push(Box::new(7)).is_ok());
        assert_eq!(consumer.pop().as_deref(), Some(&7));
    }
}
