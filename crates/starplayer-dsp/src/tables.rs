//! `2^x`, `log2`, dB↔gain, a quarter-wave sine and a one-pole time constant — every
//! transcendental H3's EQ/delay/chorus and H4's reverb/compressor need, none of them
//! calling one.
//!
//! # No libm, on either path
//!
//! Architecture §7.3 (design goal 5) bans `sin`, `exp`, `powf` and their kin from the
//! real-time path on **both** mix paths, not just the fixed one: different targets'
//! `libm` disagree on the low bits, and this crate is `#![no_std]` with no `libm`
//! dependency to begin with — a quick check confirms `f32::sin`/`round`/`sqrt` do not even
//! exist without `std` linked in (they are inherent `std` methods, not `core` ones), so
//! the ban is also the only thing that compiles.
//!
//! Everything here is built the way [`starplayer_core::tables::LINEAR_FREQUENCY_TABLE`]
//! and `starplayer_mixer::gain`'s pan table are: a `const fn` that evaluates a
//! fixed-point series or a bit-extraction algorithm — never a runtime `libm` call, and
//! (research point 1) never a *second* copy of a table core already carries.
//!
//! # One integer table per primitive, cast for the float twin
//!
//! Every public function here comes in a `_q24`/`_q16`/`_q15` fixed form and an `_f32`
//! twin, per the deliverable. The twin does not re-derive the transcendental with a
//! parallel float algorithm the way [`crate::filter`]'s two paths independently derive
//! the IT filter's *algebra* (that duplication is what lets those two paths' rounding
//! differ and still agree); a sine, a log2 mantissa or a dB conversion has no such
//! algebra to duplicate; it is *only* the table. So the `_f32` twin takes the exact same
//! integer inputs as its fixed sibling (matching [`crate::filter::resonant_low_pass_f32`]
//! and `_fixed`, which likewise share `cutoff`/`resonance`/`sample_rate_hz` and differ
//! only in the coefficient type) and returns the fixed result cast down and scaled to
//! `f32`. This keeps one source of truth per table and makes the two forms agree by
//! construction, to within the fixed form's own rounding.
//!
//! # Coefficients stay `i32` even on the float mix path
//!
//! `db_to_gain_q15`, `pow2_q24` and friends feed [`crate::biquad`]'s cookers, which — per
//! M7 decision 5 — produce **one** `i32` coefficient set consumed by both
//! [`crate::sample::DspSample`] implementations through `mul_q24`/`scale_q15`. The `_f32`
//! twins here exist for the cases that need a genuinely floating intermediate (a
//! cross-check against an `f64` reference in `#[cfg(test)]`, or a future caller working
//! in float units directly), not because the production coefficient path ever holds one.

use starplayer_core::tables::{LINEAR_FREQUENCY_TABLE, LINEAR_FREQUENCY_TABLE_LEN};

use crate::interpolate::round_shift_nearest;

// ---------------------------------------------------------------------------------------
// `2^x` — pow2_q24 / pow2_f32, exp_neg_q24 / exp_neg_f32
// ---------------------------------------------------------------------------------------

/// Fractional bits of a `pow2_q24`/`exp_neg_q24`/`time_constant_q24` exponent or result:
/// Q8.24, the same coefficient format `crate::filter` and `crate::biquad` use.
pub const POW2_FRACTION_BITS: u32 = 24;

/// `2^x` for `x` in Q16.16, as a Q8.24 fixed-point value.
///
/// Research point 1: rather than a second frequency table, this reads
/// [`LINEAR_FREQUENCY_TABLE`] directly — 768 entries of `2^(n/768)` in Q8.24, already
/// exactly what an octave's fractional part needs — and adds the one thing that table's
/// own accessors (`linear_frequency_q24`, `scale_frequency`) do not: linear interpolation
/// *between* entries, since `x`'s fractional part lands between two 1/768-octave steps
/// far more often than exactly on one. The integer part of `x` (whole octaves) is applied
/// afterwards as a shift, saturating rather than overflowing `i32` for an absurd input.
pub fn pow2_q24(x_q16: i32) -> i32 {
    let octave = x_q16.div_euclid(1 << 16);
    let remainder_q16 = x_q16.rem_euclid(1 << 16) as u32;

    // Position within the table, in Q16.16-scaled table units (0..768<<16 at most).
    let scaled_units = remainder_q16 as u64 * LINEAR_FREQUENCY_TABLE_LEN as u64;
    let index = (scaled_units >> 16) as usize;
    let fraction = (scaled_units & 0xFFFF) as i64;

    let entry0 = linear_frequency_table_entry(index) as i64;
    let entry1 = if index + 1 < LINEAR_FREQUENCY_TABLE_LEN {
        linear_frequency_table_entry(index + 1) as i64
    } else {
        // One octave up doubles: LINEAR_FREQUENCY_TABLE[0] is exactly `1 << 24`.
        linear_frequency_table_entry(0) as i64 * 2
    };
    let interpolated = entry0 + round_shift_nearest((entry1 - entry0) * fraction, 16);

    apply_octave_shift(interpolated, octave)
}

