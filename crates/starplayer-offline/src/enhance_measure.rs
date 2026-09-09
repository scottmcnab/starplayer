//! M10-K5c's measurement harness: four synthetic test instruments, the 8-bit/8 kHz
//! degradation every tracker sample has already been through, and the three scores that
//! say whether a load-time enhancer put anything back.
//!
//! # Why the instruments are synthesised rather than loaded
//!
//! An enhancer can only be judged against a **ground truth**, and no tracker module
//! carries one: a module's samples are already 8-bit and already band-limited, so there is
//! nothing to compare a rebuild against. So the harness starts from four 16-bit 44 100 Hz
//! instruments generated in code, degrades each one into exactly the shape an S3M sample
//! has — band-limited, decimated to 8 363 Hz, rounded to eight bits and widened by 256 —
//! and asks how close an enhanced rebuild of the degraded version gets back to the
//! original.
//!
//! # Why the comparison runs through the engine
//!
//! Both versions are wrapped in a one-channel, one-sample IT module
//! ([`crate::fixtures::synthetic_it_single_sample`]) and rendered by
//! [`crate::render_song_with_options`] at 44 100 Hz on the fixed path with the linear
//! kernel. That is not ceremony: an enhancer changes a sample's *rate*, and the only place
//! a rate becomes audible is the mixer's resampling step. Comparing two `Vec<i16>` in
//! isolation would score the enhancer's own arithmetic rather than what a listener hears.
//! IT is the format that makes it possible, because its `C5Speed` is an arbitrary rate:
//! at 44 100 Hz the ground truth plays at unity step and reaches the comparison
//! unresampled, while the degraded sample walks the same path a real module's does.
//!
//! # The three scores
//!
//! * **Full-band segmental SNR** ([`crate::segmental_snr_db`]) — the whole picture,
//!   dominated on an 8 kHz-sourced sample by the band the source never had.
//! * **In-band SNR** ([`crate::analysis::band_limited_snr_db`]) below
//!   [`DEGRADED_NYQUIST_HZ`] — what survived inside the band the source *did* have. This
//!   is where a denoiser has to show up, and where it must not do damage.
//! * **Log-spectral distance** ([`crate::analysis::log_spectral_distance_db`]) — the
//!   shape of the spectrum, which is where a bandwidth extender has to show up. Floored
//!   [`AUDIBLE_FLOOR_BELOW_PEAK_DB`](crate::analysis::AUDIBLE_FLOOR_BELOW_PEAK_DB) below
//!   the ground truth's own loudest bin, rather than at the −120 dB the libopenmpt
//!   comparison uses: this harness is asking whether something sounds closer, and a bin
//!   filled a hundred decibels down is not a difference anybody can hear.
//!
//! The first two are also reported over the **decay window** alone — see [`tail_window`] —
//! because that is the only stretch of an 8-bit decay where a denoiser has anything to
//! work with.
//!
//! Nothing in this module runs on the audio thread or ships to a host, so `sin`, `cos` and
//! `powf` are all fine here. The enhancers being measured may use none of them.

use starplayer::core::AtEnd;
use starplayer::dsp::Linear;
use starplayer::mixer::{FixedPath, MonoI16};
use starplayer::model::SampleEnhancer;

use crate::analysis::{Fft, audible_magnitude_floor, band_limited_snr_db, log_spectral_distance_db};
use crate::{GoldenFormat, RenderError, RenderLength, RenderOptions, SEGMENTAL_SNR_FRAMES, render_song_with_options, segmental_snr_db};

/// The rate the ground-truth instruments are synthesised at, and the rate every render in
/// the harness runs at, so the ground truth plays at unity step.
pub const GROUND_TRUTH_RATE_HZ: u32 = 44_100;

/// The rate the degraded instruments are decimated to: the tracker default, and the rate
/// every sample in the repository's own S3M fixtures sits at.
pub const DEGRADED_RATE_HZ: u32 = 8_363;

/// Half [`DEGRADED_RATE_HZ`] — the edge of the band an 8 363 Hz sample can hold, and the
/// edge the in-band SNR is measured below.
pub const DEGRADED_NYQUIST_HZ: f64 = DEGRADED_RATE_HZ as f64 / 2.0;

