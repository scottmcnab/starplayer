//! IT's per-voice resonant low-pass, through the render kernel (M6-G2).
//!
//! Three things are being pinned here, and they are different kinds of claim:
//!
//! 1. **The cross-target contract.** The first 64 filtered frames of a known input on the
//!    fixed path are written out as literals. The fixed path is the canonical bit-exact
//!    reference (architecture §7.3), so this is the check that x86, ARM and WASM agree —
//!    the same job the goldens do for the unfiltered mixer, at a size a human can read.
//!    A synthetic *golden* for a filtered fixture is not added here: G3's synthetic IT
//!    fixture will carry a `Zxx` once both land.
//! 2. **That it is a low-pass at all**, measured rather than asserted, by rendering
//!    broadband noise through it and comparing band energies against the same render with
//!    the filter bypassed.
//! 3. **That turning the filter on does not break block-size independence**, which is the
//!    invariant the whole engine is shaped around.

use starplayer_core::{FilterParams, I1F15, Step, U0F16, VoiceParams};
use starplayer_dsp::Linear;
use starplayer_mixer::{
    BusSegment, FixedFrame, FixedPath, FloatFrame, FloatPath, LoopSpan, MixPath, SampleRegion, Voice, VoicePool,
    VoiceTag, accumulate_voice, append_guarded_sample,
};

const SAMPLE_RATE_HZ: u32 = 44_100;

/// A deterministic broadband source: a plain 32-bit LCG, whose top sixteen bits are close
/// enough to white for a band-energy comparison and are the same numbers on every target.
/// Deliberately not `rand`, which would make the test depend on a crate's own version.
fn noise(length: usize) -> Vec<i16> {
    let mut state = 0x1234_5678u32;
    (0..length)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 16) as i16
        })
        .collect()
}

/// The PCM blob and region for a one-shot sample of `frames` noise frames.
fn noise_blob(frames: usize) -> (Vec<i16>, SampleRegion) {
    let mut blob = Vec::new();
    let region = append_guarded_sample(&mut blob, &noise(frames), None);
    (blob, region)
}

/// A voice playing `region` at exactly one frame per output frame, hard left at full
/// volume with its ramps already landed, so the left channel carries the filter's own
/// output with nothing else moving.
fn steady_voice(region: SampleRegion, filter: FilterParams) -> Voice {
    let params = VoiceParams { step: Step::ONE, volume: U0F16::MAX, pan: I1F15::MIN, filter, ..VoiceParams::SILENT };
    let mut voice = Voice::new(VoiceTag::default(), region, params, 0);
    voice.settle_gains();
    voice
}

fn render_fixed(filter: FilterParams, frames: usize, source_frames: usize) -> Vec<i32> {
    let (blob, region) = noise_blob(source_frames);
    let mut voice = steady_voice(region, filter);
    let mut output = vec![FixedFrame::default(); frames];
    accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, SAMPLE_RATE_HZ);
    output.iter().map(|frame| frame.left).collect()
}

fn render_float(filter: FilterParams, frames: usize, source_frames: usize) -> Vec<f32> {
    let (blob, region) = noise_blob(source_frames);
    let mut voice = steady_voice(region, filter);
    let mut output = vec![FloatFrame::default(); frames];
    accumulate_voice::<FloatPath, Linear>(&mut voice, &blob, &mut output, SAMPLE_RATE_HZ);
    output.iter().map(|frame| frame.left).collect()
}

/// The cross-target contract: cutoff 40, resonance 64, at 44.1 kHz, on the fixed path.
///
/// Regenerating these numbers means the fixed path's arithmetic changed, which is a
/// deliberate act with an accuracy-policy entry behind it — not something to paste over.
#[test]
fn the_first_sixty_four_filtered_frames_are_pinned_on_the_fixed_path() {
    let rendered = render_fixed(FilterParams::from_it(40, 64), 64, 256);
    assert_eq!(
        rendered,
        vec![
            103, 160, 249, 402, 442, 455, 446, 424,
            322, 109, -72, -159, -185, -164, -211, -159,
            -41, 50, 48, 121, 294, 568, 948, 1306,
            1609, 2005, 2298, 2477, 2672, 2780, 2833, 2844,
            2752, 2703, 2735, 2854, 2915, 2872, 2767, 2755,
            2743, 2820, 2881, 2826, 2681, 2582, 2517, 2462,
            2497, 2576, 2667, 2811, 2891, 2979, 3086, 3135,
            3164, 3202, 3208, 3171, 3014, 2899, 2800, 2718,
        ]
    );
}

#[test]
fn a_bypassed_filter_leaves_the_render_exactly_as_it_was() {
    let unfiltered = render_fixed(FilterParams::BYPASS, 512, 1024);
    // IT's own rule: cutoff 127 with resonance 0 is not a filter, and the encoding makes
    // that the same value as `BYPASS`, so this is the identical render rather than a
    // very-nearly-identical one.
    let fully_open = render_fixed(FilterParams::from_it(127, 0), 512, 1024);
    assert_eq!(unfiltered, fully_open);
}

