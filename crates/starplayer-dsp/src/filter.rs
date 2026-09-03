//! Impulse Tracker's per-voice resonant low-pass filter: the coefficients, and the
//! two-pole step that spends them.
//!
//! # What this is a filter *of*
//!
//! IT's filter is a **voice**-level effect, not a DSP-graph insert (architecture §7.2;
//! M6 master plan). It sits between the resampler and the pan gains — OpenMPT's
//! `SampleLoop` in `soundlib/MixerInterface.h` runs `interpolate(); filter(); mix();` in
//! exactly that order — so every voice carries its own two delay-line values, and two
//! voices of the same instrument on the same channel filter independently.
//!
//! # The law
//!
//! From OpenMPT's `CSoundFile::SetupChannelFilter` (`soundlib/Snd_flt.cpp`), on the
//! `kITFilterBehaviour` branch that every IT module takes:
//!
//! ```text
//! frequency = 110 · 2^(0.25 + cutoff/24) Hz, clamped to [120, 20000] and then to sr/2
//! r         = sr / (2π · frequency)
//! damping   = 10^(-resonance · (24/128) / 20)          ── "2 × damping factor"
//! d         = damping·r + damping − 1
//! e         = r²
//! input_gain =        1 / (1 + d + e)
//! feedback_1 = (d + 2e) / (1 + d + e)
//! feedback_2 =       −e / (1 + d + e)
//! y[n] = input_gain·x[n] + feedback_1·y[n−1] + feedback_2·y[n−2]
//! ```
//!
//! With the extended filter range (IT header bit; OpenMPT's `SONG_EXFILTERRANGE`) the
//! exponent's divisor becomes 20 rather than 24 — a top cutoff of 10670 Hz instead of
//! 5124 Hz, and 10670 is exactly the figure `Snd_flt.cpp`'s own header comment quotes for
//! MPT 1.16 — *and* the coefficients come from OpenMPT's other branch:
//!
//! ```text
//! t = min((1 − 2·damping) / r, 2)
//! d = (2·damping − t) · r
//! e = r²                                               ── unchanged
//! ```
//!
//! # No transcendental function anywhere
//!
//! Design goal 5 bans `pow`, `exp` and friends from the real-time path, and both of the
//! law's transcendentals are tables:
//!
//! * `2^(0.25 + cutoff/24)` is `2^(n/768)` at `n = 192 + 32·cutoff` — an exact integer
//!   index into [`LINEAR_FREQUENCY_TABLE`], because `768/24 = 32` and `768/4 = 192`.
//!   The extended range's `2^(0.25 + cutoff/20)` lands on *fifths* of a table step
//!   (`768/20 = 38.4`), so it interpolates between two neighbouring entries instead of
//!   rounding the index; the entries are `2^(1/768)` apart, so the chord error is under
//!   `10⁻⁷` relative.
//! * `10^(−resonance·(24/128)/20)` is [`IT_RESONANCE_TABLE_Q24`], the table Schism
//!   Tracker ships as `resonance_table` (`player/filters.c`) and OpenMPT computes with
//!   `std::pow` in `Snd_flt.cpp`. Transcribed here as Q0.24 integers so the fixed path
//!   never touches a float.
//!
//! Everything after the two tables is `+ − × ÷` on integers (fixed path) or on `f32`
//! (float path), which architecture §7.3 permits: they are correctly rounded on every
//! IEEE target and the workspace builds with `-C llvm-args=-fp-contract=off`, so no
//! multiply-add is fused behind our back (`cargo xtask ci --job fma-check`).

use starplayer_core::tables::{LINEAR_FREQUENCY_TABLE, LINEAR_FREQUENCY_TABLE_LEN};

use crate::interpolate::round_shift_nearest;

/// The two-pole low-pass coefficients one voice is currently filtering with.
///
/// `Sample` is the mixing path's mono sample type: `f32` on the float path, and a
/// Q8.24 `i32` on the fixed path (see [`FILTER_FRACTION_BITS`]).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct FilterCoefficients<Sample> {
    /// `a0` — what the incoming sample is scaled by.
    pub input_gain: Sample,
    /// `b0` — what `y[n−1]` is scaled by.
    pub feedback_1: Sample,
    /// `b1` — what `y[n−2]` is scaled by.
    pub feedback_2: Sample,
}

impl FilterCoefficients<f32> {
    /// Pass the input through untouched. What an inactive filter holds, so that a
    /// coefficient set is never a meaningful-looking zero.
    pub const PASS_THROUGH: FilterCoefficients<f32> = FilterCoefficients { input_gain: 1.0, feedback_1: 0.0, feedback_2: 0.0 };
}

