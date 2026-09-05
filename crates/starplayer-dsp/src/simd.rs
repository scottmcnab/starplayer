//! The vector kernels, and the scalar bodies that specify them (M7-H6).
//!
//! # What this module is
//!
//! Architecture §7.1 says SIMD is "an optimisation *inside* the monomorphised loop, never
//! a semantic change". This module is where that rule is enforced rather than promised:
//! every kernel exists twice — a `scalar_*` body that is **compiled into every build**,
//! and, behind the `simd` feature, a `wide_*` body — and
//! `crates/starplayer-dsp/tests/simd_equivalence.rs` runs both in one binary and compares
//! **bit patterns**, not magnitudes.
//!
//! The dispatch seam is three provided methods on [`DspSample`]
//! ([`DspSample::comb_bank_step`], [`DspSample::biquad_stereo_step`],
//! [`DspSample::interpolate_taps`]): the default body calls the `scalar_*` function here,
//! and `impl DspSample for f32` overrides it — under `#[cfg(feature = "simd")]` only —
//! with the `wide_*` one. An effect stays a single generic body and never mentions a
//! vector type. The sinc dot product is not on that seam because it is not generic over
//! [`DspSample`]; `crate::interpolate` picks its body with a plain `cfg` instead.
//!
//! # `wide`, not `core::simd`
//!
//! M7 master-plan decision 6: `core::simd` is nightly-only and `rust-toolchain.toml` pins
//! stable 1.97, so the vector types come from the [`wide`] crate — `no_std`, safe to call
//! from a `#![forbid(unsafe_code)]` crate, SSE2 / NEON / simd128 backends with a plain
//! array fallback everywhere else (which is what `riscv32imc-unknown-none-elf` gets).
//!
//! # Why only the float path is vectorised
//!
//! Every fixed-path primitive that is not a bare add is a **widening** multiply followed
//! by [`round_shift_nearest`](crate::interpolate::round_shift_nearest) and a clamp back
//! into `i32`. `wide` can widen (`i32x4::widening_mul` → `i64x4`) but offers no narrowing
//! conversion back, so an exact `i32` result has to leave the vector through
//! `i64x4::to_array` — a memory round trip per multiply, which is strictly worse than the
//! scalar body it replaces. The one fixed-path operation that *does* map one-to-one is
//! saturating addition (`i32x4::saturating_add`), and that is exactly the one the mixer's
//! bus summation is made of; it is vectorised there, in `starplayer_mixer::simd`.
//!
//! So on the fixed path these kernels stay scalar, deliberately, and the equivalence test
//! still runs them: `<i32 as DspSample>::comb_bank_step` and `scalar_comb_bank_step` are
//! the same function, and the test asserting so is what will catch the day that stops
//! being true.
//!
//! # Sum order is part of the specification
//!
//! Float addition is not associative, so a kernel may only re-associate a sum the scalar
//! body does not care about. Two consequences visible here:
//!
//! * The reverb's eight comb **outputs** are summed by the caller, in comb order, exactly
//!   as before — this module vectorises the eight combs' *state update*, not their sum.
//! * The sinc dot product's reduction is a **fixed** binary tree,
//!   `((l0+l1)+(l2+l3))+((l4+l5)+(l6+l7))`, in both bodies. The scalar body was changed to
//!   that order in the same commit that added the vector one and before the feature was
//!   switched on anywhere, so the two are the same arithmetic rather than two orders that
//!   happen to agree on the inputs anyone measured.

use crate::biquad::BiquadCoefficients;
use crate::frame::Stereo;
use crate::sample::DspSample;
use crate::sinc_table::SINC_TAPS;

#[cfg(feature = "simd")]
use wide::{f32x4, f32x8};

/// Parallel comb filters one [`DspSample::comb_bank_step`] call steps: the reverb's eight.
pub const COMB_LANES: usize = 8;

/// Fractional delay taps one [`DspSample::interpolate_taps`] call interpolates.
///
/// Four, because four is the native lane count of every backend `wide` has (SSE2, NEON,
/// simd128) and because the three callers want two (the delay's stereo pair and the
/// reverb's pre-delay) or three (a chorus voice's taps on one channel). A wider kernel
/// would leave more lanes idle than it filled.
pub const TAP_LANES: usize = 4;

/// `1.0 / 2^24`, exact in `f32`: what a Q8.24 coefficient becomes on the float path.
///
/// The same constant `<f32 as DspSample>::mul_q24` divides by, restated here because a
/// vector body multiplies by the *broadcast* of the converted coefficient rather than
/// calling `mul_q24` per lane. The conversion itself is done once, in `f32`, so every lane
/// multiplies by the identical scalar the scalar body would have used.
#[cfg(feature = "simd")]
const Q24_TO_F32: f32 = 1.0 / 16_777_216.0;

