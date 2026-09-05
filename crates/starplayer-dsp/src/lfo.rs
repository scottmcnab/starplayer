//! [`Lfo`] — a free-running low-frequency oscillator for a chorus's or a compressor
//! sidechain filter's modulation, built on [`crate::tables::sin_q15`].
//!
//! # An integer phase, so a run can be split anywhere
//!
//! Architecture §1.4's determinism invariant (design goal 3) requires every stateful
//! thing in the DSP graph to be a pure function of frames elapsed, never of where a host
//! block boundary happened to fall — [`GainRamp`](crate::ramp::GainRamp) is the mixer's
//! version of the same rule. An `Lfo`'s `phase` is a `u32` turn advanced by a fixed
//! `increment` every frame: rendering 128 frames in one call or as 128 calls of one frame
//! each reaches the exact same `phase`, because `u32` wrapping addition does not care how
//! many times it was called to get there. A `f32` phase accumulated by repeated addition
//! would not have this property — its rounding error depends on how many additions
//! happened to be performed — which is exactly why this is an integer.

use crate::tables::sin_q15;

/// A free-running oscillator: an integer phase and a fixed per-frame increment.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Lfo {
    /// The current position in the cycle: `0` is phase zero, `u32::MAX` is one step short
    /// of a full turn.
    phase: u32,
    /// How far `phase` moves per [`Lfo::advance`].
    increment: u32,
}

impl Lfo {
    /// An LFO at phase zero with no rate set — [`Lfo::advance`] would leave it standing
    /// still until [`Lfo::set_rate`] is called.
    pub const fn new() -> Lfo { Lfo { phase: 0, increment: 0 } }

    /// The current phase, as a `sin_q15`/`cos_q15`-compatible `u32` turn.
    pub const fn phase(self) -> u32 { self.phase }

    /// `sin(phase)` in Q1.15.
    pub fn sine(self) -> i32 { sin_q15(self.phase) }

    /// A symmetric triangle wave in Q1.15: `0` at phase zero, rising to `32767` a quarter
    /// turn in, back through `0` at the half turn, down to `-32767` at three-quarters, and
    /// back to `0` at a full turn. Built from the phase directly rather than from
    /// [`sin_q15`], so it is an exact triangle rather than a smoothed approximation of one.
    pub fn triangle(self) -> i32 {
        // Four quadrants of a turn, each a linear ramp; `phase` is a fraction of `2^32`,
        // so a quadrant is `2^30` wide and the position within it is the low 30 bits.
        const QUARTER_TURN: u32 = 1 << 30;
        const PEAK: i64 = i16::MAX as i64;
        let quadrant = self.phase >> 30;
        let position = (self.phase & (QUARTER_TURN - 1)) as i64;
        let rising = (position * PEAK) / QUARTER_TURN as i64; // 0..=PEAK across the quadrant
        let falling = PEAK - rising;
        (match quadrant {
            0 => rising,
            1 => falling,
            2 => -rising,
            _ => -falling,
        }) as i32
    }

    /// Move `phase` forward by one [`Lfo::increment`].
    pub fn advance(&mut self) { self.phase = self.phase.wrapping_add(self.increment); }

    /// Set the oscillation rate from a frequency in centi-hertz (`100` is 1 Hz) and the
    /// output sample rate. The increment is `rate / sample_rate` of a full turn per
    /// frame, in `u32` turns: `round(centi_hz × 2^32 / (100 × sample_rate_hz))`.
    pub fn set_rate(&mut self, centi_hz: u32, sample_rate_hz: u32) {
        let sample_rate_hz = sample_rate_hz.max(1) as u64;
        let numerator = centi_hz as u64 * (1u64 << 32);
        let denominator = 100 * sample_rate_hz;
        let half = denominator / 2;
        self.increment = (((numerator + half) / denominator).min(u32::MAX as u64)) as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_lfo_stands_still_at_phase_zero() {
        let mut lfo = Lfo::new();
        assert_eq!(lfo.phase(), 0);
        assert_eq!(lfo.sine(), 0);
        lfo.advance();
        assert_eq!(lfo.phase(), 0, "no rate has been set, so advancing does nothing");
    }

    #[test]
    fn set_rate_advances_the_phase_by_a_fixed_increment() {
        let mut lfo = Lfo::new();
        lfo.set_rate(100, 100); // 1 Hz at 100 Hz sample rate: one full turn per 100 frames.
        for _ in 0..100 {
            lfo.advance();
        }
        // A whole number of frames at the exact rate should land within rounding of zero.
        let error = lfo.phase().min(u32::MAX - lfo.phase());
        assert!(error < 1 << 16, "phase after a full cycle is {}, not near zero", lfo.phase());
    }

    #[test]
    fn advancing_is_independent_of_how_a_run_is_split() {
        let run = |chunk: u32| {
            let mut lfo = Lfo::new();
            lfo.set_rate(733, 44_100);
            let mut phases = alloc::vec::Vec::new();
            let mut done = 0u32;
            while done < 1_000 {
                let count = chunk.min(1_000 - done);
                for _ in 0..count {
                    lfo.advance();
                    phases.push(lfo.phase());
                }
                done += count;
            }
            phases
        };
        let whole = run(1_000);
        for chunk in [1, 3, 7, 64, 128] {
            assert_eq!(run(chunk), whole, "chunk {chunk} changed the phase sequence");
        }
    }

    #[test]
    fn sine_matches_the_table_directly() {
        let mut lfo = Lfo::new();
        lfo.set_rate(100, 400); // a quarter turn per 100 frames at a 1 Hz rate.
        for _ in 0..100 {
            lfo.advance();
        }
        assert_eq!(lfo.sine(), sin_q15(lfo.phase()));
    }

    #[test]
    fn triangle_hits_its_landmarks() {
        let mut lfo = Lfo::new();
        assert_eq!(lfo.triangle(), 0, "phase zero is the rising zero crossing");
        lfo.phase = 1 << 30;
        assert_eq!(lfo.triangle(), i16::MAX as i32, "a quarter turn is the peak");
        lfo.phase = 2 << 30;
        assert_eq!(lfo.triangle(), 0, "a half turn is the falling zero crossing");
        lfo.phase = 3 << 30;
        assert_eq!(lfo.triangle(), -(i16::MAX as i32), "three quarters is the trough");
    }

    #[test]
    fn triangle_never_exceeds_its_stated_peak() {
        let mut lfo = Lfo::new();
        for step in 0u32..4_096 {
            lfo.phase = step.wrapping_mul(1_048_576);
            assert!(lfo.triangle().abs() <= i16::MAX as i32, "phase {}", lfo.phase);
        }
    }

    #[test]
    fn set_rate_of_zero_never_advances() {
        let mut lfo = Lfo::new();
        lfo.set_rate(0, 44_100);
        for _ in 0..1_000 {
            lfo.advance();
        }
        assert_eq!(lfo.phase(), 0);
    }

    #[test]
    fn set_rate_does_not_panic_on_an_absurd_sample_rate() {
        let mut lfo = Lfo::new();
        lfo.set_rate(100, 0);
        lfo.advance();
        lfo.set_rate(u32::MAX, 1);
        lfo.advance();
    }
}