impl FilterCoefficients<i32> {
    /// Pass the input through untouched.
    pub const PASS_THROUGH: FilterCoefficients<i32> = FilterCoefficients { input_gain: 1 << FILTER_FRACTION_BITS, feedback_1: 0, feedback_2: 0 };
}

/// Fractional bits in a fixed-path coefficient: Q8.24 in an `i32`, which is OpenMPT's own
/// `MIXING_FILTER_PRECISION` (`soundlib/Mixer.h`, `static_assert(… == 24)`).
///
/// Research point 3. Q8.24 is the format because the *coefficients*, not the samples, set
/// the requirement, and their range is a property of the law rather than of the input:
///
/// * `input_gain = 1/(1+d+e)` is largest where `1+d+e = damping·(r+1) + r²` is smallest.
///   `r = sr/(2π·frequency)` bottoms out at `1/π ≈ 0.3183` (the cutoff clamped to
///   Nyquist) and `damping` at `0.0645` (resonance 127), giving `1+d+e ≥ 0.1863` and
///   `input_gain ≤ 5.37`.
/// * `feedback_1 = (d + 2e)/(1+d+e)` runs from `−3.83` at that same corner up towards
///   `+2` as `r` grows; `feedback_2 = −e/(1+d+e)` stays inside `(−1, 0]`.
///
/// So eight integer bits — a range of ±128 — is more than twenty times the worst case,
/// and the remaining 24 fractional bits put the quantisation of the coefficients at
/// `6 × 10⁻⁸`, far below the `f32` path's own `1.2 × 10⁻⁷`. The alternative of trading
/// integer bits for fractional ones buys nothing measurable and would make the format
/// differ from the reference implementation's for no reason.
pub const FILTER_FRACTION_BITS: u32 = 24;

/// Bits of pre-amplification applied to a sample before it enters the fixed-path filter,
/// and removed again on the way out: OpenMPT's `MIXING_FILTER_PREAMP` of 256
/// (`soundlib/IntMixer.h`).
///
/// The delay line, not the output, is what needs this. At a low cutoff and a high sample
/// rate `input_gain` is small and `y[n]` is a *fraction* of an `i16` LSB for many frames
/// running; without the pre-amplification the state would quantise to zero and the filter
/// would output silence. Eight bits of headroom below the sample costs nothing —
/// `i16 × 2 × 256` is `2²⁴`, and the products stay inside `i64` with 8 bits to spare.
pub const FILTER_PREAMP_BITS: u32 = 8;

/// The fixed path's delay line is clamped to **twice** the input range before it is fed
/// back, in the pre-amplified domain: OpenMPT's `ClipFilter`, `int16 × 2 × PREAMP`.
///
/// This is the saturation the resonant case needs. A two-pole with `feedback_1` near 2
/// and `feedback_2` near −1 rings; on a pathological input it can ring past the point
/// where the state would overflow, and clamping the *feedback* — rather than the output —
/// bounds the recursion without flattening the resonant peak that is the whole point of
/// the effect. The bound is asymmetric because `i16` is.
const FILTER_STATE_MAX: i32 = i16::MAX as i32 * 2 * (1 << FILTER_PREAMP_BITS);
const FILTER_STATE_MIN: i32 = i16::MIN as i32 * 2 * (1 << FILTER_PREAMP_BITS);

/// The float path's equivalent bound. OpenMPT clamps to ±2.0 on a signal normalised to
/// ±1.0; the mono value here is on the raw `i16` scale, so the same bound is ±65536.
const FILTER_STATE_LIMIT_F32: f32 = 2.0 * 32_768.0;

