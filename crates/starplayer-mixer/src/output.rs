//! [`OutputFormat`] — turning accumulator frames into whatever the host asked for.
//!
//! Architecture §7.1 asks for 8-, 16-, 24- and 32-bit signed integer output plus `f32`,
//! mono and stereo, from either mixing path. That is twenty combinations, which is twenty
//! hand-written conversion loops if the format is a type per combination — so it is not.
//! [`FloatOut`] and [`FixedOut`] are generic over a [`HostSample`] and a channel count, and
//! the aliases below name the four the engine uses today:
//!
//! | | `f32` | `i8` | `i16` | [`I24`] | `i32` |
//! |---|---|---|---|---|---|
//! | float path | [`StereoF32`], [`MonoF32`] | `FloatOut<i8, C>` | `FloatOut<i16, C>` | `FloatOut<I24, C>` | `FloatOut<i32, C>` |
//! | fixed path | `FixedOut<f32, C>` | `FixedOut<i8, C>` | [`StereoI16`], [`MonoI16`] | `FixedOut<I24, C>` | `FixedOut<i32, C>` |
//!
//! # 24-bit is 24-in-32, not three bytes (task B5 deliverable 5)
//!
//! [`I24`] is a sign-extended value in an `i32` container, `#[repr(transparent)]` so that
//! `&[I24]` is `&[i32]` in memory. Three reasons: the host APIs that take 24-bit PCM
//! mostly take 24-in-32 already (ALSA's `S24_LE`, WASAPI's 24-in-32); a packed layout
//! would force the output ring's element type to be `u8` and break the "one sample type
//! per format" shape the whole trait rests on; and packing is endian-sensitive, which is a
//! decision that belongs to the host rather than to the mixer. [`I24::to_le_bytes`] is
//! there for hosts that genuinely need three bytes, so the packing exists in exactly one
//! place.
//!
//! # Depth on the fixed path
//!
//! The fixed accumulator is on the `i16` scale by definition (architecture §7.1), so its
//! 24- and 32-bit output is an exact left shift and carries no resolution that was not
//! already there. That is honest rather than unfortunate: the fixed path is the *canonical
//! bit-exact reference*, and a host that wants real 24-bit depth wants the float path. It
//! is also why dither is a no-op there — see [`Dither`].
//!
//! # Conversion happens on whole quanta, before the ring
//!
//! Every function here converts a whole `RENDER_QUANTUM` at a time, and the engine's
//! output ring holds the *result*. That ordering is not incidental: dithering is stateful,
//! so if conversion ran after the ring it would see ragged host-sized segments and its
//! noise sequence would depend on the host's buffer size — reintroducing exactly the
//! failure this milestone's determinism test exists to prevent.

use core::marker::PhantomData;

use crate::path::{FixedFrame, FloatFrame};

/// A 24-bit sample, sign-extended into an `i32`.
///
/// Values outside `-8_388_608 ..= 8_388_607` never come out of the conversion; the type
/// does not enforce that on construction because it is a transport, not an invariant.
#[repr(transparent)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct I24(pub i32);

impl I24 {
    /// The largest 24-bit value.
    pub const MAX: I24 = I24(8_388_607);

    /// The smallest 24-bit value. Symmetric with [`I24::MAX`], because the conversion
    /// scales by `MAX` in both directions rather than using the extra negative code.
    pub const MIN: I24 = I24(-8_388_607);

    /// The three little-endian bytes a packed-24 host wants, low byte first.
    pub const fn to_le_bytes(self) -> [u8; 3] {
        let bits = self.0 as u32;
        [bits as u8, (bits >> 8) as u8, (bits >> 16) as u8]
    }
}

/// A deterministic TPDF dither generator (task B5 deliverable 5).
///
/// # Off by default, and seeded when on
///
/// Dither trades a quantisation *distortion* — which is correlated with the signal and so
/// audible as a gritty edge on a fade-out — for a quantisation *noise floor*, which is
/// not. It is worth it at 8 and 16 bits and pointless at 24 and 32, so this is an
/// explicit choice rather than a default.
///
/// When it is on it comes from a seeded xorshift, never from a system RNG: an offline
/// render has to be reproducible, and the goldens have to be able to fingerprint a
/// dithered render. The state lives in the caller — the engine — so that it advances once
/// per output frame across the whole render rather than restarting per block.
///
/// TPDF is two independent uniform values summed. That is the standard choice because it
/// decorrelates the quantisation error from the signal *and* keeps the noise floor's
/// modulation flat, which a single rectangular value does not.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Dither {
    state: u32,
    enabled: bool,
}

