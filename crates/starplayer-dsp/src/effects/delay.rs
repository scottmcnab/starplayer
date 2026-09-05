//! [`Delay`] — a stereo delay with damped feedback and an optional ping-pong (H3
//! deliverable 2).
//!
//! # The time is smoothed in frames, not in milliseconds
//!
//! [`SmoothedParam`]'s own documentation says the unit is the caller's, and a delay is the
//! case where the parameter's *host* unit is the wrong one to smooth in. One millisecond
//! is forty-four frames at 44.1 kHz, so a millisecond-resolution ramp moves the read head
//! in forty-four-frame jumps — the crackle smoothing exists to prevent. The host-facing
//! parameter is therefore whole milliseconds ([`Insert::param`] hands back exactly what
//! was set), while the value that ramps is the delay in **Q8 frames**: 1/256 of a frame of
//! resolution, read back through [`DelayLine::read_fractional`]. A time sweep then slides
//! the read head continuously through the buffer and pitch-shifts like tape, which is what
//! a delay is expected to do.
//!
//! Q8 rather than Q16 because the maximum is [`DELAY_MAX_MS`] — 2000 ms, 88,200 frames at
//! 44.1 kHz — and `88_200 << 16` does not fit an `i32` while `88_200 << 8` fits with three
//! orders of magnitude to spare.
//!
//! # Read before write
//!
//! The feedback path needs the delayed sample to decide what to write, so each frame reads
//! the line first and writes afterwards. [`DelayLine::read`]`(0)` is the most recently
//! written frame, so a delay of `D` frames reads at `D − 1` — spelled out in
//! [`Delay::delay_index_q16`] rather than left as an off-by-one to rediscover.
//!
//! # Research point 2: why the fixed path cannot run away
//!
//! Three independent bounds, any one of which would do:
//!
//! 1. **The feedback gain is clamped below unity.** 100 % on the knob is
//!    [`FEEDBACK_MAX_Q15`], 0.95, not 1.0. A tail therefore loses 0.45 dB per repeat.
//! 2. **What is written to the line is saturated** to the path's full scale
//!    ([`DspSample::saturate`], ±32767 on the fixed path), so however hot the input, the
//!    line's contents are bounded by construction and the loop gain is applied to a
//!    bounded value.
//! 3. **A multiply that does not move the sample flushes to silence.**
//!    [`DspSample::scale_q15`] rounds to nearest with ties away from zero, so on the fixed
//!    path every gain at or above 0.5 has small fixed points — `v = 10` at 0.95, `v = 1` at
//!    any gain ≥ 0.5 — which would ring at −70 dBFS for ever rather than decaying.
//!    [`attenuate`] treats "the feedback gain left this sample exactly where it was" as
//!    proof that the tail has reached the arithmetic's floor and zeroes it. On the float
//!    path a below-unity multiply leaves only zero unchanged, so it is a no-op there.
//!
//! # The damping filter's top is *off*
//!
//! [`crate::tables::one_pole_cutoff_q24`] returns exact unity at or above Nyquist, and
//! [`Delay::damping_q24_at`] returns it at [`DAMPING_OFF_HZ`] whatever the sample rate, so
//! the top of the cutoff knob is an undamped delay whose echoes are the input unaltered. That is what
//! makes an impulse's echo train exactly `1, f, f²` — a one-pole low-pass spreads an
//! impulse and lowers its peak, so a delay with damping always on could not be measured
//! against those numbers at all.

use crate::delay_line::StereoDelayLine;
use crate::frame::Stereo;
use crate::insert::{DSP_BLOCK_FRAMES, Insert, InsertDescriptor, ParamId, ParamSpec, ParamUnit};
use crate::sample::{DspSample, Q15_UNITY};
use crate::smooth::{SMOOTH_FRAMES, SmoothedParam};
use crate::tables::{equal_power_q15, one_pole_cutoff_q24};

/// The shortest delay a host may ask for, in milliseconds.
pub const DELAY_MIN_MS: i32 = 1;