/// `2 × damping factor` for each of IT's 128 resonance values, in Q0.24.
///
/// `10^(−(24/128)·i/20)`, which is `pow(10, -3·i/320)` — the formula Schism Tracker
/// states above the `resonance_table[128]` it ships in `player/filters.c`, and the one
/// OpenMPT evaluates inline as
/// `std::pow(10.0f, -resonance × ((24.0f/128.0f)/20.0f))` in `Snd_flt.cpp`. Neither
/// reference can be used as-is: OpenMPT calls `pow` at run time and Schism's table is
/// `f32` literals, and design goal 5 wants the fixed path free of both. The values here
/// are Schism's, rounded to Q0.24; `it_resonance_table_matches_the_reference` checks
/// every entry against the formula and
/// `it_resonance_table_matches_schisms_own_literals` against Schism's printed constants.
///
/// Entry 0 is exactly unity (`2²⁴`), which is why resonance 0 leaves `d = r` and the
/// filter unresonant.
pub const IT_RESONANCE_TABLE_Q24: [u32; 128] = [
    16_777_216, 16_418_932, 16_068_299, 15_725_154, 15_389_336, 15_060_691, 14_739_064, 14_424_305,
    14_116_268, 13_814_809, 13_519_788, 13_231_068, 12_948_513, 12_671_992, 12_401_377, 12_136_540,
    11_877_359, 11_623_713, 11_375_484, 11_132_556, 10_894_816, 10_662_153, 10_434_458, 10_211_626,
    9_993_552, 9_780_136, 9_571_277, 9_366_879, 9_166_845, 8_971_083, 8_779_502, 8_592_012,
    8_408_526, 8_228_959, 8_053_226, 7_881_246, 7_712_939, 7_548_226, 7_387_031, 7_229_278,
    7_074_893, 6_923_806, 6_775_945, 6_631_242, 6_489_629, 6_351_041, 6_215_412, 6_082_679,
    5_952_781, 5_825_657, 5_701_248, 5_579_495, 5_460_343, 5_343_735, 5_229_617, 5_117_937,
    5_008_641, 4_901_679, 4_797_002, 4_694_560, 4_594_306, 4_496_192, 4_400_174, 4_306_207,
    4_214_246, 4_124_249, 4_036_174, 3_949_980, 3_865_627, 3_783_075, 3_702_285, 3_623_222,
    3_545_846, 3_470_123, 3_396_017, 3_323_494, 3_252_519, 3_183_061, 3_115_085, 3_048_561,
    2_983_458, 2_919_745, 2_857_392, 2_796_372, 2_736_654, 2_678_212, 2_621_017, 2_565_044,
    2_510_267, 2_456_659, 2_404_196, 2_352_854, 2_302_607, 2_253_434, 2_205_311, 2_158_216,
    2_112_126, 2_067_021, 2_022_879, 1_979_680, 1_937_403, 1_896_029, 1_855_538, 1_815_912,
    1_777_133, 1_739_181, 1_702_041, 1_665_693, 1_630_121, 1_595_309, 1_561_241, 1_527_900,
    1_495_271, 1_463_339, 1_432_089, 1_401_506, 1_371_576, 1_342_285, 1_313_620, 1_285_568,
    1_258_114, 1_231_246, 1_204_952, 1_179_220, 1_154_037, 1_129_392, 1_105_274, 1_081_670,
];

/// `round(2π × 2³²)`. The one irrational constant here, folded to an integer at
/// StarPlayer's own build time rather than computed on a caller's machine.
const TWO_PI_Q32: u64 = 26_986_075_409;

/// The largest `r = sr/(2π·frequency)` the coefficient maths will consider, so that `r²`
/// cannot leave `u64` however absurd a sample rate a host asks for. `r = 4096` is a
/// cutoff frequency 25,000 times below the sample rate — a 3 MHz device at IT's lowest
/// cutoff — so no reachable configuration is clamped by it.
const MAX_R_Q32: u64 = 4096 << 32;

/// The IT filter coefficients on the float path.
///
/// `cutoff` and `resonance` are IT's own seven-bit values; anything above 127 is clamped,
/// as `SetupChannelFilter` clamps them. `extended_range` is the IT header's extended
/// filter range bit.
pub fn resonant_low_pass_f32(cutoff: u8, resonance: u8, sample_rate_hz: u32, extended_range: bool) -> FilterCoefficients<f32> {
    let design = FilterDesign::new(cutoff, resonance, sample_rate_hz, extended_range);
    // `r` and `damping` come out of the shared integer front end so that the two paths
    // filter at the same frequency; the algebra after them is OpenMPT's own, in `f32`.
    let damping = design.damping_q24 as f32 * (1.0 / 16_777_216.0);
    let r = design.r_q32 as f32 * (1.0 / 4_294_967_296.0);
    let e = r * r;
    let d = if extended_range {
        let mut t = (1.0 - 2.0 * damping) / r;
        if t > 2.0 {
            t = 2.0;
        }
        (2.0 * damping - t) * r
    } else {
        damping * r + damping - 1.0
    };
    let denominator = 1.0 + d + e;
    if denominator.is_nan() || denominator <= 0.0 {
        return FilterCoefficients::<f32>::PASS_THROUGH;
    }
    FilterCoefficients {
        input_gain: 1.0 / denominator,
        feedback_1: (d + e + e) / denominator,
        feedback_2: -e / denominator,
    }
}

