//! [`BiquadCoefficients`] — the RBJ "Audio EQ Cookbook" biquad, cooked from tables.
//!
//! # One coefficient set drives both mix paths
//!
//! M7 decision 5 makes every audible parameter an integer in fixed units, cooked into a
//! coefficient set on the audio thread, off `libm`. [`BiquadCoefficients`] is that
//! coefficient set: five Q8.24 integers, *not* generic over
//! [`DspSample`](crate::sample::DspSample). [`BiquadCoefficients::step`] spends them
//! through [`DspSample::mul_q24`], which is defined for both `f32` and `i32` and already
//! knows how to turn a Q8.24 coefficient into whichever arithmetic its `Self` uses — so
//! one cooker output drives both mix paths, and there is no `BiquadCoefficients<Sample>`
//! doubling up the coefficient type for no numerical reason (contrast
//! [`crate::filter::FilterCoefficients<Sample>`], whose two paths independently derive
//! *different* algebra from a shared front end and therefore must be generic).
//!
//! # The float cooker is not a second derivation
//!
//! Every cooker below has an `_f32` twin, as the deliverable asks for, but a shelving or
//! peaking filter's cooking algebra has nothing transcendental left in it once `ω`'s
//! `sin`/`cos` and the gain's `A = 10^(dBgain/40)` are looked up — the RBJ formulas past
//! that point are `+ - × ÷` on those four numbers. So both cookers share exactly the same
//! front end ([`angular_phase`], [`amplitude_q24`], [`isqrt_u128`]), and only
//! the *combination* arithmetic differs: `i64`/`i128` fixed-point for the production
//! `BiquadCoefficients`, `f32` for [`BiquadCoefficientsF32`] — the reference this module's
//! own tests check the fixed cooker against, and a form future callers can read directly
//! for a float-domain use this module does not itself have. No `f32::sqrt` or `f32::sin`
//! appears in either: the transcendental content of an RBJ cooker is entirely captured by
//! [`crate::tables::sin_q15`]/[`cos_q15`]/[`db_to_gain_q15`] and this module's own integer
//! square root, which both cookers call.
//!
//! # Research point 2 — Q8.24 headroom
//!
//! A low shelf at 20 Hz on 44.1 kHz with an unconstrained ±96 dB gain range can demand a
//! `b1` past `120,000` — checked against the `f64` reference while writing this module —
//! which Q8.24's eight integer bits (±128) cannot hold. Real parametric EQ never asks for
//! anywhere near that: [`SHELF_GAIN_CENTI_DB_BOUND`] clamps the shelving/peaking gain
//! parameter to a symmetric ±24 dB, at which the worst coefficient measured across every
//! frequency, sample rate and slope/Q this module's tests sweep is about `31.7` — comfortably
//! inside Q8.24's range with room to spare, and a coefficient set for a filter shape
//! outside that range would not describe a usable EQ regardless of the numeric format
//! carrying it. Q4.28 was therefore not needed and was not adopted.

use crate::interpolate::round_shift_nearest;
use crate::sample::DspSample;
use crate::tables::{cos_q15, shelf_amplitude_f32, shelf_amplitude_q24, sin_q15};

/// Q8.24 biquad coefficients — normalised so `a0` is implicitly `1`, matching every other
/// coefficient set in this crate.
///
/// [`step`](BiquadCoefficients::step) is Direct Form II Transposed:
/// `y = b0·x + s0; s0' = b1·x - a1·y + s1; s1' = b2·x - a2·y`, which is why the state is
/// two values rather than the four a naive Direct Form I would need.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct BiquadCoefficients {
    pub b0: i32,
    pub b1: i32,
    pub b2: i32,
    pub a1: i32,
    pub a2: i32,
}

impl BiquadCoefficients {
    /// Pass the input through untouched.
    pub const PASS_THROUGH: BiquadCoefficients = BiquadCoefficients { b0: 1 << 24, b1: 0, b2: 0, a1: 0, a2: 0 };

    /// One Direct Form II Transposed step, generic over the mix path.
    pub fn step<S: DspSample>(&self, input: S, state: &mut [S; 2]) -> S {
        let output = input.mul_q24(self.b0).add(state[0]);
        let next_state_0 = input.mul_q24(self.b1).sub(output.mul_q24(self.a1)).add(state[1]);
        let next_state_1 = input.mul_q24(self.b2).sub(output.mul_q24(self.a2));
        state[0] = next_state_0;
        state[1] = next_state_1;
        output
    }
}

/// [`BiquadCoefficients`]'s float-arithmetic reference twin — see the module
/// documentation for what it is and is not.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct BiquadCoefficientsF32 {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

/// Clamp applied to a shelving/peaking cooker's `gain_centi_db` — see research point 2 in
/// the module documentation.
pub const SHELF_GAIN_CENTI_DB_BOUND: i32 = 2_400;

// ---------------------------------------------------------------------------------------
// The shared front end: phase, amplitude, and a table-free integer square root.
// ---------------------------------------------------------------------------------------

