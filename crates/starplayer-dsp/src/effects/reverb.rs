//! [`Reverb`] — Freeverb's topology, on both mixing paths (H4 deliverable 1).
//!
//! # Why Freeverb
//!
//! Jezar at Dreampoint's public-domain design: per channel eight parallel lowpass-feedback
//! comb filters into four series allpasses, the right channel's delay lengths offset by
//! [`STEREO_SPREAD`] frames. It is table-free — every coefficient is a gain, not a filter
//! cooked from a transcendental — integer-friendly, and universally recognised as "a
//! reverb", which is exactly what M7's exit criterion asks the owner to listen for. A
//! feedback delay network or a convolution would be a better reverb and a worse first one.
//!
//! # The signal path, stated so H6 can vectorise it without changing a bit
//!
//! Per frame, in this order:
//!
//! 1. The stereo input is written to the **pre-delay** line and read back at the smoothed
//!    pre-delay position ([`DelayLine::read_fractional`], Q8 frames, exactly as H3's delay
//!    smooths its time). At zero the read is the frame just written, so a pre-delay of
//!    zero is the input unaltered.
//! 2. `comb_input = (pre_left + pre_right) · INPUT_GAIN` — one mono signal drives both
//!    channels' comb banks, as Freeverb does. [`INPUT_GAIN_Q24`] is Jezar's `0.015` times
//!    `2^`[`REVERB_HEADROOM_BITS`] (see below), so it reads as `0.96` in Q8.24.
//! 3. Each channel's **eight combs** are stepped with that input and their outputs summed
//!    into one `i32`. Integer addition is associative, so a wide sum may reduce them in
//!    any order.
//! 4. The sum is scaled by [`COMB_SUM_SCALE_Q24`] — exactly `2^-3`, the deliverable's
//!    `>> 3` headroom shift, realised through [`DspSample::mul_q24`] so that it rounds to
//!    nearest rather than towards negative infinity and so that the float path performs
//!    the identical (exact) division. **The wet signal is the mean of the eight combs**,
//!    not their sum.
//! 5. The mean runs through the channel's **four allpasses** in series.
//! 6. `WET_SCALE_Q24` (`0.375` = Jezar's `scalewet = 3`, divided by the `2^6` of headroom
//!    and multiplied by the `8` step 4 divided out) brings the wet signal back to the
//!    sample's own scale, the **width** matrix crosses the two channels, and the
//!    equal-power **mix** ([`crate::tables::equal_power_q15`]) sums it against the dry.
//!
//! # Six bits of headroom, as the EQ has
//!
//! Jezar's input gain of `0.015` is what keeps a comb bank whose loop gain reaches 0.98
//! from running away, and on the fixed path it also throws away six bits of the input
//! before the recursion that amplifies rounding error ever starts. The comb bank therefore
//! runs `2^`[`REVERB_HEADROOM_BITS`] above the sample — the input gain absorbs the
//! multiplication and [`WET_SCALE_Q24`] the division, so there is **no extra multiply per
//! frame at all**. Measured on the drum-loop fixture, fixed-versus-float agreement moves
//! from **60.6 dB without the headroom to 84.4 dB with it**, and — the reason it is not
//! merely nice — the feedback multiply's own fixed points move from −62.7 dBFS to
//! −98.7 dBFS, which is what research point 1 asks them to be below. This is the device
//! `crate::effects::eq` uses for its biquad cascade and `crate::filter` for IT's voice
//! filter, for the same reason each time.
//!
//! "The comb state saturates" therefore means at [`COMB_SATURATION_BOUND`], `32767 << 6`,
//! through [`DspSample::saturate_at`] rather than [`DspSample::saturate`].
//!
//! # Research point 1: `i32` state, no limit cycle
//!
//! [`DspSample::mul_q24`] rounds to nearest with ties away from zero, so — exactly as H3
//! found for its delay's feedback — `round(v · f) == v` has small non-zero solutions for
//! every `f` above a half: at the top of the room-size knob (`f = 0.98`) every
//! `|v| ≤ 25` is a fixed point. A comb whose feedback multiply cannot move its own state
//! rings for ever instead of decaying.
//!
//! Two things answer it, and neither is 64-bit state:
//!
//! * The six bits of headroom put those fixed points at `25 / 64` of one unit of the
//!   `i16` scale — **−98.4 dBFS**, already below the −90 dBFS this task requires of an
//!   idle reverb.
//! * [`attenuate_q24`] then flushes them: a feedback multiply that leaves a sample exactly
//!   where it was is proof the tail has reached the arithmetic's floor, and the sample is
//!   zeroed. So the measured idle floor is not −98 dBFS but **exactly zero**, and an
//!   installed reverb with nothing playing costs the bus nothing at all. On the float path
//!   a below-unity multiply moves every value but zero, so the flush is a no-op there.
//!
//! `freeze` is the one place a feedback multiply is *at* unity, and `mul_q24(1 << 24)` is
//! exact on both paths, so a frozen tail circulates without losing a bit.

use crate::delay_line::{DelayLine, StereoDelayLine};
use crate::frame::Stereo;
use crate::insert::{DSP_BLOCK_FRAMES, Insert, InsertDescriptor, ParamId, ParamSpec, ParamUnit};
use crate::sample::{DspSample, Q15_UNITY};
use crate::smooth::{SMOOTH_FRAMES, SmoothedParam};
use crate::tables::equal_power_q15;

/// Parallel comb filters per channel.
pub const COMB_COUNT: usize = 8;

/// Series allpasses per channel.
pub const ALLPASS_COUNT: usize = 4;