/// The longest delay a host may ask for, in milliseconds. The delay line is sized for this
/// at [`Delay::new`], which is why it is a constant rather than a parameter.
pub const DELAY_MAX_MS: i32 = 2_000;

/// The feedback gain 100 % on the knob resolves to, in Q1.15: 0.95, deliberately short of
/// unity — see "Research point 2" in the module documentation.
pub const FEEDBACK_MAX_Q15: i32 = 31_130;

/// The lowest damping cutoff, in hertz.
pub const DAMPING_MIN_HZ: i32 = 200;

/// The top of the damping knob, in hertz, which means **no damping at all** rather than
/// "a 20 kHz low-pass" — see the module documentation.
pub const DAMPING_OFF_HZ: i32 = 20_000;

/// Delay time, in whole milliseconds.
pub const DELAY_TIME_PARAM: ParamId = ParamId(0);
/// Feedback, in percent.
pub const DELAY_FEEDBACK_PARAM: ParamId = ParamId(1);
/// Wet/dry mix, in percent.
pub const DELAY_MIX_PARAM: ParamId = ParamId(2);
/// Ping-pong on or off.
pub const DELAY_PING_PONG_PARAM: ParamId = ParamId(3);
/// The feedback path's low-pass cutoff, in hertz.
pub const DELAY_DAMPING_PARAM: ParamId = ParamId(4);

/// What a host draws for a [`Delay`].
pub static DELAY_DESCRIPTOR: InsertDescriptor = InsertDescriptor {
    name: "delay",
    params: &[
        ParamSpec { name: "time", unit: ParamUnit::Milliseconds, min: DELAY_MIN_MS, max: DELAY_MAX_MS, default: 300 },
        ParamSpec { name: "feedback", unit: ParamUnit::Percent, min: 0, max: 100, default: 35 },
        ParamSpec { name: "mix", unit: ParamUnit::Percent, min: 0, max: 100, default: 30 },
        ParamSpec { name: "ping_pong", unit: ParamUnit::Switch, min: 0, max: 1, default: 0 },
        ParamSpec { name: "damping", unit: ParamUnit::Hertz, min: DAMPING_MIN_HZ, max: DAMPING_OFF_HZ, default: 6_000 },
    ],
};

/// A stereo delay: one line per channel, damped feedback, equal-power wet/dry.
#[derive(Clone, Debug)]
pub struct Delay<Sample: DspSample> {
    sample_rate_hz: u32,
    line: StereoDelayLine<Sample>,
    /// The largest Q8-frame delay the line can serve, leaving room for the fractional read
    /// to look one frame further back.
    max_delay_q8: i32,
    time_ms: i32,
    time_q8: SmoothedParam,
    feedback_percent: SmoothedParam,
    mix_percent: SmoothedParam,
    ping_pong: bool,
    damping_hz: SmoothedParam,
    /// Cooked once per block from `damping_hz`, like the EQ's coefficients.
    damping_q24: i32,
    damping_state_left: Sample,
    damping_state_right: Sample,
}

impl<Sample: DspSample> Delay<Sample> {
    /// A delay at this sample rate, with a line long enough for [`DELAY_MAX_MS`].
    ///
    /// **This allocates** — two rings of `2 × sample_rate` frames, rounded up to a power of
    /// two — which is exactly why an effect is built on the control thread and crosses to
    /// the engine boxed (M7 decision 4).
    pub fn new(sample_rate_hz: u32) -> Delay<Sample> {
        let sample_rate_hz = sample_rate_hz.max(1);
        // Two frames of headroom: the fractional read looks one frame further back than the
        // whole part, and the index is one less than the delay.
        let capacity_frames = (milliseconds_to_q8_frames(DELAY_MAX_MS, sample_rate_hz) / 256) as usize + 2;
        let line = StereoDelayLine::new(capacity_frames);
        let max_delay_q8 = (((line.left.capacity() - 2) as i64) * 256).min(i32::MAX as i64) as i32;
        let time_ms = default_at(0);
        let mut delay = Delay {
            sample_rate_hz,
            line,
            max_delay_q8,
            time_ms,
            time_q8: SmoothedParam::steady(0),
            feedback_percent: SmoothedParam::steady(default_at(1)),
            mix_percent: SmoothedParam::steady(default_at(2)),
            ping_pong: default_at(3) != 0,
            damping_hz: SmoothedParam::steady(default_at(4)),
            damping_q24: 0,
            damping_state_left: Sample::ZERO,
            damping_state_right: Sample::ZERO,
        };
        delay.time_q8 = SmoothedParam::steady(delay.time_to_q8(time_ms));
        delay.damping_q24 = delay.damping_q24_at(delay.damping_hz.current());
        delay
    }

