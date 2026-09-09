//! [`DecayDenoiser`] — takes the quantisation hiss out of an 8-bit sample's decay without
//! touching its attack, its level or its loop.
//!
//! # Where the noise is, and why a filter is the wrong tool
//!
//! M10-K5a's research point 3 measured it: an 8-bit source carries about 47 dB of SNR, and
//! only 1.4 % of that noise power sits above the source's own Nyquist. Signal and noise
//! occupy exactly the same band, so no filter can separate them and dither at the rebuild
//! would only add a second floor below the first.
//!
//! What *can* be exploited is that the noise floor is **constant** while the signal is
//! not. A sample's attack sits 40-odd dB above the floor and needs nothing done to it; its
//! tail falls towards the floor and eventually below it, and that is where the hiss is
//! audible — a piano note that ends in a wash of white noise rather than in silence. The
//! classic answer is a Wiener gain: attenuate each short block by how much of it is
//! believed to be signal.
//!
//! # The floor
//!
//! When **every frame is a multiple of 256** the sample is a widened 8-bit one — which is
//! true of every sample in this repository's own S3M fixtures — and the floor is known
//! exactly rather than estimated: uniform quantisation with a step of 256 has an error
//! variance of `256² / 12`, so the floor's mean square is
//! [`EIGHT_BIT_FLOOR_MEAN_SQUARE`]. Otherwise it is estimated as the pooled mean square of
//! the quietest [`QUIET_BLOCK_PERCENT`] of blocks, never below one `i16` step's own
//! `1 / 12`.
//!
//! Everything here works in the **mean-square** domain rather than in RMS. That is the
//! same quantity squared, and it means the whole enhancer needs no square root: the Wiener
//! gain is a ratio of mean squares, and the strength is the only place a root appears.
//!
//! # The gain
//!
//! Per 64-frame block, `g = max(0, 1 − floor² / rms²)`, raised to
//! [`DecayDenoiser::strength_percent`] hundredths. `g` is 1 where the signal towers over
//! the floor and 0 where the block is nothing but floor, and it is exactly the gain that
//! minimises the mean-square error against the clean signal for that block.
//!
//! Across blocks the gain **attacks instantly and releases slowly**
//! ([`RELEASE_FRACTION`]): a gain that is too high is merely less effective, while a gain
//! that is too low eats the front of a note, so the asymmetry is the safe direction. Note
//! that a *rising* signal makes the gain rise, so "attack" here is the gain's attack, and
//! it is instant precisely so that a drum's first block is never attenuated.
//!
//! Between blocks the gain is interpolated linearly from block centre to block centre, so
//! nothing steps.
//!
//! # Loops
//!
//! A gain that varied inside a loop would make the loop non-periodic — every pass would be
//! quieter or louder than the last, and the seam would step. So a loop region gets **one**
//! gain, the gain its own pooled mean square asks for, and the frames leading into it ramp
//! to that gain over at most one block so the join is smooth. A sample with a sustain loop
//! has both of its loops treated that way.

use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::Write;

use starplayer_model::{EnhancedPcm, SampleEnhancer, SamplePcm};

use crate::deterministic::{power_hundredths, saturating_i16};

/// Frames per analysis block. 64 frames is 7.6 ms at the tracker's 8 363 Hz — short enough
/// that a drum's 80 ms decay is a dozen blocks, long enough that the mean square of a
/// block is a stable estimate rather than a sample of the waveform.
pub const DENOISE_BLOCK_FRAMES: usize = 64;

/// Mean square of the quantisation error of a widened 8-bit sample: a uniform error over
/// one step of 256 `i16` units has variance `256² / 12`.
///
/// Committed as an `f64` bit pattern, like every other constant in this crate that a
/// runtime division would otherwise have to produce.
/// [`tests::the_committed_floor_constants_are_what_the_arithmetic_says`] is the gate.
pub const EIGHT_BIT_FLOOR_MEAN_SQUARE_BITS: u64 = 0x40b5_5555_5555_5555;

/// [`EIGHT_BIT_FLOOR_MEAN_SQUARE_BITS`], decoded.
pub const EIGHT_BIT_FLOOR_MEAN_SQUARE: f64 = f64::from_bits(EIGHT_BIT_FLOOR_MEAN_SQUARE_BITS);

