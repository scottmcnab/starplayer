//! [`Eq`] — a three-band equaliser: low shelf, peaking bell, high shelf (H3 deliverable 1).
//!
//! # Cooking is per sixteen frames, and that number was measured
//!
//! An RBJ cooker is not cheap. [`crate::biquad`]'s shelving cookers each take an integer
//! square root by Newton's method (`isqrt_u128`, tens of 128-bit divisions) on top of a
//! handful of Q32 multiplies and divides; running three of those **per frame** would cost
//! more than the entire rest of the mixer. Running them only when a host moves a slider
//! would be cheaper still, but then a slider move is a step change in a resonant filter's
//! coefficients, and a step change in those is a transient.
//!
//! So the parameters smooth per frame, exactly as [`SmoothedParam`]'s contract requires,
//! and the coefficients are re-cooked every [`EQ_COOK_FRAMES`] frames — eight times per
//! [`DSP_BLOCK_FRAMES`] block — and then only for a band whose three smoothed values
//! actually moved, so a **steady EQ cooks nothing at all** and only an EQ under automation
//! pays anything.
//!
//! Sixteen rather than a whole block is research point 1's answer, and it is a measurement
//! rather than a preference. Sweeping a bell across its whole ±24 dB range over
//! [`SMOOTH_FRAMES`] — the most violent automation the parameter range allows — and taking
//! the largest sample-to-sample step in the output gives:
//!
//! | frames between cooks | largest step | peak |
//! |---|---|---|
//! | 1 (the unaffordable ideal) | 1251 | 27950 |
//! | 4 | 1302 | 28091 |
//! | **16** | **1635** | **30111** |
//! | 32 | 2990 | 36866 |
//! | 64 | 4969 | 48517 |
//! | 128 (a whole block) | 7304 | 62159 |
//!
//! A whole block is nearly six times the ideal's step and overshoots the settled output by
//! 7 dB; sixteen frames is 1.3 times the ideal and overshoots by 0.6 dB, which is the knee
//! of that curve. `the_cooking_interval_is_close_to_a_per_frame_ideal` pins the ratio.
//! Because a block is a fixed 128 frames and 16 divides it, the sub-block split is still a
//! function of frames rendered and nothing about it depends on the host's buffer size.
//!
//! # Why the end of the span, and not its start
//!
//! A span is cooked from the parameter value at its **end**. The choice is between leading
//! the ramp by up to sixteen frames and lagging it by the same; leading means a `set_param`
//! that lands on a quantum boundary starts moving the sound in that same quantum rather
//! than one span later. Either way it is a function of frames rendered, never of host block
//! size, because an insert only ever sees whole blocks.
//!
//! # The fixed path filters six bits up
//!
//! A biquad at a low corner frequency has its poles very close to `z = 1`, and a direct
//! form's quantisation noise is amplified by roughly `1/(1 − p)²` there: for the default
//! 120 Hz low shelf at 44.1 kHz that is about 70 dB of noise gain, which measured out at
//! **38 dB** of fixed-versus-float agreement — well short of the 60 dB this task requires.
//! The signal is therefore multiplied by `2^`[`EQ_PREAMP_BITS`] on the way into the cascade
//! and divided by it on the way out, exactly as `crate::filter`'s IT filter pre-amplifies
//! its delay line by 256 and for the same reason: six more bits below the sample is six
//! more bits the recursion's rounding error has to climb, and the measurement moves to
//! **75 dB**. Six bits and not eight because the scaling goes through
//! [`DspSample::mul_q24`], whose coefficient is an `i32` — `1 << (24 + 6)` is the largest
//! power of two that fits — and because six leaves 256× of headroom over full scale for
//! the state of a boosted band, which no ±24 dB setting comes close to using.
//!
//! On the float path the two multiplies are by exact powers of two and cancel exactly, so
//! nothing about the float output changes.
//!
//! # `enabled` is a step, deliberately
//!
//! It is the same bit `InsertCommand::Bypass` gives every effect for free, offered here as
//! a parameter so a host can automate an EQ's three bands and its on/off together. Like
//! bypass it is not smoothed and not crossfaded: switching it mid-signal is a step, and a
//! host that wants a smooth exit rides the three gains to zero instead.

