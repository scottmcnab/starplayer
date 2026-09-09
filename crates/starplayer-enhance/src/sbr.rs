//! [`BandwidthExtender`] — puts a plausible top octave back onto a sample whose own
//! Nyquist took it away.
//!
//! # Why the missing band matters more than the noise
//!
//! M10-K5a's research point 3 settled what an 8-bit source's *noise* costs: about 47 dB of
//! SNR, of which no filter can remove any, because signal and noise share a band. What it
//! also showed is where the bigger loss is. A tracker sample at 8 363 Hz holds nothing
//! above 4 181 Hz, and a cymbal, a snare or a bright pluck lives well above that. Playing
//! it back at 44 100 Hz does not put the band back; upsampling it does not either, because
//! a resampler by construction adds no content. The top two octaves are simply gone.
//!
//! **Spectral band replication** is the classical answer: the band that is missing
//! correlates strongly with the band that is present, so transposing the octave below the
//! edge upwards, at the level the source's own roll-off predicts, gives a spectrum whose
//! shape is right even where its fine structure is invented. This is deliberately the
//! conservative, textbook form of it — no learned model, no oscillator bank, nothing that
//! could be right on one sample and grotesque on the next.
//!
//! # The band edge, and refusing to invent
//!
//! The long-term average power spectrum decides where the content stops: the **spectral
//! floor** is the mean power of the top [`SPECTRAL_FLOOR_TOP_PERCENT`] of bins, and the
//! **edge** is the highest bin standing [`BAND_EDGE_THRESHOLD_DB`] above it. Two things
//! then refuse the whole transform, and both matter more than the transform itself:
//!
//! * **No headroom.** An edge above [`MAXIMUM_EDGE_FRACTION_PERCENT`] of the arriving rate
//!   means the sample already fills its band and there is nothing to fill. The sample comes
//!   back bit-identical.
//! * **A genuinely dark source.** If the octave below the edge is *itself* falling faster
//!   than [`MINIMUM_EXTENSION_GAIN`] per octave, the sample is dark because it was recorded
//!   dark — a tape source, an oversampled instrument, a deliberately muffled pad — and not
//!   because a sample rate cut it off. Extending that is inventing a top octave that was
//!   never played. The sample comes back bit-identical.
//!
//! The second guard is the one that costs the least to get wrong in the quiet direction
//! and the most to get wrong in the loud one, which is why it is stated as a floor on the
//! gain rather than as a judgement about the edge.
//!
//! # The patch
//!
//! Per analysis frame, output bin `j ≥ edge` takes the complex value of bin `j >> k`,
//! where `k` is the number of octaves `j` sits above the edge, scaled by `gain^k` — for as
//! many octaves as [`MAXIMUM_PATCHED_OCTAVES`] allows, and only from bins the source
//! carries content in rather than noise. The
//! gain is the source's own tilt over the octave below the edge — measured as the power
//! ratio of that octave's upper half to its lower half, which *is* the per-octave
//! amplitude ratio for any spectrum falling as a power of frequency — times an extra
//! [`BandwidthExtender::tilt_db`] per octave, and it never boosts and never lands above
//! the level at the edge itself.
//!
//! # Loops
//!
//! The analysis reads the sample as the infinite sequence
//! [`InfiniteSource`](crate::upsample) defines, and every frame's output is added back at
//! the body index that sequence maps it to. For a loop that means the frames running past
//! `loop_end` fold back into the loop, so the result is **exactly periodic** by
//! construction and the seam the mixer wraps over is as continuous as it was before. See
//! [`crate::stft::overlap_add_normalisation`] for why that works whatever the loop's
//! length is.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;

use starplayer_model::{EnhancedPcm, SampleEnhancer, SamplePcm};

use crate::deterministic::{power_hundredths, saturating_i16};
use crate::stft::{STFT_BINS, STFT_HOP, STFT_SIZE, forward, inverse, window};
use crate::upsample::InfiniteSource;

