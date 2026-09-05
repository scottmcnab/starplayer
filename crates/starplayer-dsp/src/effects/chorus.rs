//! [`Chorus`] — two or three modulated taps per channel (H3 deliverable 3).
//!
//! # Research point 3: what makes it a chorus rather than a flanger
//!
//! The two effects are the same signal path — a delay of a few milliseconds, swept by an
//! LFO, summed with the dry — and are told apart entirely by where the numbers sit:
//!
//! | | flanger | chorus | this effect's defaults |
//! |---|---|---|---|
//! | base delay | 0.5–5 ms | 15–35 ms | **15 ms** ([`CHORUS_BASE_DEFAULT`]) |
//! | depth | up to the base | 1–5 ms | **2 ms** |
//! | rate | 0.05–2 Hz | 0.1–2 Hz | **0.6 Hz** |
//! | feedback | yes, often high | none | **none** |
//! | taps | one | two or more | **three** |
//!
//! A base delay under about 10 ms puts the dry and the delayed copy close enough together
//! that their sum is a comb filter whose notches sweep audibly through the spectrum — the
//! flanger's jet whoosh. Past about 15 ms the ear stops hearing a comb and starts hearing a
//! second, slightly detuned voice, which is the chorus. So the default base is 15 ms, and
//! the *range* starts at 1 ms so a host that wants a flanger can have one; what it cannot
//! have from this effect is feedback, which is the other half of a flanger and the thing
//! that would make the fixed path's stability an open question (see [`super::delay`]).
//!
//! **Three taps by default**, at LFO phases 0, ⅓ and ⅔ of a turn: a single tap detunes but
//! does not thicken, two taps in antiphase cross at the centre delay twice a cycle and comb
//! when they do, and three never coincide. Each tap is scaled by `1/voices` so the taps sum
//! to unity and the wet signal can never exceed the input's own peak.
//!
//! **The spread is the right channel's LFO phase offset**, 0–100 % of a *half* turn, so
//! 100 % puts the two channels in antiphase: when the left voice detunes sharp the right
//! detunes flat, which is what makes a chorus wide rather than merely doubled. The default
//! is 100 %.
//!
//! # Why it is a pure function of frames rendered
//!
//! [`Lfo`]'s phase is a `u32` advanced by a fixed increment once per frame, so however a
//! host splits its callbacks the phase after `n` frames is the same `u32`
//! (`crate::lfo`'s own module documentation makes the argument). The increment itself is
//! cooked once per [`DSP_BLOCK_FRAMES`] block from the smoothed rate — the same block-rate
//! cooking the EQ uses for its coefficients, and for the same reason: a rate change is a
//! 64-bit divide, and a block is a fixed 128 frames whatever the host asked for. Every
//! other parameter advances per frame.

use crate::delay_line::StereoDelayLine;
use crate::frame::Stereo;
use crate::insert::{DSP_BLOCK_FRAMES, Insert, InsertDescriptor, ParamId, ParamSpec, ParamUnit};
use crate::lfo::Lfo;
use crate::sample::{DspSample, Q15_UNITY};
use crate::simd::TAP_LANES;
use crate::smooth::{SMOOTH_FRAMES, SmoothedParam};
use crate::tables::{equal_power_q15, sin_q15};

/// The fewest modulated taps per channel.
pub const CHORUS_MIN_VOICES: i32 = 2;

/// The most modulated taps per channel.
pub const CHORUS_MAX_VOICES: i32 = 3;

/// The slowest LFO, in centi-hertz: 0.05 Hz.
pub const CHORUS_MIN_RATE_CENTI_HZ: i32 = 5;

/// The fastest LFO, in centi-hertz: 5 Hz. Past that it is a vibrato, not a chorus.
pub const CHORUS_MAX_RATE_CENTI_HZ: i32 = 500;

/// The shallowest sweep, in centi-milliseconds.
pub const CHORUS_MIN_DEPTH_CENTI_MS: i32 = 1;

/// The deepest sweep, in centi-milliseconds: 10 ms.
pub const CHORUS_MAX_DEPTH_CENTI_MS: i32 = 1_000;

/// The shortest base delay, in centi-milliseconds: 1 ms — flanger territory, offered
/// deliberately (see the module documentation).
pub const CHORUS_MIN_BASE_CENTI_MS: i32 = 100;

/// The longest base delay, in centi-milliseconds: 50 ms.
pub const CHORUS_MAX_BASE_CENTI_MS: i32 = 5_000;