impl Default for Dither {
    fn default() -> Dither { Dither::OFF }
}

impl Dither {
    /// No dither. What [`OutputFormat::convert`] uses.
    pub const OFF: Dither = Dither { state: GOLDEN_RATIO_SEED, enabled: false };

    /// A dither generator seeded with `seed`. A zero seed would leave an xorshift stuck at
    /// zero for ever, so it is replaced.
    pub const fn seeded(seed: u32) -> Dither {
        Dither { state: if seed == 0 { GOLDEN_RATIO_SEED } else { seed }, enabled: true }
    }

    /// Whether this generator adds anything.
    pub const fn is_enabled(self) -> bool { self.enabled }

    /// xorshift32. Three shifts and three xors, a period of 2^32-1, and identical on every
    /// target because it is integer arithmetic — which is the whole requirement.
    const fn next_bits(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }

    /// A triangular value in `(-step, step)`, or zero when disabled.
    const fn tpdf(&mut self, step: i32) -> i32 {
        if !self.enabled {
            return 0;
        }
        let first = (self.next_bits() >> 16) as i32;
        let second = (self.next_bits() >> 16) as i32;
        (((first - second) as i64 * step as i64) >> 16) as i32
    }

    /// The same value on the float path's scale.
    fn tpdf_f32(&mut self, step: f32) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        let first = (self.next_bits() >> 16) as i32;
        let second = (self.next_bits() >> 16) as i32;
        (first - second) as f32 * (step * (1.0 / 65_536.0))
    }
}

/// `2^32 / phi`, the usual "arbitrary but well-distributed" constant.
const GOLDEN_RATIO_SEED: u32 = 0x9E37_79B9;

/// A sample type a host can be handed.
///
/// Both conversions clamp before they scale, so no input — however loud, and including a
/// NaN — can produce anything outside the type's range.
pub trait HostSample: Copy + Default {
    /// Bits of resolution, for documentation and for deciding whether dither is worth
    /// anything.
    const DEPTH_BITS: u32;

    /// From the float path's `±1.0`.
    fn from_unit_f32(value: f32, dither: &mut Dither) -> Self;

    /// From the fixed path's `i16` scale, with `i32` headroom.
    fn from_i16_scale(value: i32, dither: &mut Dither) -> Self;
}

impl HostSample for f32 {
    const DEPTH_BITS: u32 = 24;

    fn from_unit_f32(value: f32, _dither: &mut Dither) -> f32 { clamp_unit(value) }

    /// An exact scale of the integer result: one division, and no dither, because nothing
    /// is being quantised.
    fn from_i16_scale(value: i32, _dither: &mut Dither) -> f32 { clamp_i16(value as i64) as f32 * (1.0 / 32_767.0) }
}

impl HostSample for i8 {
    const DEPTH_BITS: u32 = 8;

    fn from_unit_f32(value: f32, dither: &mut Dither) -> i8 {
        round_unit_f32(value, 127.0, dither.tpdf_f32(1.0 / 127.0)) as i8
    }

    /// Eight bits are thrown away, so this is where dither earns its keep on the fixed
    /// path. Rounding is to nearest — a truncating shift would put a whole LSB of DC on
    /// the output at this depth.
    fn from_i16_scale(value: i32, dither: &mut Dither) -> i8 {
        let dithered = value as i64 + dither.tpdf(1 << 8) as i64;
        ((clamp_i16(dithered) as i32 + 128) >> 8).clamp(-128, 127) as i8
    }
}

impl HostSample for i16 {
    const DEPTH_BITS: u32 = 16;

    fn from_unit_f32(value: f32, dither: &mut Dither) -> i16 {
        round_unit_f32(value, 32_767.0, dither.tpdf_f32(1.0 / 32_767.0)) as i16
    }

    /// Exact: the accumulator is already on this scale, so there is nothing to dither and
    /// nothing to round. This is the canonical golden path.
    fn from_i16_scale(value: i32, _dither: &mut Dither) -> i16 { clamp_i16(value as i64) }
}