    /// Milliseconds to the Q8-frame delay the line is read at, bounded by its capacity.
    fn time_to_q8(&self, milliseconds: i32) -> i32 {
        let frames_q8 = milliseconds_to_q8_frames(milliseconds.clamp(DELAY_MIN_MS, DELAY_MAX_MS), self.sample_rate_hz);
        frames_q8.clamp(256, self.max_delay_q8 as i64) as i32
    }

    /// The damping coefficient for a cutoff, with the top of the knob meaning *off*.
    ///
    /// [`crate::tables::one_pole_cutoff_q24`] switches the filter off at Nyquist, which at
    /// 96 kHz is 48 kHz — well above anything a host can ask for. The top of *this* knob
    /// has to mean off at every sample rate, or "no damping" would only be reachable below
    /// 40 kHz, so [`DAMPING_OFF_HZ`] is special-cased here rather than in the table.
    fn damping_q24_at(&self, cutoff_hz: i32) -> i32 {
        if cutoff_hz >= DAMPING_OFF_HZ { 1 << 24 } else { one_pole_cutoff_q24(cutoff_hz, self.sample_rate_hz) }
    }

    /// The Q16.16 index [`DelayLine::read_fractional`](crate::delay_line::DelayLine::read_fractional)
    /// is called with for a delay of `delay_q8` Q8 frames: `read(0)` is the frame written
    /// last, and this frame's write has not happened yet, so a `D`-frame delay reads at
    /// `D − 1`.
    fn delay_index_q16(&self, delay_q8: i32) -> u32 {
        let clamped = delay_q8.clamp(256, self.max_delay_q8);
        (((clamped as i64) << 8) - (1 << 16)).max(0) as u32
    }
}

/// `milliseconds` as Q8 frames at `sample_rate_hz`, rounded to nearest.
///
/// Computed from the two inputs each time rather than from a precomputed
/// frames-per-millisecond constant, because rounding that constant first would put a
/// common delay time a fraction of a frame off its exact position: at 44.1 kHz,
/// `round(44100 × 256 / 1000) = 11290` makes 300 ms 13230.47 frames rather than the exact
/// 13230, which spreads an echo across two frames instead of landing it on one.
fn milliseconds_to_q8_frames(milliseconds: i32, sample_rate_hz: u32) -> i64 {
    (milliseconds.max(0) as i64 * sample_rate_hz as i64 * 256 + 500) / 1_000
}

/// The descriptor's default for one parameter position.
fn default_at(index: usize) -> i32 { DELAY_DESCRIPTOR.params.get(index).map_or(0, |spec| spec.default) }

/// The descriptor's clamp for one parameter position.
fn clamp_at(index: usize, value: i32) -> i32 { DELAY_DESCRIPTOR.params.get(index).map_or(value, |spec| spec.clamp(value)) }

/// A feedback percentage as a Q1.15 gain: linear in the knob, clamped to
/// [`FEEDBACK_MAX_Q15`], so 50 % is exactly one half and 100 % is 0.95.
fn feedback_q15(percent: i32) -> i32 {
    let clamped = percent.clamp(0, 100) as i64;
    ((clamped * Q15_UNITY as i64) / 100).min(FEEDBACK_MAX_Q15 as i64) as i32
}