/// Jezar's comb tunings, in frames at 44.1 kHz. Scaled to the actual sample rate at
/// [`Reverb::new`]; mutually prime-ish lengths are what stop the eight combs' echoes
/// coinciding into a flutter.
pub const COMB_TUNING_44100: [u32; COMB_COUNT] = [1_116, 1_188, 1_277, 1_356, 1_422, 1_491, 1_557, 1_617];

/// Jezar's allpass tunings, in frames at 44.1 kHz.
pub const ALLPASS_TUNING_44100: [u32; ALLPASS_COUNT] = [556, 441, 341, 225];

/// Frames the right channel's lines are longer than the left's, at 44.1 kHz — Freeverb's
/// `stereospread`, and the whole of its stereo width before the width matrix.
pub const STEREO_SPREAD: u32 = 23;

/// Bits the comb bank runs above the sample. Six, for the reason
/// [`crate::effects::eq`]'s cascade uses six: `1 << (24 + 6)` is the largest power of two a
/// Q8.24 `i32` coefficient can carry, and six still leaves 256× of headroom over full scale.
pub const REVERB_HEADROOM_BITS: u32 = 6;

/// What "the comb state saturates" means: full scale at the headroom's own scale.
pub const COMB_SATURATION_BOUND: i32 = (i16::MAX as i32) << REVERB_HEADROOM_BITS;

/// Freeverb's input gain (`0.015`) times `2^REVERB_HEADROOM_BITS`, in Q8.24 — so `0.96`.
pub const INPUT_GAIN_Q24: i32 = 16_106_127;

/// The deliverable's `>> 3`: exactly `2^-3` in Q8.24, applied through
/// [`DspSample::mul_q24`] so it rounds to nearest on the fixed path and is exact on the
/// float one.
pub const COMB_SUM_SCALE_Q24: i32 = 1 << 21;

/// Freeverb's allpass feedback, `0.5`, in Q8.24.
pub const ALLPASS_FEEDBACK_Q24: i32 = 1 << 23;

/// What the wet signal is multiplied by on its way out of the allpass chain: Jezar's
/// `scalewet = 3`, times the `8` [`COMB_SUM_SCALE_Q24`] divided out, divided by the
/// `2^REVERB_HEADROOM_BITS` the comb bank ran above the sample — `3 × 8 / 64 = 0.375`.
pub const WET_SCALE_Q24: i32 = 6_291_456;

/// The comb feedback at room size 0 %, in Q8.24: `0.7`, Freeverb's `offsetroom`.
pub const ROOM_MIN_Q24: i32 = 11_744_051;

/// What the room-size knob adds across its whole travel, in Q8.24: `0.28`, Freeverb's
/// `scaleroom`, so 100 % is `0.98`.
pub const ROOM_SPAN_Q24: i32 = 4_697_620;

/// The comb's one-pole damping coefficient at damping 100 %, in Q8.24: `0.4`, Freeverb's
/// `scaledamp`. Above a half the one-pole would have fixed points of its own, and at unity
/// it would stop tracking its input altogether.
pub const DAMPING_MAX_Q24: i32 = 6_710_886;

/// The longest pre-delay a host may ask for, in milliseconds. The line is sized for this
/// at [`Reverb::new`], which is why it is a constant rather than a parameter.
pub const PRE_DELAY_MAX_MS: i32 = 100;

/// Room size, in percent: the comb feedback, 0.7 at 0 % to 0.98 at 100 %.
pub const REVERB_ROOM_PARAM: ParamId = ParamId(0);
/// Damping, in percent: the comb's one-pole coefficient, 0 to 0.4.
pub const REVERB_DAMPING_PARAM: ParamId = ParamId(1);
/// Stereo width, in percent.
pub const REVERB_WIDTH_PARAM: ParamId = ParamId(2);
/// Wet/dry mix, in percent.
pub const REVERB_MIX_PARAM: ParamId = ParamId(3);
/// Pre-delay, in whole milliseconds.
pub const REVERB_PRE_DELAY_PARAM: ParamId = ParamId(4);
/// Freeze: unity feedback, no damping, no new input.
pub const REVERB_FREEZE_PARAM: ParamId = ParamId(5);

/// What a host draws for a [`Reverb`].
pub static REVERB_DESCRIPTOR: InsertDescriptor = InsertDescriptor {
    name: "reverb",
    params: &[
        ParamSpec { name: "room", unit: ParamUnit::Percent, min: 0, max: 100, default: 50 },
        ParamSpec { name: "damping", unit: ParamUnit::Percent, min: 0, max: 100, default: 50 },
        ParamSpec { name: "width", unit: ParamUnit::Percent, min: 0, max: 100, default: 100 },
        ParamSpec { name: "mix", unit: ParamUnit::Percent, min: 0, max: 100, default: 30 },
        ParamSpec { name: "pre_delay", unit: ParamUnit::Milliseconds, min: 0, max: PRE_DELAY_MAX_MS, default: 0 },
        ParamSpec { name: "freeze", unit: ParamUnit::Switch, min: 0, max: 1, default: 0 },
    ],
};

/// One lowpass-feedback comb filter: a delay line, its damping one-pole's state, and the
/// delay it is read at.
#[derive(Clone, Debug)]
struct Comb<Sample: DspSample> {
    line: DelayLine<Sample>,
    length: u32,
    store: Sample,
}

impl<Sample: DspSample> Comb<Sample> {
    fn new(length: u32) -> Comb<Sample> {
        let length = length.max(1);
        Comb { line: DelayLine::new(length as usize), length, store: Sample::ZERO }
    }