/// `2^x` for the same Q16.16 `x`, cast to `f32`. See the module documentation for why this
/// reads the fixed table rather than an independent float series.
pub fn pow2_f32(x_q16: i32) -> f32 { pow2_q24(x_q16) as f32 * Q24_TO_F32 }

/// `exp(-x)` for `x` in Q16.16, as `2^(-x·log2 e)` — the one place this module names `e`,
/// and only as an integer constant multiplying an exponent that already avoids `libm`.
pub fn exp_neg_q24(x_q16: i32) -> i32 { pow2_q24(-mul_q16(x_q16, LOG2E_Q16)) }

/// `exp(-x)`, cast to `f32`.
pub fn exp_neg_f32(x_q16: i32) -> f32 { exp_neg_q24(x_q16) as f32 * Q24_TO_F32 }

/// `round(log2(e) × 2^16)`.
const LOG2E_Q16: i32 = 94_548;

/// One entry of [`LINEAR_FREQUENCY_TABLE`], or unity if `index` is somehow out of range —
/// unreachable given `pow2_q24`'s own masking, but the render path may never panic.
fn linear_frequency_table_entry(index: usize) -> u32 { LINEAR_FREQUENCY_TABLE.get(index).copied().unwrap_or(1 << 24) }

/// Q16.16 multiply, widened so the product cannot overflow, rounded to nearest.
fn mul_q16(left: i32, right: i32) -> i32 { round_shift_nearest(left as i64 * right as i64, 16) as i32 }

/// Apply a whole-octave shift to a Q8.24 value, saturating into `i32` rather than
/// overflowing. `octave` is clamped before shifting so the shift amount itself can never
/// be undefined behaviour for an absurd `x_q16`.
fn apply_octave_shift(value_q24: i64, octave: i32) -> i32 {
    if octave >= 0 {
        let shift = octave.min(40) as u32;
        clamp_to_i32((value_q24 as i128) << shift)
    } else {
        let shift = (-octave).min(63) as u32;
        round_shift_nearest(value_q24, shift) as i32
    }
}

fn clamp_to_i32(value: i128) -> i32 {
    if value > i32::MAX as i128 {
        i32::MAX
    } else if value < i32::MIN as i128 {
        i32::MIN
    } else {
        value as i32
    }
}

// ---------------------------------------------------------------------------------------
// `log2` — log2_q16 / log2_f32
// ---------------------------------------------------------------------------------------

/// `log2(value)` in Q16.16, for `value > 0`. `log2_q16(0)` returns `i32::MIN` — there is
/// no finite answer, and the render path may not panic, so the caller gets the most
/// negative value rather than a divide-by-zero.
///
/// Leading-zero count gives the integer part; the fractional part comes from
/// [`LOG2_MANTISSA_TABLE`], a 257-entry table of `log2(1 + i/256)` for `i` in `0..=256`,
/// linearly interpolated. Error is under `2^-16` (research point 3's log2 half of the
/// compressor's future envelope-follower needs), comfortably inside the `2^-12` this
/// deliverable asks for.
pub fn log2_q16(value: u32) -> i32 {
    if value == 0 {
        return i32::MIN;
    }
    let msb = 31 - value.leading_zeros();
    // Normalise so the leading one bit sits at bit 31: `mantissa_q32` represents a value
    // in `[1<<31, 1<<32)`, i.e. the mantissa in `[1, 2)` scaled by `2^31`.
    let mantissa_q32 = if msb >= 31 { value } else { value << (31 - msb) };
    let fraction_q31 = mantissa_q32 - (1u32 << 31);

    // The top 8 bits select the table entry; the next 16 interpolate within it.
    let index = (fraction_q31 >> 23) as usize;
    let interpolation_fraction = ((fraction_q31 >> 7) & 0xFFFF) as i64;

    let entry0 = log2_mantissa_entry(index) as i64;
    let entry1 = log2_mantissa_entry(index + 1) as i64;
    let mantissa_log2_q16 = entry0 + round_shift_nearest((entry1 - entry0) * interpolation_fraction, 16);

    (msb as i64 * (1 << 16) + mantissa_log2_q16) as i32
}

