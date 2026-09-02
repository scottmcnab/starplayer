//! [`MixPath`] — the mixer's accumulator type, as a monomorphised parameter.
//!
//! Architecture §7.1 requires two voice-rendering paths:
//!
//! * [`FixedPath`] — `i16` sample, `i32` accumulator. Integer arithmetic throughout, so
//!   it is bit-identical on x86, ARM and WASM. This is **the canonical golden reference**
//!   and the embedded path (§7.3).
//! * [`FloatPath`] — `f32` accumulator. The default for desktop and browser.
//!
//! They are two implementations of one trait rather than one generic body over a scalar
//! type, for the same reason [`Interpolate`] has two methods: the rounding rules of the
//! fixed path are part of its specification, not an incidental consequence of what the
//! scalar type happens to do.
//!
//! # The accumulation buses are interleaved, not planar (task B5 research point 1)
//!
//! A bus is a `[Stereo<T>]` — one array of left/right *frames* — rather than two parallel
//! `[T]` channel arrays. Three reasons, in order of weight:
//!
//! 1. **The kernel produces one interpolated value and spends it twice.** Resampling is
//!    per *frame*; only the gain differs between channels. An interleaved bus writes both
//!    halves of one 8-byte object while the value is still in a register. A planar bus
//!    would carry two write cursors through the inner loop and touch two cache lines
//!    1 KB apart for every frame, to save nothing.
//! 2. **One associated type.** `MixPath::Accumulator` is a single type that the kernel,
//!    the voice pool, the engine's quantum buffer and every
//!    [`OutputFormat`](crate::output::OutputFormat) are already
//!    written against. Planar buses would make that a *pair* of slices and put a lifetime
//!    into the signature of everything that touches a bus.
//! 3. **A quantum is 1 KB.** `128 × Stereo<f32>` fits in a small fraction of L1 either
//!    way, so the locality argument that favours planar layouts for long buffers does not
//!    apply at this size.
//!
//! **The cost of changing it later**, recorded because it is the point of the question:
//! `Accumulator` is the only seam, and it is crate-internal in five places — `path`,
//! `kernel`, `voice`, `output`, and the engine's quantum buffer. No format crate, no
//! loader and no effect processor names it. SIMD (M7) does not force the question either:
//! `f32x4` holds two interleaved stereo frames perfectly well, and if a DSP effect ever
//! wants planar input it can deinterleave once per quantum at the graph boundary — 128
//! frames of shuffle against a whole quantum of processing.
//!
//! # Gains, and when they are hoisted
//!
//! A voice's left/right gain is [`crate::gain::voice_gain_units`]: its volume folded
//! together with the pan law, kept at full 31-bit width, and ramped by a
//! [`GainRamp`](starplayer_dsp::GainRamp) on a change. [`MixPath::gain`] is the conversion
//! from that shared integer space into whatever each path multiplies by — one shift on the
//! fixed path, one multiply on the float path.
//!
//! While a ramp is live the kernel converts per frame; when it is not, the conversion is
//! hoisted out of the run. Both are the same arithmetic, so a ramp that has just landed on
//! its target produces exactly the frame the hoisted path would have — which is what lets
//! a run be split anywhere without changing a sample.

use starplayer_core::{I1F15, U0F16};
use starplayer_dsp::{Interpolate, round_shift_nearest};

use crate::gain::{GAIN_FRACTION_BITS, GAIN_UNITY, voice_gain_units};
use crate::master::{MasterSettings, process_fixed, process_float};

/// A left/right pair of whatever the path uses for gain or for accumulated signal.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Stereo<T> {
    /// Left channel.
    pub left: T,
    /// Right channel.
    pub right: T,
}

impl<T> Stereo<T> {
    /// A left/right pair.
    pub const fn new(left: T, right: T) -> Stereo<T> { Stereo { left, right } }
}

/// One accumulated output frame on the float path.
pub type FloatFrame = Stereo<f32>;

/// One accumulated output frame on the fixed path, on the raw `i16` scale with `i32`
/// headroom for summing many voices before the output clamp.
pub type FixedFrame = Stereo<i32>;