/// Cutoff of the decimation filter, as a fraction of [`DEGRADED_NYQUIST_HZ`].
///
/// 0.92 rather than the upsampler's 0.9: the point of the degradation is to model a real
/// sampler's band limit, and leaving the top of the band clear of the filter's own
/// transition keeps the octave below the edge — which is what a bandwidth extender
/// measures its tilt over — representative of the material rather than of the filter.
const DECIMATION_CUTOFF_FRACTION: f64 = 0.92;

/// Taps in the decimation filter.
///
/// Five hundred and twelve, not sixty-four. The instruments carry harmonics up to
/// [`INSTRUMENT_BANDWIDTH_HZ`], and a Kaiser-windowed sinc's transition width is inversely
/// proportional to its length: at 64 taps the transition is about 4.4 kHz wide at
/// 44 100 Hz, so everything from 4 kHz to 8 kHz would fold back into the degraded sample
/// and the harness would be measuring **aliasing** rather than the loss of a band. At 512
/// taps it is 550 Hz wide, which fits between the 3 847 Hz cutoff and the 4 181 Hz Nyquist
/// with a hundred decibels to spare.
const DECIMATION_TAPS: usize = 512;

/// Taps in the 3 kHz low-pass instrument (d) is made with — unchanged, and deliberately
/// modest: (d) is the "tape-sourced, oversampled" case, whose roll-off is gentle rather
/// than a wall.
const DARK_LOW_PASS_TAPS: usize = 127;

/// FFT size for the in-band SNR. 1 024 points at 44 100 Hz is 23 ms and 43 Hz — short
/// enough that a decaying tail is scored over dozens of frames rather than a handful.
const IN_BAND_FFT_SIZE: usize = 1_024;

/// FFT size for the log-spectral distance, matching `starplayer-perceptual`'s.
const SPECTRAL_FFT_SIZE: usize = 4_096;

/// The four test instruments.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Instrument {
    /// (a) A plucked harmonic decay: harmonics of 700 Hz at `1/n` **up to 20 kHz**,
    /// falling to −60 dB over 1.5 s.
    ///
    /// The harmonic count matters more than it looks. An earlier version of this harness
    /// stopped at eight harmonics, so the ground truth itself was silent above 5.6 kHz —
    /// and a bandwidth extender was then scored as *wrong* for putting anything there,
    /// against a truth no real instrument resembles. A plucked string has harmonics all
    /// the way up; twenty kilohertz is where hearing stops, so that is where the
    /// instrument stops.
    PluckedDecay,
    /// (b) A sustained looped tone with slow vibrato and slight inharmonicity, its
    /// stretched partials running **up to 20 kHz** for the same reason (a)'s harmonics do.
    ///
    /// Every partial's frequency is a multiple of 0.5 Hz and the vibrato is 5 Hz, so the
    /// whole two-second body is exactly one period and the loop is seamless before
    /// anything touches it.
    SustainedTone,
    /// (c) A noise-burst drum: low-passed noise from a seeded xorshift, falling to −60 dB
    /// over 80 ms. The instrument a denoiser is most likely to damage, because its attack
    /// is one block long and its decay is faster than any gain smoother.
    NoiseDrum,
    /// (d) Instrument (a) low-passed at 3 kHz — the "tape-sourced, oversampled" case,
    /// genuinely dark rather than band-limited by its sample rate. A bandwidth extender
    /// that invents a top octave here is inventing one that was never recorded.
    DarkPluck,
}

impl Instrument {
    /// Every instrument, in the order the report tabulates them.
    pub const ALL: [Instrument; 4] = [Instrument::PluckedDecay, Instrument::SustainedTone, Instrument::NoiseDrum, Instrument::DarkPluck];

    /// The letter the task file and the report name this instrument by.
    pub const fn letter(self) -> char {
        match self {
            Instrument::PluckedDecay => 'a',
            Instrument::SustainedTone => 'b',
            Instrument::NoiseDrum => 'c',
            Instrument::DarkPluck => 'd',
        }
    }

