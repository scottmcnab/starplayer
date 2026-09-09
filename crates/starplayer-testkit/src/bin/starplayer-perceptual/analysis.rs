//! The signal analysis behind the perceptual comparison: RMS normalisation and the two
//! scores T10 asks for.
//!
//! The transform itself, the Hann window and the log-spectral distance live in
//! [`starplayer_offline::analysis`] — M10-K5c gave them a second caller in the
//! enhancement harness, so they moved to the crate both tools already depend on rather
//! than being copied. What stays here is the part that is specific to comparing two
//! *renders*: the RMS/peak normalisation and the [`Scores`] a fixture reports.

use starplayer_offline::analysis::{Fft, SPECTRAL_MAGNITUDE_FLOOR, log_spectral_distance_db};
use starplayer_offline::segmental_snr_db;

/// Segment length for the segmental SNR, in frames of one channel.
pub const SEGMENT_FRAMES: usize = 4_096;

/// Analysis frame length for the log-spectral distance, in frames of one channel.
pub use starplayer_offline::analysis::SPECTRAL_FRAME_FRAMES;

/// Largest sample magnitude either normalised render is allowed to reach.
///
/// Both renders are scaled to the same RMS, then both by one further shared factor so the
/// louder of the two peaks lands here. The shared factor cancels out of every score; what
/// it buys is a reference signal that survives the `i16` quantisation
/// [`segmental_snr_db`] takes without clipping.
const PEAK_CEILING: f64 = 0.999;

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
        let Some(lsd_db) = log_spectral_distance_db(&fft, &reference_channel, &candidate_channel, SPECTRAL_MAGNITUDE_FLOOR) else { continue };

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

#[cfg(test)]
mod tests {
    use super::*;

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
