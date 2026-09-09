//! Spectral analysis shared by the diagnostic tools: a small radix-2 FFT, the
//! log-spectral distance, and the band-limited SNR the enhancement harness measures with.
//!
//! Nothing here is real-time code — it runs in `std` tools and tests — so `f64`,
//! `sin`/`cos` and `log10` are fine. The FFT is written out rather than pulled from a
//! crate for the same reason `xtask` has no dependencies: the accuracy tooling must build
//! on a machine that has never seen crates.io beyond the pinned workspace lock.
//!
//! This module was lifted out of `starplayer-testkit`'s `starplayer-perceptual` binary by
//! M10-K5c, which needs the same transform and the same distance from a second caller —
//! `examples/enhance_report.rs` and `tests/enhance_quality.rs`. The binary now reads it
//! from here rather than owning a private copy.

/// Analysis frame length for the log-spectral distance, in frames of one channel.
pub const SPECTRAL_FRAME_FRAMES: usize = 4_096;

/// 50 % overlap.
pub const SPECTRAL_HOP_FRAMES: usize = SPECTRAL_FRAME_FRAMES / 2;

/// Magnitude floor for the log-spectral distance, relative to a full-scale sine's
/// normalised bin magnitude of 0.25. −120 dB, so an empty bin cannot contribute an
/// infinite distance. This is the floor the libopenmpt comparison uses, where the
/// question is "do two engines agree" and a difference nobody can hear is still a
/// difference in the arithmetic.
pub const SPECTRAL_MAGNITUDE_FLOOR: f64 = 1.0e-6;

/// Analysis frames quieter than this — in both signals — carry no spectrum worth
/// comparing. It is the same −80 dBFS mean-square gate
/// [`segmental_snr_db`](crate::segmental_snr_db) applies to a segment.
pub const SPECTRAL_SILENCE_MEAN_SQUARE: f64 = 1.0e-8;

/// How far below the reference's loudest bin a comparison stops caring what is in a bin:
/// **−60 dB**, the audibility floor M10-K5c's enhancement harness scores against.
///
/// # Why it is measured from the signal rather than from the number `1.0`
///
/// "Sixty decibels below full scale" is the intent, and stating it as an absolute
/// magnitude does not deliver it. These magnitudes are normalised so that a **full-scale
/// sine** puts 0.25 in one bin; a real instrument spreads the same energy over thousands
/// of bins, so its per-bin magnitudes sit 30 to 40 dB below its own level before anything
/// has been lost. Measured on this harness's four instruments, an absolute floor of
/// `0.25 × 10⁻³` left **0.3 % to 7.2 %** of the reference's bins above it — it did not
/// stop inaudible differences dominating the score, it stopped the score being a
/// measurement at all.
///
/// Anchoring the same −60 dB to the loudest bin of the reference gives the intended
/// dynamic range, is what "full scale" means for a signal that is not at full scale, and
/// is independent of the transform length. [`SPECTRAL_MAGNITUDE_FLOOR`] stays absolute
/// because the libopenmpt comparison normalises both renders to a fixed peak first.
pub const AUDIBLE_FLOOR_BELOW_PEAK_DB: f64 = -60.0;

/// The magnitude floor [`AUDIBLE_FLOOR_BELOW_PEAK_DB`] asks for, against `reference`.
///
/// Returns zero for a silent reference, which floors nothing and lets the distance speak
/// for itself.
pub fn audible_magnitude_floor(fft: &Fft, reference: &[f64]) -> f64 {
    let window = hann_window(fft.size());
    let hop = fft.size() / 2;
    let mut peak = 0.0f64;
    let mut start = 0usize;
    while start + fft.size() <= reference.len() {
        for magnitude in fft.windowed_magnitudes(&reference[start..start + fft.size()], &window, fft.size() as f64) {
            if magnitude > peak {
                peak = magnitude;
            }
        }
        start += hop;
    }
    peak * 10.0f64.powf(AUDIBLE_FLOOR_BELOW_PEAK_DB / 20.0)
}

