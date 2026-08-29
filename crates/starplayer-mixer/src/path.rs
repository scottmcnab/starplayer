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
//! # Gains are computed once per segment
//!
//! Volume and pan are folded into a left/right gain pair at the start of each render
//! segment, not per frame. Because segments are split at event boundaries and quantum
//! boundaries — both of which are absolute-frame properties — the gain applied to any
//! given output frame is the same whatever host block size asked for it. That is half of
//! why the block-size determinism test passes; the other half is that the per-frame
//! arithmetic below never depends on the segment length.
//!
//! Per-frame gain *ramping* (M1) will be driven by the voice's own absolute frame
//! position for exactly the same reason.

use starplayer_core::{I1F15, U0F16};
use starplayer_dsp::Interpolate;

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

    /// Fold volume and pan into a left/right gain pair.
    fn gains(volume: U0F16, pan: I1F15) -> Stereo<Self::Gain>;

    /// Interpolate one source frame and add it to `destination`.
    fn mix<Interp: Interpolate>(
        destination: &mut Self::Accumulator,
        frames: &[i16],
        index: usize,
        fraction_bits: u32,
        gains: Stereo<Self::Gain>,
    );
}

/// The float mixing path: `f32` accumulator, output normalised to ±1.0.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FloatPath;

impl MixPath for FloatPath {
    type Accumulator = FloatFrame;
    type Gain = f32;

    fn gains(volume: U0F16, pan: I1F15) -> Stereo<f32> {
        // `1/32768` folds the raw `i16` scale of the interpolator's output into the gain,
        // so the inner loop is one multiply rather than two. It is a power of two, so it
        // is exact.
        let volume = volume.to_bits() as f32 * (1.0 / 65_535.0) * (1.0 / 32_768.0);
        let balance = pan_balance_q15(pan);
        Stereo::new(volume * (balance.left as f32 * Q15_SCALE_F32), volume * (balance.right as f32 * Q15_SCALE_F32))
    }

    fn mix<Interp: Interpolate>(destination: &mut FloatFrame, frames: &[i16], index: usize, fraction_bits: u32, gains: Stereo<f32>) {
        let value = Interp::sample_f32(frames, index, fraction_bits);
        destination.left += value * gains.left;
        destination.right += value * gains.right;
    }
}

/// The fixed-point mixing path: `i16` sample, `i32` accumulator, integer arithmetic only.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FixedPath;

impl MixPath for FixedPath {
    type Accumulator = FixedFrame;
    /// Q15 gain: `32767` is unity.
    type Gain = i32;

    fn gains(volume: U0F16, pan: I1F15) -> Stereo<i32> {
        let volume = volume.to_bits() as i32;
        let balance = pan_balance_q15(pan);
        // Q0.16 volume times Q15 balance, back down to Q15. Truncating, deliberately:
        // the fixed path's rounding rules are stated once and then never change, because
        // the goldens encode them.
        Stereo::new((volume * balance.left) >> 16, (volume * balance.right) >> 16)
    }

    fn mix<Interp: Interpolate>(destination: &mut FixedFrame, frames: &[i16], index: usize, fraction_bits: u32, gains: Stereo<i32>) {
        let value = Interp::sample_fixed(frames, index, fraction_bits) as i64;
        destination.left = destination.left.saturating_add(((value * gains.left as i64) >> 15) as i32);
        destination.right = destination.right.saturating_add(((value * gains.right as i64) >> 15) as i32);
    }
}

/// `1.0 / 32767`, the reciprocal of Q15 unity.
const Q15_SCALE_F32: f32 = 1.0 / 32_767.0;

/// Pan as a Q15 left/right gain pair, using a **balance** law: centre is unity on both
/// channels, hard left silences the right channel and vice versa.
///
/// This is a placeholder, and a deliberately cheap one — no square roots and no tables,
/// so it costs nothing and commits to nothing. The pan law that matters is the one each
/// format actually uses (S3M's 16 positions, IT's 64 plus surround, MOD's fixed LRRL),
/// and it lands with the format crates in M1 as a table lookup, never as a `sin`
/// (architecture §7.3 bans transcendental functions from the RT path).
fn pan_balance_q15(pan: I1F15) -> Stereo<i32> {
    // `I1F15::MIN` is an exact -1.0 with no positive counterpart; clamping to the
    // symmetric range keeps the two channels' gains mirror images of one another.
    let pan = pan.to_bits().max(-32_767) as i32;
    Stereo::new(32_767 - pan.max(0), 32_767 + pan.min(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer_dsp::Nearest;

    #[test]
    fn centre_pan_is_unity_on_both_channels() {
        let balance = pan_balance_q15(I1F15::ZERO);
        assert_eq!(balance, Stereo::new(32_767, 32_767));
    }

    #[test]
    fn hard_pan_silences_the_far_channel() {
        assert_eq!(pan_balance_q15(I1F15::MAX), Stereo::new(0, 32_767));
        assert_eq!(pan_balance_q15(I1F15::MIN), Stereo::new(32_767, 0), "an exact -1.0 clamps into the symmetric range");
    }

    #[test]
    fn full_volume_centre_reproduces_the_source_on_the_fixed_path() {
        let gains = FixedPath::gains(U0F16::MAX, I1F15::ZERO);
        let mut frame = FixedFrame::default();
        FixedPath::mix::<Nearest>(&mut frame, &[20_000], 0, 0, gains);
        // Unity is one LSB below 1.0 on both scales and both shifts truncate, so a
        // full-scale voice lands a couple of LSBs low. Asserted exactly, because this is
        // the path the goldens fingerprint.
        assert_eq!(frame, Stereo::new(19_998, 19_998));
    }

    #[test]
    fn the_float_path_normalises_to_plus_minus_one() {
        let gains = FloatPath::gains(U0F16::MAX, I1F15::ZERO);
        let mut frame = FloatFrame::default();
        FloatPath::mix::<Nearest>(&mut frame, &[-32_768], 0, 0, gains);
        assert!(frame.left >= -1.0 && frame.left <= -0.999, "got {}", frame.left);
        assert_eq!(frame.left, frame.right);
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
}