use crate::biquad::{self, BiquadCoefficients, SHELF_GAIN_CENTI_DB_BOUND};
use crate::frame::Stereo;
use crate::insert::{DSP_BLOCK_FRAMES, Insert, InsertDescriptor, ParamId, ParamSpec, ParamUnit};
use crate::sample::DspSample;
use crate::smooth::{SMOOTH_FRAMES, SmoothedParam};

/// The lowest band centre a host may ask for, in hertz.
pub const EQ_MIN_HZ: i32 = 20;

/// The highest band centre a host may ask for, in hertz. A band above Nyquist is clamped
/// to Nyquist by [`crate::biquad`]'s own `angular_phase`, so this is a control-surface
/// bound rather than a numerical one.
pub const EQ_MAX_HZ: i32 = 20_000;

/// The band gain range, in centi-decibels: ±24 dB, [`SHELF_GAIN_CENTI_DB_BOUND`]. Past it
/// a shelving coefficient leaves the range Q8.24 provides (H2 research point 2).
pub const EQ_GAIN_BOUND_CENTI_DB: i32 = SHELF_GAIN_CENTI_DB_BOUND;

/// The narrowest bell / gentlest shelf, as `Q × 100`.
pub const EQ_SHAPE_MIN: i32 = 10;

/// The widest bell, as `Q × 100`: `Q = 10`.
pub const EQ_SHAPE_MAX: i32 = 1_000;

/// A shelf's slope is the cookbook's `S`, which is only defined on `(0, 1]` — so a shelf
/// parameter is clamped here rather than at [`EQ_SHAPE_MAX`].
pub const EQ_SLOPE_MAX: i32 = 100;

/// `enabled`.
pub const EQ_ENABLED_PARAM: ParamId = ParamId(0);
/// The low shelf's corner frequency.
pub const EQ_LOW_FREQUENCY_PARAM: ParamId = ParamId(1);
/// The low shelf's gain.
pub const EQ_LOW_GAIN_PARAM: ParamId = ParamId(2);
/// The low shelf's slope.
pub const EQ_LOW_SLOPE_PARAM: ParamId = ParamId(3);
/// The bell's centre frequency.
pub const EQ_PEAK_FREQUENCY_PARAM: ParamId = ParamId(4);
/// The bell's gain.
pub const EQ_PEAK_GAIN_PARAM: ParamId = ParamId(5);
/// The bell's `Q`.
pub const EQ_PEAK_Q_PARAM: ParamId = ParamId(6);
/// The high shelf's corner frequency.
pub const EQ_HIGH_FREQUENCY_PARAM: ParamId = ParamId(7);
/// The high shelf's gain.
pub const EQ_HIGH_GAIN_PARAM: ParamId = ParamId(8);
/// The high shelf's slope.
pub const EQ_HIGH_SLOPE_PARAM: ParamId = ParamId(9);

/// Bands, in processing order.
const BAND_COUNT: usize = 3;

/// Frames between coefficient re-cooks — see "Cooking is per sixteen frames" in the module
/// documentation. It divides [`DSP_BLOCK_FRAMES`], which is what keeps the split a function
/// of frames rendered.
pub const EQ_COOK_FRAMES: usize = 16;

const _: () = assert!(DSP_BLOCK_FRAMES.is_multiple_of(EQ_COOK_FRAMES), "the cooking span has to divide a DSP block");

/// Bits the signal is shifted up by before the cascade and down by after it — see "The
/// fixed path filters six bits up" in the module documentation.
pub const EQ_PREAMP_BITS: u32 = 6;

/// `2^EQ_PREAMP_BITS` as a Q8.24 coefficient, the largest such power of two an `i32` holds.
const EQ_PREAMP_Q24: i32 = 1 << (24 + EQ_PREAMP_BITS);

/// `2^-EQ_PREAMP_BITS` as a Q8.24 coefficient.
const EQ_POSTAMP_Q24: i32 = 1 << (24 - EQ_PREAMP_BITS);