/// Mean over analysis frames of the RMS difference of the two log-magnitude spectra, in
/// dB. Returns `None` when both signals are silent everywhere or are shorter than one
/// analysis frame.
///
/// The hop is half the transform, so the caller picks the resolution by picking the
/// [`Fft`] it hands in, and `magnitude_floor` is how quiet a bin has to be before the
/// caller stops caring what is in it — [`SPECTRAL_MAGNITUDE_FLOOR`] for an
/// engine-against-engine comparison, [`AUDIBLE_MAGNITUDE_FLOOR`] for a
/// does-this-sound-closer one.
pub fn log_spectral_distance_db(fft: &Fft, reference: &[f64], candidate: &[f64], magnitude_floor: f64) -> Option<f64> {
    let length = reference.len().min(candidate.len());
    if length < fft.size() {
        return None;
    }
    let window = hann_window(fft.size());
    let bin_count = fft.size() / 2 + 1;
    let normalisation = fft.size() as f64;
    let hop = fft.size() / 2;

    let mut total_db = 0.0f64;
    let mut compared_frames = 0usize;
    let mut start = 0usize;
    while start + fft.size() <= length {
        let reference_frame = &reference[start..start + fft.size()];
        let candidate_frame = &candidate[start..start + fft.size()];
        start += hop;

        if mean_square(reference_frame) < SPECTRAL_SILENCE_MEAN_SQUARE && mean_square(candidate_frame) < SPECTRAL_SILENCE_MEAN_SQUARE {
            continue;
        }

        let reference_magnitudes = fft.windowed_magnitudes(reference_frame, &window, normalisation);
        let candidate_magnitudes = fft.windowed_magnitudes(candidate_frame, &window, normalisation);
        let mut squared_difference = 0.0f64;
        for bin in 0..bin_count {
            let reference_db = 20.0 * reference_magnitudes[bin].max(magnitude_floor).log10();
            let candidate_db = 20.0 * candidate_magnitudes[bin].max(magnitude_floor).log10();
            let difference = reference_db - candidate_db;
            squared_difference += difference * difference;
        }
        total_db += (squared_difference / bin_count as f64).sqrt();
        compared_frames += 1;
    }

    if compared_frames == 0 { None } else { Some(total_db / compared_frames as f64) }
}

/// Segmental SNR restricted to the bins **below** `band_edge_hz` (M10-K5c).
///
/// `segmental_snr_db` answers "how much of the reference survived" over the whole band,
/// which on an 8 kHz-sourced sample is dominated by the band the source never had. This
/// answers the other half — how much of the band the source *did* have survived — by
/// transforming the reference and the error signal frame by frame and summing only the
/// bins the caller asked for. The error is `reference − candidate`, so a candidate that
/// adds energy above the edge is not penalised here and a candidate that damages the
/// low band is.
///
/// Per-frame scores are clamped to `−20 ..= 120` dB and frames whose in-band reference
/// energy is below the silence gate are skipped, exactly as
/// [`segmental_snr_db`](crate::segmental_snr_db) does, so a long quiet tail cannot make
/// the average meaningless. Returns `None` when the two signals differ in length, are
/// shorter than one transform, or are silent throughout.
pub fn band_limited_snr_db(fft: &Fft, reference: &[f64], candidate: &[f64], sample_rate_hz: f64, band_edge_hz: f64) -> Option<f64> {
    if reference.len() != candidate.len() || reference.len() < fft.size() || sample_rate_hz <= 0.0 {
        return None;
    }
    let window = hann_window(fft.size());
    let normalisation = fft.size() as f64;
    let hop = fft.size() / 2;
    // The highest bin whose centre frequency is at or below the edge.
    let bin_hz = sample_rate_hz / fft.size() as f64;
    let last_bin = ((band_edge_hz / bin_hz).floor() as usize).min(fft.size() / 2);

    let mut total_db = 0.0f64;
    let mut compared_frames = 0usize;
    let mut start = 0usize;
    while start + fft.size() <= reference.len() {
        let reference_frame = &reference[start..start + fft.size()];
        let candidate_frame = &candidate[start..start + fft.size()];
        start += hop;

        let error_frame: Vec<f64> = reference_frame.iter().zip(candidate_frame.iter()).map(|(&wanted, &got)| wanted - got).collect();
        let reference_magnitudes = fft.windowed_magnitudes(reference_frame, &window, normalisation);
        let error_magnitudes = fft.windowed_magnitudes(&error_frame, &window, normalisation);

        let mut signal_energy = 0.0f64;
        let mut error_energy = 0.0f64;
        for bin in 0..=last_bin {
            signal_energy += reference_magnitudes[bin] * reference_magnitudes[bin];
            error_energy += error_magnitudes[bin] * error_magnitudes[bin];
        }
        if signal_energy < SPECTRAL_SILENCE_MEAN_SQUARE {
            continue;
        }
        let frame_db = if error_energy == 0.0 { 120.0 } else { (10.0 * (signal_energy / error_energy).log10()).clamp(-20.0, 120.0) };
        total_db += frame_db;
        compared_frames += 1;
    }

    if compared_frames == 0 { None } else { Some(total_db / compared_frames as f64) }
}

/// Mean square of a slice, zero for an empty one.
pub fn mean_square(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().map(|&sample| sample * sample).sum::<f64>() / samples.len() as f64
}

