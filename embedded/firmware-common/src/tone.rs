//! Integer-only diagnostic sine used by board audio-path experiments.

use starplayer_model::{InstrumentDef, Module, ModuleBuilder, ModuleFormat, ModuleHeader, ORDER_END, SampleSpec};

/// Peak signed amplitude of the diagnostic tone, about −18 dBFS.
pub const DIAGNOSTIC_TONE_AMPLITUDE: i16 = 4_096;

/// Human-readable exact tone frequency at 44 100 Hz.
pub const DIAGNOSTIC_TONE_FREQUENCY: &str = "172.265625 Hz";

/// Frames of tone in one gate interval: 172 exact 256-frame table cycles.
pub const DIAGNOSTIC_TONE_ACTIVE_FRAMES: u32 = 44_032;

/// Frames of exact digital zero following each active interval.
pub const DIAGNOSTIC_TONE_SILENT_FRAMES: u32 = 44_032;

/// Audible frequency of the standalone engine-path diagnostic.
pub const ENGINE_TONE_OUTPUT_FREQUENCY_HZ: u32 = 125;

/// Native playback rate attached to the engine-tone sample.
pub const ENGINE_TONE_REFERENCE_RATE_HZ: u32 = 32_000;

/// Signed peak amplitude of the engine-tone's real 16-bit sample.
pub const ENGINE_TONE_SOURCE_AMPLITUDE: i16 = 28_672;

/// Frames in the engine-tone's whole-sample forward loop.
pub const ENGINE_TONE_LOOP_FRAMES: u32 = 256;

/// Frequency of the strict post-render comparison against the engine tone.
pub const MATCHED_TONE_FREQUENCY_HZ: u32 = 125;

/// Signed peak written to the left channel by [`MatchedTone`].
pub const MATCHED_TONE_LEFT_PEAK: i16 = 4_796;

/// Signed peak written to the right channel by [`MatchedTone`].
pub const MATCHED_TONE_RIGHT_PEAK: i16 = 5_327;

/// Rounded full-turn phase increment for 125 Hz at 44 100 Hz.
pub const MATCHED_TONE_PHASE_INCREMENT: u32 =
    ((MATCHED_TONE_FREQUENCY_HZ as u64 * (1u64 << 32) + crate::SAMPLE_RATE_HZ as u64 / 2) / crate::SAMPLE_RATE_HZ as u64) as u32;

const TABLE_LENGTH: usize = 256;
const PHASE_STEP: usize = 1;
const TABLE_CYCLE_FRAMES: u32 = 256;
const GATE_FRAMES: u32 = DIAGNOSTIC_TONE_ACTIVE_FRAMES + DIAGNOSTIC_TONE_SILENT_FRAMES;
const ENGINE_TONE_SAMPLE_SCALE: i16 = 7;
const S3M_CELL_BYTES: usize = 5;
const ENGINE_TONE_PATTERN_BYTES: usize = starplayer::s3m::ROWS as usize * S3M_CELL_BYTES;

const _: () = assert!(TABLE_LENGTH.is_power_of_two());
const _: () = assert!(TABLE_CYCLE_FRAMES as usize * PHASE_STEP == TABLE_LENGTH);
const _: () = assert!(DIAGNOSTIC_TONE_ACTIVE_FRAMES == 172 * TABLE_CYCLE_FRAMES);
const _: () = assert!(DIAGNOSTIC_TONE_SILENT_FRAMES == 172 * TABLE_CYCLE_FRAMES);
const _: () = assert!(ENGINE_TONE_LOOP_FRAMES as usize == TABLE_LENGTH);
const _: () = assert!(DIAGNOSTIC_TONE_AMPLITUDE as i32 * ENGINE_TONE_SAMPLE_SCALE as i32 == ENGINE_TONE_SOURCE_AMPLITUDE as i32);
const _: () = assert!(ENGINE_TONE_REFERENCE_RATE_HZ / ENGINE_TONE_LOOP_FRAMES == ENGINE_TONE_OUTPUT_FREQUENCY_HZ);
const _: () = assert!(MATCHED_TONE_PHASE_INCREMENT == 12_173_944);