/// The default base delay, in centi-milliseconds: 15 ms, the shortest delay that reads as a
/// second voice rather than as a comb filter.
pub const CHORUS_BASE_DEFAULT: i32 = 1_500;

/// How many taps per channel.
pub const CHORUS_VOICES_PARAM: ParamId = ParamId(0);
/// The LFO rate, in centi-hertz.
pub const CHORUS_RATE_PARAM: ParamId = ParamId(1);
/// The sweep depth, in centi-milliseconds.
pub const CHORUS_DEPTH_PARAM: ParamId = ParamId(2);
/// The base delay the sweep is centred on, in centi-milliseconds.
pub const CHORUS_BASE_PARAM: ParamId = ParamId(3);
/// Wet/dry mix, in percent.
pub const CHORUS_MIX_PARAM: ParamId = ParamId(4);
/// The right channel's LFO phase offset, in percent of a half turn.
pub const CHORUS_SPREAD_PARAM: ParamId = ParamId(5);

/// What a host draws for a [`Chorus`].
pub static CHORUS_DESCRIPTOR: InsertDescriptor = InsertDescriptor {
    name: "chorus",
    params: &[
        ParamSpec { name: "voices", unit: ParamUnit::Count, min: CHORUS_MIN_VOICES, max: CHORUS_MAX_VOICES, default: CHORUS_MAX_VOICES },
        ParamSpec { name: "rate", unit: ParamUnit::CentiHertz, min: CHORUS_MIN_RATE_CENTI_HZ, max: CHORUS_MAX_RATE_CENTI_HZ, default: 60 },
        ParamSpec { name: "depth", unit: ParamUnit::CentiMilliseconds, min: CHORUS_MIN_DEPTH_CENTI_MS, max: CHORUS_MAX_DEPTH_CENTI_MS, default: 200 },
        ParamSpec { name: "base", unit: ParamUnit::CentiMilliseconds, min: CHORUS_MIN_BASE_CENTI_MS, max: CHORUS_MAX_BASE_CENTI_MS, default: CHORUS_BASE_DEFAULT },
        ParamSpec { name: "mix", unit: ParamUnit::Percent, min: 0, max: 100, default: 50 },
        ParamSpec { name: "spread", unit: ParamUnit::Percent, min: 0, max: 100, default: 100 },
    ],
};

/// A modulated multi-tap delay: the chorus.
#[derive(Clone, Debug)]
pub struct Chorus<Sample: DspSample> {
    sample_rate_hz: u32,
    line: StereoDelayLine<Sample>,
    /// The largest Q16.16-frame delay the line can serve.
    max_delay_q16: i32,
    voices: i32,
    rate_centi_hz: SmoothedParam,
    depth_centi_ms: i32,
    base_centi_ms: i32,
    depth_q16: SmoothedParam,
    base_q16: SmoothedParam,
    mix_percent: SmoothedParam,
    spread_percent: SmoothedParam,
    lfo: Lfo,
}

impl<Sample: DspSample> Chorus<Sample> {
    /// A chorus at this sample rate, with a line long enough for the deepest sweep around
    /// the longest base delay. **This allocates** — see [`super::delay::Delay::new`].
    pub fn new(sample_rate_hz: u32) -> Chorus<Sample> {
        let sample_rate_hz = sample_rate_hz.max(1);
        let longest_centi_ms = CHORUS_MAX_BASE_CENTI_MS + CHORUS_MAX_DEPTH_CENTI_MS;
        let capacity_frames = (centi_milliseconds_to_q16_frames(longest_centi_ms, sample_rate_hz) >> 16) as usize + 2;
        let line = StereoDelayLine::new(capacity_frames);
        let max_delay_q16 = ((line.left.capacity() as i64 - 2) << 16).min(i32::MAX as i64) as i32;
        let depth_centi_ms = default_at(2);
        let base_centi_ms = default_at(3);
        let mut lfo = Lfo::new();
        lfo.set_rate(default_at(1).max(0) as u32, sample_rate_hz);
        Chorus {
            sample_rate_hz,
            line,
            max_delay_q16,
            voices: default_at(0),
            rate_centi_hz: SmoothedParam::steady(default_at(1)),
            depth_centi_ms,
            base_centi_ms,
            depth_q16: SmoothedParam::steady(centi_milliseconds_to_q16_frames(depth_centi_ms, sample_rate_hz) as i32),
            base_q16: SmoothedParam::steady(centi_milliseconds_to_q16_frames(base_centi_ms, sample_rate_hz) as i32),
            mix_percent: SmoothedParam::steady(default_at(4)),
            spread_percent: SmoothedParam::steady(default_at(5)),
            lfo,
        }
    }