/// `log2(value)`, cast to `f32`. `log2_f32(0)` is `f32::NEG_INFINITY`, matching
/// `f64::log2(0.0)`'s own sign convention rather than [`log2_q16`]'s saturated sentinel,
/// since `f32` can represent the true answer exactly.
pub fn log2_f32(value: u32) -> f32 {
    if value == 0 {
        return f32::NEG_INFINITY;
    }
    log2_q16(value) as f32 * Q16_TO_F32
}

/// `log2(1 + i/256)` in Q16.16, for `i` in `0..=256` — [`LOG2_MANTISSA_TABLE_LEN`] entries.
const LOG2_MANTISSA_TABLE_LEN: usize = 257;

/// Internal working precision for [`log2_fraction_q60`]'s repeated-squaring bit
/// extraction. 60, matching [`starplayer_core::tables`]'s own `EXP2_WORKING_FRACTION_BITS`
/// convention, leaves a squared intermediate (`< 4 << 60`) comfortably inside `u128`
/// (`< 2^123`) at every step.
const LOG2_WORKING_FRACTION_BITS: u32 = 60;

const LOG2_MANTISSA_TABLE: [u32; LOG2_MANTISSA_TABLE_LEN] = build_log2_mantissa_table();

#[allow(clippy::indexing_slicing)]
const fn build_log2_mantissa_table() -> [u32; LOG2_MANTISSA_TABLE_LEN] {
    let mut table = [0u32; LOG2_MANTISSA_TABLE_LEN];
    let mut index = 0;
    while index < LOG2_MANTISSA_TABLE_LEN {
        // `log2(2)` is exactly `1.0`, one whole octave past what the bit-extraction loop
        // below is written for (mantissas in `[1, 2)`); the last entry is special-cased
        // rather than fed to it.
        let fraction_q60 = if index == LOG2_MANTISSA_TABLE_LEN - 1 {
            1u64 << LOG2_WORKING_FRACTION_BITS
        } else {
            log2_fraction_q60(256 + index as u64, 256)
        };
        table[index] = (fraction_q60 >> (LOG2_WORKING_FRACTION_BITS - 16)) as u32;
        index += 1;
    }
    table
}

/// `log2(numerator/denominator)` in Q0.60, for a ratio in `[1, 2)`, by repeated-squaring
/// bit extraction: square the remaining mantissa each step; if it overflows 2, that step's
/// bit is a one and the mantissa is halved back into range before the next step. Purely
/// integer — multiply, compare, shift — so it is exact fixed-point computation rather than
/// a libm call, and safe to run in the const evaluator.
const fn log2_fraction_q60(numerator: u64, denominator: u64) -> u64 {
    let mut mantissa = ((numerator as u128) << LOG2_WORKING_FRACTION_BITS) / denominator as u128;
    let mut result: u64 = 0;
    let mut bit = 0u32;
    while bit < LOG2_WORKING_FRACTION_BITS {
        mantissa = (mantissa * mantissa) >> LOG2_WORKING_FRACTION_BITS;
        if mantissa >= (2u128 << LOG2_WORKING_FRACTION_BITS) {
            result |= 1u64 << (LOG2_WORKING_FRACTION_BITS - 1 - bit);
            mantissa >>= 1;
        }
        bit += 1;
    }
    result
}

fn log2_mantissa_entry(index: usize) -> u32 { LOG2_MANTISSA_TABLE.get(index).copied().unwrap_or(1 << 16) }

/// `2^-16`, exact in `f32`.
const Q16_TO_F32: f32 = 1.0 / 65_536.0;

// ---------------------------------------------------------------------------------------
// dB ↔ linear gain — db_to_gain_q15 / db_to_gain_f32, gain_to_centi_db / _f32
// ---------------------------------------------------------------------------------------

/// The clamp every dB parameter in this crate is bounded to: −96 dB to +24 dB, in
/// centi-dB. Wide enough for a compressor's make-up gain or a shelf's boost and narrow
/// enough that `pow2_q24`'s Q8.24 result never approaches saturation from this input alone
/// (see `crate::biquad`'s own, narrower clamp for shelving/peaking gain specifically).
pub const CENTI_DB_MIN: i32 = -9_600;
pub const CENTI_DB_MAX: i32 = 2_400;

/// `round(log2(10) × 2^16)`.
const LOG2_10_Q16: i64 = 217_706;