/// The IT filter coefficients on the fixed path, in Q8.24 — integer arithmetic
/// throughout, so the result is bit-identical on x86, ARM and WASM.
pub fn resonant_low_pass_fixed(cutoff: u8, resonance: u8, sample_rate_hz: u32, extended_range: bool) -> FilterCoefficients<i32> {
    let design = FilterDesign::new(cutoff, resonance, sample_rate_hz, extended_range);
    let damping_q32 = (design.damping_q24 as i64) << 8;
    let r_q32 = design.r_q32 as i64;
    // `e = r²`, kept in Q32: `r` is at most `MAX_R_Q32`, so `e` is at most 2^24 and the
    // widened product is comfortably inside `u64`.
    let e_q32 = ((design.r_q32 as u128 * design.r_q32 as u128) >> 32) as i64;
    let d_q32 = if extended_range {
        let mut t_q32 = ((((1i64 << 32) - 2 * damping_q32) as i128) << 32) / r_q32 as i128;
        if t_q32 > (2i128 << 32) {
            t_q32 = 2i128 << 32;
        }
        (((2 * damping_q32 as i128 - t_q32) * r_q32 as i128) >> 32) as i64
    } else {
        (((damping_q32 as i128 * r_q32 as i128) >> 32) as i64) + damping_q32 - (1i64 << 32)
    };
    let denominator_q32 = (1i64 << 32) + d_q32 + e_q32;
    if denominator_q32 <= 0 {
        return FilterCoefficients::<i32>::PASS_THROUGH;
    }
    let denominator = denominator_q32 as i128;
    let input_gain = divide_to_q24(1i128 << 32, denominator);
    FilterCoefficients {
        // OpenMPT floors the same value to zero and then forces it back to one, "to
        // prevent silence at low filter cutoff and very high sampling rate".
        input_gain: if input_gain == 0 { 1 } else { input_gain },
        feedback_1: divide_to_q24(d_q32 as i128 + 2 * e_q32 as i128, denominator),
        feedback_2: divide_to_q24(-(e_q32 as i128), denominator),
    }
}

/// One two-pole step on the float path.
///
/// `value` and the delay line are both on the raw `i16` scale the interpolators produce,
/// so the filter runs before the pan gains and before any normalisation.
#[inline]
pub fn resonate_f32(value: f32, state: &mut [f32; 2], coefficients: &FilterCoefficients<f32>) -> f32 {
    let [previous, older] = *state;
    let filtered = value * coefficients.input_gain
        + clamp_state_f32(previous) * coefficients.feedback_1
        + clamp_state_f32(older) * coefficients.feedback_2;
    *state = [filtered, previous];
    filtered
}

/// One two-pole step on the fixed path.
///
/// `value` is one interpolated sample on the raw `i16` scale; the delay line is kept in
/// the pre-amplified domain (see [`FILTER_PREAMP_BITS`]) and the return value is brought
/// back out of it, rounded to nearest with ties away from zero — the C6 rule every other
/// fixed-path precision reduction uses.
///
/// # Headroom
///
/// A resonant two-pole overshoots: a full-scale input can leave here at several times
/// full scale, exactly as it does in OpenMPT, and that is the sound. Nothing is clamped
/// on the way out, because the mixer's accumulator already saturates
/// (`FixedPath::mix`'s `saturating_add`) and the master bus bounds the quantum after it.
/// The internal arithmetic is nonetheless proved not to overflow: the pre-amplified
/// input is at most `2²³`, the clamped delay line at most `2²⁴`, and even against a
/// coefficient saturated to `i32::MAX` the widened sum stays below `2⁵⁷`.
#[inline]
pub fn resonate_fixed(value: i32, state: &mut [i32; 2], coefficients: &FilterCoefficients<i32>) -> i32 {
    let [previous, older] = *state;
    let amplified = (value as i64) << FILTER_PREAMP_BITS;
    let accumulated = amplified * coefficients.input_gain as i64
        + clamp_state_fixed(previous) as i64 * coefficients.feedback_1 as i64
        + clamp_state_fixed(older) as i64 * coefficients.feedback_2 as i64;
    let filtered = round_shift_nearest(accumulated, FILTER_FRACTION_BITS).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    *state = [filtered, previous];
    round_shift_nearest(filtered as i64, FILTER_PREAMP_BITS) as i32
}

/// What both paths share: the cutoff frequency, and the two values the algebra is written
/// in terms of.
struct FilterDesign {
    /// `damping`, in Q0.24 — one entry of [`IT_RESONANCE_TABLE_Q24`].
    damping_q24: u32,
    /// `r = sr / (2π · frequency)`, in Q32.32.
    r_q32: u64,
}

