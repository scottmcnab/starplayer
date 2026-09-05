//! [`Compressor`] — a feed-forward, stereo-linked peak compressor whose gain computer runs
//! in the log domain off H2's tables (H4 deliverable 2).
//!
//! # Where the work happens
//!
//! Per **frame**: one comparison per channel to find the block's peak, and one
//! [`DspSample::scale_q15`] per channel to apply the gain. That is the whole per-sample
//! cost, and it is what the deliverable's "per-frame cost is one multiply" means.
//!
//! Per [`GAIN_COMPUTE_FRAMES`]: the gain computer. The interval's peak becomes a level in
//! centi-decibels through [`crate::tables::gain_to_centi_db`], the static curve is
//! evaluated there — threshold, soft knee, ratio — the attack/release ballistics smooth
//! the *reduction* (see below), make-up gain is added, and
//! [`crate::tables::db_to_gain_q15`] turns the result back into a Q1.15 gain that a
//! [`SmoothedParam`] ramps across the interval. Both conversions are table lookups;
//! nothing here calls a transcendental, on either path (§7.3).
//!
//! Everything from the peak onwards is integer arithmetic shared by the two mixing paths.
//! The **only** place they differ is [`DspSample::magnitude_i32`], which is how a sample
//! becomes a number the level detector can compare — which is why this effect's
//! fixed-versus-float agreement is limited by the applied gain's rounding and nothing else.
//!
//! # The ballistics smooth the gain, not the level
//!
//! The textbook feed-forward compressor puts its attack/release one-pole on the *level*
//! and computes the gain from the smoothed level. That topology cannot meet this task's own
//! timing requirement, and the reason is arithmetic rather than incidental: a one-pole in
//! the **linear** domain reaches 90 % of a level step in `2.3 τ`, but the gain reduction it
//! produces is a *logarithm* of that level, and a logarithm compresses the end of an
//! exponential approach. Worked through for the deliverable's own −20 → 0 dBFS step at
//! threshold −20 / ratio 4:1, a level-domain release reaches 90 % of its gain change only
//! after `3.6 τ` — over half again the budget — while the attack reaches it at `1.5 τ`.
//! One ballistic, two very different answers.
//!
//! So the one-pole sits on the **reduction in centi-decibels** instead, which is the
//! quantity the requirement is stated about: `reduction += (target − reduction) · (1 − a)`,
//! with `a` from [`crate::tables::time_constant_q24`] at the *control* rate — the sample
//! rate divided by [`GAIN_COMPUTE_FRAMES`] — so `attack` and `release` mean the same
//! number of milliseconds they would on the level, and 90 % of any change lands at `2.3 τ`
//! either way. This is the "smoothed branching" arrangement, and it is also why a slow
//! release does not pump on transients: the reduction is what is being smoothed.
//!
//! Two consequences are stated rather than hidden. The control signal is recomputed every
//! [`GAIN_COMPUTE_FRAMES`] frames, so a 90 % crossing is *observed* at the next control
//! update; and the Q1.15 gain that update produces is ramped across the interval that
//! follows it, so it is *reached* an interval later still. The measured time to 90 % is
//! therefore in `[2.3 τ, 2.3 τ + 2 · GAIN_COMPUTE_FRAMES)` — at the smallest attack this
//! effect offers that is a 1.5 ms window, and it is what
//! `the_attack_and_release_take_the_time_their_parameters_say` budgets for.
//!
//! # Peak, not RMS (research point 2)
//!
//! Peak. This compressor's job description includes sitting in the master chain ahead of
//! `starplayer_mixer::master`'s soft limiter and *not fighting it* — which means catching
//! the peaks the limiter would otherwise have to, and an RMS detector by construction does
//! not see them. Measured on this crate's own drum-loop fixture the crest factor is
//! **9.8 dB**, so an RMS detector at the same threshold would hand the limiter peaks nearly
//! ten decibels hotter than it thinks it is passing. The cost is not the argument — an RMS detector is
//! one extra multiply per sample and *half* a `log2`, since `log2(√x) = log2(x)/2` needs no
//! square root at all in a log-domain gain computer — the behaviour is.
//!
//! # No look-ahead, and therefore no latency
//!
//! There is no delay line in this file. The master chain must not delay the mix against the
//! scope taps and the telemetry, which sample voice state rather than the bus (§9.3), and a
//! look-ahead limiter would put the two a millisecond apart. What the design does instead
//! is take the peak of the interval it is *about* to process — zero output latency, and the
//! gain is already moving on the transient that caused it rather than one interval behind.

use crate::frame::Stereo;
use crate::insert::{DSP_BLOCK_FRAMES, Insert, InsertDescriptor, ParamId, ParamSpec, ParamUnit};
use crate::interpolate::round_shift_nearest;
use crate::sample::{DspSample, Q15_UNITY};
use crate::smooth::{SMOOTH_FRAMES, SmoothedParam};
use crate::tables::{CENTI_DB_MAX, CENTI_DB_MIN, db_to_gain_q15, gain_to_centi_db, time_constant_q24};

/// Frames between gain computations. Research point 3's answer, measured the way H3
/// measured the EQ's cooking interval — see
/// `the_gain_computers_interval_is_close_to_a_per_frame_ideal`.
///
/// It divides [`DSP_BLOCK_FRAMES`], so the sub-block split is a function of frames rendered
/// and nothing about it depends on the host's buffer size.
pub const GAIN_COMPUTE_FRAMES: usize = 32;