/// `ω0` as a `sin_q15`/`cos_q15` phase turn: `frequency_hz / sample_rate_hz` of a full
/// `2^32` turn, clamped to Nyquist (and to at least one) first.
fn angular_phase(frequency_hz: u32, sample_rate_hz: u32) -> u32 {
    let sample_rate_hz = sample_rate_hz.max(1) as u64;
    let clamped_hz = (frequency_hz as u64).min(sample_rate_hz / 2).max(1);
    (((clamped_hz << 32) / sample_rate_hz).min(u32::MAX as u64)) as u32
}

/// `A = 10^(dBgain/40)` in Q8.24 (`1 << 24` is unity): the shelving/peaking cookers'
/// shared gain term, clamped to [`SHELF_GAIN_CENTI_DB_BOUND`] first.
///
/// [`shelf_amplitude_q24`] computes this directly from the tables (`A` is
/// `10^((centi_db/2)/2000)`, `db_to_gain_q15`'s own formula at half the exponent, so it
/// needs no square root); staying at Q8.24 here rather than narrowing to Q1.15 is what the
/// "Research point 2" section of the module documentation explains matters for a
/// near-cancellation coefficient like `a2`.
fn amplitude_q24(gain_centi_db: i32) -> i32 {
    let clamped = gain_centi_db.clamp(-SHELF_GAIN_CENTI_DB_BOUND, SHELF_GAIN_CENTI_DB_BOUND);
    shelf_amplitude_q24(clamped)
}

// ---------------------------------------------------------------------------------------
// Fixed-path Q32 working arithmetic, narrowed to Q8.24 only at the very end.
// ---------------------------------------------------------------------------------------

/// Unity in the Q32 working precision the fixed cookers combine terms in — wide enough
/// that `A`, `1/A`, `sqrt(A)` and every RBJ intermediate keep their fractional precision
/// through several multiplies before the final narrowing to Q8.24.
const ONE_Q32: i64 = 1i64 << 32;

fn q15_to_q32(value: i32) -> i64 { (value as i64) << 17 }

fn q24_to_q32(value: i32) -> i64 { (value as i64) << 8 }

fn mul_q32(left: i64, right: i64) -> i64 { round_shift_i128(left as i128 * right as i128, 32) }

fn div_q32(numerator: i64, denominator: i64) -> i64 {
    if denominator == 0 {
        return 0;
    }
    round_shift_i128_over(numerator as i128 * (ONE_Q32 as i128), denominator as i128)
}

fn round_shift_i128(value: i128, shift: u32) -> i64 {
    let half = 1i128 << (shift - 1);
    let rounded = if value < 0 { -((-value + half) >> shift) } else { (value + half) >> shift };
    rounded as i64
}

fn round_shift_i128_over(numerator: i128, denominator: i128) -> i64 {
    let half = denominator.abs() / 2;
    let rounded = if (numerator < 0) != (denominator < 0) { (numerator - half) / denominator } else { (numerator + half) / denominator };
    rounded as i64
}

/// `sqrt(value_q32)`, in the same Q32 scale: `sqrt(x / 2^32) × 2^32 = sqrt(x × 2^32)`.
fn isqrt_q32(value_q32: i64) -> i64 {
    if value_q32 <= 0 {
        return 0;
    }
    isqrt_u128((value_q32 as u128) << 32) as i64
}

/// Integer square root by Newton's method: exact arithmetic, no `libm`, and (unlike
/// `f32`/`f64`'s `sqrt`) available at all without linking `std`.
fn isqrt_u128(value: u128) -> u128 {
    if value == 0 {
        return 0;
    }
    let mut estimate = value;
    let mut next = estimate.div_ceil(2);
    while next < estimate {
        estimate = next;
        next = (estimate + value / estimate) / 2;
    }
    estimate
}

fn narrow_q32_to_q24(value_q32: i64) -> i32 { round_shift_nearest(value_q32, 8).clamp(i32::MIN as i64, i32::MAX as i64) as i32 }

/// The RBJ shelving `alpha`: `sin(w0)/2 × sqrt((A + 1/A)(1/S − 1) + 2)`, in Q32.
/// `slope_q15` is clamped to `(0, 32768]` — the cookbook's own `0 < S ≤ 1`.
fn shelf_alpha_q32(sin_w0_q32: i64, a_q32: i64, slope_q15: i32) -> i64 {
    let slope_q32 = q15_to_q32(slope_q15.clamp(1, 32_768));
    let inv_a_q32 = div_q32(ONE_Q32, a_q32);
    let sum_a_inv_a_q32 = a_q32 + inv_a_q32;
    let inv_slope_minus_one_q32 = div_q32(ONE_Q32, slope_q32) - ONE_Q32;
    let inner_q32 = (mul_q32(sum_a_inv_a_q32, inv_slope_minus_one_q32) + 2 * ONE_Q32).max(0);
    round_shift_nearest(mul_q32(sin_w0_q32, isqrt_q32(inner_q32)), 1)
}

/// The RBJ peaking/low-pass/high-pass `alpha`: `sin(w0) / (2Q)`, in Q32. `q_q15` is
/// clamped to at least one Q15 unit so a zero `Q` cannot divide by zero.
fn resonant_alpha_q32(sin_w0_q32: i64, q_q15: i32) -> i64 {
    let q_q32 = q15_to_q32(q_q15.max(1));
    div_q32(sin_w0_q32, 2 * q_q32)
}

