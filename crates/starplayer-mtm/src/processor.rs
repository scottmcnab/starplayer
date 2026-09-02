//! Native MTM row decoding over the shared ProTracker-compatible effect core.
//!
//! The boundary is deliberate: track indirection and three-byte cells remain MTM-owned,
//! while commands 0..F reuse the mature tick machinery in `starplayer-mod`. The shared
//! core has an explicit MultiTracker personality for the source-backed differences:
//! hexadecimal Dxx, immediate Fxx, optional speed/BPM counterpart resets, no Amiga
//! period clamp, and no ProTracker double-offset pointer bug.

use starplayer_core::{Frame, TempoModel};
use starplayer_engine::{EndOfSongPolicy, OrderEntry, PatternData, PatternSequencer, RowRef, SequencerSettings, TickContext, TickOutcome, TrackerProcessor};
use starplayer_model::{Module, OrderEntry as ModelOrderEntry};
use starplayer_mod::{EffectCell, EffectNote, EffectSemantics, ModChannel, ModProcessor};
use starplayer_rt::Arc;

use crate::loader::{TempoMode, tempo_mode};
use crate::pattern::{CELL_BYTES, MtmCell};

/// Owned native MTM pattern access for the generic sequencer.
#[derive(Clone, Debug)]
pub struct MtmPatternData(pub Arc<Module>);

impl PatternData for MtmPatternData {
    fn order_count(&self) -> u16 { self.0.orders().len().min(u16::MAX as usize) as u16 }

    fn order(&self, order: u16) -> Option<OrderEntry> {
        match self.0.order_entry(order as usize)? {
            ModelOrderEntry::Pattern(pattern) => Some(OrderEntry::Pattern(pattern.0)),
            ModelOrderEntry::Marker => Some(OrderEntry::Skip),
            ModelOrderEntry::End => Some(OrderEntry::End),
        }
    }

    fn channel_count(&self) -> u8 { self.0.header().channel_count }
    fn rows_in_pattern(&self, pattern: u16) -> Option<u16> { self.0.pattern(starplayer_model::PatternId(pattern)).map(|index| index.rows()) }

    fn row_bytes(&self, pattern: u16, row: u16) -> Option<&[u8]> {
        let pattern_id = starplayer_model::PatternId(pattern);
        let index = self.0.pattern(pattern_id)?;
        if row >= index.rows() { return None; }
        let row_length = index.channels() as usize * CELL_BYTES;
        let start = row as usize * row_length;
        self.0.pattern_bytes(pattern_id)?.get(start..start + row_length)
    }
}

/// MultiTracker's native processor. Serialized MOD cells and S3M commands never enter
/// this type; only decoded command semantics cross into the shared effect core.
pub struct MtmProcessor {
    effects: ModProcessor,
}

impl MtmProcessor {
    pub fn new(module: Arc<Module>, sample_rate_hz: u32) -> MtmProcessor {
        let reset_counterpart = tempo_mode(&module) == Some(TempoMode::MultiTracker);
        MtmProcessor {
            effects: ModProcessor::with_semantics(module, sample_rate_hz, EffectSemantics::MultiTracker { reset_counterpart }),
        }
    }

    pub fn channels(&self) -> &[ModChannel] { self.effects.channels() }
    pub fn channel(&self, channel: u8) -> Option<&ModChannel> { self.effects.channel(channel) }
}

impl TrackerProcessor for MtmProcessor {
    fn row(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) -> TickOutcome {
        let cells = row.bytes.chunks_exact(CELL_BYTES).map(|bytes| {
            let cell = MtmCell::from_bytes(bytes).unwrap_or(MtmCell::EMPTY);
            EffectCell {
                note: cell.linear_note().map(EffectNote::Linear).unwrap_or(EffectNote::None),
                instrument: cell.instrument,
                effect: cell.effect,
                param: cell.param,
            }
        });
        self.effects.row_effects(context, row.order, row.pattern, row.row, cells)
    }

