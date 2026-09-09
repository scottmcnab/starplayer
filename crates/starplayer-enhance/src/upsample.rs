//! [`SincUpsampler`] — a polyphase windowed-sinc rebuild of one sample at two or four
//! times its stored rate.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use starplayer_model::{EnhancedPcm, LoopMode, SampleEnhancer, SamplePcm, SustainLoop, ping_pong_reflect};

use crate::polyphase::{UPSAMPLE_LEADING_TAPS, UPSAMPLE_PHASES, UPSAMPLE_TAPS, coefficient};

/// How far a sample is upsampled. Both factors are powers of two, because the module
/// records the factor as a shift.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum UpsampleFactor {
    /// Twice the stored rate, using the table's half-step phases.
    Two,
    /// Four times the stored rate, using all four quarter-step phases.
    Four,
}

impl UpsampleFactor {
    /// The factor itself.
    pub const fn ratio(self) -> u32 {
        match self {
            UpsampleFactor::Two => 2,
            UpsampleFactor::Four => 4,
        }
    }

    /// `log2` of the factor — what the module stores.
    pub const fn log2(self) -> u8 {
        match self {
            UpsampleFactor::Two => 1,
            UpsampleFactor::Four => 2,
        }
    }

    /// The next factor down, or `None` for "leave the sample alone". The rate ceiling
    /// walks this chain.
    const fn next_lower(self) -> Option<UpsampleFactor> {
        match self {
            UpsampleFactor::Four => Some(UpsampleFactor::Two),
            UpsampleFactor::Two => None,
        }
    }
}

/// Rebuild a sample at two or four times its stored rate through a 64-tap polyphase
/// windowed sinc.
///
/// # What the filter sees past the end of the sample
///
/// The source is treated as an **infinite** sequence, so the taps that reach past either
/// end read something meaningful rather than a hard zero in the middle of a waveform:
///
/// * before frame 0 — silence, which is what the module's own pre-roll holds;
/// * a **forward** loop with no sustain loop — the loop's periodic continuation, so the
///   frames the filter reads past `loop_end` are the frames playback will actually reach;
/// * a **ping-pong** loop with no sustain loop — the loop's reflection, computed with
///   `starplayer_model::ping_pong_reflect`, which is the same function the module builder
///   fills a ping-pong sample's guard frames with;
/// * a one-shot, or any sample carrying a sustain loop — silence. A sample with a sustain
///   loop cannot be periodic in two loops at once, so it is resampled as a zero-extended
///   one-shot with both loops' points scaled. Its guard frames are already silence
///   (`ModuleBuilder::add_sample`), so nothing is lost that playback would have heard.
///
/// # How the loop points scale
///
/// A forward loop, a one-shot and a sustain-loop sample all scale straight through:
/// `start' = F·start`, `end' = F·end`. A **ping-pong** loop does not. The mixer turns *on*
/// `start` and *on* `end − 1` with period `2(len − 1)`, so the turning frames are what have
/// to land on multiples of `F`: `start' = F·start` and `end' = F·(end − 1) + 1`.
///
/// # Rate ceiling
///
/// [`SincUpsampler::with_rate_ceiling`] caps the *effective* playback rate of each sample
/// independently: a module whose samples are already at 44.1 kHz gains nothing from being
/// quadrupled, and costs sixteen times the memory for it. The effective rate is the
/// sample's own rate for MOD, MTM, S3M and IT; for XM it is
/// `rate_hz · 2^((relative_note + finetune/128)/12)`, because the XM loader stores the
/// nominal 8363 for every sample and keeps its tuning in those two raw fields. One formula
/// covers both, since `relative_note` and `finetune` are zero for every other format.
///
/// If the requested factor would take a sample past the ceiling the largest one that fits
/// is used — 4x, then 2x, then the sample is left alone — **per sample**, so a module that
/// mixes a 4 kHz bass with a 32 kHz break gets both treated properly.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SincUpsampler {
    factor: UpsampleFactor,
    rate_ceiling_hz: Option<u32>,
}

impl SincUpsampler {
    /// An upsampler at `factor`, with no rate ceiling.
    pub const fn new(factor: UpsampleFactor) -> SincUpsampler {
        SincUpsampler { factor, rate_ceiling_hz: None }
    }