const _: () = assert!(DSP_BLOCK_FRAMES.is_multiple_of(GAIN_COMPUTE_FRAMES), "the gain computer's interval has to divide a DSP block");

/// The bottom of the threshold knob, in centi-decibels.
pub const THRESHOLD_MIN_CENTI_DB: i32 = -6_000;

/// The gentlest ratio, ×100: 1:1, which is no compression at all.
pub const RATIO_MIN: i32 = 100;

/// The steepest finite ratio, ×100: 20:1. Past that is the `limit` switch, which is ∞:1.
pub const RATIO_MAX: i32 = 2_000;

/// The widest soft knee, in centi-decibels: ±12 dB either side of the threshold.
pub const KNEE_MAX_CENTI_DB: i32 = 2_400;

/// Threshold, in centi-decibels below full scale.
pub const COMPRESSOR_THRESHOLD_PARAM: ParamId = ParamId(0);
/// Ratio, ×100.
pub const COMPRESSOR_RATIO_PARAM: ParamId = ParamId(1);
/// Attack, in whole milliseconds.
pub const COMPRESSOR_ATTACK_PARAM: ParamId = ParamId(2);
/// Release, in whole milliseconds.
pub const COMPRESSOR_RELEASE_PARAM: ParamId = ParamId(3);
/// Knee width, in centi-decibels. Zero is a hard knee.
pub const COMPRESSOR_KNEE_PARAM: ParamId = ParamId(4);
/// Make-up gain, in centi-decibels.
pub const COMPRESSOR_MAKEUP_PARAM: ParamId = ParamId(5);
/// Compute the make-up gain from the threshold and the ratio instead of reading it.
pub const COMPRESSOR_AUTO_MAKEUP_PARAM: ParamId = ParamId(6);
/// Ratio ∞:1 — a limiter, whatever the ratio knob says.
pub const COMPRESSOR_LIMIT_PARAM: ParamId = ParamId(7);

/// What a host draws for a [`Compressor`]. [`ParamId::GAIN_REDUCTION`] is *not* in here:
/// it is a meter, and a host draws it as one.
pub static COMPRESSOR_DESCRIPTOR: InsertDescriptor = InsertDescriptor {
    name: "compressor",
    params: &[
        ParamSpec { name: "threshold", unit: ParamUnit::CentiDecibels, min: THRESHOLD_MIN_CENTI_DB, max: 0, default: -1_200 },
        ParamSpec { name: "ratio", unit: ParamUnit::Ratio, min: RATIO_MIN, max: RATIO_MAX, default: 200 },
        ParamSpec { name: "attack", unit: ParamUnit::Milliseconds, min: 1, max: 500, default: 10 },
        ParamSpec { name: "release", unit: ParamUnit::Milliseconds, min: 1, max: 2_000, default: 100 },
        ParamSpec { name: "knee", unit: ParamUnit::CentiDecibels, min: 0, max: KNEE_MAX_CENTI_DB, default: 600 },
        ParamSpec { name: "makeup", unit: ParamUnit::CentiDecibels, min: -1_200, max: CENTI_DB_MAX, default: 0 },
        ParamSpec { name: "auto_makeup", unit: ParamUnit::Switch, min: 0, max: 1, default: 0 },
        ParamSpec { name: "limit", unit: ParamUnit::Switch, min: 0, max: 1, default: 0 },
    ],
};

/// A feed-forward, stereo-linked peak compressor.
#[derive(Clone, Debug)]
pub struct Compressor<Sample: DspSample> {
    sample_rate_hz: u32,
    threshold_centi_db: SmoothedParam,
    ratio: SmoothedParam,
    attack_ms: SmoothedParam,
    release_ms: SmoothedParam,
    knee_centi_db: SmoothedParam,
    makeup_centi_db: SmoothedParam,
    auto_makeup: bool,
    limit: bool,
    /// The smoothed gain reduction, in **Q8 centi-decibels** and non-negative — the state
    /// the attack/release ballistics act on. [`ParamId::GAIN_REDUCTION`] reads it back in
    /// whole centi-decibels; the eight fractional bits exist because the one-pole's own
    /// step near the end of a long attack is smaller than a centi-decibel, and rounding
    /// each step to the nearest one slows the ballistics measurably (a 100 ms attack
    /// measured 3 % long before this was in Q8, and inside a frame of the ideal after).
    reduction_q8: i32,
    /// The Q1.15 gain the per-frame multiply uses, ramped across each interval.
    gain: SmoothedParam,
    /// Frames between gain computations. Always [`GAIN_COMPUTE_FRAMES`] in a shipped
    /// build; a field rather than the constant itself only so that research point 3's
    /// experiment can measure the alternatives on the production effect rather than on a
    /// reimplementation of it.
    compute_interval: usize,
    /// `Sample` never appears in a field, but the effect is still one path's or the other's:
    /// `magnitude_i32` is where they differ.
    marker: core::marker::PhantomData<fn() -> Sample>,
}