    /// A short description for the report's table.
    pub const fn label(self) -> &'static str {
        match self {
            Instrument::PluckedDecay => "plucked decay",
            Instrument::SustainedTone => "sustained loop",
            Instrument::NoiseDrum => "noise drum",
            Instrument::DarkPluck => "dark pluck (3 kHz)",
        }
    }

    /// Frames of ground truth this instrument holds, at [`GROUND_TRUTH_RATE_HZ`].
    pub const fn ground_truth_frames(self) -> usize {
        match self {
            Instrument::PluckedDecay | Instrument::DarkPluck => 66_150,
            Instrument::SustainedTone => 88_200,
            Instrument::NoiseDrum => 11_025,
        }
    }

    /// Output frames the harness renders and compares for this instrument.
    ///
    /// The ground truth's own length: a one-shot has nothing to say past its end, and the
    /// looped tone is compared over exactly one period so the comparison is not dominated
    /// by whichever copy of the loop it happened to reach.
    pub const fn render_frames(self) -> usize { self.ground_truth_frames() }
}

/// One instrument's PCM, at the rate it was built for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestSample {
    /// The frames themselves.
    pub frames: Vec<i16>,
    /// The rate they represent.
    pub rate_hz: u32,
    /// A forward loop over `[start, end)`, or `None` for a one-shot.
    pub loop_span: Option<(u32, u32)>,
}

/// The 16-bit 44 100 Hz ground truth for one instrument.
pub fn ground_truth(instrument: Instrument) -> TestSample {
    let frames = match instrument {
        Instrument::PluckedDecay => plucked_decay(),
        Instrument::SustainedTone => sustained_tone(),
        Instrument::NoiseDrum => noise_drum(),
        Instrument::DarkPluck => low_pass(&plucked_decay(), 3_000.0, GROUND_TRUTH_RATE_HZ as f64),
    };
    let loop_span = match instrument {
        Instrument::SustainedTone => Some((0, frames.len() as u32)),
        _ => None,
    };
    TestSample { frames, rate_hz: GROUND_TRUTH_RATE_HZ, loop_span }
}

/// The same instrument as a tracker would carry it: band-limited, decimated to
/// [`DEGRADED_RATE_HZ`], rounded to eight bits and widened by 256.
///
/// The widening is what makes every frame a multiple of 256, which is exactly the shape
/// `REFLEX.S3M` and `PETRI.S3M` have and the signature `DecayDenoiser` reads its noise
/// floor from.
pub fn degraded(instrument: Instrument) -> TestSample {
    let source = ground_truth(instrument);
    let periodic = source.loop_span.is_some();
    let decimated = decimate(&source.frames, GROUND_TRUTH_RATE_HZ as f64, DEGRADED_RATE_HZ as f64, periodic);
    let quantised: Vec<i16> = decimated.iter().map(|frame| quantise_to_eight_bits(*frame)).collect();
    let loop_span = source.loop_span.map(|_| (0, quantised.len() as u32));
    TestSample { frames: quantised, rate_hz: DEGRADED_RATE_HZ, loop_span }
}

/// Render one sample through the real pipeline: a one-channel IT playing it once at C-5,
/// at [`GROUND_TRUTH_RATE_HZ`], on the fixed path with the linear kernel.
pub fn render(sample: &TestSample, enhancer: Option<&dyn SampleEnhancer>, frames: usize) -> Result<Vec<i16>, RenderError> {
    let bytes = crate::fixtures::synthetic_it_single_sample(&sample.frames, sample.rate_hz, sample.loop_span);
    let length = RenderLength { repeat_count: 0, at_end: AtEnd::Stop, fade_frames: 0, max_frames: frames as u64 };
    let options = RenderOptions { inserts: &[], enhancer };
    let rendered = render_song_with_options::<FixedPath, Linear, MonoI16>(GoldenFormat::It, &bytes, GROUND_TRUTH_RATE_HZ, 128, length, &options)?;
    Ok(rendered)
}

/// What one instrument-and-chain pair scored.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Metrics {
    /// Average segmental SNR over the whole band, in dB.
    pub full_band_snr_db: f64,
    /// Average SNR below [`DEGRADED_NYQUIST_HZ`], in dB.
    pub in_band_snr_db: f64,
    /// Average log-spectral distance, in dB, floored
    /// [`AUDIBLE_FLOOR_BELOW_PEAK_DB`](crate::analysis::AUDIBLE_FLOOR_BELOW_PEAK_DB) below
    /// the ground truth's loudest bin. Lower is better.
    pub log_spectral_distance_db: f64,
    /// In-band SNR over the **decay window** — see [`tail_window`] — in dB, which is where
    /// an 8-bit sample's quantisation noise is loudest relative to what is left of the
    /// signal. `NaN` when the instrument has no such window.
    pub tail_in_band_snr_db: f64,
}