/// One full, rounded 256-entry sine cycle at [`DIAGNOSTIC_TONE_AMPLITUDE`].
const SINE: [i16; TABLE_LENGTH] = [
    0, 101, 201, 301, 401, 501, 601, 700, 799, 897, 995, 1092, 1189, 1285, 1380, 1474,
    1567, 1660, 1751, 1842, 1931, 2019, 2106, 2191, 2276, 2359, 2440, 2520, 2598, 2675, 2751, 2824,
    2896, 2967, 3035, 3102, 3166, 3229, 3290, 3349, 3406, 3461, 3513, 3564, 3612, 3659, 3703, 3745,
    3784, 3822, 3857, 3889, 3920, 3948, 3973, 3996, 4017, 4036, 4052, 4065, 4076, 4085, 4091, 4095,
    4096, 4095, 4091, 4085, 4076, 4065, 4052, 4036, 4017, 3996, 3973, 3948, 3920, 3889, 3857, 3822,
    3784, 3745, 3703, 3659, 3612, 3564, 3513, 3461, 3406, 3349, 3290, 3229, 3166, 3102, 3035, 2967,
    2896, 2824, 2751, 2675, 2598, 2520, 2440, 2359, 2276, 2191, 2106, 2019, 1931, 1842, 1751, 1660,
    1567, 1474, 1380, 1285, 1189, 1092, 995, 897, 799, 700, 601, 501, 401, 301, 201, 101,
    0, -101, -201, -301, -401, -501, -601, -700, -799, -897, -995, -1092, -1189, -1285, -1380, -1474,
    -1567, -1660, -1751, -1842, -1931, -2019, -2106, -2191, -2276, -2359, -2440, -2520, -2598, -2675, -2751, -2824,
    -2896, -2967, -3035, -3102, -3166, -3229, -3290, -3349, -3406, -3461, -3513, -3564, -3612, -3659, -3703, -3745,
    -3784, -3822, -3857, -3889, -3920, -3948, -3973, -3996, -4017, -4036, -4052, -4065, -4076, -4085, -4091, -4095,
    -4096, -4095, -4091, -4085, -4076, -4065, -4052, -4036, -4017, -3996, -3973, -3948, -3920, -3889, -3857, -3822,
    -3784, -3745, -3703, -3659, -3612, -3564, -3513, -3461, -3406, -3349, -3290, -3229, -3166, -3102, -3035, -2967,
    -2896, -2824, -2751, -2675, -2598, -2520, -2440, -2359, -2276, -2191, -2106, -2019, -1931, -1842, -1751, -1660,
    -1567, -1474, -1380, -1285, -1189, -1092, -995, -897, -799, -700, -601, -501, -401, -301, -201, -101,
];

/// Build the standalone one-channel native S3M used to isolate the engine path.
///
/// Construction allocates and is called once before playback. The sample retains the sine
/// table's real 16-bit values, loops across its full 256 frames, and is played by a native C-4
/// cell at volume 64. Rows 1..62 are empty fixed-stride [`starplayer::s3m::S3mCell`]s;
/// row 63 carries native `B00`, looping to order zero without growing the scanned timeline.
pub fn engine_tone_module() -> Result<Module, starplayer::core::Error> {
    let pcm: [i16; TABLE_LENGTH] = core::array::from_fn(|index| {
        SINE.get(index).copied().unwrap_or(0) * ENGINE_TONE_SAMPLE_SCALE
    });

    let mut pattern = [0u8; ENGINE_TONE_PATTERN_BYTES];
    for cell in pattern.chunks_exact_mut(S3M_CELL_BYTES) {
        cell.copy_from_slice(&starplayer::s3m::S3mCell::EMPTY.to_bytes());
    }
    if let Some(row_zero) = pattern.chunks_exact_mut(S3M_CELL_BYTES).next() {
        row_zero.copy_from_slice(
            &starplayer::s3m::S3mCell { note: 0x40, instrument: 1, volume: 64, command: 0, info: 0 }.to_bytes(),
        );
    }
    if let Some(row_63) = pattern.chunks_exact_mut(S3M_CELL_BYTES).last() {
        row_63.copy_from_slice(&starplayer::s3m::S3mCell { command: 2, info: 0, ..starplayer::s3m::S3mCell::EMPTY }.to_bytes());
    }

    let mut builder = ModuleBuilder::new();
    let sample = builder.add_sample(
        &pcm,
        SampleSpec::one_shot("engine-tone")
            .with_forward_loop(0, ENGINE_TONE_LOOP_FRAMES)
            .with_reference_rate(ENGINE_TONE_REFERENCE_RATE_HZ),
    )?;
    builder.add_instrument(InstrumentDef::from_sample("engine-tone", sample, starplayer::core::U0F16::MAX))?;
    let pattern = builder.add_pattern(&pattern, starplayer::s3m::ROWS, 1)?;
    builder.set_orders(&[pattern.0, ORDER_END]);
    let mut header = ModuleHeader::new(ModuleFormat::S3m, 1);
    header.title = "ENGINE-TONE".into();
    builder.set_header(header);
    builder.build()
}