// ── the spectral claim ──────────────────────────────────────────────────────────────

/// An in-place iterative radix-2 FFT, in `f64`, on `(real, imaginary)` pairs. Test-only
/// code: the engine itself never evaluates a transcendental, but a test measuring a
/// filter's response has to.
fn fft(values: &mut [(f64, f64)]) {
    let length = values.len();
    assert!(length.is_power_of_two(), "radix-2 needs a power of two");

    let mut target = 0usize;
    for source in 1..length {
        let mut bit = length >> 1;
        while target & bit != 0 {
            target ^= bit;
            bit >>= 1;
        }
        target |= bit;
        if source < target {
            values.swap(source, target);
        }
    }

    let mut span = 2usize;
    while span <= length {
        let angle = -2.0 * std::f64::consts::PI / span as f64;
        for block in (0..length).step_by(span) {
            for index in 0..span / 2 {
                let (twiddle_real, twiddle_imaginary) = (f64::cos(angle * index as f64), f64::sin(angle * index as f64));
                let (even_real, even_imaginary) = values[block + index];
                let (odd_real, odd_imaginary) = values[block + index + span / 2];
                let product_real = odd_real * twiddle_real - odd_imaginary * twiddle_imaginary;
                let product_imaginary = odd_real * twiddle_imaginary + odd_imaginary * twiddle_real;
                values[block + index] = (even_real + product_real, even_imaginary + product_imaginary);
                values[block + index + span / 2] = (even_real - product_real, even_imaginary - product_imaginary);
            }
        }
        span <<= 1;
    }
}

/// Energy in `[low_hz, high_hz)` of `samples`, whose length must be a power of two.
fn band_energy(samples: &[i32], low_hz: f64, high_hz: f64) -> f64 {
    let mut spectrum: Vec<(f64, f64)> = samples.iter().map(|&value| (value as f64, 0.0)).collect();
    fft(&mut spectrum);
    let bin_hz = SAMPLE_RATE_HZ as f64 / samples.len() as f64;
    let first = (low_hz / bin_hz).ceil() as usize;
    let last = ((high_hz / bin_hz) as usize).min(samples.len() / 2);
    spectrum[first..last].iter().map(|&(real, imaginary)| real * real + imaginary * imaginary).sum()
}

const SPECTRUM_FRAMES: usize = 8_192;

#[test]
fn a_closed_filter_removes_at_least_twenty_four_decibels_above_two_kilohertz() {
    let unfiltered = render_fixed(FilterParams::BYPASS, SPECTRUM_FRAMES, SPECTRUM_FRAMES * 2);
    let filtered = render_fixed(FilterParams::from_it(0, 0), SPECTRUM_FRAMES, SPECTRUM_FRAMES * 2);

    let nyquist = SAMPLE_RATE_HZ as f64 / 2.0;
    let open_energy = band_energy(&unfiltered, 2_000.0, nyquist);
    let closed_energy = band_energy(&filtered, 2_000.0, nyquist);
    let attenuation_db = 10.0 * f64::log10(open_energy / closed_energy.max(f64::MIN_POSITIVE));
    assert!(attenuation_db >= 24.0, "only {attenuation_db:.1} dB of attenuation above 2 kHz");

    // …and it is a low-pass, not an attenuator: the band below the 131 Hz cutoff is
    // essentially untouched.
    let open_low = band_energy(&unfiltered, 20.0, 100.0);
    let closed_low = band_energy(&filtered, 20.0, 100.0);
    assert!(closed_low > open_low * 0.5, "the pass band lost too much: {closed_low:e} against {open_low:e}");
}

#[test]
fn full_resonance_puts_a_peak_at_the_cutoff() {
    // Cutoff 64 is 110·2^(0.25 + 64/24) = 831 Hz.
    let unfiltered = render_fixed(FilterParams::BYPASS, SPECTRUM_FRAMES, SPECTRUM_FRAMES * 2);
    let resonant = render_fixed(FilterParams::from_it(64, 127), SPECTRUM_FRAMES, SPECTRUM_FRAMES * 2);
    let damped = render_fixed(FilterParams::from_it(64, 0), SPECTRUM_FRAMES, SPECTRUM_FRAMES * 2);

    let around_cutoff = |samples: &[i32]| band_energy(samples, 700.0, 1_000.0);
    let resonant_gain = around_cutoff(&resonant) / around_cutoff(&unfiltered);
    let damped_gain = around_cutoff(&damped) / around_cutoff(&unfiltered);

    assert!(resonant_gain > 4.0, "resonance 127 should lift the cutoff band, gain was {resonant_gain:.2}");
    assert!(damped_gain < 1.5, "resonance 0 should not, gain was {damped_gain:.2}");
    assert!(resonant_gain > damped_gain * 4.0, "the resonant peak is not distinguishable from the unresonant response");

    // The peak is at the cutoff, not somewhere else: a band an octave above is not lifted.
    let above = band_energy(&resonant, 1_600.0, 2_200.0) / band_energy(&unfiltered, 1_600.0, 2_200.0);
    assert!(above < resonant_gain / 2.0, "the lift at 1.6-2.2 kHz ({above:.2}) is not below the peak's");
}