/// Upper edge of the decay window, relative to the ground truth's own peak.
pub const TAIL_WINDOW_UPPER_DB: f64 = -24.0;

/// Lower edge of the decay window, relative to the ground truth's own peak.
pub const TAIL_WINDOW_LOWER_DB: f64 = -48.0;

/// Block length the decay window is found with.
const TAIL_BLOCK_FRAMES: usize = 1_024;

/// The frames of a render over which its level lies between [`TAIL_WINDOW_UPPER_DB`] and
/// [`TAIL_WINDOW_LOWER_DB`] of its own peak — the zone where a decay crosses the 8-bit
/// quantisation floor, and the only zone where a denoiser has anything to do.
///
/// # Why not simply "the last 40 %"
///
/// Because on an 8-bit sample that window is mostly **digital silence** and cannot move.
/// An exponential decay reaching −60 dB passes below half a quantisation step at about
/// −48 dB, and every value after that rounds to zero: there is no noise left to remove and
/// no signal left to keep, so those frames score exactly 0 dB whatever any enhancer does,
/// and they drag the average of any window that contains them towards zero. Measured on
/// instrument (a), three of the four tenths in the last 40 % were pinned at 0.00 dB.
///
/// −24 dB is where the decay first gets close enough to the floor for the floor to matter;
/// −48 dB is where the sample stops existing. Between them is the whole of what a decay
/// denoiser is for.
pub fn tail_window(reference: &[i16]) -> Option<core::ops::Range<usize>> {
    let peak = reference.iter().map(|frame| (*frame as i32).unsigned_abs()).max()? as f64;
    if peak <= 0.0 {
        return None;
    }
    let upper = peak * 10.0f64.powf(TAIL_WINDOW_UPPER_DB / 20.0);
    let lower = peak * 10.0f64.powf(TAIL_WINDOW_LOWER_DB / 20.0);

    let block_rms = |block: &[i16]| -> f64 {
        let energy: f64 = block.iter().map(|frame| *frame as f64 * *frame as f64).sum();
        (energy / block.len().max(1) as f64).sqrt()
    };
    let blocks: Vec<f64> = reference.chunks(TAIL_BLOCK_FRAMES).map(block_rms).collect();
    let first = blocks.iter().position(|rms| *rms <= upper)?;
    let last = blocks.iter().rposition(|rms| *rms >= lower)?;
    if last < first {
        return None;
    }
    let start = first * TAIL_BLOCK_FRAMES;
    let end = ((last + 1) * TAIL_BLOCK_FRAMES).min(reference.len());
    if end - start < IN_BAND_FFT_SIZE { None } else { Some(start..end) }
}

/// Score `candidate` against `reference`, both rendered at [`GROUND_TRUTH_RATE_HZ`].
pub fn measure(reference: &[i16], candidate: &[i16]) -> Metrics {
    let length = reference.len().min(candidate.len());
    let reference = &reference[..length];
    let candidate = &candidate[..length];

    let candidate_float: Vec<f32> = candidate.iter().map(|frame| *frame as f32 / 32_767.0).collect();
    let full_band_snr_db = segmental_snr_db(reference, &candidate_float, SEGMENTAL_SNR_FRAMES).unwrap_or(f64::NAN);

    let reference_scaled: Vec<f64> = reference.iter().map(|frame| *frame as f64 / 32_767.0).collect();
    let candidate_scaled: Vec<f64> = candidate.iter().map(|frame| *frame as f64 / 32_767.0).collect();

    let in_band_fft = Fft::new(IN_BAND_FFT_SIZE);
    let spectral_fft = Fft::new(SPECTRAL_FFT_SIZE);
    let rate = GROUND_TRUTH_RATE_HZ as f64;
    let in_band_snr_db = band_limited_snr_db(&in_band_fft, &reference_scaled, &candidate_scaled, rate, DEGRADED_NYQUIST_HZ).unwrap_or(f64::NAN);
    let magnitude_floor = audible_magnitude_floor(&spectral_fft, &reference_scaled);
    let log_spectral_distance_db = log_spectral_distance_db(&spectral_fft, &reference_scaled, &candidate_scaled, magnitude_floor).unwrap_or(f64::NAN);

    let tail_in_band_snr_db = match tail_window(reference) {
        Some(window) => band_limited_snr_db(&in_band_fft, &reference_scaled[window.clone()], &candidate_scaled[window], rate, DEGRADED_NYQUIST_HZ)
            .unwrap_or(f64::NAN),
        None => f64::NAN,
    };

    Metrics { full_band_snr_db, in_band_snr_db, log_spectral_distance_db, tail_in_band_snr_db }
}

