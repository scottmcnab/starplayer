//! Measurement helpers shared by the effects' own tests (H3 deliverable 5).
//!
//! Compiled only under `cargo test`. Everything here runs in `f64` with `libm` through
//! `std` — a *measurement* may use a transcendental, because it never ships: the shipped
//! crate is `#![no_std]` and the effects themselves read tables. `crate::sinc_table`'s
//! regeneration test makes the same distinction.
//!
//! The three measurements the task file asks for live here so that each effect's tests are
//! about the effect: a DFT-ratio band gain (the EQ's spectral proof), a segmental SNR (the
//! fixed-versus-float proof every effect needs) and a white-noise source seeded from
//! `starplayer_core::Xorshift32`.

use alloc::vec;
use alloc::vec::Vec;

use starplayer_core::Xorshift32;

use crate::frame::Stereo;
use crate::insert::{DSP_BLOCK_FRAMES, Insert};
use crate::sample::DspSample;

/// The sample rate every effect test builds at.
pub const RATE: u32 = 44_100;

/// Frames the spectral and SNR measurements window: a 4096-point DFT, as the task file
/// specifies.
pub const WINDOW_FRAMES: usize = 4_096;

/// One DSP block whose left and right both hold `value(index)`.
pub fn block_of<Sample: DspSample>(value: impl Fn(usize) -> Sample) -> Vec<Stereo<Sample>> {
    (0..DSP_BLOCK_FRAMES).map(|index| Stereo::new(value(index), value(index))).collect()
}

/// `frames` of white noise on the raw `i16` scale, from the core's own fixed-seed
/// xorshift, bounded to `±amplitude`.
///
/// The stream is the one architecture §7.3 already relies on for MOD's random waveform, so
/// this noise is the same on x86, ARM and WASM and a measurement made from it is
/// reproducible rather than merely probable.
pub fn white_noise(frames: usize, amplitude: i32) -> Vec<i32> {
    let mut stream = Xorshift32::new(0x5EED_1234);
    (0..frames)
        .map(|_| {
            let raw = (stream.next_u32() >> 8) as i32; // 24 bits, unsigned
            let span = amplitude.max(1) as i64 * 2 + 1;
            ((raw as i64 * span) >> 24) as i32 - amplitude
        })
        .collect()
}

/// `frames` of a synthetic drum loop on the raw `i16` scale: a half-second bar with a hit
/// on each beat, each hit a noise burst under an exponential decay.
///
/// H4's fixed-versus-float measurements ask for a drum loop rather than white noise
/// because a reverb's recursion amplifies rounding, and what that costs depends on the
/// signal's crest factor: steady noise never leaves the recursion quiet enough for its own
/// error floor to matter, and a drum loop does. Built from the same fixed-seed
/// [`Xorshift32`] [`white_noise`] uses, so the loop is identical on x86, ARM and WASM.
pub fn drum_loop(frames: usize) -> Vec<i32> {
    /// `(offset in the bar as a fraction of 16, peak amplitude, decay time constant in frames)`.
    const HITS: [(usize, i32, f64); 5] = [(0, 26_000, 2_600.0), (4, 13_000, 700.0), (8, 22_000, 2_000.0), (12, 13_000, 700.0), (14, 9_000, 400.0)];
    let bar = (RATE as usize / 2).max(16);
    let mut stream = Xorshift32::new(0x0D12_5EED);
    let mut samples = vec![0i32; frames];
    let mut bar_start = 0usize;
    while bar_start < frames {
        for (sixteenth, peak, decay) in HITS {
            let start = bar_start + sixteenth * bar / 16;
            for offset in 0..frames.saturating_sub(start).min((decay * 6.0) as usize) {
                let envelope = (-(offset as f64) / decay).exp();
                let noise = ((stream.next_u32() >> 8) as f64 / (1u32 << 23) as f64) - 1.0;
                if let Some(slot) = samples.get_mut(start + offset) {
                    *slot = (*slot + (noise * envelope * peak as f64) as i32).clamp(-32_767, 32_767);
                }
            }
        }
        bar_start += bar;
    }
    samples
}

/// Run `input` through `insert` a whole block at a time, both stereo channels carrying the
/// same signal, and hand back the left channel.
///
/// `input.len()` is rounded down to a whole number of blocks, because an insert is only
/// ever handed a whole one.
pub fn render_noise<Sample: DspSample, I: Insert<Sample> + ?Sized>(insert: &mut I, input: &[Sample]) -> Vec<Sample> {
    let mut output = Vec::with_capacity(input.len());
    let mut block = vec![Stereo::new(Sample::ZERO, Sample::ZERO); DSP_BLOCK_FRAMES];
    for chunk in input.chunks_exact(DSP_BLOCK_FRAMES) {
        for (slot, value) in block.iter_mut().zip(chunk.iter()) {
            *slot = Stereo::new(*value, *value);
        }
        insert.process(&mut block);
        output.extend(block.iter().map(|frame| frame.left));
    }
    output
}

