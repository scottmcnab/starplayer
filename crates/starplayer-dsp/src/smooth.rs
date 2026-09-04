//! [`SmoothedParam`] — an effect parameter that moves to its new value over a fixed number
//! of frames instead of jumping (M7 master-plan decision 4, H1 deliverable 4).
//!
//! # Why this is not just [`GainRamp`](crate::ramp::GainRamp)
//!
//! It *is* a [`GainRamp`](crate::ramp::GainRamp) inside, deliberately: the discipline is
//! the point, not a new interpolator. What differs is the length. `RAMP_FRAMES` is 64
//! frames — 1.5 ms — because a retriggered voice has to reach its new gain before the next
//! tick. A filter cutoff or a delay mix moving that fast is itself an audible artefact, so
//! an effect parameter takes [`SMOOTH_FRAMES`], two whole render quanta.
//!
//! # Why it is block-size independent
//!
//! [`SmoothedParam::advance`] moves the value by exactly one output frame and knows
//! nothing about the block it is called from, and the last frame of a ramp is *assigned*
//! the target rather than incremented onto it. A parameter is therefore a function of
//! frames elapsed since the change, never of where a host block boundary fell — which is
//! what architecture §1.4's determinism invariant requires, and what
//! `crates/starplayer-engine/tests/block_size_determinism.rs` pins with an insert active.
//!
//! The unit is the caller's. A gain smooths in centi-decibels and converts to a linear
//! gain per frame; a cutoff would smooth in cents. Smoothing in the *parameter's* units
//! rather than in the cooked coefficient is what keeps the cooking table-driven.

use crate::ramp::GainRamp;

/// Frames an effect parameter takes to reach a new value unless the caller says otherwise:
/// two whole render quanta, about 5.8 ms at 44.1 kHz.
pub const SMOOTH_FRAMES: u32 = 256;

/// An integer effect parameter that ramps linearly to its target.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SmoothedParam {
    ramp: GainRamp,
}

impl SmoothedParam {
    /// A parameter already at `value` with nothing pending.
    pub const fn steady(value: i32) -> SmoothedParam { SmoothedParam { ramp: GainRamp::steady(value) } }

    /// Head for `value` over the next `frames` output frames.
    ///
    /// Re-stating the target the parameter is already heading for is a no-op, for the same
    /// reason [`GainRamp::glide_to`] makes it one: a host that re-sends a slider's value
    /// every frame must not restart the ramp, or the value would depend on how often the
    /// host sent it.
    pub const fn set_target(&mut self, value: i32, frames: u32) { self.ramp.glide_to(value, frames); }

    /// The value this frame is processed with.
    pub const fn current(&self) -> i32 { self.ramp.current() }

    /// Where the parameter is heading.
    pub const fn target(&self) -> i32 { self.ramp.target() }

    /// Whether a ramp is in progress.
    pub const fn is_moving(&self) -> bool { self.ramp.is_ramping() }

    /// Advance by one output frame and return the value to process that frame with.
    pub const fn advance(&mut self) -> i32 { self.ramp.advance() }

    /// Land on the target immediately.
    ///
    /// What [`Insert::reset`](crate::insert::Insert::reset) does to a parameter that was
    /// still moving: a reset clears delay lines and envelopes, and a half-finished ramp is
    /// an envelope.
    pub const fn snap(&mut self) {
        let target = self.ramp.target();
        self.ramp.jump_to(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn a_steady_parameter_never_moves() {
        let mut parameter = SmoothedParam::steady(-1_200);
        assert!(!parameter.is_moving());
        for _ in 0..10 {
            assert_eq!(parameter.advance(), -1_200);
        }
    }

    #[test]
    fn a_parameter_reaches_its_target_on_exactly_the_last_frame() {
        let mut parameter = SmoothedParam::steady(0);
        parameter.set_target(-6_000, SMOOTH_FRAMES);
        for frame in 1..SMOOTH_FRAMES {
            let value = parameter.advance();
            assert!(value < 0 && value > -6_000, "frame {frame} left the parameter at {value}");
            assert!(parameter.is_moving(), "frame {frame} ended the ramp early");
        }
        assert_eq!(parameter.advance(), -6_000, "the last frame lands exactly on the target");
        assert!(!parameter.is_moving());
        assert_eq!(parameter.advance(), -6_000, "and it stays there");
    }

    #[test]
    fn a_change_that_does_not_divide_evenly_still_lands_exactly() {
        let mut parameter = SmoothedParam::steady(0);
        parameter.set_target(SMOOTH_FRAMES as i32 * 5 + 7, SMOOTH_FRAMES);
        for _ in 0..SMOOTH_FRAMES {
            parameter.advance();
        }
        assert_eq!(parameter.current(), SMOOTH_FRAMES as i32 * 5 + 7);
    }

    #[test]
    fn restating_the_current_target_does_not_restart_the_ramp() {
        let mut parameter = SmoothedParam::steady(0);
        parameter.set_target(1_200, SMOOTH_FRAMES);
        for _ in 0..100 {
            parameter.advance();
        }
        let midway = parameter;
        parameter.set_target(1_200, SMOOTH_FRAMES);
        assert_eq!(parameter, midway, "a repeated target must not restart the ramp");
    }

    #[test]
    fn snapping_lands_on_the_target_at_once() {
        let mut parameter = SmoothedParam::steady(0);
        parameter.set_target(-3_000, SMOOTH_FRAMES);
        parameter.advance();
        assert!(parameter.is_moving());
        parameter.snap();
        assert!(!parameter.is_moving());
        assert_eq!(parameter.current(), -3_000);
        assert_eq!(parameter.advance(), -3_000);
    }

    #[test]
    fn however_it_is_split_the_sequence_is_the_same() {
        let sequence = |chunk: usize| {
            let mut parameter = SmoothedParam::steady(-6_000);
            parameter.set_target(1_200, SMOOTH_FRAMES);
            let mut values = Vec::new();
            let mut done = 0;
            let total = SMOOTH_FRAMES as usize * 2;
            while done < total {
                let count = chunk.min(total - done);
                for _ in 0..count {
                    values.push(parameter.advance());
                }
                done += count;
            }
            values
        };
        let whole = sequence(SMOOTH_FRAMES as usize * 2);
        for chunk in [1usize, 3, 7, 128, 129] {
            assert_eq!(sequence(chunk), whole, "chunk {chunk} changed the parameter");
        }
    }

    #[test]
    fn a_zero_length_ramp_is_a_jump() {
        let mut parameter = SmoothedParam::steady(10);
        parameter.set_target(20, 0);
        assert_eq!(parameter.current(), 20);
        assert!(!parameter.is_moving());
    }
}