impl FilterDesign {
    fn new(cutoff: u8, resonance: u8, sample_rate_hz: u32, extended_range: bool) -> FilterDesign {
        let frequency_q24 = cutoff_frequency_q24(cutoff, sample_rate_hz, extended_range);
        // `2π·frequency` in Q24, then `r` in Q32. Both are widened: `2π·20000·2²⁴` alone
        // is 2^41, and the numerator below carries 56 more bits.
        let two_pi_frequency_q24 = ((TWO_PI_Q32 as u128 * frequency_q24 as u128) >> 32).max(1);
        let r_q32 = (((sample_rate_hz.max(1) as u128) << 56) / two_pi_frequency_q24) as u64;
        let index = (resonance.min(127)) as usize;
        let damping_q24 = IT_RESONANCE_TABLE_Q24[index];
        FilterDesign { damping_q24, r_q32: r_q32.clamp(1, MAX_R_Q32) }
    }
}

/// The cutoff frequency in Hz, Q40.24, with OpenMPT's clamps applied in OpenMPT's order:
/// `Limit(frequency, 120, 20000)` and only then `LimitMax(frequency, sr/2)`.
fn cutoff_frequency_q24(cutoff: u8, sample_rate_hz: u32, extended_range: bool) -> u64 {
    let cutoff = cutoff.min(127) as u32;
    let unclamped = if extended_range {
        // `0.25 + cutoff/20` octaves is `(960 + 192·cutoff)/3840`, and 3840 is five times
        // the table's own 768, so the exact index is a fifth of a step. Interpolate.
        let fifths = 960 + 192 * cutoff;
        let units = fifths / 5;
        let remainder = (fifths % 5) as u64;
        let base = exp2_units_q24(units);
        let next = exp2_units_q24(units + 1);
        110 * (base + (next - base) * remainder / 5)
    } else {
        // `0.25 + cutoff/24` octaves is exactly `(192 + 32·cutoff)/768`.
        110 * exp2_units_q24(192 + 32 * cutoff)
    };
    let nyquist_q24 = (sample_rate_hz.max(1) as u64) << 23;
    unclamped.clamp(120 << 24, 20_000 << 24).min(nyquist_q24)
}

/// `2^(units/768)` in Q40.24, for the `units` this module asks for (at most 5069, so at
/// most seven octaves and nowhere near `u64`).
fn exp2_units_q24(units: u32) -> u64 {
    let index = (units as usize) % LINEAR_FREQUENCY_TABLE_LEN;
    let octave = units / LINEAR_FREQUENCY_TABLE_LEN as u32;
    let entry = LINEAR_FREQUENCY_TABLE.get(index).copied().unwrap_or(1 << 24);
    (entry as u64) << octave
}

/// `numerator_q32 / denominator_q32` as a Q8.24 coefficient, rounded to nearest with ties
/// away from zero and saturated into `i32`.
fn divide_to_q24(numerator_q32: i128, denominator_q32: i128) -> i32 {
    let scaled = numerator_q32 << FILTER_FRACTION_BITS;
    let half = denominator_q32 / 2;
    let rounded = if scaled < 0 { (scaled - half) / denominator_q32 } else { (scaled + half) / denominator_q32 };
    rounded.clamp(i32::MIN as i128, i32::MAX as i128) as i32
}

#[inline]
fn clamp_state_fixed(value: i32) -> i32 { value.clamp(FILTER_STATE_MIN, FILTER_STATE_MAX) }

#[inline]
fn clamp_state_f32(value: f32) -> f32 { value.clamp(-FILTER_STATE_LIMIT_F32, FILTER_STATE_LIMIT_F32) }

#[cfg(test)]
mod tests {
    use super::*;