/// Periodic Hann window.
pub fn hann_window(size: usize) -> Vec<f64> {
    (0..size)
        .map(|index| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * index as f64 / size as f64).cos())
        .collect()
}

/// An in-place radix-2 decimation-in-time FFT of a fixed power-of-two size.
///
/// The twiddle factors are computed once per instance: computing them per butterfly would
/// mean millions of `sin` calls per fixture, and accumulating them by recurrence loses
/// enough precision to be visible in a Parseval check.
pub struct Fft {
    size: usize,
    twiddle_real: Vec<f64>,
    twiddle_imaginary: Vec<f64>,
}

impl Fft {
    /// A transform of `size` points. `size` must be a power of two of at least two.
    pub fn new(size: usize) -> Fft {
        assert!(size >= 2 && size.is_power_of_two(), "the FFT size must be a power of two");
        let mut twiddle_real = Vec::with_capacity(size / 2);
        let mut twiddle_imaginary = Vec::with_capacity(size / 2);
        for index in 0..size / 2 {
            let angle = -2.0 * std::f64::consts::PI * index as f64 / size as f64;
            twiddle_real.push(angle.cos());
            twiddle_imaginary.push(angle.sin());
        }
        Fft { size, twiddle_real, twiddle_imaginary }
    }

    /// Points this transform takes.
    pub fn size(&self) -> usize { self.size }

    /// Transform `real` and `imaginary` in place, both of length [`Fft::size`].
    pub fn forward(&self, real: &mut [f64], imaginary: &mut [f64]) {
        assert_eq!(real.len(), self.size, "the real part must be one transform long");
        assert_eq!(imaginary.len(), self.size, "the imaginary part must be one transform long");

        // Decimation in time needs the input in bit-reversed order.
        let mut destination = 0usize;
        for source in 1..self.size {
            let mut bit = self.size >> 1;
            while destination & bit != 0 {
                destination ^= bit;
                bit >>= 1;
            }
            destination |= bit;
            if source < destination {
                real.swap(source, destination);
                imaginary.swap(source, destination);
            }
        }

        let mut span = 2usize;
        while span <= self.size {
            let half = span / 2;
            let stride = self.size / span;
            let mut start = 0usize;
            while start < self.size {
                for offset in 0..half {
                    let twiddle_index = offset * stride;
                    let cosine = self.twiddle_real[twiddle_index];
                    let sine = self.twiddle_imaginary[twiddle_index];
                    let even = start + offset;
                    let odd = even + half;
                    let product_real = real[odd] * cosine - imaginary[odd] * sine;
                    let product_imaginary = real[odd] * sine + imaginary[odd] * cosine;
                    real[odd] = real[even] - product_real;
                    imaginary[odd] = imaginary[even] - product_imaginary;
                    real[even] += product_real;
                    imaginary[even] += product_imaginary;
                }
                start += span;
            }
            span <<= 1;
        }
    }

