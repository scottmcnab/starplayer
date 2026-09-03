//! The signal analysis behind the perceptual comparison: RMS normalisation, a small
//! radix-2 FFT and the two scores T10 asks for.
//!
//! Nothing here is real-time code — it runs in a `std` diagnostic binary — so `f64` and
//! `sin`/`cos` are fine. The FFT is written out rather than pulled from a crate for the
//! same reason `xtask` has no dependencies: the accuracy tooling must build on a machine
//! that has never seen crates.io beyond the pinned workspace lock.

use starplayer_offline::segmental_snr_db;

/// Segment length for the segmental SNR, in frames of one channel.
pub const SEGMENT_FRAMES: usize = 4_096;

/// Analysis frame length for the log-spectral distance, in frames of one channel.
pub const SPECTRAL_FRAME_FRAMES: usize = 4_096;

/// 50 % overlap, as the deliverable asks.
pub const SPECTRAL_HOP_FRAMES: usize = SPECTRAL_FRAME_FRAMES / 2;

/// Largest sample magnitude either normalised render is allowed to reach.
///
/// Both renders are scaled to the same RMS, then both by one further shared factor so the
/// louder of the two peaks lands here. The shared factor cancels out of every score; what
/// it buys is a reference signal that survives the `i16` quantisation
/// [`segmental_snr_db`] takes without clipping.
const PEAK_CEILING: f64 = 0.999;

/// Magnitude floor for the log-spectral distance, relative to a full-scale sine's
/// normalised bin magnitude of 0.25. −120 dB, so an empty bin cannot contribute an
/// infinite distance.
const SPECTRAL_MAGNITUDE_FLOOR: f64 = 1.0e-6;

/// Analysis frames quieter than this — in both renders — carry no spectrum worth
/// comparing. It is the same −80 dBFS mean-square gate [`segmental_snr_db`] applies to a
/// segment.
const SPECTRAL_SILENCE_MEAN_SQUARE: f64 = 1.0e-8;

/// What one fixture scored.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Scores {
    /// Average segmental SNR in dB, libopenmpt taken as the reference signal.
    pub segmental_snr_db: f64,
    /// Average log-spectral distance in dB.
    pub log_spectral_distance_db: f64,
    /// StarPlayer's RMS over libopenmpt's, **before** normalisation. A level mismatch is
    /// itself a finding, so it is reported rather than silently divided away.
    pub rms_ratio: f64,
}

/// Score `candidate` (StarPlayer) against `reference` (libopenmpt).
///
/// Both slices are interleaved with `channel_count` channels and must already be trimmed
/// to the same length. Returns `None` when either render is silent or the two are not
/// comparable at all, which is a finding for the caller to report rather than an error.
pub fn compare(candidate: &[f32], reference: &[f32], channel_count: usize) -> Option<Scores> {
    if channel_count == 0 || candidate.len() != reference.len() || candidate.is_empty() {
        return None;
    }

    let candidate_rms = root_mean_square(candidate);
    let reference_rms = root_mean_square(reference);
    if candidate_rms <= 0.0 || reference_rms <= 0.0 {
        return None;
    }
    let rms_ratio = candidate_rms / reference_rms;

    let mut candidate_scale = 1.0 / candidate_rms;
    let mut reference_scale = 1.0 / reference_rms;
    let scaled_peak = (peak(candidate) * candidate_scale).max(peak(reference) * reference_scale);
    if scaled_peak > PEAK_CEILING {
        let shared = PEAK_CEILING / scaled_peak;
        candidate_scale *= shared;
        reference_scale *= shared;
    }

    let fft = Fft::new(SPECTRAL_FRAME_FRAMES);
    let mut total_snr_db = 0.0f64;
    let mut total_lsd_db = 0.0f64;
    let mut scored_channels = 0usize;
    for channel in 0..channel_count {
        let candidate_channel: Vec<f64> = channel_samples(candidate, channel, channel_count, candidate_scale);
        let reference_channel: Vec<f64> = channel_samples(reference, channel, channel_count, reference_scale);

        // `segmental_snr_db` takes its reference as `i16` — it was written for the
        // float-versus-fixed check. Quantising libopenmpt's normalised render into that
        // domain reuses it unchanged; at a peak of −0.01 dBFS the quantisation floor it
        // adds sits near 90 dB SNR, four decades above the scores this comparison is
        // looking for.
        let quantised_reference: Vec<i16> = reference_channel.iter().map(|&sample| quantise(sample)).collect();
        let candidate_f32: Vec<f32> = candidate_channel.iter().map(|&sample| sample as f32).collect();
        let Some(snr_db) = segmental_snr_db(&quantised_reference, &candidate_f32, SEGMENT_FRAMES) else { continue };
        let Some(lsd_db) = log_spectral_distance_db(&fft, &reference_channel, &candidate_channel) else { continue };

        total_snr_db += snr_db;
        total_lsd_db += lsd_db;
        scored_channels += 1;
    }

    if scored_channels == 0 {
        return None;
    }
    Some(Scores {
        segmental_snr_db: total_snr_db / scored_channels as f64,
        log_spectral_distance_db: total_lsd_db / scored_channels as f64,
        rms_ratio,
    })
}

/// Root mean square of an interleaved buffer, in the ±1 sample domain.
pub fn root_mean_square(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let energy: f64 = samples.iter().map(|&sample| sample as f64 * sample as f64).sum();
    (energy / samples.len() as f64).sqrt()
}

/// Largest absolute sample value.
fn peak(samples: &[f32]) -> f64 {
    samples.iter().fold(0.0f64, |largest, &sample| largest.max((sample as f64).abs()))
}