    /// Schism Tracker's `resonance_table` (`player/filters.c`), transcribed **as
    /// printed** — every digit Schism's own source carries, so that the transcription can
    /// be diffed character-for-character against the reference. `f32` keeps more digits
    /// than it can represent, which is the point: the literals round to exactly the
    /// `float` values Schism's compiler produces.
    #[allow(clippy::excessive_precision)]
    const SCHISM_RESONANCE_TABLE: [f32; 128] = [
        1.0000000000000000, 0.9786446094512940, 0.9577452540397644, 0.9372922182083130,
        0.9172759056091309, 0.8976871371269226, 0.8785166740417481, 0.8597555756568909,
        0.8413951396942139, 0.8234267830848694, 0.8058421611785889, 0.7886331081390381,
        0.7717915177345276, 0.7553095817565918, 0.7391796708106995, 0.7233941555023193,
        0.7079457640647888, 0.6928272843360901, 0.6780316829681397, 0.6635520458221436,
        0.6493816375732422, 0.6355138421058655, 0.6219421625137329, 0.6086603403091431,
        0.5956621170043945, 0.5829415321350098, 0.5704925656318665, 0.5583094954490662,
        0.5463865399360657, 0.5347182154655457, 0.5232990980148315, 0.5121238231658936,
        0.5011872053146362, 0.4904841780662537, 0.4800096750259399, 0.4697588682174683,
        0.4597269892692566, 0.4499093294143677, 0.4403013288974762, 0.4308985173702240,
        0.4216965138912201, 0.4126909971237183, 0.4038778245449066, 0.3952528536319733,
        0.3868120610713959, 0.3785515129566193, 0.3704673945903778, 0.3625559210777283,
        0.3548133969306946, 0.3472362160682678, 0.3398208320140839, 0.3325638175010681,
        0.3254617750644684, 0.3185114264488220, 0.3117094635963440, 0.3050527870655060,
        0.2985382676124573, 0.2921628654003143, 0.2859236001968384, 0.2798175811767578,
        0.2738419771194458, 0.2679939568042755, 0.2622708380222321, 0.2566699385643005,
        0.2511886358261108, 0.2458244115114212, 0.2405747324228287, 0.2354371547698975,
        0.2304092943668366, 0.2254888117313385, 0.2206734120845795, 0.2159608304500580,
        0.2113489061594009, 0.2068354636430740, 0.2024184018373489, 0.1980956792831421,
        0.1938652694225311, 0.1897251904010773, 0.1856735348701477, 0.1817083954811096,
        0.1778279393911362, 0.1740303486585617, 0.1703138649463654, 0.1666767448186874,
        0.1631172895431519, 0.1596338599920273, 0.1562248021364212, 0.1528885662555695,
        0.1496235728263855, 0.1464282870292664, 0.1433012634515762, 0.1402409970760346,
        0.1372461020946503, 0.1343151479959488, 0.1314467936754227, 0.1286396980285645,
        0.1258925348520279, 0.1232040524482727, 0.1205729842185974, 0.1179980933666229,
        0.1154781952500343, 0.1130121126770973, 0.1105986908078194, 0.1082368120551109,
        0.1059253737330437, 0.1036632955074310, 0.1014495193958283, 0.0992830246686935,
        0.0971627980470657, 0.0950878411531448, 0.0930572077631950, 0.0910699293017387,
        0.0891250967979431, 0.0872217938303947, 0.0853591337800026, 0.0835362523794174,
        0.0817523002624512, 0.0800064504146576, 0.0782978758215904, 0.0766257941722870,
        0.0749894231557846, 0.0733879879117012, 0.0718207582831383, 0.0702869966626167,
        0.0687859877943993, 0.0673170387744904, 0.0658794566988945, 0.0644725710153580,
    ];

    /// The `f64` law both references state, evaluated with the transcendental this module
    /// is not allowed to call at run time.
    fn reference_damping(resonance: u8) -> f64 { f64::powf(10.0, -3.0 * resonance as f64 / 320.0) }

    fn reference_frequency(cutoff: u8, sample_rate_hz: u32, extended_range: bool) -> f64 {
        let divisor = if extended_range { 20.0 } else { 24.0 };
        let raw = 110.0 * f64::exp2(0.25 + cutoff as f64 / divisor);
        raw.clamp(120.0, 20_000.0).min(sample_rate_hz as f64 / 2.0)
    }

    #[test]
    fn it_resonance_table_matches_the_reference() {
        for (resonance, &entry) in IT_RESONANCE_TABLE_Q24.iter().enumerate() {
            let expected = (reference_damping(resonance as u8) * 16_777_216.0).round() as u32;
            assert_eq!(entry, expected, "resonance {resonance}");
        }
        assert_eq!(IT_RESONANCE_TABLE_Q24[0], 1 << 24, "no resonance is a damping factor of exactly one");
    }

    /// Schism's constants are `f32` literals, so they already carry a rounding of their
    /// own: at resonance 40 the `f32` lands exactly halfway between two Q0.24 integers and
    /// rounds the other way from the `f64` law. One LSB of Q0.24 — `6 × 10⁻⁸` of the
    /// damping factor — is the whole disagreement, and the shipped table follows the law
    /// rather than the `f32`.
    #[test]
    fn it_resonance_table_matches_schisms_own_literals() {
        for (resonance, &printed) in SCHISM_RESONANCE_TABLE.iter().enumerate() {
            let expected = (printed as f64 * 16_777_216.0).round() as i64;
            let entry = IT_RESONANCE_TABLE_Q24.get(resonance).copied().unwrap_or(0) as i64;
            assert!((entry - expected).abs() <= 1, "resonance {resonance}: {entry} against Schism's {expected}");
        }
    }