/// RMS of the first `milliseconds` of a render, in dB relative to full scale — the attack
/// measurement instrument (c)'s acceptance criterion is stated in.
pub fn attack_rms_db(render: &[i16], milliseconds: f64) -> f64 {
    let frames = ((GROUND_TRUTH_RATE_HZ as f64 * milliseconds / 1_000.0) as usize).min(render.len()).max(1);
    let energy: f64 = render[..frames].iter().map(|frame| (*frame as f64 / 32_767.0) * (*frame as f64 / 32_767.0)).sum();
    let mean_square = energy / frames as f64;
    if mean_square <= 0.0 { -200.0 } else { 10.0 * mean_square.log10() }
}

// ---------------------------------------------------------------------------
// The instruments
// ---------------------------------------------------------------------------

/// The highest frequency any test instrument carries: where hearing stops, and therefore
/// where a harmonic series has to stop for the comparison to mean anything.
pub const INSTRUMENT_BANDWIDTH_HZ: f64 = 20_000.0;

/// Harmonics of 700 Hz at `1/n` up to [`INSTRUMENT_BANDWIDTH_HZ`], falling exponentially
/// to −60 dB over 1.5 s.
fn plucked_decay() -> Vec<i16> {
    let frames = Instrument::PluckedDecay.ground_truth_frames();
    let rate = GROUND_TRUTH_RATE_HZ as f64;
    let harmonics: Vec<usize> = (1..).take_while(|harmonic| 700.0 * *harmonic as f64 <= INSTRUMENT_BANDWIDTH_HZ).collect();
    let harmonic_sum: f64 = harmonics.iter().map(|harmonic| 1.0 / *harmonic as f64).sum();
    let peak = 0.9 * 32_767.0 / harmonic_sum;
    (0..frames)
        .map(|index| {
            let seconds = index as f64 / rate;
            // −60 dB over the whole 1.5 s, which is a factor of 1 000 in amplitude.
            let envelope = 10.0f64.powf(-3.0 * seconds / 1.5);
            let mut value = 0.0f64;
            for harmonic in &harmonics {
                let frequency = 700.0 * *harmonic as f64;
                value += (1.0 / *harmonic as f64) * (std::f64::consts::TAU * frequency * seconds).sin();
            }
            round_to_i16(peak * envelope * value)
        })
        .collect()
}

/// Twelve slightly stretched partials of 440 Hz with 5 Hz vibrato, over exactly one
/// two-second period.
///
/// Every partial is rounded to a multiple of 0.5 Hz, so its phase advances by a whole
/// number of turns over two seconds, and the vibrato's 5 Hz completes ten turns in the
/// same time. The body is therefore exactly periodic and its loop seam is continuous
/// before any smoother touches it.
fn sustained_tone() -> Vec<i16> {
    let frames = Instrument::SustainedTone.ground_truth_frames();
    let rate = GROUND_TRUTH_RATE_HZ as f64;
    let partials: Vec<(f64, f64)> = (1..)
        .map(|partial: usize| {
            let stretched = 440.0 * partial as f64 * (1.0 + 0.0008 * (partial * partial) as f64);
            // To the nearest half hertz: two seconds of it is then a whole number of turns.
            ((stretched * 2.0).round() / 2.0, 1.0 / partial as f64)
        })
        .take_while(|(frequency, _)| *frequency <= INSTRUMENT_BANDWIDTH_HZ)
        .collect();
    let amplitude_sum: f64 = partials.iter().map(|(_, amplitude)| *amplitude).sum();
    let peak = 0.85 * 32_767.0 / amplitude_sum;
    (0..frames)
        .map(|index| {
            let seconds = index as f64 / rate;
            let vibrato = (std::f64::consts::TAU * 5.0 * seconds).sin();
            let mut value = 0.0f64;
            for (frequency, amplitude) in &partials {
                // ±0.3 % of the partial's own frequency, as a phase deviation.
                let deviation = 0.003 * frequency / 5.0;
                value += amplitude * (std::f64::consts::TAU * frequency * seconds + deviation * vibrato).sin();
            }
            round_to_i16(peak * value)
        })
        .collect()
}