/// The lowest floor an estimate may report: one `i16` step's own quantisation variance,
/// `1 / 12`. A sample cannot be quieter than the grid it is stored on.
pub const MINIMUM_FLOOR_MEAN_SQUARE_BITS: u64 = 0x3fb5_5555_5555_5555;

/// [`MINIMUM_FLOOR_MEAN_SQUARE_BITS`], decoded.
pub const MINIMUM_FLOOR_MEAN_SQUARE: f64 = f64::from_bits(MINIMUM_FLOOR_MEAN_SQUARE_BITS);

/// Percentage of the quietest blocks pooled into an estimated floor.
pub const QUIET_BLOCK_PERCENT: usize = 5;

/// Ceiling on an **estimated** floor, as a fraction of the whole sample's mean square:
/// 10^-4, which is 40 dB down.
///
/// Without it the estimator is catastrophic on the one thing it is asked to estimate for.
/// A sustained 16-bit tone has no quiet passage at all — its quietest 5 % of blocks are
/// as loud as its loudest — so "the RMS of the quietest blocks" *is* the signal, the
/// Wiener gain reads the whole sample as noise, and M10-K5c's harness measured the
/// in-band SNR of a 16-bit sustained tone falling from a perfect 120 dB to 5.2 dB. With
/// the ceiling the same sample's gain is 0.9999 and it is left alone.
///
/// Forty decibels is the argument in one number: a noise floor worth removing is at least
/// that far below the material sitting on it — an 8-bit source's is 47 dB down — and an
/// estimate that comes back louder has not found a floor, it has found the signal.
pub const MAXIMUM_ESTIMATED_FLOOR_FRACTION_BITS: u64 = 0x3f1a_36e2_eb1c_432d;

/// [`MAXIMUM_ESTIMATED_FLOOR_FRACTION_BITS`], decoded. The `f64` nearest 10^-4.
pub const MAXIMUM_ESTIMATED_FLOOR_FRACTION: f64 = f64::from_bits(MAXIMUM_ESTIMATED_FLOOR_FRACTION_BITS);

/// How far the gain moves towards a **lower** block gain, per block. A quarter of the way
/// per 64 frames is a 26 ms time constant at 8 363 Hz: slower than a tracker's tick, so it
/// cannot pump, and far faster than any decay it has to follow.
pub const RELEASE_FRACTION_BITS: u64 = 0x3fd0_0000_0000_0000;

/// [`RELEASE_FRACTION_BITS`], decoded. Exactly 0.25.
pub const RELEASE_FRACTION: f64 = f64::from_bits(RELEASE_FRACTION_BITS);

/// The strength that is the plain Wiener gain, in hundredths.
pub const DEFAULT_DENOISE_STRENGTH_PERCENT: u32 = 100;

/// Attenuate a sample's quiet blocks by a Wiener gain measured against its own noise floor.
///
/// See the [module documentation](self) for the floor, the gain and the loop rule.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct DecayDenoiser {
    /// The exponent the Wiener gain is raised to, in hundredths. 100 is the plain gain;
    /// above it the enhancer is more aggressive, below it more cautious.
    pub strength_percent: u32,
}

impl DecayDenoiser {
    /// The plain Wiener gain.
    pub const fn new() -> DecayDenoiser { DecayDenoiser { strength_percent: DEFAULT_DENOISE_STRENGTH_PERCENT } }

    /// The gain raised to `percent / 100`.
    pub const fn with_strength_percent(percent: u32) -> DecayDenoiser { DecayDenoiser { strength_percent: percent } }
}

impl Default for DecayDenoiser {
    fn default() -> DecayDenoiser { DecayDenoiser::new() }
}

impl SampleEnhancer for DecayDenoiser {
    fn name(&self) -> String {
        let mut name = String::from("denoise");
        if self.strength_percent != DEFAULT_DENOISE_STRENGTH_PERCENT {
            let _ = write!(&mut name, "={}", self.strength_percent);
        }
        name
    }

    fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm {
        let unchanged = || EnhancedPcm::unchanged(sample);
        if sample.frames.is_empty() || self.strength_percent == 0 {
            return unchanged();
        }

        let floor_mean_square = noise_floor_mean_square(sample.frames);
        let gains = frame_gains(sample.frames, floor_mean_square, self.strength_percent, &sample);
        // A sample the enhancer decided to leave alone comes back bit-identical rather
        // than rounded through a multiply by one.
        if gains.iter().all(|gain| *gain == 1.0) {
            return unchanged();
        }

        let frames: Vec<i16> = sample.frames.iter().zip(gains.iter()).map(|(frame, gain)| saturating_i16(*frame as f64 * *gain)).collect();
        EnhancedPcm { frames, ..unchanged() }
    }
}

/// The sample's noise floor, as a mean square in `i16` units.
///
/// A widened 8-bit sample is recognised exactly and its floor is the arithmetic one; every
/// other sample's floor is the pooled mean square of its quietest blocks.
pub fn noise_floor_mean_square(frames: &[i16]) -> f64 {
    if frames.iter().all(|frame| *frame as i32 % 256 == 0) {
        return EIGHT_BIT_FLOOR_MEAN_SQUARE;
    }
    let mut blocks = block_mean_squares(frames);
    let mut whole = 0.0f64;
    for value in blocks.iter() {
        whole += *value;
    }
    let ceiling = whole / blocks.len() as f64 * MAXIMUM_ESTIMATED_FLOOR_FRACTION;

    blocks.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let quiet = (blocks.len() * QUIET_BLOCK_PERCENT / 100).max(1).min(blocks.len());
    let mut pooled = 0.0f64;
    for value in blocks.iter().take(quiet) {
        pooled += *value;
    }
    let estimate = pooled / quiet as f64;
    let estimate = if estimate > ceiling { ceiling } else { estimate };
    if estimate < MINIMUM_FLOOR_MEAN_SQUARE { MINIMUM_FLOOR_MEAN_SQUARE } else { estimate }
}

/// Mean square of every [`DENOISE_BLOCK_FRAMES`]-frame block, the last one short if the
/// sample does not divide evenly.
fn block_mean_squares(frames: &[i16]) -> Vec<f64> {
    frames
        .chunks(DENOISE_BLOCK_FRAMES)
        .map(|block| {
            let mut energy = 0.0f64;
            for frame in block {
                let value = *frame as f64;
                energy += value * value;
            }
            energy / block.len() as f64
        })
        .collect()
}

/// The Wiener gain for one block's mean square, raised to `strength_percent` hundredths.
fn wiener_gain(mean_square: f64, floor_mean_square: f64, strength_percent: u32) -> f64 {
    if !mean_square.is_finite() || mean_square <= floor_mean_square {
        return 0.0;
    }
    let gain = 1.0 - floor_mean_square / mean_square;
    if strength_percent == DEFAULT_DENOISE_STRENGTH_PERCENT { gain } else { power_hundredths(gain, strength_percent) }
}

/// One gain per frame: block gains, smoothed and interpolated, with each loop region
/// flattened to a single gain and ramped into.
fn frame_gains(frames: &[i16], floor_mean_square: f64, strength_percent: u32, sample: &SamplePcm<'_>) -> Vec<f64> {
    let block_gains = smoothed_block_gains(frames, floor_mean_square, strength_percent);
    let mut gains: Vec<f64> = (0..frames.len()).map(|index| interpolated_gain(&block_gains, index)).collect();

    // The sustain loop first, so an overlapping main loop wins — the main loop is the one
    // a voice ends up in, and only one of the two can own a frame's gain.
    if let Some(sustain) = sample.sustain_loop
        && sustain.mode.is_looping()
    {
        flatten_region(&mut gains, frames, sustain.start as usize, sustain.end as usize, floor_mean_square, strength_percent);
    }
    if sample.loop_mode.is_looping() {
        flatten_region(&mut gains, frames, sample.loop_start as usize, sample.loop_end as usize, floor_mean_square, strength_percent);
    }
    gains
}

/// Per-block gains with an instant attack and a [`RELEASE_FRACTION`] release.
fn smoothed_block_gains(frames: &[i16], floor_mean_square: f64, strength_percent: u32) -> Vec<f64> {
    let mut smoothed: Vec<f64> = Vec::with_capacity(frames.len().div_ceil(DENOISE_BLOCK_FRAMES));
    let mut current = 0.0f64;
    for (index, mean_square) in block_mean_squares(frames).into_iter().enumerate() {
        let raw = wiener_gain(mean_square, floor_mean_square, strength_percent);
        current = match index {
            0 => raw,
            _ if raw > current => raw,
            _ => current + (raw - current) * RELEASE_FRACTION,
        };
        smoothed.push(current);
    }
    smoothed
}