/// `10^(centi_db/2000)` as a Q1.15 gain (`32768` is unity), clamped to
/// [`CENTI_DB_MIN`]/[`CENTI_DB_MAX`] first.
///
/// `gain = 10^(dB/20)` is the standard amplitude-ratio definition and `centi_db = dB × 100`,
/// so the exponent is `10^(centi_db/2000) = 2^(centi_db · log2(10) / 2000)` —
/// [`pow2_q24`] does the `2^x`, narrowed from Q8.24 down to Q1.15.
pub fn db_to_gain_q15(centi_db: i32) -> i32 {
    let clamped = centi_db.clamp(CENTI_DB_MIN, CENTI_DB_MAX);
    let exponent_q16 = round_divide(clamped as i64 * LOG2_10_Q16, 2_000) as i32;
    round_shift_nearest(pow2_q24(exponent_q16) as i64, POW2_FRACTION_BITS - 15) as i32
}

/// `db_to_gain_q15`, cast to `f32`.
pub fn db_to_gain_f32(centi_db: i32) -> f32 { db_to_gain_q15(centi_db) as f32 * Q15_TO_F32 }

/// The inverse of [`db_to_gain_q15`]: `20 × log10(gain)` in centi-dB, clamped to the same
/// range. `gain_q15 <= 0` returns [`CENTI_DB_MIN`] rather than computing a `log2` of zero.
pub fn gain_to_centi_db(gain_q15: i32) -> i32 {
    if gain_q15 <= 0 {
        return CENTI_DB_MIN;
    }
    // `log2(gain_actual) = log2(gain_q15) - log2(32768) = log2_q16(gain_q15) - 15<<16`.
    let log2_gain_q16 = log2_q16(gain_q15 as u32) as i64 - (15i64 << 16);
    let centi_db = round_divide(log2_gain_q16 * 2_000, LOG2_10_Q16);
    (centi_db as i32).clamp(CENTI_DB_MIN, CENTI_DB_MAX)
}

/// `gain_to_centi_db`, cast to `f32`. The value is an integer fixed unit on both paths
/// (M7 decision 5); this exists only so a float-path caller need not special-case it.
pub fn gain_to_centi_db_f32(gain_q15: i32) -> f32 { gain_to_centi_db(gain_q15) as f32 }

/// `A = 10^(dBgain/40)` in Q8.24 (`1 << 24` is unity) — the RBJ cookbook's shelving/peaking
/// amplitude term, for `dBgain = centi_db / 100`.
///
/// `crate::biquad` needs this rather than [`db_to_gain_q15`] itself: `10^(dBgain/40)` is
/// `10^((centi_db/2)/2000)`, i.e. [`db_to_gain_q15`]'s own formula evaluated at half the
/// exponent, so this calls straight through to [`pow2_q24`] rather than taking a square
/// root of [`db_to_gain_q15`]'s result — both algebraically simpler and, because it never
/// narrows through Q1.15 on the way, an order of magnitude more precise than
/// `sqrt(db_to_gain_q15(centi_db))` would be. A biquad's `a2`-style coefficients are the
/// difference of several `O(1)` terms, so the amplification of whatever error `A` carries
/// is severe; keeping this at Q8.24 rather than Q1.15 is what let the fixed and float
/// cookers in `crate::biquad` agree as tightly as their own tests require.
pub fn shelf_amplitude_q24(gain_centi_db: i32) -> i32 {
    let clamped = gain_centi_db.clamp(CENTI_DB_MIN, CENTI_DB_MAX);
    let exponent_q16 = round_divide(clamped as i64 * LOG2_10_Q16, 4_000) as i32;
    pow2_q24(exponent_q16)
}

/// `shelf_amplitude_q24`, cast to `f32`.
pub fn shelf_amplitude_f32(gain_centi_db: i32) -> f32 { shelf_amplitude_q24(gain_centi_db) as f32 * Q24_TO_F32 }

/// `2^-15`, exact in `f32`.
const Q15_TO_F32: f32 = 1.0 / 32_768.0;

/// `2^-24`, exact in `f32`.
const Q24_TO_F32: f32 = 1.0 / 16_777_216.0;

/// `numerator / denominator`, rounded to nearest with ties away from zero.
/// `denominator` must be positive; every call site here passes a positive literal.
fn round_divide(numerator: i64, denominator: i64) -> i64 {
    let half = denominator / 2;
    if numerator >= 0 { (numerator + half) / denominator } else { (numerator - half) / denominator }
}

// ---------------------------------------------------------------------------------------
// Quarter-wave sine — sin_q15 / sin_f32, cos_q15 / cos_f32
// ---------------------------------------------------------------------------------------