/// `1.0 / 2^15`, exact in `f32`: the Q1.15 sinc coefficient scale, as
/// `crate::interpolate` applies it.
const SINC_SCALE_F32: f32 = 1.0 / 32_768.0;

/// `value × coefficient`, with the fixed path's rounding fixed points flushed to silence.
///
/// H4's reverb research point 1: [`DspSample::mul_q24`] rounds to nearest with ties away
/// from zero, so `round(v · f) == v` has small non-zero solutions for every `f` above a
/// half, and a comb whose feedback multiply cannot move its own state rings for ever. A
/// multiply that leaves a sample exactly where it was is proof the tail has reached the
/// arithmetic's floor, so the sample is zeroed. At unity — which is what the reverb's
/// `freeze` sets the comb feedback to — the multiply is exact and the flush cannot fire.
///
/// It lives here rather than in `crate::effects::reverb` because the comb kernel below is
/// the other caller and the two must be the same three lines.
pub fn attenuate_q24<S: DspSample>(value: S, coefficient_q24: i32) -> S {
    let scaled = value.mul_q24(coefficient_q24);
    if coefficient_q24 < 1 << 24 && scaled == value { S::ZERO } else { scaled }
}

// ── 1. the reverb's comb bank ───────────────────────────────────────────────────────────

/// One step of [`COMB_LANES`] lowpass-feedback combs that share an input.
///
/// `delayed[i]` is what comb `i` read from its own line this frame; `stores[i]` is its
/// damping one-pole's state and is advanced in place. The return value is what each comb
/// must **write back** to its line, already bounded at `saturation_bound`. The caller owns
/// the delay lines — they are eight independent rings with eight different lengths, which
/// no vector load can address — and owns the sum of `delayed`, which stays in comb order
/// because float addition is not associative.
///
/// This is `crate::effects::reverb::Comb::step` for eight combs at once, and it is the
/// specification the `simd` body has to match bit for bit.
pub fn scalar_comb_bank_step<S: DspSample>(
    input: S,
    delayed: [S; COMB_LANES],
    stores: &mut [S; COMB_LANES],
    feedback_q24: i32,
    damping_q24: i32,
    damping_complement_q24: i32,
    saturation_bound: i32,
) -> [S; COMB_LANES] {
    let mut written = [S::ZERO; COMB_LANES];
    for lane in 0..COMB_LANES {
        let Some(store) = stores.get_mut(lane) else { continue };
        let Some(read) = delayed.get(lane) else { continue };
        *store = read.mul_q24(damping_complement_q24).add(store.mul_q24(damping_q24));
        let Some(slot) = written.get_mut(lane) else { continue };
        *slot = input.add(attenuate_q24(*store, feedback_q24)).saturate_at(saturation_bound);
    }
    written
}

/// [`scalar_comb_bank_step`] on the float path, eight lanes wide.
///
/// `saturate_at` is the identity on `f32`, so it does not appear. The flush inside
/// [`attenuate_q24`] does: a lane whose feedback multiply did not move it is selected to
/// `+0.0`, which is what the scalar body's `scaled == value` branch does — including for
/// `-0.0`, where `-0.0 == -0.0` holds and the scalar body returns `f32::ZERO`.
#[cfg(feature = "simd")]
pub fn wide_comb_bank_step_f32(
    input: f32,
    delayed: [f32; COMB_LANES],
    stores: &mut [f32; COMB_LANES],
    feedback_q24: i32,
    damping_q24: i32,
    damping_complement_q24: i32,
    _saturation_bound: i32,
) -> [f32; COMB_LANES] {
    let damping = f32x8::splat(damping_q24 as f32 * Q24_TO_F32);
    let complement = f32x8::splat(damping_complement_q24 as f32 * Q24_TO_F32);
    let feedback = f32x8::splat(feedback_q24 as f32 * Q24_TO_F32);

    let next_store = f32x8::new(delayed) * complement + f32x8::new(*stores) * damping;
    let scaled = next_store * feedback;
    let attenuated = if feedback_q24 < 1 << 24 {
        next_store.simd_eq(scaled).select(f32x8::ZERO, scaled)
    } else {
        scaled
    };
    *stores = next_store.to_array();
    (f32x8::splat(input) + attenuated).to_array()
}

// ── 2. the equaliser's stereo biquad ────────────────────────────────────────────────────