    /// The same upsampler, refusing to take any sample's effective rate past `hz`.
    pub const fn with_rate_ceiling(self, hz: u32) -> SincUpsampler {
        SincUpsampler { rate_ceiling_hz: Some(hz), ..self }
    }

    /// The factor this upsampler was asked for, before any ceiling applies.
    pub const fn factor(self) -> UpsampleFactor { self.factor }

    /// The rate ceiling, if one was set.
    pub const fn rate_ceiling_hz(self) -> Option<u32> { self.rate_ceiling_hz }

    /// The factor this sample actually gets: the requested one, or the largest smaller one
    /// whose result fits under the ceiling, or `None` to leave the sample alone.
    pub fn factor_for(&self, sample: SamplePcm<'_>) -> Option<UpsampleFactor> {
        let Some(ceiling_hz) = self.rate_ceiling_hz else { return Some(self.factor) };
        let effective_hz = effective_rate_hz(sample);
        let mut candidate = Some(self.factor);
        while let Some(factor) = candidate {
            if effective_hz.saturating_mul(factor.ratio()) <= ceiling_hz {
                return Some(factor);
            }
            candidate = factor.next_lower();
        }
        None
    }
}

impl SampleEnhancer for SincUpsampler {
    fn name(&self) -> String {
        let mut name = String::new();
        let _ = write!(&mut name, "sinc{}x", self.factor.ratio());
        if let Some(ceiling_hz) = self.rate_ceiling_hz {
            let _ = write!(&mut name, "-ceil{ceiling_hz}");
        }
        name
    }

    fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm {
        let Some(factor) = self.factor_for(sample) else { return EnhancedPcm::unchanged(sample) };
        if sample.frames.is_empty() {
            return EnhancedPcm { rate_hz: sample.rate_hz.saturating_mul(factor.ratio()), ..EnhancedPcm::unchanged(sample) };
        }
        resample(sample, factor)
    }
}

/// The rate a sample really sounds its reference note at.
///
/// `rate_hz · 2^((relative_note + finetune/128)/12)`, through `starplayer_dsp::pow2_q24`
/// so there is no transcendental function here either. Every format but XM leaves both
/// raw fields zero, where the exponent is zero and this is `rate_hz` exactly.
fn effective_rate_hz(sample: SamplePcm<'_>) -> u32 {
    if sample.relative_note == 0 && sample.finetune == 0 {
        return sample.rate_hz;
    }
    // Semitones in Q16.16: `relative_note + finetune/128`, divided by the twelve semitones
    // in an octave, which is what `pow2_q24` wants.
    let semitones_scaled = (sample.relative_note as i64) * 128 + sample.finetune as i64;
    let octaves_q16 = ((semitones_scaled << 16) / (12 * 128)) as i32;
    let factor_q24 = starplayer_dsp::pow2_q24(octaves_q16).max(0) as u64;
    let scaled = (sample.rate_hz as u64).saturating_mul(factor_q24) >> starplayer_dsp::tables::POW2_FRACTION_BITS;
    u32::try_from(scaled).unwrap_or(u32::MAX)
}

/// The source, read as an infinite sequence. See [`SincUpsampler`] for the four cases.
struct InfiniteSource<'pcm> {
    frames: &'pcm [i16],
    extension: Extension,
    loop_start: u32,
    loop_end: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Extension {
    Zero,
    Forward,
    PingPong,
}

impl InfiniteSource<'_> {
    fn at(&self, index: i64) -> f64 {
        if index < 0 {
            return 0.0;
        }
        let index = index as u64;
        if let Some(frame) = usize::try_from(index).ok().and_then(|index| self.frames.get(index)) {
            return *frame as f64;
        }
        let source = match self.extension {
            Extension::Zero => return 0.0,
            Extension::Forward => {
                let length = (self.loop_end - self.loop_start) as u64;
                self.loop_start as u64 + (index - self.loop_start as u64) % length
            }
            Extension::PingPong => {
                // Reduce into one period first, so the position handed to the model's own
                // reflection is always inside `start .. start + period` and cannot
                // overflow the `u32` it takes.
                let length = (self.loop_end - self.loop_start) as u64;
                if length <= 1 {
                    self.loop_start as u64
                } else {
                    let period = 2 * (length - 1);
                    let phase = (index - self.loop_start as u64) % period;
                    let position = u32::try_from(self.loop_start as u64 + phase).unwrap_or(self.loop_start);
                    ping_pong_reflect(self.loop_start, self.loop_end, position) as u64
                }
            }
        };
        usize::try_from(source).ok().and_then(|source| self.frames.get(source)).map(|frame| *frame as f64).unwrap_or(0.0)
    }
}

