//! [`OutputFormat`] — turning accumulator frames into whatever the host asked for.
//!
//! M0 ships the four combinations the two mixing paths and the two channel counts give:
//! [`StereoF32`], [`MonoF32`], [`StereoI16`] and [`MonoI16`]. The remaining depths
//! (8/24/32-bit integer) and dithering land in M1 — architecture §7.1 wants all of them,
//! but each needs a stated rounding rule and, for the dithered cases, a noise source
//! whose state has to be part of the golden fingerprint, and none of that belongs in the
//! commit that establishes the determinism invariant.
//!
//! # Conversion happens on whole quanta, before the ring
//!
//! Every function here converts a whole `RENDER_QUANTUM` at a time, and the engine's
//! output ring holds the *result*. That ordering is not incidental: dithering (M1) is
//! stateful, so if conversion ran after the ring it would see ragged host-sized segments
//! and its noise sequence would depend on the host's buffer size — reintroducing exactly
//! the failure this milestone's determinism test exists to prevent.

use crate::path::{FixedFrame, FloatFrame};

/// How accumulator frames become host samples.
///
/// `convert` writes `CHANNELS` samples per source frame, interleaved. It writes as many
/// frames as both slices have room for and ignores any excess, so a mismatched call
/// produces short output rather than a panic.
pub trait OutputFormat {
    /// The accumulator frame this format consumes — ties the format to a
    /// [`MixPath`](crate::path::MixPath).
    type Accumulator: Copy + Default;

    /// The host's sample type.
    type Sample: Copy + Default;

    /// Samples written per source frame.
    const CHANNELS: usize;

    /// Convert `source` frames into interleaved `destination` samples.
    fn convert(source: &[Self::Accumulator], destination: &mut [Self::Sample]);
}

/// Interleaved stereo `f32`, clamped to ±1.0. The default for desktop and browser hosts.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct StereoF32;

impl OutputFormat for StereoF32 {
    type Accumulator = FloatFrame;
    type Sample = f32;
    const CHANNELS: usize = 2;

    fn convert(source: &[FloatFrame], destination: &mut [f32]) {
        for (frame, pair) in source.iter().zip(destination.chunks_exact_mut(2)) {
            if let [left, right] = pair {
                *left = clamp_unit(frame.left);
                *right = clamp_unit(frame.right);
            }
        }
    }
}

/// Mono `f32`: the two channels averaged, then clamped to ±1.0.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MonoF32;

impl OutputFormat for MonoF32 {
    type Accumulator = FloatFrame;
    type Sample = f32;
    const CHANNELS: usize = 1;

    fn convert(source: &[FloatFrame], destination: &mut [f32]) {
        for (frame, sample) in source.iter().zip(destination.iter_mut()) {
            *sample = clamp_unit((frame.left + frame.right) * 0.5);
        }
    }
}

/// Interleaved stereo `i16`, clamped. **Integer arithmetic only** — this is the canonical
/// bit-exact path (architecture §7.3), so nothing here may go through `f32`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct StereoI16;

impl OutputFormat for StereoI16 {
    type Accumulator = FixedFrame;
    type Sample = i16;
    const CHANNELS: usize = 2;

    fn convert(source: &[FixedFrame], destination: &mut [i16]) {
        for (frame, pair) in source.iter().zip(destination.chunks_exact_mut(2)) {
            if let [left, right] = pair {
                *left = clamp_to_i16(frame.left as i64);
                *right = clamp_to_i16(frame.right as i64);
            }
        }
    }
}

/// Mono `i16`: the two channels summed and halved with an arithmetic shift, then clamped.
///
/// The shift floors rather than rounding to nearest — one LSB of DC on negative signal —
/// which is what every integer tracker mixer has always done, including the original's
/// `Mixer_8bitMono`. Stated rather than accidental, because the goldens encode it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MonoI16;

impl OutputFormat for MonoI16 {
    type Accumulator = FixedFrame;
    type Sample = i16;
    const CHANNELS: usize = 1;

    fn convert(source: &[FixedFrame], destination: &mut [i16]) {
        for (frame, sample) in source.iter().zip(destination.iter_mut()) {
            *sample = clamp_to_i16((frame.left as i64 + frame.right as i64) >> 1);
        }
    }
}

/// Clamp to ±1.0. A NaN accumulator — which nothing in the mixer can currently produce —
/// converts to silence rather than propagating into the host's buffer.
fn clamp_unit(value: f32) -> f32 {
    if value.is_nan() { 0.0 } else { value.clamp(-1.0, 1.0) }
}

/// Clamp an accumulator value into `i16` range. Saturation, never wrap-around: a loud
/// module should sound clipped, not inverted.
fn clamp_to_i16(value: i64) -> i16 { value.clamp(i16::MIN as i64, i16::MAX as i64) as i16 }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::Stereo;

    #[test]
    fn stereo_f32_interleaves_and_clamps() {
        let source = [Stereo::new(0.5, -0.25), Stereo::new(2.0, -3.0)];
        let mut destination = [0.0f32; 4];
        StereoF32::convert(&source, &mut destination);
        assert_eq!(destination, [0.5, -0.25, 1.0, -1.0]);
    }

    #[test]
    fn mono_f32_averages_the_channels() {
        let source = [Stereo::new(0.5, -0.5), Stereo::new(1.0, 1.0)];
        let mut destination = [0.0f32; 2];
        MonoF32::convert(&source, &mut destination);
        assert_eq!(destination, [0.0, 1.0]);
    }

    #[test]
    fn stereo_i16_saturates_rather_than_wrapping() {
        let source = [Stereo::new(1_000, -1_000), Stereo::new(i32::MAX, i32::MIN)];
        let mut destination = [0i16; 4];
        StereoI16::convert(&source, &mut destination);
        assert_eq!(destination, [1_000, -1_000, i16::MAX, i16::MIN]);
    }

    #[test]
    fn mono_i16_halves_with_an_arithmetic_shift() {
        let source = [Stereo::new(1_000, 2_000), Stereo::new(-1, 0), Stereo::new(i32::MAX, i32::MAX)];
        let mut destination = [0i16; 3];
        MonoI16::convert(&source, &mut destination);
        assert_eq!(destination, [1_500, -1, i16::MAX], "the shift floors, so -1/2 is -1, not 0");
    }

    #[test]
    fn a_short_destination_converts_what_fits_rather_than_panicking() {
        let source = [Stereo::new(1_000, 1_000); 4];
        let mut destination = [0i16; 3];
        StereoI16::convert(&source, &mut destination);
        assert_eq!(destination, [1_000, 1_000, 0], "the odd trailing sample is left alone");
    }

    #[test]
    fn a_nan_accumulator_becomes_silence() {
        let source = [Stereo::new(f32::NAN, 0.5)];
        let mut destination = [0.0f32; 2];
        StereoF32::convert(&source, &mut destination);
        assert_eq!(destination, [0.0, 0.5]);
    }
}