impl<Sample: DspSample> Compressor<Sample> {
    /// A compressor at this sample rate. Allocates nothing — there is no look-ahead line —
    /// but it is still built off the audio thread and installed by command like every other
    /// insert.
    pub fn new(sample_rate_hz: u32) -> Compressor<Sample> {
        Compressor {
            sample_rate_hz: sample_rate_hz.max(1),
            threshold_centi_db: SmoothedParam::steady(default_at(0)),
            ratio: SmoothedParam::steady(default_at(1)),
            attack_ms: SmoothedParam::steady(default_at(2)),
            release_ms: SmoothedParam::steady(default_at(3)),
            knee_centi_db: SmoothedParam::steady(default_at(4)),
            makeup_centi_db: SmoothedParam::steady(default_at(5)),
            auto_makeup: default_at(6) != 0,
            limit: default_at(7) != 0,
            reduction_q8: 0,
            gain: SmoothedParam::steady(Q15_UNITY),
            compute_interval: GAIN_COMPUTE_FRAMES,
            marker: core::marker::PhantomData,
        }
    }

    /// The rate the ballistics run at: one step per [`GAIN_COMPUTE_FRAMES`] output frames.
    ///
    /// Passing this to [`time_constant_q24`] rather than the audio sample rate is what
    /// makes `attack` and `release` mean the same milliseconds they would if the one-pole
    /// ran per frame.
    fn control_rate_hz(&self) -> u32 { (self.sample_rate_hz / GAIN_COMPUTE_FRAMES as u32).max(1) }

    /// Advance every smoothed parameter by one control interval. Sampling a per-frame ramp
    /// once an interval is still a function of frames rendered, so it is block-size
    /// independent for the same reason the EQ's sixteen-frame cooking is.
    fn advance_parameters(&mut self, frames: usize) {
        for parameter in [
            &mut self.threshold_centi_db,
            &mut self.ratio,
            &mut self.attack_ms,
            &mut self.release_ms,
            &mut self.knee_centi_db,
            &mut self.makeup_centi_db,
        ] {
            if parameter.is_moving() {
                for _ in 0..frames {
                    parameter.advance();
                }
            }
        }
    }

    /// The static curve's slope, `1 − 1/ratio`, in Q1.15. The `limit` switch is ∞:1, whose
    /// slope is exactly unity.
    fn slope_q15(&self) -> i32 {
        if self.limit {
            return Q15_UNITY;
        }
        let ratio = self.ratio.current().clamp(RATIO_MIN, RATIO_MAX) as i64;
        (((ratio - RATIO_MIN as i64) * Q15_UNITY as i64) / ratio) as i32
    }

    /// The static curve: how much reduction, in centi-decibels, a level of `level_centi_db`
    /// earns. Hard below the knee, quadratic through it, linear above it — the standard
    /// smooth-knee curve, evaluated entirely in the log domain where it is a polynomial.
    fn static_reduction_centi_db(&self, level_centi_db: i32) -> i32 {
        let slope = self.slope_q15() as i64;
        let over = (level_centi_db - self.threshold_centi_db.current()) as i64;
        let knee = self.knee_centi_db.current().clamp(0, KNEE_MAX_CENTI_DB) as i64;
        let half_knee = knee / 2;
        if over <= -half_knee {
            return 0;
        }
        if knee > 0 && over < half_knee {
            let distance = over + half_knee;
            return round_divide(distance * distance * slope, 2 * knee * Q15_UNITY as i64) as i32;
        }
        round_shift_nearest(over * slope, 15) as i32
    }

    /// The make-up gain in centi-decibels: the knob, or — with `auto_makeup` on — exactly
    /// the reduction the curve applies to a full-scale signal, so that 0 dBFS in is 0 dBFS
    /// out and turning the ratio up does not turn the mix down.
    fn makeup_centi_db(&self) -> i32 {
        if !self.auto_makeup {
            return self.makeup_centi_db.current();
        }
        self.static_reduction_centi_db(0).clamp(0, CENTI_DB_MAX)
    }

    /// One step of the attack/release one-pole on the reduction, towards `target`, in Q8
    /// centi-decibels.
    fn ballistics_step(&mut self, target_centi_db: i32) {
        let target_q8 = target_centi_db.saturating_mul(256);
        let milliseconds = if target_q8 > self.reduction_q8 { self.attack_ms.current() } else { self.release_ms.current() };
        let coefficient_q24 = time_constant_q24(milliseconds, self.control_rate_hz());
        let difference = (self.reduction_q8 - target_q8) as i64;
        let remaining = round_shift_nearest(difference * coefficient_q24 as i64, 24) as i32;
        // The same flush H3's delay needs and for the same reason: `round(v · a)` has small
        // non-zero fixed points for every `a` above a half, so a reduction left exactly
        // where it was has reached the arithmetic's floor. A Q8 centi-decibel is 0.00004 dB,
        // so this can only ever discard a remainder nothing could measure.
        let remaining = if coefficient_q24 < 1 << 24 && remaining == difference as i32 { 0 } else { remaining };
        self.reduction_q8 = target_q8.saturating_add(remaining).clamp(0, -CENTI_DB_MIN * 256);
    }

    /// The gain computer, run once per [`GAIN_COMPUTE_FRAMES`] on the interval's peak.
    fn compute_gain(&mut self, peak: i32, frames: usize) {
        self.advance_parameters(frames);
        let level_centi_db = gain_to_centi_db(peak);
        let target = self.static_reduction_centi_db(level_centi_db);
        self.ballistics_step(target);
        let gain_centi_db = self.makeup_centi_db().saturating_sub(self.gain_reduction_centi_db());
        self.gain.set_target(db_to_gain_q15(gain_centi_db), frames as u32);
    }