    #[test]
    fn the_cutoff_law_reproduces_the_documented_range_endpoints() {
        // `Snd_flt.cpp`'s own header comment says the extended range "upped this to the
        // current 10670 Hz", which is what `110 · 2^(0.25 + 127/20)` comes to — the
        // strongest available check that the exponent's divisor is the right one.
        let top = cutoff_frequency_q24(127, 192_000, false) as f64 / 16_777_216.0;
        assert!((top - 5_123.9).abs() < 0.5, "standard top cutoff {top}");
        let extended = cutoff_frequency_q24(127, 192_000, true) as f64 / 16_777_216.0;
        assert!((extended - 10_670.6).abs() < 0.5, "extended top cutoff {extended}");
    }

    #[test]
    fn the_cutoff_law_matches_the_transcendental_at_every_value() {
        for extended_range in [false, true] {
            for cutoff in 0..=127u8 {
                let expected = reference_frequency(cutoff, 192_000, extended_range);
                let actual = cutoff_frequency_q24(cutoff, 192_000, extended_range) as f64 / 16_777_216.0;
                let relative = (actual - expected).abs() / expected;
                assert!(relative < 1e-6, "cutoff {cutoff} extended {extended_range}: {actual} vs {expected}");
            }
        }
    }

    #[test]
    fn the_cutoff_clamps_follow_openmpt_in_openmpts_order() {
        // 110·2^0.25 is 130.8 Hz, above the 120 Hz floor, so the floor only bites through
        // the Nyquist limit below it.
        assert_eq!(cutoff_frequency_q24(0, 44_100, false), cutoff_frequency_q24(0, 96_000, false));
        assert_eq!(cutoff_frequency_q24(127, 8_000, false), 4_000 << 24, "the cutoff never passes Nyquist");
        assert_eq!(cutoff_frequency_q24(127, 200, false), 100 << 24, "Nyquist wins over the 120 Hz floor, as in OpenMPT");
    }

    /// The reference algebra in `f64`, for the two paths to be compared against.
    fn reference_coefficients(cutoff: u8, resonance: u8, sample_rate_hz: u32, extended_range: bool) -> (f64, f64, f64) {
        let damping = reference_damping(resonance);
        let r = sample_rate_hz as f64 / (2.0 * core::f64::consts::PI * reference_frequency(cutoff, sample_rate_hz, extended_range));
        let e = r * r;
        let d = if extended_range {
            let t = ((1.0 - 2.0 * damping) / r).min(2.0);
            (2.0 * damping - t) * r
        } else {
            damping * r + damping - 1.0
        };
        let denominator = 1.0 + d + e;
        (1.0 / denominator, (d + e + e) / denominator, -e / denominator)
    }

    #[test]
    fn both_paths_reproduce_the_reference_algebra() {
        for extended_range in [false, true] {
            for cutoff in [0u8, 1, 32, 64, 100, 126, 127] {
                for resonance in [0u8, 1, 32, 64, 96, 127] {
                    let (gain, feedback_1, feedback_2) = reference_coefficients(cutoff, resonance, 44_100, extended_range);
                    let float = resonant_low_pass_f32(cutoff, resonance, 44_100, extended_range);
                    assert!((float.input_gain as f64 - gain).abs() < 1e-5, "float gain, cutoff {cutoff} resonance {resonance}");
                    assert!((float.feedback_1 as f64 - feedback_1).abs() < 1e-5, "float b0, cutoff {cutoff} resonance {resonance}");
                    assert!((float.feedback_2 as f64 - feedback_2).abs() < 1e-5, "float b1, cutoff {cutoff} resonance {resonance}");

                    let fixed = resonant_low_pass_fixed(cutoff, resonance, 44_100, extended_range);
                    let scale = (1 << FILTER_FRACTION_BITS) as f64;
                    assert!((fixed.input_gain as f64 / scale - gain).abs() < 1e-5, "fixed gain, cutoff {cutoff} resonance {resonance}");
                    assert!((fixed.feedback_1 as f64 / scale - feedback_1).abs() < 1e-5, "fixed b0, cutoff {cutoff} resonance {resonance}");
                    assert!((fixed.feedback_2 as f64 / scale - feedback_2).abs() < 1e-5, "fixed b1, cutoff {cutoff} resonance {resonance}");
                }
            }
        }
    }