impl HostSample for I24 {
    const DEPTH_BITS: u32 = 24;

    fn from_unit_f32(value: f32, dither: &mut Dither) -> I24 {
        I24(round_unit_f32(value, 8_388_607.0, dither.tpdf_f32(1.0 / 8_388_607.0)))
    }

    /// An exact left shift — see the note on depth above.
    fn from_i16_scale(value: i32, _dither: &mut Dither) -> I24 { I24(clamp_i16(value as i64) as i32 * 256) }
}

impl HostSample for i32 {
    const DEPTH_BITS: u32 = 32;

    /// No dither: an `f32` accumulator carries 24 bits of mantissa, so the low eight bits
    /// of a 32-bit sample are already whatever the multiply left there. Dithering them
    /// would add noise to describe precision that does not exist.
    fn from_unit_f32(value: f32, _dither: &mut Dither) -> i32 {
        // `as i32` saturates in Rust, and the clamp has already bounded the value; the
        // multiply is by the largest power-of-two-minus-one an `f32` can hold exactly
        // below `i32::MAX`.
        (clamp_unit(value) * 2_147_483_520.0) as i32
    }

    /// An exact left shift — see the note on depth above.
    fn from_i16_scale(value: i32, _dither: &mut Dither) -> i32 { clamp_i16(value as i64) as i32 * 65_536 }
}

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

    /// Convert `source` frames into interleaved `destination` samples, advancing `dither`
    /// once per sample it is worth anything for.
    fn convert_dithered(dither: &mut Dither, source: &[Self::Accumulator], destination: &mut [Self::Sample]);

    /// Convert with dither off, which is the default (see [`Dither`]).
    fn convert(source: &[Self::Accumulator], destination: &mut [Self::Sample]) {
        let mut dither = Dither::OFF;
        Self::convert_dithered(&mut dither, source, destination);
    }
}

/// Output from the float accumulator, in `Sample`, with `CHANNELS` channels.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FloatOut<Sample, const CHANNELS: usize>(PhantomData<Sample>);

/// Output from the fixed accumulator, in `Sample`, with `CHANNELS` channels.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FixedOut<Sample, const CHANNELS: usize>(PhantomData<Sample>);

impl<Sample: HostSample> OutputFormat for FloatOut<Sample, 2> {
    type Accumulator = FloatFrame;
    type Sample = Sample;
    const CHANNELS: usize = 2;

    fn convert_dithered(dither: &mut Dither, source: &[FloatFrame], destination: &mut [Sample]) {
        for (frame, pair) in source.iter().zip(destination.chunks_exact_mut(2)) {
            if let [left, right] = pair {
                *left = Sample::from_unit_f32(frame.left, dither);
                *right = Sample::from_unit_f32(frame.right, dither);
            }
        }
    }
}

/// Mono on the float path: the two channels averaged, then converted.
impl<Sample: HostSample> OutputFormat for FloatOut<Sample, 1> {
    type Accumulator = FloatFrame;
    type Sample = Sample;
    const CHANNELS: usize = 1;

    fn convert_dithered(dither: &mut Dither, source: &[FloatFrame], destination: &mut [Sample]) {
        for (frame, sample) in source.iter().zip(destination.iter_mut()) {
            *sample = Sample::from_unit_f32((frame.left + frame.right) * 0.5, dither);
        }
    }
}

impl<Sample: HostSample> OutputFormat for FixedOut<Sample, 2> {
    type Accumulator = FixedFrame;
    type Sample = Sample;
    const CHANNELS: usize = 2;

    fn convert_dithered(dither: &mut Dither, source: &[FixedFrame], destination: &mut [Sample]) {
        for (frame, pair) in source.iter().zip(destination.chunks_exact_mut(2)) {
            if let [left, right] = pair {
                *left = Sample::from_i16_scale(frame.left, dither);
                *right = Sample::from_i16_scale(frame.right, dither);
            }
        }
    }
}

/// Mono on the fixed path: the two channels summed and halved with an arithmetic shift.
///
/// The shift floors rather than rounding to nearest — one LSB of DC on negative signal —
/// which is what every integer tracker mixer has always done, including the original's
/// `Mixer_8bitMono`. Stated rather than accidental, because the goldens encode it.
impl<Sample: HostSample> OutputFormat for FixedOut<Sample, 1> {
    type Accumulator = FixedFrame;
    type Sample = Sample;
    const CHANNELS: usize = 1;