    /// The tap gain: `1/voices` in Q1.15, so the taps sum to unity and the wet signal can
    /// never exceed the peak of what went in.
    fn tap_gain_q15(&self) -> i32 { Q15_UNITY / self.voices.clamp(CHORUS_MIN_VOICES, CHORUS_MAX_VOICES) }

    /// The phase offset of tap `index`: `index / voices` of a full turn.
    fn tap_offset(&self, index: i32) -> u32 {
        let voices = self.voices.clamp(CHORUS_MIN_VOICES, CHORUS_MAX_VOICES) as u64;
        ((index.max(0) as u64 * (1u64 << 32)) / voices) as u32
    }

    /// The read position for one tap, in Q16.16 frames: the base delay plus the depth times
    /// the LFO's sine at that tap's phase, bounded to a whole frame at the bottom and to the
    /// line's capacity at the top.
    fn tap_delay_q16(&self, base_q16: i32, depth_q16: i32, phase: u32) -> u32 {
        let swing = (depth_q16 as i64 * sin_q15(phase) as i64) >> 15;
        (base_q16 as i64 + swing).clamp(1 << 16, self.max_delay_q16 as i64) as u32
    }
}

/// `centi_milliseconds` as Q16.16 frames at `sample_rate_hz`, rounded to nearest.
///
/// The largest value this is called with — 60 ms at 192 kHz — is 11,520 frames, so the Q16
/// product stays four orders of magnitude inside `i32`; a delay whose maximum is measured
/// in seconds (`super::delay`) cannot use Q16 and uses Q8 instead.
fn centi_milliseconds_to_q16_frames(centi_milliseconds: i32, sample_rate_hz: u32) -> i64 {
    (centi_milliseconds.max(0) as i64 * sample_rate_hz as i64 * 65_536 + 50_000) / 100_000
}

/// The descriptor's default for one parameter position.
fn default_at(index: usize) -> i32 { CHORUS_DESCRIPTOR.params.get(index).map_or(0, |spec| spec.default) }

/// The descriptor's clamp for one parameter position.
fn clamp_at(index: usize, value: i32) -> i32 { CHORUS_DESCRIPTOR.params.get(index).map_or(value, |spec| spec.clamp(value)) }

/// A spread percentage as a phase offset: 100 % is half a turn, so the two channels are in
/// antiphase.
fn spread_offset(percent: i32) -> u32 { ((percent.clamp(0, 100) as u64 * (1u64 << 31)) / 100) as u32 }

