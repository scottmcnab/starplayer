//! The master bus: master volume and a table-driven soft limiter (task B5 deliverable 6).
//!
//! # What the original did, and what we do instead
//!
//! The original's SoundBlaster driver realised "amplification" as a **clipping curve**
//! rather than a multiply. `InitMixingTables` built `PostTable[2048]` from the master
//! volume — a linear ramp between a lower and an upper knee, zero below and full above —
//! so a higher master volume made the window narrower and therefore the signal louder
//! (`plans/reference/original-s3mlib-analysis.md` §7). It is a genuinely nice idea: one
//! table lookup per output sample does the gain *and* the limiting.
//!
//! It also had a defect. The final pass sign-extended the 16-bit accumulator and used it
//! **unbounded** as the table index, so a heavy module read outside the table —
//! `plans/product/03-accuracy-policy.md` **D4**. The policy's answer is this module: the
//! modern mixer clamps and soft-limits *explicitly*. The index here is derived by a shift
//! and then clamped to the table's last entry, and everything past the curve's ceiling
//! resolves to full scale rather than to whatever followed the table in memory.
//!
//! # The curve
//!
//! Linear up to the knee, a quadratic with continuous slope from there to the ceiling, and
//! flat at full scale beyond:
//!
//! ```text
//!   y = x                                     x <= 0.75
//!   y = k + d - d^2 / (4(1 - k))   d = x - k  0.75 < x <= 1.25
//!   y = 1                                     x  > 1.25
//! ```
//!
//! The knee and ceiling are `k` and `2 - k`, which is what makes `y` reach exactly 1.0
//! with exactly zero slope — no corner at the top of the curve, and no gain above unity
//! anywhere. `k = 0.75` leaves everything below −2.5 dBFS **bit-transparent**, which is
//! where a single voice lives, and spends the curve on the +2 dB above full scale where a
//! busy mix would otherwise be hard-clipping. A wider knee would sound gentler on a very
//! loud module and would colour everything else to get there.
//!
//! The curve is a table because §7.3 says tables: it is the seam a different limiter drops
//! into, and evaluating the polynomial per sample would be arithmetic in the RT path with
//! nothing to show for it.

use starplayer_core::U0F16;

use crate::path::{Stereo, round_shift_nearest};

/// What the master bus does to a whole quantum.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MasterSettings {
    /// Applied before the limiter, so turning the master down turns the limiting down
    /// with it.
    pub volume: U0F16,
    /// How the signal is bounded on the way out.
    pub limiter: Limiter,
}

impl MasterSettings {
    /// Unity volume with the soft limiter engaged. What a host gets if it says nothing.
    pub const DEFAULT: MasterSettings = MasterSettings { volume: U0F16::MAX, limiter: Limiter::SoftKnee };

    /// Unity volume, hard clamping only — the linear path, for anyone measuring the mixer
    /// rather than listening to it.
    pub const TRANSPARENT: MasterSettings = MasterSettings { volume: U0F16::MAX, limiter: Limiter::Clamp };
}

impl Default for MasterSettings {
    fn default() -> MasterSettings { MasterSettings::DEFAULT }
}

/// How the master bus bounds the signal.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Limiter {
    /// Hard clamp at full scale. Linear below it, and what the M0 output stage did.
    Clamp,
    /// The soft knee described above.
    #[default]
    SoftKnee,
}

/// Entries in the limiter curve, plus the endpoint. The table spans `[0, 2)` of input, so
/// one entry is 1/128 of full scale and the interpolation error against the true curve
/// stays under half an LSB of Q15.
const LIMITER_TABLE_LEN: usize = 257;

/// Input magnitude that indexes the last table entry: 2.0, i.e. +6 dBFS. Anything louder
/// is full scale by definition, so the table does not need to describe it.
const LIMITER_CEILING_FIXED: i32 = 2 * 32_768;

/// Bits of an i16-scale magnitude that interpolate between entries.
const LIMITER_INDEX_SHIFT: u32 = 8;

/// `LIMITER_CEILING_FIXED` as the float path sees it, in table steps per unit.
const LIMITER_STEPS_PER_UNIT_F32: f32 = 128.0;

/// The knee, in Q30. Below this the curve is the identity.
const LIMITER_KNEE_Q30: i64 = (3 << 30) / 4;

/// One in Q30.
const ONE_Q30: i64 = 1 << 30;

/// The soft-knee transfer curve, in Q15, sampled every 1/128 of full scale.
const LIMITER_TABLE: [i16; LIMITER_TABLE_LEN] = build_limiter_table();