    fn convert_dithered(dither: &mut Dither, source: &[FixedFrame], destination: &mut [Sample]) {
        for (frame, sample) in source.iter().zip(destination.iter_mut()) {
            let summed = (frame.left as i64 + frame.right as i64) >> 1;
            *sample = Sample::from_i16_scale(clamp_i32(summed), dither);
        }
    }
}

/// Interleaved stereo `f32`, clamped to ±1.0. The default for desktop and browser hosts.
pub type StereoF32 = FloatOut<f32, 2>;

/// Mono `f32`.
pub type MonoF32 = FloatOut<f32, 1>;

/// Interleaved stereo `i16` from the fixed path. **Integer arithmetic only** — this is the
/// canonical bit-exact path (architecture §7.3), so nothing here goes through `f32`.
pub type StereoI16 = FixedOut<i16, 2>;

/// Mono `i16` from the fixed path.
pub type MonoI16 = FixedOut<i16, 1>;

/// Clamp to ±1.0. A NaN accumulator — which nothing in the mixer can currently produce —
/// converts to silence rather than propagating into the host's buffer.
fn clamp_unit(value: f32) -> f32 {
    if value.is_nan() { 0.0 } else { value.clamp(-1.0, 1.0) }
}

/// Clamp an accumulator value into `i16` range. Saturation, never wrap-around: a loud
/// module should sound clipped, not inverted.
fn clamp_i16(value: i64) -> i16 { value.clamp(i16::MIN as i64, i16::MAX as i64) as i16 }

/// Clamp a widened accumulator sum back into the accumulator's own type.
fn clamp_i32(value: i64) -> i32 { value.clamp(i32::MIN as i64, i32::MAX as i64) as i32 }