    /// `y = x + f·((1 − d)·y[n − L] + d·store)`, Jezar's comb exactly, with the write
    /// saturated at the headroom's own full scale and the feedback multiply flushed.
    ///
    /// The line is read *before* it is written, so a delay of `L` frames reads at `L − 1`
    /// — the same off-by-one H3's delay spells out rather than leaving to be rediscovered.
    fn step(&mut self, input: Sample, feedback_q24: i32, damping_q24: i32, damping_complement_q24: i32) -> Sample {
        let delayed = self.line.read(self.length - 1);
        self.store = delayed.mul_q24(damping_complement_q24).add(self.store.mul_q24(damping_q24));
        let written = input.add(attenuate_q24(self.store, feedback_q24));
        self.line.write(written.saturate_at(COMB_SATURATION_BOUND));
        delayed
    }

    fn reset(&mut self) {
        self.line.reset();
        self.store = Sample::ZERO;
    }
}

/// One Schroeder allpass, at Freeverb's fixed feedback of a half.
#[derive(Clone, Debug)]
struct Allpass<Sample: DspSample> {
    line: DelayLine<Sample>,
    length: u32,
}

impl<Sample: DspSample> Allpass<Sample> {
    fn new(length: u32) -> Allpass<Sample> {
        let length = length.max(1);
        Allpass { line: DelayLine::new(length as usize), length }
    }

    fn step(&mut self, input: Sample) -> Sample {
        let delayed = self.line.read(self.length - 1);
        self.line.write(input.add(attenuate_q24(delayed, ALLPASS_FEEDBACK_Q24)).saturate_at(COMB_SATURATION_BOUND));
        delayed.sub(input)
    }

    fn reset(&mut self) { self.line.reset(); }
}

/// One channel's comb bank and allpass chain.
#[derive(Clone, Debug)]
struct Tank<Sample: DspSample> {
    combs: [Comb<Sample>; COMB_COUNT],
    allpasses: [Allpass<Sample>; ALLPASS_COUNT],
}

impl<Sample: DspSample> Tank<Sample> {
    /// `offset` is [`STEREO_SPREAD`] for the right channel and zero for the left, already
    /// scaled to the sample rate.
    fn new(sample_rate_hz: u32, offset: u32) -> Tank<Sample> {
        Tank {
            combs: core::array::from_fn(|index| Comb::new(scaled_length(COMB_TUNING_44100.get(index).copied().unwrap_or(1_116), sample_rate_hz) + offset)),
            allpasses: core::array::from_fn(|index| Allpass::new(scaled_length(ALLPASS_TUNING_44100.get(index).copied().unwrap_or(556), sample_rate_hz) + offset)),
        }
    }

    /// Steps 3 to 5 of the module documentation's signal path.
    fn step(&mut self, input: Sample, feedback_q24: i32, damping_q24: i32, damping_complement_q24: i32) -> Sample {
        let mut sum = Sample::ZERO;
        for comb in self.combs.iter_mut() {
            sum = sum.add(comb.step(input, feedback_q24, damping_q24, damping_complement_q24));
        }
        let mut value = sum.mul_q24(COMB_SUM_SCALE_Q24);
        for allpass in self.allpasses.iter_mut() {
            value = allpass.step(value);
        }
        value
    }

    fn reset(&mut self) {
        for comb in self.combs.iter_mut() {
            comb.reset();
        }
        for allpass in self.allpasses.iter_mut() {
            allpass.reset();
        }
    }

    /// Frames of delay line this tank holds, for the memory measurement.
    fn capacity_frames(&self) -> usize {
        self.combs.iter().map(|comb| comb.line.capacity()).sum::<usize>() + self.allpasses.iter().map(|allpass| allpass.line.capacity()).sum::<usize>()
    }
}

/// A Freeverb: a stereo pre-delay into two tanks, crossed by width and mixed against the dry.
#[derive(Clone, Debug)]
pub struct Reverb<Sample: DspSample> {
    sample_rate_hz: u32,
    pre_delay: StereoDelayLine<Sample>,
    /// The largest Q8-frame pre-delay the line can serve, leaving the fractional read a
    /// frame to look further back.
    max_pre_delay_q8: i32,
    left: Tank<Sample>,
    right: Tank<Sample>,
    room_percent: SmoothedParam,
    damping_percent: SmoothedParam,
    width_percent: SmoothedParam,
    mix_percent: SmoothedParam,
    pre_delay_ms: i32,
    pre_delay_q8: SmoothedParam,
    freeze: bool,
    /// Cooked once per block from `room_percent`, `damping_percent` and `freeze`.
    feedback_q24: i32,
    damping_q24: i32,
}

impl<Sample: DspSample> Reverb<Sample> {
    /// A reverb at this sample rate, with every line sized for it.
    ///
    /// **This allocates** — sixteen comb rings, eight allpass rings and a stereo pre-delay
    /// — which is exactly why an effect is built on the control thread and crosses to the
    /// engine boxed (M7 decision 4). See
    /// `the_memory_one_instance_costs_is_the_number_the_task_asked_for` for the total.
    pub fn new(sample_rate_hz: u32) -> Reverb<Sample> {
        let sample_rate_hz = sample_rate_hz.max(1);
        let offset = scaled_length(STEREO_SPREAD, sample_rate_hz);
        // Two frames of headroom: the fractional read looks one frame further back than
        // the whole part.
        let capacity_frames = (milliseconds_to_q8_frames(PRE_DELAY_MAX_MS, sample_rate_hz) / 256) as usize + 2;
        let pre_delay = StereoDelayLine::new(capacity_frames);
        let max_pre_delay_q8 = (((pre_delay.left.capacity() - 2) as i64) * 256).min(i32::MAX as i64) as i32;
        let pre_delay_ms = default_at(4);
        let mut reverb = Reverb {
            sample_rate_hz,
            pre_delay,
            max_pre_delay_q8,
            left: Tank::new(sample_rate_hz, 0),
            right: Tank::new(sample_rate_hz, offset),
            room_percent: SmoothedParam::steady(default_at(0)),
            damping_percent: SmoothedParam::steady(default_at(1)),
            width_percent: SmoothedParam::steady(default_at(2)),
            mix_percent: SmoothedParam::steady(default_at(3)),
            pre_delay_ms,
            pre_delay_q8: SmoothedParam::steady(0),
            freeze: default_at(5) != 0,
            feedback_q24: 0,
            damping_q24: 0,
        };
        reverb.pre_delay_q8 = SmoothedParam::steady(reverb.pre_delay_to_q8(pre_delay_ms));
        reverb.cook();
        reverb
    }