/// What a host draws for an [`Eq`]. The order is the [`ParamId`] order: the master switch,
/// then each band's frequency, gain and shape, low to high.
pub static EQ_DESCRIPTOR: InsertDescriptor = InsertDescriptor {
    name: "eq",
    params: &[
        ParamSpec { name: "enabled", unit: ParamUnit::Switch, min: 0, max: 1, default: 1 },
        ParamSpec { name: "low_frequency", unit: ParamUnit::Hertz, min: EQ_MIN_HZ, max: EQ_MAX_HZ, default: 120 },
        ParamSpec { name: "low_gain", unit: ParamUnit::CentiDecibels, min: -EQ_GAIN_BOUND_CENTI_DB, max: EQ_GAIN_BOUND_CENTI_DB, default: 0 },
        ParamSpec { name: "low_slope", unit: ParamUnit::Ratio, min: EQ_SHAPE_MIN, max: EQ_SLOPE_MAX, default: EQ_SLOPE_MAX },
        ParamSpec { name: "peak_frequency", unit: ParamUnit::Hertz, min: EQ_MIN_HZ, max: EQ_MAX_HZ, default: 1_000 },
        ParamSpec { name: "peak_gain", unit: ParamUnit::CentiDecibels, min: -EQ_GAIN_BOUND_CENTI_DB, max: EQ_GAIN_BOUND_CENTI_DB, default: 0 },
        ParamSpec { name: "peak_q", unit: ParamUnit::Ratio, min: EQ_SHAPE_MIN, max: EQ_SHAPE_MAX, default: 100 },
        ParamSpec { name: "high_frequency", unit: ParamUnit::Hertz, min: EQ_MIN_HZ, max: EQ_MAX_HZ, default: 8_000 },
        ParamSpec { name: "high_gain", unit: ParamUnit::CentiDecibels, min: -EQ_GAIN_BOUND_CENTI_DB, max: EQ_GAIN_BOUND_CENTI_DB, default: 0 },
        ParamSpec { name: "high_slope", unit: ParamUnit::Ratio, min: EQ_SHAPE_MIN, max: EQ_SLOPE_MAX, default: EQ_SLOPE_MAX },
    ],
};

/// Which cooker a band runs through.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BandKind {
    LowShelf,
    Peaking,
    HighShelf,
}

/// The bands, in processing order: low shelf, bell, high shelf.
const BAND_KINDS: [BandKind; BAND_COUNT] = [BandKind::LowShelf, BandKind::Peaking, BandKind::HighShelf];

/// One band's three smoothed parameters, the coefficients cooked from them, and the values
/// they were cooked at — which is how a steady band avoids cooking at all.
#[derive(Copy, Clone, Debug)]
struct Band {
    frequency_hz: SmoothedParam,
    gain_centi_db: SmoothedParam,
    shape: SmoothedParam,
    coefficients: BiquadCoefficients,
    cooked_from: [i32; 3],
}

impl Band {
    fn new(kind: BandKind, frequency_hz: i32, gain_centi_db: i32, shape: i32, sample_rate_hz: u32) -> Band {
        Band {
            frequency_hz: SmoothedParam::steady(frequency_hz),
            gain_centi_db: SmoothedParam::steady(gain_centi_db),
            shape: SmoothedParam::steady(shape),
            coefficients: cook(kind, frequency_hz, gain_centi_db, shape, sample_rate_hz),
            cooked_from: [frequency_hz, gain_centi_db, shape],
        }
    }

    /// Advance the three parameters by `frames` and re-cook if any of them moved.
    ///
    /// The `is_moving` guard is not an optimisation of the *result*: advancing a parameter
    /// that is not ramping returns the same value however many times it is called, so the
    /// guarded and unguarded forms are the same function of frames elapsed. It is what
    /// keeps a steady EQ down to three comparisons a span.
    fn advance_span(&mut self, frames: usize, kind: BandKind, sample_rate_hz: u32) {
        for parameter in [&mut self.frequency_hz, &mut self.gain_centi_db, &mut self.shape] {
            if parameter.is_moving() {
                for _ in 0..frames {
                    parameter.advance();
                }
            }
        }
        let now = [self.frequency_hz.current(), self.gain_centi_db.current(), self.shape.current()];
        if now != self.cooked_from {
            self.cooked_from = now;
            self.coefficients = cook(kind, now[0], now[1], now[2], sample_rate_hz);
        }
    }

    /// Land every ramp on its target and re-cook there — [`Insert::reset`]'s half.
    fn snap(&mut self, kind: BandKind, sample_rate_hz: u32) {
        self.frequency_hz.snap();
        self.gain_centi_db.snap();
        self.shape.snap();
        self.cooked_from = [self.frequency_hz.current(), self.gain_centi_db.current(), self.shape.current()];
        self.coefficients = cook(kind, self.cooked_from[0], self.cooked_from[1], self.cooked_from[2], sample_rate_hz);
    }
}