    /// The gain reduction currently applied, in centi-decibels and non-negative — what
    /// [`ParamId::GAIN_REDUCTION`] hands a host.
    pub const fn gain_reduction_centi_db(&self) -> i32 { (self.reduction_q8 + 128) >> 8 }

    /// Research point 3's experiment only: run the gain computer every `frames` frames
    /// instead of every [`GAIN_COMPUTE_FRAMES`]. Clamped to a divisor of a block, because
    /// anything else would make the effect depend on where a block boundary fell.
    #[cfg(test)]
    fn set_compute_interval(&mut self, frames: usize) {
        self.compute_interval = if frames > 0 && DSP_BLOCK_FRAMES.is_multiple_of(frames) { frames } else { GAIN_COMPUTE_FRAMES };
    }
}

/// The descriptor's default for one parameter position.
fn default_at(index: usize) -> i32 { COMPRESSOR_DESCRIPTOR.params.get(index).map_or(0, |spec| spec.default) }

/// The descriptor's clamp for one parameter position.
fn clamp_at(index: usize, value: i32) -> i32 { COMPRESSOR_DESCRIPTOR.params.get(index).map_or(value, |spec| spec.clamp(value)) }

/// `numerator / denominator`, rounded to nearest with ties away from zero. `denominator` is
/// positive at every call site here.
fn round_divide(numerator: i64, denominator: i64) -> i64 {
    let denominator = denominator.max(1);
    if numerator >= 0 { (numerator + denominator / 2) / denominator } else { -((-numerator + denominator / 2) / denominator) }
}

impl<Sample: DspSample> Insert<Sample> for Compressor<Sample> {
    fn process(&mut self, block: &mut [Stereo<Sample>]) {
        debug_assert_eq!(block.len(), DSP_BLOCK_FRAMES, "an insert only ever sees a whole DSP block");
        let interval = self.compute_interval;
        for span in block.chunks_mut(interval) {
            // Pass one: the interval's stereo-linked peak. Reading the input before the
            // gain is applied is what makes this feed-forward, and taking the peak of the
            // interval about to be processed is what makes look-ahead unnecessary.
            let mut peak = 0i32;
            for frame in span.iter() {
                peak = peak.max(frame.left.magnitude_i32()).max(frame.right.magnitude_i32());
            }
            self.compute_gain(peak, span.len());

            // Pass two: one multiply per sample.
            for frame in span.iter_mut() {
                let gain = self.gain.advance();
                frame.left = frame.left.scale_q15(gain);
                frame.right = frame.right.scale_q15(gain);
            }
        }
    }

    fn set_param(&mut self, id: ParamId, value: i32) {
        match id {
            COMPRESSOR_THRESHOLD_PARAM => self.threshold_centi_db.set_target(clamp_at(0, value), SMOOTH_FRAMES),
            COMPRESSOR_RATIO_PARAM => self.ratio.set_target(clamp_at(1, value), SMOOTH_FRAMES),
            COMPRESSOR_ATTACK_PARAM => self.attack_ms.set_target(clamp_at(2, value), SMOOTH_FRAMES),
            COMPRESSOR_RELEASE_PARAM => self.release_ms.set_target(clamp_at(3, value), SMOOTH_FRAMES),
            COMPRESSOR_KNEE_PARAM => self.knee_centi_db.set_target(clamp_at(4, value), SMOOTH_FRAMES),
            COMPRESSOR_MAKEUP_PARAM => self.makeup_centi_db.set_target(clamp_at(5, value), SMOOTH_FRAMES),
            COMPRESSOR_AUTO_MAKEUP_PARAM => self.auto_makeup = clamp_at(6, value) != 0,
            COMPRESSOR_LIMIT_PARAM => self.limit = clamp_at(7, value) != 0,
            // Everything else, meters included, is not settable.
            _ => {}
        }
    }

    fn param(&self, id: ParamId) -> Option<i32> {
        match id {
            COMPRESSOR_THRESHOLD_PARAM => Some(self.threshold_centi_db.target()),
            COMPRESSOR_RATIO_PARAM => Some(self.ratio.target()),
            COMPRESSOR_ATTACK_PARAM => Some(self.attack_ms.target()),
            COMPRESSOR_RELEASE_PARAM => Some(self.release_ms.target()),
            COMPRESSOR_KNEE_PARAM => Some(self.knee_centi_db.target()),
            COMPRESSOR_MAKEUP_PARAM => Some(self.makeup_centi_db.target()),
            COMPRESSOR_AUTO_MAKEUP_PARAM => Some(i32::from(self.auto_makeup)),
            COMPRESSOR_LIMIT_PARAM => Some(i32::from(self.limit)),
            ParamId::GAIN_REDUCTION => Some(self.gain_reduction_centi_db()),
            _ => None,
        }
    }

    fn reset(&mut self) {
        self.threshold_centi_db.snap();
        self.ratio.snap();
        self.attack_ms.snap();
        self.release_ms.snap();
        self.knee_centi_db.snap();
        self.makeup_centi_db.snap();
        self.reduction_q8 = 0;
        self.gain = SmoothedParam::steady(db_to_gain_q15(self.makeup_centi_db()));
    }