fn resample(sample: SamplePcm<'_>, factor: UpsampleFactor) -> EnhancedPcm {
    let ratio = factor.ratio();
    // 4x walks the four quarter-step phases in order; 2x takes every other one, which is
    // phases 0 and 2 — the half-step pair.
    let phase_stride = UPSAMPLE_PHASES / ratio as usize;

    let body_frames = sample.frames.len() as u64;
    let periodic = sample.sustain_loop.is_none() && sample.loop_mode.is_looping();
    let extension = match (periodic, sample.loop_mode) {
        (true, LoopMode::Forward) => Extension::Forward,
        (true, LoopMode::PingPong) => Extension::PingPong,
        _ => Extension::Zero,
    };
    let source = InfiniteSource { frames: sample.frames, extension, loop_start: sample.loop_start, loop_end: sample.loop_end };

    let (loop_start, loop_end) = scaled_span(sample.loop_mode, sample.loop_start, sample.loop_end, ratio);
    let output_frames = match extension {
        // A ping-pong loop with no sustain loop stores exactly `loop_end` frames, and its
        // scaled end is the turning frame plus one rather than `ratio × end`.
        Extension::PingPong => loop_end as u64,
        _ => body_frames.saturating_mul(ratio as u64),
    };
    let output_frames = usize::try_from(output_frames).unwrap_or(usize::MAX);

    let mut frames = Vec::with_capacity(output_frames);
    for output_index in 0..output_frames as u64 {
        let base = (output_index / ratio as u64) as i64 - UPSAMPLE_LEADING_TAPS as i64;
        let phase = (output_index % ratio as u64) as usize * phase_stride;
        // Fixed order, tap 0 to tap 63, no tree reduction: the sum has to be the same
        // sequence of `f64` operations on every target.
        let mut accumulator = 0.0f64;
        for tap in 0..UPSAMPLE_TAPS {
            accumulator += coefficient(phase, tap) * source.at(base + tap as i64);
        }
        frames.push(saturating_i16(accumulator));
    }

    EnhancedPcm {
        frames,
        rate_hz: sample.rate_hz.saturating_mul(ratio),
        loop_mode: sample.loop_mode,
        loop_start,
        loop_end,
        sustain_loop: sample.sustain_loop.map(|sustain| {
            let (start, end) = scaled_span(sustain.mode, sustain.start, sustain.end, ratio);
            SustainLoop { mode: sustain.mode, start, end }
        }),
    }
}

/// A loop's scaled points.
///
/// A ping-pong loop turns on `start` and on `end − 1`, so it is the **turning frames** that
/// have to land on multiples of the factor; every other shape scales straight through.
fn scaled_span(mode: LoopMode, start: u32, end: u32, ratio: u32) -> (u32, u32) {
    let scaled_start = start.saturating_mul(ratio);
    let scaled_end = match mode {
        LoopMode::PingPong => end.saturating_sub(1).saturating_mul(ratio).saturating_add(1),
        _ => end.saturating_mul(ratio),
    };
    (scaled_start, scaled_end)
}