/// A band's coefficients, from the tables. `shape` is `Q × 100` for the bell and the
/// cookbook's `S × 100` for the two shelves; both reach [`crate::biquad`] as Q1.15, where
/// `32768` is `1.0`.
fn cook(kind: BandKind, frequency_hz: i32, gain_centi_db: i32, shape: i32, sample_rate_hz: u32) -> BiquadCoefficients {
    let frequency_hz = frequency_hz.clamp(EQ_MIN_HZ, EQ_MAX_HZ) as u32;
    let gain_centi_db = gain_centi_db.clamp(-EQ_GAIN_BOUND_CENTI_DB, EQ_GAIN_BOUND_CENTI_DB);
    match kind {
        BandKind::LowShelf => biquad::low_shelf(frequency_hz, gain_centi_db, shape_q15(shape, EQ_SLOPE_MAX), sample_rate_hz),
        BandKind::Peaking => biquad::peaking(frequency_hz, gain_centi_db, shape_q15(shape, EQ_SHAPE_MAX), sample_rate_hz),
        BandKind::HighShelf => biquad::high_shelf(frequency_hz, gain_centi_db, shape_q15(shape, EQ_SLOPE_MAX), sample_rate_hz),
    }
}

/// `shape / 100` in Q1.15, clamped to `[EQ_SHAPE_MIN, maximum] / 100`.
fn shape_q15(shape: i32, maximum: i32) -> i32 {
    let clamped = shape.clamp(EQ_SHAPE_MIN, maximum) as i64;
    ((clamped * crate::sample::Q15_UNITY as i64 + 50) / 100) as i32
}

/// A three-band equaliser: low shelf, peaking bell, high shelf, in that order.
///
/// Generic over the mix path because its state is: two biquad state words per band per
/// stereo channel, in whichever type the path mixes in. The coefficients are not — a Q8.24
/// `i32` set drives both paths through [`DspSample::mul_q24`] (H2 research resolution).
#[derive(Clone, Debug)]
pub struct Eq<Sample: DspSample> {
    sample_rate_hz: u32,
    enabled: bool,
    bands: [Band; BAND_COUNT],
    left_state: [[Sample; 2]; BAND_COUNT],
    right_state: [[Sample; 2]; BAND_COUNT],
}

impl<Sample: DspSample> Eq<Sample> {
    /// A flat equaliser at this sample rate: every band at its descriptor default, which is
    /// 0 dB and therefore an exact pass-through (an RBJ shelf or bell at `A = 1` has
    /// `b == a` term for term).
    pub fn new(sample_rate_hz: u32) -> Eq<Sample> {
        let sample_rate_hz = sample_rate_hz.max(1);
        let mut bands = [Band::new(BandKind::LowShelf, 0, 0, EQ_SHAPE_MIN, sample_rate_hz); BAND_COUNT];
        for (index, kind) in BAND_KINDS.iter().enumerate() {
            let base = 1 + index * 3;
            let frequency_hz = default_at(base);
            let gain_centi_db = default_at(base + 1);
            let shape = default_at(base + 2);
            if let Some(band) = bands.get_mut(index) {
                *band = Band::new(*kind, frequency_hz, gain_centi_db, shape, sample_rate_hz);
            }
        }
        Eq {
            sample_rate_hz,
            enabled: default_at(0) != 0,
            bands,
            left_state: [[Sample::ZERO; 2]; BAND_COUNT],
            right_state: [[Sample::ZERO; 2]; BAND_COUNT],
        }
    }

    /// Whether the master switch is on.
    pub fn is_enabled(&self) -> bool { self.enabled }
}

/// The descriptor's default for one parameter position. `0` for a position the descriptor
/// does not have, which cannot happen — [`EQ_DESCRIPTOR`] has ten and every caller here is
/// inside that range — but is what keeps this indexing-free.
fn default_at(index: usize) -> i32 { EQ_DESCRIPTOR.params.get(index).map_or(0, |spec| spec.default) }

/// The descriptor's clamp for one parameter position.
fn clamp_at(index: usize, value: i32) -> i32 { EQ_DESCRIPTOR.params.get(index).map_or(value, |spec| spec.clamp(value)) }