/// The same, but keeping both channels — what a stereo effect's tests need.
pub fn render_stereo<Sample: DspSample, I: Insert<Sample> + ?Sized>(insert: &mut I, input: &[Stereo<Sample>]) -> Vec<Stereo<Sample>> {
    let mut output = Vec::with_capacity(input.len());
    let mut block = vec![Stereo::new(Sample::ZERO, Sample::ZERO); DSP_BLOCK_FRAMES];
    for chunk in input.chunks_exact(DSP_BLOCK_FRAMES) {
        block.copy_from_slice(chunk);
        insert.process(&mut block);
        output.extend_from_slice(&block);
    }
    output
}

/// `20·log10(|Wet(f)| / |Dry(f)|)` in decibels, measured over the **last**
/// [`WINDOW_FRAMES`] frames of both signals.
///
/// A ratio of two spectra of the same realisation, not an estimate of one spectrum: the
/// noise cancels and what is left is the filter's own magnitude response, which is why a
/// single 4096-point window is enough to hold ±0.5 dB. The window is Hann — a rectangular
/// one leaks a boosted band into an unboosted measurement point several decibels — and the
/// magnitude is summed in power over five bin-spaced frequencies either side of `hz`, so
/// an unlucky near-null in one bin of the noise cannot dominate.
pub fn dft_band_gain_db(wet: &[f64], dry: &[f64], hz: f64) -> f64 {
    let bin_hz = RATE as f64 / WINDOW_FRAMES as f64;
    let mut wet_power = 0.0f64;
    let mut dry_power = 0.0f64;
    for offset in -2i32..=2 {
        let frequency = hz + offset as f64 * bin_hz;
        wet_power += dft_magnitude(wet, frequency).powi(2);
        dry_power += dft_magnitude(dry, frequency).powi(2);
    }
    if dry_power <= 0.0 { 0.0 } else { 10.0 * (wet_power / dry_power).log10() }
}

/// The summed power of the DFT bins from `hz − bins·Δf` to `hz + bins·Δf`, where `Δf` is
/// [`WINDOW_FRAMES`]'s own bin spacing — how much of a signal's energy sits in a band.
pub fn band_power(samples: &[f64], hz: f64, bins: i32) -> f64 {
    let bin_hz = RATE as f64 / WINDOW_FRAMES as f64;
    (-bins..=bins).map(|offset| dft_magnitude(samples, hz + offset as f64 * bin_hz).powi(2)).sum()
}

/// `|X(f)|` over the last [`WINDOW_FRAMES`] frames, Hann-windowed.
pub fn dft_magnitude(samples: &[f64], hz: f64) -> f64 {
    let start = samples.len().saturating_sub(WINDOW_FRAMES);
    let window = samples.get(start..).unwrap_or(&[]);
    let length = window.len().max(1) as f64;
    let mut real = 0.0f64;
    let mut imaginary = 0.0f64;
    for (index, sample) in window.iter().enumerate() {
        let position = index as f64;
        let taper = 0.5 - 0.5 * (core::f64::consts::TAU * position / length).cos();
        let angle = core::f64::consts::TAU * hz * position / RATE as f64;
        real += sample * taper * angle.cos();
        imaginary -= sample * taper * angle.sin();
    }
    (real * real + imaginary * imaginary).sqrt()
}

/// Frames per segment in [`segmental_snr_db`], matching `starplayer_offline`'s own.
pub const SNR_SEGMENT_FRAMES: usize = 1_024;

/// Average segmental SNR in decibels, treating the fixed path as the reference — a local
/// copy of `starplayer_offline::segmental_snr_db`, which `starplayer-dsp` cannot depend on
/// (it is a `std` crate that sits above this one).
///
/// Both inputs are on the raw `i16` scale, which is what
/// [`DspSample::from_i16`](crate::sample::DspSample::from_i16) puts a sample on. Segments
/// quieter than −80 dBFS are skipped, so a decaying tail cannot dominate the average, and
/// an exact match is capped at 120 dB so it stays finite.
pub fn segmental_snr_db(fixed: &[i32], float: &[f32]) -> Option<f64> {
    if fixed.len() != float.len() || fixed.is_empty() {
        return None;
    }
    let mut total_db = 0.0f64;
    let mut compared = 0usize;
    for (fixed_segment, float_segment) in fixed.chunks(SNR_SEGMENT_FRAMES).zip(float.chunks(SNR_SEGMENT_FRAMES)) {
        let mut signal_energy = 0.0f64;
        let mut error_energy = 0.0f64;
        for (fixed_sample, float_sample) in fixed_segment.iter().zip(float_segment.iter()) {
            let reference = *fixed_sample as f64 * (1.0 / 32_767.0);
            let error = reference - *float_sample as f64 * (1.0 / 32_767.0);
            signal_energy += reference * reference;
            error_energy += error * error;
        }
        if signal_energy / (fixed_segment.len().max(1) as f64) < 1.0e-8 {
            continue;
        }
        total_db += if error_energy == 0.0 { 120.0 } else { (10.0 * (signal_energy / error_energy).log10()).clamp(-20.0, 120.0) };
        compared += 1;
    }
    if compared == 0 { None } else { Some(total_db / compared as f64) }
}