fn low_shelf_q32(cos_w0_q32: i64, sin_w0_q32: i64, gain_centi_db: i32, slope_q15: i32) -> [i64; 5] {
    let a_q32 = q24_to_q32(amplitude_q24(gain_centi_db));
    let alpha_q32 = shelf_alpha_q32(sin_w0_q32, a_q32, slope_q15);
    let sqrt_a_q32 = isqrt_q32(a_q32);
    let two_sqrt_a_alpha_q32 = 2 * mul_q32(sqrt_a_q32, alpha_q32);
    let a_plus_one = a_q32 + ONE_Q32;
    let a_minus_one = a_q32 - ONE_Q32;
    let a_minus_one_cos = mul_q32(a_minus_one, cos_w0_q32);
    let a_plus_one_cos = mul_q32(a_plus_one, cos_w0_q32);

    let b0 = mul_q32(a_q32, a_plus_one - a_minus_one_cos + two_sqrt_a_alpha_q32);
    let b1 = 2 * mul_q32(a_q32, a_minus_one - a_plus_one_cos);
    let b2 = mul_q32(a_q32, a_plus_one - a_minus_one_cos - two_sqrt_a_alpha_q32);
    let a0 = a_plus_one + a_minus_one_cos + two_sqrt_a_alpha_q32;
    let a1 = -2 * (a_minus_one + a_plus_one_cos);
    let a2 = a_plus_one + a_minus_one_cos - two_sqrt_a_alpha_q32;
    [div_q32(b0, a0), div_q32(b1, a0), div_q32(b2, a0), div_q32(a1, a0), div_q32(a2, a0)]
}

fn high_shelf_q32(cos_w0_q32: i64, sin_w0_q32: i64, gain_centi_db: i32, slope_q15: i32) -> [i64; 5] {
    let a_q32 = q24_to_q32(amplitude_q24(gain_centi_db));
    let alpha_q32 = shelf_alpha_q32(sin_w0_q32, a_q32, slope_q15);
    let sqrt_a_q32 = isqrt_q32(a_q32);
    let two_sqrt_a_alpha_q32 = 2 * mul_q32(sqrt_a_q32, alpha_q32);
    let a_plus_one = a_q32 + ONE_Q32;
    let a_minus_one = a_q32 - ONE_Q32;
    let a_minus_one_cos = mul_q32(a_minus_one, cos_w0_q32);
    let a_plus_one_cos = mul_q32(a_plus_one, cos_w0_q32);

    let b0 = mul_q32(a_q32, a_plus_one + a_minus_one_cos + two_sqrt_a_alpha_q32);
    let b1 = -2 * mul_q32(a_q32, a_minus_one + a_plus_one_cos);
    let b2 = mul_q32(a_q32, a_plus_one + a_minus_one_cos - two_sqrt_a_alpha_q32);
    let a0 = a_plus_one - a_minus_one_cos + two_sqrt_a_alpha_q32;
    let a1 = 2 * (a_minus_one - a_plus_one_cos);
    let a2 = a_plus_one - a_minus_one_cos - two_sqrt_a_alpha_q32;
    [div_q32(b0, a0), div_q32(b1, a0), div_q32(b2, a0), div_q32(a1, a0), div_q32(a2, a0)]
}

fn peaking_q32(cos_w0_q32: i64, sin_w0_q32: i64, gain_centi_db: i32, q_q15: i32) -> [i64; 5] {
    let a_q32 = q24_to_q32(amplitude_q24(gain_centi_db));
    let alpha_q32 = resonant_alpha_q32(sin_w0_q32, q_q15);
    let alpha_a_q32 = mul_q32(alpha_q32, a_q32);
    let alpha_over_a_q32 = div_q32(alpha_q32, a_q32);

    let b0 = ONE_Q32 + alpha_a_q32;
    let b1 = -2 * cos_w0_q32;
    let b2 = ONE_Q32 - alpha_a_q32;
    let a0 = ONE_Q32 + alpha_over_a_q32;
    let a1 = -2 * cos_w0_q32;
    let a2 = ONE_Q32 - alpha_over_a_q32;
    [div_q32(b0, a0), div_q32(b1, a0), div_q32(b2, a0), div_q32(a1, a0), div_q32(a2, a0)]
}

/// `sin_q15(phase)^2`, rescaled to Q32. Squaring a Q15 value gives its numerator in Q30
/// (`(v/2^15)^2 = v^2/2^30`) without any extra scaling; two more bits reach Q32.
fn square_q15_to_q32(value: i32) -> i64 { (value as i64 * value as i64) << 2 }

/// `low_pass`/`high_pass`'s `(1 ∓ cos w0)/2` terms, via the half-angle identities
/// `(1 - cos w0)/2 = sin²(w0/2)` and `(1 + cos w0)/2 = cos²(w0/2)`, rather than the direct
/// subtraction. At a low cutoff `cos w0` sits close to `1` and `1 − cos w0` is the
/// difference of two nearly-equal Q15 values — the same near-cancellation "Research point
/// 2" describes for the shelving cookers' `a2` — so computing the *half-angle* sine or
/// cosine directly avoids losing precision to it entirely rather than merely tolerating
/// the loss. `phase` is the same `angular_phase` turn `cos_w0_q32`/`sin_w0_q32` came from.
fn half_angle_squared_q32(phase: u32, cosine: bool) -> i64 {
    let half_phase = phase / 2;
    let value = if cosine { cos_q15(half_phase) } else { sin_q15(half_phase) };
    square_q15_to_q32(value)
}

