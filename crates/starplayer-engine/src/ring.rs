//! [`OutputRing`] — the adapter between the engine's fixed render quantum and whatever
//! block size the host happens to ask for.
//!
//! # What it holds, and why (task A3, research point 1)
//!
//! **Converted host samples, not accumulator frames.**
//!
//! The alternative — buffering pre-conversion accumulator frames and converting on the
//! way out — would let a host request a different sample format on every call, which is
//! not a thing any real host does: the format is fixed when the stream is opened. Against
//! that non-benefit it has one decisive cost. Output conversion is stateful the moment
//! dithering arrives in M1, and converting on the way out means converting whatever
//! ragged 3-, 17- or 411-frame segment the host asked for, so the dither sequence — and
//! therefore the output — would depend on the host's buffer size. That is precisely the
//! silent failure architecture §1.4 and this task exist to make impossible.
//!
//! Holding converted samples also makes the common path a `copy_from_slice` with no
//! per-call arithmetic, and it is smaller for every host format narrower than the
//! accumulator.
//!
//! # Why it is not circular
//!
//! Capacity is exactly one render quantum, and the engine only ever refills the buffer
//! when it has been fully drained, so the wrap-around case a circular buffer exists to
//! handle cannot arise. The implementation is therefore a fixed buffer plus a read
//! cursor: same behaviour, no modular arithmetic, nothing to get wrong. It keeps the name
//! the architecture gives it because that is the role it plays.

use alloc::boxed::Box;
use alloc::vec;

/// A one-quantum staging buffer of converted host samples.
#[derive(Debug)]
pub struct OutputRing<Sample: Copy + Default> {
    samples: Box<[Sample]>,
    read_cursor: usize,
}

impl<Sample: Copy + Default> OutputRing<Sample> {
    /// A ring holding `capacity` interleaved samples, allocated once. It starts empty, so
    /// the first [`OutputRing::drain_into`] renders.
    pub fn new(capacity: usize) -> OutputRing<Sample> {
        OutputRing { samples: vec![Sample::default(); capacity].into_boxed_slice(), read_cursor: capacity }
    }

    /// Samples still waiting to be handed to the host.
    pub fn remaining(&self) -> usize { self.samples.len().saturating_sub(self.read_cursor) }

    /// Whether the host has taken everything, so the engine owes it another quantum.
    pub fn is_empty(&self) -> bool { self.remaining() == 0 }

    /// Hand the whole buffer to `fill`, then mark it full.
    ///
    /// `fill` must write every sample; it is called with the buffer's previous contents,
    /// which is why the engine's conversion step writes rather than accumulates.
    pub fn refill_with(&mut self, fill: impl FnOnce(&mut [Sample])) {
        fill(&mut self.samples);
        self.read_cursor = 0;
    }

    /// Copy as much as fits into `destination`, and report how much that was.
    pub fn drain_into(&mut self, destination: &mut [Sample]) -> usize {
        let count = self.remaining().min(destination.len());
        let end = self.read_cursor.saturating_add(count);
        let (Some(source), Some(target)) = (self.samples.get(self.read_cursor..end), destination.get_mut(..count)) else {
            return 0;
        };
        target.copy_from_slice(source);
        self.read_cursor = end;
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_ring_is_empty() {
        let ring: OutputRing<f32> = OutputRing::new(4);
        assert!(ring.is_empty());
        assert_eq!(ring.remaining(), 0);
    }

    #[test]
    fn a_refilled_ring_drains_in_order_across_several_calls() {
        let mut ring: OutputRing<i16> = OutputRing::new(4);
        ring.refill_with(|samples| samples.copy_from_slice(&[10, 20, 30, 40]));
        assert_eq!(ring.remaining(), 4);

        let mut first = [0i16; 3];
        assert_eq!(ring.drain_into(&mut first), 3);
        assert_eq!(first, [10, 20, 30]);

        let mut second = [0i16; 3];
        assert_eq!(ring.drain_into(&mut second), 1, "only one sample was left");
        assert_eq!(second, [40, 0, 0]);
        assert!(ring.is_empty());
        assert_eq!(ring.drain_into(&mut second), 0, "draining an empty ring is a no-op");
    }

    #[test]
    fn draining_more_than_the_ring_holds_is_not_an_error() {
        let mut ring: OutputRing<i16> = OutputRing::new(2);
        ring.refill_with(|samples| samples.copy_from_slice(&[1, 2]));
        let mut destination = [0i16; 8];
        assert_eq!(ring.drain_into(&mut destination), 2);
        assert_eq!(destination, [1, 2, 0, 0, 0, 0, 0, 0]);
    }
}
