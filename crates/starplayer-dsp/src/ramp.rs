//! [`GainRamp`] — the anti-click parameter ramp (task B5 deliverable 4).
//!
//! # Why the mixer ramps at all
//!
//! The original's GUS driver never started a voice directly. `GUS_ProcessTracks` ramped
//! the *old* note down to zero, took the GF1's ramp-end IRQ, and only then did
//! `_GIRQStartVoice` program the new sample and start it
//! (`plans/reference/original-s3mlib-analysis.md` §7). Every retrigger was therefore
//! click-free, and that click-free reputation is worth preserving. The GF1 did it in
//! hardware; here it is four `i32`s per channel and one add per frame.
//!
//! # The unit is the caller's
//!
//! A ramp is a plain `i32` interpolator with no opinion about what the number means. The
//! mixer ramps a *composite* gain — voice volume folded together with the pan law's
//! channel gain (`starplayer_mixer::gain`) — because ramping the composite makes one
//! mechanism serve `VOLUME`, `PAN` and `STOP` alike, and because a pan sweep and a volume
//! slide arriving on the same frame must not fight over two separate ramps.
//!
//! # Why this is block-size independent
//!
//! [`GainRamp::advance`] moves the ramp by exactly one output frame and knows nothing
//! about the segment it is being called from. A ramp is therefore a function of *frames
//! elapsed since the change*, never of where a host block boundary fell, which is what
//! architecture §1.4's determinism invariant requires. The final frame snaps to the
//! target rather than accumulating an increment, so a ramp cannot leave a residue that
//! depends on how it was split either.

/// A linear ramp from the current value to a target, over a fixed number of frames.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct GainRamp {
    current: i32,
    target: i32,
    increment: i32,
    frames_remaining: u32,
}

impl GainRamp {
    /// A ramp that is already at `value` with nothing pending.
    pub const fn steady(value: i32) -> GainRamp {
        GainRamp { current: value, target: value, increment: 0, frames_remaining: 0 }
    }

    /// The value this frame is mixed with.
    pub const fn current(self) -> i32 { self.current }

    /// Where the ramp is heading.
    pub const fn target(self) -> i32 { self.target }

    /// Whether a ramp is in progress.
    pub const fn is_ramping(self) -> bool { self.frames_remaining > 0 }

    /// Frames of ramp left to run. The mixer uses this to bound a render run, so that the
    /// per-frame ramping kernel is entered only while a ramp is actually live.
    pub const fn frames_remaining(self) -> u32 { self.frames_remaining }

    /// Jump straight to `value` with no ramp — a fresh voice, or a deliberate hard cut.
    pub const fn jump_to(&mut self, value: i32) { *self = GainRamp::steady(value); }

    /// Head for `target` over the next `frames` output frames.
    ///
    /// Re-stating a target the ramp is already heading for is a **no-op**, deliberately:
    /// the mixer re-reads a voice's parameters at the start of every render segment, and a
    /// ramp that restarted on each of those would never finish at a small host block size
    /// — the same value would produce different output at different block sizes, which is
    /// exactly the failure the determinism test exists to catch.
    pub const fn glide_to(&mut self, target: i32, frames: u32) {
        if target == self.target {
            return;
        }
        self.target = target;
        if frames == 0 || target == self.current {
            self.current = target;
            self.increment = 0;
            self.frames_remaining = 0;
            return;
        }
        // Widened, because the two gains can be a full scale apart and because a caller is
        // allowed to ramp a signed value.
        self.increment = ((target as i64 - self.current as i64) / frames as i64) as i32;
        self.frames_remaining = frames;
    }