    /// Milliseconds to the Q8-frame position the pre-delay line is read at.
    fn pre_delay_to_q8(&self, milliseconds: i32) -> i32 {
        let frames_q8 = milliseconds_to_q8_frames(milliseconds.clamp(0, PRE_DELAY_MAX_MS), self.sample_rate_hz);
        frames_q8.clamp(0, self.max_pre_delay_q8 as i64) as i32
    }

    /// The comb feedback and damping coefficients for the parameters as they currently
    /// stand. Cooked once per block — a comb's feedback is a scalar loop gain, not a
    /// resonant biquad's pole position, so moving it changes a decay rate rather than
    /// ringing something; this is the same call H3 made for its delay's damping one-pole.
    fn cook(&mut self) {
        if self.freeze {
            // Unity feedback (exact through `mul_q24`), no damping, and — in `process` —
            // no new input: the tail circulates unchanged.
            self.feedback_q24 = 1 << 24;
            self.damping_q24 = 0;
            return;
        }
        self.feedback_q24 = room_feedback_q24(self.room_percent.current());
        self.damping_q24 = damping_coefficient_q24(self.damping_percent.current());
    }

    /// Frames of delay line one instance holds, across the pre-delay and both tanks.
    pub fn capacity_frames(&self) -> usize {
        self.pre_delay.left.capacity() + self.pre_delay.right.capacity() + self.left.capacity_frames() + self.right.capacity_frames()
    }
}

/// A 44.1 kHz tuning at `sample_rate_hz`, rounded to nearest and never zero.
fn scaled_length(tuning_44100: u32, sample_rate_hz: u32) -> u32 {
    let scaled = (tuning_44100 as u64 * sample_rate_hz as u64 + 22_050) / 44_100;
    scaled.max(1).min(u32::MAX as u64) as u32
}

/// `milliseconds` as Q8 frames at `sample_rate_hz`, computed from both inputs each time
/// for the reason H3's delay spells out: a rounded frames-per-millisecond constant puts a
/// delay a fraction of a frame off its exact position.
fn milliseconds_to_q8_frames(milliseconds: i32, sample_rate_hz: u32) -> i64 {
    (milliseconds.max(0) as i64 * sample_rate_hz as i64 * 256 + 500) / 1_000
}

/// The comb feedback for a room-size percentage, in Q8.24: 0.7 at 0 %, 0.98 at 100 %.
fn room_feedback_q24(percent: i32) -> i32 {
    let clamped = percent.clamp(0, 100) as i64;
    ROOM_MIN_Q24 + ((clamped * ROOM_SPAN_Q24 as i64) / 100) as i32
}

/// The comb's damping one-pole coefficient for a damping percentage, in Q8.24.
fn damping_coefficient_q24(percent: i32) -> i32 {
    let clamped = percent.clamp(0, 100) as i64;
    ((clamped * DAMPING_MAX_Q24 as i64) / 100) as i32
}

/// Freeverb's width matrix: `(own, crossed)` in Q1.15, `(1, 0)` at 100 % and
/// `(0.5, 0.5)` — a mono wet signal — at 0 %.
fn width_gains_q15(percent: i32) -> (i32, i32) {
    let clamped = percent.clamp(0, 100) as i64;
    let half = Q15_UNITY as i64 / 2;
    let own = half + (clamped * half) / 100;
    let crossed = ((100 - clamped) * half) / 100;
    (own as i32, crossed as i32)
}

/// The descriptor's default for one parameter position.
fn default_at(index: usize) -> i32 { REVERB_DESCRIPTOR.params.get(index).map_or(0, |spec| spec.default) }

/// The descriptor's clamp for one parameter position.
fn clamp_at(index: usize, value: i32) -> i32 { REVERB_DESCRIPTOR.params.get(index).map_or(value, |spec| spec.clamp(value)) }

/// `value × coefficient`, with the fixed path's rounding fixed points flushed to silence —
/// see "Research point 1" in the module documentation. At unity (which is what `freeze`
/// sets the comb feedback to) the multiply is exact and the flush cannot fire.
fn attenuate_q24<Sample: DspSample>(value: Sample, coefficient_q24: i32) -> Sample {
    let scaled = value.mul_q24(coefficient_q24);
    if coefficient_q24 < 1 << 24 && scaled == value { Sample::ZERO } else { scaled }
}