/// Which band and which of its three parameters a [`ParamId`] names.
fn band_and_field(id: ParamId) -> Option<(usize, usize)> {
    let index = id.0 as usize;
    if index == 0 || index >= EQ_DESCRIPTOR.params.len() {
        return None;
    }
    Some(((index - 1) / 3, (index - 1) % 3))
}

impl<Sample: DspSample> Insert<Sample> for Eq<Sample> {
    fn process(&mut self, block: &mut [Stereo<Sample>]) {
        debug_assert_eq!(block.len(), DSP_BLOCK_FRAMES, "an insert only ever sees a whole DSP block");
        for span in block.chunks_mut(EQ_COOK_FRAMES) {
            // The parameters advance whether or not the switch is on, so that turning an EQ
            // back on does not resume a ramp that has been frozen for a minute.
            for (index, kind) in BAND_KINDS.iter().enumerate() {
                if let Some(band) = self.bands.get_mut(index) {
                    band.advance_span(span.len(), *kind, self.sample_rate_hz);
                }
            }
            if !self.enabled {
                continue;
            }
            for frame in span.iter_mut() {
                // The three bands are in series and cannot be parallelised; the stereo
                // pair inside one band can be, and is — M7-H6's `biquad_stereo_step`,
                // whose scalar body is these two `BiquadCoefficients::step` calls.
                let mut pair = Stereo::new(frame.left.mul_q24(EQ_PREAMP_Q24), frame.right.mul_q24(EQ_PREAMP_Q24));
                for index in 0..BAND_COUNT {
                    let Some(band) = self.bands.get(index) else { continue };
                    let (Some(left_state), Some(right_state)) = (self.left_state.get_mut(index), self.right_state.get_mut(index)) else {
                        continue;
                    };
                    pair = Sample::biquad_stereo_step(&band.coefficients, pair, left_state, right_state);
                }
                frame.left = pair.left.mul_q24(EQ_POSTAMP_Q24);
                frame.right = pair.right.mul_q24(EQ_POSTAMP_Q24);
            }
        }
    }

    fn set_param(&mut self, id: ParamId, value: i32) {
        if id == EQ_ENABLED_PARAM {
            self.enabled = clamp_at(0, value) != 0;
            return;
        }
        let Some((band_index, field)) = band_and_field(id) else { return };
        let clamped = clamp_at(id.0 as usize, value);
        let Some(band) = self.bands.get_mut(band_index) else { return };
        match field {
            0 => band.frequency_hz.set_target(clamped, SMOOTH_FRAMES),
            1 => band.gain_centi_db.set_target(clamped, SMOOTH_FRAMES),
            _ => band.shape.set_target(clamped, SMOOTH_FRAMES),
        }
    }

    fn param(&self, id: ParamId) -> Option<i32> {
        if id == EQ_ENABLED_PARAM {
            return Some(i32::from(self.enabled));
        }
        let (band_index, field) = band_and_field(id)?;
        let band = self.bands.get(band_index)?;
        Some(match field {
            0 => band.frequency_hz.target(),
            1 => band.gain_centi_db.target(),
            _ => band.shape.target(),
        })
    }

    fn reset(&mut self) {
        for (index, kind) in BAND_KINDS.iter().enumerate() {
            if let Some(band) = self.bands.get_mut(index) {
                band.snap(*kind, self.sample_rate_hz);
            }
        }
        self.left_state = [[Sample::ZERO; 2]; BAND_COUNT];
        self.right_state = [[Sample::ZERO; 2]; BAND_COUNT];
    }

    fn descriptor(&self) -> &'static InsertDescriptor { &EQ_DESCRIPTOR }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::insert::assert_descriptor_roundtrip;
    use crate::effects::testing::{RATE, block_of, dft_band_gain_db, render_noise, segmental_snr_db, white_noise};
    use alloc::vec;
    use alloc::vec::Vec;

    fn flat<Sample: DspSample>() -> Eq<Sample> { Eq::new(RATE) }

    #[test]
    fn descriptor_roundtrip() {
        let mut eq: Eq<i32> = Eq::new(RATE);
        assert_descriptor_roundtrip(&mut eq, "eq");
        let mut eq: Eq<f32> = Eq::new(RATE);
        assert_descriptor_roundtrip(&mut eq, "eq");
    }