    #[test]
    fn the_two_paths_agree_to_within_their_quantisation() {
        for cutoff in 0..=127u8 {
            for resonance in [0u8, 40, 127] {
                let float = resonant_low_pass_f32(cutoff, resonance, 44_100, false);
                let fixed = resonant_low_pass_fixed(cutoff, resonance, 44_100, false);
                let scale = (1 << FILTER_FRACTION_BITS) as f32;
                assert!((fixed.input_gain as f32 / scale - float.input_gain).abs() < 1e-5, "gain at {cutoff}/{resonance}");
                assert!((fixed.feedback_1 as f32 / scale - float.feedback_1).abs() < 1e-5, "b0 at {cutoff}/{resonance}");
                assert!((fixed.feedback_2 as f32 / scale - float.feedback_2).abs() < 1e-5, "b1 at {cutoff}/{resonance}");
            }
        }
    }

    /// Research point 3, checked rather than asserted: sweep every cutoff, every
    /// resonance and a wide spread of sample rates, and confirm that no coefficient ever
    /// needs more than the eight integer bits Q8.24 provides.
    #[test]
    fn no_coefficient_leaves_the_range_q8_24_provides() {
        let limit = 128.0;
        for &sample_rate_hz in &[8_000u32, 11_025, 22_050, 44_100, 48_000, 96_000, 192_000, 384_000] {
            for extended_range in [false, true] {
                for cutoff in 0..=127u8 {
                    for resonance in 0..=127u8 {
                        let coefficients = resonant_low_pass_f32(cutoff, resonance, sample_rate_hz, extended_range);
                        for value in [coefficients.input_gain, coefficients.feedback_1, coefficients.feedback_2] {
                            assert!(value.abs() < limit, "{value} at {sample_rate_hz} Hz, cutoff {cutoff}, resonance {resonance}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_pass_through_filter_returns_its_input() {
        let mut state = [0.0f32; 2];
        assert_eq!(resonate_f32(1234.0, &mut state, &FilterCoefficients::<f32>::PASS_THROUGH), 1234.0);
        let mut state = [0i32; 2];
        assert_eq!(resonate_fixed(1234, &mut state, &FilterCoefficients::<i32>::PASS_THROUGH), 1234);
    }

    #[test]
    fn the_filter_settles_on_a_constant_input_at_unity_gain() {
        // The transfer function at DC is `input_gain / (1 - feedback_1 - feedback_2)`,
        // which the law makes exactly one: a step input settles on itself.
        let coefficients = resonant_low_pass_fixed(40, 0, 44_100, false);
        let mut state = [0i32; 2];
        let mut last = 0;
        for _ in 0..20_000 {
            last = resonate_fixed(10_000, &mut state, &coefficients);
        }
        assert!((last - 10_000).abs() <= 2, "settled at {last}");
    }

    #[test]
    fn a_closed_filter_attenuates_a_fast_alternating_input() {
        let coefficients = resonant_low_pass_fixed(0, 0, 44_100, false);
        let mut state = [0i32; 2];
        let mut peak = 0;
        for index in 0..4_000 {
            let filtered = resonate_fixed(if index % 2 == 0 { 20_000 } else { -20_000 }, &mut state, &coefficients);
            if index > 2_000 {
                peak = peak.max(filtered.abs());
            }
        }
        assert!(peak < 200, "Nyquist should be far down at cutoff zero, peak was {peak}");
    }

    #[test]
    fn the_delay_line_is_clamped_rather_than_allowed_to_run_away() {
        let coefficients = FilterCoefficients { input_gain: 1 << FILTER_FRACTION_BITS, feedback_1: 4 << FILTER_FRACTION_BITS, feedback_2: 0 };
        let mut state = [0i32; 2];
        for _ in 0..64 {
            resonate_fixed(32_767, &mut state, &coefficients);
        }
        let [previous, _] = state;
        // Without the feedback clamp this diverges; with it the state settles at
        // `input + 4 × clamp`, which is finite and well inside `i32`.
        assert!(previous.abs() < i32::MAX / 8, "state ran to {previous}");
    }

    #[test]
    fn an_absurd_sample_rate_neither_panics_nor_divides_by_zero() {
        for sample_rate_hz in [0u32, 1, 2, 200, u32::MAX] {
            for extended_range in [false, true] {
                let fixed = resonant_low_pass_fixed(0, 127, sample_rate_hz, extended_range);
                assert!(fixed.input_gain != 0, "a zero gain would silence the voice");
                let float = resonant_low_pass_f32(0, 127, sample_rate_hz, extended_range);
                assert!(float.input_gain.is_finite() && float.feedback_1.is_finite() && float.feedback_2.is_finite());
            }
        }
    }

    #[test]
    fn values_above_the_seven_bit_range_clamp_rather_than_wrapping() {
        assert_eq!(resonant_low_pass_fixed(200, 200, 44_100, false), resonant_low_pass_fixed(127, 127, 44_100, false));
    }
}