/// Extra roll-off per octave the default extender adds on top of the source's own tilt, in
/// dB. Negative: the invented band is quieter than pure extrapolation would make it, which
/// is the direction a wrong guess is forgivable in.
pub const DEFAULT_SBR_TILT_DB: i32 = -6;

/// How far above the spectral floor a bin must stand to count as content, in dB.
pub const BAND_EDGE_THRESHOLD_DB: i32 = 12;

/// [`BAND_EDGE_THRESHOLD_DB`] as a **power** ratio: `10^(12/10)`. Committed as a bit
/// pattern; [`tests::the_committed_constants_are_what_the_arithmetic_says`] is the gate.
pub const BAND_EDGE_THRESHOLD_POWER_BITS: u64 = 0x402f_b2a7_3489_7866;

/// [`BAND_EDGE_THRESHOLD_POWER_BITS`], decoded.
pub const BAND_EDGE_THRESHOLD_POWER: f64 = f64::from_bits(BAND_EDGE_THRESHOLD_POWER_BITS);

/// Percentage of the highest bins the spectral floor is read from.
///
/// The floor is their **median** rather than their mean. On the sample this enhancer
/// exists for — one band-limited by its own rate — the two agree, because the top tenth of
/// the spectrum is nothing but stopband. They part company on a sample that already fills
/// its band, where the top tenth carries real partials: their mean is then dominated by
/// the partials, which lifts the threshold, hides the true edge and makes a full-band
/// sample look like one with headroom to fill. The median steps over them.
pub const SPECTRAL_FLOOR_TOP_PERCENT: usize = 10;

/// An edge above this percentage of the arriving rate means there is no headroom to fill.
pub const MAXIMUM_EDGE_FRACTION_PERCENT: usize = 45;

/// The lowest per-octave gain worth extending with.
///
/// The gain already carries the default −6 dB of extra roll-off, so this floor trips when
/// the source itself is falling by more than about 16 dB per octave at its own top: a
/// recording that is dark rather than a sample rate that is low. It is a safety rail
/// rather than a fine judgement — M10-K5c's harness found the extender *helps* a
/// 3 kHz-limited pluck, which sits well above this floor — and what it catches is the
/// near-brick-wall case, where the octave below the edge is mostly the filter's own
/// transition and there is nothing in it worth transposing.
pub const MINIMUM_EXTENSION_GAIN_BITS: u64 = 0x3fb4_7ae1_47ae_147b;

/// [`MINIMUM_EXTENSION_GAIN_BITS`], decoded. Exactly the `f64` nearest 0.08.
pub const MINIMUM_EXTENSION_GAIN: f64 = f64::from_bits(MINIMUM_EXTENSION_GAIN_BITS);

/// The lowest band edge the tilt measurement can be made over: the octave below it has to
/// hold at least two bins in each of its halves.
const MINIMUM_EDGE_BIN: usize = 8;

/// Octaves the patch reaches above the edge.
///
/// **One**, measured rather than assumed. The obvious design fills everything up to
/// Nyquist, and M10-K5c's harness says that is worse: against the four ground-truth
/// instruments, extending two octaves instead of one costs 0.32 dB of log-spectral
/// distance on the plucked decay and 0.27 dB on the sustained tone, and buys 0.05 dB on
/// the drum and −0.06 dB on the dark pluck. The reason is that the second octave is where
/// a transposed partial is least likely to land on a real one: doubling maps harmonic `n`
/// to harmonic `2n`, which is a real harmonic, but quadrupling maps it to `4n`, which for
/// most instruments is past where the instrument has any harmonics at all.
///
/// One octave above a 4x-upsampled tracker sample's 3.7 kHz edge reaches 7.5 kHz, which is
/// the octave that carries an instrument's brightness. The full sweep is in the task's
/// Research resolution.
pub const MAXIMUM_PATCHED_OCTAVES: usize = 1;