/// Low-passed xorshift noise with a 1 ms attack and a decay to −60 dB over 80 ms.
fn noise_drum() -> Vec<i16> {
    let frames = Instrument::NoiseDrum.ground_truth_frames();
    let rate = GROUND_TRUTH_RATE_HZ as f64;
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next_noise = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        // The top 32 bits, mapped to ±1.
        ((state >> 32) as f64 / 2_147_483_648.0) - 1.0
    };
    // Two cascaded one-pole low-passes at 6 kHz: enough shaping to be "filtered noise"
    // rather than white, while leaving real content above the degraded Nyquist.
    let coefficient = 1.0 - (-std::f64::consts::TAU * 6_000.0 / rate).exp();
    let (mut first, mut second) = (0.0f64, 0.0f64);
    let shaped: Vec<f64> = (0..frames)
        .map(|index| {
            let seconds = index as f64 / rate;
            first += coefficient * (next_noise() - first);
            second += coefficient * (first - second);
            let attack = (seconds / 0.001).min(1.0);
            let envelope = 10.0f64.powf(-3.0 * seconds / 0.08);
            attack * envelope * second
        })
        .collect();
    // The two poles cost an amount of level that depends on the noise sequence, so the
    // burst is normalised to a fixed peak rather than guessed at: the whole sequence is
    // deterministic, so the scale is too.
    let peak = shaped.iter().fold(0.0f64, |largest, value| largest.max(value.abs())).max(1.0e-12);
    let scale = 0.9 * 32_767.0 / peak;
    shaped.iter().map(|value| round_to_i16(scale * value)).collect()
}

/// A linear-phase windowed-sinc low-pass, applied to a one-shot with zero extension.
fn low_pass(frames: &[i16], cutoff_hz: f64, rate_hz: f64) -> Vec<i16> {
    let taps = DARK_LOW_PASS_TAPS;
    let leading = (taps / 2) as i64;
    let normalised_cutoff = 2.0 * cutoff_hz / rate_hz;
    let mut kernel = vec![0.0f64; taps];
    let mut sum = 0.0f64;
    for (index, coefficient) in kernel.iter_mut().enumerate() {
        let offset = index as f64 - leading as f64;
        let window = 0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / (taps - 1) as f64).cos();
        *coefficient = normalised_cutoff * sinc(normalised_cutoff * offset) * window;
        sum += *coefficient;
    }
    for coefficient in kernel.iter_mut() {
        *coefficient /= sum;
    }
    (0..frames.len())
        .map(|index| {
            let mut value = 0.0f64;
            for (tap, coefficient) in kernel.iter().enumerate() {
                let source = index as i64 + tap as i64 - leading;
                let sample = if source < 0 { 0.0 } else { frames.get(source as usize).copied().unwrap_or(0) as f64 };
                value += coefficient * sample;
            }
            round_to_i16(value)
        })
        .collect()
}

/// Band-limit and decimate from `from_hz` to `to_hz` through a 64-tap Kaiser-windowed
/// sinc, reading the source periodically when `periodic` so a looped instrument's
/// degradation is still exactly one period.
fn decimate(frames: &[i16], from_hz: f64, to_hz: f64, periodic: bool) -> Vec<i16> {
    let output_frames = ((frames.len() as f64) * to_hz / from_hz) as usize;
    let leading = (DECIMATION_TAPS / 2 - 1) as i64;
    // Normalised to the *source* rate: the band the output can hold, less a twentieth.
    let normalised_cutoff = DECIMATION_CUTOFF_FRACTION * to_hz / from_hz;
    let step = from_hz / to_hz;
    let at = |index: i64| -> f64 {
        if frames.is_empty() {
            return 0.0;
        }
        match periodic {
            true => frames[index.rem_euclid(frames.len() as i64) as usize] as f64,
            false if index < 0 => 0.0,
            false => frames.get(index as usize).copied().unwrap_or(0) as f64,
        }
    };
    (0..output_frames)
        .map(|output_index| {
            let position = output_index as f64 * step;
            let base = position.floor() as i64;
            let fraction = position - base as f64;
            let mut value = 0.0f64;
            let mut sum = 0.0f64;
            for tap in 0..DECIMATION_TAPS {
                let offset = tap as f64 - leading as f64 - fraction;
                let window_position = (offset + leading as f64 + 1.0) / DECIMATION_TAPS as f64;
                let coefficient = normalised_cutoff * sinc(normalised_cutoff * offset) * kaiser(window_position, 10.0);
                value += coefficient * at(base + tap as i64 - leading);
                sum += coefficient;
            }
            round_to_i16(value / sum)
        })
        .collect()
}

