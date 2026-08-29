//! Resampling kernels: [`Interpolate`], with the two implementations M0 needs.
//!
//! # Why a trait, and why here
//!
//! Architecture §7.1 lists four kernels — nearest, linear, cubic Hermite and windowed
//! sinc — and requires them to be *monomorphised parameters of the mixer's inner loop,
//! never a `dyn` call per sample*. A trait with zero-sized implementations is exactly
//! that: `Path::mix::<Linear>(..)` inlines to the same code a hand-written linear mixer
//! would produce, and swapping kernels costs one type parameter rather than a vtable
//! lookup per output frame.
//!
//! Committing the trait now does not violate the "no trait until its second real
//! implementation exists" rule (architecture §10.1): [`Nearest`] and [`Linear`] both land
//! here, in this commit, and both are exercised by the block-size determinism test.
//!
//! # Two entry points, not a generic sample type
//!
//! Each kernel offers a float method and a fixed-point method rather than being generic
//! over an accumulator type. The two paths are not the same arithmetic expressed over
//! different scalars: the fixed path is *the canonical bit-exact reference* (architecture
//! §7.3) and has to be written in whole integers with a stated rounding rule, while the
//! float path is allowed to multiply and add in `f32`. Papering over that with one
//! generic body would hide the one thing about it that matters.
//!
//! # Reading past the end
//!
//! Every method here reads source frames at and after `index`. Callers guarantee those
//! reads are in bounds by appending guard frames to the sample data
//! (`starplayer_mixer::sample::GUARD_FRAMES`); the out-of-range fallback below returns
//! silence rather than panicking, because nothing in the RT path may panic
//! (architecture §8), but with correctly built sample data it is unreachable.

/// A resampling kernel.
///
/// `fraction_bits` is the Q0.32 fractional part of the sample position: `0` sits exactly
/// on `index`, `0x8000_0000` is halfway to `index + 1`.
///
/// Returned values are on the **raw `i16` scale**, not normalised to ±1.0. Normalisation
/// is folded into the mixer's gain so it costs nothing extra in the inner loop.
pub trait Interpolate {
    /// How many source frames past the last addressable one this kernel reads.
    ///
    /// The mixer's `GUARD_FRAMES` must be at least this large for every kernel that can
    /// be selected at run time, which is why that constant is sized for the widest kernel
    /// planned rather than for the widest kernel implemented.
    const GUARD_FRAMES_REQUIRED: usize;

    /// The float path.
    fn sample_f32(frames: &[i16], index: usize, fraction_bits: u32) -> f32;

    /// The fixed-point path. Integer arithmetic only, so the result is bit-identical on
    /// x86, ARM and WASM.
    fn sample_fixed(frames: &[i16], index: usize, fraction_bits: u32) -> i32;
}

/// No interpolation: take the frame the position currently sits in.
///
/// This is the tracker's classic "no interpolation" — a **truncation**, not a rounding to
/// the nearest frame — and it is what the original's `Mixer_8bitMono` does with its
/// `_Mix_HighSpeed` / `_Mix_LowSpeed` position pair (`STARPLAY/S3MLIB.ASM` ~5795).
/// It corresponds to [`starplayer_core::Interpolator::None`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Nearest;

impl Interpolate for Nearest {
    const GUARD_FRAMES_REQUIRED: usize = 0;

    fn sample_f32(frames: &[i16], index: usize, _fraction_bits: u32) -> f32 { frame_at(frames, index) as f32 }

    fn sample_fixed(frames: &[i16], index: usize, _fraction_bits: u32) -> i32 { frame_at(frames, index) }
}

/// Linear interpolation between `index` and `index + 1`.
///
/// The default kernel and the golden-hash reference (architecture §7.1). Corresponds to
/// [`starplayer_core::Interpolator::Linear`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Linear;

impl Interpolate for Linear {
    const GUARD_FRAMES_REQUIRED: usize = 1;