/// Entries in the quarter-wave sine table: `sin(i/1024 × π/2)` for `i` in `0..1024`, in
/// Q1.15 (peak `32767`, matching `starplayer_mixer::gain`'s pan table convention rather
/// than the two's-complement-exact `I1F15` one, since amplitude ±32767 is what an LFO or
/// a biquad's `ω` needs, not a signed fixed-point *value* with its own MIN/MAX asymmetry).
const SINE_QUARTER_TABLE_LEN: usize = 1024;

/// One quarter turn of a `u32` phase, where a full turn is `0..2^32`.
const QUARTER_TURN: u32 = 1 << 30;

/// Peak amplitude of [`SINE_QUARTER_TABLE`] and of [`sin_q15`]/[`cos_q15`]'s output.
const SINE_UNITY: i32 = i16::MAX as i32;

const SINE_QUARTER_TABLE: [i16; SINE_QUARTER_TABLE_LEN] = build_sine_quarter_table();

/// π/2 in Q0.30.
const QUARTER_TURN_RADIANS_Q30: i64 = 1_686_629_713;

/// Q0.30 multiply, widened so the product cannot overflow.
const fn mul_q30(left: i64, right: i64) -> i64 { ((left as i128 * right as i128) >> 30) as i64 }

/// `sin(angle)` for `angle` in Q0.30 over `[0, π/2]`, by a nine-term Taylor series — the
/// same series and term count as `starplayer_mixer::gain`'s `sin_q30`, which measures it
/// exact to about `1e-9` over the quarter wave, three orders of magnitude finer than the
/// Q15 this table rounds to.
const fn sin_q30_series(angle_q30: i64) -> i64 {
    let squared = mul_q30(angle_q30, angle_q30);
    let mut term = angle_q30;
    let mut sum = angle_q30;
    let mut order = 1i64;
    while order <= 4 {
        term = mul_q30(term, squared) / ((2 * order) * (2 * order + 1));
        sum = if order % 2 == 1 { sum - term } else { sum + term };
        order += 1;
    }
    sum
}

#[allow(clippy::indexing_slicing)]
const fn build_sine_quarter_table() -> [i16; SINE_QUARTER_TABLE_LEN] {
    let mut table = [0i16; SINE_QUARTER_TABLE_LEN];
    let mut index = 0;
    while index < SINE_QUARTER_TABLE_LEN {
        let angle_q30 = (QUARTER_TURN_RADIANS_Q30 * index as i64) / SINE_QUARTER_TABLE_LEN as i64;
        let sine_q30 = sin_q30_series(angle_q30);
        table[index] = ((sine_q30 * SINE_UNITY as i64 + (1 << 29)) >> 30) as i16;
        index += 1;
    }
    table
}

fn sine_quarter_entry(index: usize) -> i32 { SINE_QUARTER_TABLE.get(index).copied().unwrap_or(0) as i32 }

/// Interpolated lookup for a position within one quarter turn, `0..=QUARTER_TURN`
/// inclusive: [`SINE_QUARTER_TABLE`] covers `[0, QUARTER_TURN)` and the exact endpoint is
/// unity by construction (`sin(π/2) = 1`), so it is handled directly rather than stored.
fn quarter_wave_lookup(position: u32) -> i32 {
    if position >= QUARTER_TURN {
        return SINE_UNITY;
    }
    let scaled = position as u64 * SINE_QUARTER_TABLE_LEN as u64;
    let index = (scaled >> 30) as usize;
    let fraction = (scaled & ((1u64 << 30) - 1)) as i64;

    let entry0 = sine_quarter_entry(index);
    let entry1 = if index + 1 < SINE_QUARTER_TABLE_LEN { sine_quarter_entry(index + 1) } else { SINE_UNITY };
    entry0 + round_shift_nearest((entry1 - entry0) as i64 * fraction, 30) as i32
}

/// `sin(θ)` in Q1.15 (peak `32767`), for `phase` a `u32` turn: `0` is `θ = 0`,
/// `1 << 32` (wrapping to `0`) is a full `2π`.
pub fn sin_q15(phase: u32) -> i32 {
    let quadrant = phase >> 30;
    let phase_in_quadrant = phase & (QUARTER_TURN - 1);
    match quadrant {
        0 => quarter_wave_lookup(phase_in_quadrant),
        1 => quarter_wave_lookup(QUARTER_TURN - phase_in_quadrant),
        2 => -quarter_wave_lookup(phase_in_quadrant),
        _ => -quarter_wave_lookup(QUARTER_TURN - phase_in_quadrant),
    }
}

/// `cos(θ)` in Q1.15: `sin` a quarter turn ahead.
pub fn cos_q15(phase: u32) -> i32 { sin_q15(phase.wrapping_add(QUARTER_TURN)) }