/// Round half away from zero, then saturate to `i16`.
///
/// `as i64` truncates towards zero and saturates on a non-finite input, so adding half a
/// unit in the value's own direction first is exactly "round half away from zero" with no
/// branch on the rounding mode of the target.
fn saturating_i16(value: f64) -> i16 {
    let rounded = if value >= 0.0 { (value + 0.5) as i64 } else { (value - 0.5) as i64 };
    rounded.clamp(i16::MIN as i64, i16::MAX as i64) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer_model::DEFAULT_REFERENCE_RATE_HZ;
    use std::vec;

    fn one_shot(frames: &[i16]) -> SamplePcm<'_> {
        SamplePcm {
            frames,
            rate_hz: DEFAULT_REFERENCE_RATE_HZ,
            relative_note: 0,
            finetune: 0,
            loop_mode: LoopMode::None,
            loop_start: 0,
            loop_end: 0,
            sustain_loop: None,
        }
    }

    /// A 1 kHz sine at 8 kHz, long enough that the Goertzel below resolves cleanly.
    fn sine(frames: usize, rate_hz: f64, frequency_hz: f64) -> std::vec::Vec<i16> {
        (0..frames)
            .map(|index| {
                let phase = core::f64::consts::TAU * frequency_hz * index as f64 / rate_hz;
                (phase.sin() * 12_000.0).round() as i16
            })
            .collect()
    }

    /// Magnitude of `frequency_hz` in `frames`, by the Goertzel algorithm in `f64`.
    fn goertzel(frames: &[i16], rate_hz: f64, frequency_hz: f64) -> f64 {
        let omega = core::f64::consts::TAU * frequency_hz / rate_hz;
        let coefficient = 2.0 * omega.cos();
        let (mut previous, mut older) = (0.0f64, 0.0f64);
        for frame in frames {
            let current = *frame as f64 + coefficient * previous - older;
            older = previous;
            previous = current;
        }
        (previous * previous + older * older - coefficient * previous * older).sqrt() / (frames.len() as f64 / 2.0)
    }

    #[test]
    fn a_sine_upsampled_four_times_keeps_its_fundamental_and_rejects_its_image() {
        // The taps at either end read silence, so the measurement window skips a tap's
        // worth of run-in and run-out at the upsampled rate.
        let source = sine(2_048, 8_000.0, 1_000.0);
        let enhanced = SincUpsampler::new(UpsampleFactor::Four).enhance(one_shot(&source));
        assert_eq!(enhanced.frames.len(), source.len() * 4);
        assert_eq!(enhanced.rate_hz, DEFAULT_REFERENCE_RATE_HZ * 4);

        let window = enhanced.frames.get(512..enhanced.frames.len() - 512).expect("the steady-state window");
        let fundamental = goertzel(window, 32_000.0, 1_000.0);
        let image = goertzel(window, 32_000.0, 7_000.0);
        let reference = goertzel(source.get(128..source.len() - 128).expect("the source window"), 8_000.0, 1_000.0);

        let fundamental_db = 20.0 * (fundamental / reference).log10();
        assert!(fundamental_db.abs() < 0.05, "the fundamental moved by {fundamental_db} dB");
        let image_db = 20.0 * (image / fundamental).log10();
        assert!(image_db < -80.0, "the 7 kHz image is only {image_db} dB down");
    }

    #[test]
    fn a_sine_upsampled_twice_keeps_its_fundamental_and_rejects_its_image() {
        let source = sine(2_048, 8_000.0, 1_000.0);
        let enhanced = SincUpsampler::new(UpsampleFactor::Two).enhance(one_shot(&source));
        assert_eq!(enhanced.frames.len(), source.len() * 2);

        let window = enhanced.frames.get(256..enhanced.frames.len() - 256).expect("the steady-state window");
        let fundamental = goertzel(window, 16_000.0, 1_000.0);
        let image = goertzel(window, 16_000.0, 7_000.0);
        let reference = goertzel(source.get(128..source.len() - 128).expect("the source window"), 8_000.0, 1_000.0);

        assert!((20.0 * (fundamental / reference).log10()).abs() < 0.05);
        assert!(20.0 * (image / fundamental).log10() < -80.0, "the 7 kHz image is not rejected");
    }

    /// The taps reach 31 source frames back and 32 forward, so an output frame is only
    /// comparable against an unrolled reference where the whole tap window is inside both.
    const COMPARABLE_FROM_SOURCE_FRAME: usize = UPSAMPLE_LEADING_TAPS;

    #[test]
    fn a_forward_loops_seam_matches_the_upsampled_periodic_extension() {
        // A loop whose two ends do not meet. Every output frame whose taps reach past
        // `loop_end` has to be what the filter would have produced from an unrolled copy
        // of the loop, so that the frames playback reaches over the wrap — and the guard
        // frames the builder fills from `loop_start` — are the loop's real continuation.
        let source: std::vec::Vec<i16> = (0..64).map(|index| ((index as f64 * 0.3).sin() * 9_000.0) as i16).collect();
        let looping = SamplePcm { loop_mode: LoopMode::Forward, loop_start: 8, loop_end: 64, ..one_shot(&source) };
        let enhanced = SincUpsampler::new(UpsampleFactor::Four).enhance(looping);
        assert_eq!((enhanced.loop_start, enhanced.loop_end), (32, 256));
        assert_eq!(enhanced.frames.len(), 256);

        // Unroll two further copies of the loop by hand and upsample that as a one-shot.
        // The periodic extension is exactly this array, so the two must agree frame for
        // frame wherever both have a full tap window.
        let mut unrolled = std::vec::Vec::from(source.as_slice());
        for _ in 0..2 {
            unrolled.extend_from_slice(source.get(8..64).expect("the loop body"));
        }
        let reference = SincUpsampler::new(UpsampleFactor::Four).enhance(one_shot(&unrolled));
        let first = COMPARABLE_FROM_SOURCE_FRAME * 4;
        for index in first..enhanced.frames.len() {
            let from_loop = enhanced.frames.get(index).copied().expect("a looped frame");
            let from_unrolled = reference.frames.get(index).copied().expect("an unrolled frame");
            assert_eq!(from_loop, from_unrolled, "output frame {index} differs from the unrolled loop");
        }
        // And the very last frames — the ones whose taps read only the periodic
        // continuation — are the interesting half of that window.
        assert!(first < 224, "the comparison window has to include the frames past loop_end");
    }

    #[test]
    fn a_ping_pong_loops_scaled_turn_matches_the_mixers_reflection() {
        let source: std::vec::Vec<i16> = (0..64).map(|index| ((index as f64 * 0.21).sin() * 9_000.0) as i16).collect();
        let bouncing = SamplePcm { loop_mode: LoopMode::PingPong, loop_start: 8, loop_end: 64, ..one_shot(&source) };
        let enhanced = SincUpsampler::new(UpsampleFactor::Four).enhance(bouncing);
        // `start' = 4·start`, `end' = 4·(end − 1) + 1`, so both turning frames scale by four
        // exactly: 4·8 = 32 and 4·63 = 252.
        assert_eq!((enhanced.loop_start, enhanced.loop_end), (32, 253));
        assert_eq!(enhanced.frames.len(), 253, "a ping-pong loop stores exactly its scaled loop end");

        // The scaled span reflects the way the model's own arithmetic — the same function
        // the builder fills a ping-pong guard with — says it should: `end' + k` maps to
        // `end' − 2 − k`, which is the turn at `end' − 1` walking back.
        for offset in 0..8u32 {
            let reflected = ping_pong_reflect(enhanced.loop_start, enhanced.loop_end, enhanced.loop_end + offset);
            assert_eq!(reflected, enhanced.loop_end - 2 - offset);
        }

        // The frames near the turn are what unrolling the reflection by hand produces.
        let mut unrolled = std::vec::Vec::from(source.as_slice());
        for position in 64..128u32 {
            let frame = source.get(ping_pong_reflect(8, 64, position) as usize).copied().expect("a reflected frame");
            unrolled.push(frame);
        }
        let reference = SincUpsampler::new(UpsampleFactor::Four).enhance(one_shot(&unrolled));
        for index in COMPARABLE_FROM_SOURCE_FRAME * 4..enhanced.frames.len() {
            let from_loop = enhanced.frames.get(index).copied().expect("a looped frame");
            let from_unrolled = reference.frames.get(index).copied().expect("an unrolled frame");
            assert_eq!(from_loop, from_unrolled, "output frame {index} differs from the unrolled reflection");
        }
    }

    #[test]
    fn a_sample_with_a_sustain_loop_is_resampled_as_a_zero_extended_one_shot() {
        let source: std::vec::Vec<i16> = (0..48).map(|index| (index as i16) * 100).collect();
        let sustained = SamplePcm {
            loop_mode: LoopMode::Forward,
            loop_start: 32,
            loop_end: 48,
            sustain_loop: Some(SustainLoop { mode: LoopMode::PingPong, start: 8, end: 24 }),
            ..one_shot(&source)
        };
        let enhanced = SincUpsampler::new(UpsampleFactor::Four).enhance(sustained);
        assert_eq!(enhanced.frames.len(), 48 * 4, "the whole body is kept, as the builder requires of a sustain-loop sample");
        assert_eq!((enhanced.loop_start, enhanced.loop_end), (128, 192));
        assert_eq!(enhanced.sustain_loop, Some(SustainLoop { mode: LoopMode::PingPong, start: 32, end: 4 * 23 + 1 }));
    }

    #[test]
    fn the_rate_ceiling_falls_back_per_sample() {
        let frames = vec![0i16; 64];
        let upsampler = SincUpsampler::new(UpsampleFactor::Four).with_rate_ceiling(48_000);

        let low = SamplePcm { rate_hz: 8_000, ..one_shot(&frames) };
        assert_eq!(upsampler.factor_for(low), Some(UpsampleFactor::Four), "8 kHz × 4 fits under 48 kHz");

        let middle = SamplePcm { rate_hz: 16_000, ..one_shot(&frames) };
        assert_eq!(upsampler.factor_for(middle), Some(UpsampleFactor::Two), "16 kHz × 4 does not, 16 kHz × 2 does");

        let high = SamplePcm { rate_hz: 44_100, ..one_shot(&frames) };
        assert_eq!(upsampler.factor_for(high), None, "an already-high sample is left alone");
        let unchanged = upsampler.enhance(high);
        assert_eq!(unchanged.frames, frames, "and its frames come back untouched");
        assert_eq!(unchanged.rate_hz, 44_100);
    }

    #[test]
    fn an_xm_samples_effective_rate_follows_its_relative_note_and_finetune() {
        let frames = vec![0i16; 8];
        let nominal = SamplePcm { rate_hz: 8_363, ..one_shot(&frames) };
        assert_eq!(effective_rate_hz(nominal), 8_363);

        let octave_up = SamplePcm { relative_note: 12, ..nominal };
        assert!((effective_rate_hz(octave_up) as i64 - 16_726).abs() <= 2, "an octave up is twice the rate");

        let octave_down = SamplePcm { relative_note: -12, ..nominal };
        assert!((effective_rate_hz(octave_down) as i64 - 4_181).abs() <= 2, "an octave down is half the rate");

        // A ceiling that admits the nominal rate at 4x still refuses the transposed one.
        let upsampler = SincUpsampler::new(UpsampleFactor::Four).with_rate_ceiling(40_000);
        assert_eq!(upsampler.factor_for(nominal), Some(UpsampleFactor::Four));
        assert_eq!(upsampler.factor_for(SamplePcm { relative_note: 12, ..nominal }), Some(UpsampleFactor::Two), "16.7 kHz × 4 is past 40 kHz, × 2 is not");
        assert_eq!(upsampler.factor_for(SamplePcm { relative_note: 24, ..nominal }), None, "33 kHz cannot even be doubled");
    }

    #[test]
    fn an_empty_sample_round_trips() {
        let empty: std::vec::Vec<i16> = std::vec::Vec::new();
        let enhanced = SincUpsampler::new(UpsampleFactor::Four).enhance(one_shot(&empty));
        assert!(enhanced.frames.is_empty());
        assert_eq!(enhanced.rate_hz, DEFAULT_REFERENCE_RATE_HZ * 4);
    }

    #[test]
    fn the_name_encodes_every_parameter_that_changes_the_output() {
        assert_eq!(SincUpsampler::new(UpsampleFactor::Four).name(), "sinc4x");
        assert_eq!(SincUpsampler::new(UpsampleFactor::Two).name(), "sinc2x");
        assert_eq!(SincUpsampler::new(UpsampleFactor::Four).with_rate_ceiling(48_000).name(), "sinc4x-ceil48000");
    }

    #[test]
    fn a_constant_sample_stays_constant_because_every_phase_has_unit_gain() {
        let flat = vec![4_242i16; 128];
        let enhanced = SincUpsampler::new(UpsampleFactor::Four).enhance(one_shot(&flat));
        for frame in enhanced.frames.get(256..enhanced.frames.len() - 256).expect("the steady-state window") {
            assert_eq!(*frame, 4_242, "unit DC gain means a constant comes out unchanged");
        }
    }

    #[test]
    fn the_same_input_upsamples_to_the_same_bytes_twice() {
        let source = sine(512, 8_000.0, 700.0);
        let first = SincUpsampler::new(UpsampleFactor::Four).enhance(one_shot(&source));
        let second = SincUpsampler::new(UpsampleFactor::Four).enhance(one_shot(&source));
        assert_eq!(first, second);
    }
}
