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
//! # Reading past the end, and before the start
//!
//! [`Nearest`] and [`Linear`] read source frames at and after `index`; [`Cubic`] and
//! [`Sinc`] are symmetric and also read up to [`Interpolate::LEADING_FRAMES`] frames
//! *before* it. Callers guarantee both directions are in bounds by giving every sample
//! guard frames after it and a pre-roll before it
//! (`starplayer_core::GUARD_FRAMES`, `starplayer_core::PRE_ROLL_FRAMES`); the
//! out-of-range fallback below returns silence rather than panicking, because nothing in
//! the RT path may panic (architecture §8), but with correctly built sample data it is
//! unreachable.

use crate::sinc_table::{SINC_FRACTION_BITS, SINC_TABLE_Q15, SINC_TAPS};

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

    /// How many source frames *before* `index` this kernel reads.
    ///
    /// Zero for the two asymmetric kernels, one for [`Cubic`] and three for [`Sinc`]. The
    /// mixer answers it in two places: `starplayer_core::PRE_ROLL_FRAMES` frames of
    /// silence sit in front of every sample so a note's first frames have something to
    /// read, and a forward loop's wrap is deferred by this many frames so that the frames
    /// behind the interpolation point are the real ones the voice has just played rather
    /// than whatever precedes `loop_start`.
    const LEADING_FRAMES: usize;

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
    const LEADING_FRAMES: usize = 0;

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
    const LEADING_FRAMES: usize = 0;

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
        // Use the whole Q0.32 position fraction and round the 48-bit signed product to
        // nearest, ties away from zero. C6 makes this the canonical fixed-path rounding
        // rule: unlike an arithmetic shift it has no negative-slope DC bias, and the
        // widened intermediate is still comfortably inside `i64`.
        let interpolated_delta = round_shift_nearest((next - current) * fraction_bits as i64, 32);
        (current + interpolated_delta) as i32
    }
}

/// Cubic Hermite interpolation over `x[-1] .. x[2]`, in the Catmull-Rom form.
///
/// The quality real-time kernel of architecture §7.1: four taps, a continuous first
/// derivative, and an exact fit through `x[0]` and `x[1]`, which is what keeps a slow
/// sample sounding like itself rather than like a resampler. Corresponds to
/// [`starplayer_core::Interpolator::Cubic`].
///
/// # The polynomial, and its integer form
///
/// With `t` the fraction and the four frames named `before, current, next, after`, the
/// Catmull-Rom spline is
///
/// ```text
/// y = current + t/2 * (slope + t * (curve + t * jerk))
/// slope = next - before
/// curve = 2*before - 5*current + 4*next - after
/// jerk  = -before + 3*current - 3*next + after
/// ```
///
/// The fixed twin evaluates exactly that in `i64` with the fraction in **Q24** and one
/// rounding of the result. Two rescalings sit inside the Horner chain — they are what
/// keeps the cubed fraction inside `i64` at full scale — and each throws away less than
/// `2^-12` of a coefficient unit, which reaches the output as under `10^-4` of one `i16`
/// step. H5's research point 3 measured the alternatives: this form lands within
/// **0.51** of the exact rational value and of the `f32` twin, against 3.5 for a Q15
/// fraction and 6.5 for a Q14 one, because at eight taps of headroom the fraction's own
/// width dominates the rounding rule.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cubic;

/// The three Catmull-Rom difference terms, in whole `i16` units.
struct CubicTerms {
    current: i64,
    slope: i64,
    curve: i64,
    jerk: i64,
}

#[inline]
fn cubic_terms(frames: &[i16], index: usize) -> CubicTerms {
    let before = frame_at(frames, index.wrapping_sub(1)) as i64;
    let current = frame_at(frames, index) as i64;
    let next = frame_at(frames, index.wrapping_add(1)) as i64;
    let after = frame_at(frames, index.wrapping_add(2)) as i64;
    CubicTerms {
        current,
        slope: next - before,
        curve: 2 * before - 5 * current + 4 * next - after,
        jerk: -before + 3 * current - 3 * next + after,
    }
}

impl Interpolate for Cubic {
    const GUARD_FRAMES_REQUIRED: usize = 2;
    const LEADING_FRAMES: usize = 1;