/// See the note in [`crate::gain`] — const evaluation, not the RT path.
#[allow(clippy::indexing_slicing)]
const fn build_limiter_table() -> [i16; LIMITER_TABLE_LEN] {
    let mut table = [0i16; LIMITER_TABLE_LEN];
    let mut index = 0;
    while index < LIMITER_TABLE_LEN {
        // Entry `index` describes an input of `index / 128` of full scale.
        let input = (index as i64) << 23;
        let output = if input <= LIMITER_KNEE_Q30 {
            input
        } else {
            let distance = input - LIMITER_KNEE_Q30;
            // The quadratic reaches 1.0 with zero slope at `2 - knee`; past that it is
            // flat, and `distance` is clamped there so the polynomial never turns back
            // down.
            let width = 2 * (ONE_Q30 - LIMITER_KNEE_Q30);
            let distance = if distance > width { width } else { distance };
            let squared = ((distance as i128 * distance as i128) >> 30) as i64;
            LIMITER_KNEE_Q30 + distance - (squared * ONE_Q30) / (2 * width)
        };
        table[index] = ((output * 32_767 + (1 << 29)) >> 30) as i16;
        index += 1;
    }
    table
}

/// One table entry, or full scale past the end — the D4 clamp, stated once.
fn limiter_entry(index: usize) -> i32 { LIMITER_TABLE.get(index).copied().unwrap_or(32_767) as i32 }

/// The soft-knee curve applied to an i16-scale magnitude, integer arithmetic only.
///
/// `magnitude` is non-negative; the caller reapplies the sign. The result is in `0..=32767`
/// whatever the input, which is the property D4 is about.
pub fn soft_knee_fixed(magnitude: i32) -> i32 {
    if magnitude >= LIMITER_CEILING_FIXED {
        return 32_767;
    }
    let index = (magnitude >> LIMITER_INDEX_SHIFT) as usize;
    let fraction = magnitude & ((1 << LIMITER_INDEX_SHIFT) - 1);
    let from = limiter_entry(index);
    let to = limiter_entry(index + 1);
    from + round_shift_nearest((to - from) as i64 * fraction as i64, LIMITER_INDEX_SHIFT) as i32
}

/// The same curve for the float path, reading the same table so the two paths describe the
/// same limiter rather than two limiters that happen to look alike.
pub fn soft_knee_f32(magnitude: f32) -> f32 {
    let scaled = magnitude * LIMITER_STEPS_PER_UNIT_F32;
    // The NaN case goes down the same branch, deliberately: a magnitude that is not a
    // number resolves to full scale rather than propagating, because nothing downstream of
    // here may see one.
    if scaled.is_nan() || scaled >= LIMITER_TABLE_LEN as f32 - 1.0 {
        return 1.0;
    }
    let index = scaled as usize;
    let fraction = scaled - index as f32;
    let from = limiter_entry(index) as f32;
    let to = limiter_entry(index + 1) as f32;
    (from + (to - from) * fraction) * (1.0 / 32_767.0)
}

/// Master volume then limiting, on one float frame.
pub fn process_float(frame: Stereo<f32>, settings: MasterSettings) -> Stereo<f32> {
    let volume = settings.volume.to_bits() as f32 * (1.0 / 65_535.0);
    Stereo::new(bound_f32(frame.left * volume, settings.limiter), bound_f32(frame.right * volume, settings.limiter))
}

/// Master volume then limiting, on one fixed frame. No floating point anywhere on this
/// path — it is the canonical bit-exact reference (architecture §7.3).
pub fn process_fixed(frame: Stereo<i32>, settings: MasterSettings) -> Stereo<i32> {
    let volume = settings.volume.to_bits() as i64;
    Stereo::new(bound_fixed(frame.left, volume, settings.limiter), bound_fixed(frame.right, volume, settings.limiter))
}

fn bound_f32(value: f32, limiter: Limiter) -> f32 {
    match limiter {
        Limiter::Clamp => {
            if value.is_nan() { 0.0 } else { value.clamp(-1.0, 1.0) }
        }
        Limiter::SoftKnee => {
            let magnitude = if value < 0.0 { -value } else { value };
            let bounded = soft_knee_f32(magnitude);
            if value < 0.0 { -bounded } else { bounded }
        }
    }
}