impl<Sample: DspSample> Insert<Sample> for Chorus<Sample> {
    fn process(&mut self, block: &mut [Stereo<Sample>]) {
        debug_assert_eq!(block.len(), DSP_BLOCK_FRAMES, "an insert only ever sees a whole DSP block");
        if self.rate_centi_hz.is_moving() {
            for _ in 0..DSP_BLOCK_FRAMES {
                self.rate_centi_hz.advance();
            }
        }
        self.lfo.set_rate(self.rate_centi_hz.current().max(0) as u32, self.sample_rate_hz);

        let voices = self.voices.clamp(CHORUS_MIN_VOICES, CHORUS_MAX_VOICES);
        let tap_gain = self.tap_gain_q15();
        for frame in block.iter_mut() {
            let base_q16 = self.base_q16.advance();
            let depth_q16 = self.depth_q16.advance();
            let spread = spread_offset(self.spread_percent.advance());
            let phase = self.lfo.phase();
            self.lfo.advance();

            // Written first, so a tap's `read_fractional(D)` is the input `D` frames ago
            // with no off-by-one: a chorus has no feedback, so nothing needs the delayed
            // sample before the write the way `super::delay` does.
            self.line.write(frame.left, frame.right);

            // Every tap's two frames are gathered first — they sit at arbitrary
            // distances in the ring and no vector load can reach them — and the
            // interpolation itself goes through one `interpolate_taps` call per channel
            // (M7-H6). A chorus has at most `CHORUS_MAX_VOICES` taps, which is inside
            // `TAP_LANES`; the lanes past `voices` stay silent and are never summed.
            let mut left_current = [Sample::ZERO; TAP_LANES];
            let mut left_next = [Sample::ZERO; TAP_LANES];
            let mut left_fraction = [0i32; TAP_LANES];
            let mut right_current = [Sample::ZERO; TAP_LANES];
            let mut right_next = [Sample::ZERO; TAP_LANES];
            let mut right_fraction = [0i32; TAP_LANES];
            for tap in 0..voices {
                let lane = tap.max(0) as usize;
                let left_phase = phase.wrapping_add(self.tap_offset(tap));
                let right_phase = left_phase.wrapping_add(spread);
                let left_delay = self.tap_delay_q16(base_q16, depth_q16, left_phase);
                let right_delay = self.tap_delay_q16(base_q16, depth_q16, right_phase);
                let (current, next, fraction) = self.line.left.read_fractional_parts(left_delay);
                if let (Some(a), Some(b), Some(c)) = (left_current.get_mut(lane), left_next.get_mut(lane), left_fraction.get_mut(lane)) {
                    (*a, *b, *c) = (current, next, fraction);
                }
                let (current, next, fraction) = self.line.right.read_fractional_parts(right_delay);
                if let (Some(a), Some(b), Some(c)) = (right_current.get_mut(lane), right_next.get_mut(lane), right_fraction.get_mut(lane)) {
                    (*a, *b, *c) = (current, next, fraction);
                }
            }
            let left_taps = Sample::interpolate_taps(left_current, left_next, left_fraction);
            let right_taps = Sample::interpolate_taps(right_current, right_next, right_fraction);

            let mut wet_left = Sample::ZERO;
            let mut wet_right = Sample::ZERO;
            for tap in 0..voices {
                let lane = tap.max(0) as usize;
                wet_left = wet_left.add(left_taps.get(lane).copied().unwrap_or(Sample::ZERO).scale_q15(tap_gain));
                wet_right = wet_right.add(right_taps.get(lane).copied().unwrap_or(Sample::ZERO).scale_q15(tap_gain));
            }

            let (dry, wet) = equal_power_q15(self.mix_percent.advance());
            frame.left = frame.left.scale_q15(dry).add(wet_left.scale_q15(wet));
            frame.right = frame.right.scale_q15(dry).add(wet_right.scale_q15(wet));
        }
    }

    fn set_param(&mut self, id: ParamId, value: i32) {
        match id {
            CHORUS_VOICES_PARAM => self.voices = clamp_at(0, value),
            CHORUS_RATE_PARAM => self.rate_centi_hz.set_target(clamp_at(1, value), SMOOTH_FRAMES),
            CHORUS_DEPTH_PARAM => {
                self.depth_centi_ms = clamp_at(2, value);
                let target = centi_milliseconds_to_q16_frames(self.depth_centi_ms, self.sample_rate_hz) as i32;
                self.depth_q16.set_target(target, SMOOTH_FRAMES);
            }
            CHORUS_BASE_PARAM => {
                self.base_centi_ms = clamp_at(3, value);
                let target = centi_milliseconds_to_q16_frames(self.base_centi_ms, self.sample_rate_hz) as i32;
                self.base_q16.set_target(target, SMOOTH_FRAMES);
            }
            CHORUS_MIX_PARAM => self.mix_percent.set_target(clamp_at(4, value), SMOOTH_FRAMES),
            CHORUS_SPREAD_PARAM => self.spread_percent.set_target(clamp_at(5, value), SMOOTH_FRAMES),
            _ => {}
        }
    }

    fn param(&self, id: ParamId) -> Option<i32> {
        match id {
            CHORUS_VOICES_PARAM => Some(self.voices),
            CHORUS_RATE_PARAM => Some(self.rate_centi_hz.target()),
            CHORUS_DEPTH_PARAM => Some(self.depth_centi_ms),
            CHORUS_BASE_PARAM => Some(self.base_centi_ms),
            CHORUS_MIX_PARAM => Some(self.mix_percent.target()),
            CHORUS_SPREAD_PARAM => Some(self.spread_percent.target()),
            _ => None,
        }
    }

    fn reset(&mut self) {
        self.line.reset();
        self.lfo = Lfo::new();
        self.rate_centi_hz.snap();
        self.depth_q16.snap();
        self.base_q16.snap();
        self.mix_percent.snap();
        self.spread_percent.snap();
        self.lfo.set_rate(self.rate_centi_hz.current().max(0) as u32, self.sample_rate_hz);
    }

    fn descriptor(&self) -> &'static InsertDescriptor { &CHORUS_DESCRIPTOR }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::testing::{RATE, band_power, block_of, render_noise, render_stereo, segmental_snr_db, white_noise};
    use crate::insert::assert_descriptor_roundtrip;
    use alloc::vec::Vec;