/// Samples shorter than this are returned unchanged: a hop is the least a windowed
/// analysis can say anything about, and a sample that short is a chip-tune single cycle
/// whose spectrum is a harmonic series the transform would only restate.
pub const MINIMUM_EXTENDABLE_FRAMES: usize = STFT_HOP;

/// Transpose the octave below a sample's band edge upwards to fill the band its own
/// sample rate took away.
///
/// See the [module documentation](self) for the edge, the two refusals, the patch and the
/// loop rule.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BandwidthExtender {
    /// Extra roll-off per octave, in dB, on top of the source's measured tilt. Zero or
    /// positive means no extra roll-off: the extender never boosts.
    pub tilt_db: i32,
}

impl BandwidthExtender {
    /// The default extender: the source's own tilt plus [`DEFAULT_SBR_TILT_DB`].
    pub const fn new() -> BandwidthExtender { BandwidthExtender { tilt_db: DEFAULT_SBR_TILT_DB } }

    /// The same extender with a different extra roll-off.
    pub const fn with_tilt_db(tilt_db: i32) -> BandwidthExtender { BandwidthExtender { tilt_db } }

    /// The extra per-octave amplitude factor [`BandwidthExtender::tilt_db`] asks for.
    fn extra_roll_off(&self) -> f64 {
        if self.tilt_db >= 0 {
            return 1.0;
        }
        // `10^(tilt/20)` for a negative tilt is `0.1^(|tilt|/20)`, and the exponent is
        // expressed in hundredths so the deterministic power can take it.
        let hundredths = (self.tilt_db.unsigned_abs() * 100) / 20;
        power_hundredths(0.1, hundredths)
    }
}

impl Default for BandwidthExtender {
    fn default() -> BandwidthExtender { BandwidthExtender::new() }
}

impl SampleEnhancer for BandwidthExtender {
    fn name(&self) -> String {
        let mut name = String::from("sbr");
        if self.tilt_db != DEFAULT_SBR_TILT_DB {
            let _ = write!(&mut name, "={}", self.tilt_db);
        }
        name
    }

    fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm {
        let unchanged = || EnhancedPcm::unchanged(sample);
        let body_frames = sample.frames.len();
        if body_frames < MINIMUM_EXTENDABLE_FRAMES {
            return unchanged();
        }
        let source = InfiniteSource::for_sample(&sample);
        let Some(plan) = self.plan(&average_power_spectrum(&source, body_frames)) else { return unchanged() };

        let mut signal = vec![0.0f64; body_frames];
        let mut weight = vec![0.0f64; body_frames];
        let mut real = [0.0f64; STFT_SIZE];
        let mut imaginary = [0.0f64; STFT_SIZE];
        for start in frame_starts(body_frames) {
            load_frame(&source, start, &mut real, &mut imaginary);
            forward(&mut real, &mut imaginary);
            patch(&mut real, &mut imaginary, &plan);
            inverse(&mut real, &mut imaginary);
            for offset in 0..STFT_SIZE {
                let Some(target) = source.periodic_body_index(start + offset as i64) else { continue };
                let weighting = window(offset);
                signal[target] += real[offset] * weighting;
                weight[target] += weighting * weighting;
            }
        }

        let frames: Vec<i16> = (0..body_frames)
            .map(|index| match weight[index] > 0.0 {
                true => saturating_i16(signal[index] / weight[index]),
                // Unreachable while the frame grid covers the body, and a plain copy
                // rather than a panic if it ever is not.
                false => sample.frames[index],
            })
            .collect();
        EnhancedPcm { frames, ..unchanged() }
    }
}

