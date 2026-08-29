//! The pan law, the gain-unit space the mixer ramps in, and [`RAMP_FRAMES`].
//!
//! # The pan law is constant-power, from a table (task B5 deliverable 1)
//!
//! `pan_gains_q15` reads a quarter-wave sine table, so a voice's two channel gains are
//! `(cos θ, sin θ)` for `θ = (pan + 1)/2 × π/2`: hard left is `(1, 0)`, hard right is
//! `(0, 1)`, and centre is `(0.7071, 0.7071)`. The sum of the squares is constant, so a
//! voice keeps a constant *perceived* loudness as it sweeps — which is the whole point,
//! because S3M's `Xxx`, IT's panbrello and MOD's `8xx` all sweep pan while a note sounds,
//! and a law that changes loudness while they do makes every pan slide sound like a
//! volume slide as well.
//!
//! **The two laws that were on the table**, and why this one:
//!
//! | Law | Centre gain per channel | Sweep behaviour |
//! |---|---|---|
//! | Balance (what M0-A3 shipped) | 1.0 | +3 dB louder in the middle |
//! | Linear (`L = 1 - p`, `R = p`) | 0.5 | −3 dB quieter in the middle for uncorrelated material |
//! | **Constant power (this)** | **0.7071** | **flat** |
//!
//! The original specifies none of them: its SoundBlaster driver is 8-bit *mono* and
//! "panning is computed and stored but never used", and GUS emulation is explicitly not
//! offered (`plans/product/03-accuracy-policy.md` §4). So there is no canonical behaviour
//! to match here and the choice is a modern one, made on the merits. Note the audible
//! consequence, since it is a real change from M0: a centred voice is now 3 dB quieter
//! than it was relative to a hard-panned one. That is the correct direction — under the
//! balance law a centred voice was 3 dB *louder* in total power than the same voice panned
//! hard left, which is what makes mono-ish modules clip first.
//!
//! Swapping the law later costs one table: nothing outside this module knows the shape of
//! the curve, only that it hands back a Q15 pair.
//!
//! # No `sin` at run time, and none at compile time either
//!
//! Architecture §7.3 bans transcendental functions from the RT path — different libms
//! disagree, and the fixed path has to be bit-identical on x86, ARM and WASM. The table is
//! built by a `const fn` that evaluates a nine-term Taylor series in
//! Q30 **integer** arithmetic (`build_pan_table`, below), so the numbers are baked into
//! the binary by `rustc`'s own
//! const evaluator rather than by whatever libm the build host happens to have. A
//! hand-pasted literal table would have been equally correct and rather less checkable.

use starplayer_core::{I1F15, U0F16};

use crate::path::Stereo;

/// How long a volume or pan change takes to arrive, in output frames (task B5 research
/// point 2).
///
/// # Why 64
///
/// 64 frames is 1.45 ms at 44.1 kHz and 1.33 ms at 48 kHz, which is the length the
/// question really turns on:
///
/// * **Long enough to kill the click.** A gain step is a discontinuity; smearing it over
///   ~1.4 ms puts the energy it radiates below ~700 Hz, where a step's click lives. This
///   is the same order as every mixer that ramps — OpenMPT's default is 0.5 ms with an
///   option up to 5 ms — and the same order as the GF1 hardware ramp the original leaned
///   on (`plans/reference/original-s3mlib-analysis.md` §7: the GUS driver ramped the old
///   note out, took the ramp-end IRQ, and started the new note from the handler).
/// * **Short enough not to eat a retrigger.** The fastest thing S3M can ask for is `Qx1`,
///   a retrigger every tick. A tick at the default speed 6 / 125 BPM is 20 ms — 882 frames
///   at 44.1 kHz — so even `Qx1` gets 93% of its attack. Only a tempo above ~900 BPM would
///   make the ramp a meaningful fraction of a tick.
/// * **A power of two that divides `RENDER_QUANTUM`**, so a ramp spans at most two
///   quanta. That is a convenience, not a correctness property: the ramp is a function of
///   frames elapsed, never of block boundaries (see [`GainRamp`](starplayer_dsp::GainRamp)).
///
/// It is a frame count rather than a duration because the mixer does not know the output
/// rate — nothing below the engine does. A rate-aware ramp length would be a `MixerConfig`
/// field, and is not worth one until a host asks for it.
pub const RAMP_FRAMES: u32 = 64;

/// Unity in the mixer's internal gain-unit space: `U0F16::MAX × I1F15::MAX`.
///
/// A voice's gain is `volume × pan_gain` kept at full width — 31 bits — rather than being
/// rounded down to Q15 before it is ramped. That matters: at a tracker volume of 1/64 and
/// centre pan, a Q15 composite would have nine bits left to ramp through, and a tremolo
/// would step audibly. The 16 extra bits are free — both paths have to scale the value
/// anyway, so the shift or multiply that does it costs the same whichever space it starts
/// from.
pub const GAIN_UNITY: i32 = U0F16_FULL_SCALE * PAN_FULL_SCALE;