/// How voice output is accumulated.
pub trait MixPath {
    /// One accumulated output frame.
    type Accumulator: Copy + Default;

    /// One channel's gain, in whatever form the path multiplies by.
    type Gain: Copy;

    /// Convert a gain from the shared ramping space (`0 ..= GAIN_UNITY`) into this path's
    /// multiplier.
    fn gain(units: i32) -> Self::Gain;

    /// Interpolate one source frame and add it to `destination`.
    fn mix<Interp: Interpolate>(
        destination: &mut Self::Accumulator,
        frames: &[i16],
        index: usize,
        fraction_bits: u32,
        gains: Stereo<Self::Gain>,
    );

    /// Master volume and limiting, over a whole `RENDER_QUANTUM` (architecture §1.4 — the
    /// master bus never sees a ragged segment).
    fn master(quantum: &mut [Self::Accumulator], settings: MasterSettings);

    /// Fold volume and pan into a left/right gain pair, with no ramp.
    ///
    /// The steady-state gain of a voice whose ramps have all landed. The kernel does not
    /// call this — it ramps — but everything that wants to know what a voice *will*
    /// settle at does.
    fn gains(volume: U0F16, pan: I1F15) -> Stereo<Self::Gain> {
        let units = voice_gain_units(volume, pan);
        Stereo::new(Self::gain(units.left), Self::gain(units.right))
    }
}

/// The float mixing path: `f32` accumulator, output normalised to ±1.0.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FloatPath;

impl MixPath for FloatPath {
    type Accumulator = FloatFrame;
    type Gain = f32;

    fn gain(units: i32) -> f32 { units as f32 * FLOAT_GAIN_SCALE }

    fn mix<Interp: Interpolate>(destination: &mut FloatFrame, frames: &[i16], index: usize, fraction_bits: u32, gains: Stereo<f32>) {
        let value = Interp::sample_f32(frames, index, fraction_bits);
        destination.left += value * gains.left;
        destination.right += value * gains.right;
    }

    fn master(quantum: &mut [FloatFrame], settings: MasterSettings) {
        for frame in quantum.iter_mut() {
            *frame = process_float(*frame, settings);
        }
    }
}

/// The fixed-point mixing path: `i16` sample, `i32` accumulator, integer arithmetic only.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FixedPath;

impl MixPath for FixedPath {
    type Accumulator = FixedFrame;
    /// Q15 gain: `32767` is unity.
    type Gain = i32;

    /// Round to nearest, ties away from zero. C6 makes this rule part of the golden
    /// contract for every fixed-path precision reduction.
    fn gain(units: i32) -> i32 { round_shift_nearest(units as i64, GAIN_FRACTION_BITS) as i32 }

    fn mix<Interp: Interpolate>(destination: &mut FixedFrame, frames: &[i16], index: usize, fraction_bits: u32, gains: Stereo<i32>) {
        let value = Interp::sample_fixed(frames, index, fraction_bits) as i64;
        destination.left = destination.left.saturating_add(round_shift_nearest(value * gains.left as i64, 15) as i32);
        destination.right = destination.right.saturating_add(round_shift_nearest(value * gains.right as i64, 15) as i32);
    }

    fn master(quantum: &mut [FixedFrame], settings: MasterSettings) {
        for frame in quantum.iter_mut() {
            *frame = process_fixed(*frame, settings);
        }
    }
}