/// `value × gain`, with the fixed path's rounding fixed points flushed to silence — see
/// "Research point 2" in the module documentation.
fn attenuate<Sample: DspSample>(value: Sample, gain_q15: i32) -> Sample {
    let scaled = value.scale_q15(gain_q15);
    if gain_q15 < Q15_UNITY && scaled == value { Sample::ZERO } else { scaled }
}

/// One step of `y[n] = y[n−1] + a·(x[n] − y[n−1])`. `a` at or above unity is a
/// pass-through, which is how [`DAMPING_OFF_HZ`] switches the filter off entirely.
fn one_pole<Sample: DspSample>(state: &mut Sample, input: Sample, coefficient_q24: i32) -> Sample {
    if coefficient_q24 >= 1 << 24 {
        *state = input;
        return input;
    }
    let next = state.add(input.sub(*state).mul_q24(coefficient_q24));
    *state = next;
    next
}

impl<Sample: DspSample> Insert<Sample> for Delay<Sample> {
    fn process(&mut self, block: &mut [Stereo<Sample>]) {
        debug_assert_eq!(block.len(), DSP_BLOCK_FRAMES, "an insert only ever sees a whole DSP block");
        // A coefficient cooks once per block; a gain and the read position are per frame.
        // Same division of labour as the EQ, and for the same reason.
        if self.damping_hz.is_moving() {
            for _ in 0..DSP_BLOCK_FRAMES {
                self.damping_hz.advance();
            }
        }
        let damping_q24 = self.damping_q24_at(self.damping_hz.current());
        self.damping_q24 = damping_q24;

        for frame in block.iter_mut() {
            let delay_q8 = self.time_q8.advance();
            let index_q16 = self.delay_index_q16(delay_q8);
            // Both channels read at the same position, so the two taps are two lanes of
            // one `interpolate_taps` call (M7-H6); the remaining lanes stay silent.
            let (left_current, left_next, fraction) = self.line.left.read_fractional_parts(index_q16);
            let (right_current, right_next, _) = self.line.right.read_fractional_parts(index_q16);
            let taps = Sample::interpolate_taps(
                [left_current, right_current, Sample::ZERO, Sample::ZERO],
                [left_next, right_next, Sample::ZERO, Sample::ZERO],
                [fraction, fraction, 0, 0],
            );
            let delayed_left = taps.first().copied().unwrap_or(Sample::ZERO);
            let delayed_right = taps.get(1).copied().unwrap_or(Sample::ZERO);

            let damped_left = one_pole(&mut self.damping_state_left, delayed_left, damping_q24);
            let damped_right = one_pole(&mut self.damping_state_right, delayed_right, damping_q24);
            let (into_left, into_right) = if self.ping_pong { (damped_right, damped_left) } else { (damped_left, damped_right) };

            let feedback = feedback_q15(self.feedback_percent.advance());
            let write_left = frame.left.add(attenuate(into_left, feedback)).saturate();
            let write_right = frame.right.add(attenuate(into_right, feedback)).saturate();
            self.line.write(write_left, write_right);

            let (dry, wet) = equal_power_q15(self.mix_percent.advance());
            frame.left = frame.left.scale_q15(dry).add(delayed_left.scale_q15(wet));
            frame.right = frame.right.scale_q15(dry).add(delayed_right.scale_q15(wet));
        }
    }

    fn set_param(&mut self, id: ParamId, value: i32) {
        match id {
            DELAY_TIME_PARAM => {
                self.time_ms = clamp_at(0, value);
                let target = self.time_to_q8(self.time_ms);
                self.time_q8.set_target(target, SMOOTH_FRAMES);
            }
            DELAY_FEEDBACK_PARAM => self.feedback_percent.set_target(clamp_at(1, value), SMOOTH_FRAMES),
            DELAY_MIX_PARAM => self.mix_percent.set_target(clamp_at(2, value), SMOOTH_FRAMES),
            DELAY_PING_PONG_PARAM => self.ping_pong = clamp_at(3, value) != 0,
            DELAY_DAMPING_PARAM => self.damping_hz.set_target(clamp_at(4, value), SMOOTH_FRAMES),
            _ => {}
        }
    }