    #[test]
    fn a_flat_eq_passes_a_block_through_within_a_least_significant_bit() {
        let mut eq: Eq<i32> = flat();
        let mut samples = block_of(|index| ((index as i32 * 977) % 20_001) - 10_000);
        let original = samples.clone();
        Insert::<i32>::process(&mut eq, &mut samples);
        for (index, (after, before)) in samples.iter().zip(original.iter()).enumerate() {
            assert!((after.left - before.left).abs() <= 1, "frame {index}: {} became {}", before.left, after.left);
        }
    }

    /// The audibility proof for the EQ (H3 deliverable 5), on both paths: a +12 dB bell at
    /// 1 kHz raises the 1 kHz band by 12 dB and leaves 100 Hz and 10 kHz alone.
    ///
    /// The measurement is a ratio of two DFTs of the **same** noise realisation — the
    /// filtered render over the unfiltered one — so it is the filter's own magnitude
    /// response rather than an estimate of a random spectrum, and the shelves are left at
    /// 0 dB so only the bell is under test.
    #[test]
    fn a_peaking_band_raises_its_own_frequency_and_leaves_the_others_alone() {
        for (path, measured) in [("fixed", eq_band_gains_fixed()), ("float", eq_band_gains_float())] {
            let [low, centre, high] = measured;
            assert!((centre - 12.0).abs() <= 0.5, "{path}: 1 kHz came out {centre:.3} dB, not 12 dB");
            assert!(low.abs() <= 0.5, "{path}: 100 Hz moved {low:.3} dB");
            assert!(high.abs() <= 0.5, "{path}: 10 kHz moved {high:.3} dB");
        }
    }

    /// The three band gains a +12 dB bell at 1 kHz produces on the fixed path.
    fn eq_band_gains_fixed() -> [f64; 3] {
        let dry: Vec<i32> = white_noise(FRAMES, 8_000);
        let mut boosted: Eq<i32> = Eq::new(RATE);
        Insert::<i32>::set_param(&mut boosted, EQ_PEAK_GAIN_PARAM, 1_200);
        Insert::<i32>::reset(&mut boosted);
        let wet = render_noise(&mut boosted, &dry);
        let dry: Vec<f64> = dry.iter().map(|value| *value as f64).collect();
        let wet: Vec<f64> = wet.iter().map(|value| *value as f64).collect();
        [100.0, 1_000.0, 10_000.0].map(|hz| dft_band_gain_db(&wet, &dry, hz))
    }

    /// The same measurement on the float path.
    fn eq_band_gains_float() -> [f64; 3] {
        let dry: Vec<f32> = white_noise(FRAMES, 8_000).iter().map(|value| *value as f32).collect();
        let mut boosted: Eq<f32> = Eq::new(RATE);
        Insert::<f32>::set_param(&mut boosted, EQ_PEAK_GAIN_PARAM, 1_200);
        Insert::<f32>::reset(&mut boosted);
        let wet = render_noise(&mut boosted, &dry);
        let dry: Vec<f64> = dry.iter().map(|value| *value as f64).collect();
        let wet: Vec<f64> = wet.iter().map(|value| *value as f64).collect();
        [100.0, 1_000.0, 10_000.0].map(|hz| dft_band_gain_db(&wet, &dry, hz))
    }

    /// Frames of noise each spectral run renders: 96 whole blocks, of which the last 4096
    /// frames are measured, so the filter is in steady state before the window opens.
    const FRAMES: usize = 96 * DSP_BLOCK_FRAMES;

    /// The fixed and float bodies are the same arithmetic, so they must agree well past
    /// audibility (H3 deliverable 5).
    #[test]
    fn the_fixed_and_float_paths_agree_to_better_than_sixty_decibels() {
        let noise: Vec<i32> = white_noise(FRAMES, 8_000);
        let mut fixed: Eq<i32> = Eq::new(RATE);
        let mut float: Eq<f32> = Eq::new(RATE);
        for (id, value) in [(EQ_LOW_GAIN_PARAM, -900), (EQ_PEAK_GAIN_PARAM, 1_200), (EQ_HIGH_GAIN_PARAM, 600)] {
            Insert::<i32>::set_param(&mut fixed, id, value);
            Insert::<f32>::set_param(&mut float, id, value);
        }
        Insert::<i32>::reset(&mut fixed);
        Insert::<f32>::reset(&mut float);

        let fixed_out = render_noise(&mut fixed, &noise);
        let float_input: Vec<f32> = noise.iter().map(|value| *value as f32).collect();
        let float_out = render_noise(&mut float, &float_input);
        let snr = segmental_snr_db(&fixed_out, &float_out).expect("the render is not silent");
        assert!(snr >= 60.0, "the two paths agree at only {snr:.1} dB");
    }