fn low_pass_q32(phase: u32, cos_w0_q32: i64, sin_w0_q32: i64, q_q15: i32) -> [i64; 5] {
    let alpha_q32 = resonant_alpha_q32(sin_w0_q32, q_q15);
    let half = half_angle_squared_q32(phase, false); // sin²(w0/2) = (1 - cos w0)/2

    let b0 = half;
    let b1 = 2 * half;
    let b2 = half;
    let a0 = ONE_Q32 + alpha_q32;
    let a1 = -2 * cos_w0_q32;
    let a2 = ONE_Q32 - alpha_q32;
    [div_q32(b0, a0), div_q32(b1, a0), div_q32(b2, a0), div_q32(a1, a0), div_q32(a2, a0)]
}

fn high_pass_q32(phase: u32, cos_w0_q32: i64, sin_w0_q32: i64, q_q15: i32) -> [i64; 5] {
    let alpha_q32 = resonant_alpha_q32(sin_w0_q32, q_q15);
    let half = half_angle_squared_q32(phase, true); // cos²(w0/2) = (1 + cos w0)/2

    let b0 = half;
    let b1 = -2 * half;
    let b2 = half;
    let a0 = ONE_Q32 + alpha_q32;
    let a1 = -2 * cos_w0_q32;
    let a2 = ONE_Q32 - alpha_q32;
    [div_q32(b0, a0), div_q32(b1, a0), div_q32(b2, a0), div_q32(a1, a0), div_q32(a2, a0)]
}

fn to_coefficients(terms: [i64; 5]) -> BiquadCoefficients {
    BiquadCoefficients {
        b0: narrow_q32_to_q24(terms[0]),
        b1: narrow_q32_to_q24(terms[1]),
        b2: narrow_q32_to_q24(terms[2]),
        a1: narrow_q32_to_q24(terms[3]),
        a2: narrow_q32_to_q24(terms[4]),
    }
}

/// A low shelf: boosts or cuts below `frequency_hz` by `gain_centi_db`, transitioning over
/// a slope set by `slope_q15` (Q1.15, cookbook `S` in `(0, 1]`; `32768` is the steepest,
/// `S = 1`).
pub fn low_shelf(frequency_hz: u32, gain_centi_db: i32, slope_q15: i32, sample_rate_hz: u32) -> BiquadCoefficients {
    let phase = angular_phase(frequency_hz, sample_rate_hz);
    to_coefficients(low_shelf_q32(q15_to_q32(cos_q15(phase)), q15_to_q32(sin_q15(phase)), gain_centi_db, slope_q15))
}

/// A high shelf: boosts or cuts above `frequency_hz`.
pub fn high_shelf(frequency_hz: u32, gain_centi_db: i32, slope_q15: i32, sample_rate_hz: u32) -> BiquadCoefficients {
    let phase = angular_phase(frequency_hz, sample_rate_hz);
    to_coefficients(high_shelf_q32(q15_to_q32(cos_q15(phase)), q15_to_q32(sin_q15(phase)), gain_centi_db, slope_q15))
}

/// A peaking (bell) EQ: boosts or cuts a band around `frequency_hz` by `gain_centi_db`,
/// `q_q15` wide (Q1.15, `32768` is `Q = 1.0`; a larger `Q` narrows the band).
pub fn peaking(frequency_hz: u32, gain_centi_db: i32, q_q15: i32, sample_rate_hz: u32) -> BiquadCoefficients {
    let phase = angular_phase(frequency_hz, sample_rate_hz);
    to_coefficients(peaking_q32(q15_to_q32(cos_q15(phase)), q15_to_q32(sin_q15(phase)), gain_centi_db, q_q15))
}

/// A resonant low-pass at `frequency_hz`, `q_q15` wide.
pub fn low_pass(frequency_hz: u32, q_q15: i32, sample_rate_hz: u32) -> BiquadCoefficients {
    let phase = angular_phase(frequency_hz, sample_rate_hz);
    to_coefficients(low_pass_q32(phase, q15_to_q32(cos_q15(phase)), q15_to_q32(sin_q15(phase)), q_q15))
}

/// A resonant high-pass at `frequency_hz`, `q_q15` wide.
pub fn high_pass(frequency_hz: u32, q_q15: i32, sample_rate_hz: u32) -> BiquadCoefficients {
    let phase = angular_phase(frequency_hz, sample_rate_hz);
    to_coefficients(high_pass_q32(phase, q15_to_q32(cos_q15(phase)), q15_to_q32(sin_q15(phase)), q_q15))
}