/// Continuous post-render 125 Hz comparison matched to the host engine-tone channel peaks.
///
/// A wrapping `u32` represents one full turn. Every output frame reads the shared integer Q15
/// sine table, scales the channels independently, then advances by the exact rounded phase
/// increment. The state is allocation-free and remains owned by the audio refill from prefill
/// onward.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchedTone {
    phase: u32,
}

impl MatchedTone {
    /// Start at the rising zero crossing.
    pub const fn new() -> MatchedTone { MatchedTone { phase: 0 } }

    /// Replace interleaved stereo with the continuous matched tone.
    ///
    /// An unexpected unpaired trailing sample is zeroed without advancing phase. This keeps an
    /// invalid caller slice harmless without logging, allocating, locking or panicking.
    pub fn overwrite(&mut self, destination: &mut [i16]) {
        let mut frames = destination.chunks_exact_mut(2);
        for frame in &mut frames {
            let sine = starplayer::dsp::sin_q15(self.phase);
            frame[0] = scale_q15_to_peak(sine, MATCHED_TONE_LEFT_PEAK);
            frame[1] = scale_q15_to_peak(sine, MATCHED_TONE_RIGHT_PEAK);
            self.phase = self.phase.wrapping_add(MATCHED_TONE_PHASE_INCREMENT);
        }
        frames.into_remainder().fill(0);
    }
}

#[inline(always)]
fn scale_q15_to_peak(sine: i32, peak: i16) -> i16 {
    let magnitude = (i64::from(sine.abs()) * i64::from(peak) + i64::from(i16::MAX) / 2) / i64::from(i16::MAX);
    if sine < 0 { (-magnitude) as i16 } else { magnitude as i16 }
}

/// Stateful dual-mono tone/zero generator. Phase and gate position continue across calls.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticTone {
    phase: usize,
    gate_frame: u32,
}

impl DiagnosticTone {
    /// Start at the sine's zero crossing and the beginning of the active gate.
    pub const fn new() -> DiagnosticTone { DiagnosticTone { phase: 0, gate_frame: 0 } }