    fn descriptor(&self) -> &'static InsertDescriptor { &COMPRESSOR_DESCRIPTOR }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::testing::{RATE, block_of, drum_loop, render_noise, render_stereo, segmental_snr_db};
    use crate::insert::assert_descriptor_roundtrip;
    use alloc::vec::Vec;

    /// The deliverable's static-curve settings: threshold −20 dB, 4:1, hard knee.
    fn static_curve<Sample: DspSample>() -> Compressor<Sample> {
        let mut compressor: Compressor<Sample> = Compressor::new(RATE);
        Insert::<Sample>::set_param(&mut compressor, COMPRESSOR_THRESHOLD_PARAM, -2_000);
        Insert::<Sample>::set_param(&mut compressor, COMPRESSOR_RATIO_PARAM, 400);
        Insert::<Sample>::set_param(&mut compressor, COMPRESSOR_KNEE_PARAM, 0);
        Insert::<Sample>::set_param(&mut compressor, COMPRESSOR_ATTACK_PARAM, 5);
        Insert::<Sample>::set_param(&mut compressor, COMPRESSOR_RELEASE_PARAM, 200);
        Insert::<Sample>::reset(&mut compressor);
        compressor
    }

    /// A sine at `dbfs`, `frames` long, on the raw `i16` scale.
    fn sine(frames: usize, hz: f64, dbfs: f64) -> Vec<i32> {
        let amplitude = 32_767.0 * 10.0f64.powf(dbfs / 20.0);
        (0..frames).map(|index| (amplitude * (core::f64::consts::TAU * hz * index as f64 / RATE as f64).sin()).round() as i32).collect()
    }

    /// The signal every timing measurement uses: `|x|` is exactly `amplitude` on every
    /// frame, so the applied gain can be read straight off the output without a detector of
    /// the test's own.
    fn alternating(frames: usize, amplitude: i32) -> Vec<i32> {
        (0..frames).map(|index| if index % 2 == 0 { amplitude } else { -amplitude }).collect()
    }

    /// The last `frames` of `samples`, in dBFS, measured as an RMS lifted to a sine's peak.
    fn level_dbfs(samples: &[i32], frames: usize) -> f64 {
        let window = samples.get(samples.len().saturating_sub(frames)..).unwrap_or(&[]);
        let energy: f64 = window.iter().map(|value| (*value as f64) * (*value as f64)).sum();
        let rms = (energy / window.len().max(1) as f64).sqrt();
        if rms <= 0.0 { -200.0 } else { 20.0 * (rms * core::f64::consts::SQRT_2 / 32_767.0).log10() }
    }

    #[test]
    fn descriptor_roundtrip() {
        let mut compressor: Compressor<i32> = Compressor::new(RATE);
        assert_descriptor_roundtrip(&mut compressor, "compressor");
        let mut compressor: Compressor<f32> = Compressor::new(RATE);
        assert_descriptor_roundtrip(&mut compressor, "compressor");
    }

    /// The meter is read-only and lives outside the descriptor, which is what
    /// [`ParamId::METER_BASE`] is for.
    #[test]
    fn the_gain_reduction_meter_is_read_only_and_is_not_a_slider() {
        let mut compressor: Compressor<i32> = Compressor::new(RATE);
        assert!(ParamId::GAIN_REDUCTION.is_meter());
        assert!(COMPRESSOR_DESCRIPTOR.params.len() < ParamId::METER_BASE.0 as usize, "no parameter position may reach into the meter range");
        assert_eq!(Insert::<i32>::param(&compressor, ParamId::GAIN_REDUCTION), Some(0));
        Insert::<i32>::set_param(&mut compressor, ParamId::GAIN_REDUCTION, 1_234);
        assert_eq!(Insert::<i32>::param(&compressor, ParamId::GAIN_REDUCTION), Some(0), "the meter took a written value");
    }

    /// H4 deliverable 4's static curve, on both paths: −40, −20, −10 and 0 dBFS in come out
    /// at −40, −20, −17.5 and −15 dBFS through threshold −20 / 4:1 / hard knee.
    #[test]
    fn the_static_curve_is_the_one_the_ratio_and_threshold_describe() {
        const FRAMES: usize = 512 * DSP_BLOCK_FRAMES;
        for (input_dbfs, expected) in [(-40.0f64, -40.0f64), (-20.0, -20.0), (-10.0, -17.5), (0.0, -15.0)] {
            let input = sine(FRAMES, 1_000.0, input_dbfs);
            let mut fixed: Compressor<i32> = static_curve();
            let fixed_output = render_noise(&mut fixed, &input);
            let measured = level_dbfs(&fixed_output, 16 * DSP_BLOCK_FRAMES);
            assert!((measured - expected).abs() <= 0.3, "fixed: {input_dbfs} dBFS in came out at {measured:.2} dBFS, not {expected}");

            let float_input: Vec<f32> = input.iter().map(|value| *value as f32).collect();
            let mut float: Compressor<f32> = static_curve();
            let float_output = render_noise(&mut float, &float_input);
            let as_fixed: Vec<i32> = float_output.iter().map(|value| value.round() as i32).collect();
            let measured = level_dbfs(&as_fixed, 16 * DSP_BLOCK_FRAMES);
            assert!((measured - expected).abs() <= 0.3, "float: {input_dbfs} dBFS in came out at {measured:.2} dBFS, not {expected}");
        }
    }