#[test]
fn the_float_path_and_the_fixed_path_hear_the_same_filter() {
    let filter = FilterParams::from_it(40, 96);
    let fixed = render_fixed(filter, 4_096, 8_192);
    let float = render_float(filter, 4_096, 8_192);

    let mut worst = 0.0f32;
    for (&fixed_value, &float_value) in fixed.iter().zip(float.iter()) {
        // The float path's accumulator is normalised to ±1.0 and the fixed path's is on
        // the raw `i16` scale.
        worst = worst.max((fixed_value as f32 / 32_768.0 - float_value).abs());
    }
    assert!(worst < 2e-3, "the two paths drifted by {worst}");
}

// ── block-size independence, with the filter live ───────────────────────────────────

#[test]
fn splitting_a_filtered_run_produces_the_same_frames_as_rendering_it_whole() {
    let filter = FilterParams::from_it(30, 100);
    let (blob, region) = noise_blob(4_096);

    let render = |chunk_length: usize| {
        let mut voice = steady_voice(region, filter);
        let mut output = vec![FixedFrame::default(); 2_048];
        let mut produced = 0;
        while produced < output.len() {
            let next = (produced + chunk_length).min(output.len());
            accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output[produced..next], SAMPLE_RATE_HZ);
            produced = next;
        }
        output
    };

    let whole = render(2_048);
    for chunk_length in [1, 3, 7, 64, 128, 511, 1_024] {
        assert_eq!(render(chunk_length), whole, "chunk length {chunk_length} changed the output");
    }
}

/// Research point 4: a muted voice's filter has to keep running, because the two-pole is a
/// *recursion* — its output depends on the two frames before it, so a gap in the delay
/// line is audible when the channel is unmuted. `accumulate_masked` renders muted voices
/// into a discard buffer for exactly this reason, and the filter is inside that.
#[test]
fn a_muted_voices_filter_keeps_its_delay_line_moving() {
    let filter = FilterParams::from_it(24, 110);
    let (blob, region) = noise_blob(4_096);
    let params = VoiceParams { step: Step::ONE, volume: U0F16::MAX, pan: I1F15::MIN, filter, ..VoiceParams::SILENT };

    // One pool renders 1024 frames muted then 1024 audible; the other renders all 2048
    // audible and its second half is what the first pool's second half must equal.
    let mut muted_first = VoicePool::new(1);
    let identifier = muted_first.allocate(VoiceTag::default(), region, params, 0).expect("a free slot");
    muted_first.get_mut(identifier).expect("live voice").settle_gains();
    let mut discard = vec![FixedFrame::default(); 1_024];
    let mut hidden = vec![FixedFrame::default(); 1_024];
    muted_first.accumulate_masked::<FixedPath, Linear>(&blob, &mut BusSegment::none(), &mut hidden, &mut discard, SAMPLE_RATE_HZ, |_| true);
    let mut heard = vec![FixedFrame::default(); 1_024];
    muted_first.accumulate_masked::<FixedPath, Linear>(&blob, &mut BusSegment::none(), &mut heard, &mut discard, SAMPLE_RATE_HZ, |_| false);

    let mut always_audible = VoicePool::new(1);
    let identifier = always_audible.allocate(VoiceTag::default(), region, params, 0).expect("a free slot");
    always_audible.get_mut(identifier).expect("live voice").settle_gains();
    let mut reference = vec![FixedFrame::default(); 2_048];
    always_audible.accumulate::<FixedPath, Linear>(&blob, &mut reference, SAMPLE_RATE_HZ);

    assert!(hidden.iter().all(|frame| *frame == FixedFrame::default()), "a muted voice must not reach the bus");
    assert_eq!(heard, reference[1_024..], "unmuting resumed a filter that had not been running");
}