    /// Overwrite interleaved stereo samples with tone or exact gated silence.
    ///
    /// Integer lookup and phase arithmetic only. An unexpected unpaired trailing sample is set
    /// to zero rather than leaking the rendered engine output into the diagnostic stream.
    pub fn overwrite(&mut self, destination: &mut [i16]) {
        let mut frames = destination.chunks_exact_mut(2);
        for frame in &mut frames {
            let sample = if self.gate_frame < DIAGNOSTIC_TONE_ACTIVE_FRAMES {
                SINE.get(self.phase).copied().unwrap_or(0)
            } else {
                0
            };
            frame.fill(sample);
            self.phase = self.phase.wrapping_add(PHASE_STEP) & (TABLE_LENGTH - 1);
            self.gate_frame = self.gate_frame.wrapping_add(1);
            if self.gate_frame >= GATE_FRAMES {
                self.phase = 0;
                self.gate_frame = 0;
            }
        }
        frames.into_remainder().fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use starplayer::core::{U0F16, SampleId};
    use starplayer::dsp::Linear;
    use starplayer::model::{LoopMode, PatternId};
    use starplayer::rt::Arc;
    use starplayer_host_embedded::EmbeddedPlayer;

    #[test]
    fn table_has_both_exact_peaks_and_opposite_signed_halves() {
        assert_eq!(SINE.iter().copied().min(), Some(-DIAGNOSTIC_TONE_AMPLITUDE));
        assert_eq!(SINE.iter().copied().max(), Some(DIAGNOSTIC_TONE_AMPLITUDE));
        for index in 0..TABLE_LENGTH / 2 {
            assert_eq!(SINE[index + TABLE_LENGTH / 2], -SINE[index]);
        }
    }

    #[test]
    fn output_is_dual_mono_in_range_and_phase_continues_across_calls() {
        let mut period_tone = DiagnosticTone::new();
        let mut period = [i16::MAX; TABLE_CYCLE_FRAMES as usize * 2];
        period_tone.overwrite(&mut period);
        let mut minimum = i16::MAX;
        let mut maximum = i16::MIN;
        for frame in period.chunks_exact(2) {
            assert_eq!(frame[0], frame[1]);
            assert!(frame[0].abs() <= DIAGNOSTIC_TONE_AMPLITUDE);
            minimum = minimum.min(frame[0]);
            maximum = maximum.max(frame[0]);
        }
        assert_eq!(minimum, -DIAGNOSTIC_TONE_AMPLITUDE);
        assert_eq!(maximum, DIAGNOSTIC_TONE_AMPLITUDE);

        let mut continuous = DiagnosticTone::new();
        let mut whole = [i16::MAX; 12];
        continuous.overwrite(&mut whole);

        let mut split = DiagnosticTone::new();
        let mut first = [i16::MAX; 4];
        let mut second = [i16::MAX; 8];
        split.overwrite(&mut first);
        split.overwrite(&mut second);

        assert_eq!(&whole[..4], &first);
        assert_eq!(&whole[4..], &second);
    }

    #[test]
    fn gate_changes_only_on_cycle_boundaries_and_silence_is_exact_zero() {
        let mut tone = DiagnosticTone::new();
        let mut table_cycle = [i16::MAX; TABLE_CYCLE_FRAMES as usize * 2];

        for _ in 0..172 {
            tone.overwrite(&mut table_cycle);
        }
        assert_eq!(tone.gate_frame, DIAGNOSTIC_TONE_ACTIVE_FRAMES);
        assert_eq!(tone.phase, 0, "the active gate ends at the sine's zero crossing");

        for _ in 0..172 {
            table_cycle.fill(i16::MAX);
            tone.overwrite(&mut table_cycle);
            assert!(table_cycle.iter().all(|sample| *sample == 0));
        }
        assert_eq!(tone.gate_frame, 0);
        assert_eq!(tone.phase, 0, "the complete on/off gate resets at the same zero crossing");

        let mut restarted = [i16::MAX; 4];
        tone.overwrite(&mut restarted);
        assert_eq!(restarted, [0, 0, SINE[PHASE_STEP], SINE[PHASE_STEP]]);
    }

    #[test]
    fn matched_tone_phase_increment_is_the_exact_nearest_full_turn_step() {
        let numerator = u64::from(MATCHED_TONE_FREQUENCY_HZ) * (1u64 << 32);
        let sample_rate = u64::from(crate::SAMPLE_RATE_HZ);
        assert_eq!(MATCHED_TONE_PHASE_INCREMENT, 12_173_944);
        let chosen_error = (u64::from(MATCHED_TONE_PHASE_INCREMENT) * sample_rate).abs_diff(numerator);
        let lower_error = (u64::from(MATCHED_TONE_PHASE_INCREMENT - 1) * sample_rate).abs_diff(numerator);
        let upper_error = (u64::from(MATCHED_TONE_PHASE_INCREMENT + 1) * sample_rate).abs_diff(numerator);
        assert!(chosen_error < lower_error && chosen_error < upper_error);
    }

    #[test]
    fn matched_tone_hits_independent_signed_channel_peaks() {
        let mut positive = MatchedTone { phase: 1 << 30 };
        let mut frame = [0i16; 2];
        positive.overwrite(&mut frame);
        assert_eq!(frame, [MATCHED_TONE_LEFT_PEAK, MATCHED_TONE_RIGHT_PEAK]);

        let mut negative = MatchedTone { phase: 3 << 30 };
        negative.overwrite(&mut frame);
        assert_eq!(frame, [-MATCHED_TONE_LEFT_PEAK, -MATCHED_TONE_RIGHT_PEAK]);
    }

    #[test]
    fn matched_tone_is_continuous_bounded_and_keeps_channel_polarity() {
        let mut tone = MatchedTone::new();
        let mut previous: Option<[i16; 2]> = None;
        for _ in 0..crate::SAMPLE_RATE_HZ as usize * 2 / 128 {
            let mut block = [i16::MAX; 128 * 2];
            tone.overwrite(&mut block);
            assert!(block.iter().any(|sample| *sample != 0), "the continuous comparison has no silence gate");
            for frame in block.chunks_exact(2) {
                assert!(frame[0].abs() <= MATCHED_TONE_LEFT_PEAK);
                assert!(frame[1].abs() <= MATCHED_TONE_RIGHT_PEAK);
                assert!(i32::from(frame[0]) * i32::from(frame[1]) >= 0);
                if let Some(previous_frame) = previous {
                    assert!(frame[0].abs_diff(previous_frame[0]) <= 128);
                    assert!(frame[1].abs_diff(previous_frame[1]) <= 128);
                }
                previous = Some([frame[0], frame[1]]);
            }
        }
    }

    #[test]
    fn matched_tone_phase_continues_across_unequal_calls_and_ignores_an_unpaired_sample() {
        let mut whole_tone = MatchedTone::new();
        let mut whole = [i16::MAX; 514];
        whole_tone.overwrite(&mut whole);

        let mut split_tone = MatchedTone::new();
        let mut split = [i16::MAX; 514];
        split_tone.overwrite(&mut split[..6]);
        split_tone.overwrite(&mut split[6..204]);
        split_tone.overwrite(&mut split[204..]);
        assert_eq!(split, whole);
        assert_eq!(split_tone, whole_tone);

        let mut odd_tone = MatchedTone::new();
        let mut odd = [i16::MAX; 3];
        odd_tone.overwrite(&mut odd);
        assert_eq!(odd, [0, 0, 0]);
        let mut following = [i16::MAX; 2];
        odd_tone.overwrite(&mut following);
        assert_eq!(following, &whole[2..4]);
    }

    #[test]
    fn engine_tone_is_a_native_one_channel_s3m_with_fixed_stride_cells() {
        let module = engine_tone_module().expect("engine-tone module");
        assert_eq!(module.header().format, ModuleFormat::S3m);
        assert_eq!(module.header().channel_count, 1);
        assert_eq!(module.header().title.as_ref(), "ENGINE-TONE");
        assert_eq!(module.samples().len(), 1);
        assert_eq!(module.instruments().len(), 1);
        assert_eq!(module.patterns().len(), 1);
        assert_eq!(module.orders(), &[0, ORDER_END]);
        assert_eq!(module.pattern_bytes(PatternId(0)).map(<[u8]>::len), Some(ENGINE_TONE_PATTERN_BYTES));

        let pattern = starplayer::s3m::PatternView::new(&module, PatternId(0)).expect("native S3M pattern");
        assert_eq!(pattern.cell(0, 0), Some(starplayer::s3m::S3mCell { note: 0x40, instrument: 1, volume: 64, command: 0, info: 0 }));
        for row in 1..starplayer::s3m::ROWS - 1 {
            assert_eq!(pattern.cell(row, 0), Some(starplayer::s3m::S3mCell::EMPTY));
        }
        assert_eq!(pattern.cell(starplayer::s3m::ROWS - 1, 0), Some(starplayer::s3m::S3mCell { command: 2, info: 0, ..starplayer::s3m::S3mCell::EMPTY }));
    }

    #[test]
    fn engine_tone_sample_keeps_full_precision_and_whole_forward_loop() {
        let module = engine_tone_module().expect("engine-tone module");
        let sample = module.sample(SampleId(0)).expect("engine-tone sample");
        assert_eq!(sample.length_frames(), ENGINE_TONE_LOOP_FRAMES);
        assert_eq!(sample.loop_mode(), LoopMode::Forward);
        assert_eq!(sample.loop_start(), 0);
        assert_eq!(sample.loop_end(), ENGINE_TONE_LOOP_FRAMES);
        assert_eq!(sample.reference_rate_hz(), ENGINE_TONE_REFERENCE_RATE_HZ);

        let pcm = module.sample_pcm(SampleId(0)).expect("engine-tone PCM and guard");
        let body = pcm.get(..ENGINE_TONE_LOOP_FRAMES as usize).expect("whole sample body");
        for (actual, table) in body.iter().zip(SINE.iter()) {
            assert_eq!(*actual, *table * ENGINE_TONE_SAMPLE_SCALE);
        }
        assert_eq!(body.iter().copied().min(), Some(-ENGINE_TONE_SOURCE_AMPLITUDE));
        assert_eq!(body.iter().copied().max(), Some(ENGINE_TONE_SOURCE_AMPLITUDE));
        assert!(body.iter().any(|sample| *sample % 256 != 0), "the source retains values finer than widened 8-bit PCM");
        assert_eq!(pcm.get(ENGINE_TONE_LOOP_FRAMES as usize), body.first(), "the linear guard wraps to frame zero");
    }

    #[test]
    fn engine_tone_crosses_the_b00_loop_bounded_and_without_warnings() {
        let module = Arc::new(engine_tone_module().expect("engine-tone module"));
        let (mut render, mut control) = EmbeddedPlayer::<Linear>::open(module, crate::SAMPLE_RATE_HZ).expect("engine-tone player");
        let scanned_frames = control.song_length().expect("B00 produces a bounded scanned loop");
        assert!((u64::from(crate::SAMPLE_RATE_HZ) * 7..=u64::from(crate::SAMPLE_RATE_HZ) * 8).contains(&scanned_frames));
        control.set_master_volume(U0F16::from_bits(16_384)).expect("quarter master");
        control.play().expect("play engine tone");
        assert_eq!(control.at_end(), starplayer::core::AtEnd::Continue);

        let mut heard_signal = false;
        let mut heard_signal_after_first_pattern = false;
        let mut peak = 0u16;
        let mut stereo_difference = 0u16;
        let mut rendered_frames = 0usize;
        let quanta_for_ten_seconds = (crate::SAMPLE_RATE_HZ as usize * 10).div_ceil(starplayer::engine::RENDER_QUANTUM);
        for _ in 0..quanta_for_ten_seconds {
            let mut quantum = [0i16; starplayer::engine::RENDER_QUANTUM * 2];
            peak = peak.max(render.render(&mut quantum) as u16);
            for frame in quantum.chunks_exact(2) {
                assert!(i32::from(frame[0]) * i32::from(frame[1]) >= 0, "both outputs carry the same-polarity mono source");
                stereo_difference = stereo_difference.max(frame[0].abs_diff(frame[1]));
                heard_signal |= frame[0] != 0 && frame[1] != 0;
                if rendered_frames >= crate::SAMPLE_RATE_HZ as usize * 8 {
                    heard_signal_after_first_pattern |= frame[0] != 0 && frame[1] != 0;
                }
                rendered_frames += 1;
            }
        }

        assert!(heard_signal);
        assert!(heard_signal_after_first_pattern, "B00 keeps the tone audible after the first 7.68-second pattern");
        assert!(peak > 0 && peak <= (ENGINE_TONE_SOURCE_AMPLITUDE as u16 / 4), "quarter-master peak stays bounded: {peak}");
        assert!(stereo_difference <= peak / 8 + 2, "S3M's native centre nibble keeps both channels close: difference {stereo_difference}, peak {peak}");
        assert!(!control.warnings().any(), "multi-quantum engine-tone playback raises no warning: {:?}", control.warnings());
    }
}