    /// The meter reads back what the output actually lost, which is what H7 will draw.
    #[test]
    fn the_gain_reduction_read_back_matches_the_measured_reduction() {
        const FRAMES: usize = 512 * DSP_BLOCK_FRAMES;
        let input = sine(FRAMES, 1_000.0, 0.0);
        let mut compressor: Compressor<i32> = static_curve();
        let output = render_noise(&mut compressor, &input);
        let measured = level_dbfs(&input, 16 * DSP_BLOCK_FRAMES) - level_dbfs(&output, 16 * DSP_BLOCK_FRAMES);
        let read_back = Insert::<i32>::param(&compressor, ParamId::GAIN_REDUCTION).expect("the meter is there") as f64 / 100.0;
        assert!((measured - read_back).abs() <= 0.5, "the meter says {read_back:.2} dB and the output lost {measured:.2} dB");
    }

    /// Where the applied gain, read frame by frame off an alternating signal, first reaches
    /// `fraction` of the way from its starting reduction to its settled one.
    fn frames_to_reach(output: &[i32], amplitude: i32, fraction: f64) -> Option<usize> {
        let reduction = |value: i32| -20.0 * ((value.abs() as f64 / amplitude as f64).max(1e-9)).log10();
        let start = reduction(*output.first()?);
        let settled = reduction(*output.last()?);
        let target = start + (settled - start) * fraction;
        output.iter().position(|value| if settled > start { reduction(*value) >= target } else { reduction(*value) <= target })
    }

    /// H4 deliverable 4's timing: a −20 → 0 dBFS step reaches 90 % of its gain reduction
    /// within `attack × 2.3`, and a 0 → −20 dBFS step releases likewise.
    ///
    /// The budget carries one [`GAIN_COMPUTE_FRAMES`] on top of the continuous ideal,
    /// because that is when the crossing is *observed* — see the module documentation.
    #[test]
    fn the_attack_and_release_take_the_time_their_parameters_say() {
        for milliseconds in [10i32, 20, 50, 100] {
            let ideal = 2.3 * milliseconds as f64 * RATE as f64 / 1_000.0;
            let budget = ideal + 2.0 * GAIN_COMPUTE_FRAMES as f64;
            let frames = (ideal as usize * 4).next_multiple_of(DSP_BLOCK_FRAMES);

            let mut compressor: Compressor<i32> = static_curve();
            Insert::<i32>::set_param(&mut compressor, COMPRESSOR_ATTACK_PARAM, milliseconds);
            Insert::<i32>::set_param(&mut compressor, COMPRESSOR_RELEASE_PARAM, milliseconds);
            Insert::<i32>::reset(&mut compressor);

            // Settle at −20 dBFS, where the curve takes nothing off, then step to 0 dBFS.
            let quiet = 3_277i32;
            let _ = render_noise(&mut compressor, &alternating(frames, quiet));
            let attacked = render_noise(&mut compressor, &alternating(frames, 32_767));
            let attack_frames = frames_to_reach(&attacked, 32_767, 0.9).expect("the attack settles");
            assert!(attack_frames as f64 <= budget, "a {milliseconds} ms attack took {attack_frames} frames, past a budget of {budget:.0}");

            let released = render_noise(&mut compressor, &alternating(frames, quiet));
            let release_frames = frames_to_reach(&released, quiet, 0.9).expect("the release settles");
            assert!(release_frames as f64 <= budget, "a {milliseconds} ms release took {release_frames} frames, past a budget of {budget:.0}");
        }
    }