/// A trigger resets the delay line, a sample swap under tone portamento does not
/// (accuracy policy D73; OpenMPT `FilterPortaSmpChange.it`).
#[test]
fn a_retrigger_resets_the_delay_line_but_a_sample_swap_does_not() {
    let filter = FilterParams::from_it(20, 90);
    let (blob, region) = noise_blob(4_096);
    let mut output = vec![FixedFrame::default(); 256];

    let mut voice = steady_voice(region, filter);
    accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, SAMPLE_RATE_HZ);
    assert_ne!(voice.filter().fixed.state, [0; 2], "the delay line should be carrying something by now");

    let mut retriggered = voice;
    retriggered.retrigger(0);
    assert_eq!(retriggered.filter().fixed.state, [0; 2], "a new note starts the filter from silence");

    let mut swapped = voice;
    swapped.set_region(region);
    assert_eq!(swapped.filter().fixed.state, voice.filter().fixed.state, "a mid-note sample swap is not a new note");
}

/// The coefficients are cached against the parameters they were derived from, so a voice
/// whose filter never moves never recomputes them — and one whose filter does move picks
/// the change up on the very next segment.
#[test]
fn coefficients_follow_the_parameters_without_a_dirty_bit() {
    let (blob, region) = noise_blob(1_024);
    let mut voice = steady_voice(region, FilterParams::from_it(20, 0));
    let mut output = vec![FixedFrame::default(); 64];
    accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, SAMPLE_RATE_HZ);
    let first = voice.filter().fixed.coefficients;
    assert_eq!(first, FixedPath::coefficients(20, 0, SAMPLE_RATE_HZ, false));

    voice.params.set_filter(FilterParams::from_it(90, 0));
    accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, SAMPLE_RATE_HZ);
    assert_eq!(voice.filter().fixed.coefficients, FixedPath::coefficients(90, 0, SAMPLE_RATE_HZ, false));

    // A sample-rate change is picked up the same way, without anyone having to say so.
    accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, 22_050);
    assert_eq!(voice.filter().fixed.coefficients, FixedPath::coefficients(90, 0, 22_050, false));
}

#[test]
fn the_extended_filter_range_opens_the_cutoff_further() {
    let (blob, region) = noise_blob(1_024);
    let mut standard = steady_voice(region, FilterParams::from_it(127, 40));
    let mut extended = steady_voice(region, FilterParams::from_it(127, 40));
    extended.filter_mut().set_extended_range(true);
    assert!(extended.filter().has_extended_range());

    let mut output = vec![FixedFrame::default(); 64];
    accumulate_voice::<FixedPath, Linear>(&mut standard, &blob, &mut output, SAMPLE_RATE_HZ);
    accumulate_voice::<FixedPath, Linear>(&mut extended, &blob, &mut output, SAMPLE_RATE_HZ);
    assert_ne!(standard.filter().fixed.coefficients, extended.filter().fixed.coefficients);

    // A wider-open filter passes more of a broadband signal, so its `input_gain` is larger.
    assert!(extended.filter().fixed.coefficients.input_gain > standard.filter().fixed.coefficients.input_gain);
}

/// A guard on the seam rather than on the filter: the interpolate/accumulate split the
/// filter was threaded through must still be exactly what `MixPath::mix` does.
#[test]
fn the_split_seam_is_the_same_arithmetic_as_mix() {
    let frames: Vec<i16> = noise(64);
    for &fraction in &[0u32, 1, 0x4000_0000, 0x8000_0000, 0xFFFF_FFFF] {
        for index in 0..32usize {
            let gains = FixedPath::gains(U0F16::MAX, I1F15::ZERO);
            let mut through_mix = FixedFrame::default();
            FixedPath::mix::<Linear>(&mut through_mix, &frames, index, fraction, gains);
            let mut through_parts = FixedFrame::default();
            FixedPath::accumulate(&mut through_parts, FixedPath::interpolate::<Linear>(&frames, index, fraction), gains);
            assert_eq!(through_mix, through_parts);

            let gains = FloatPath::gains(U0F16::MAX, I1F15::ZERO);
            let mut through_mix = FloatFrame::default();
            FloatPath::mix::<Linear>(&mut through_mix, &frames, index, fraction, gains);
            let mut through_parts = FloatFrame::default();
            FloatPath::accumulate(&mut through_parts, FloatPath::interpolate::<Linear>(&frames, index, fraction), gains);
            assert_eq!(through_mix.left.to_bits(), through_parts.left.to_bits());
            assert_eq!(through_mix.right.to_bits(), through_parts.right.to_bits());
        }
    }
}

/// A looping sample filters the same way an unlooped one does — the delay line belongs to
/// the voice, not to the sample, so a wrap does not disturb it.
#[test]
fn a_loop_wrap_does_not_disturb_the_delay_line() {
    let mut blob = Vec::new();
    let region = append_guarded_sample(&mut blob, &noise(64), LoopSpan::new(0, 64));
    let mut voice = steady_voice(region, FilterParams::from_it(50, 80));
    let mut output = vec![FixedFrame::default(); 512];
    accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, SAMPLE_RATE_HZ);
    assert!(output.iter().any(|frame| frame.left != 0), "the voice should still be sounding after eight wraps");
}