/// What the extender decided to do to every frame of one sample.
#[derive(Clone, Debug, PartialEq)]
struct Plan {
    /// The first bin the patch writes.
    edge_bin: usize,
    /// Amplitude gain per octave above the edge, indexed by octave: entry `k` is `gain^k`.
    octave_gains: Vec<f64>,
    /// Which bins below the edge carry **content** rather than the source's own noise, by
    /// the same 12 dB rule that found the edge.
    ///
    /// Only these are transposed. Without the mask the patch copies the 8-bit quantisation
    /// noise sitting between an instrument's partials as faithfully as it copies the
    /// partials, and puts a hiss into an octave that previously had none — which measures
    /// worse than doing nothing at all on any sustained tone, and sounds worse too. With
    /// it, what goes upstairs is the harmonic structure and not the floor it sits on.
    content: Vec<bool>,
}

impl Plan {
    /// One past the highest bin the patch writes: [`MAXIMUM_PATCHED_OCTAVES`] above the
    /// edge, or Nyquist, whichever comes first. Bins above it are left exactly as the
    /// source had them.
    fn patched_to(&self) -> usize { (self.edge_bin << MAXIMUM_PATCHED_OCTAVES).min(STFT_BINS) }
}

impl BandwidthExtender {
    /// The band edge and the per-octave gains, or `None` for one of the two refusals.
    fn plan(&self, power: &[f64; STFT_BINS]) -> Option<Plan> {
        // Two passes, because a resampled tracker sample has **two** band edges and only
        // the second is the one worth transposing from.
        //
        // A 4x upsample leaves the polyphase filter's own stopband — a hundred decibels
        // down — above the source's Nyquist, and the source's 8-bit quantisation noise
        // spread flat across everything below it. A floor read from the top of the whole
        // spectrum therefore measures the *stopband*, and the highest bin standing 12 dB
        // above that is the resampler's cutoff, not the point where the instrument stops.
        // So the first pass finds that hard limit, and the second re-reads the floor from
        // the top tenth of the band **below** it — which is the source's own noise floor —
        // and finds where the content really ends against it.
        //
        // Two passes and no more: the first answers "how much band is there", which is
        // what the headroom refusal is about, and the second answers "how much of it is
        // used", which is what the patch needs. Iterating to a fixed point would keep
        // walking a smoothly decaying spectrum downwards with nothing to stop it.
        let (band_limit_bin, _) = band_edge(power, STFT_BINS)?;
        // No headroom: the sample already fills its band.
        if band_limit_bin * 100 > MAXIMUM_EDGE_FRACTION_PERCENT * STFT_SIZE {
            return None;
        }
        let (edge_bin, content_threshold) = band_edge(power, band_limit_bin)?;
        if edge_bin < MINIMUM_EDGE_BIN {
            return None;
        }

        // The octave below the edge, split in half. A spectrum falling as a power of
        // frequency has the same *power* ratio across half an octave as its *amplitude*
        // ratio across a whole one, which is why this ratio is the per-octave gain — and
        // why the patch can never land above the level at the edge: the first patched bin
        // takes the bin an octave below it and scales it by exactly the ratio between
        // those two levels, times an extra roll-off that is never above one.
        let lower_mean = mean(power, edge_bin / 2, edge_bin * 3 / 4);
        let upper_mean = mean(power, edge_bin * 3 / 4, edge_bin);
        if !(lower_mean > 0.0) {
            return None;
        }
        let tilt = upper_mean / lower_mean;
        let gain = if tilt > 1.0 { 1.0 } else { tilt } * self.extra_roll_off();
        if gain < MINIMUM_EXTENSION_GAIN {
            return None;
        }

        let mut octave_gains = vec![1.0f64];
        let mut running = gain;
        for _ in 0..MAXIMUM_PATCHED_OCTAVES {
            octave_gains.push(running);
            running *= gain;
        }
        // Which of the source's bins are worth transposing. See `Plan::content`.
        let content: Vec<bool> = (0..edge_bin).map(|bin| power[bin] > content_threshold).collect();
        Some(Plan { edge_bin, octave_gains, content })
    }
}

