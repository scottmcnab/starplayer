#![cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
use starplayer_core::{ChannelId, Frame, RowClock, U0F16};
use starplayer_engine::{ChannelTable, RowRef, SongPosition, TickContext, TrackerProcessor};
use starplayer_mixer::VoicePool;
use starplayer_model::{InstrumentDef, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec};
use starplayer_mtm::{MtmCell, MtmProcessor};
use starplayer_rt::Arc;

fn processor() -> MtmProcessor {
    let mut builder = ModuleBuilder::new();
    builder.add_pattern(&[0; 64 * 3], 64, 1).unwrap(); builder.set_orders(&[0, starplayer_model::ORDER_END]);
    let sample = builder.add_sample(&vec![1000; 8192], SampleSpec::one_shot("one-shot")).unwrap();
    builder.add_instrument(InstrumentDef::from_sample("instrument", sample, U0F16::MAX)).unwrap();
    builder.set_header(ModuleHeader::new(ModuleFormat::Mtm, 1));
    MtmProcessor::new(Arc::new(builder.build().unwrap()), 8000)
}
fn row(processor: &mut MtmProcessor, channels: &mut ChannelTable, voices: &mut VoicePool, cell: MtmCell, frame: u64) {
    let bytes = cell.to_bytes();
    let mut context = TickContext::new(Frame(frame), voices, channels, RowClock::new(6), SongPosition::default(), 125);
    let _ = processor.row(&mut context, RowRef { order: 0, pattern: 0, row: 0, bytes: &bytes });
}

#[cfg(feature = "note-overlap")]
#[test]
fn isolated_mtm_feature_forwards_actual_overlap_and_post_effect_onset_capture() {
    use starplayer_core::{InstrumentId, Note, OriginalNote, PlaybackCapabilities};
    use starplayer_engine::{PlaybackConfiguration, VoiceCapacity, VoiceDomain};
    let configuration = PlaybackConfiguration { identity: Default::default(), capabilities: PlaybackCapabilities::NOTE_OVERLAP, instrument_count: 1,
        capacity: VoiceCapacity { native: 1, custom: 2, live: 0, interactive_transport: false } };
    let mut channels = ChannelTable::new(1); channels.prepare_playback(configuration).unwrap();
    let playback = channels.playback_mut().unwrap(); playback.set_note_overlap_enabled(Default::default(), true).unwrap(); playback.set_let_notes_ring(Default::default(), InstrumentId(0), true).unwrap();
    let mut voices = VoicePool::new(configuration.capacity.physical()); let mut processor = processor();
    row(&mut processor, &mut channels, &mut voices, MtmCell { pitch: 12, instrument: 1, effect: 0xC, param: 20 }, 13);
    let old = channels.foreground(ChannelId(0)).unwrap();
    row(&mut processor, &mut channels, &mut voices, MtmCell { pitch: 19, instrument: 0, effect: 0xE, param: 0xA1 }, 173);
    let playback = channels.playback().unwrap(); let new = channels.foreground(ChannelId(0)).unwrap();
    assert_eq!(playback.active_count(VoiceDomain::Custom, &voices), 1);
    assert_eq!(playback.get(old, &voices).unwrap().identity().unwrap().note, OriginalNote::Note(Note::new(36)));
    assert_eq!(playback.get(new, &voices).unwrap().identity().unwrap().onset_volume, 21);
}

#[cfg(feature = "interactive-audition")]
#[test]
fn isolated_mtm_audition_feature_forwards_clear_suppression_to_the_effect_core() {
    let mut processor = processor(); let mut channels = ChannelTable::new(1); let mut voices = VoicePool::new(2);
    row(&mut processor, &mut channels, &mut voices, MtmCell { pitch: 12, instrument: 1, effect: 0xE, param: 0x91 }, 0);
    assert!(channels.foreground(ChannelId(0)).is_some());
    voices.release_all(); channels.forget_all();
    let mut clock = RowClock::new(6); clock.tick_in_row = 1;
    let mut context = TickContext::new(Frame(160), &mut voices, &mut channels, clock, SongPosition::default(), 125);
    processor.clear_sound(starplayer_engine::ClearScope::Song, &mut context);
    let _ = processor.tick(&mut context);
    assert_eq!(voices.voices_active(), 0, "E91 after clear may not resurrect a killed MTM voice");
}