impl<Sample: DspSample> Insert<Sample> for Reverb<Sample> {
    fn process(&mut self, block: &mut [Stereo<Sample>]) {
        debug_assert_eq!(block.len(), DSP_BLOCK_FRAMES, "an insert only ever sees a whole DSP block");
        // The two coefficients cook once per block; the width matrix, the mix and the
        // pre-delay position are per frame. Same division of labour as the EQ and the
        // delay, and for the same reason.
        for parameter in [&mut self.room_percent, &mut self.damping_percent] {
            if parameter.is_moving() {
                for _ in 0..DSP_BLOCK_FRAMES {
                    parameter.advance();
                }
            }
        }
        self.cook();
        let feedback_q24 = self.feedback_q24;
        let damping_q24 = self.damping_q24;
        let damping_complement_q24 = (1 << 24) - damping_q24;
        let input_gain_q24 = if self.freeze { 0 } else { INPUT_GAIN_Q24 };

        for frame in block.iter_mut() {
            let position_q8 = self.pre_delay_q8.advance().clamp(0, self.max_pre_delay_q8);
            self.pre_delay.write(frame.left, frame.right);
            let index_q16 = (position_q8 as u32) << 8;
            let pre_left = self.pre_delay.left.read_fractional(index_q16);
            let pre_right = self.pre_delay.right.read_fractional(index_q16);

            let tank_input = pre_left.add(pre_right).mul_q24(input_gain_q24);
            let wet_left = self.left.step(tank_input, feedback_q24, damping_q24, damping_complement_q24).mul_q24(WET_SCALE_Q24);
            let wet_right = self.right.step(tank_input, feedback_q24, damping_q24, damping_complement_q24).mul_q24(WET_SCALE_Q24);

            let (own, crossed) = width_gains_q15(self.width_percent.advance());
            let widened_left = wet_left.scale_q15(own).add(wet_right.scale_q15(crossed));
            let widened_right = wet_right.scale_q15(own).add(wet_left.scale_q15(crossed));

            let (dry, wet) = equal_power_q15(self.mix_percent.advance());
            frame.left = frame.left.scale_q15(dry).add(widened_left.scale_q15(wet));
            frame.right = frame.right.scale_q15(dry).add(widened_right.scale_q15(wet));
        }
    }

    fn set_param(&mut self, id: ParamId, value: i32) {
        match id {
            REVERB_ROOM_PARAM => self.room_percent.set_target(clamp_at(0, value), SMOOTH_FRAMES),
            REVERB_DAMPING_PARAM => self.damping_percent.set_target(clamp_at(1, value), SMOOTH_FRAMES),
            REVERB_WIDTH_PARAM => self.width_percent.set_target(clamp_at(2, value), SMOOTH_FRAMES),
            REVERB_MIX_PARAM => self.mix_percent.set_target(clamp_at(3, value), SMOOTH_FRAMES),
            REVERB_PRE_DELAY_PARAM => {
                self.pre_delay_ms = clamp_at(4, value);
                let target = self.pre_delay_to_q8(self.pre_delay_ms);
                self.pre_delay_q8.set_target(target, SMOOTH_FRAMES);
            }
            REVERB_FREEZE_PARAM => self.freeze = clamp_at(5, value) != 0,
            _ => {}
        }
    }

    fn param(&self, id: ParamId) -> Option<i32> {
        match id {
            REVERB_ROOM_PARAM => Some(self.room_percent.target()),
            REVERB_DAMPING_PARAM => Some(self.damping_percent.target()),
            REVERB_WIDTH_PARAM => Some(self.width_percent.target()),
            REVERB_MIX_PARAM => Some(self.mix_percent.target()),
            REVERB_PRE_DELAY_PARAM => Some(self.pre_delay_ms),
            REVERB_FREEZE_PARAM => Some(i32::from(self.freeze)),
            _ => None,
        }
    }

    fn reset(&mut self) {
        self.pre_delay.reset();
        self.left.reset();
        self.right.reset();
        self.room_percent.snap();
        self.damping_percent.snap();
        self.width_percent.snap();
        self.mix_percent.snap();
        self.pre_delay_q8.snap();
        self.cook();
    }

    fn descriptor(&self) -> &'static InsertDescriptor { &REVERB_DESCRIPTOR }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::testing::{RATE, block_of, drum_loop, render_noise, render_stereo, segmental_snr_db};
    use crate::insert::assert_descriptor_roundtrip;
    use alloc::vec;
    use alloc::vec::Vec;

    /// The impulse amplitude every decay measurement uses: full scale, so a level in
    /// decibels below it is a level in dBFS.
    const IMPULSE: i32 = 32_767;

    /// Frames one decay window covers: 100 ms, the resolution the deliverable states.
    const WINDOW_FRAMES: usize = (RATE as usize) / 10;

    fn configured<Sample: DspSample>(room: i32, damping: i32, mix: i32) -> Reverb<Sample> {
        let mut reverb: Reverb<Sample> = Reverb::new(RATE);
        Insert::<Sample>::set_param(&mut reverb, REVERB_ROOM_PARAM, room);
        Insert::<Sample>::set_param(&mut reverb, REVERB_DAMPING_PARAM, damping);
        Insert::<Sample>::set_param(&mut reverb, REVERB_MIX_PARAM, mix);
        Insert::<Sample>::reset(&mut reverb);
        reverb
    }

    /// An impulse on frame zero of both channels, silence afterwards.
    fn impulse_input<Sample: DspSample>(frames: usize) -> Vec<Stereo<Sample>> {
        (0..frames)
            .map(|index| {
                let value = if index == 0 { Sample::from_i16(IMPULSE as i16) } else { Sample::ZERO };
                Stereo::new(value, value)
            })
            .collect()
    }

    /// Per-window RMS of a left channel, in dBFS.
    fn window_rms_dbfs(left: &[f64], window_frames: usize) -> Vec<f64> {
        left.chunks(window_frames)
            .filter(|window| window.len() == window_frames)
            .map(|window| {
                let energy: f64 = window.iter().map(|value| value * value).sum();
                let rms = (energy / window.len() as f64).sqrt();
                if rms <= 0.0 { -200.0 } else { 20.0 * (rms / 32_768.0).log10() }
            })
            .collect()
    }