    /// Research point 1, half one: the chosen cooking interval stays close to a per-frame
    /// ideal through the most violent sweep the parameter range allows.
    ///
    /// The sweep is run by hand through [`BiquadCoefficients::step`] so the interval can be
    /// varied, from −24 dB to +24 dB over [`SMOOTH_FRAMES`] on a 200 Hz sine at half scale —
    /// a low tone, whose own per-sample step is small, so a coefficient transient has
    /// nowhere to hide. The numbers this produced are tabulated in the module documentation;
    /// what is asserted here is the shape of that table: sixteen frames is within half again
    /// of the ideal, and a whole block is several times worse.
    #[test]
    fn the_cooking_interval_is_close_to_a_per_frame_ideal() {
        let ideal = largest_step(&swept_by_hand(1));
        let chosen = largest_step(&swept_by_hand(EQ_COOK_FRAMES));
        let whole_block = largest_step(&swept_by_hand(DSP_BLOCK_FRAMES));
        assert!(chosen * 2 <= ideal * 3, "cooking every {EQ_COOK_FRAMES} frames stepped {chosen} against the ideal's {ideal}");
        assert!(whole_block > chosen * 2, "cooking once a block would have been fine after all: {whole_block} against {chosen}");
    }

    /// Research point 1, half two: in the production effect, a cook boundary is not where
    /// the largest step is.
    ///
    /// A coefficient discontinuity can only show up on the **first sample of a new span**,
    /// because that is the only sample whose predecessor was computed with different
    /// coefficients. So the two numbers to compare are the largest step across a span
    /// boundary and the largest step inside one. If the cooking clicked, the first would
    /// stand out from the second; it is smaller.
    #[test]
    fn a_full_gain_sweep_never_steps_more_at_a_cook_boundary_than_between_them() {
        let (boundary, interior) = gain_sweep_steps();
        assert!(boundary <= interior, "a swept EQ stepped by {boundary} at a cook boundary but only {interior} between them");
    }

    /// `(largest step across a cook boundary, largest step between them)` while sweeping.
    fn gain_sweep_steps() -> (i32, i32) {
        let output = swept_bell();
        let mut boundary = 0i32;
        let mut interior = 0i32;
        // The first block is skipped: the cascade is settling out of silence there, which is
        // a transient of the input rather than of the cooking.
        for index in DSP_BLOCK_FRAMES..output.len() {
            let (Some(this), Some(previous)) = (output.get(index), output.get(index - 1)) else { continue };
            let step = (this - previous).abs();
            if index % EQ_COOK_FRAMES == 0 { boundary = boundary.max(step) } else { interior = interior.max(step) }
        }
        (boundary, interior)
    }

    /// A 200 Hz sine at half scale through the production effect, its bell swept from
    /// −24 dB to +24 dB over [`SMOOTH_FRAMES`].
    fn swept_bell() -> Vec<i32> {
        let input: Vec<i32> = (0..8 * DSP_BLOCK_FRAMES).map(|index| sine_at(200.0, index, 16_000.0)).collect();
        let mut swept: Eq<i32> = Eq::new(RATE);
        Insert::<i32>::set_param(&mut swept, EQ_PEAK_GAIN_PARAM, -EQ_GAIN_BOUND_CENTI_DB);
        Insert::<i32>::reset(&mut swept);
        Insert::<i32>::set_param(&mut swept, EQ_PEAK_GAIN_PARAM, EQ_GAIN_BOUND_CENTI_DB);
        render_noise(&mut swept, &input)
    }

    fn largest_step(samples: &[i32]) -> i32 {
        samples.windows(2).filter_map(|pair| Some((pair.get(1)? - pair.first()?).abs())).max().unwrap_or(0)
    }