    fn tick(&mut self, context: &mut TickContext<'_>) -> TickOutcome { self.effects.tick(context) }

    /// MTM owns no replay state of its own — track indirection is resolved at load time —
    /// so the whole reset is the shared effect core's.
    fn reset(&mut self) { self.effects.reset(); }
}

/// Build a public MTM sequencer using native pattern data and MultiTracker timing.
pub fn sequencer_for<Tempo: TempoModel>(module: Arc<Module>, sample_rate_hz: u32, tempo_model: Tempo) -> PatternSequencer<Tempo, MtmProcessor, MtmPatternData> {
    let settings = SequencerSettings {
        sample_rate_hz,
        first_tick_frame: Frame::ZERO,
        initial_speed: module.header().initial_speed,
        initial_tempo_bpm: module.header().initial_tempo,
        restart_order: 0,
        end_of_song: EndOfSongPolicy::Loop,
    };
    PatternSequencer::new(tempo_model, MtmPatternData(Arc::clone(&module)), MtmProcessor::new(module, sample_rate_hz), settings)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use alloc::vec;

    use starplayer_core::{ChannelId, RowClock, U0F16};
    use starplayer_dsp::Linear;
    use starplayer_engine::{ChannelTable, SongPosition};
    use starplayer_mixer::{FixedFrame, FixedPath, VoicePool, VoiceStatus, accumulate_voice};
    use starplayer_model::{InstrumentDef, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec, ORDER_END};

    use super::*;

    fn processor(native_tempo: bool, sample_frames: usize) -> MtmProcessor {
        processor_with_instruments(native_tempo, sample_frames, 1)
    }

    fn processor_with_instruments(native_tempo: bool, sample_frames: usize, instrument_count: usize) -> MtmProcessor {
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&[0; 64 * CELL_BYTES], 64, 1).expect("pattern");
        let sample = builder.add_sample(&vec![0; sample_frames], SampleSpec::one_shot("sample")).expect("sample");
        for _ in 0..instrument_count {
            builder.add_instrument(InstrumentDef::from_sample("sample", sample, U0F16::MAX)).expect("instrument");
        }
        builder.set_orders(&[0, ORDER_END]);
        let mut header = ModuleHeader::new(ModuleFormat::Mtm, 1);
        if native_tempo { header.format_extra = 1 << 16; }
        builder.set_header(header);
        MtmProcessor::new(Arc::new(builder.build().expect("module")), 44_100)
    }

    fn row(processor: &mut MtmProcessor, cell: MtmCell) -> TickOutcome {
        row_with_timing(processor, cell, 6, 125)
    }

    fn row_with_timing(processor: &mut MtmProcessor, cell: MtmCell, speed: u8, tempo_bpm: u16) -> TickOutcome {
        execute_row(processor, cell, speed, tempo_bpm).0
    }