    fn sample_f32(frames: &[i16], index: usize, fraction_bits: u32) -> f32 {
        let current = frame_at(frames, index) as f32;
        let next = frame_at(frames, index.wrapping_add(1)) as f32;
        // `2^-32` is exact in `f32`, and `u32 -> f32` is IEEE round-to-nearest, so this
        // multiply is reproducible on every target (architecture §7.3 permits `+ - * /`).
        current + (next - current) * (fraction_bits as f32 * FRACTION_SCALE_F32)
    }

    fn sample_fixed(frames: &[i16], index: usize, fraction_bits: u32) -> i32 {
        let current = frame_at(frames, index) as i64;
        let next = frame_at(frames, index.wrapping_add(1)) as i64;
        // Q0.16 of the position fraction: the low 16 bits are dropped rather than
        // rounded, which is what a 16.16-position tracker mixer does and keeps the
        // expression to one multiply and one shift. The 17-bit difference times the
        // 16-bit fraction needs 33 bits, so the intermediate is `i64`.
        let fraction = (fraction_bits >> 16) as i64;
        (current + (((next - current) * fraction) >> 16)) as i32
    }
}

/// `1.0 / 2^32`, exact in `f32`.
const FRACTION_SCALE_F32: f32 = 1.0 / 4_294_967_296.0;

/// Read one source frame, or silence if the index is out of range.
///
/// With correctly guarded sample data the fallback is unreachable; it exists because a
/// corrupt module must degrade to silence rather than panic (architecture §8).
#[inline]
fn frame_at(frames: &[i16], index: usize) -> i32 { frames.get(index).copied().unwrap_or(0) as i32 }

#[cfg(test)]
mod tests {
    use super::*;

    const RAMP: [i16; 4] = [0, 1000, -1000, 32767];

    #[test]
    fn nearest_truncates_rather_than_rounding() {
        assert_eq!(Nearest::sample_fixed(&RAMP, 0, 0), 0);
        assert_eq!(Nearest::sample_fixed(&RAMP, 0, 0xFFFF_FFFF), 0, "just short of the next frame is still this frame");
        assert_eq!(Nearest::sample_fixed(&RAMP, 1, 0x8000_0000), 1000);
    }

    #[test]
    fn linear_hits_the_endpoints_exactly() {
        assert_eq!(Linear::sample_fixed(&RAMP, 0, 0), 0);
        assert_eq!(Linear::sample_fixed(&RAMP, 1, 0), 1000);
        assert_eq!(Linear::sample_f32(&RAMP, 1, 0), 1000.0);
    }

    #[test]
    fn linear_interpolates_the_midpoint() {
        assert_eq!(Linear::sample_fixed(&RAMP, 0, 0x8000_0000), 500);
        assert_eq!(Linear::sample_f32(&RAMP, 0, 0x8000_0000), 500.0);
        assert_eq!(Linear::sample_fixed(&RAMP, 1, 0x8000_0000), 0, "1000 -> -1000 halfway is zero");
    }

    #[test]
    fn linear_fixed_truncates_toward_negative_infinity() {
        // 1000 + ((-2000 * 16384) >> 16) = 1000 - 500 = 500 at a quarter step.
        assert_eq!(Linear::sample_fixed(&RAMP, 1, 0x4000_0000), 500);
        // The arithmetic shift floors, so a descending slope lands one LSB low rather
        // than rounding to nearest. Stated here because the goldens will encode it.
        assert_eq!(Linear::sample_fixed(&[0, -1], 0, 0x0001_0000), -1, "one Q0.16 tick down a descending slope floors to -1");
    }

    #[test]
    fn reads_past_the_end_yield_silence_rather_than_panicking() {
        assert_eq!(Nearest::sample_fixed(&RAMP, 99, 0), 0);
        assert_eq!(Linear::sample_fixed(&RAMP, 3, 0x8000_0000), 16383, "the guard frame reads as silence");
        assert_eq!(Linear::sample_f32(&[], 0, 0), 0.0);
    }
}
