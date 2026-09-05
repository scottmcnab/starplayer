//! [`DspSample`] — the arithmetic every effect is generic over.
//!
//! # Why a trait, and why exactly this one
//!
//! M7 decision 2 (`plans/engine/M7-master-plan.md`, "The task graph") puts the `Insert`
//! trait (H1) in this crate, generic over an arithmetic trait implemented for `f32` and
//! `i32` — one effect body, both mix paths, from the day it lands, without a third generic
//! parameter naming a scalar type nobody outside the trait needs to see. `DspSample` is
//! that trait. Its shape mirrors [`crate::filter::FilterCoefficients`]'s `Sample`: the
//! fixed path is `i32`, the float path is `f32`, and both are on the mixer's own **raw
//! `i16` scale**, not normalised to ±1.0 — `starplayer_mixer::path::MixPath::Mono` is
//! exactly this pair, and [`DspSample::from_i16`] is how a sample enters an insert chain
//! from the bus this crate cannot itself name.
//!
//! # Coefficients are always integers, arithmetic is not
//!
//! M7 decision 5 makes every audible parameter — cutoff, gain, rate, time — an integer in
//! fixed units, cooked into Q8.24 or Q1.15 coefficients on the audio thread, off `libm`,
//! from the tables in [`crate::tables`]. [`DspSample::mul_q24`] and
//! [`DspSample::scale_q15`] both take that coefficient as a plain `i32`, on *either* path:
//! the fixed path never widens it further because it already **is** the working type, and
//! the float path divides it down to a fraction once per call. One coefficient format, two
//! bodies — never a `BiquadCoefficients<DspSample>` that would need a second cooker per
//! effect for no numerical reason (see `crates/starplayer-dsp/src/biquad.rs`, which keeps
//! its coefficients as plain `i32` and drives both paths through this trait).
//!
//! # Headroom, and where `saturate` sits
//!
//! The fixed accumulator carries `i16 × gain` sums in `i32` — sixteen bits of headroom
//! over the sample it started from — exactly as `FixedPath::Mono` does
//! (`starplayer_mixer::path`). An insert chain is allowed to run *through* that headroom:
//! a delay tap, a filter's resonant peak, a handful of comb sums, all legitimately exceed
//! `i16` between one insert and the next, and clamping on every intermediate `add` would
//! flatten exactly the overshoot that makes a resonant filter or a hot delay feedback sound
//! the way it does (`crate::filter::resonate_fixed`'s own doc comment makes the identical
//! argument for the IT filter). [`DspSample::add`] and [`DspSample::sub`] are therefore
//! **not** saturating on the fixed path — they use `saturating_add`/`saturating_sub` only
//! against `i32`'s own full range, which no realistic accumulation of a handful of
//! `i16`-scale taps can reach, so in practice they are exact. [`DspSample::mul_q24`] and
//! [`DspSample::scale_q15`] narrow a widened `i64` product back to `i32` and must bound
//! that narrowing themselves — there is no other point at which they could.
//!
//! [`DspSample::saturate`] is the **one** place an insert chain is expected to call down
//! to the path's actual audio full scale before handing a sample back to the bus — the
//! fixed-path equivalent of the master bus's own `Limiter::Clamp`
//! (`starplayer_mixer::master`), which bounds to ±32767 rather than `i16::MIN..=i16::MAX`
//! so the clamp is symmetric. The float path has no such floor: nothing downstream of an
//! insert chain clips a float until the master bus's soft knee does, so
//! [`DspSample::saturate`] is the identity there. This is why the two implementations
//! genuinely differ rather than both being a no-op, which a first reading of "bound to the
//! path's full scale" might suggest.