/// The highest bin below `limit` standing [`BAND_EDGE_THRESHOLD_DB`] above the floor of
/// the band `0 .. limit`, and that threshold itself.
///
/// The floor is the **median** of that band's top [`SPECTRAL_FLOOR_TOP_PERCENT`]; the
/// threshold it returns is what the caller also uses to decide, bin by bin, which of the
/// source's bins are content rather than its own noise.
fn band_edge(power: &[f64; STFT_BINS], limit: usize) -> Option<(usize, f64)> {
    let limit = limit.min(STFT_BINS);
    if limit < 2 {
        return None;
    }
    let floor_from = limit - (limit * SPECTRAL_FLOOR_TOP_PERCENT / 100).max(1);
    let mut top_bins: Vec<f64> = power[floor_from..limit].to_vec();
    top_bins.sort_by(|left, right| left.partial_cmp(right).unwrap_or(core::cmp::Ordering::Equal));
    let threshold = top_bins[top_bins.len() / 2] * BAND_EDGE_THRESHOLD_POWER;
    let edge = (0..limit).rev().find(|bin| power[*bin] > threshold)?;
    Some((edge, threshold))
}

/// Mean of `power[from..to]`, zero for an empty span.
fn mean(power: &[f64], from: usize, to: usize) -> f64 {
    let to = to.min(power.len());
    if from >= to {
        return 0.0;
    }
    let mut total = 0.0f64;
    for value in &power[from..to] {
        total += *value;
    }
    total / (to - from) as f64
}

/// Where every analysis frame starts, in the sample's own frame numbering.
///
/// The grid begins one window minus one hop **before** frame zero, so every frame of the
/// body is covered by the same four windows as every other and the overlap-add needs no
/// special case at either end.
fn frame_starts(body_frames: usize) -> impl Iterator<Item = i64> {
    let first = -((STFT_SIZE - STFT_HOP) as i64);
    let last = body_frames as i64;
    core::iter::successors(Some(first), |start| Some(start + STFT_HOP as i64)).take_while(move |start| *start < last)
}

/// Read one windowed analysis frame out of the infinite sequence.
fn load_frame(source: &InfiniteSource<'_>, start: i64, real: &mut [f64; STFT_SIZE], imaginary: &mut [f64; STFT_SIZE]) {
    for offset in 0..STFT_SIZE {
        real[offset] = source.at_periodic(start + offset as i64) * window(offset);
        imaginary[offset] = 0.0;
    }
}

/// The long-term average power spectrum of the whole sample, one entry per non-negative
/// bin.
fn average_power_spectrum(source: &InfiniteSource<'_>, body_frames: usize) -> [f64; STFT_BINS] {
    let mut power = [0.0f64; STFT_BINS];
    let mut real = [0.0f64; STFT_SIZE];
    let mut imaginary = [0.0f64; STFT_SIZE];
    let mut frames = 0usize;
    for start in frame_starts(body_frames) {
        load_frame(source, start, &mut real, &mut imaginary);
        forward(&mut real, &mut imaginary);
        for bin in 0..STFT_BINS {
            power[bin] += real[bin] * real[bin] + imaginary[bin] * imaginary[bin];
        }
        frames += 1;
    }
    if frames > 0 {
        let scale = 1.0 / frames as f64;
        for value in power.iter_mut() {
            *value *= scale;
        }
    }
    power
}

