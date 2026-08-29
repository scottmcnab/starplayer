//! [`Frame`] — absolute output-frame position.

/// An absolute position on the engine's output timeline, counted in output frames from
/// the moment the engine was created.
///
/// # Invariants
///
/// * **Monotonic.** A `Frame` never moves backwards. Seeking changes what the sequencer
///   plays, not what the clock reads.
/// * **Engine-owned.** There is exactly one authoritative frame counter, held by the
///   engine, and every event source reports *absolute* frames against it rather than
///   keeping its own elapsed-time bookkeeping (architecture §3). That is what stops
///   sources from slowly desyncing from one another.
/// * **`u64` is enough.** 2^64 frames is roughly 12 million years at 48 kHz, so overflow
///   is not a case that needs handling — but every operation here saturates anyway,
///   because `render()` may not panic (architecture §8).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Frame(pub u64);

impl Frame {
    /// The start of the timeline.
    pub const ZERO: Frame = Frame(0);

    /// The end of the timeline. Used as "no event pending" where an `Option` would cost
    /// a branch in a hot loop.
    pub const MAX: Frame = Frame(u64::MAX);

    /// The raw frame index.
    pub const fn get(self) -> u64 { self.0 }

    /// Move forward by `frames`, saturating.
    pub const fn saturating_add(self, frames: u64) -> Frame { Frame(self.0.saturating_add(frames)) }

    /// Move backwards by `frames`, saturating at [`Frame::ZERO`].
    ///
    /// Present for computing windows relative to a known position, not for rewinding the
    /// engine clock — see the monotonicity invariant.
    pub const fn saturating_sub(self, frames: u64) -> Frame { Frame(self.0.saturating_sub(frames)) }

    /// Frames from `self` forward to `later`, or `0` if `later` is not in the future.
    ///
    /// This is the render loop's gap computation: the number of frames that may be
    /// rendered before the next event has to be dispatched.
    pub const fn frames_until(self, later: Frame) -> u64 { later.0.saturating_sub(self.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_arithmetic_saturates() {
        assert_eq!(Frame::MAX.saturating_add(1), Frame::MAX);
        assert_eq!(Frame::ZERO.saturating_sub(1), Frame::ZERO);
        assert_eq!(Frame(10).saturating_add(5), Frame(15));
        assert_eq!(Frame(10).saturating_sub(5), Frame(5));
    }

    #[test]
    fn frames_until_never_goes_negative() {
        assert_eq!(Frame(100).frames_until(Frame(228)), 128);
        assert_eq!(Frame(228).frames_until(Frame(100)), 0, "a past frame yields a zero gap, not a wrap");
        assert_eq!(Frame(228).frames_until(Frame(228)), 0);
    }
}