/// The arithmetic surface every DSP primitive and effect in this crate is generic over.
///
/// Implemented for `f32` (the float mix path) and `i32` (the fixed mix path, Q8.24
/// coefficients narrowed through [`crate::interpolate::round_shift_nearest`]). See the
/// module documentation for why the trait exists, why coefficients stay plain `i32` on
/// both paths, and what each method does at the fixed/float boundary.
/// Q1.15 unity: the `scale_q15` gain that is the identity on both paths.
pub const Q15_UNITY: i32 = 1 << 15;

/// The `'static` bound is what lets an effect that *holds* a `Sample` — H3's EQ state, a
/// delay line — be coerced to `Box<dyn Insert<Sample>>`, which is a `'static` trait object.
/// Both implementations are plain scalars, so it costs nothing.
pub trait DspSample: Copy + Default + Send + PartialEq + core::fmt::Debug + 'static {
    /// Silence.
    const ZERO: Self;

    /// Widen a raw `i16` sample onto this path's working type. Exact on both paths: `i32`
    /// holds every `i16` without narrowing, and `f32`'s 24-bit mantissa holds every `i16`
    /// exactly as well.
    fn from_i16(value: i16) -> Self;

    /// `self + other`. Not a saturating clamp to `i16` range on the fixed path — see the
    /// module documentation's "Headroom" section for why an insert chain is allowed to run
    /// through the widened accumulator.
    fn add(self, other: Self) -> Self;

    /// `self - other`, the same headroom rule as [`DspSample::add`].
    fn sub(self, other: Self) -> Self;

    /// Multiply by a Q8.24 coefficient (float: `coefficient as f32 / 2^24`).
    fn mul_q24(self, coefficient: i32) -> Self;

    /// Multiply by a Q1.15 gain (`0..=32768` is `0.0..=1.0`).
    fn scale_q15(self, gain: i32) -> Self;

    /// Bound to the path's full scale (fixed: `i32` saturating; float: identity).
    fn saturate(self) -> Self;
}

/// `2^-24`, exact in `f32`: what a Q8.24 coefficient is divided by on the float path.
const Q24_TO_F32: f32 = 1.0 / 16_777_216.0;

/// `2^-15`, exact in `f32`: what a Q1.15 gain is divided by on the float path.
const Q15_TO_F32: f32 = 1.0 / 32_768.0;

/// The fixed path's saturation bound. `starplayer_mixer::master::Limiter::Clamp` clamps
/// the master bus to `±32767` rather than `i16::MIN..=i16::MAX`, so [`DspSample::saturate`]
/// lands an insert's output on exactly the range its own eventual output would.
const FIXED_SATURATION_BOUND: i32 = i16::MAX as i32;

impl DspSample for f32 {
    const ZERO: f32 = 0.0;

    fn from_i16(value: i16) -> f32 { value as f32 }

    fn add(self, other: f32) -> f32 { self + other }

    fn sub(self, other: f32) -> f32 { self - other }

    // Plain IEEE multiply, no fused multiply-add — `cargo xtask ci --job fma-check`
    // audits this crate's optimized codegen for exactly that.
    fn mul_q24(self, coefficient: i32) -> f32 { self * (coefficient as f32 * Q24_TO_F32) }

    fn scale_q15(self, gain: i32) -> f32 { self * (gain as f32 * Q15_TO_F32) }

    fn saturate(self) -> f32 { self }
}

impl DspSample for i32 {
    const ZERO: i32 = 0;

    fn from_i16(value: i16) -> i32 { value as i32 }

    fn add(self, other: i32) -> i32 { self.saturating_add(other) }

    fn sub(self, other: i32) -> i32 { self.saturating_sub(other) }