/// One channel of an interleaved buffer, scaled.
fn channel_samples(samples: &[f32], channel: usize, channel_count: usize, scale: f64) -> Vec<f64> {
    samples.iter().skip(channel).step_by(channel_count).map(|&sample| sample as f64 * scale).collect()
}

/// The ±1 domain into the `i16` domain `segmental_snr_db` reads back as `value / 32_767`.
fn quantise(sample: f64) -> i16 {
    (sample * 32_767.0).round().clamp(-32_768.0, 32_767.0) as i16
}

/// Mean over analysis frames of the RMS difference of the two log-magnitude spectra, in
/// dB. Returns `None` when both signals are silent everywhere or are shorter than one
/// analysis frame.
pub fn log_spectral_distance_db(fft: &Fft, reference: &[f64], candidate: &[f64]) -> Option<f64> {
    let length = reference.len().min(candidate.len());
    if length < fft.size {
        return None;
    }
    let window = hann_window(fft.size);
    let bin_count = fft.size / 2 + 1;
    let normalisation = fft.size as f64;

    let mut total_db = 0.0f64;
    let mut compared_frames = 0usize;
    let mut start = 0usize;
    while start + fft.size <= length {
        let reference_frame = &reference[start..start + fft.size];
        let candidate_frame = &candidate[start..start + fft.size];
        start += SPECTRAL_HOP_FRAMES;

        if mean_square(reference_frame) < SPECTRAL_SILENCE_MEAN_SQUARE && mean_square(candidate_frame) < SPECTRAL_SILENCE_MEAN_SQUARE {
            continue;
        }

        let reference_magnitudes = fft.windowed_magnitudes(reference_frame, &window, normalisation);
        let candidate_magnitudes = fft.windowed_magnitudes(candidate_frame, &window, normalisation);
        let mut squared_difference = 0.0f64;
        for bin in 0..bin_count {
            let reference_db = 20.0 * reference_magnitudes[bin].max(SPECTRAL_MAGNITUDE_FLOOR).log10();
            let candidate_db = 20.0 * candidate_magnitudes[bin].max(SPECTRAL_MAGNITUDE_FLOOR).log10();
            let difference = reference_db - candidate_db;
            squared_difference += difference * difference;
        }
        total_db += (squared_difference / bin_count as f64).sqrt();
        compared_frames += 1;
    }

    if compared_frames == 0 { None } else { Some(total_db / compared_frames as f64) }
}

fn mean_square(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.iter().map(|&sample| sample * sample).sum::<f64>() / samples.len() as f64
}

/// Periodic Hann window.
fn hann_window(size: usize) -> Vec<f64> {
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
    fn windowed_magnitudes(&self, samples: &[f64], window: &[f64], normalisation: f64) -> Vec<f64> {
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
        // A deterministic, spectrally busy signal — not a single tone, so a wrong twiddle
        // cannot cancel out of the sum.
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

    /// The scores of a signal against itself: infinite SNR (capped at 120 dB by
    /// `segmental_snr_db`), zero spectral distance, unit level ratio.
    #[test]
    fn a_render_compared_against_itself_scores_perfectly() {
        let frames = SPECTRAL_FRAME_FRAMES * 4;
        let samples: Vec<f32> = (0..frames * 2)
            .map(|index| (0.4 * (index as f64 * 0.013).sin() + 0.2 * (index as f64 * 0.0031).cos()) as f32)
            .collect();
        let scores = compare(&samples, &samples, 2).expect("a non-silent render scores");

        assert!((scores.rms_ratio - 1.0).abs() < 1.0e-9, "level ratio {}", scores.rms_ratio);
        assert!(scores.log_spectral_distance_db < 1.0e-9, "spectral distance {}", scores.log_spectral_distance_db);
        assert!(scores.segmental_snr_db > 80.0, "segmental SNR {}", scores.segmental_snr_db);
    }

    /// A level difference alone is reported as a ratio and normalised out of the scores.
    #[test]
    fn a_pure_level_difference_shows_up_only_in_the_ratio() {
        let frames = SPECTRAL_FRAME_FRAMES * 4;
        let reference: Vec<f32> = (0..frames * 2).map(|index| (0.3 * (index as f64 * 0.011).sin()) as f32).collect();
        let candidate: Vec<f32> = reference.iter().map(|&sample| sample * 0.5).collect();
        let scores = compare(&candidate, &reference, 2).expect("a non-silent render scores");

        assert!((scores.rms_ratio - 0.5).abs() < 1.0e-6, "level ratio {}", scores.rms_ratio);
        assert!(scores.log_spectral_distance_db < 1.0e-6, "spectral distance {}", scores.log_spectral_distance_db);
        assert!(scores.segmental_snr_db > 80.0, "segmental SNR {}", scores.segmental_snr_db);
    }

    /// Two unrelated signals score badly on both metrics. The point of the test is that
    /// the scores are finite and ordered, not the exact numbers.
    #[test]
    fn unrelated_renders_score_badly() {
        let frames = SPECTRAL_FRAME_FRAMES * 4;
        let reference: Vec<f32> = (0..frames * 2).map(|index| (0.3 * (index as f64 * 0.011).sin()) as f32).collect();
        let candidate: Vec<f32> = (0..frames * 2).map(|index| (0.3 * (index as f64 * 0.37).sin()) as f32).collect();
        let scores = compare(&candidate, &reference, 2).expect("a non-silent render scores");

        assert!(scores.segmental_snr_db < 6.0, "segmental SNR {}", scores.segmental_snr_db);
        assert!(scores.log_spectral_distance_db > 10.0, "spectral distance {}", scores.log_spectral_distance_db);
    }
}