    fn sample_f32(frames: &[i16], index: usize, fraction_bits: u32) -> f32 {
        let before = frame_at(frames, index.wrapping_sub(1)) as f32;
        let current = frame_at(frames, index) as f32;
        let next = frame_at(frames, index.wrapping_add(1)) as f32;
        let after = frame_at(frames, index.wrapping_add(2)) as f32;
        let slope = next - before;
        let curve = 2.0 * before - 5.0 * current + 4.0 * next - after;
        let jerk = -before + 3.0 * current - 3.0 * next + after;
        let fraction = fraction_bits as f32 * FRACTION_SCALE_F32;
        current + 0.5 * fraction * (slope + fraction * (curve + fraction * jerk))
    }

    fn sample_fixed(frames: &[i16], index: usize, fraction_bits: u32) -> i32 {
        let terms = cubic_terms(frames, index);
        let fraction = (fraction_bits >> 8) as i64;
        // Horner in Q24, rescaled to Q12 after each multiply so that the cubed fraction
        // cannot leave `i64`: the widest intermediate here is under 2^56 at full scale.
        let mut accumulator = terms.jerk * fraction;
        accumulator = round_shift_nearest(accumulator, 12) + (terms.curve << 12);
        accumulator *= fraction;
        accumulator = round_shift_nearest(accumulator, 24) + (terms.slope << 12);
        accumulator *= fraction;
        // Q12 x Q24 is Q36, and the halving the spline asks for makes it 37.
        (terms.current + round_shift_nearest(accumulator, 37)) as i32
    }
}

/// Eight-tap windowed-sinc interpolation over `x[-3] .. x[4]`, from a 256-phase table.
///
/// The offline-quality kernel of architecture §7.1, and the one the extra
/// `reflex__i16_mono_44100_sinc` golden pins across targets. Corresponds to
/// [`starplayer_core::Interpolator::Sinc`].
///
/// # The filter
///
/// A sinc with its cutoff at **0.9 of Nyquist**, windowed by a **Kaiser window with
/// β = 8**, sampled at 256 phases and stored in [`SINC_TABLE_Q15`] as Q1.15 `i16`
/// coefficients whose rows each sum to exactly `32768` — so DC gain is exactly unity on
/// the fixed path and no dot product can drift the signal level.
///
/// H5's research point 2 measured the alternatives on swept sines. At eight taps the
/// Kaiser window beats Blackman-Harris by a wide margin (worst image over the lower four
/// fifths of the band −22.6 dB against −17.0 dB, and −72 dB against −53 dB at 0.19 of the
/// source rate), because Blackman-Harris spends its sidelobe attenuation on a mainlobe
/// this short a filter cannot afford. Moving the cutoff to 0.95 buys about 0.9 dB less
/// droop at 0.4 of the source rate and costs 3 dB of image rejection everywhere and
/// 11 dB at 0.25, so 0.9 stays. The 256-phase quantisation sets the spur floor at about
/// −92 dB, measured while down-sampling by two.
///
/// The table is committed generated data, not a run-time computation: architecture §7.3
/// bans transcendental functions from the RT path, and a constant table is the same bytes
/// on x86, ARM and WASM. `sinc_table::tests` regenerates it in `f64` and asserts equality.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Sinc;

impl Interpolate for Sinc {
    const GUARD_FRAMES_REQUIRED: usize = 4;
    const LEADING_FRAMES: usize = 3;

    fn sample_f32(frames: &[i16], index: usize, fraction_bits: u32) -> f32 {
        let taps = sinc_taps(frames, index);
        let Some(coefficients) = SINC_TABLE_Q15.get(sinc_phase(fraction_bits)) else { return 0.0 };
        let mut accumulator = 0.0f32;
        for (frame, coefficient) in taps.iter().zip(coefficients.iter()) {
            accumulator += *frame as f32 * *coefficient as f32;
        }
        // `2^-15` is exact in `f32`, so the scaling is a rounding-free exponent change.
        accumulator * SINC_SCALE_F32
    }

    fn sample_fixed(frames: &[i16], index: usize, fraction_bits: u32) -> i32 {
        let taps = sinc_taps(frames, index);
        let Some(coefficients) = SINC_TABLE_Q15.get(sinc_phase(fraction_bits)) else { return 0 };
        let mut accumulator = 0i64;
        for (frame, coefficient) in taps.iter().zip(coefficients.iter()) {
            accumulator += *frame as i64 * *coefficient as i64;
        }
        round_shift_nearest(accumulator, SINC_FRACTION_BITS) as i32
    }
}