/// The gain at one frame, linearly interpolated between the centres of the blocks either
/// side of it. Frames before the first centre and after the last hold the end gains.
///
/// Block `b`'s centre is frame `b · DENOISE_BLOCK_FRAMES + DENOISE_BLOCK_FRAMES / 2`, so
/// the frame's position **relative to the first centre** picks the left block and the
/// fraction in one step.
fn interpolated_gain(block_gains: &[f64], frame: usize) -> f64 {
    let Some(last) = block_gains.len().checked_sub(1) else { return 1.0 };
    let half = DENOISE_BLOCK_FRAMES / 2;
    let Some(from_first_centre) = frame.checked_sub(half) else { return block_gains[0] };
    let left = from_first_centre / DENOISE_BLOCK_FRAMES;
    if left >= last {
        return block_gains[last];
    }
    let fraction = (from_first_centre % DENOISE_BLOCK_FRAMES) as f64 / DENOISE_BLOCK_FRAMES as f64;
    block_gains[left] + (block_gains[left + 1] - block_gains[left]) * fraction
}

/// Give `[start, end)` one gain — the one its own pooled mean square asks for — and ramp
/// the frames on either side of it to that gain over at most one block.
fn flatten_region(gains: &mut [f64], frames: &[i16], start: usize, end: usize, floor_mean_square: f64, strength_percent: u32) {
    let end = end.min(frames.len());
    if start >= end {
        return;
    }
    let mut energy = 0.0f64;
    for frame in &frames[start..end] {
        let value = *frame as f64;
        energy += value * value;
    }
    let region_gain = wiener_gain(energy / (end - start) as f64, floor_mean_square, strength_percent);
    for gain in &mut gains[start..end] {
        *gain = region_gain;
    }

    let lead_in = DENOISE_BLOCK_FRAMES.min(start);
    if lead_in > 0 {
        let from = gains[start - lead_in];
        for step in 0..lead_in {
            gains[start - lead_in + step] = from + (region_gain - from) * (step as f64 / lead_in as f64);
        }
    }
    let lead_out = DENOISE_BLOCK_FRAMES.min(gains.len() - end);
    if lead_out > 0 {
        let to = gains[end + lead_out - 1];
        for step in 0..lead_out {
            gains[end + step] = region_gain + (to - region_gain) * ((step + 1) as f64 / lead_out as f64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer_model::{DEFAULT_REFERENCE_RATE_HZ, LoopMode, SustainLoop};

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

    /// A widened 8-bit decay: loud for the first half, then a tail sitting just above the
    /// quantisation floor.
    ///
    /// The tail is one step every sixteenth frame, alternating sign, so its mean square is
    /// `2 · 256² / 16 = 8 192` against the 8-bit floor's `5 461` — a Wiener gain of a third.
    /// Close enough to the floor that the enhancer has real work to do, far enough above it
    /// that the gain is a number rather than a zero.
    fn eight_bit_decay(frames: usize) -> Vec<i16> {
        (0..frames)
            .map(|index| {
                let step = match index < frames / 2 {
                    true => match index % 4 {
                        0 => 40,
                        1 => -37,
                        2 => 33,
                        _ => -41,
                    },
                    false => match index % 16 {
                        0 => 1,
                        8 => -1,
                        _ => 0,
                    },
                };
                (step * 256) as i16
            })
            .collect()
    }

    #[test]
    fn the_committed_floor_constants_are_what_the_arithmetic_says() {
        assert_eq!(EIGHT_BIT_FLOOR_MEAN_SQUARE.to_bits(), (256.0f64 * 256.0 / 12.0).to_bits());
        assert_eq!(MINIMUM_FLOOR_MEAN_SQUARE.to_bits(), (1.0f64 / 12.0).to_bits());
        assert_eq!(MAXIMUM_ESTIMATED_FLOOR_FRACTION.to_bits(), 1.0e-4f64.to_bits());
        assert_eq!(RELEASE_FRACTION, 0.25);
    }

    /// Research point 1's finding, as a test: a sustained 16-bit tone has no quiet passage
    /// to estimate a floor from, and the ceiling is what stops the estimator reading the
    /// whole signal as noise.
    #[test]
    fn a_sustained_sixteen_bit_tone_is_left_alone_because_the_estimate_is_capped() {
        let frames: Vec<i16> = (0..4_096)
            .map(|index| {
                // Sixteen samples per cycle, so every 64-frame block holds exactly four
                // cycles and every block's mean square is identical.
                let step = index % 16;
                let table = [0, 3_136, 5_792, 7_568, 8_192, 7_568, 5_792, 3_136, 0, -3_136, -5_792, -7_568, -8_192, -7_568, -5_792, -3_136];
                table[step] as i16
            })
            .collect();
        let mean_square: f64 = frames.iter().map(|frame| *frame as f64 * *frame as f64).sum::<f64>() / frames.len() as f64;
        let floor = noise_floor_mean_square(&frames);
        assert!(floor <= mean_square * MAXIMUM_ESTIMATED_FLOOR_FRACTION * 1.001, "the estimate {floor} is not capped against the sample's own {mean_square}");

        let enhanced = DecayDenoiser::new().enhance(one_shot(&frames));
        for (after, before) in enhanced.frames.iter().zip(frames.iter()) {
            assert!((*after as i32 - *before as i32).abs() <= 2, "a sustained 16-bit tone must survive: {after} against {before}");
        }
    }

    #[test]
    fn a_widened_eight_bit_sample_is_recognised_and_gets_the_arithmetic_floor() {
        let frames = eight_bit_decay(1_024);
        assert_eq!(noise_floor_mean_square(&frames), EIGHT_BIT_FLOOR_MEAN_SQUARE);
    }

    #[test]
    fn a_sixteen_bit_sample_gets_an_estimated_floor_no_lower_than_one_step() {
        // A pure tone at full resolution: the quietest blocks are still the tone, so the
        // estimate is the tone's own energy rather than a floor — which is exactly why a
        // 16-bit source gets almost no denoising. Research point 1.
        let frames: Vec<i16> = (0..1_024).map(|index| ((index * 37 % 256) as i16 - 128) * 51).collect();
        let floor = noise_floor_mean_square(&frames);
        assert!(floor >= MINIMUM_FLOOR_MEAN_SQUARE, "an estimate may never fall below one step: {floor}");
        assert_ne!(floor, EIGHT_BIT_FLOOR_MEAN_SQUARE, "this sample is not a widened 8-bit one");

        // And a genuinely silent 16-bit tail floors the estimate at one step rather than
        // at zero, so the gain cannot be driven to nothing by a lucky block.
        let mut with_silence = frames.clone();
        with_silence.extend(core::iter::repeat_n(0i16, 1_024));
        assert_eq!(noise_floor_mean_square(&with_silence), MINIMUM_FLOOR_MEAN_SQUARE);
    }

    #[test]
    fn the_loud_half_keeps_its_level_and_the_quiet_half_is_attenuated() {
        let frames = eight_bit_decay(2_048);
        let enhanced = DecayDenoiser::new().enhance(one_shot(&frames));
        assert_eq!(enhanced.frames.len(), frames.len());
        assert_eq!(enhanced.rate_hz, DEFAULT_REFERENCE_RATE_HZ, "a denoiser changes no rate");

        let energy = |slice: &[i16]| slice.iter().map(|frame| *frame as f64 * *frame as f64).sum::<f64>();
        let loud_before = energy(&frames[..512]);
        let loud_after = energy(&enhanced.frames[..512]);
        assert!(loud_after > loud_before * 0.99, "the loud half lost {:.3} of its energy", 1.0 - loud_after / loud_before);

        let quiet_before = energy(&frames[1_536..]);
        let quiet_after = energy(&enhanced.frames[1_536..]);
        assert!(quiet_after < quiet_before * 0.2, "the quiet tail kept {:.3} of its energy", quiet_after / quiet_before);
    }

    #[test]
    fn a_sample_far_above_its_floor_everywhere_is_left_alone() {
        // Every block towers over the 8-bit floor, so every gain rounds to one and the
        // frames come back untouched rather than rounded through a multiply.
        let frames: Vec<i16> = (0..512).map(|index| (((index % 8) as i16) - 4) * 4_096).collect();
        let enhanced = DecayDenoiser::new().enhance(one_shot(&frames));
        assert!(enhanced.frames.iter().zip(frames.iter()).all(|(after, before)| (*after as i32 - *before as i32).abs() <= 1));
    }

    #[test]
    fn an_empty_sample_round_trips() {
        let empty: Vec<i16> = Vec::new();
        let enhanced = DecayDenoiser::new().enhance(one_shot(&empty));
        assert!(enhanced.frames.is_empty());
    }

    /// The loop rule: one gain across the whole region, so every pass through the loop is
    /// the same and the seam the mixer wraps over is untouched.
    #[test]
    fn a_forward_loop_gets_one_constant_gain_across_its_whole_region() {
        // A body whose first half is loud and whose loop — the quiet second half — would
        // otherwise be given a falling gain block by block.
        let frames = eight_bit_decay(2_048);
        let looping = SamplePcm { loop_mode: LoopMode::Forward, loop_start: 1_024, loop_end: 2_048, ..one_shot(&frames) };
        let enhanced = DecayDenoiser::new().enhance(looping);

        let ratios: Vec<f64> = (1_024..2_048)
            .filter(|index| frames[*index] != 0)
            .map(|index| enhanced.frames[index] as f64 / frames[index] as f64)
            .collect();
        assert!(!ratios.is_empty(), "the loop has to carry some non-zero frames");
        let first = ratios[0];
        for ratio in &ratios {
            assert!((ratio - first).abs() < 0.02, "the loop's gain moved from {first} to {ratio}");
        }
    }

    /// The frames leading into a loop reach the loop's own gain rather than stepping to it.
    #[test]
    fn the_run_up_to_a_loop_ramps_to_the_loops_gain() {
        let frames = eight_bit_decay(2_048);
        let looping = SamplePcm { loop_mode: LoopMode::Forward, loop_start: 1_024, loop_end: 2_048, ..one_shot(&frames) };
        let enhanced = DecayDenoiser::new().enhance(looping);
        let gain_at = |index: usize| match frames[index] {
            0 => None,
            value => Some(enhanced.frames[index] as f64 / value as f64),
        };
        let inside = (1_024..1_100).find_map(gain_at).expect("a non-zero frame inside the loop");
        let just_before = (960..1_024).rev().find_map(gain_at).expect("a non-zero frame just before it");
        assert!((just_before - inside).abs() < 0.2, "the run-up steps from {just_before} to {inside}");
    }

    #[test]
    fn a_sustain_loop_is_flattened_too_and_the_main_loop_wins_where_they_meet() {
        let frames = eight_bit_decay(2_048);
        let sustained = SamplePcm {
            loop_mode: LoopMode::Forward,
            loop_start: 1_024,
            loop_end: 2_048,
            sustain_loop: Some(SustainLoop { mode: LoopMode::Forward, start: 1_500, end: 1_800 }),
            ..one_shot(&frames)
        };
        let enhanced = DecayDenoiser::new().enhance(sustained);
        let ratios: Vec<f64> = (1_024..2_048)
            .filter(|index| frames[*index] != 0)
            .map(|index| enhanced.frames[index] as f64 / frames[index] as f64)
            .collect();
        let first = ratios[0];
        for ratio in &ratios {
            assert!((ratio - first).abs() < 0.02, "the overlapping sustain loop broke the main loop's constant gain");
        }
    }

    #[test]
    fn the_name_encodes_the_strength_and_nothing_else() {
        assert_eq!(DecayDenoiser::new().name(), "denoise");
        assert_eq!(DecayDenoiser::with_strength_percent(100).name(), "denoise");
        assert_eq!(DecayDenoiser::with_strength_percent(150).name(), "denoise=150");
    }

    #[test]
    fn the_same_input_denoises_to_the_same_bytes_twice() {
        let frames = eight_bit_decay(1_024);
        let first = DecayDenoiser::with_strength_percent(137).enhance(one_shot(&frames));
        let second = DecayDenoiser::with_strength_percent(137).enhance(one_shot(&frames));
        assert_eq!(first, second);
    }
}