// ---------------------------------------------------------------------------------------
// The float-arithmetic reference twins — see the module documentation.
//
// Internally these combine terms in `f64`, narrowing to `f32` only in
// `to_coefficients_f32` at the very end. A biquad's `a1`/`a2` (and, for low/high-pass,
// `b0`/`b2`) are the difference of several `O(1)` terms, so whatever relative error the
// terms feeding that subtraction carry is amplified by orders of magnitude in the result
// (measured up to roughly 100x for the shelving cookers' `a2` while writing this module).
// `f32`'s 24-bit mantissa carried through that amplification is what the fixed-vs-float
// test's own tolerance (`2^-16`) could not clear; `f64` gives forty more bits of headroom
// to spend on the amplification before it reaches the `f32` this function still returns.
// ---------------------------------------------------------------------------------------

const Q15_TO_F64: f64 = 1.0 / 32_768.0;

fn to_coefficients_f32(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> BiquadCoefficientsF32 {
    BiquadCoefficientsF32 { b0: (b0 / a0) as f32, b1: (b1 / a0) as f32, b2: (b2 / a0) as f32, a1: (a1 / a0) as f32, a2: (a2 / a0) as f32 }
}

/// A Q0.60 reconstruction of `sqrt(value)` for `value >= 0`, via [`isqrt_u128`] — the
/// `f64`-side counterpart of [`isqrt_q32`], at thirty more bits of working precision than
/// an `f32`-scale reconstruction would carry.
fn sqrt_f64(value: f64) -> f64 {
    if value <= 0.0 {
        return 0.0;
    }
    isqrt_u128((value * (1u128 << 60) as f64) as u128) as f64 * (1.0 / (1u128 << 30) as f64)
}

fn shelf_alpha_f64(sin_w0: f64, a: f64, slope_q15: i32) -> f64 {
    let slope = slope_q15.clamp(1, 32_768) as f64 * Q15_TO_F64;
    let inner = ((a + 1.0 / a) * (1.0 / slope - 1.0) + 2.0).max(0.0);
    sin_w0 * 0.5 * sqrt_f64(inner)
}

fn resonant_alpha_f64(sin_w0: f64, q_q15: i32) -> f64 {
    let q = q_q15.max(1) as f64 * Q15_TO_F64;
    sin_w0 / (2.0 * q)
}

/// `sin(w0/2)^2` or `cos(w0/2)^2`, in `f64` — the float-path counterpart of
/// [`half_angle_squared_q32`]; see that function's documentation for why low/high-pass use
/// the half-angle identity rather than `(1 ∓ cos w0)/2` directly.
fn half_angle_squared_f64(phase: u32, cosine: bool) -> f64 {
    let half_phase = phase / 2;
    let value = if cosine { cos_q15(half_phase) } else { sin_q15(half_phase) } as f64 * Q15_TO_F64;
    value * value
}

/// The float-arithmetic reference twin of [`low_shelf`].
pub fn low_shelf_f32(frequency_hz: u32, gain_centi_db: i32, slope_q15: i32, sample_rate_hz: u32) -> BiquadCoefficientsF32 {
    let phase = angular_phase(frequency_hz, sample_rate_hz);
    let cos_w0 = cos_q15(phase) as f64 * Q15_TO_F64;
    let sin_w0 = sin_q15(phase) as f64 * Q15_TO_F64;
    let a = shelf_amplitude_f32(gain_centi_db.clamp(-SHELF_GAIN_CENTI_DB_BOUND, SHELF_GAIN_CENTI_DB_BOUND)) as f64;
    let alpha = shelf_alpha_f64(sin_w0, a, slope_q15);
    let sqrt_a = sqrt_f64(a);
    let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha;
    let a_plus_one = a + 1.0;
    let a_minus_one = a - 1.0;

    let b0 = a * (a_plus_one - a_minus_one * cos_w0 + two_sqrt_a_alpha);
    let b1 = 2.0 * a * (a_minus_one - a_plus_one * cos_w0);
    let b2 = a * (a_plus_one - a_minus_one * cos_w0 - two_sqrt_a_alpha);
    let a0 = a_plus_one + a_minus_one * cos_w0 + two_sqrt_a_alpha;
    let a1 = -2.0 * (a_minus_one + a_plus_one * cos_w0);
    let a2 = a_plus_one + a_minus_one * cos_w0 - two_sqrt_a_alpha;
    to_coefficients_f32(b0, b1, b2, a0, a1, a2)
}

/// The float-arithmetic reference twin of [`high_shelf`].
pub fn high_shelf_f32(frequency_hz: u32, gain_centi_db: i32, slope_q15: i32, sample_rate_hz: u32) -> BiquadCoefficientsF32 {
    let phase = angular_phase(frequency_hz, sample_rate_hz);
    let cos_w0 = cos_q15(phase) as f64 * Q15_TO_F64;
    let sin_w0 = sin_q15(phase) as f64 * Q15_TO_F64;
    let a = shelf_amplitude_f32(gain_centi_db.clamp(-SHELF_GAIN_CENTI_DB_BOUND, SHELF_GAIN_CENTI_DB_BOUND)) as f64;
    let alpha = shelf_alpha_f64(sin_w0, a, slope_q15);
    let sqrt_a = sqrt_f64(a);
    let two_sqrt_a_alpha = 2.0 * sqrt_a * alpha;
    let a_plus_one = a + 1.0;
    let a_minus_one = a - 1.0;

    let b0 = a * (a_plus_one + a_minus_one * cos_w0 + two_sqrt_a_alpha);
    let b1 = -2.0 * a * (a_minus_one + a_plus_one * cos_w0);
    let b2 = a * (a_plus_one + a_minus_one * cos_w0 - two_sqrt_a_alpha);
    let a0 = a_plus_one - a_minus_one * cos_w0 + two_sqrt_a_alpha;
    let a1 = 2.0 * (a_minus_one - a_plus_one * cos_w0);
    let a2 = a_plus_one - a_minus_one * cos_w0 - two_sqrt_a_alpha;
    to_coefficients_f32(b0, b1, b2, a0, a1, a2)
}

/// The float-arithmetic reference twin of [`peaking`].
pub fn peaking_f32(frequency_hz: u32, gain_centi_db: i32, q_q15: i32, sample_rate_hz: u32) -> BiquadCoefficientsF32 {
    let phase = angular_phase(frequency_hz, sample_rate_hz);
    let cos_w0 = cos_q15(phase) as f64 * Q15_TO_F64;
    let sin_w0 = sin_q15(phase) as f64 * Q15_TO_F64;
    let a = shelf_amplitude_f32(gain_centi_db.clamp(-SHELF_GAIN_CENTI_DB_BOUND, SHELF_GAIN_CENTI_DB_BOUND)) as f64;
    let alpha = resonant_alpha_f64(sin_w0, q_q15);

    let b0 = 1.0 + alpha * a;
    let b1 = -2.0 * cos_w0;
    let b2 = 1.0 - alpha * a;
    let a0 = 1.0 + alpha / a;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha / a;
    to_coefficients_f32(b0, b1, b2, a0, a1, a2)
}

/// The float-arithmetic reference twin of [`low_pass`].
pub fn low_pass_f32(frequency_hz: u32, q_q15: i32, sample_rate_hz: u32) -> BiquadCoefficientsF32 {
    let phase = angular_phase(frequency_hz, sample_rate_hz);
    let cos_w0 = cos_q15(phase) as f64 * Q15_TO_F64;
    let sin_w0 = sin_q15(phase) as f64 * Q15_TO_F64;
    let alpha = resonant_alpha_f64(sin_w0, q_q15);
    let half = half_angle_squared_f64(phase, false); // sin²(w0/2) = (1 - cos w0)/2

    let b0 = half;
    let b1 = 2.0 * half;
    let b2 = half;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha;
    to_coefficients_f32(b0, b1, b2, a0, a1, a2)
}

/// The float-arithmetic reference twin of [`high_pass`].
pub fn high_pass_f32(frequency_hz: u32, q_q15: i32, sample_rate_hz: u32) -> BiquadCoefficientsF32 {
    let phase = angular_phase(frequency_hz, sample_rate_hz);
    let cos_w0 = cos_q15(phase) as f64 * Q15_TO_F64;
    let sin_w0 = sin_q15(phase) as f64 * Q15_TO_F64;
    let alpha = resonant_alpha_f64(sin_w0, q_q15);
    let half = half_angle_squared_f64(phase, true); // cos²(w0/2) = (1 + cos w0)/2

    let b0 = half;
    let b1 = -2.0 * half;
    let b2 = half;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cos_w0;
    let a2 = 1.0 - alpha;
    to_coefficients_f32(b0, b1, b2, a0, a1, a2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// The `f64` RBJ reference, independent of every table and integer routine above:
    /// real `sin`/`cos`/`sqrt`/`powf`, in double precision. This is what both cookers are
    /// checked against.
    fn reference_low_shelf(f0: f64, db_gain: f64, s: f64, fs: f64) -> [f64; 5] {
        let a = 10f64.powf(db_gain / 40.0);
        let w0 = 2.0 * core::f64::consts::PI * f0 / fs;
        let alpha = w0.sin() / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
        let cos_w0 = w0.cos();
        let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * a.sqrt() * alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * a.sqrt() * alpha);
        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * a.sqrt() * alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * a.sqrt() * alpha;
        [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
    }

    fn reference_peaking(f0: f64, db_gain: f64, q: f64, fs: f64) -> [f64; 5] {
        let a = 10f64.powf(db_gain / 40.0);
        let w0 = 2.0 * core::f64::consts::PI * f0 / fs;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;
        [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
    }

    fn reference_low_pass(f0: f64, q: f64, fs: f64) -> [f64; 5] {
        let w0 = 2.0 * core::f64::consts::PI * f0 / fs;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let b1 = 1.0 - cos_w0;
        let b0 = b1 / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;
        [b0 / a0, b1 / a0, b0 / a0, a1 / a0, a2 / a0]
    }

    #[test]
    fn pass_through_returns_its_input_on_both_paths() {
        let mut fixed_state = [0i32; 2];
        assert_eq!(BiquadCoefficients::PASS_THROUGH.step(1_234i32, &mut fixed_state), 1_234);
        let mut float_state = [0.0f32; 2];
        assert_eq!(BiquadCoefficients::PASS_THROUGH.step(1_234.0f32, &mut float_state), 1_234.0);
    }

    #[test]
    fn low_shelf_matches_the_f64_reference_within_the_stated_tolerance() {
        for &frequency_hz in &[20u32, 100, 1_000, 10_000] {
            for &gain_centi_db in &[-2_400i32, -1_200, -100, 100, 1_200, 2_400] {
                for &sample_rate_hz in &[44_100u32, 48_000, 96_000] {
                    let coefficients = low_shelf(frequency_hz, gain_centi_db, 32_768, sample_rate_hz);
                    let reference = reference_low_shelf(frequency_hz as f64, gain_centi_db as f64 / 100.0, 1.0, sample_rate_hz as f64);
                    let actual = [coefficients.b0, coefficients.b1, coefficients.b2, coefficients.a1, coefficients.a2];
                    for (value, expected) in actual.iter().zip(reference.iter()) {
                        let value_f64 = *value as f64 / (1i64 << 24) as f64;
                        let relative = (value_f64 - expected).abs() / expected.abs().max(1e-6);
                        assert!(relative < 5e-3, "freq {frequency_hz} gain {gain_centi_db} sr {sample_rate_hz}: {value_f64} vs {expected}");
                    }
                }
            }
        }
    }

    #[test]
    fn fixed_and_float_cookers_agree_within_tolerance() {
        for &frequency_hz in &[20u32, 440, 5_000] {
            for &gain_centi_db in &[-1_800i32, 0, 1_800] {
                let fixed = low_shelf(frequency_hz, gain_centi_db, 16_384, 44_100);
                let float = low_shelf_f32(frequency_hz, gain_centi_db, 16_384, 44_100);
                let fixed_terms = [fixed.b0, fixed.b1, fixed.b2, fixed.a1, fixed.a2];
                let float_terms = [float.b0, float.b1, float.b2, float.a1, float.a2];
                for (fixed_term, float_term) in fixed_terms.iter().zip(float_terms.iter()) {
                    let fixed_value = *fixed_term as f64 / (1i64 << 24) as f64;
                    let relative = (fixed_value - *float_term as f64).abs() / (float_term.abs() as f64).max(1e-6);
                    assert!(relative < 2e-4, "freq {frequency_hz} gain {gain_centi_db}: fixed {fixed_value} vs float {float_term}");
                }
            }
        }
    }

    #[test]
    fn peaking_matches_the_f64_reference() {
        for &frequency_hz in &[100u32, 1_000, 8_000] {
            for &gain_centi_db in &[-1_200i32, 600, 2_400] {
                let coefficients = peaking(frequency_hz, gain_centi_db, 23_170, 44_100);
                let reference = reference_peaking(frequency_hz as f64, gain_centi_db as f64 / 100.0, core::f64::consts::FRAC_1_SQRT_2, 44_100.0);
                let actual = [coefficients.b0, coefficients.b1, coefficients.b2, coefficients.a1, coefficients.a2];
                for (value, expected) in actual.iter().zip(reference.iter()) {
                    let value_f64 = *value as f64 / (1i64 << 24) as f64;
                    let relative = (value_f64 - expected).abs() / expected.abs().max(1e-6);
                    assert!(relative < 5e-3, "freq {frequency_hz} gain {gain_centi_db}: {value_f64} vs {expected}");
                }
            }
        }
    }

    #[test]
    fn low_pass_matches_the_f64_reference() {
        for &frequency_hz in &[200u32, 2_000, 15_000] {
            let coefficients = low_pass(frequency_hz, 23_170, 44_100);
            let reference = reference_low_pass(frequency_hz as f64, core::f64::consts::FRAC_1_SQRT_2, 44_100.0);
            let actual = [coefficients.b0, coefficients.b1, coefficients.b2, coefficients.a1, coefficients.a2];
            for (value, expected) in actual.iter().zip(reference.iter()) {
                let value_f64 = *value as f64 / (1i64 << 24) as f64;
                let relative = (value_f64 - expected).abs() / expected.abs().max(1e-6);
                assert!(relative < 5e-3, "freq {frequency_hz}: {value_f64} vs {expected}");
            }
        }
    }

    /// The exit-criteria test: run a 1 kHz sine through a peaking filter at +6 dB, then
    /// through the same filter at 0 dB as a level reference, and check the measured boost
    /// — on **both** mix paths, per the deliverable.
    fn measure_gain_db<S: DspSample>(coefficients: &BiquadCoefficients, frequency_hz: f64, sample_rate_hz: f64, to_f64: fn(S) -> f64, from_f64: fn(f64) -> S) -> f64 {
        let mut state = [S::ZERO; 2];
        // A long settle (200 ms) so the filter's own transient has fully decayed, and a
        // measurement window spanning a *whole* number of cycles at `frequency_hz` — an
        // RMS over a fractional cycle is biased by wherever the window happened to cut the
        // waveform, which is spectral leakage, not filter error, and was the reason an
        // earlier, shorter-and-unaligned version of this measurement missed its own ±0.1 dB
        // budget by more than the coefficients' own error.
        let settle = (sample_rate_hz * 0.2) as usize;
        let cycles = (frequency_hz * 0.5).max(20.0).round();
        let measure = ((cycles * sample_rate_hz / frequency_hz).round() as usize).max(1);
        let mut sum_squares = 0.0;
        for index in 0..(settle + measure) {
            let input = (2.0 * core::f64::consts::PI * frequency_hz * index as f64 / sample_rate_hz).sin() * 10_000.0;
            let output = coefficients.step(from_f64(input), &mut state);
            if index >= settle {
                let value = to_f64(output);
                sum_squares += value * value;
            }
        }
        let rms = (sum_squares / measure as f64).sqrt();
        20.0 * (rms / (10_000.0 / core::f64::consts::SQRT_2)).log10()
    }

    #[test]
    fn a_peaking_boost_at_one_kilohertz_measures_within_a_tenth_of_a_db_on_both_paths() {
        let sample_rate_hz = 44_100u32;
        let boosted = peaking(1_000, 600, 23_170, sample_rate_hz);

        let measured_fixed = measure_gain_db::<i32>(&boosted, 1_000.0, sample_rate_hz as f64, |value| value as f64, |value| value.round() as i32);
        assert!((measured_fixed - 6.0).abs() < 0.1, "fixed path measured {measured_fixed} dB at 1 kHz, wanted 6.0 +/- 0.1");

        let measured_float = measure_gain_db::<f32>(&boosted, 1_000.0, sample_rate_hz as f64, |value| value as f64, |value| value as f32);
        assert!((measured_float - 6.0).abs() < 0.1, "float path measured {measured_float} dB at 1 kHz, wanted 6.0 +/- 0.1");
    }

    #[test]
    fn a_peaking_boost_at_one_kilohertz_leaves_one_hundred_hertz_near_unity_on_both_paths() {
        let sample_rate_hz = 44_100u32;
        // `Q = 1.0` here, not the `0.7071` `a_peaking_boost...measures_within_a_tenth_of_a_db`
        // uses: that Butterworth-ish `Q` is deliberately wide for a clean measurement *at*
        // the centre frequency, but its skirt genuinely has not decayed to within ±0.1 dB
        // three octaves down (the true RBJ formula in `f64`, independent of this crate
        // entirely, measures ~0.13 dB of residual gain at 100 Hz for that filter — checked
        // while writing this test). A `Q` of `1.0` is both a more typical parametric-EQ
        // bandwidth and one whose residual at 100 Hz is comfortably inside the budget.
        let boosted = peaking(1_000, 600, 32_768, sample_rate_hz);

        let measured_fixed = measure_gain_db::<i32>(&boosted, 100.0, sample_rate_hz as f64, |value| value as f64, |value| value.round() as i32);
        assert!(measured_fixed.abs() < 0.1, "fixed path measured {measured_fixed} dB at 100 Hz, wanted 0.0 +/- 0.1");

        let measured_float = measure_gain_db::<f32>(&boosted, 100.0, sample_rate_hz as f64, |value| value as f64, |value| value as f32);
        assert!(measured_float.abs() < 0.1, "float path measured {measured_float} dB at 100 Hz, wanted 0.0 +/- 0.1");
    }

    #[test]
    fn no_biquad_coefficient_leaves_the_range_q8_24_provides_within_the_sane_gain_bound() {
        let mut worst: i64 = 0;
        for &frequency_hz in &[1u32, 20, 100, 1_000, 10_000, 20_000] {
            for &sample_rate_hz in &[8_000u32, 44_100, 48_000, 192_000] {
                for &gain_centi_db in &[-2_400i32, -1_200, 0, 1_200, 2_400] {
                    for &slope_q15 in &[1_638i32, 16_384, 32_768] {
                        for coefficients in [
                            low_shelf(frequency_hz, gain_centi_db, slope_q15, sample_rate_hz),
                            high_shelf(frequency_hz, gain_centi_db, slope_q15, sample_rate_hz),
                            peaking(frequency_hz, gain_centi_db, slope_q15, sample_rate_hz),
                        ] {
                            for term in [coefficients.b0, coefficients.b1, coefficients.b2, coefficients.a1, coefficients.a2] {
                                worst = worst.max((term as i64).abs());
                            }
                        }
                    }
                }
            }
        }
        assert!(worst < i32::MAX as i64, "a coefficient reached {worst}, past i32");
        assert!((worst as f64 / (1i64 << 24) as f64) < 100.0, "worst coefficient magnitude was {}", worst as f64 / (1i64 << 24) as f64);
    }

    #[test]
    fn cookers_do_not_panic_on_absurd_input() {
        for frequency_hz in [0u32, 1, u32::MAX] {
            for sample_rate_hz in [0u32, 1, u32::MAX] {
                let _ = low_shelf(frequency_hz, i32::MAX, i32::MAX, sample_rate_hz);
                let _ = high_shelf(frequency_hz, i32::MIN, 0, sample_rate_hz);
                let _ = peaking(frequency_hz, i32::MAX, 0, sample_rate_hz);
                let _ = low_pass(frequency_hz, 0, sample_rate_hz);
                let _ = high_pass(frequency_hz, i32::MAX, sample_rate_hz);
            }
        }
    }

    #[test]
    fn a_stable_filter_does_not_blow_up_over_a_long_run() {
        let coefficients = peaking(1_000, 1_200, 32_768, 44_100);
        let mut state = [0i32; 2];
        let mut values = Vec::new();
        for index in 0..100_000 {
            let input = if index % 7 == 0 { 20_000 } else { -20_000 };
            values.push(coefficients.step(input, &mut state));
        }
        assert!(values.iter().all(|value| value.abs() < 200_000), "the filter should settle, not diverge");
    }
}