    /// Magnitude spectrum of `samples * window`, divided by `normalisation`, over the
    /// non-negative frequency bins.
    pub fn windowed_magnitudes(&self, samples: &[f64], window: &[f64], normalisation: f64) -> Vec<f64> {
        let mut real: Vec<f64> = samples.iter().zip(window.iter()).map(|(&sample, &weight)| sample * weight).collect();
        let mut imaginary = vec![0.0f64; self.size];
        self.forward(&mut real, &mut imaginary);
        (0..self.size / 2 + 1)
            .map(|bin| (real[bin] * real[bin] + imaginary[bin] * imaginary[bin]).sqrt() / normalisation)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cosine at exactly bin `k` must put all of its energy in bins `k` and `size - k`.
    #[test]
    fn a_pure_tone_lands_in_one_bin() {
        let size = 64;
        let bin = 7usize;
        let fft = Fft::new(size);
        let mut real: Vec<f64> = (0..size).map(|index| (2.0 * std::f64::consts::PI * bin as f64 * index as f64 / size as f64).cos()).collect();
        let mut imaginary = vec![0.0f64; size];
        fft.forward(&mut real, &mut imaginary);

        for index in 0..size {
            let magnitude = (real[index] * real[index] + imaginary[index] * imaginary[index]).sqrt();
            if index == bin || index == size - bin {
                assert!((magnitude - size as f64 / 2.0).abs() < 1.0e-9, "bin {index} carries {magnitude}");
            } else {
                assert!(magnitude < 1.0e-9, "bin {index} should be empty, carries {magnitude}");
            }
        }
    }

    /// Parseval: the energy of the samples equals the energy of the spectrum over the
    /// transform length.
    #[test]
    fn the_transform_preserves_energy_within_parseval() {
        let size = 256;
        let fft = Fft::new(size);
        let samples: Vec<f64> = (0..size)
            .map(|index| {
                let position = index as f64;
                (0.31 * position).sin() + 0.5 * (1.7 * position + 0.4).cos() + 0.25 * (0.07 * position).sin()
            })
            .collect();
        let time_energy: f64 = samples.iter().map(|&sample| sample * sample).sum();

        let mut real = samples.clone();
        let mut imaginary = vec![0.0f64; size];
        fft.forward(&mut real, &mut imaginary);
        let spectrum_energy: f64 = real.iter().zip(imaginary.iter()).map(|(&re, &im)| re * re + im * im).sum::<f64>() / size as f64;

        assert!((time_energy - spectrum_energy).abs() < 1.0e-6, "time {time_energy} versus spectrum {spectrum_energy}");
    }

    /// A signal against itself is a perfect score on both metrics.
    #[test]
    fn a_signal_against_itself_scores_perfectly() {
        let fft = Fft::new(1_024);
        let samples: Vec<f64> = (0..8_192).map(|index| 0.4 * (index as f64 * 0.013).sin() + 0.2 * (index as f64 * 0.0031).cos()).collect();
        assert!(log_spectral_distance_db(&fft, &samples, &samples, SPECTRAL_MAGNITUDE_FLOOR).expect("a score") < 1.0e-9);
        assert_eq!(band_limited_snr_db(&fft, &samples, &samples, 44_100.0, 4_181.0), Some(120.0));
    }

    /// The band limit is what makes the in-band score different from the whole-band one:
    /// damage that lives entirely above the edge does not show up below it.
    #[test]
    fn damage_above_the_edge_does_not_count_against_the_in_band_score() {
        let fft = Fft::new(1_024);
        let rate = 44_100.0;
        // A 500 Hz tone, and the same tone with a 10 kHz tone added at a quarter of the
        // level. Below 4 kHz the two are the same signal.
        let reference: Vec<f64> = (0..8_192).map(|index| (2.0 * std::f64::consts::PI * 500.0 * index as f64 / rate).sin()).collect();
        let candidate: Vec<f64> = reference.iter().enumerate()
            .map(|(index, &sample)| sample + 0.25 * (2.0 * std::f64::consts::PI * 10_000.0 * index as f64 / rate).sin())
            .collect();

        let in_band = band_limited_snr_db(&fft, &reference, &candidate, rate, 4_181.0).expect("a score");
        let whole_band = band_limited_snr_db(&fft, &reference, &candidate, rate, rate / 2.0).expect("a score");
        assert!(in_band > 60.0, "the low band is untouched, so it should score near-perfectly: {in_band} dB");
        assert!(whole_band < 20.0, "over the whole band the added tone is plainly visible: {whole_band} dB");
    }

    /// The audible floor stops a difference nobody can hear from dominating the score.
    #[test]
    fn a_difference_below_the_audible_floor_costs_nothing_under_it_and_plenty_under_the_other() {
        let fft = Fft::new(1_024);
        let rate = 44_100.0;
        let reference: Vec<f64> = (0..8_192).map(|index| 0.5 * (2.0 * std::f64::consts::PI * 500.0 * index as f64 / rate).sin()).collect();
        // The same signal plus a 10 kHz tone 80 dB below it: inaudible, 20 dB below the
        // audible floor, and 40 dB above the arithmetic one.
        let candidate: Vec<f64> = reference.iter().enumerate()
            .map(|(index, &sample)| sample + 5.0e-5 * (2.0 * std::f64::consts::PI * 10_000.0 * index as f64 / rate).sin())
            .collect();

        let audible = log_spectral_distance_db(&fft, &reference, &candidate, audible_magnitude_floor(&fft, &reference)).expect("a score");
        let arithmetic = log_spectral_distance_db(&fft, &reference, &candidate, SPECTRAL_MAGNITUDE_FLOOR).expect("a score");
        assert!(audible < 0.5, "an inaudible addition scored {audible} dB under the audible floor");
        assert!(arithmetic > audible * 4.0, "the arithmetic floor should charge far more for it: {arithmetic} against {audible}");
    }

    /// Two unrelated signals score badly, and the scores are finite.
    #[test]
    fn unrelated_signals_score_badly() {
        let fft = Fft::new(1_024);
        let reference: Vec<f64> = (0..8_192).map(|index| 0.3 * (index as f64 * 0.011).sin()).collect();
        let candidate: Vec<f64> = (0..8_192).map(|index| 0.3 * (index as f64 * 0.37).sin()).collect();
        assert!(log_spectral_distance_db(&fft, &reference, &candidate, SPECTRAL_MAGNITUDE_FLOOR).expect("a score") > 10.0);
        assert!(band_limited_snr_db(&fft, &reference, &candidate, 44_100.0, 22_050.0).expect("a score") < 6.0);
    }
}