/// `U0F16::MAX`, the volume scale's unity.
const U0F16_FULL_SCALE: i32 = u16::MAX as i32;

/// `I1F15::MAX`, the pan table's unity.
const PAN_FULL_SCALE: i32 = i16::MAX as i32;

/// Fractional bits of the gain-unit space that a Q15 gain does not have. The fixed path's
/// gain is `units >> GAIN_FRACTION_BITS`.
pub const GAIN_FRACTION_BITS: u32 = 16;

/// Entries in the quarter-wave pan table, plus the endpoint. 257 entries put a table step
/// at 1/256 of the full sweep, which is finer than any format's pan resolution (IT's 64
/// positions plus surround is the widest) — and the low bits of the pan value are
/// interpolated between entries anyway, so the law is continuous over the full 16-bit
/// range of [`I1F15`].
const PAN_TABLE_LEN: usize = 257;

/// Bits of the pan position that index the table; the rest interpolate between entries.
const PAN_INDEX_SHIFT: u32 = 8;

/// `sin(θ) × 32767` for `θ` from 0 to π/2 in 256 steps.
const PAN_TABLE: [i16; PAN_TABLE_LEN] = build_pan_table();

/// π/2 in Q30.
const QUARTER_TURN_Q30: i64 = 1_686_629_713;

/// Q30 multiply, widened so the product cannot overflow.
const fn mul_q30(left: i64, right: i64) -> i64 { ((left as i128 * right as i128) >> 30) as i64 }

/// `sin(angle)` for `angle` in Q30 over `[0, π/2]`, by Taylor series.
///
/// Nine terms is exact to about `1e-9` over the quarter wave, which is three orders of
/// magnitude finer than the Q15 the result is rounded to — so the table is the correctly
/// rounded sine, and it is reproducible because every operation here is an integer one.
const fn sin_q30(angle: i64) -> i64 {
    let squared = mul_q30(angle, angle);
    let mut term = angle;
    let mut sum = angle;
    let mut order = 1i64;
    while order <= 4 {
        term = mul_q30(term, squared) / ((2 * order) * (2 * order + 1));
        if order % 2 == 1 { sum -= term } else { sum += term }
        order += 1;
    }
    sum
}

/// Indexing an array by a loop variable is exactly what `clippy::indexing_slicing` exists
/// to stop in the render path, and `get_mut` is not available in a `const fn`. This runs
/// in the const evaluator, never at run time, and its loop bound *is* the array length.
#[allow(clippy::indexing_slicing)]
const fn build_pan_table() -> [i16; PAN_TABLE_LEN] {
    let mut table = [0i16; PAN_TABLE_LEN];
    let mut index = 0;
    while index < PAN_TABLE_LEN {
        let angle = (QUARTER_TURN_Q30 * index as i64) / (PAN_TABLE_LEN as i64 - 1);
        // Round to nearest on the way into Q15; the table is read millions of times and
        // built once.
        table[index] = ((sin_q30(angle) * PAN_FULL_SCALE as i64 + (1 << 29)) >> 30) as i16;
        index += 1;
    }
    table
}

/// One table entry, or zero if the index is out of range — which it never is, but
/// `render()` may not panic and the mixer denies slice indexing outright.
fn pan_entry(index: usize) -> i32 { PAN_TABLE.get(index).copied().unwrap_or(0) as i32 }

/// Interpolate between two table entries. `fraction` is `PAN_INDEX_SHIFT` bits wide.
const fn pan_lerp(from: i32, to: i32, fraction: i32) -> i32 {
    from + (((to - from) * fraction) >> PAN_INDEX_SHIFT)
}

/// The constant-power channel gains for a pan position, as a Q15 pair.
///
/// Hard left is exactly `(32767, 0)` and centre is `(23170, 23170)`. Hard *right* is
/// `(0, 32766)` rather than `(0, 32767)`: [`I1F15::MAX`] is `32767/32768`, one LSB short of
/// a mathematical 1.0, so the sweep is one table step short of its far end. The asymmetry
/// is [`I1F15`]'s, not the table's, and one part in 32768 of gain is 55 dB below anything
/// audible.
pub fn pan_gains_q15(pan: I1F15) -> Stereo<i32> {
    // 0 at hard left, 65535 at hard right.
    let position = (pan.to_bits() as i32 + 32_768) as u32;
    let index = (position >> PAN_INDEX_SHIFT) as usize;
    let fraction = (position & ((1 << PAN_INDEX_SHIFT) - 1)) as i32;

    // Right reads the table forwards (sine), left reads it backwards (cosine): one table,
    // two channels, and exact mirror symmetry between them by construction.
    let right = pan_lerp(pan_entry(index), pan_entry(index + 1), fraction);
    let left = pan_lerp(pan_entry(PAN_TABLE_LEN - 1 - index), pan_entry(PAN_TABLE_LEN - 2 - index), fraction);
    Stereo::new(left, right)
}