    /// Research point 3, measured: how far a gain computer running every `N` frames is from
    /// one running every frame, and what actually decides the interval.
    ///
    /// The experiment mirrors H3's for the EQ's cooking interval. The signal is a click
    /// train at full scale **riding on a steady −20 dBFS carrier of constant magnitude**,
    /// through a limiter at −20 dBFS with a 1 ms attack and a 50 ms release — the most
    /// violent thing the parameter range allows. The carrier is what makes the measurement
    /// meaningful: between the clicks its output is exactly the applied gain, so comparing
    /// two intervals' output away from the clicks compares their gain trajectories, which
    /// is what "pumping" is. The clicks themselves are excluded, because how much of a
    /// four-sample click escapes a compressor with no look-ahead is a question about
    /// look-ahead, not about the interval.
    ///
    /// | frames between gain computations | agreement with the per-frame ideal |
    /// |---|---|
    /// | 2 | 15.1 dB |
    /// | 4 | 10.2 dB |
    /// | 8 | 5.5 dB |
    /// | 16 | 3.2 dB |
    /// | **32 (chosen)** | **2.1 dB** |
    /// | 64 | 1.6 dB |
    /// | 128 (a whole block) | 1.4 dB |
    ///
    /// The curve is monotone and has **no knee**, which is the finding: at an attack of one
    /// millisecond — 44 frames — no block-rate interval reproduces a per-frame detector,
    /// because the interval's *peak hold* is itself comparable to the attack. What the
    /// number of frames does decide, and what this test therefore asserts, is the timing
    /// quantisation of `attack` and `release`: the gain reaches an update's value two
    /// intervals after the level that caused it (see the module documentation), which at 32
    /// frames is 6 % of the *default* 10 ms attack's `2.3 τ` and at a whole block is 25 %.
    #[test]
    fn the_gain_computers_interval_is_close_to_a_per_frame_ideal() {
        const FRAMES: usize = 64 * DSP_BLOCK_FRAMES;
        const CLICK_PERIOD: usize = 512;
        const CARRIER: i32 = 3_277;

        fn render_at(interval: usize) -> Vec<i32> {
            let input: Vec<i32> = (0..FRAMES)
                .map(|index| {
                    let amplitude = if index % CLICK_PERIOD < 4 { 32_767 } else { CARRIER };
                    if index % 2 == 0 { amplitude } else { -amplitude }
                })
                .collect();
            let mut compressor: Compressor<i32> = Compressor::new(RATE);
            compressor.set_compute_interval(interval);
            Insert::<i32>::set_param(&mut compressor, COMPRESSOR_THRESHOLD_PARAM, -2_000);
            Insert::<i32>::set_param(&mut compressor, COMPRESSOR_LIMIT_PARAM, 1);
            Insert::<i32>::set_param(&mut compressor, COMPRESSOR_KNEE_PARAM, 0);
            Insert::<i32>::set_param(&mut compressor, COMPRESSOR_ATTACK_PARAM, 1);
            Insert::<i32>::set_param(&mut compressor, COMPRESSOR_RELEASE_PARAM, 50);
            Insert::<i32>::reset(&mut compressor);
            render_noise(&mut compressor, &input)
        }

        /// Signal-to-error ratio in decibels between a candidate and the per-frame ideal,
        /// over the carrier only.
        fn agreement_db(candidate: &[i32], ideal: &[i32]) -> f64 {
            let mut signal = 0.0f64;
            let mut error = 0.0f64;
            for (index, (measured, reference)) in candidate.iter().zip(ideal.iter()).enumerate() {
                if index % CLICK_PERIOD < 8 {
                    continue;
                }
                signal += (*reference as f64) * (*reference as f64);
                error += ((*measured - *reference) as f64) * ((*measured - *reference) as f64);
            }
            if error <= 0.0 { 120.0 } else { 10.0 * (signal / error).log10() }
        }

        let ideal = render_at(1);
        let measured: Vec<(usize, f64)> = [2usize, 4, 8, 16, GAIN_COMPUTE_FRAMES, 64, DSP_BLOCK_FRAMES].iter().map(|interval| (*interval, agreement_db(&render_at(*interval), &ideal))).collect();
        for pair in measured.windows(2) {
            let (finer, coarser) = (pair.first().copied().unwrap_or((0, 0.0)), pair.get(1).copied().unwrap_or((0, 0.0)));
            assert!(coarser.1 <= finer.1 + 0.5, "{} frames agreed better ({:.1} dB) than {} frames ({:.1} dB)", coarser.0, coarser.1, finer.0, finer.1);
        }
        let chosen = measured.iter().find(|(interval, _)| *interval == GAIN_COMPUTE_FRAMES).map(|(_, db)| *db).unwrap_or(0.0);
        let whole_block = measured.iter().find(|(interval, _)| *interval == DSP_BLOCK_FRAMES).map(|(_, db)| *db).unwrap_or(0.0);
        assert!(chosen > whole_block, "the chosen interval ({chosen:.1} dB) has to be at least a little closer to the ideal than a whole block ({whole_block:.1} dB)");

        // What actually decides the interval: the timing quantisation it puts on the
        // default attack, which has to stay a small fraction of the attack itself.
        let default_attack_ms = COMPRESSOR_DESCRIPTOR.params.get(2).map(|spec| spec.default).unwrap_or(10) as f64;
        let ninety_percent_frames = 2.3 * default_attack_ms * RATE as f64 / 1_000.0;
        let quantisation = 2.0 * GAIN_COMPUTE_FRAMES as f64;
        assert!(quantisation <= ninety_percent_frames / 10.0, "the interval quantises the default attack by {:.0} %", 100.0 * quantisation / ninety_percent_frames);
        assert!(2.0 * DSP_BLOCK_FRAMES as f64 > ninety_percent_frames / 10.0, "a whole block would have done just as well, so the split is not earning its keep");
    }

    #[test]
    fn a_compressor_below_its_threshold_is_bit_transparent() {
        let mut compressor: Compressor<i32> = Compressor::new(RATE);
        Insert::<i32>::reset(&mut compressor);
        // −40 dBFS, well under the −12 dB default threshold and its 6 dB knee.
        let mut samples = block_of(|index| ((index as i32 * 37) % 655) - 327);
        let original = samples.clone();
        Insert::<i32>::process(&mut compressor, &mut samples);
        assert_eq!(samples, original, "a compressor doing nothing changed the block");
        assert_eq!(Insert::<i32>::param(&compressor, ParamId::GAIN_REDUCTION), Some(0));
    }

    #[test]
    fn the_limit_switch_is_an_infinite_ratio() {
        let mut compressor: Compressor<i32> = static_curve();
        assert_eq!(compressor.slope_q15(), (3 * Q15_UNITY) / 4, "4:1 takes three quarters of the excess");
        Insert::<i32>::set_param(&mut compressor, COMPRESSOR_LIMIT_PARAM, 1);
        assert_eq!(compressor.slope_q15(), Q15_UNITY, "∞:1 takes all of it");
        assert_eq!(compressor.static_reduction_centi_db(0), 2_000, "a full-scale signal is brought to the threshold exactly");
    }