    /// Advance by one output frame and return the value to mix that frame with.
    ///
    /// The last frame of a ramp is assigned the target rather than incremented onto it, so
    /// that the integer division in [`GainRamp::glide_to`] can never leave the ramp a few
    /// units short of where it was asked to end up.
    pub const fn advance(&mut self) -> i32 {
        match self.frames_remaining {
            0 => {}
            1 => {
                self.current = self.target;
                self.increment = 0;
                self.frames_remaining = 0;
            }
            _ => {
                self.current = self.current.saturating_add(self.increment);
                self.frames_remaining -= 1;
            }
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAMES: u32 = 64;

    #[test]
    fn a_steady_ramp_never_moves() {
        let mut ramp = GainRamp::steady(1_000);
        assert!(!ramp.is_ramping());
        for _ in 0..10 {
            assert_eq!(ramp.advance(), 1_000);
        }
    }

    #[test]
    fn a_ramp_reaches_its_target_on_exactly_the_last_frame() {
        let mut ramp = GainRamp::steady(0);
        ramp.glide_to(1_000_000, FRAMES);
        for frame in 1..FRAMES {
            let value = ramp.advance();
            assert!(value > 0 && value < 1_000_000, "frame {frame} left the ramp at {value}");
            assert!(ramp.is_ramping(), "frame {frame} ended the ramp early");
        }
        assert_eq!(ramp.advance(), 1_000_000, "the last frame lands exactly on the target");
        assert!(!ramp.is_ramping());
        assert_eq!(ramp.advance(), 1_000_000, "and it stays there");
    }

    #[test]
    fn a_ramp_that_does_not_divide_evenly_still_lands_exactly() {
        let mut ramp = GainRamp::steady(0);
        ramp.glide_to(FRAMES as i32 * 7 + 3, FRAMES);
        for _ in 0..FRAMES {
            ramp.advance();
        }
        assert_eq!(ramp.current(), FRAMES as i32 * 7 + 3);
    }

    #[test]
    fn a_change_smaller_than_the_ramp_length_still_arrives() {
        let mut ramp = GainRamp::steady(0);
        ramp.glide_to(3, FRAMES);
        assert_eq!(ramp.increment, 0, "3 / 64 truncates to no movement at all");
        for _ in 0..FRAMES - 1 {
            assert_eq!(ramp.advance(), 0);
        }
        assert_eq!(ramp.advance(), 3, "so the whole change lands on the snap frame");
    }

    #[test]
    fn restating_the_current_target_does_not_restart_the_ramp() {
        let mut ramp = GainRamp::steady(0);
        ramp.glide_to(64_000, FRAMES);
        for _ in 0..32 {
            ramp.advance();
        }
        let midway = ramp;
        ramp.glide_to(64_000, FRAMES);
        assert_eq!(ramp, midway, "a repeated target must not restart the ramp");
    }

    #[test]
    fn a_new_target_ramps_from_wherever_the_ramp_had_got_to() {
        let mut ramp = GainRamp::steady(0);
        ramp.glide_to(64_000, FRAMES);
        for _ in 0..32 {
            ramp.advance();
        }
        let midway = ramp.current();
        ramp.glide_to(0, FRAMES);
        assert_eq!(ramp.current(), midway, "the new ramp starts from here, not from the old target");
        for _ in 0..FRAMES {
            ramp.advance();
        }
        assert_eq!(ramp.current(), 0);
    }

    #[test]
    fn splitting_a_ramp_anywhere_produces_the_same_sequence() {
        let sequence = |chunk: usize| {
            let mut ramp = GainRamp::steady(-500_000);
            ramp.glide_to(1_500_000, FRAMES);
            let mut values = alloc::vec::Vec::new();
            let mut done = 0;
            while done < FRAMES as usize * 2 {
                let count = chunk.min(FRAMES as usize * 2 - done);
                for _ in 0..count {
                    values.push(ramp.advance());
                }
                done += count;
            }
            values
        };
        let whole = sequence(FRAMES as usize * 2);
        for chunk in [1usize, 3, 7, 64, 100] {
            assert_eq!(sequence(chunk), whole, "chunk {chunk} changed the ramp");
        }
    }

    #[test]
    fn a_zero_length_ramp_is_a_jump() {
        let mut ramp = GainRamp::steady(10);
        ramp.glide_to(20, 0);
        assert_eq!(ramp.current(), 20);
        assert!(!ramp.is_ramping());
    }
}