    fn execute_row(processor: &mut MtmProcessor, cell: MtmCell, speed: u8, tempo_bpm: u16) -> (TickOutcome, VoicePool, ChannelTable) {
        let mut voices = VoicePool::new(2);
        let mut channels = ChannelTable::new(1);
        let bytes = cell.to_bytes();
        let outcome = {
            let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(speed), SongPosition::default(), tempo_bpm);
            processor.row(&mut context, RowRef { order: 0, pattern: 0, row: 0, bytes: &bytes })
        };
        (outcome, voices, channels)
    }

    fn processor_with_loop(looped: bool) -> (MtmProcessor, Arc<Module>) {
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&[0; 64 * CELL_BYTES], 64, 1).expect("pattern");
        let specification = if looped { SampleSpec::one_shot("sample").with_forward_loop(128, 384) } else { SampleSpec::one_shot("sample") };
        let sample = builder.add_sample(&vec![1_000; 512], specification).expect("sample");
        builder.add_instrument(InstrumentDef::from_sample("sample", sample, U0F16::MAX)).expect("instrument");
        builder.set_orders(&[0, ORDER_END]);
        let mut header = ModuleHeader::new(ModuleFormat::Mtm, 1);
        header.format_extra = 1 << 16;
        builder.set_header(header);
        let module = Arc::new(builder.build().expect("module"));
        (MtmProcessor::new(Arc::clone(&module), 44_100), module)
    }

    #[test]
    fn native_pitch_enters_the_shared_core_as_a_linear_note() {
        let mut processor = processor(true, 1024);
        let _ = row(&mut processor, MtmCell { pitch: 12, instrument: 1, effect: 0, param: 0 });
        let channel = processor.channel(0).expect("channel");
        assert_eq!(channel.current_note, 36);
        assert_eq!(channel.current_period, 856);
    }

    #[test]
    fn pattern_break_is_hexadecimal_instead_of_protracker_bcd() {
        let mut processor = processor(true, 1024);
        assert_eq!(row(&mut processor, MtmCell { effect: 0xD, param: 0x31, ..MtmCell::EMPTY }).jump, Some(starplayer_engine::Jump::break_to_row(49)));
    }

    #[test]
    fn native_tempo_resets_the_counterpart_while_dmp_timing_does_not() {
        let mut native = processor(true, 1024);
        let speed = row_with_timing(&mut native, MtmCell { effect: 0xF, param: 3, ..MtmCell::EMPTY }, 4, 140);
        assert_eq!((speed.speed, speed.tempo_bpm), (3, 125));
        let tempo = row_with_timing(&mut native, MtmCell { effect: 0xF, param: 150, ..MtmCell::EMPTY }, 4, 140);
        assert_eq!((tempo.speed, tempo.tempo_bpm), (6, 150));

        let mut dmp = processor(false, 1024);
        let speed = row_with_timing(&mut dmp, MtmCell { effect: 0xF, param: 3, ..MtmCell::EMPTY }, 4, 140);
        assert_eq!((speed.speed, speed.tempo_bpm), (3, 140));
        let tempo = row_with_timing(&mut dmp, MtmCell { effect: 0xF, param: 150, ..MtmCell::EMPTY }, 4, 140);
        assert_eq!((tempo.speed, tempo.tempo_bpm), (4, 150));
    }

    #[test]
    fn sample_offset_is_a_single_absolute_offset_not_the_pt_pointer_quirk() {
        let mut processor = processor(true, 1024);
        let _ = row(&mut processor, MtmCell { pitch: 12, instrument: 1, effect: 9, param: 1 });
        assert_eq!(processor.channel(0).map(|channel| channel.sample_offset), Some(256));
    }

    #[test]
    fn a_past_end_mtm_offset_keeps_the_one_shot_region_and_ends_in_the_mixer() {
        let (mut processor, module) = processor_with_loop(false);
        let (_, mut voices, channels) = execute_row(&mut processor, MtmCell { pitch: 12, instrument: 1, effect: 9, param: 2 }, 6, 125);
        let voice_id = channels.foreground(ChannelId(0)).expect("triggered voice");
        let voice = voices.get(voice_id).expect("live voice before mixing");
        assert_eq!(voice.position(), 512u64 << 32);
        assert_eq!(voice.region().length_frames(), 512);
        assert_eq!(voice.region().loop_span(), None);
        assert!(!processor.channel(0).expect("channel").offset_past_end);

        let mut output = [FixedFrame::default(); 4];
        let status = accumulate_voice::<FixedPath, Linear>(voices.get_mut(voice_id).expect("live voice"), module.pcm(), &mut output);
        assert_eq!(status, VoiceStatus::Finished);
        assert_eq!(output, [FixedFrame::default(); 4]);
    }

    #[test]
    fn a_past_end_mtm_offset_keeps_the_loop_region_for_mixer_repositioning() {
        let (mut processor, module) = processor_with_loop(true);
        let (_, mut voices, channels) = execute_row(&mut processor, MtmCell { pitch: 12, instrument: 1, effect: 9, param: 2 }, 6, 125);
        let voice_id = channels.foreground(ChannelId(0)).expect("triggered voice");
        let voice = voices.get(voice_id).expect("live voice before mixing");
        assert_eq!(voice.position(), 512u64 << 32);
        assert_eq!(voice.region().loop_span().map(|span| (span.start(), span.end())), Some((128, 384)));
        assert!(!processor.channel(0).expect("channel").offset_past_end);

        let mut output = [FixedFrame::default(); 1];
        let status = accumulate_voice::<FixedPath, Linear>(voices.get_mut(voice_id).expect("live voice"), module.pcm(), &mut output);
        assert_eq!(status, VoiceStatus::Sounding);
        let normalized = voices.get(voice_id).expect("looping voice").position();
        assert!((128u64 << 32..384u64 << 32).contains(&normalized), "the mixer wrapped the absolute offset into the loop");
    }

    #[test]
    fn native_six_bit_instrument_field_reaches_instruments_above_mod_s_limit() {
        let mut processor = processor_with_instruments(true, 1024, 63);
        let _ = row(&mut processor, MtmCell { pitch: 12, instrument: 63, effect: 0, param: 0 });
        assert_eq!(processor.channel(0).map(|channel| channel.sample_number), Some(63));
    }

    // ── C3b: MultiTracker corrections ─────────────────────────────────────────────

    #[test]
    fn f00_is_a_no_op_rather_than_protracker_s_stop() {
        let mut processor = processor(true, 1024);
        let outcome = row(&mut processor, MtmCell { effect: 0xF, param: 0, ..MtmCell::EMPTY });
        assert!(!outcome.stop, "libxmp's fx_s3m_speed ignores F00 and MultiTracker has no stop command");
        assert_eq!((outcome.speed, outcome.tempo_bpm), (6, 125));
    }

    #[test]
    fn e8x_uses_the_loader_s_nibble_pan_curve() {
        let mut processor = processor(true, 1024);
        let _ = row(&mut processor, MtmCell { effect: 0xE, param: 0x8F, ..MtmCell::EMPTY });
        assert_eq!(processor.channel(0).map(|channel| channel.pan), Some(crate::loader::mtm_pan(15)), "E8F equals header pan 15");
        let _ = row(&mut processor, MtmCell { effect: 0xE, param: 0x88, ..MtmCell::EMPTY });
        assert_eq!(processor.channel(0).map(|channel| channel.pan), Some(crate::loader::mtm_pan(8)), "E88 equals header pan 8");
        let _ = row(&mut processor, MtmCell { effect: 0xE, param: 0x80, ..MtmCell::EMPTY });
        assert_eq!(processor.channel(0).map(|channel| channel.pan), Some(crate::loader::mtm_pan(0)));
    }

    #[test]
    fn a_seek_resets_the_shared_effect_core() {
        let mut processor = processor(true, 1024);
        let _ = row(&mut processor, MtmCell { pitch: 12, instrument: 1, effect: 4, param: 0x8F });
        assert_eq!(processor.channel(0).map(|channel| channel.vibrato_memory), Some(0x8F));
        TrackerProcessor::reset(&mut processor);
        assert_eq!(processor.channel(0).map(|channel| channel.vibrato_memory), Some(0));
        assert_eq!(processor.channel(0).map(|channel| channel.current_note), Some(u8::MAX));
    }
}