    /// Frames the spectral runs render: 96 whole blocks, of which the last 4096 are
    /// measured.
    const FRAMES: usize = 96 * DSP_BLOCK_FRAMES;

    /// The test tone, in hertz.
    const TONE_HZ: f64 = 1_000.0;

    /// The tone's amplitude, on the raw `i16` scale.
    const TONE_AMPLITUDE: f64 = 12_000.0;

    fn tone(frames: usize) -> Vec<i32> {
        (0..frames).map(|index| (TONE_AMPLITUDE * (core::f64::consts::TAU * TONE_HZ * index as f64 / RATE as f64).sin()) as i32).collect()
    }

    #[test]
    fn descriptor_roundtrip() {
        let mut chorus: Chorus<i32> = Chorus::new(RATE);
        assert_descriptor_roundtrip(&mut chorus, "chorus");
        let mut chorus: Chorus<f32> = Chorus::new(RATE);
        assert_descriptor_roundtrip(&mut chorus, "chorus");
    }

    /// The audibility proof (H3 deliverable 5): a 1 kHz sine through the chorus keeps its
    /// energy at 1 kHz — a chorus detunes by a few cents, it does not transpose — and never
    /// exceeds the input peak by more than the summed tap gains.
    ///
    /// A sinusoidally swept delay is a phase modulator, so the output is the tone plus
    /// sidebands at multiples of the LFO rate. The peak frequency deviation is
    /// `f · depth · 2π · rate`: at the defaults, 2 ms of depth at 0.6 Hz gives ±7.5 Hz on a
    /// 1 kHz tone, which is well inside the ±5-bin (±54 Hz) band this measures. So the claim
    /// under test is that essentially all of the output's energy is in that band and that
    /// frequencies a musical distance away carry none.
    #[test]
    fn a_sine_stays_at_its_own_frequency_and_the_output_stays_inside_its_input() {
        let input = tone(FRAMES);
        let mut chorus: Chorus<i32> = Chorus::new(RATE);
        Insert::<i32>::reset(&mut chorus);
        let output = render_noise(&mut chorus, &input);

        let dry: Vec<f64> = input.iter().map(|value| *value as f64).collect();
        let wet: Vec<f64> = output.iter().map(|value| *value as f64).collect();
        let at_tone = band_power(&wet, TONE_HZ, 5);
        let dry_at_tone = band_power(&dry, TONE_HZ, 5);
        let retained_db = 10.0 * (at_tone / dry_at_tone).log10();
        assert!((-6.0..=3.0).contains(&retained_db), "the 1 kHz band came out {retained_db:.2} dB, so the energy did not stay there");

        for away_hz in [700.0, 850.0, 1_150.0, 1_300.0, 2_000.0] {
            let away = band_power(&wet, away_hz, 5);
            let down_db = 10.0 * (away / at_tone).log10();
            assert!(down_db <= -20.0, "{away_hz} Hz is only {down_db:.1} dB below the tone — the chorus is transposing, not detuning");
        }

        // The summed tap gains are unity by construction, and the mix is equal power, so
        // `dry + wet` can reach at most `sqrt(2)` of the input peak.
        let input_peak = input.iter().map(|value| value.abs()).max().unwrap_or(0) as f64;
        let output_peak = output.iter().map(|value| value.abs()).max().unwrap_or(0) as f64;
        let bound = input_peak * core::f64::consts::SQRT_2;
        assert!(output_peak <= bound, "the output peaked at {output_peak}, past the {bound} the tap gains allow");
    }

    /// The taps do sweep: the chorused tone is not the dry tone.
    #[test]
    fn the_taps_actually_modulate() {
        let input = tone(FRAMES);
        let mut chorus: Chorus<i32> = Chorus::new(RATE);
        Insert::<i32>::reset(&mut chorus);
        let output = render_noise(&mut chorus, &input);
        assert!(output.iter().zip(input.iter()).any(|(after, before)| after != before), "the chorus left the tone alone");
    }

    #[test]
    fn a_chorus_at_zero_mix_is_bit_transparent() {
        let mut chorus: Chorus<i32> = Chorus::new(RATE);
        Insert::<i32>::set_param(&mut chorus, CHORUS_MIX_PARAM, 0);
        Insert::<i32>::reset(&mut chorus);
        let mut samples = block_of(|index| ((index as i32 * 977) % 20_001) - 10_000);
        let original = samples.clone();
        Insert::<i32>::process(&mut chorus, &mut samples);
        assert_eq!(samples, original, "a fully dry chorus changed the block");
    }