/// Copy the octave below the edge upwards, octave by octave, and restore the spectrum's
/// Hermitian symmetry so the inverse transform gives a real signal back.
fn patch(real: &mut [f64; STFT_SIZE], imaginary: &mut [f64; STFT_SIZE], plan: &Plan) {
    let last_octave = plan.octave_gains.len() - 1;
    let patched_to = plan.patched_to();
    for bin in plan.edge_bin..patched_to {
        let mut source_bin = bin;
        let mut octave = 0usize;
        while source_bin >= plan.edge_bin {
            source_bin /= 2;
            octave += 1;
        }
        // Source bins all sit below the edge and target bins all sit at or above it, so
        // nothing is read after it has been written.
        let gain = match plan.content.get(source_bin) {
            Some(true) if octave <= last_octave => plan.octave_gains[octave],
            _ => 0.0,
        };
        real[bin] = gain * real[source_bin];
        imaginary[bin] = gain * imaginary[source_bin];
    }
    // The Nyquist bin of a real signal has no imaginary part, and it is only this frame's
    // business when the patch actually reached it.
    if patched_to > STFT_SIZE / 2 {
        imaginary[STFT_SIZE / 2] = 0.0;
    }
    for bin in plan.edge_bin..patched_to.min(STFT_SIZE / 2) {
        real[STFT_SIZE - bin] = real[bin];
        imaginary[STFT_SIZE - bin] = -imaginary[bin];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer_model::{DEFAULT_REFERENCE_RATE_HZ, LoopMode};
    use std::vec;

    const RATE: f64 = 33_452.0;

    fn one_shot(frames: &[i16]) -> SamplePcm<'_> {
        SamplePcm {
            frames,
            rate_hz: DEFAULT_REFERENCE_RATE_HZ * 4,
            relative_note: 0,
            finetune: 0,
            loop_mode: LoopMode::None,
            loop_start: 0,
            loop_end: 0,
            sustain_loop: None,
        }
    }

    /// A harmonic series of `fundamental_hz` whose partials stop at `edge_hz`, falling at
    /// `1/n` — a band-limited sample with real headroom above it.
    fn band_limited_harmonics(frames: usize, fundamental_hz: f64, edge_hz: f64) -> Vec<i16> {
        // Snapped to a whole number of cycles over `frames`, so the body is exactly one
        // period and the looped tests start from a seam that is already continuous.
        let cycles = (fundamental_hz * frames as f64 / RATE).round().max(1.0);
        let fundamental_hz = RATE * cycles / frames as f64;
        let partials: Vec<usize> = (1..).take_while(|partial| fundamental_hz * *partial as f64 <= edge_hz).collect();
        let sum: f64 = partials.iter().map(|partial| 1.0 / *partial as f64).sum();
        (0..frames)
            .map(|index| {
                let seconds = index as f64 / RATE;
                let mut value = 0.0f64;
                for partial in &partials {
                    value += (1.0 / *partial as f64) * (core::f64::consts::TAU * fundamental_hz * *partial as f64 * seconds).sin();
                }
                (0.8 * 32_767.0 * value / sum) as i16
            })
            .collect()
    }

    /// Mean power per bin of a slice, through the crate's own transform.
    fn spectrum(frames: &[i16], loop_span: Option<(u32, u32)>) -> [f64; STFT_BINS] {
        let sample = match loop_span {
            Some((start, end)) => SamplePcm { loop_mode: LoopMode::Forward, loop_start: start, loop_end: end, ..one_shot(frames) },
            None => one_shot(frames),
        };
        average_power_spectrum(&InfiniteSource::for_sample(&sample), frames.len())
    }

    fn energy_above(power: &[f64; STFT_BINS], bin: usize) -> f64 {
        power[bin..].iter().sum()
    }

    #[test]
    fn the_committed_constants_are_what_the_arithmetic_says() {
        assert_eq!(BAND_EDGE_THRESHOLD_POWER.to_bits(), std::primitive::f64::powf(10.0, BAND_EDGE_THRESHOLD_DB as f64 / 10.0).to_bits());
        assert_eq!(MINIMUM_EXTENSION_GAIN.to_bits(), 0.08f64.to_bits());
        assert_eq!(BandwidthExtender::new().tilt_db, -6);
    }

    #[test]
    fn the_default_tilt_is_six_decibels_of_extra_roll_off_per_octave() {
        let extra = BandwidthExtender::new().extra_roll_off();
        assert!((extra - 0.501_187_233_627_272_4).abs() < 1.0e-12, "six dB down is {extra}");
        assert_eq!(BandwidthExtender::with_tilt_db(0).extra_roll_off(), 1.0);
        assert_eq!(BandwidthExtender::with_tilt_db(6).extra_roll_off(), 1.0, "the extender never boosts");
    }

    #[test]
    fn the_name_encodes_the_tilt_and_nothing_else() {
        assert_eq!(BandwidthExtender::new().name(), "sbr");
        assert_eq!(BandwidthExtender::with_tilt_db(-6).name(), "sbr");
        assert_eq!(BandwidthExtender::with_tilt_db(-12).name(), "sbr=-12");
    }

    /// The whole point: a sample with headroom above its content gets content there.
    #[test]
    fn a_band_limited_harmonic_series_gains_energy_above_its_edge() {
        let frames = band_limited_harmonics(16_384, 220.0, 4_000.0);
        let edge_bin = (4_000.0 / RATE * STFT_SIZE as f64) as usize;
        let before = spectrum(&frames, None);
        let enhanced = BandwidthExtender::new().enhance(one_shot(&frames));
        assert_eq!(enhanced.frames.len(), frames.len(), "the body length is untouched");
        assert_eq!(enhanced.rate_hz, DEFAULT_REFERENCE_RATE_HZ * 4, "and so is the rate");
        let after = spectrum(&enhanced.frames, None);

        let gained = energy_above(&after, edge_bin + 4) / energy_above(&before, edge_bin + 4).max(1.0e-30);
        assert!(gained > 10.0, "the band above the edge gained only a factor of {gained}");
        // And the band below it is left where it was.
        let below_before: f64 = before[..edge_bin - 4].iter().sum();
        let below_after: f64 = after[..edge_bin - 4].iter().sum();
        assert!((below_after / below_before - 1.0).abs() < 0.02, "the band below the edge moved by {}", below_after / below_before - 1.0);
    }

    /// A sample that already fills its band is returned bit-identical: the first refusal.
    #[test]
    fn a_sample_with_no_headroom_is_returned_unchanged() {
        let frames = band_limited_harmonics(8_192, 220.0, 15_500.0);
        let enhanced = BandwidthExtender::new().enhance(one_shot(&frames));
        assert_eq!(enhanced.frames, frames, "there is nothing above 0.45 of the rate to fill");
    }

    /// A harmonic series rolling off steeply above `knee_hz` — content that is dark
    /// because it was recorded dark, not because a sample rate cut it off.
    fn dark_harmonics(frames: usize, fundamental_hz: f64, knee_hz: f64, decibels_per_khz: f64) -> Vec<i16> {
        let amplitudes: Vec<(f64, f64)> = (1..=60)
            .map(|partial| {
                let frequency = fundamental_hz * partial as f64;
                let above = (frequency - knee_hz).max(0.0) / 1_000.0;
                let roll_off = std::primitive::f64::powf(10.0, -decibels_per_khz * above / 20.0);
                (frequency, roll_off / partial as f64)
            })
            .filter(|(frequency, _)| *frequency < RATE / 2.0)
            .collect();
        let sum: f64 = amplitudes.iter().map(|(_, amplitude)| *amplitude).sum();
        (0..frames)
            .map(|index| {
                let seconds = index as f64 / RATE;
                let mut value = 0.0f64;
                for (frequency, amplitude) in &amplitudes {
                    value += amplitude * (core::f64::consts::TAU * frequency * seconds).sin();
                }
                (0.8 * 32_767.0 * value / sum) as i16
            })
            .collect()
    }

    /// A source rolling off across the whole octave below its edge is returned
    /// bit-identical: the second refusal, and the one that keeps a dark instrument dark.
    ///
    /// The knee is at 1 kHz and the roll-off is 40 dB per kHz, so the octave the tilt is
    /// measured over is entirely inside the roll-off — which is what the refusal is
    /// looking for. A source with a **brick wall** at its top is a different animal and is
    /// deliberately *not* refused: its octave below the wall is ordinary content, and it
    /// is genuinely indistinguishable from a sample whose rate was simply lower. See
    /// `MINIMUM_EXTENSION_GAIN`.
    #[test]
    fn a_source_that_is_dark_rather_than_band_limited_is_returned_unchanged() {
        let frames = dark_harmonics(16_384, 220.0, 1_000.0, 40.0);
        let enhanced = BandwidthExtender::new().enhance(one_shot(&frames));
        assert_eq!(enhanced.frames, frames, "a dark source must not be given a top octave it never had");
    }

    #[test]
    fn a_sample_shorter_than_a_hop_is_returned_unchanged() {
        let frames = vec![1_000i16; MINIMUM_EXTENDABLE_FRAMES - 1];
        assert_eq!(BandwidthExtender::new().enhance(one_shot(&frames)).frames, frames);
    }

    #[test]
    fn an_empty_sample_round_trips() {
        let empty: Vec<i16> = Vec::new();
        assert!(BandwidthExtender::new().enhance(one_shot(&empty)).frames.is_empty());
    }

    /// The overlap-add identity: with the patch disabled the analysis and synthesis
    /// reconstruct a **looped** sample exactly, whatever its length does to the hop grid.
    ///
    /// This is the property the loop rule rests on — see
    /// [`crate::stft::overlap_add_normalisation`].
    #[test]
    fn a_looped_sample_that_is_not_patched_comes_back_unchanged() {
        // A length that is deliberately not a multiple of the hop.
        let frames = band_limited_harmonics(4_099, 220.0, 4_000.0);
        let sample = SamplePcm { loop_mode: LoopMode::Forward, loop_start: 0, loop_end: frames.len() as u32, ..one_shot(&frames) };
        let source = InfiniteSource::for_sample(&sample);

        let mut signal = vec![0.0f64; frames.len()];
        let mut weight = vec![0.0f64; frames.len()];
        let mut real = [0.0f64; STFT_SIZE];
        let mut imaginary = [0.0f64; STFT_SIZE];
        for start in frame_starts(frames.len()) {
            load_frame(&source, start, &mut real, &mut imaginary);
            forward(&mut real, &mut imaginary);
            inverse(&mut real, &mut imaginary);
            for offset in 0..STFT_SIZE {
                let Some(target) = source.periodic_body_index(start + offset as i64) else { continue };
                let weighting = window(offset);
                signal[target] += real[offset] * weighting;
                weight[target] += weighting * weighting;
            }
        }
        for index in 0..frames.len() {
            let reconstructed = signal[index] / weight[index];
            assert!((reconstructed - frames[index] as f64).abs() < 1.0e-6, "frame {index}: {reconstructed} against {}", frames[index]);
        }
    }

    /// A patched loop is still a loop: the seam steps no more than the waveform does
    /// inside it, which is `mixer_determinism.rs`'s own definition of click-free.
    #[test]
    fn a_patched_loop_stays_periodic_at_its_seam() {
        let frames = band_limited_harmonics(4_099, 220.0, 4_000.0);
        let sample = SamplePcm { loop_mode: LoopMode::Forward, loop_start: 0, loop_end: frames.len() as u32, ..one_shot(&frames) };
        let enhanced = BandwidthExtender::new().enhance(sample);
        assert_ne!(enhanced.frames, frames, "the loop really was patched");

        let body = &enhanced.frames;
        let wrap_step = (body[0] as i32 - body[body.len() - 1] as i32).abs();
        let largest_inside = body.windows(2).map(|pair| (pair[1] as i32 - pair[0] as i32).abs()).max().unwrap_or(0);
        assert!(wrap_step <= largest_inside, "the seam steps by {wrap_step} against {largest_inside} inside the loop");
    }

    #[test]
    fn the_same_input_extends_to_the_same_bytes_twice() {
        let frames = band_limited_harmonics(4_096, 300.0, 4_000.0);
        let first = BandwidthExtender::new().enhance(one_shot(&frames));
        let second = BandwidthExtender::new().enhance(one_shot(&frames));
        assert_eq!(first, second);
    }

}