    /// The left channel of a fixed-path render, as `f64`.
    fn left_fixed(output: &[Stereo<i32>]) -> Vec<f64> { output.iter().map(|frame| frame.left as f64).collect() }

    /// The left channel of a float-path render, as `f64` — kept at full precision rather
    /// than rounded to the `i16` grid, so a decay measurement is not floored by the cast.
    fn left_float(output: &[Stereo<f32>]) -> Vec<f64> { output.iter().map(|frame| frame.left as f64).collect() }

    /// RT60 by the acoustician's method, and for the acoustician's reason: the tail's own
    /// arithmetic floor is reached long before 60 dB of decay, so the slope is fitted over
    /// the windows between `from_db` and `to_db` below the first window and extrapolated
    /// to 60 dB. Returns seconds, or `None` if the decay never covers that range.
    fn rt60_seconds(windows: &[f64], from_db: f64, to_db: f64, window_frames: usize) -> Option<f64> {
        let first = windows.first().copied()?;
        let points: Vec<(f64, f64)> = windows
            .iter()
            .enumerate()
            .filter(|(_, level)| **level <= first - from_db && **level >= first - to_db)
            .map(|(index, level)| (index as f64 * window_frames as f64 / RATE as f64, *level))
            .collect();
        if points.len() < 3 {
            return None;
        }
        let count = points.len() as f64;
        let mean_time = points.iter().map(|(time, _)| *time).sum::<f64>() / count;
        let mean_level = points.iter().map(|(_, level)| *level).sum::<f64>() / count;
        let covariance: f64 = points.iter().map(|(time, level)| (time - mean_time) * (level - mean_level)).sum();
        let variance: f64 = points.iter().map(|(time, _)| (time - mean_time) * (time - mean_time)).sum();
        if variance <= 0.0 || covariance >= 0.0 {
            return None;
        }
        Some(-60.0 / (covariance / variance))
    }

    #[test]
    fn descriptor_roundtrip() {
        let mut reverb: Reverb<i32> = Reverb::new(RATE);
        assert_descriptor_roundtrip(&mut reverb, "reverb");
        let mut reverb: Reverb<f32> = Reverb::new(RATE);
        assert_descriptor_roundtrip(&mut reverb, "reverb");
    }

    /// H4 deliverable 4: an impulse at room 50 % / mix 100 % decays monotonically per
    /// 100 ms window, and its RT60 lands between 0.5 s and 3 s on both paths.
    #[test]
    fn an_impulse_decays_monotonically_with_an_rt60_a_room_would_have() {
        let frames = 1_024 * DSP_BLOCK_FRAMES;
        let mut fixed: Reverb<i32> = configured(50, 50, 100);
        let fixed_output = render_stereo(&mut fixed, &impulse_input::<i32>(frames));
        let windows = window_rms_dbfs(&left_fixed(&fixed_output), WINDOW_FRAMES);
        assert!(windows.len() >= 20, "the render has to cover the whole decay");

        // Monotone from the second window: the first holds the impulse itself and the
        // reverb's own build-up, which is not part of the decay.
        for pair in windows.get(1..).unwrap_or(&[]).windows(2) {
            let (earlier, later) = (pair.first().copied().unwrap_or(0.0), pair.get(1).copied().unwrap_or(0.0));
            assert!(later <= earlier + 0.001, "the tail rose from {earlier:.2} dBFS to {later:.2} dBFS");
        }

        let fixed_rt60 = rt60_seconds(windows.get(1..).unwrap_or(&[]), 5.0, 35.0, WINDOW_FRAMES).expect("the tail decays");
        assert!((0.5..=3.0).contains(&fixed_rt60), "the fixed path's RT60 is {fixed_rt60:.2} s");

        let mut float: Reverb<f32> = configured(50, 50, 100);
        let float_output = render_stereo(&mut float, &impulse_input::<f32>(frames));
        let float_windows = window_rms_dbfs(&left_float(&float_output), WINDOW_FRAMES);
        let float_rt60 = rt60_seconds(float_windows.get(1..).unwrap_or(&[]), 5.0, 35.0, WINDOW_FRAMES).expect("the float tail decays");
        assert!((0.5..=3.0).contains(&float_rt60), "the float path's RT60 is {float_rt60:.2} s");
        assert!((fixed_rt60 - float_rt60).abs() < 0.2, "the two paths disagree: {fixed_rt60:.2} s against {float_rt60:.2} s");
    }

    /// The smallest room the knob offers is a short one: everything from 300 ms after the
    /// impulse onwards is below −60 dBFS.
    ///
    /// Measured as one RMS over the whole remainder of the render rather than as a peak,
    /// because a decaying reverb tail is a dense series of echoes and it is the energy in
    /// the room, not the tallest surviving sample, that "decays below −60 dBFS" is about.
    /// Per 100 ms window the tail reads −33.1, −41.8, −55.0, −65.2, −74.9, −83.9, −92.7,
    /// −108.0 dBFS and is then exactly zero, so the level crosses −60 dBFS at about 270 ms.
    #[test]
    fn the_smallest_room_with_full_damping_is_below_minus_sixty_dbfs_within_three_hundred_milliseconds() {
        let frames = 512 * DSP_BLOCK_FRAMES;
        let mut reverb: Reverb<i32> = configured(0, 100, 100);
        let output = render_stereo(&mut reverb, &impulse_input::<i32>(frames));
        let left = left_fixed(&output);
        let after = left.get(3 * WINDOW_FRAMES..).expect("the render runs past 300 ms");
        let tail = window_rms_dbfs(after, after.len()).first().copied().expect("one window over the whole remainder");
        assert!(tail < -60.0, "everything past 300 ms averages {tail:.1} dBFS");
    }