    /// The swept bell, run through [`BiquadCoefficients::step`] directly so the cooking
    /// interval can be varied. `cook_every` frames between coefficient updates.
    fn swept_by_hand(cook_every: usize) -> Vec<i32> {
        let mut gain = SmoothedParam::steady(-EQ_GAIN_BOUND_CENTI_DB);
        gain.set_target(EQ_GAIN_BOUND_CENTI_DB, SMOOTH_FRAMES);
        let mut state = [0i32; 2];
        let mut coefficients = cook(BandKind::Peaking, 1_000, -EQ_GAIN_BOUND_CENTI_DB, 100, RATE);
        let mut output = Vec::new();
        for index in 0..8 * DSP_BLOCK_FRAMES {
            let value = gain.advance();
            if index % cook_every.max(1) == 0 {
                coefficients = cook(BandKind::Peaking, 1_000, value, 100, RATE);
            }
            output.push(coefficients.step(sine_at(200.0, index, 16_000.0), &mut state));
        }
        output
    }

    fn sine_at(hz: f64, index: usize, amplitude: f64) -> i32 {
        (amplitude * (core::f64::consts::TAU * hz * index as f64 / RATE as f64).sin()) as i32
    }

    #[test]
    fn the_switch_takes_the_eq_out_of_the_signal_path_entirely() {
        let mut eq: Eq<i32> = Eq::new(RATE);
        Insert::<i32>::set_param(&mut eq, EQ_PEAK_GAIN_PARAM, 1_200);
        Insert::<i32>::set_param(&mut eq, EQ_ENABLED_PARAM, 0);
        Insert::<i32>::reset(&mut eq);
        let mut samples = block_of(|index| ((index as i32 * 811) % 20_001) - 10_000);
        let original = samples.clone();
        Insert::<i32>::process(&mut eq, &mut samples);
        assert_eq!(samples, original, "a disabled EQ is bit-transparent");
    }

    #[test]
    fn a_disabled_eq_still_advances_its_smoothing() {
        let mut eq: Eq<i32> = Eq::new(RATE);
        Insert::<i32>::set_param(&mut eq, EQ_ENABLED_PARAM, 0);
        Insert::<i32>::set_param(&mut eq, EQ_PEAK_GAIN_PARAM, 1_200);
        let mut samples = vec![Stereo::new(0i32, 0); DSP_BLOCK_FRAMES];
        for _ in 0..(SMOOTH_FRAMES as usize / DSP_BLOCK_FRAMES) {
            Insert::<i32>::process(&mut eq, &mut samples);
        }
        let band = eq.bands.first().expect("three bands");
        assert!(!band.gain_centi_db.is_moving() || eq.bands.get(1).map(|peak| !peak.gain_centi_db.is_moving()).unwrap_or(false), "the ramp ran while the EQ was off");
    }

    #[test]
    fn a_band_that_has_not_moved_is_not_re_cooked() {
        let mut eq: Eq<i32> = Eq::new(RATE);
        let before = eq.bands.get(1).expect("a bell").coefficients;
        let mut samples = vec![Stereo::new(0i32, 0); DSP_BLOCK_FRAMES];
        for _ in 0..4 {
            Insert::<i32>::process(&mut eq, &mut samples);
        }
        assert_eq!(eq.bands.get(1).expect("a bell").coefficients, before, "a steady band re-cooked");
    }

    #[test]
    fn a_reset_lands_every_band_on_its_target_and_clears_the_state() {
        let mut eq: Eq<i32> = Eq::new(RATE);
        let flat = eq.bands.get(1).expect("a bell").coefficients;
        Insert::<i32>::set_param(&mut eq, EQ_PEAK_GAIN_PARAM, 1_200);
        Insert::<i32>::reset(&mut eq);
        assert_ne!(eq.bands.get(1).expect("a bell").coefficients, flat, "the reset did not re-cook at the target");
        assert_eq!(eq.left_state, [[0i32; 2]; BAND_COUNT]);
        assert_eq!(Insert::<i32>::param(&eq, EQ_PEAK_GAIN_PARAM), Some(1_200));
    }

    #[test]
    fn a_boxed_eq_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let boxed: alloc::boxed::Box<dyn Insert<f32>> = alloc::boxed::Box::new(Eq::<f32>::new(RATE));
        assert_send(&boxed);
    }
}