/// Which of the table's 256 phases the Q0.32 fraction selects. The low bits are dropped
/// rather than rounded, so the phase never wraps into the next source frame.
#[inline]
const fn sinc_phase(fraction_bits: u32) -> usize { (fraction_bits >> 24) as usize }

/// The eight source frames `x[-3] .. x[4]`, with out-of-range reads as silence.
///
/// The whole window is one bounds check with correctly built sample data; the per-tap
/// path exists only for the corrupt case the trait documents.
#[inline]
fn sinc_taps(frames: &[i16], index: usize) -> [i16; SINC_TAPS] {
    let base = index.wrapping_sub(<Sinc as Interpolate>::LEADING_FRAMES);
    let mut taps = [0i16; SINC_TAPS];
    match frames.get(base..base.wrapping_add(SINC_TAPS)) {
        Some(window) => {
            for (tap, frame) in taps.iter_mut().zip(window.iter()) {
                *tap = *frame;
            }
        }
        None => {
            for (offset, tap) in taps.iter_mut().enumerate() {
                *tap = frames.get(base.wrapping_add(offset)).copied().unwrap_or(0);
            }
        }
    }
    taps
}

/// `1.0 / 2^15`, exact in `f32`: the Q1.15 coefficient scale.
const SINC_SCALE_F32: f32 = 1.0 / 32_768.0;