/// A voice's left/right gain in gain units — volume folded together with the pan law.
///
/// This is the value the mixer ramps. Folding the two together before ramping is what lets
/// one ramp serve `VOLUME`, `PAN` and `STOP` alike, and it means a pan slide and a volume
/// slide landing on the same frame produce one smooth movement rather than two ramps
/// fighting over the same gain.
pub fn voice_gain_units(volume: U0F16, pan: I1F15) -> Stereo<i32> {
    let volume = volume.to_bits() as i32;
    let pan = pan_gains_q15(pan);
    Stereo::new(volume * pan.left, volume * pan.right)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is a sine to within half an LSB of Q15 — checked against values that can
    /// be written down exactly rather than against another implementation of the same
    /// series.
    #[test]
    fn the_table_is_a_quarter_wave_sine() {
        assert_eq!(pan_entry(0), 0, "sin 0");
        assert_eq!(pan_entry(PAN_TABLE_LEN - 1), 32_767, "sin pi/2");
        assert_eq!(pan_entry(128), 23_170, "sin pi/4 = 0.70710678 x 32767 = 23170.0");
        assert_eq!(pan_entry(64), 12_539, "sin pi/8 = 0.38268343 x 32767 = 12539.4");
        assert_eq!(pan_entry(192), 30_273, "sin 3pi/8 = 0.92387953 x 32767 = 30272.8");
    }

    #[test]
    fn the_table_is_monotonic() {
        for index in 1..PAN_TABLE_LEN {
            assert!(pan_entry(index) > pan_entry(index - 1), "entry {index} is not above its predecessor");
        }
    }

    /// The property the law is chosen for: the total power is the same everywhere.
    #[test]
    fn the_law_is_constant_power_across_the_whole_sweep() {
        let unity = PAN_FULL_SCALE as i64 * PAN_FULL_SCALE as i64;
        for step in 0..=256 {
            let pan = I1F15::from_bits((step * 256 - 32_768).clamp(-32_768, 32_767) as i16);
            let gains = pan_gains_q15(pan);
            let power = gains.left as i64 * gains.left as i64 + gains.right as i64 * gains.right as i64;
            // Half a per mille, which is the table's own rounding and nothing more.
            assert!((power - unity).abs() * 2_000 < unity, "step {step}: power {power} strays from {unity}");
        }
    }

    #[test]
    fn hard_pan_silences_the_far_channel() {
        assert_eq!(pan_gains_q15(I1F15::MIN), Stereo::new(32_767, 0), "hard left");
        assert_eq!(pan_gains_q15(I1F15::ZERO), Stereo::new(23_170, 23_170), "centre is -3 dB on both channels");
        let hard_right = pan_gains_q15(I1F15::MAX);
        assert_eq!(hard_right.left, 0, "hard right silences the left channel");
        assert_eq!(hard_right.right, 32_766, "one LSB short, because I1F15::MAX is one LSB short of 1.0");
    }

    #[test]
    fn the_law_is_symmetric_about_centre() {
        for step in 1..=127i32 {
            let bits = (step * 256) as i16;
            let right_of_centre = pan_gains_q15(I1F15::from_bits(bits));
            let left_of_centre = pan_gains_q15(I1F15::from_bits(-bits));
            assert_eq!(right_of_centre.left, left_of_centre.right, "step {step}");
            assert_eq!(right_of_centre.right, left_of_centre.left, "step {step}");
        }
    }

    #[test]
    fn the_law_is_continuous() {
        let mut previous = pan_gains_q15(I1F15::MIN);
        for bits in -32_767..=32_767i32 {
            let gains = pan_gains_q15(I1F15::from_bits(bits as i16));
            assert!((gains.right - previous.right) <= 1, "a one-LSB pan change moved the gain by more than one LSB at {bits}");
            assert!((previous.left - gains.left) <= 1, "a one-LSB pan change moved the gain by more than one LSB at {bits}");
            previous = gains;
        }
    }

    #[test]
    fn gain_units_span_zero_to_unity() {
        assert_eq!(voice_gain_units(U0F16::ZERO, I1F15::ZERO), Stereo::new(0, 0));
        assert_eq!(voice_gain_units(U0F16::MAX, I1F15::MIN), Stereo::new(GAIN_UNITY, 0), "full volume hard left is exactly unity");
        let centre = voice_gain_units(U0F16::MAX, I1F15::ZERO);
        assert_eq!(centre.left, centre.right);
        assert!(centre.left < GAIN_UNITY && centre.left > GAIN_UNITY / 2, "centre sits at 1/sqrt(2) of unity, not at unity");
    }

    #[test]
    fn unity_fits_a_signed_32_bit_gain() {
        assert_eq!(GAIN_UNITY, 2_147_385_345);
        const { assert!(GAIN_UNITY < i32::MAX, "the gain-unit space must not overflow the ramp's i32") };
        assert_eq!(GAIN_UNITY >> GAIN_FRACTION_BITS, 32_766, "one LSB below Q15 unity, as it was in M0");
    }
}