/// One Direct Form II Transposed biquad step on a stereo pair, both channels sharing the
/// coefficients and keeping their own two state words.
///
/// This is `BiquadCoefficients::step` called twice, spelled once so the `simd` body can
/// put left in lane 0 and right in lane 1.
pub fn scalar_biquad_stereo_step<S: DspSample>(
    coefficients: &BiquadCoefficients,
    input: Stereo<S>,
    left_state: &mut [S; 2],
    right_state: &mut [S; 2],
) -> Stereo<S> {
    Stereo::new(coefficients.step(input.left, left_state), coefficients.step(input.right, right_state))
}

/// [`scalar_biquad_stereo_step`] on the float path.
///
/// Two of the four lanes carry signal and two carry zero: `wide`'s narrowest float vector
/// is four wide on every backend, and a two-lane pair still halves the multiply count
/// against two scalar chains. The idle lanes hold `0.0` and never reach any state.
#[cfg(feature = "simd")]
pub fn wide_biquad_stereo_step_f32(
    coefficients: &BiquadCoefficients,
    input: Stereo<f32>,
    left_state: &mut [f32; 2],
    right_state: &mut [f32; 2],
) -> Stereo<f32> {
    let b0 = f32x4::splat(coefficients.b0 as f32 * Q24_TO_F32);
    let b1 = f32x4::splat(coefficients.b1 as f32 * Q24_TO_F32);
    let b2 = f32x4::splat(coefficients.b2 as f32 * Q24_TO_F32);
    let a1 = f32x4::splat(coefficients.a1 as f32 * Q24_TO_F32);
    let a2 = f32x4::splat(coefficients.a2 as f32 * Q24_TO_F32);

    let x = f32x4::new([input.left, input.right, 0.0, 0.0]);
    let state_0 = f32x4::new([left_state[0], right_state[0], 0.0, 0.0]);
    let state_1 = f32x4::new([left_state[1], right_state[1], 0.0, 0.0]);

    let output = x * b0 + state_0;
    let next_0 = (x * b1 - output * a1) + state_1;
    let next_1 = x * b2 - output * a2;

    let next_0 = next_0.to_array();
    let next_1 = next_1.to_array();
    *left_state = [next_0[0], next_1[0]];
    *right_state = [next_0[1], next_1[1]];
    let output = output.to_array();
    Stereo::new(output[0], output[1])
}

// ── 3. fractional delay taps ────────────────────────────────────────────────────────────

/// [`TAP_LANES`] linear fractional delay reads, given the two frames each tap falls
/// between and its Q0.16 fraction.
///
/// This is the arithmetic half of [`crate::delay_line::DelayLine::read_fractional`]; the
/// caller gathers `current` and `next` through
/// [`crate::delay_line::DelayLine::read_fractional_parts`], because the taps are at
/// arbitrary distances in a ring buffer and no vector load can gather them. A lane whose
/// fraction is zero returns `current` untouched, exactly as the scalar read's early return
/// does — on the float path that is not the same as multiplying by zero, because
/// `-0.0 + 0.0` is `+0.0`.
///
/// Unused lanes should be passed as `S::ZERO` with a zero fraction; they return `S::ZERO`.
pub fn scalar_interpolate_taps<S: DspSample>(current: [S; TAP_LANES], next: [S; TAP_LANES], fraction_q16: [i32; TAP_LANES]) -> [S; TAP_LANES] {
    let mut interpolated = [S::ZERO; TAP_LANES];
    for lane in 0..TAP_LANES {
        let (Some(slot), Some(from), Some(to), Some(fraction)) = (interpolated.get_mut(lane), current.get(lane), next.get(lane), fraction_q16.get(lane)) else {
            continue;
        };
        *slot = if *fraction == 0 { *from } else { from.add(to.sub(*from).mul_q24(*fraction << 8)) };
    }
    interpolated
}

/// [`scalar_interpolate_taps`] on the float path.
///
/// The zero-fraction lanes are selected back to `current` **after** the vector multiply
/// rather than relying on a zero weight, because `-0.0 + 0.0` is `+0.0` and the scalar
/// body's early return is not.
#[cfg(feature = "simd")]
pub fn wide_interpolate_taps_f32(current: [f32; TAP_LANES], next: [f32; TAP_LANES], fraction_q16: [i32; TAP_LANES]) -> [f32; TAP_LANES] {
    let from = f32x4::new(current);
    let to = f32x4::new(next);
    let weights = f32x4::new([
        (fraction_q16[0] << 8) as f32 * Q24_TO_F32,
        (fraction_q16[1] << 8) as f32 * Q24_TO_F32,
        (fraction_q16[2] << 8) as f32 * Q24_TO_F32,
        (fraction_q16[3] << 8) as f32 * Q24_TO_F32,
    ]);
    let interpolated = (from + (to - from) * weights).to_array();
    [
        if fraction_q16[0] == 0 { current[0] } else { interpolated[0] },
        if fraction_q16[1] == 0 { current[1] } else { interpolated[1] },
        if fraction_q16[2] == 0 { current[2] } else { interpolated[2] },
        if fraction_q16[3] == 0 { current[3] } else { interpolated[3] },
    ]
}