/// Remove fractional bits with the canonical fixed-mixer rounding rule: nearest, with
/// exact half-way values rounded away from zero.
///
/// This is the single definition of that rule for the whole engine. `starplayer-mixer`
/// applies it to gains, to the master bus and to every host output conversion; it lives
/// here, in the lower crate, so the interpolator and the mixer cannot drift apart.
/// All callers use products small enough that adding the half-bit cannot overflow the
/// unsigned magnitude.
pub const fn round_shift_nearest(value: i64, fractional_bits: u32) -> i64 {
    if fractional_bits == 0 {
        return value;
    }
    if fractional_bits >= 64 {
        return 0;
    }
    let half = 1u64 << (fractional_bits - 1);
    let rounded = value.unsigned_abs().saturating_add(half) >> fractional_bits;
    if value < 0 { -(rounded as i64) } else { rounded as i64 }
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
    fn linear_fixed_rounds_to_nearest_without_a_negative_bias() {
        assert_eq!(Linear::sample_fixed(&RAMP, 1, 0x4000_0000), 500);
        assert_eq!(Linear::sample_fixed(&[0, -1], 0, 1), 0, "a tiny descending fraction rounds to zero, not minus one");
        assert_eq!(Linear::sample_fixed(&[0, 1], 0, 0x8000_0000), 1, "positive half-way values round away from zero");
        assert_eq!(Linear::sample_fixed(&[0, -1], 0, 0x8000_0000), -1, "negative half-way values round away from zero");
    }

    #[test]
    fn reads_past_the_end_yield_silence_rather_than_panicking() {
        assert_eq!(Nearest::sample_fixed(&RAMP, 99, 0), 0);
        assert_eq!(Linear::sample_fixed(&RAMP, 3, 0x8000_0000), 16383, "the guard frame reads as silence");
        assert_eq!(Linear::sample_f32(&[], 0, 0), 0.0);
    }

    // ── H5: cubic Hermite ───────────────────────────────────────────────────────────

    /// A ramp with a frame on either side, so index 2 has all four taps in range.
    const RAMP_WITH_ROOM: [i16; 6] = [-1000, 0, 1000, 2000, 3000, 4000];

    #[test]
    fn cubic_passes_exactly_through_the_frame_it_sits_on() {
        for (index, frame) in RAMP_WITH_ROOM.iter().enumerate().take(5).skip(1) {
            assert_eq!(Cubic::sample_fixed(&RAMP_WITH_ROOM, index, 0), *frame as i32, "frame {index}");
            assert_eq!(Cubic::sample_f32(&RAMP_WITH_ROOM, index, 0), *frame as f32, "frame {index}");
        }
    }

    /// Catmull-Rom reproduces a straight line exactly, which is the cheapest statement of
    /// "the spline is not adding curvature of its own".
    #[test]
    fn cubic_reproduces_a_straight_line() {
        assert_eq!(Cubic::sample_fixed(&RAMP_WITH_ROOM, 2, 0x8000_0000), 1500);
        assert_eq!(Cubic::sample_fixed(&RAMP_WITH_ROOM, 2, 0x4000_0000), 1250);
        assert_eq!(Cubic::sample_f32(&RAMP_WITH_ROOM, 2, 0x8000_0000), 1500.0);
    }

    /// Research point 3: the fixed twin's measured distance from the float one. The Q24
    /// fraction with two rescalings lands inside one `i16` step everywhere, against 3.5
    /// for a Q15 fraction and 6.5 for a Q14 one.
    #[test]
    fn cubic_fixed_and_float_agree_to_within_one_step() {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut worst = 0.0f32;
        for _ in 0..20_000 {
            let frames = [(next() >> 32) as i16, (next() >> 32) as i16, (next() >> 32) as i16, (next() >> 32) as i16];
            let fraction_bits = next() as u32;
            let difference = (Cubic::sample_fixed(&frames, 1, fraction_bits) as f32 - Cubic::sample_f32(&frames, 1, fraction_bits)).abs();
            if difference > worst { worst = difference; }
        }
        assert!(worst <= 1.0, "the fixed cubic drifted {worst} from the float one");
    }

    #[test]
    fn cubic_reads_outside_the_sample_as_silence_rather_than_panicking() {
        assert_eq!(Cubic::sample_fixed(&RAMP, 0, 0x8000_0000), Cubic::sample_fixed(&[0, 0, 1000, -1000, 32767], 1, 0x8000_0000), "index 0 reads x[-1] as silence");
        assert_eq!(Cubic::sample_fixed(&[], 0, 0x4000_0000), 0);
        assert_eq!(Cubic::sample_f32(&[], 9_999, 0x4000_0000), 0.0);
    }

    // ── H5: windowed sinc ───────────────────────────────────────────────────────────

    #[test]
    fn sinc_holds_a_constant_signal_exactly() {
        let flat = [4_321i16; 16];
        for phase in [0u32, 1, 0x4000_0000, 0x8000_0000, 0xFFFF_FFFF] {
            assert_eq!(Sinc::sample_fixed(&flat, 8, phase), 4_321, "the row sums to unity, so DC is unity");
        }
    }

    #[test]
    fn sinc_is_centred_on_the_frame_it_sits_on() {
        let mut impulse = [0i16; 16];
        impulse[8] = 30_000;
        let on_the_frame = Sinc::sample_fixed(&impulse, 8, 0);
        assert!(on_the_frame > 26_000, "phase 0 is dominated by x[0], not {on_the_frame}");
        assert!(Sinc::sample_fixed(&impulse, 4, 0).abs() < 100, "four frames away the impulse is all but gone");
    }

    #[test]
    fn sinc_fixed_and_float_agree_to_within_one_step() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut worst = 0.0f32;
        for _ in 0..20_000 {
            let mut frames = [0i16; 16];
            for frame in frames.iter_mut() {
                *frame = (next() >> 32) as i16;
            }
            let fraction_bits = next() as u32;
            let difference = (Sinc::sample_fixed(&frames, 8, fraction_bits) as f32 - Sinc::sample_f32(&frames, 8, fraction_bits)).abs();
            if difference > worst { worst = difference; }
        }
        assert!(worst <= 1.0, "the fixed sinc drifted {worst} from the float one");
    }

    #[test]
    fn sinc_reads_outside_the_sample_as_silence_rather_than_panicking() {
        assert_eq!(Sinc::sample_fixed(&[], 0, 0x8000_0000), 0);
        assert_eq!(Sinc::sample_f32(&[], 9_999, 0), 0.0);
        // A window that hangs off both ends still answers, using silence where there is
        // no data — the corrupt-module path the trait documents.
        assert_eq!(Sinc::sample_fixed(&[1_000], 0, 0), Sinc::sample_fixed(&[0, 0, 0, 1_000, 0, 0, 0, 0], 3, 0));
    }

    #[test]
    fn every_kernel_declares_what_the_sample_layout_has_to_carry() {
        assert_eq!((Nearest::GUARD_FRAMES_REQUIRED, Nearest::LEADING_FRAMES), (0, 0));
        assert_eq!((Linear::GUARD_FRAMES_REQUIRED, Linear::LEADING_FRAMES), (1, 0));
        assert_eq!((Cubic::GUARD_FRAMES_REQUIRED, Cubic::LEADING_FRAMES), (2, 1));
        assert_eq!((Sinc::GUARD_FRAMES_REQUIRED, Sinc::LEADING_FRAMES), (4, 3));
        // What those numbers have to fit inside is asserted at compile time, where the
        // sample layout is: see `starplayer_mixer::sample`'s `const _: ()` block.
    }
}