    fn param(&self, id: ParamId) -> Option<i32> {
        match id {
            DELAY_TIME_PARAM => Some(self.time_ms),
            DELAY_FEEDBACK_PARAM => Some(self.feedback_percent.target()),
            DELAY_MIX_PARAM => Some(self.mix_percent.target()),
            DELAY_PING_PONG_PARAM => Some(i32::from(self.ping_pong)),
            DELAY_DAMPING_PARAM => Some(self.damping_hz.target()),
            _ => None,
        }
    }

    fn reset(&mut self) {
        self.line.reset();
        self.damping_state_left = Sample::ZERO;
        self.damping_state_right = Sample::ZERO;
        self.time_q8.snap();
        self.feedback_percent.snap();
        self.mix_percent.snap();
        self.damping_hz.snap();
        self.damping_q24 = self.damping_q24_at(self.damping_hz.current());
    }

    fn descriptor(&self) -> &'static InsertDescriptor { &DELAY_DESCRIPTOR }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::testing::{RATE, block_of, render_noise, render_stereo, segmental_snr_db, white_noise};
    use crate::insert::assert_descriptor_roundtrip;
    use alloc::vec;
    use alloc::vec::Vec;

    /// The echo test's delay: 300 ms.
    const ECHO_MS: i32 = 300;

    /// The impulse amplitude, on the raw `i16` scale.
    const IMPULSE: i32 = 20_000;

    /// Frames the echo test renders: enough for four repeats of a 300 ms delay.
    const ECHO_FRAMES: usize = 320 * DSP_BLOCK_FRAMES;

    fn configured<Sample: DspSample>(ping_pong: bool) -> Delay<Sample> {
        let mut delay: Delay<Sample> = Delay::new(RATE);
        Insert::<Sample>::set_param(&mut delay, DELAY_TIME_PARAM, ECHO_MS);
        Insert::<Sample>::set_param(&mut delay, DELAY_FEEDBACK_PARAM, 50);
        Insert::<Sample>::set_param(&mut delay, DELAY_MIX_PARAM, 100);
        Insert::<Sample>::set_param(&mut delay, DELAY_DAMPING_PARAM, DAMPING_OFF_HZ);
        Insert::<Sample>::set_param(&mut delay, DELAY_PING_PONG_PARAM, i32::from(ping_pong));
        Insert::<Sample>::reset(&mut delay);
        delay
    }

    /// A stereo impulse train input: one impulse on frame zero, silence afterwards.
    fn impulse_input(frames: usize, left: bool, right: bool) -> Vec<Stereo<i32>> {
        (0..frames)
            .map(|index| {
                let value = if index == 0 { IMPULSE } else { 0 };
                Stereo::new(if left { value } else { 0 }, if right { value } else { 0 })
            })
            .collect()
    }

    #[test]
    fn descriptor_roundtrip() {
        let mut delay: Delay<i32> = Delay::new(RATE);
        assert_descriptor_roundtrip(&mut delay, "delay");
        let mut delay: Delay<f32> = Delay::new(RATE);
        assert_descriptor_roundtrip(&mut delay, "delay");
    }

    /// The audibility proof (H3 deliverable 5): 300 ms / 50 % / 100 % wet produces echoes
    /// at 300, 600 and 900 ms with amplitudes 1, ½ and ¼ of the impulse.
    #[test]
    fn an_impulse_comes_back_at_every_multiple_of_the_delay_halving_each_time() {
        let mut delay: Delay<i32> = configured(false);
        let output = render_stereo(&mut delay, &impulse_input(ECHO_FRAMES, true, true));
        let step = (ECHO_MS as f64 * RATE as f64 / 1_000.0).round() as usize;
        for (repeat, expected) in [(1usize, IMPULSE), (2, IMPULSE / 2), (3, IMPULSE / 4)] {
            let frame = output.get(repeat * step).expect("the render covers three repeats");
            assert!((frame.left - expected).abs() <= 1, "repeat {repeat} came back at {} not {expected}", frame.left);
            assert!((frame.right - expected).abs() <= 1, "repeat {repeat} right came back at {}", frame.right);
        }
        // And nowhere else: the frame before each echo is silent.
        for repeat in 1..=3usize {
            let quiet = output.get(repeat * step - 8).expect("in range");
            assert_eq!(quiet.left, 0, "there is signal {} frames before repeat {repeat}", 8);
        }
    }

