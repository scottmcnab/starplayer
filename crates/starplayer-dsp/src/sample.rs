//! [`DspSample`] — the arithmetic an effect is written against, so one effect body serves
//! both mixing paths.
//!
//! `f32` on the float path and `i32` on the fixed path, both on the raw `i16` scale the
//! interpolators and the mixer's accumulators already work in (architecture §7.1). The
//! fixed implementation uses integer arithmetic and tables only, so its output is
//! bit-identical on x86, ARM and WASM (§7.3).
//!
//! # Q15 is the shared gain space
//!
//! [`Q15_UNITY`] is `32768`, not `32767`: a power of two, so `scale_q15` at unity is the
//! *identity* on both paths — an insert sitting at its default gain leaves the bus
//! bit-identical, which is what lets the buses be always on without moving a golden.

use crate::interpolate::round_shift_nearest;

/// The Q15 value that means a gain of exactly 1.0.
pub const Q15_UNITY: i32 = 1 << 15;

/// One channel of one frame, as an effect does arithmetic on it.
///
/// The surface is deliberately small and named the same on both implementations, so an
/// effect body is written once. `Send` is on the trait because a host builds an effect on
/// its own thread and sends the box to the audio thread.
pub trait DspSample: Copy + Default + Send {
    /// Silence.
    const ZERO: Self;

    /// Sum, saturating rather than wrapping on the fixed path.
    fn add(self, other: Self) -> Self;

    /// Multiply by a Q15 gain, where [`Q15_UNITY`] is 1.0.
    ///
    /// The fixed path rounds to nearest, ties away from zero — the same rule every other
    /// fixed-path precision reduction follows (M2-C6) — and saturates rather than
    /// wrapping, because a gain above unity can push a loud bus past `i32`.
    fn scale_q15(self, gain: i32) -> Self;
}

impl DspSample for f32 {
    const ZERO: f32 = 0.0;

    fn add(self, other: f32) -> f32 { self + other }

    /// `1 / 32768` is a power of two, so the conversion of the gain contributes no
    /// rounding of its own and unity is exactly `x * 1.0`.
    fn scale_q15(self, gain: i32) -> f32 { self * (gain as f32 * Q15_TO_FLOAT) }
}

impl DspSample for i32 {
    const ZERO: i32 = 0;

    fn add(self, other: i32) -> i32 { self.saturating_add(other) }

    fn scale_q15(self, gain: i32) -> i32 {
        let scaled = round_shift_nearest(self as i64 * gain as i64, 15);
        scaled.clamp(i32::MIN as i64, i32::MAX as i64) as i32
    }
}

/// Q15 to a float multiplier. Evaluated by the compiler, so it is identical on every
/// target.
const Q15_TO_FLOAT: f32 = 1.0 / 32_768.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_is_the_identity_on_both_paths() {
        for value in [0i32, 1, -1, 20_000, -20_000, i32::MAX / 2, i32::MIN / 2] {
            assert_eq!(value.scale_q15(Q15_UNITY), value, "fixed unity moved {value}");
        }
        for value in [0.0f32, 1.0, -1.0, 0.123_456_79, -7.5e-3, 1.0e6] {
            assert_eq!(value.scale_q15(Q15_UNITY).to_bits(), value.to_bits(), "float unity moved {value}");
        }
    }

    #[test]
    fn half_gain_halves_a_sample_on_both_paths() {
        assert_eq!(20_000i32.scale_q15(Q15_UNITY / 2), 10_000);
        assert_eq!((-20_000i32).scale_q15(Q15_UNITY / 2), -10_000);
        assert_eq!(1.0f32.scale_q15(Q15_UNITY / 2), 0.5);
    }

    #[test]
    fn zero_gain_is_silence() {
        assert_eq!(20_000i32.scale_q15(0), 0);
        assert_eq!(1.0f32.scale_q15(0), 0.0);
    }

    #[test]
    fn the_fixed_path_rounds_half_away_from_zero() {
        // 3 x 16384 >> 15 is exactly 1.5, so the tie rounds away from zero either way.
        assert_eq!(3i32.scale_q15(Q15_UNITY / 2), 2);
        assert_eq!((-3i32).scale_q15(Q15_UNITY / 2), -2);
    }

    #[test]
    fn the_fixed_path_saturates_rather_than_wrapping() {
        assert_eq!(i32::MAX.scale_q15(Q15_UNITY * 2), i32::MAX);
        assert_eq!(i32::MIN.scale_q15(Q15_UNITY * 2), i32::MIN);
    }

    #[test]
    fn addition_saturates_on_the_fixed_path() {
        assert_eq!(i32::MAX.add(1), i32::MAX);
        assert_eq!(i32::MIN.add(-1), i32::MIN);
        assert_eq!(<i32 as DspSample>::ZERO.add(5), 5);
        assert_eq!(<f32 as DspSample>::ZERO.add(0.5), 0.5);
    }
}