    /// Freeze holds the tail rather than decaying it: unity feedback is exact through
    /// `mul_q24` on both paths, so the comb buffers circulate without losing a bit.
    #[test]
    fn freeze_holds_the_tail_for_five_seconds() {
        let mut reverb: Reverb<i32> = configured(50, 50, 100);
        // Half a second of noise to fill the tanks, then freeze and let it run.
        let excitation = drum_loop(RATE as usize / 2);
        let filled = render_noise(&mut reverb, &excitation);
        assert!(filled.iter().any(|value| *value != 0), "the tanks have something in them");
        Insert::<i32>::set_param(&mut reverb, REVERB_FREEZE_PARAM, 1);

        // Six seconds: one to settle after the freeze, then the five the deliverable asks
        // about. The window is a whole second rather than the decay measurements' 100 ms,
        // because a frozen tail is eight comb buffers circulating at their own periods and
        // a short window sees a different part of each — which is a measurement artefact,
        // not the tail changing level.
        let frames = 6 * RATE as usize;
        let held = render_stereo(&mut reverb, &vec![Stereo::new(0i32, 0); frames]);
        let windows = window_rms_dbfs(&left_fixed(&held), RATE as usize);
        let settled = windows.get(1..).unwrap_or(&[]);
        let highest = settled.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let lowest = settled.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(lowest > -80.0, "the frozen tail died away to {lowest:.1} dBFS");
        assert!(highest - lowest <= 1.0, "the frozen tail moved {:.2} dB, from {lowest:.1} to {highest:.1} dBFS", highest - lowest);
    }

    /// Research point 1, measured: the fixed points a Q8.24 feedback multiply has, where
    /// six bits of headroom put them, and what the flush does about them.
    #[test]
    fn the_feedback_multiplys_fixed_points_are_below_minus_ninety_dbfs_and_are_flushed_anyway() {
        let top = room_feedback_q24(100);
        let largest_fixed_point = (1..1_000i32).take_while(|value| value.mul_q24(top) == *value).last().expect("there are some");
        assert_eq!(largest_fixed_point, 24, "the room-size knob's top has fixed points up to 24");
        let as_dbfs = 20.0 * ((largest_fixed_point as f64 / (1 << REVERB_HEADROOM_BITS) as f64) / 32_768.0).log10();
        assert!(as_dbfs < -90.0, "unflushed, the idle floor would be {as_dbfs:.1} dBFS");
        assert_eq!(attenuate_q24(largest_fixed_point, top), 0, "a multiply that cannot move a sample is the floor");
        assert_eq!(attenuate_q24(24i32, 1 << 24), 24, "at unity — which is what freeze sets — nothing is flushed");
        assert_eq!(attenuate_q24(0.25f32, top), 0.25f32.mul_q24(top), "the float path is untouched");
    }

    /// Research point 1's actual question: after the tail has gone, is the bus silent?
    #[test]
    fn the_idle_noise_floor_after_the_tail_is_exactly_zero() {
        // The top of the room-size knob, where the fixed points are largest.
        let mut reverb: Reverb<i32> = configured(100, 0, 100);
        let excitation = impulse_input::<i32>(64 * DSP_BLOCK_FRAMES);
        let _ = render_stereo(&mut reverb, &excitation);
        // Sixty seconds of silence is far past any tail a 0.98 feedback can hold.
        let frames = 60 * RATE as usize;
        let quiet = render_stereo(&mut reverb, &vec![Stereo::new(0i32, 0); frames]);
        let tail = quiet.get(quiet.len() - DSP_BLOCK_FRAMES..).expect("a last block");
        let floor = tail.iter().map(|frame| frame.left.abs().max(frame.right.abs())).max().unwrap_or(0);
        assert_eq!(floor, 0, "the idle floor is {floor} units, not silence");
    }

    #[test]
    fn a_reverb_at_zero_mix_is_bit_transparent() {
        let mut reverb: Reverb<i32> = Reverb::new(RATE);
        Insert::<i32>::set_param(&mut reverb, REVERB_MIX_PARAM, 0);
        Insert::<i32>::reset(&mut reverb);
        let mut samples = block_of(|index| ((index as i32 * 977) % 20_001) - 10_000);
        let original = samples.clone();
        Insert::<i32>::process(&mut reverb, &mut samples);
        assert_eq!(samples, original, "a fully dry reverb changed the block");
    }

    #[test]
    fn the_fixed_and_float_paths_agree_on_a_drum_loop() {
        let loop_frames = 32 * DSP_BLOCK_FRAMES;
        let fixed_input = drum_loop(loop_frames);
        let float_input: Vec<f32> = fixed_input.iter().map(|value| *value as f32).collect();
        let mut fixed: Reverb<i32> = configured(50, 50, 50);
        let mut float: Reverb<f32> = configured(50, 50, 50);
        let fixed_output = render_noise(&mut fixed, &fixed_input);
        let float_output = render_noise(&mut float, &float_input);
        let snr = segmental_snr_db(&fixed_output, &float_output).expect("the render is not silent");
        assert!(snr >= 50.0, "the two paths agree at only {snr:.1} dB");
    }