    fn mul_q24(self, coefficient: i32) -> i32 {
        let product = self as i64 * coefficient as i64;
        crate::interpolate::round_shift_nearest(product, 24).clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    fn scale_q15(self, gain: i32) -> i32 {
        let product = self as i64 * gain as i64;
        crate::interpolate::round_shift_nearest(product, 15).clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }

    fn saturate(self) -> i32 { self.clamp(-FIXED_SATURATION_BOUND, FIXED_SATURATION_BOUND) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_i16_is_exact_on_both_paths() {
        for value in [0i16, 1, -1, i16::MIN, i16::MAX, 12_345, -12_345] {
            assert_eq!(i32::from_i16(value), value as i32);
            assert_eq!(f32::from_i16(value), value as f32);
        }
    }

    #[test]
    fn zero_is_the_additive_identity() {
        assert_eq!(i32::ZERO.add(1_234), 1_234);
        assert_eq!(f32::ZERO.add(1_234.0), 1_234.0);
        assert_eq!(1_234i32.sub(i32::ZERO), 1_234);
        assert_eq!(1_234.0f32.sub(f32::ZERO), 1_234.0);
    }

    #[test]
    fn fixed_add_and_sub_saturate_at_i32_full_scale_rather_than_wrapping() {
        assert_eq!(i32::MAX.add(1), i32::MAX);
        assert_eq!(i32::MIN.sub(1), i32::MIN);
        assert_eq!(1_000i32.add(2_000), 3_000, "realistic headroom stays exact");
    }

    #[test]
    fn mul_q24_scales_by_the_coefficient_on_both_paths() {
        const UNITY_Q24: i32 = 1 << 24;
        const HALF_Q24: i32 = 1 << 23;
        assert_eq!(20_000i32.mul_q24(UNITY_Q24), 20_000, "unity coefficient is a no-op");
        assert_eq!(20_000i32.mul_q24(HALF_Q24), 10_000, "half coefficient halves the sample");
        assert_eq!(20_000.0f32.mul_q24(UNITY_Q24), 20_000.0);
        assert_eq!(20_000.0f32.mul_q24(HALF_Q24), 10_000.0);
    }

    #[test]
    fn mul_q24_saturates_on_overflow_rather_than_wrapping() {
        assert_eq!(i32::MAX.mul_q24(2 << 24), i32::MAX, "doubling near the top clamps rather than wrapping negative");
        assert_eq!(i32::MIN.mul_q24(2 << 24), i32::MIN);
    }

    #[test]
    fn scale_q15_scales_by_the_gain_on_both_paths() {
        const UNITY_Q15: i32 = 32_768;
        assert_eq!(20_000i32.scale_q15(UNITY_Q15), 20_000);
        assert_eq!(20_000i32.scale_q15(UNITY_Q15 / 2), 10_000);
        assert_eq!(20_000.0f32.scale_q15(UNITY_Q15), 20_000.0);
        assert_eq!(20_000.0f32.scale_q15(UNITY_Q15 / 2), 10_000.0);
        assert_eq!(20_000i32.scale_q15(0), 0, "zero gain silences");
    }

    #[test]
    fn saturate_bounds_the_fixed_path_to_the_symmetric_i16_scale_and_leaves_float_alone() {
        assert_eq!(100_000i32.saturate(), FIXED_SATURATION_BOUND);
        assert_eq!((-100_000i32).saturate(), -FIXED_SATURATION_BOUND);
        assert_eq!(1_234i32.saturate(), 1_234, "within scale is untouched");
        assert_eq!(100_000.0f32.saturate(), 100_000.0, "the float path never clamps here");
    }

    #[test]
    fn every_implementation_is_default_and_comparable() {
        assert_eq!(i32::default(), 0);
        assert_eq!(f32::default(), 0.0);
        assert_ne!(1i32, 2i32);
    }

    #[test]
    fn unity_is_the_identity_on_both_paths() {
        for value in [0i32, 1, -1, 20_000, -20_000, i32::MAX / 2, i32::MIN / 2] {
            assert_eq!(value.scale_q15(Q15_UNITY), value, "fixed unity moved {value}");
        }
        for value in [0.0f32, 1.0, -1.0, 0.123_456_79, -7.5e-3, 1.0e6] {
            assert_eq!(value.scale_q15(Q15_UNITY).to_bits(), value.to_bits(), "float unity moved {value}");
        }
    }
}