    #[test]
    fn ping_pong_alternates_the_channels() {
        let mut delay: Delay<i32> = configured(true);
        let output = render_stereo(&mut delay, &impulse_input(ECHO_FRAMES, true, false));
        let step = (ECHO_MS as f64 * RATE as f64 / 1_000.0).round() as usize;
        let first = output.get(step).expect("in range");
        let second = output.get(2 * step).expect("in range");
        let third = output.get(3 * step).expect("in range");
        assert!(first.left > 0 && first.right == 0, "the first repeat is on the left: {first:?}");
        assert!(second.right > 0 && second.left == 0, "the second repeat crossed to the right: {second:?}");
        assert!(third.left > 0 && third.right == 0, "the third repeat crossed back: {third:?}");
        assert!((second.right - IMPULSE / 2).abs() <= 1, "the crossed repeat kept its amplitude: {second:?}");
    }

    /// Research point 2, measured rather than asserted: at 100 % feedback the tail decays
    /// and reaches exact silence, rather than ringing for ever.
    #[test]
    fn full_feedback_settles_to_silence_on_the_fixed_path() {
        let mut delay: Delay<i32> = Delay::new(RATE);
        // A short time so the whole decay fits in a reasonable render.
        Insert::<i32>::set_param(&mut delay, DELAY_TIME_PARAM, 1);
        Insert::<i32>::set_param(&mut delay, DELAY_FEEDBACK_PARAM, 100);
        Insert::<i32>::set_param(&mut delay, DELAY_MIX_PARAM, 100);
        Insert::<i32>::set_param(&mut delay, DELAY_DAMPING_PARAM, DAMPING_OFF_HZ);
        Insert::<i32>::reset(&mut delay);

        let frames = 256 * DSP_BLOCK_FRAMES;
        let output = render_stereo(&mut delay, &impulse_input(frames, true, true));
        let tail = output.len() - DSP_BLOCK_FRAMES;
        for frame in output.get(tail..).expect("a tail").iter() {
            assert_eq!(frame.left, 0, "the tail had not reached silence");
        }
        let peak_after_half = output.get(output.len() / 2..).expect("half").iter().map(|frame| frame.left.abs()).max().unwrap_or(0);
        assert!(peak_after_half < IMPULSE / 4, "half way through the render the tail is still at {peak_after_half}");
    }

    #[test]
    fn a_delay_at_zero_mix_is_bit_transparent() {
        let mut delay: Delay<i32> = Delay::new(RATE);
        Insert::<i32>::set_param(&mut delay, DELAY_MIX_PARAM, 0);
        Insert::<i32>::reset(&mut delay);
        let mut samples = block_of(|index| ((index as i32 * 977) % 20_001) - 10_000);
        let original = samples.clone();
        Insert::<i32>::process(&mut delay, &mut samples);
        assert_eq!(samples, original, "a fully dry delay changed the block");
    }

    #[test]
    fn a_time_sweep_slides_the_read_head_rather_than_jumping_it() {
        let mut delay: Delay<i32> = Delay::new(RATE);
        Insert::<i32>::set_param(&mut delay, DELAY_TIME_PARAM, 100);
        Insert::<i32>::reset(&mut delay);
        let before = delay.time_q8.current();
        Insert::<i32>::set_param(&mut delay, DELAY_TIME_PARAM, 101);
        let mut moved = Vec::new();
        for _ in 0..SMOOTH_FRAMES {
            moved.push(delay.time_q8.advance());
        }
        let after = delay.time_q8.current();
        assert_eq!(after - before, (milliseconds_to_q8_frames(101, RATE) - milliseconds_to_q8_frames(100, RATE)) as i32, "one millisecond is one millisecond of frames");
        let largest = moved.windows(2).map(|pair| pair.get(1).copied().unwrap_or(0) - pair.first().copied().unwrap_or(0)).max().unwrap_or(0);
        assert!(largest <= 256, "the read head jumped {largest} Q8 frames — more than one whole frame — in a single frame");
    }