// ── 4. the windowed-sinc dot product ────────────────────────────────────────────────────

/// The eight-tap sinc dot product on the float path, reduced in a **fixed** binary tree.
///
/// `((l0+l1)+(l2+l3))+((l4+l5)+(l6+l7))`, which is the order the vector body's pairwise
/// add produces and therefore the order the scalar body has to use. See the module
/// documentation.
pub fn scalar_sinc_dot_f32(taps: &[i16; SINC_TAPS], coefficients: &[i16; SINC_TAPS]) -> f32 {
    let mut products = [0.0f32; SINC_TAPS];
    for lane in 0..SINC_TAPS {
        let (Some(slot), Some(tap), Some(coefficient)) = (products.get_mut(lane), taps.get(lane), coefficients.get(lane)) else {
            continue;
        };
        *slot = *tap as f32 * *coefficient as f32;
    }
    let low = (products[0] + products[1]) + (products[2] + products[3]);
    let high = (products[4] + products[5]) + (products[6] + products[7]);
    (low + high) * SINC_SCALE_F32
}

/// [`scalar_sinc_dot_f32`] with the eight products computed in two vectors.
///
/// The even and odd taps are gathered into separate vectors so that one vector add
/// produces `[l0+l1, l2+l3, l4+l5, l6+l7]` — the scalar body's four pairs, in its order.
/// The last two adds are scalar; `wide` has no horizontal reduce whose association order
/// is specified, and a reduce whose order is unspecified is exactly what this kernel may
/// not use.
#[cfg(feature = "simd")]
pub fn wide_sinc_dot_f32(taps: &[i16; SINC_TAPS], coefficients: &[i16; SINC_TAPS]) -> f32 {
    let even_taps = f32x4::new([taps[0] as f32, taps[2] as f32, taps[4] as f32, taps[6] as f32]);
    let odd_taps = f32x4::new([taps[1] as f32, taps[3] as f32, taps[5] as f32, taps[7] as f32]);
    let even_coefficients = f32x4::new([coefficients[0] as f32, coefficients[2] as f32, coefficients[4] as f32, coefficients[6] as f32]);
    let odd_coefficients = f32x4::new([coefficients[1] as f32, coefficients[3] as f32, coefficients[5] as f32, coefficients[7] as f32]);

    let pairs = (even_taps * even_coefficients + odd_taps * odd_coefficients).to_array();
    ((pairs[0] + pairs[1]) + (pairs[2] + pairs[3])) * SINC_SCALE_F32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attenuate_flushes_the_fixed_paths_rounding_fixed_points_but_not_unity() {
        // `room_feedback_q24(100)` is 0.98 in Q8.24; 24 is one of its rounding fixed
        // points, which `crate::effects::reverb`'s own research-point test measures.
        assert_eq!(attenuate_q24(24i32, 16_441_671), 0, "a multiply that cannot move its own state is flushed");
        assert_eq!(attenuate_q24(24i32, 1 << 24), 24, "unity is exact and never flushes");
        assert_eq!(attenuate_q24(0.0f32, 1 << 23), 0.0);
    }

    #[test]
    fn an_unused_tap_lane_stays_silent() {
        let taps = scalar_interpolate_taps([0i32; TAP_LANES], [0i32; TAP_LANES], [0i32; TAP_LANES]);
        assert_eq!(taps, [0i32; TAP_LANES]);
    }

    #[test]
    fn a_zero_fraction_tap_is_the_frame_it_sits_on() {
        let taps = scalar_interpolate_taps([1_000i32, 2_000, 0, 0], [9_999i32, 9_999, 0, 0], [0, 1 << 15, 0, 0]);
        assert_eq!(taps[0], 1_000, "a zero fraction reads the frame itself");
        assert_eq!(taps[1], 6_000, "halfway between 2000 and 9999 rounds to nearest");
    }

    #[test]
    fn the_sinc_reduction_is_the_documented_tree() {
        let taps = [1i16, 2, 3, 4, 5, 6, 7, 8];
        let coefficients = [4_096i16; SINC_TAPS];
        let expected = ((1.0f32 * 4_096.0 + 2.0 * 4_096.0) + (3.0 * 4_096.0 + 4.0 * 4_096.0))
            + ((5.0 * 4_096.0 + 6.0 * 4_096.0) + (7.0 * 4_096.0 + 8.0 * 4_096.0));
        assert_eq!(scalar_sinc_dot_f32(&taps, &coefficients).to_bits(), (expected * SINC_SCALE_F32).to_bits());
    }
}