/// Scale a unit float to `full_scale`, with `noise` added before rounding, rounding half
/// away from zero.
///
/// `f32::round` is not available in `core`, and calling out to a libm would break
/// cross-target determinism outright (architecture §7.3) — so the rounding is written out.
/// `as i32` saturates in Rust, which makes the clamp belt-and-braces rather than the only
/// guard.
fn round_unit_f32(value: f32, full_scale: f32, noise: f32) -> i32 {
    let scaled = clamp_unit(clamp_unit(value) + noise) * full_scale;
    if scaled >= 0.0 { (scaled + 0.5) as i32 } else { (scaled - 0.5) as i32 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
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

        let mut destination = [0i16; 2];
        FloatOut::<i16, 2>::convert(&source, &mut destination);
        assert_eq!(destination, [0, 16_384], "and it converts to integer silence too");
    }

    /// Task B5's verification: `f32` to `i16` and back stays within one LSB.
    #[test]
    fn the_i16_round_trip_stays_within_one_lsb() {
        let source: Vec<FloatFrame> = (-2_000..=2_000)
            .map(|step| {
                let value = step as f32 / 2_000.0;
                Stereo::new(value, -value)
            })
            .collect();
        let mut converted = alloc::vec![0i16; source.len() * 2];
        FloatOut::<i16, 2>::convert(&source, &mut converted);

        for (frame, pair) in source.iter().zip(converted.chunks_exact(2)) {
            if let [left, right] = pair {
                let back = *left as f32 / 32_767.0;
                assert!((back - frame.left).abs() <= 1.0 / 32_767.0, "{} came back as {back}", frame.left);
                let back = *right as f32 / 32_767.0;
                assert!((back - frame.right).abs() <= 1.0 / 32_767.0, "{} came back as {back}", frame.right);
            }
        }
    }

    #[test]
    fn every_depth_reaches_its_own_full_scale_and_no_further() {
        let source = [Stereo::new(1.0, -1.0), Stereo::new(4.0, -4.0)];

        let mut eight = [0i8; 4];
        FloatOut::<i8, 2>::convert(&source, &mut eight);
        assert_eq!(eight, [127, -127, 127, -127]);

        let mut sixteen = [0i16; 4];
        FloatOut::<i16, 2>::convert(&source, &mut sixteen);
        assert_eq!(sixteen, [32_767, -32_767, 32_767, -32_767]);

        let mut twenty_four = [I24::default(); 4];
        FloatOut::<I24, 2>::convert(&source, &mut twenty_four);
        assert_eq!(twenty_four, [I24::MAX, I24::MIN, I24::MAX, I24::MIN]);

        let mut thirty_two = [0i32; 4];
        FloatOut::<i32, 2>::convert(&source, &mut thirty_two);
        assert!(thirty_two.iter().all(|sample| sample.abs() > 2_147_000_000), "got {thirty_two:?}");
    }

    #[test]
    fn the_fixed_path_widens_by_an_exact_shift() {
        let source = [Stereo::new(1_234, -1_234)];

        let mut twenty_four = [I24::default(); 2];
        FixedOut::<I24, 2>::convert(&source, &mut twenty_four);
        assert_eq!(twenty_four, [I24(1_234 * 256), I24(-1_234 * 256)]);

        let mut thirty_two = [0i32; 2];
        FixedOut::<i32, 2>::convert(&source, &mut thirty_two);
        assert_eq!(thirty_two, [1_234 * 65_536, -1_234 * 65_536]);

        let mut eight = [0i8; 2];
        FixedOut::<i8, 2>::convert(&source, &mut eight);
        assert_eq!(eight, [5, -5], "1234/256 rounds to nearest, not toward zero");
    }

    #[test]
    fn twenty_four_bit_packs_little_endian() {
        assert_eq!(I24(0x123456).to_le_bytes(), [0x56, 0x34, 0x12]);
        assert_eq!(I24(-1).to_le_bytes(), [0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn dither_is_off_by_default_and_deterministic_when_on() {
        let source = [Stereo::new(0.001f32, -0.001); 64];

        let mut undithered = [0i8; 128];
        FloatOut::<i8, 2>::convert(&source, &mut undithered);
        let mut again = [0i8; 128];
        FloatOut::<i8, 2>::convert(&source, &mut again);
        assert_eq!(undithered, again, "the default conversion adds nothing");
        assert!(undithered.iter().all(|sample| *sample == 0), "and quantises a tiny signal to nothing");

        let mut dithered = [0i8; 128];
        FloatOut::<i8, 2>::convert_dithered(&mut Dither::seeded(1), &source, &mut dithered);
        let mut repeated = [0i8; 128];
        FloatOut::<i8, 2>::convert_dithered(&mut Dither::seeded(1), &source, &mut repeated);
        assert_eq!(dithered, repeated, "the same seed gives the same noise, every time");
        assert!(dithered.iter().any(|sample| *sample != 0), "and the signal survives quantisation");

        let mut other_seed = [0i8; 128];
        FloatOut::<i8, 2>::convert_dithered(&mut Dither::seeded(99), &source, &mut other_seed);
        assert_ne!(dithered, other_seed);
    }

    /// Dither state carries across calls, which is what makes a whole render's noise
    /// sequence independent of how it was split into quanta.
    #[test]
    fn dither_state_carries_across_calls() {
        let source = [Stereo::new(0.001f32, -0.001); 8];
        let mut whole = [0i8; 16];
        FloatOut::<i8, 2>::convert_dithered(&mut Dither::seeded(7), &source, &mut whole);

        let mut split = [0i8; 16];
        let mut dither = Dither::seeded(7);
        if let (Some(first), Some(second)) = (source.get(..3), source.get(3..)) {
            if let Some(head) = split.get_mut(..6) {
                FloatOut::<i8, 2>::convert_dithered(&mut dither, first, head);
            }
            if let Some(tail) = split.get_mut(6..) {
                FloatOut::<i8, 2>::convert_dithered(&mut dither, second, tail);
            }
        }
        assert_eq!(split, whole);
    }

    #[test]
    fn dither_never_pushes_a_full_scale_signal_out_of_range() {
        let source = [Stereo::new(1.0f32, -1.0); 32];
        let mut destination = [0i16; 64];
        FloatOut::<i16, 2>::convert_dithered(&mut Dither::seeded(3), &source, &mut destination);
        for pair in destination.chunks_exact(2) {
            if let [left, right] = pair {
                assert!(*left >= 32_760, "a full-scale sample came back as {left}");
                assert!(*right <= -32_760, "a full-scale sample came back as {right}");
            }
        }
    }
}