    #[test]
    fn the_width_matrix_crosses_the_channels_at_zero_and_leaves_them_alone_at_a_hundred() {
        assert_eq!(width_gains_q15(100), (Q15_UNITY, 0));
        assert_eq!(width_gains_q15(0), (Q15_UNITY / 2, Q15_UNITY / 2));

        let render = |width: i32| {
            let mut reverb: Reverb<i32> = configured(50, 50, 100);
            Insert::<i32>::set_param(&mut reverb, REVERB_WIDTH_PARAM, width);
            Insert::<i32>::reset(&mut reverb);
            // Excite one channel only, so the two tanks carry different signals.
            let input: Vec<Stereo<i32>> = (0..16 * DSP_BLOCK_FRAMES).map(|index| Stereo::new(if index == 0 { IMPULSE } else { 0 }, 0)).collect();
            render_stereo(&mut reverb, &input)
        };
        let narrow = render(0);
        assert!(narrow.iter().all(|frame| frame.left == frame.right), "at width 0 the wet signal is mono");
        let wide = render(100);
        assert!(wide.iter().any(|frame| frame.left != frame.right), "at width 100 the two tanks stay apart");
    }

    /// The room-size and damping knobs land exactly on the endpoints the deliverable names.
    #[test]
    fn the_knobs_reach_the_coefficients_the_design_states() {
        assert_eq!(room_feedback_q24(0), ROOM_MIN_Q24);
        assert_eq!(room_feedback_q24(100), ROOM_MIN_Q24 + ROOM_SPAN_Q24);
        assert!((room_feedback_q24(100) as f64 / (1 << 24) as f64 - 0.98).abs() < 1e-6);
        assert!((room_feedback_q24(0) as f64 / (1 << 24) as f64 - 0.7).abs() < 1e-6);
        assert_eq!(damping_coefficient_q24(0), 0, "no damping is exactly a pass-through of the delayed sample");
        assert_eq!(damping_coefficient_q24(100), DAMPING_MAX_Q24);
        assert!((INPUT_GAIN_Q24 as f64 / (1 << 24) as f64 / (1 << REVERB_HEADROOM_BITS) as f64 - 0.015).abs() < 1e-9);
        assert_eq!(WET_SCALE_Q24, ((3 * 8) << 24) >> REVERB_HEADROOM_BITS, "the wet scale is Jezar's 3, the comb sum's 8 and the headroom's 64");
    }

    /// Freeverb's tunings, scaled — and the right channel offset that gives the two tanks
    /// their difference.
    #[test]
    fn the_line_lengths_scale_with_the_sample_rate() {
        let at_44100: Reverb<i32> = Reverb::new(44_100);
        assert_eq!(at_44100.left.combs.first().map(|comb| comb.length), Some(1_116), "at the tuning rate the lengths are Jezar's own");
        assert_eq!(at_44100.right.combs.first().map(|comb| comb.length), Some(1_116 + STEREO_SPREAD));
        let at_96000: Reverb<i32> = Reverb::new(96_000);
        assert_eq!(at_96000.left.combs.first().map(|comb| comb.length), Some(2_429), "1116 × 96000/44100");
        let at_8000: Reverb<i32> = Reverb::new(8_000);
        assert!(at_8000.left.combs.iter().all(|comb| comb.length >= 1), "no line may be shorter than a frame");
    }

    /// Deliverable 1 asks for the total at 48 kHz stereo, and for a
    /// `DelayLine::with_exact_capacity` if it exceeds 256 KB. It does not.
    #[test]
    fn the_memory_one_instance_costs_is_the_number_the_task_asked_for() {
        let reverb: Reverb<i32> = Reverb::new(48_000);
        let bytes = reverb.capacity_frames() * core::mem::size_of::<i32>();
        assert_eq!(bytes, 216_064, "the measured total at 48 kHz, in bytes");
        assert!(bytes <= 256 * 1_024, "{bytes} bytes is past the budget a conditional-subtract wrap would have to buy back");
        let float: Reverb<f32> = Reverb::new(48_000);
        assert_eq!(float.capacity_frames(), reverb.capacity_frames(), "both paths hold the same frames");
    }

    #[test]
    fn a_reset_clears_the_tanks() {
        let mut reverb: Reverb<i32> = configured(50, 50, 100);
        let _ = render_stereo(&mut reverb, &impulse_input::<i32>(4 * DSP_BLOCK_FRAMES));
        Insert::<i32>::reset(&mut reverb);
        let quiet = render_stereo(&mut reverb, &vec![Stereo::new(0i32, 0); 4 * DSP_BLOCK_FRAMES]);
        assert!(quiet.iter().all(|frame| frame.left == 0 && frame.right == 0), "a reset reverb still had a tail");
    }

    #[test]
    fn a_pre_delay_holds_the_wet_signal_back() {
        let mut reverb: Reverb<i32> = configured(50, 0, 100);
        Insert::<i32>::set_param(&mut reverb, REVERB_PRE_DELAY_PARAM, 50);
        Insert::<i32>::reset(&mut reverb);
        let output = render_stereo(&mut reverb, &impulse_input::<i32>(64 * DSP_BLOCK_FRAMES));
        let pre_delay_frames = (50 * RATE as usize) / 1_000;
        let before = output.get(..pre_delay_frames).expect("the render covers the pre-delay");
        assert!(before.iter().all(|frame| frame.left == 0), "there is wet signal before the pre-delay has run out");
        let after = output.get(pre_delay_frames..).expect("in range");
        assert!(after.iter().any(|frame| frame.left != 0), "nothing arrived after the pre-delay");
    }

    #[test]
    fn a_boxed_reverb_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let boxed: alloc::boxed::Box<dyn Insert<f32>> = alloc::boxed::Box::new(Reverb::<f32>::new(RATE));
        assert_send(&boxed);
    }
}