    #[test]
    fn the_fixed_and_float_paths_agree_to_better_than_sixty_decibels() {
        let noise: Vec<i32> = white_noise(32 * DSP_BLOCK_FRAMES, 8_000);
        let mut fixed: Delay<i32> = Delay::new(RATE);
        let mut float: Delay<f32> = Delay::new(RATE);
        for (id, value) in [(DELAY_TIME_PARAM, 37), (DELAY_FEEDBACK_PARAM, 60), (DELAY_MIX_PARAM, 50), (DELAY_DAMPING_PARAM, 4_000)] {
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

    #[test]
    fn damping_at_the_top_of_its_range_is_off() {
        assert_eq!(one_pole_cutoff_q24(DAMPING_OFF_HZ, 30_000), 1 << 24, "past Nyquist the table itself switches off");
        for rate in [22_050u32, 44_100, 48_000, 96_000, 192_000] {
            let delay: Delay<i32> = Delay::new(rate);
            assert_eq!(delay.damping_q24_at(DAMPING_OFF_HZ), 1 << 24, "the top of the knob must be off at {rate} Hz too");
            // One hertz down is a real filter wherever Nyquist leaves room for one; at
            // 22.05 kHz the whole top of the knob is already past it.
            let expected_off = (DAMPING_OFF_HZ - 1) * 2 >= rate as i32;
            assert_eq!(delay.damping_q24_at(DAMPING_OFF_HZ - 1) == 1 << 24, expected_off, "one hertz down at {rate} Hz");
        }
        let mut state = 1_000i32;
        assert_eq!(one_pole(&mut state, 500, 1 << 24), 500);
        assert_eq!(state, 500, "an off filter tracks its input exactly");
    }

    #[test]
    fn the_feedback_knob_is_linear_below_its_clamp() {
        assert_eq!(feedback_q15(0), 0);
        assert_eq!(feedback_q15(50), Q15_UNITY / 2, "50 % has to be exactly one half for the echo train to halve");
        assert_eq!(feedback_q15(100), FEEDBACK_MAX_Q15, "the top of the knob is clamped below unity");
        assert!(feedback_q15(99) < FEEDBACK_MAX_Q15 || feedback_q15(99) == FEEDBACK_MAX_Q15);
    }

    #[test]
    fn attenuation_flushes_the_fixed_paths_rounding_fixed_points() {
        // 10 x 0.95 rounds back to 10 — the ring that never decays.
        assert_eq!(10i32.scale_q15(FEEDBACK_MAX_Q15), 10, "the premise of the flush");
        assert_eq!(attenuate(10i32, FEEDBACK_MAX_Q15), 0, "a sample the gain cannot move is the floor");
        assert_eq!(attenuate(1_000i32, FEEDBACK_MAX_Q15), 950);
        assert_eq!(attenuate(0.25f32, FEEDBACK_MAX_Q15), 0.25f32.scale_q15(FEEDBACK_MAX_Q15), "the float path is untouched");
    }

    #[test]
    fn a_reset_clears_the_line() {
        let mut delay: Delay<i32> = configured(false);
        let _ = render_stereo(&mut delay, &impulse_input(4 * DSP_BLOCK_FRAMES, true, true));
        Insert::<i32>::reset(&mut delay);
        let quiet = render_stereo(&mut delay, &vec![Stereo::new(0i32, 0); 4 * DSP_BLOCK_FRAMES]);
        assert!(quiet.iter().all(|frame| frame.left == 0 && frame.right == 0), "a reset delay still had a tail");
    }

    #[test]
    fn a_boxed_delay_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let boxed: alloc::boxed::Box<dyn Insert<f32>> = alloc::boxed::Box::new(Delay::<f32>::new(RATE));
        assert_send(&boxed);
    }
}