/// `sin_q15`, cast to `f32`.
pub fn sin_f32(phase: u32) -> f32 { sin_q15(phase) as f32 * SINE_TO_F32 }

/// `cos_q15`, cast to `f32`.
pub fn cos_f32(phase: u32) -> f32 { cos_q15(phase) as f32 * SINE_TO_F32 }

/// `1 / 32767`, what [`sin_q15`]/[`cos_q15`]'s peak amplitude divides by.
const SINE_TO_F32: f32 = 1.0 / 32_767.0;

// ---------------------------------------------------------------------------------------
// One-pole time constant — time_constant_q24 / time_constant_f32
// ---------------------------------------------------------------------------------------

/// The one-pole coefficient `exp(-1/(t·sr))` for an envelope follower, in Q8.24, for a
/// time constant of `milliseconds` at `sample_rate_hz`. `milliseconds` and
/// `sample_rate_hz` are both floored to `1` first, so a zero or negative time constant
/// settles instantly (coefficient `0`) rather than dividing by zero.
pub fn time_constant_q24(milliseconds: i32, sample_rate_hz: u32) -> i32 {
    let denominator = milliseconds.max(1) as u64 * sample_rate_hz.max(1) as u64;
    // `x = 1000 / (milliseconds × sample_rate_hz)`, in Q16.16; `1000 << 16` fits `u64`
    // comfortably, and the whole numerator is at most `65_536_000`.
    let numerator = 1_000u64 << 16;
    let half = denominator / 2;
    let x_q16 = ((numerator + half) / denominator).min(i32::MAX as u64) as i32;
    exp_neg_q24(x_q16)
}