/// Round to the nearest 8-bit step and widen back by 256 — an `i8` sample as the loaders
/// present it.
fn quantise_to_eight_bits(frame: i16) -> i16 {
    let step = (frame as f64 / 256.0).round().clamp(-128.0, 127.0);
    (step as i32 * 256) as i16
}

fn round_to_i16(value: f64) -> i16 { value.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16 }

fn sinc(x: f64) -> f64 {
    if x.abs() < 1.0e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) }
}

fn kaiser(position: f64, beta: f64) -> f64 {
    let normalised = 2.0 * position - 1.0;
    let inner = 1.0 - normalised * normalised;
    if inner <= 0.0 {
        return 0.0;
    }
    bessel_i0(beta * inner.sqrt()) / bessel_i0(beta)
}

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for index in 1..60 {
        let ratio = x / 2.0 / index as f64;
        term *= ratio * ratio;
        sum += term;
        if term < 1.0e-18 * sum {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_instrument_synthesises_at_its_declared_length_and_reaches_a_sane_level() {
        for instrument in Instrument::ALL {
            let sample = ground_truth(instrument);
            assert_eq!(sample.frames.len(), instrument.ground_truth_frames(), "{:?}", instrument);
            assert_eq!(sample.rate_hz, GROUND_TRUTH_RATE_HZ);
            let peak = sample.frames.iter().map(|frame| (*frame as i32).abs()).max().unwrap_or(0);
            assert!(peak > 8_000 && peak < 32_768, "{:?} peaks at {peak}", instrument);
        }
    }

    #[test]
    fn the_degraded_version_is_an_eight_bit_sample_at_the_tracker_rate() {
        for instrument in Instrument::ALL {
            let sample = degraded(instrument);
            assert_eq!(sample.rate_hz, DEGRADED_RATE_HZ, "{:?}", instrument);
            assert!(sample.frames.iter().all(|frame| *frame as i32 % 256 == 0), "{:?} is not a widened 8-bit sample", instrument);
            let expected = (instrument.ground_truth_frames() as f64 * DEGRADED_RATE_HZ as f64 / GROUND_TRUTH_RATE_HZ as f64) as usize;
            assert_eq!(sample.frames.len(), expected, "{:?}", instrument);
        }
    }

    /// The looped instrument is periodic before degradation and periodic after it, which
    /// is what lets the loop smoother leave it alone and the extender fold its analysis
    /// circularly.
    #[test]
    fn the_sustained_tone_loops_seamlessly_at_both_rates() {
        for sample in [ground_truth(Instrument::SustainedTone), degraded(Instrument::SustainedTone)] {
            let frames = &sample.frames;
            let wrap_step = (frames[0] as i32 - frames[frames.len() - 1] as i32).abs();
            let largest_inside = frames.windows(2).map(|pair| (pair[1] as i32 - pair[0] as i32).abs()).max().unwrap_or(0);
            assert!(wrap_step <= largest_inside, "the seam steps by {wrap_step} against {largest_inside} inside the loop");
            assert_eq!(sample.loop_span, Some((0, frames.len() as u32)));
        }
    }

    /// The whole point of the harness: the ground truth renders back as itself, so any
    /// score a degraded render gets is the degradation and the enhancer, not the pipeline.
    #[test]
    fn the_ground_truth_renders_to_a_near_perfect_score_against_itself() {
        let instrument = Instrument::PluckedDecay;
        let sample = ground_truth(instrument);
        let rendered = render(&sample, None, instrument.render_frames()).expect("the ground truth renders");
        assert_eq!(rendered.len(), instrument.render_frames());
        let metrics = measure(&rendered, &rendered);
        assert_eq!(metrics.full_band_snr_db, 120.0);
        assert!(metrics.log_spectral_distance_db < 1.0e-9, "{metrics:?}");
    }

    /// Instruments (a) and (b) carry content all the way to the top of hearing, which is
    /// what makes an extension of the missing band scorable at all.
    #[test]
    fn the_two_harmonic_instruments_reach_the_top_of_the_audible_band() {
        use crate::analysis::{Fft, hann_window};

        let fft = Fft::new(4_096);
        let window = hann_window(4_096);
        for instrument in [Instrument::PluckedDecay, Instrument::SustainedTone] {
            let frames = ground_truth(instrument).frames;
            let scaled: Vec<f64> = frames[..4_096].iter().map(|frame| *frame as f64 / 32_767.0).collect();
            let magnitudes = fft.windowed_magnitudes(&scaled, &window, 4_096.0);
            // Energy in the octave below 20 kHz, against the octave below the degraded
            // sample's own Nyquist. A truth that stops at 6 kHz would have none.
            let bin_of = |hz: f64| (hz / GROUND_TRUTH_RATE_HZ as f64 * 4_096.0) as usize;
            let top: f64 = magnitudes[bin_of(10_000.0)..bin_of(20_000.0)].iter().map(|value| value * value).sum();
            let source_band: f64 = magnitudes[bin_of(2_000.0)..bin_of(4_181.0)].iter().map(|value| value * value).sum();
            assert!(top > source_band * 1.0e-6, "({}) has nothing above 10 kHz: {top:.3e} against {source_band:.3e}", instrument.letter());
        }
    }

    /// The decay window is where the ground truth crosses the 8-bit floor, and it has to
    /// contain real signal rather than the silence the last-40 % window mostly held.
    #[test]
    fn the_decay_window_lands_between_the_two_levels_it_names() {
        let instrument = Instrument::PluckedDecay;
        let reference = render(&ground_truth(instrument), None, instrument.render_frames()).expect("the ground truth renders");
        let window = tail_window(&reference).expect("a decaying instrument has a decay window");
        assert!(window.start > 0 && window.end <= reference.len());

        let peak = reference.iter().map(|frame| (*frame as i32).unsigned_abs()).max().expect("a peak") as f64;
        let rms = |slice: &[i16]| -> f64 {
            let energy: f64 = slice.iter().map(|frame| *frame as f64 * *frame as f64).sum();
            (energy / slice.len().max(1) as f64).sqrt()
        };
        let level_db = 20.0 * (rms(&reference[window.clone()]) / peak).log10();
        assert!(level_db < TAIL_WINDOW_UPPER_DB && level_db > TAIL_WINDOW_LOWER_DB, "the decay window sits at {level_db:.1} dB below peak");
        // And it is not the silent end of the sample: something is genuinely there.
        assert!(rms(&reference[window]) > 1.0, "the decay window is silent, which is exactly the failure it exists to avoid");
    }

    /// The degraded render is plainly worse than the ground truth, and worse above the
    /// source Nyquist than below it — the shape every later measurement is read against.
    #[test]
    fn the_degraded_render_scores_worse_than_the_ground_truth_and_worst_out_of_band() {
        let instrument = Instrument::PluckedDecay;
        let reference = render(&ground_truth(instrument), None, instrument.render_frames()).expect("the ground truth renders");
        let candidate = render(&degraded(instrument), None, instrument.render_frames()).expect("the degraded module renders");
        let metrics = measure(&reference, &candidate);
        assert!(metrics.full_band_snr_db < 30.0, "{metrics:?}");
        assert!(metrics.in_band_snr_db > metrics.full_band_snr_db, "the loss should be worse out of band: {metrics:?}");
        // Half a decibel of log-spectral distance is a large number under the audible
        // floor — a perfect score is zero, and the whole spread between chains lives
        // inside a couple of decibels once inaudible bins stop being charged for.
        assert!(metrics.log_spectral_distance_db > 0.2, "{metrics:?}");
        assert!(metrics.tail_in_band_snr_db.is_finite(), "a decaying instrument has a decay window: {metrics:?}");
    }
}