/// Gain units to a float multiplier.
///
/// The extra `1/32768` folds the raw `i16` scale of the interpolator's output into the
/// gain, so the inner loop is one multiply rather than two. It is a power of two, so it
/// contributes no rounding of its own; the whole constant is evaluated once, by the
/// compiler, and is therefore identical on every target.
const FLOAT_GAIN_SCALE: f32 = 1.0 / (GAIN_UNITY as f32 * 32_768.0);

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer_dsp::Nearest;

    #[test]
    fn full_volume_hard_left_reproduces_the_source_on_the_fixed_path() {
        let gains = FixedPath::gains(U0F16::MAX, I1F15::MIN);
        let mut frame = FixedFrame::default();
        FixedPath::mix::<Nearest>(&mut frame, &[20_000], 0, 0, gains);
        // Q15 positive unity is one LSB below 1.0. The two precision reductions round to
        // nearest, so only that representational LSB remains.
        assert_eq!(frame, Stereo::new(19_999, 0));
    }

    #[test]
    fn centre_pan_is_three_decibels_down_on_both_channels() {
        let gains = FixedPath::gains(U0F16::MAX, I1F15::ZERO);
        let mut frame = FixedFrame::default();
        FixedPath::mix::<Nearest>(&mut frame, &[20_000], 0, 0, gains);
        assert_eq!(frame.left, frame.right);
        // 20000 x 0.70710678 = 14142.1, rounded to nearest.
        assert_eq!(frame.left, 14_142);
    }

    #[test]
    fn the_float_path_normalises_to_plus_minus_one() {
        let gains = FloatPath::gains(U0F16::MAX, I1F15::MIN);
        let mut frame = FloatFrame::default();
        FloatPath::mix::<Nearest>(&mut frame, &[-32_768], 0, 0, gains);
        assert!(frame.left >= -1.0 && frame.left <= -0.9999, "got {}", frame.left);
        assert_eq!(frame.right, 0.0, "hard left is silent on the right");
    }

    #[test]
    fn the_two_paths_agree_on_gain_to_within_their_rounding() {
        for units in [0, 1_000_000, GAIN_UNITY / 3, GAIN_UNITY / 2, GAIN_UNITY] {
            let fixed = FixedPath::gain(units) as f32 / 32_767.0;
            let float = FloatPath::gain(units) * 32_768.0;
            assert!((fixed - float).abs() < 1e-4, "units {units}: fixed {fixed} vs float {float}");
        }
    }

    #[test]
    fn silence_mixes_to_nothing() {
        let mut fixed = FixedFrame::default();
        FixedPath::mix::<Nearest>(&mut fixed, &[12_345], 0, 0, FixedPath::gains(U0F16::ZERO, I1F15::ZERO));
        assert_eq!(fixed, Stereo::new(0, 0));

        let mut float = FloatFrame::default();
        FloatPath::mix::<Nearest>(&mut float, &[12_345], 0, 0, FloatPath::gains(U0F16::ZERO, I1F15::ZERO));
        assert_eq!(float, Stereo::new(0.0, 0.0));
    }

    #[test]
    fn the_fixed_accumulator_saturates_rather_than_wrapping() {
        let gains = FixedPath::gains(U0F16::MAX, I1F15::ZERO);
        let mut frame = Stereo::new(i32::MAX - 1, i32::MIN + 1);
        FixedPath::mix::<Nearest>(&mut frame, &[32_767], 0, 0, gains);
        assert_eq!(frame.left, i32::MAX);
        FixedPath::mix::<Nearest>(&mut frame, &[-32_768], 0, 0, gains);
        assert_eq!(frame.right, i32::MIN);
    }

    #[test]
    fn fixed_precision_reductions_round_half_away_from_zero() {
        assert_eq!(round_shift_nearest(3, 1), 2);
        assert_eq!(round_shift_nearest(-3, 1), -2);
        assert_eq!(round_shift_nearest(1, 1), 1);
        assert_eq!(round_shift_nearest(-1, 1), -1);
    }

    #[test]
    fn the_master_bus_bounds_a_whole_quantum() {
        let mut quantum = [Stereo::new(i32::MAX, i32::MIN); 4];
        FixedPath::master(&mut quantum, MasterSettings::DEFAULT);
        assert_eq!(quantum, [Stereo::new(32_767, -32_767); 4]);

        let mut quantum = [Stereo::new(4.0, -4.0); 4];
        FloatPath::master(&mut quantum, MasterSettings::DEFAULT);
        assert_eq!(quantum, [Stereo::new(1.0, -1.0); 4]);
    }
}