/// `time_constant_q24`, cast to `f32`.
pub fn time_constant_f32(milliseconds: i32, sample_rate_hz: u32) -> f32 { time_constant_q24(milliseconds, sample_rate_hz) as f32 * Q24_TO_F32 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pow2_is_exact_at_whole_numbers() {
        assert_eq!(pow2_q24(0), 1 << 24, "2^0 = 1");
        assert_eq!(pow2_q24(1 << 16), 2 << 24, "2^1 = 2");
        assert_eq!(pow2_q24(2 << 16), 4 << 24, "2^2 = 4");
        assert_eq!(pow2_q24(-(1 << 16)), 1 << 23, "2^-1 = 0.5");
        assert_eq!(pow2_q24(-(3 << 16)), 1 << 21, "2^-3 = 0.125");
    }

    #[test]
    fn pow2_saturates_rather_than_overflowing() {
        assert_eq!(pow2_q24(20 << 16), i32::MAX, "2^20 vastly exceeds Q8.24's range");
        assert!(pow2_q24(-(30 << 16)) >= 0, "a deeply negative exponent must not go negative");
    }

    #[test]
    fn pow2_and_f32_agree_and_pow2_and_f32_match_f64() {
        for numerator in [-40, -12, -3, -1, 0, 1, 3, 7, 12] {
            let x_q16 = numerator * (1 << 16) / 4; // quarter-integer exponents
            let fixed = pow2_q24(x_q16);
            let float = pow2_f32(x_q16);
            assert_eq!(float, fixed as f32 * Q24_TO_F32, "f32 twin must derive from the fixed table");
            let reference = 2f64.powf(x_q16 as f64 / 65_536.0);
            let relative = (fixed as f64 / (1i64 << 24) as f64 - reference).abs() / reference;
            assert!(relative < 1e-5, "x_q16 {x_q16}: fixed {fixed} vs reference {reference}, relative {relative}");
        }
    }

    #[test]
    fn exp_neg_matches_f64_exp() {
        for numerator in [0, 1, 2, 4, 8, 16, 32] {
            let x_q16 = numerator << 16;
            let fixed = exp_neg_q24(x_q16) as f64 / (1i64 << 24) as f64;
            let reference = (-(numerator as f64)).exp();
            assert!((fixed - reference).abs() < reference.max(1e-6) * 1e-4 + 1e-6, "x={numerator}: fixed {fixed} vs reference {reference}");
        }
    }

    #[test]
    fn log2_is_exact_at_powers_of_two() {
        for shift in 0u32..24 {
            assert_eq!(log2_q16(1 << shift), (shift as i32) << 16, "log2(2^{shift})");
        }
    }

    #[test]
    fn log2_matches_f64_within_the_stated_error() {
        for value in [1u32, 2, 3, 5, 7, 10, 100, 1_000, 65_535, 1_000_000, u32::MAX] {
            let fixed = log2_q16(value) as f64 / 65_536.0;
            let reference = (value as f64).log2();
            assert!((fixed - reference).abs() < f64::from(2f32.powi(-12)), "value {value}: fixed {fixed} vs reference {reference}");
        }
    }

    #[test]
    fn log2_of_zero_does_not_panic_and_returns_the_most_negative_value() {
        assert_eq!(log2_q16(0), i32::MIN);
        assert_eq!(log2_f32(0), f32::NEG_INFINITY);
    }

    #[test]
    fn log2_and_f32_twin_agree() {
        for value in [1u32, 17, 256, 70_000] {
            assert_eq!(log2_f32(value), log2_q16(value) as f32 * Q16_TO_F32);
        }
    }

    #[test]
    fn db_to_gain_is_unity_at_zero_db() {
        assert_eq!(db_to_gain_q15(0), 32_768);
        assert_eq!(db_to_gain_f32(0), 1.0);
    }

    #[test]
    fn db_to_gain_matches_f64_within_a_fraction_of_a_percent() {
        // At the deepest cuts the true gain is itself well under one Q1.15 LSB
        // (`1/32768 ≈ -90.3 dB`), so a *relative* bound alone is unmeetable — accept
        // either bound, matching Q1.15's own resolution floor.
        const Q15_LSB: f64 = 1.0 / 32_768.0;
        for centi_db in [-9_600, -4_800, -2_000, -600, -100, 0, 100, 600, 1_200, 2_400] {
            let fixed = db_to_gain_q15(centi_db) as f64 / 32_768.0;
            let reference = 10f64.powf(centi_db as f64 / 2_000.0);
            let relative = (fixed - reference).abs() / reference;
            let absolute = (fixed - reference).abs();
            assert!(relative < 1e-3 || absolute < Q15_LSB, "centi_db {centi_db}: fixed {fixed} vs reference {reference}");
        }
    }

    #[test]
    fn db_to_gain_clamps_extreme_input() {
        assert_eq!(db_to_gain_q15(i32::MAX), db_to_gain_q15(CENTI_DB_MAX));
        assert_eq!(db_to_gain_q15(i32::MIN), db_to_gain_q15(CENTI_DB_MIN));
    }

    #[test]
    fn gain_to_centi_db_is_the_approximate_inverse_of_db_to_gain() {
        // Near the -96 dB floor, `db_to_gain_q15` itself has only one or two Q1.15 units
        // to represent the gain in (see `db_to_gain_matches_f64_within_a_fraction_of_a_percent`),
        // so the round trip through that coarse an intermediate is only loosely invertible;
        // the tolerance widens there rather than pretending Q1.15 has resolution it does not.
        for centi_db in [-9_600, -4_800, -1_200, -100, 0, 100, 1_200, 2_400] {
            let gain = db_to_gain_q15(centi_db);
            let round_tripped = gain_to_centi_db(gain);
            let tolerance = if centi_db <= -4_800 { 600 } else { 5 };
            assert!((round_tripped - centi_db).abs() <= tolerance, "centi_db {centi_db} round-tripped to {round_tripped}");
        }
    }

    #[test]
    fn shelf_amplitude_matches_f64_and_is_the_square_root_of_db_to_gain_at_double_the_db() {
        for centi_db in [-2_400i32, -1_200, -600, 0, 600, 1_200, 2_400] {
            let fixed = shelf_amplitude_q24(centi_db) as f64 / (1i64 << 24) as f64;
            let reference = 10f64.powf(centi_db as f64 / 4_000.0);
            let relative = (fixed - reference).abs() / reference;
            assert!(relative < 1e-5, "centi_db {centi_db}: fixed {fixed} vs reference {reference}");
        }
        // `A = 10^(centi_db/4000)`, so `A^2 = 10^(centi_db/2000)`, which is
        // `db_to_gain_q15`'s own formula at the *same* `centi_db` — not at double it.
        for centi_db in [-2_400i32, -1_200, -600, 0, 600, 1_200, 2_400] {
            let fixed = shelf_amplitude_q24(centi_db) as f64 / (1i64 << 24) as f64;
            let gain = db_to_gain_q15(centi_db) as f64 / 32_768.0;
            assert!((fixed * fixed - gain).abs() < 1e-3, "A^2 should equal db_to_gain_q15 at the same centi_db {centi_db}");
        }
    }

    #[test]
    fn shelf_amplitude_f32_twin_agrees_with_the_fixed_value() {
        for centi_db in [-2_400i32, 0, 2_400] {
            assert_eq!(shelf_amplitude_f32(centi_db), shelf_amplitude_q24(centi_db) as f32 * Q24_TO_F32);
        }
    }

    #[test]
    fn gain_to_centi_db_of_zero_or_negative_is_the_floor() {
        assert_eq!(gain_to_centi_db(0), CENTI_DB_MIN);
        assert_eq!(gain_to_centi_db(-100), CENTI_DB_MIN);
        assert_eq!(gain_to_centi_db_f32(0), CENTI_DB_MIN as f32);
    }

    #[test]
    fn sine_hits_the_four_quadrant_landmarks_exactly() {
        assert_eq!(sin_q15(0), 0);
        assert_eq!(sin_q15(1 << 30), 32_767, "sin(pi/2)");
        assert_eq!(sin_q15(2 << 30), 0, "sin(pi) rounds to zero");
        assert_eq!(sin_q15(3 << 30), -32_767, "sin(3pi/2)");
        assert_eq!(cos_q15(0), 32_767, "cos(0)");
        assert_eq!(cos_q15(1 << 30), 0, "cos(pi/2) rounds to zero");
        assert_eq!(cos_q15(2 << 30), -32_767, "cos(pi)");
    }

    #[test]
    fn sine_matches_f64_across_a_full_turn() {
        for step in 0u32..360 {
            let phase = (step as u64 * (1u64 << 32) / 360) as u32;
            let fixed_sin = sin_q15(phase) as f64 / 32_767.0;
            let reference_sin = (step as f64 * core::f64::consts::PI / 180.0).sin();
            assert!((fixed_sin - reference_sin).abs() < 2e-3, "step {step}: sin {fixed_sin} vs {reference_sin}");

            let fixed_cos = cos_q15(phase) as f64 / 32_767.0;
            let reference_cos = (step as f64 * core::f64::consts::PI / 180.0).cos();
            assert!((fixed_cos - reference_cos).abs() < 2e-3, "step {step}: cos {fixed_cos} vs {reference_cos}");
        }
    }

    #[test]
    fn sine_f32_twin_agrees_with_the_fixed_table() {
        for phase in [0u32, 1 << 28, 1 << 30, 1 << 31, 3 << 30, u32::MAX] {
            assert_eq!(sin_f32(phase), sin_q15(phase) as f32 * SINE_TO_F32);
            assert_eq!(cos_f32(phase), cos_q15(phase) as f32 * SINE_TO_F32);
        }
    }

    #[test]
    fn sine_never_exceeds_its_stated_unity() {
        for step in 0u32..1024 {
            let phase = step.wrapping_mul(4_194_304); // spread across the full turn
            assert!(sin_q15(phase).abs() <= SINE_UNITY, "phase {phase}");
            assert!(cos_q15(phase).abs() <= SINE_UNITY, "phase {phase}");
        }
    }

    #[test]
    fn time_constant_decays_towards_one_as_the_time_grows() {
        let short = time_constant_q24(1, 44_100);
        let long = time_constant_q24(1_000, 44_100);
        assert!(short < long, "a shorter time constant should be a smaller coefficient");
        assert!(long < (1 << 24), "even a long time constant never reaches unity");
        assert!(short >= 0);
    }

    #[test]
    fn time_constant_matches_f64_reference() {
        for milliseconds in [1i32, 10, 100, 1_000] {
            for sample_rate_hz in [8_000u32, 44_100, 48_000, 96_000] {
                let fixed = time_constant_q24(milliseconds, sample_rate_hz) as f64 / (1i64 << 24) as f64;
                let reference = (-1.0 / (milliseconds as f64 / 1_000.0 * sample_rate_hz as f64)).exp();
                assert!((fixed - reference).abs() < reference.max(1e-6) * 1e-3 + 1e-6, "ms {milliseconds} sr {sample_rate_hz}: fixed {fixed} vs reference {reference}");
            }
        }
    }

    #[test]
    fn time_constant_does_not_panic_or_divide_by_zero_on_absurd_input() {
        for milliseconds in [0i32, -1, i32::MIN] {
            for sample_rate_hz in [0u32, 1, u32::MAX] {
                let coefficient = time_constant_q24(milliseconds, sample_rate_hz);
                // `(1 << 24)` (unity) is reachable, and correctly so: a negligible-length
                // time constant against an enormous sample rate makes `1/(t*sr)` round to
                // zero, and `exp(-0)` is exactly one — no decay at all in the limit.
                assert!((0..=(1 << 24)).contains(&coefficient), "ms {milliseconds} sr {sample_rate_hz}: {coefficient}");
            }
        }
    }

    #[test]
    fn time_constant_f32_twin_agrees_with_the_fixed_value() {
        assert_eq!(time_constant_f32(100, 44_100), time_constant_q24(100, 44_100) as f32 * Q24_TO_F32);
    }
}