    #[test]
    fn the_spread_puts_the_channels_out_of_step() {
        let input: Vec<Stereo<i32>> = tone(FRAMES).into_iter().map(|value| Stereo::new(value, value)).collect();
        let mut wide: Chorus<i32> = Chorus::new(RATE);
        Insert::<i32>::set_param(&mut wide, CHORUS_SPREAD_PARAM, 100);
        Insert::<i32>::reset(&mut wide);
        let widened = render_stereo(&mut wide, &input);
        assert!(widened.iter().any(|frame| frame.left != frame.right), "a spread chorus produced a mono image");

        let mut narrow: Chorus<i32> = Chorus::new(RATE);
        Insert::<i32>::set_param(&mut narrow, CHORUS_SPREAD_PARAM, 0);
        Insert::<i32>::reset(&mut narrow);
        let narrowed = render_stereo(&mut narrow, &input);
        assert!(narrowed.iter().all(|frame| frame.left == frame.right), "with no spread the two channels are the same signal");
    }

    #[test]
    fn two_voices_and_three_voices_are_different_effects() {
        let input = tone(16 * DSP_BLOCK_FRAMES);
        let render = |voices: i32| {
            let mut chorus: Chorus<i32> = Chorus::new(RATE);
            Insert::<i32>::set_param(&mut chorus, CHORUS_VOICES_PARAM, voices);
            Insert::<i32>::reset(&mut chorus);
            render_noise(&mut chorus, &input)
        };
        assert_ne!(render(2), render(3), "the voice count did nothing");
        assert_eq!(Q15_UNITY / 2 * 2, Q15_UNITY, "two taps sum to exactly unity");
    }

    #[test]
    fn the_lfo_phase_is_the_same_however_the_render_is_split() {
        // The engine only ever hands whole blocks, so the interesting split is over
        // *blocks*: a chorus rendered as one long run and as many short ones must agree.
        let input = tone(32 * DSP_BLOCK_FRAMES);
        let mut whole: Chorus<i32> = Chorus::new(RATE);
        Insert::<i32>::reset(&mut whole);
        let reference = render_noise(&mut whole, &input);

        let mut split: Chorus<i32> = Chorus::new(RATE);
        Insert::<i32>::reset(&mut split);
        let mut assembled = Vec::new();
        for chunk in input.chunks(4 * DSP_BLOCK_FRAMES) {
            assembled.extend(render_noise(&mut split, chunk));
        }
        assert_eq!(assembled, reference, "splitting the render changed the chorus");
    }

    #[test]
    fn the_fixed_and_float_paths_agree_to_better_than_sixty_decibels() {
        let noise: Vec<i32> = white_noise(32 * DSP_BLOCK_FRAMES, 8_000);
        let mut fixed: Chorus<i32> = Chorus::new(RATE);
        let mut float: Chorus<f32> = Chorus::new(RATE);
        Insert::<i32>::reset(&mut fixed);
        Insert::<f32>::reset(&mut float);
        let fixed_out = render_noise(&mut fixed, &noise);
        let float_input: Vec<f32> = noise.iter().map(|value| *value as f32).collect();
        let float_out = render_noise(&mut float, &float_input);
        let snr = segmental_snr_db(&fixed_out, &float_out).expect("the render is not silent");
        assert!(snr >= 60.0, "the two paths agree at only {snr:.1} dB");
    }

    #[test]
    fn the_tap_offsets_divide_the_turn_evenly() {
        let mut chorus: Chorus<i32> = Chorus::new(RATE);
        Insert::<i32>::set_param(&mut chorus, CHORUS_VOICES_PARAM, 3);
        assert_eq!(chorus.tap_offset(0), 0);
        assert_eq!(chorus.tap_offset(1), ((1u64 << 32) / 3) as u32, "a third of a turn, as integer division leaves it");
        assert!(chorus.tap_offset(2) > chorus.tap_offset(1));
        assert_eq!(spread_offset(100), 1 << 31, "a full spread is half a turn");
        assert_eq!(spread_offset(0), 0);
    }

    #[test]
    fn a_boxed_chorus_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let boxed: alloc::boxed::Box<dyn Insert<f32>> = alloc::boxed::Box::new(Chorus::<f32>::new(RATE));
        assert_send(&boxed);
    }
}