    #[test]
    fn the_soft_knee_is_continuous_where_it_meets_the_straight_parts() {
        let mut compressor: Compressor<i32> = static_curve();
        Insert::<i32>::set_param(&mut compressor, COMPRESSOR_KNEE_PARAM, 1_200);
        Insert::<i32>::reset(&mut compressor);
        let threshold = -2_000;
        assert_eq!(compressor.static_reduction_centi_db(threshold - 600), 0, "the bottom of the knee takes nothing off");
        // At the top of the knee the quadratic has to meet the 4:1 line, which at 6 dB over
        // the threshold takes 4.5 dB off.
        assert!((compressor.static_reduction_centi_db(threshold + 600) - 450).abs() <= 2, "the top of the knee does not meet the line");
        // And halfway through it takes a quarter of what the line would.
        assert!((compressor.static_reduction_centi_db(threshold) - 112).abs() <= 2, "the knee's middle is not the quadratic's");
        // Monotone, with no step anywhere across the knee.
        let mut previous = 0;
        for level in threshold - 700..=threshold + 700 {
            let reduction = compressor.static_reduction_centi_db(level);
            assert!(reduction >= previous, "the curve went backwards at {level}");
            assert!(reduction - previous <= 2, "the curve stepped {} centi-dB at {level}", reduction - previous);
            previous = reduction;
        }
    }

    #[test]
    fn auto_makeup_brings_a_full_scale_signal_back_to_full_scale() {
        let mut compressor: Compressor<i32> = static_curve();
        Insert::<i32>::set_param(&mut compressor, COMPRESSOR_AUTO_MAKEUP_PARAM, 1);
        Insert::<i32>::reset(&mut compressor);
        assert_eq!(compressor.makeup_centi_db(), 1_500, "threshold −20 at 4:1 takes 15 dB off a full-scale signal");
        const FRAMES: usize = 512 * DSP_BLOCK_FRAMES;
        let input = sine(FRAMES, 1_000.0, 0.0);
        let output = render_noise(&mut compressor, &input);
        let measured = level_dbfs(&output, 16 * DSP_BLOCK_FRAMES);
        assert!(measured.abs() <= 0.3, "a full-scale signal came back at {measured:.2} dBFS");
    }

    #[test]
    fn the_fixed_and_float_paths_agree_on_a_drum_loop() {
        let input = drum_loop(32 * DSP_BLOCK_FRAMES);
        let float_input: Vec<f32> = input.iter().map(|value| *value as f32).collect();
        let mut fixed: Compressor<i32> = Compressor::new(RATE);
        let mut float: Compressor<f32> = Compressor::new(RATE);
        for (id, value) in [(COMPRESSOR_THRESHOLD_PARAM, -1_800), (COMPRESSOR_RATIO_PARAM, 400), (COMPRESSOR_ATTACK_PARAM, 5), (COMPRESSOR_RELEASE_PARAM, 80)] {
            Insert::<i32>::set_param(&mut fixed, id, value);
            Insert::<f32>::set_param(&mut float, id, value);
        }
        Insert::<i32>::reset(&mut fixed);
        Insert::<f32>::reset(&mut float);
        let fixed_output = render_noise(&mut fixed, &input);
        let float_output = render_noise(&mut float, &float_input);
        let snr = segmental_snr_db(&fixed_output, &float_output).expect("the render is not silent");
        assert!(snr >= 50.0, "the two paths agree at only {snr:.1} dB");
    }

    /// Research point 2's deciding measurement: the crest factor an RMS detector would
    /// hide from the master limiter.
    #[test]
    fn a_peak_detector_sees_what_an_rms_detector_would_hide() {
        let loop_frames = 32 * DSP_BLOCK_FRAMES;
        let samples = drum_loop(loop_frames);
        let peak = samples.iter().map(|value| value.abs()).max().unwrap_or(0) as f64;
        let energy: f64 = samples.iter().map(|value| (*value as f64) * (*value as f64)).sum();
        let rms = (energy / samples.len() as f64).sqrt();
        let crest_db = 20.0 * (peak / rms).log10();
        assert!(crest_db > 8.0, "the fixture's crest factor is only {crest_db:.1} dB, which would not make the case either way");
    }

    #[test]
    fn a_compressor_does_not_change_the_block_length_or_delay_it() {
        // Look-ahead 0: the effect holds no delay line at all, so frame `n` out is a
        // multiple of frame `n` in and nothing is shifted against the scope taps.
        let mut compressor: Compressor<i32> = Compressor::new(RATE);
        Insert::<i32>::reset(&mut compressor);
        let input: Vec<Stereo<i32>> = (0..DSP_BLOCK_FRAMES).map(|index| Stereo::new(if index == 7 { 30_000 } else { 0 }, 0)).collect();
        let output = render_stereo(&mut compressor, &input);
        assert!(output.iter().enumerate().all(|(index, frame)| (index == 7) == (frame.left != 0)), "the impulse moved");
    }

    #[test]
    fn a_boxed_compressor_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let boxed: alloc::boxed::Box<dyn Insert<f32>> = alloc::boxed::Box::new(Compressor::<f32>::new(RATE));
        assert_send(&boxed);
    }
}