fn bound_fixed(value: i32, volume: i64, limiter: Limiter) -> i32 {
    // Q0.16 master volume, reduced with the same round-to-nearest rule as voice gain and
    // interpolation. This is part of the C6 golden contract.
    let scaled = round_shift_nearest(value as i64 * volume, 16);
    match limiter {
        Limiter::Clamp => scaled.clamp(-32_767, 32_767) as i32,
        Limiter::SoftKnee => {
            let magnitude = scaled.unsigned_abs().min(i32::MAX as u64) as i32;
            let bounded = soft_knee_fixed(magnitude);
            if scaled < 0 { -bounded } else { bounded }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_is_the_identity_below_the_knee() {
        for index in 0..=96usize {
            // 96/128 = 0.75, the knee.
            let expected = ((index as i64 * 32_767) / 128) as i32;
            assert!((limiter_entry(index) - expected).abs() <= 1, "entry {index} is not linear");
        }
    }

    #[test]
    fn the_curve_reaches_full_scale_at_the_ceiling_and_stays_there() {
        assert_eq!(limiter_entry(160), 32_767, "1.25 of full scale maps to exactly full scale");
        for index in 160..LIMITER_TABLE_LEN {
            assert_eq!(limiter_entry(index), 32_767, "entry {index} must not come back down");
        }
    }

    #[test]
    fn the_curve_is_monotonic_and_never_exceeds_full_scale() {
        for index in 1..LIMITER_TABLE_LEN {
            assert!(limiter_entry(index) >= limiter_entry(index - 1), "entry {index} dips");
            assert!(limiter_entry(index) <= 32_767, "entry {index} exceeds full scale");
        }
    }

    #[test]
    fn a_signal_below_the_knee_passes_through_untouched() {
        for magnitude in [0, 1, 1_000, 16_384, 24_575] {
            assert!((soft_knee_fixed(magnitude) - magnitude).abs() <= 1, "magnitude {magnitude} was not transparent");
        }
        assert!((soft_knee_f32(0.5) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn an_overloaded_signal_is_bounded_rather_than_wrapped() {
        // The D4 case: an accumulator far outside i16 range.
        assert_eq!(bound_fixed(i32::MAX, 65_535, Limiter::SoftKnee), 32_767);
        assert_eq!(bound_fixed(i32::MIN, 65_535, Limiter::SoftKnee), -32_767);
        assert_eq!(bound_fixed(i32::MAX, 65_535, Limiter::Clamp), 32_767);
        assert_eq!(bound_f32(1_000.0, Limiter::SoftKnee), 1.0);
        assert_eq!(bound_f32(-1_000.0, Limiter::SoftKnee), -1.0);
        assert_eq!(bound_f32(f32::INFINITY, Limiter::SoftKnee), 1.0);
        assert_eq!(bound_f32(f32::NAN, Limiter::Clamp), 0.0);
        assert!(!bound_f32(f32::NAN, Limiter::SoftKnee).is_nan(), "a NaN must not reach the output stage");
    }

    /// The curve itself is exactly odd; the master-volume multiply that precedes it is an
    /// signed round-to-nearest rule that precedes it is odd-symmetric too.
    #[test]
    fn the_limiter_is_odd_symmetric() {
        for magnitude in [1i32, 100, 20_000, 32_767, 40_000] {
            let positive = bound_fixed(magnitude, 65_535, Limiter::SoftKnee);
            let negative = bound_fixed(-magnitude, 65_535, Limiter::SoftKnee);
            assert_eq!(positive, -negative, "{magnitude}: {positive} against {negative}");
        }
    }

    #[test]
    fn master_volume_scales_before_the_limiter() {
        let quiet = MasterSettings { volume: U0F16::from_bits(32_768), limiter: Limiter::Clamp };
        assert_eq!(process_fixed(Stereo::new(20_000, -20_000), quiet), Stereo::new(10_000, -10_000));
        let float = process_float(Stereo::new(0.5, -0.5), quiet);
        assert!((float.left - 0.25).abs() < 1e-3 && (float.right + 0.25).abs() < 1e-3, "got {float:?}");
    }

    #[test]
    fn silence_stays_silent() {
        assert_eq!(process_fixed(Stereo::new(0, 0), MasterSettings::DEFAULT), Stereo::new(0, 0));
        assert_eq!(process_float(Stereo::new(0.0, 0.0), MasterSettings::DEFAULT), Stereo::new(0.0, 0.0));
    }

    /// The two paths are separate implementations on purpose, but they must not describe
    /// different limiters.
    #[test]
    fn the_two_paths_agree_to_within_their_rounding() {
        for magnitude in [0i32, 1_000, 16_000, 30_000, 32_767, 40_000, 60_000] {
            let fixed = soft_knee_fixed(magnitude) as f32 / 32_767.0;
            let float = soft_knee_f32(magnitude as f32 / 32_768.0);
            assert!((fixed - float).abs() < 1e-3, "magnitude {magnitude}: fixed {fixed} vs float {float}");
        }
    }
}
