#![cfg(all(feature = "note-overlap", feature = "xm", feature = "it"))]
use starplayer::core::{ChannelId, Frame, InstrumentId, OriginalNote, PlaybackCapabilities, RowClock, TriggerIdentity, U0F16, VoiceId};
use starplayer::core::quirks::QuirkSelection;
use starplayer::engine::{ChannelTable, Engine, EngineSettings, PlaybackConfiguration, SongPosition, TickContext, TrackerProcessor, VoiceCapacity, VoiceDomain, VoiceLifecycle};
use starplayer::mixer::{FixedPath, StereoI16, VoicePool};
use starplayer::model::{DuplicateAction, DuplicateCheck, InstrumentDef, Module, ModuleBuilder, ModuleFormat, ModuleHeader, NewNoteAction, SampleSpec};
use starplayer::rt::Arc;
const FORMATS: [ModuleFormat; 2] = [ModuleFormat::Xm, ModuleFormat::It];
#[derive(Copy, Clone, Default)]
struct Cell { note: Option<u8>, instrument: u8, volume: Option<u8>, effect: u8, parameter: u8 }
fn note(note: u8) -> Cell { Cell { note: Some(note), instrument: 1, ..Cell::default() } }
fn encode(format: ModuleFormat, cells: [Cell; 2]) -> Vec<u8> {
    cells.into_iter().flat_map(|cell| match format {
        ModuleFormat::Xm => starplayer::xm::XmCell { note: cell.note.map_or(0, |note| note + 1), instrument: cell.instrument, volume: cell.volume.unwrap_or(0), effect: cell.effect, parameter: cell.parameter }.to_bytes().to_vec(),
        ModuleFormat::It => starplayer::it::ItCell { note: cell.note.unwrap_or(starplayer::it::NOTE_NONE), instrument: cell.instrument, volume: cell.volume.unwrap_or(starplayer::it::VOLUME_NONE), command: cell.effect, info: cell.parameter }.to_bytes().to_vec(),
        _ => unreachable!(),
    }).collect()
}
fn module(format: ModuleFormat, rows: &[[Cell; 2]], sample_mode: bool, nna: NewNoteAction, dct: DuplicateCheck, dca: DuplicateAction) -> Arc<Module> {
    let mut builder = ModuleBuilder::new();
    for index in 0..3 {
        let specification = if index == 2 { SampleSpec::one_shot("loop").with_forward_loop(0, 32768) } else { SampleSpec::one_shot("one shot") };
        let pcm: Vec<i16> = (0..32768).map(|frame| if (frame / 19 + index) % 2 == 0 { 1500 } else { -1500 }).collect();
        let sample = builder.add_sample(&pcm, specification).unwrap();
        let mut instrument = InstrumentDef::from_sample("instrument", sample, U0F16::MAX);
        instrument.new_note_action = nna; instrument.duplicate_check = dct; instrument.duplicate_action = dca;
        instrument.note_sample_map.fill(index as u16 + 1);
        for (note, mapped) in instrument.note_transpose_map.iter_mut().enumerate() { *mapped = note as u8; }
        // Instrument 2 is mixed: C is one shot, G is a loop. Original note identity
        // must survive this actual-sample distinction and instrument 1's transpose.
        if index == 1 { instrument.note_sample_map[55] = 3; }
        if index == 0 { instrument.note_transpose_map[48] = 60; }
        builder.add_instrument(instrument).unwrap();
    }
    let mut bytes = Vec::new();
    for row in 0..64 { bytes.extend(encode(format, rows.get(row).copied().unwrap_or_default())); }
    builder.add_pattern(&bytes, 64, 2).unwrap(); builder.set_orders(&[0, starplayer::model::ORDER_END]);
    let mut header = ModuleHeader::new(format, 2); header.initial_speed = 6;
    header.flags.linear_slides = true;
    if format == ModuleFormat::It { header.format_extra = starplayer::it::ItFormatExtra { flags: if sample_mode { 8 } else { 12 }, special: 0, old_instruments: false, has_midi_configuration: false }.encode(); }
    builder.set_header(header); Arc::new(builder.build().unwrap())
}
fn configuration(format: ModuleFormat, custom: usize) -> PlaybackConfiguration {
    PlaybackConfiguration { identity: Default::default(), capabilities: PlaybackCapabilities::NOTE_OVERLAP, instrument_count: 3,
        capacity: VoiceCapacity { native: if format == ModuleFormat::It { 256 } else { 2 }, custom, live: 0, interactive_transport: false } }
}
struct Harness { module: Arc<Module>, processor: Box<dyn TrackerProcessor>, voices: VoicePool, channels: ChannelTable }
impl Harness {
    fn with_module(module: Arc<Module>, custom: usize, checked: bool, enabled: bool) -> Self {
        let configuration = configuration(module.header().format, custom);
        Self::configured(module, configuration, checked, enabled)
    }
    fn configured(module: Arc<Module>, configuration: PlaybackConfiguration, checked: bool, enabled: bool) -> Self {
        let capacity = configuration.capacity.physical();
        let processor: Box<dyn TrackerProcessor> = match module.header().format {
            ModuleFormat::Xm => Box::new(starplayer::xm::XmProcessor::try_with_articulation(Arc::clone(&module), 8000, QuirkSelection::FromDialect, capacity).unwrap()),
            ModuleFormat::It => Box::new(starplayer::it::ItProcessor::try_with_quirks_and_voice_capacity(Arc::clone(&module), 8000, QuirkSelection::FromDialect, capacity).unwrap()),
            _ => unreachable!(),
        };
        let mut channels = if configuration.capacity.live != 0 { ChannelTable::try_with_layout(starplayer::engine::LaneLayout::interactive(2).unwrap()).unwrap() } else { ChannelTable::new(2) }; channels.prepare_playback(configuration).unwrap();
        let playback = channels.playback_mut().unwrap();
        for instrument in 0..3 { playback.set_let_notes_ring(Default::default(), InstrumentId(instrument), checked).unwrap(); }
        playback.set_note_overlap_enabled(Default::default(), enabled).unwrap();
        Self { module, processor, voices: VoicePool::new(capacity), channels }
    }
    fn new(format: ModuleFormat, custom: usize, checked: bool, enabled: bool) -> Self { Self::with_module(module(format, &[], false, NewNoteAction::Cut, DuplicateCheck::Off, DuplicateAction::Cut), custom, checked, enabled) }
    fn row(&mut self, cells: [Cell; 2]) {
        let bytes = encode(self.module.header().format, cells);
        let mut context = TickContext::new(Frame(17), &mut self.voices, &mut self.channels, RowClock::new(6), SongPosition::default(), 125);
        let _ = self.processor.row(&mut context, starplayer::engine::RowRef { order: 0, pattern: 0, row: 0, bytes: &bytes });
    }
    fn tick(&mut self, tick: u16) {
        let mut clock = RowClock::new(6); clock.tick_in_row = tick;
        let mut context = TickContext::new(Frame(17 + tick as u64 * 160), &mut self.voices, &mut self.channels, clock, SongPosition::default(), 125);
        let _ = self.processor.tick(&mut context);
    }
    fn foreground(&self) -> VoiceId { self.channels.foreground(ChannelId(0)).unwrap() }
    fn identity(&self, voice: VoiceId) -> TriggerIdentity { self.channels.playback().unwrap().get(voice, &self.voices).unwrap().identity().unwrap() }
    fn active(&self, domain: VoiceDomain) -> usize { self.channels.playback().unwrap().active_count(domain, &self.voices) }
    fn retiring(&self, voice: VoiceId) -> bool { self.channels.playback().unwrap().get(voice, &self.voices).is_some_and(|owned| owned.lifecycle == VoiceLifecycle::Retiring) }
}
#[test]
fn musical_identity_and_exact_volume_replace_all_matches_on_the_origin_only() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 8, true, true);
        harness.row([note(48), Cell::default()]); let first = harness.foreground();
        harness.row([note(55), note(48)]); let second = harness.foreground(); let other = harness.channels.foreground(ChannelId(1)).unwrap();
        assert_eq!(harness.active(VoiceDomain::Custom), 1);
        assert_eq!(harness.identity(first).note, OriginalNote::Note(starplayer::core::Note::new(48)));
        harness.row([note(48), Cell::default()]); assert!(harness.retiring(first)); assert!(!harness.retiring(second)); assert!(!harness.retiring(other));
        let loud = harness.foreground();
        let quiet = Cell { volume: Some(if format == ModuleFormat::Xm { 16 + 20 } else { 20 }), ..note(48) };
        harness.row([quiet, Cell::default()]); let echo = harness.foreground();
        assert_eq!(harness.identity(echo).onset_volume, if format == ModuleFormat::Xm { 20 } else { 80 });
        assert!(!harness.retiring(loud)); harness.row([quiet, Cell::default()]); assert!(harness.retiring(echo)); assert!(!harness.retiring(loud));
    }
}
#[test]
fn unchecked_and_bypassed_onsets_are_passive_and_enable_then_repeat_matches() {
    for format in FORMATS { for (checked, enabled) in [(false, true), (true, false)] {
        let mut harness = Harness::new(format, 4, checked, enabled);
        harness.row([note(48), Cell::default()]); let old = harness.foreground(); let identity = harness.identity(old);
        let playback = harness.channels.playback_mut().unwrap();
        playback.set_let_notes_ring(Default::default(), InstrumentId(0), true).unwrap(); playback.set_note_overlap_enabled(Default::default(), true).unwrap();
        harness.row([note(48), Cell::default()]); assert!(harness.retiring(old)); assert_eq!(harness.identity(harness.foreground()), identity);
    } }
}
#[test]
fn post_effect_base_includes_fine_slides_and_delayed_volume_on_the_actual_tick() {
    for format in FORMATS {
        let cases = if format == ModuleFormat::Xm { vec![(Some(36), 0xE, 0xA3, 23), (Some(36), 0xE, 0xB3, 17), (Some(0x83), 0, 0, 61), (Some(0x93), 0, 0, 64)] }
            else { vec![(Some(20), 4, 0x3F, 92), (Some(20), 4, 0xF3, 68), (Some(68), 0, 0, 256), (Some(78), 0, 0, 244)] };
        for (volume, effect, parameter, expected) in cases {
            let mut harness = Harness::new(format, 4, true, true);
            let cell = Cell { volume, effect, parameter, ..note(48) };
            harness.row([cell, Cell::default()]); let old = harness.foreground(); assert_eq!(harness.identity(old).onset_volume, expected, "{format:?} {volume:?} {effect:x}{parameter:x}");
            harness.row([cell, Cell::default()]); assert!(harness.retiring(old));
        }
        let mut harness = Harness::new(format, 4, true, true);
        harness.row([note(48), Cell::default()]); let old = harness.foreground();
        harness.row([Cell { volume: Some(if format == ModuleFormat::Xm { 36 } else { 20 }), effect: if format == ModuleFormat::Xm { 0xE } else { 19 }, parameter: 0xD2, ..note(55) }, Cell::default()]);
        assert_eq!(harness.foreground(), old); harness.tick(1); assert_eq!(harness.foreground(), old);
        harness.tick(2); assert_ne!(harness.foreground(), old); assert_eq!(harness.identity(harness.foreground()).onset_volume, if format == ModuleFormat::Xm { 20 } else { 80 });
    }
}

#[test]
fn it_candidate_policy_is_current_but_native_and_custom_provenance_never_migrate() {
    let module = module(ModuleFormat::It, &[], false, NewNoteAction::Continue, DuplicateCheck::Instrument, DuplicateAction::Cut);
    let mut harness = Harness::with_module(Arc::clone(&module), 8, false, true);
    harness.row([note(48), Cell::default()]); let native = harness.foreground();
    // Different instrument avoids authored DCT while preserving native NNA.
    harness.row([Cell { instrument: 2, ..note(48) }, Cell::default()]);
    assert_eq!(harness.active(VoiceDomain::Native), 2);
    harness.channels.playback_mut().unwrap().set_let_notes_ring(Default::default(), InstrumentId(0), true).unwrap();
    harness.row([note(55), Cell::default()]);
    assert!(!harness.retiring(native), "custom note rule replaces native DCT=Instrument per candidate");
    assert_eq!(harness.channels.playback().unwrap().get(native, &harness.voices).unwrap().domain, VoiceDomain::Native);
    harness.channels.clear_scope(starplayer::engine::ClearScope::CustomDetached, &mut harness.voices);
    assert!(!harness.retiring(native), "native-created backgrounds survive bypass cleanup");
    harness.row([note(48), Cell::default()]); assert!(harness.retiring(native), "late scan matches native provenance too");

    let mut harness = Harness::with_module(module, 8, true, true);
    harness.row([note(48), Cell::default()]); let custom = harness.foreground();
    harness.row([note(55), Cell::default()]); assert_eq!(harness.active(VoiceDomain::Custom), 1);
    harness.channels.playback_mut().unwrap().set_let_notes_ring(Default::default(), InstrumentId(0), false).unwrap();
    harness.row([note(60), Cell::default()]);
    assert!(harness.voices.get(custom).is_none(), "unchecked custom candidate follows native DCT=Instrument cut");
}

#[test]
fn it_mixed_map_incoming_loop_does_not_force_native_dct_on_eligible_tails() {
    let mut harness = Harness::with_module(module(ModuleFormat::It, &[], false, NewNoteAction::Continue, DuplicateCheck::Instrument, DuplicateAction::Cut), 8, true, true);
    harness.row([Cell { instrument: 2, ..note(48) }, Cell::default()]); let one_shot = harness.foreground();
    harness.row([Cell { instrument: 2, ..note(55) }, Cell::default()]); let looping = harness.foreground();
    assert!(!harness.retiring(one_shot)); assert_eq!(harness.active(VoiceDomain::Custom), 1);
    harness.row([note(60), Cell::default()]);
    assert!(harness.voices.get(looping).is_some());
    assert_eq!(harness.channels.playback().unwrap().get(looping, &harness.voices).unwrap().domain, VoiceDomain::Native);
    assert_eq!(harness.active(VoiceDomain::Custom), 1, "loop keeps authored NNA rather than custom provenance");
}

#[test]
fn it_sample_mode_has_custom_overlap_and_passive_identity() {
    let mut harness = Harness::with_module(module(ModuleFormat::It, &[], true, NewNoteAction::Cut, DuplicateCheck::Off, DuplicateAction::Cut), 4, true, true);
    harness.row([note(48), Cell::default()]); let first = harness.foreground();
    harness.row([note(55), Cell::default()]); assert_eq!(harness.active(VoiceDomain::Custom), 1);
    harness.row([note(48), Cell::default()]); assert!(harness.retiring(first));
}

#[test]
fn custom_capacity_retirement_reuses_generations_without_losing_incoming_identity() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 2, true, true);
        harness.row([note(48), note(48)]); let protected = harness.channels.foreground(ChannelId(1)).unwrap();
        let mut old = Vec::new();
        for index in 0..24 { old.push(harness.foreground()); harness.row([note(49 + index), Cell::default()]); assert!(harness.voices.get(protected).is_some()); assert_eq!(harness.identity(harness.foreground()).note, OriginalNote::Note(starplayer::core::Note::new(49 + index))); assert!(harness.active(VoiceDomain::Custom) <= 2); }
        assert!(harness.channels.playback().unwrap().overlap_diagnostics().custom_evictions > 0);
        assert!(old.iter().any(|voice| harness.voices.get(*voice).is_none()));
        let incoming = harness.foreground(); harness.row([note(72), Cell::default()]); assert!(harness.retiring(incoming));
        assert!(!harness.retiring(harness.foreground()), "same slot index in a new generation cannot match itself");
    }
}

#[test]
fn native_release_keeps_identity_and_instrument_only_and_retrigger_do_not_detach() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 8, true, true);
        harness.row([note(48), Cell::default()]); let old = harness.foreground(); let identity = harness.identity(old);
        harness.row([Cell { instrument: 1, ..Cell::default() }, Cell::default()]);
        assert_eq!(harness.active(VoiceDomain::Custom), 0); assert_eq!(harness.identity(harness.foreground()), identity);
        harness.row([Cell { effect: if format == ModuleFormat::Xm { 0xE } else { 17 }, parameter: if format == ModuleFormat::Xm { 0x91 } else { 1 }, ..Cell::default() }, Cell::default()]); harness.tick(1);
        assert_eq!(harness.active(VoiceDomain::Custom), 0); assert_eq!(harness.identity(harness.foreground()), identity);
    }
}

#[test]
fn xm_backgrounds_step_after_their_origin_and_before_the_next_global_volume_command() {
    let mut harness = Harness::new(ModuleFormat::Xm, 4, true, true);
    harness.row([note(48), note(48)]); let left = harness.foreground(); let right = harness.channels.foreground(ChannelId(1)).unwrap();
    harness.row([Cell { effect: 0x10, parameter: 32, ..note(55) }, Cell { effect: 0x10, parameter: 16, ..note(55) }]);
    let left_volume = harness.voices.get(left).unwrap().params.volume.to_bits();
    let right_volume = harness.voices.get(right).unwrap().params.volume.to_bits();
    assert_eq!(left_volume, right_volume * 2, "each origin observes its own point in ordered Gxx processing");
    harness.tick(1);
    assert_eq!(harness.voices.get(left).unwrap().params.volume, harness.voices.get(right).unwrap().params.volume);
}

#[test]
fn reset_retains_only_custom_articulation_until_scoped_retirement() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 4, true, true);
        harness.row([note(48), Cell::default()]); let tail = harness.foreground();
        harness.row([note(55), Cell::default()]); let identity = harness.identity(tail);
        harness.processor.reset();
        harness.tick(1); assert_eq!(harness.identity(tail), identity); assert_eq!(harness.active(VoiceDomain::Custom), 1);
        harness.channels.clear_scope(starplayer::engine::ClearScope::CustomDetached, &mut harness.voices);
        assert!(harness.retiring(tail));
        let params = harness.voices.get(tail).unwrap().params;
        harness.tick(2); assert_eq!(harness.voices.get(tail).unwrap().params, params, "retirement cannot receive articulation writes");
    }
}

fn render(module: Arc<Module>, choices: Option<(bool, [bool; 3])>, block: usize) -> Vec<i16> {
    let format = module.header().format;
    let settings = EngineSettings { sample_rate_hz: 8000, channel_count: 2, voice_capacity: if format == ModuleFormat::It { 256 } else { 2 }, ..EngineSettings::default() };
    let settings = if choices.is_some() { settings.with_playback(configuration(format, 4)).unwrap() } else { settings };
    let mut engine: Engine<FixedPath, starplayer::dsp::Linear, StereoI16, Arc<Module>> = Engine::with_settings(settings);
    engine.take_control().unwrap().load_module(Arc::clone(&module)).ok().unwrap();
    let source = if choices.is_some() { starplayer::NativeSequencer::new_with_articulation(module, 8000, QuirkSelection::FromDialect, settings.voice_capacity).unwrap() }
        else { starplayer::NativeSequencer::new_with_voice_capacity(module, 8000, QuirkSelection::FromDialect, settings.voice_capacity).unwrap() };
    engine.set_source(Box::new(source));
    if let Some((enabled, selections)) = choices { engine.restore_selections(Default::default(), enabled, false, &selections); }
    let mut output = vec![0; 12000]; for chunk in output.chunks_mut(block * 2) { engine.render(chunk); } output
}
#[test]
fn six_block_sizes_and_three_fresh_canonical_configurations_keep_exact_pcm() {
    for format in FORMATS {
        let rows = [[note(48), Cell::default()], [note(55), note(48)], [note(48), Cell::default()], [Cell { instrument: 3, ..note(48) }, Cell::default()], [note(55), Cell::default()]];
        let module = module(format, &rows, false, NewNoteAction::Continue, DuplicateCheck::Off, DuplicateAction::Cut);
        let native = render(Arc::clone(&module), None, 128); let enabled = render(Arc::clone(&module), Some((true, [true; 3])), 128);
        assert!(native != enabled, "{format:?} fixture must exercise audible policy");
        for size in [1, 3, 64, 128, 4096, 8191] {
            assert!(render(Arc::clone(&module), Some((true, [true; 3])), size) == enabled, "enabled {format:?} block {size}");
            for choices in [(true, [false; 3]), (false, [true; 3]), (true, [false, false, true])] {
                assert!(render(Arc::clone(&module), Some(choices), size) == native, "canonical {format:?} {choices:?} block {size}");
            }
        }
    }
}

#[test]
fn non_clamped_volume_column_fine_up_is_included_in_late_matching() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 4, true, true);
        let initial = Cell { volume: Some(if format == ModuleFormat::Xm { 36 } else { 20 }), ..note(48) };
        harness.row([initial, Cell::default()]);
        let raised = Cell { instrument: 0, volume: Some(if format == ModuleFormat::Xm { 0x93 } else { 68 }), ..note(48) };
        harness.row([raised, Cell::default()]); let first_raised = harness.foreground();
        assert_eq!(harness.identity(first_raised).onset_volume, if format == ModuleFormat::Xm { 23 } else { 92 });
        harness.row([initial, Cell::default()]); harness.row([raised, Cell::default()]); assert!(harness.retiring(first_raised));
    }
}

#[test]
fn invalid_starts_portamento_and_native_offset_rules_do_not_invent_custom_onsets() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 8, true, true);
        harness.row([note(48), Cell::default()]); let old = harness.foreground(); let identity = harness.identity(old);
        harness.row([Cell { instrument: 0, effect: if format == ModuleFormat::Xm { 3 } else { 7 }, parameter: 3, ..note(55) }, Cell::default()]);
        assert_eq!(harness.foreground(), old); assert_eq!(harness.identity(old), identity); assert_eq!(harness.active(VoiceDomain::Custom), 0);
        harness.row([Cell { instrument: 99, ..note(55) }, Cell::default()]); assert_eq!(harness.active(VoiceDomain::Custom), 0);
        let mut harness = Harness::new(format, 8, true, true);
        harness.row([note(48), Cell::default()]); let old = harness.foreground();
        harness.row([Cell { effect: if format == ModuleFormat::Xm { 9 } else { 15 }, parameter: 255, ..note(55) }, Cell::default()]);
        if format == ModuleFormat::Xm { assert_eq!(harness.active(VoiceDomain::Custom), 0); assert!(harness.voices.get(old).is_none_or(|voice| voice.wants_stop())); }
        else { assert_eq!(harness.active(VoiceDomain::Custom), 1, "native IT ignores past-end Oxx, so this is a real onset"); assert_eq!(harness.identity(harness.foreground()).note, OriginalNote::Note(starplayer::core::Note::new(55))); }
    }
}

#[test]
#[cfg(feature = "interactive-audition")]
fn it_full_256_native_64_custom_16_live_pool_keeps_domains_and_resolves_onsets() {
    let module = module(ModuleFormat::It, &[], false, NewNoteAction::Continue, DuplicateCheck::Off, DuplicateAction::Cut);
    let mut configuration = configuration(ModuleFormat::It, 64);
    configuration.capabilities = PlaybackCapabilities::NOTE_OVERLAP | PlaybackCapabilities::INTERACTIVE_AUDITION;
    configuration.capacity.live = 16; configuration.capacity.interactive_transport = true;
    let mut harness = Harness::configured(module, configuration, false, true);
    for _ in 0..256 { harness.row([note(48), Cell::default()]); }
    assert_eq!(harness.active(VoiceDomain::Native), 256);
    let sample = harness.module.sample(starplayer::core::SampleId(0)).unwrap();
    let region = starplayer::mixer::SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames());
    let mut live = Vec::new();
    for lane in 64..80 {
        live.push(harness.channels.trigger(ChannelId(lane), &mut harness.voices, starplayer::mixer::VoiceTag::default(), region, starplayer::core::VoiceParams::SILENT, 0).unwrap());
    }
    harness.channels.playback_mut().unwrap().set_let_notes_ring(Default::default(), InstrumentId(1), true).unwrap();
    // First native instrument 1 foreground becomes an authored background; subsequent
    // instrument 2 notes migrate only their outgoing foreground to C.
    for index in 0..65 { harness.row([Cell { instrument: 2, volume: Some((index % 64) as u8), ..note(48 + (index / 64) as u8) }, Cell::default()]); }
    assert_eq!(harness.active(VoiceDomain::Native), 256); assert_eq!(harness.active(VoiceDomain::Custom), 64); assert_eq!(harness.active(VoiceDomain::Live), 16);
    for index in 0..20 {
        harness.row([Cell { instrument: 2, volume: Some(index), ..note(60) }, Cell::default()]);
        assert_eq!(harness.identity(harness.foreground()).onset_volume, index as u16 * 4);
        assert_eq!(harness.active(VoiceDomain::Native), 256); assert_eq!(harness.active(VoiceDomain::Custom), 64);
        assert!(live.iter().all(|voice| harness.voices.get(*voice).is_some()));
        assert!(harness.voices.voices_active() <= 339);
    }
    let fine = Cell { instrument: 2, volume: Some(20), effect: 4, parameter: 0x3F, ..note(60) };
    harness.row([fine, Cell::default()]); let earlier_fine = harness.foreground();
    assert_eq!(harness.identity(earlier_fine).onset_volume, 92);
    harness.row([Cell { instrument: 2, ..note(62) }, Cell::default()]);
    harness.row([fine, Cell::default()]);
    assert!(harness.retiring(earlier_fine)); assert_eq!(harness.identity(harness.foreground()).onset_volume, 92);
    assert!(!harness.retiring(harness.foreground()));
    // Disable custom policy: native stealing must not pick a quiet C or L voice.
    harness.channels.playback_mut().unwrap().set_let_notes_ring(Default::default(), InstrumentId(1), false).unwrap();
    let custom: Vec<_> = harness.channels.playback().unwrap().iter(&harness.voices).filter(|owned| owned.domain == VoiceDomain::Custom && owned.lifecycle != VoiceLifecycle::Retiring).map(|owned| owned.voice).collect();
    harness.row([note(50), Cell::default()]); assert!(custom.iter().all(|voice| harness.voices.get(*voice).is_some()));
    harness.processor.reset(); harness.row([note(52), Cell::default()]);
    assert_eq!(harness.identity(harness.foreground()).note, OriginalNote::Note(starplayer::core::Note::new(52)), "reset's own native foreground remains net-zero at quota");
    assert!(live.iter().all(|voice| harness.voices.get(*voice).is_some()));
}

#[test]
fn it_native_action_matrix_stays_native_except_checked_eligible_candidates() {
    for nna in [NewNoteAction::Cut, NewNoteAction::Continue, NewNoteAction::NoteOff, NewNoteAction::NoteFade] {
        for dct in [DuplicateCheck::Off, DuplicateCheck::Note, DuplicateCheck::Sample, DuplicateCheck::Instrument] {
            for dca in [DuplicateAction::Cut, DuplicateAction::NoteOff, DuplicateAction::NoteFade] {
                for checked in [false, true] { for enabled in [false, true] { for looping in [false, true] {
                    let module = module(ModuleFormat::It, &[], false, nna, dct, dca);
                    let mut harness = Harness::with_module(Arc::clone(&module), 4, checked, enabled);
                    let mut native = Harness::with_module(module, 4, false, false);
                    native.channels = ChannelTable::new(2); // Truly unconfigured native comparison.
                    let instrument = if looping { 3 } else { 1 };
                    let first = [Cell { instrument, ..note(48) }, Cell::default()]; let second = [Cell { instrument, ..note(55) }, Cell::default()];
                    harness.row(first); native.row(first); let old = harness.foreground();
                    assert_eq!(harness.identity(old).instrument, InstrumentId(instrument as u16 - 1), "passive identity includes ineligible starts");
                    harness.row(second); native.row(second);
                    if checked && enabled && !looping {
                        assert_eq!(harness.active(VoiceDomain::Custom), 1, "{nna:?}/{dct:?}/{dca:?}");
                        assert!(!harness.retiring(old)); assert!(!harness.voices.get(old).unwrap().wants_stop());
                    } else {
                        let actual: Vec<_> = harness.voices.iter().map(|(_, voice)| (voice.tag, voice.params)).collect();
                        let expected: Vec<_> = native.voices.iter().map(|(_, voice)| (voice.tag, voice.params)).collect();
                        assert_eq!(actual, expected, "native {nna:?}/{dct:?}/{dca:?}, checked{checked}, enabled{enabled}, loop{looping}");
                        assert_eq!(harness.active(VoiceDomain::Custom), 0);
                    }
                } } }
            }
        }
    }
}

#[test]
fn note_only_and_delayed_note_only_keep_instrument_and_nondefault_base() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 8, true, true);
        harness.row([Cell { volume: Some(if format == ModuleFormat::Xm { 36 } else { 20 }), ..note(48) }, Cell::default()]); let first = harness.foreground();
        harness.row([Cell { instrument: 0, ..note(55) }, Cell::default()]);
        assert_eq!(harness.identity(harness.foreground()).onset_volume, if format == ModuleFormat::Xm { 20 } else { 80 });
        harness.row([Cell { instrument: 0, effect: if format == ModuleFormat::Xm { 14 } else { 19 }, parameter: 0xD2, ..note(48) }, Cell::default()]); harness.tick(2);
        assert!(harness.retiring(first)); assert_eq!(harness.identity(harness.foreground()).instrument, InstrumentId(0));
    }
}

#[test]
fn preexisting_native_it_note_only_after_nna_loses_foreground_mapping() {
    let mut harness = Harness::with_module(module(ModuleFormat::It, &[], false, NewNoteAction::Continue, DuplicateCheck::Off, DuplicateAction::Cut), 4, false, false);
    harness.row([note(48), Cell::default()]); let old = harness.foreground();
    harness.row([Cell { instrument: 0, ..note(55) }, Cell::default()]);
    assert!(harness.channels.foreground(ChannelId(0)).is_none(), "existing authored-NNA mapping gap; optional custom path must not copy it");
    assert!(harness.voices.get(old).is_some()); assert_eq!(harness.active(VoiceDomain::Native), 1);
}

fn articulated_module(format: ModuleFormat, sustain: bool, sustain_sample: bool) -> Arc<Module> {
    use starplayer::model::{AutoVibrato, AutoVibratoWaveform, Envelope, EnvelopePoint, EnvelopeSpan, LoopMode, SustainLoop};
    let mut builder = ModuleBuilder::new();
    let sample = builder.add_sample(&vec![1500; 32768], SampleSpec {
        auto_vibrato: AutoVibrato { waveform: AutoVibratoWaveform::Sine, sweep: 8, depth: 4, rate: 4 },
        sustain_loop: sustain_sample.then_some(SustainLoop { mode: LoopMode::Forward, start: 0, end: 32768 }),
        ..SampleSpec::one_shot("articulated")
    }).unwrap();
    let mut instrument = InstrumentDef::from_sample("articulated", sample, U0F16::MAX);
    instrument.note_sample_map.fill(1);
    for (note, mapped) in instrument.note_transpose_map.iter_mut().enumerate() { *mapped = note as u8; }
    instrument.volume_envelope = Some(Envelope { points: vec![EnvelopePoint { tick: 0, value: 64 }, EnvelopePoint { tick: 1, value: 48 }, EnvelopePoint { tick: 2, value: 32 }, EnvelopePoint { tick: 12, value: 16 }].into_boxed_slice(), sustain: sustain.then_some(EnvelopeSpan { start: 1, end: 1 }), carry: format == ModuleFormat::It, ..Envelope::default() });
    instrument.panning_envelope = Some(Envelope { points: vec![EnvelopePoint { tick: 0, value: if format == ModuleFormat::Xm { 32 } else { 0 } }, EnvelopePoint { tick: 10, value: 24 }].into_boxed_slice(), ..Envelope::default() });
    instrument.fadeout = 128; instrument.new_note_action = NewNoteAction::Continue;
    instrument.initial_filter_cutoff = Some(60); instrument.initial_filter_resonance = Some(20);
    builder.add_instrument(instrument).unwrap();
    let bytes = encode(format, [Cell::default(); 2]); builder.add_pattern(&bytes, 1, 2).unwrap(); builder.set_orders(&[0]);
    let mut header = ModuleHeader::new(format, 2); header.flags.linear_slides = true;
    if format == ModuleFormat::It { header.format_extra = starplayer::it::ItFormatExtra { flags: 12, special: 0, old_instruments: false, has_midi_configuration: false }.encode(); }
    builder.set_header(header); Arc::new(builder.build().unwrap())
}

#[test]
fn detached_articulation_steps_once_and_keeps_envelopes_pan_auto_vibrato_and_filter() {
    for format in FORMATS { for sustain in [false, true] {
        let module = articulated_module(format, sustain, false);
        let mut detached = Harness::with_module(Arc::clone(&module), 4, true, true);
        let mut reference = Harness::with_module(module, 4, false, false);
        detached.row([note(48), Cell::default()]); reference.row([note(48), Cell::default()]); let tail = detached.foreground(); let original = reference.foreground();
        detached.row([note(55), Cell::default()]); reference.tick(1);
        if format == ModuleFormat::It { assert_eq!(detached.voices.get(tail).unwrap().params.volume, detached.voices.get(detached.foreground()).unwrap().params.volume, "native carry reads the custom-detached predecessor"); }
        for tick in 2..6 {
            let actual = detached.voices.get(tail).unwrap().params; let expected = reference.voices.get(original).unwrap().params;
            assert_eq!((actual.step, actual.volume, actual.pan, actual.filter), (expected.step, expected.volume, expected.pan, expected.filter), "{format:?} sustain{sustain} tick{tick}");
            detached.tick(tick); reference.tick(tick);
        }
    } }
}

#[test]
fn key_off_identity_survives_release_and_sustain_samples_remain_native() {
    for format in FORMATS {
        let mut harness = Harness::with_module(articulated_module(format, true, false), 4, true, true);
        harness.row([note(48), Cell::default()]); let old = harness.foreground(); let identity = harness.identity(old);
        let release = if format == ModuleFormat::Xm { Cell { effect: 0x14, parameter: 0, ..Cell::default() } } else { Cell { note: Some(starplayer::it::NOTE_OFF), ..Cell::default() } };
        harness.row([release, Cell::default()]); harness.tick(1); assert_eq!(harness.identity(old), identity);
        harness.row([note(48), Cell::default()]); assert!(harness.retiring(old));
        let mut harness = Harness::with_module(articulated_module(format, true, true), 4, true, true);
        harness.row([note(48), Cell::default()]); let old = harness.foreground();
        harness.row([note(55), Cell::default()]); assert_eq!(harness.active(VoiceDomain::Custom), 0);
        if format == ModuleFormat::It { assert!(harness.voices.get(old).is_some()); assert_eq!(harness.active(VoiceDomain::Native), 2); }
    }
}

#[test]
fn context_free_public_seek_keeps_custom_generations_through_the_next_dispatch() {
    use starplayer::engine::{ControlClock, EngineContext, EventSource};
    for format in FORMATS {
        let module = module(format, &[[note(48), Cell::default()], [note(55), Cell::default()]], false, NewNoteAction::Cut, DuplicateCheck::Off, DuplicateAction::Cut);
        let mut harness = Harness::with_module(Arc::clone(&module), 4, true, true);
        let mut sequencer = starplayer::NativeSequencer::new_with_articulation(module, 8000, QuirkSelection::FromDialect, harness.voices.capacity()).unwrap();
        let mut control = ControlClock::new(8000, Frame::ZERO);
        for _ in 0..7 {
            let frame = sequencer.next_event_frame().unwrap();
            sequencer.dispatch(frame, &mut EngineContext::new(frame, &mut harness.voices, &mut harness.channels, &mut control));
        }
        let tail = harness.channels.playback().unwrap().iter(&harness.voices).find(|owned| owned.domain == VoiceDomain::Custom).unwrap().voice;
        let identity = harness.identity(tail);
        sequencer.seek_row(2);
        let frame = sequencer.next_event_frame().unwrap(); sequencer.dispatch(frame, &mut EngineContext::new(frame, &mut harness.voices, &mut harness.channels, &mut control));
        assert_eq!(harness.identity(tail), identity); assert_eq!(harness.active(VoiceDomain::Custom), 1);
        assert!(sequencer.seek_order_at(0, Frame(2000)));
        let frame = sequencer.next_event_frame().unwrap(); sequencer.dispatch(frame, &mut EngineContext::new(frame, &mut harness.voices, &mut harness.channels, &mut control));
        assert!(harness.retiring(tail), "repeat after public seek still finds the old custom identity");
    }
}

#[test]
fn it_pending_instrument_mapping_is_resolved_before_custom_preservation() {
    let mut builder = ModuleBuilder::new();
    let sample = builder.add_sample(&vec![1000; 32768], SampleSpec::one_shot("mapped")).unwrap();
    for index in 0..2 {
        let mut instrument = InstrumentDef::from_sample("mapped", sample, U0F16::MAX);
        instrument.note_sample_map.fill(0); instrument.note_sample_map[48] = 1;
        if index == 1 { instrument.note_sample_map[55] = 1; }
        for (note, mapped) in instrument.note_transpose_map.iter_mut().enumerate() { *mapped = note as u8; }
        builder.add_instrument(instrument).unwrap();
    }
    builder.add_pattern(&encode(ModuleFormat::It, [Cell::default(); 2]), 1, 2).unwrap(); builder.set_orders(&[0]);
    let mut header = ModuleHeader::new(ModuleFormat::It, 2); header.format_extra = starplayer::it::ItFormatExtra { flags: 12, special: 0, old_instruments: false, has_midi_configuration: false }.encode(); builder.set_header(header);
    let mut harness = Harness::with_module(Arc::new(builder.build().unwrap()), 4, true, true);
    harness.row([note(48), Cell::default()]);
    // An unmapped explicit note remembers pending instrument 2 but discards its cell,
    // leaving the actual PCM/instrument 1 voice bound. Instrument 1 cannot map G.
    harness.row([Cell { instrument: 2, ..note(60) }, Cell::default()]); let old = harness.foreground();
    assert_eq!(harness.voices.get(old).unwrap().tag.instrument, 1);
    harness.row([Cell { instrument: 0, ..note(55) }, Cell::default()]);
    assert_eq!(harness.active(VoiceDomain::Custom), 1); assert!(!harness.retiring(old));
    assert_eq!(harness.identity(harness.foreground()).instrument, InstrumentId(1));
}

#[test]
fn deferred_reset_advances_custom_envelopes_but_never_retained_native_snapshots() {
    for format in FORMATS {
        let mut harness = Harness::with_module(articulated_module(format, false, false), 4, true, true);
        harness.row([note(48), note(48)]); let tail = harness.foreground(); let native = harness.channels.foreground(ChannelId(1)).unwrap();
        harness.row([note(55), Cell::default()]);
        let before_tail = harness.voices.get(tail).unwrap().params;
        let before_native = harness.voices.get(native).unwrap().params;
        harness.processor.reset();
        let mut context = TickContext::new(Frame(337), &mut harness.voices, &mut harness.channels, RowClock::new(6), SongPosition::default(), 125);
        harness.processor.control_step(&mut context);
        assert_ne!(harness.voices.get(tail).unwrap().params.volume, before_tail.volume);
        assert_eq!(harness.voices.get(native).unwrap().params, before_native, "native legacy-reset snapshot no longer articulates");
        harness.channels.clear_scope(starplayer::engine::ClearScope::Song, &mut harness.voices);
        assert_eq!(harness.active(VoiceDomain::Custom), 0); assert_eq!(harness.active(VoiceDomain::Native), 0);
        let retiring = harness.voices.get(tail).unwrap().params;
        let mut context = TickContext::new(Frame(497), &mut harness.voices, &mut harness.channels, RowClock::new(6), SongPosition::default(), 125);
        harness.processor.control_step(&mut context); assert_eq!(harness.voices.get(tail).unwrap().params, retiring);
    }
}

#[test]
fn it_runtime_nna_overrides_and_background_commands_keep_native_scope() {
    for checked in [false, true] {
        let mut harness = Harness::new(ModuleFormat::It, 4, checked, true);
        harness.row([note(48), Cell::default()]); let old = harness.foreground();
        harness.row([Cell { effect: 19, parameter: 0x74, ..Cell::default() }, Cell::default()]);
        harness.row([note(55), Cell::default()]); assert!(harness.voices.get(old).is_some());
        assert_eq!(harness.channels.playback().unwrap().get(old, &harness.voices).unwrap().domain, if checked { VoiceDomain::Custom } else { VoiceDomain::Native });
        let foreground = harness.foreground();
        harness.row([Cell { effect: 19, parameter: 0x70, ..Cell::default() }, Cell::default()]);
        assert!(harness.voices.get(old).is_none()); assert_eq!(harness.foreground(), foreground);
    }
}

#[test]
fn it_new_note_past_a_retained_offset_does_not_inherit_the_previous_identity() {
    let mut builder = ModuleBuilder::new();
    for length in [1024, 64] {
        let sample = builder.add_sample(&vec![1000; length], SampleSpec::one_shot("offset target")).unwrap();
        let mut instrument = InstrumentDef::from_sample("offset target", sample, U0F16::MAX);
        instrument.note_sample_map.fill(sample.0 + 1);
        for (note, mapped) in instrument.note_transpose_map.iter_mut().enumerate() { *mapped = note as u8; }
        builder.add_instrument(instrument).unwrap();
    }
    builder.add_pattern(&encode(ModuleFormat::It, [Cell::default(); 2]), 1, 2).unwrap(); builder.set_orders(&[0]);
    let mut header = ModuleHeader::new(ModuleFormat::It, 2);
    header.format_extra = starplayer::it::ItFormatExtra { flags: 12, special: 0, old_instruments: false, has_midi_configuration: false }.encode();
    builder.set_header(header);
    let module = Arc::new(builder.build().unwrap());
    let mut configured = Harness::with_module(Arc::clone(&module), 4, true, true);
    let mut native = Harness::with_module(module, 4, false, false);
    native.channels = ChannelTable::new(2);
    for harness in [&mut configured, &mut native] {
        harness.row([note(48), Cell::default()]);
        let previous = harness.foreground();
        // A same-instrument-only O01 row does not trigger. It retains source offset256,
        // valid for the long sample, for the next note's initial mixer admission.
        harness.row([Cell { instrument: 1, effect: 15, parameter: 1, ..Cell::default() }, Cell::default()]);
        assert_eq!(harness.foreground(), previous);
        harness.row([Cell { instrument: 2, ..note(55) }, Cell::default()]);
        assert_ne!(harness.foreground(), previous);
        assert!(harness.voices.get(previous).is_none());
    }
    assert_eq!(configured.active(VoiceDomain::Custom), 0);
    let incoming = configured.foreground();
    assert!(configured.channels.playback().unwrap().get(incoming, &configured.voices).unwrap().identity().is_none(), "a genuine new note that cannot sound is not a continuation of the previous musical identity");
    let actual: Vec<_> = configured.voices.iter().map(|(_, voice)| (voice.tag, voice.params, voice.position())).collect();
    let expected: Vec<_> = native.voices.iter().map(|(_, voice)| (voice.tag, voice.params, voice.position())).collect();
    assert_eq!(actual, expected, "identity correction must leave native voice state unchanged");
}

#[cfg(feature = "interactive-audition")]
#[test]
fn no08_native_it_instrument_restart_after_real_sample_completion_survives_unrelated_clears() {
    use starplayer::engine::ClearScope;
    for sample_mode in [false, true] {
        for scope in [None, Some(ClearScope::Live), Some(ClearScope::CustomDetached)] {
            let module = module(ModuleFormat::It, &[], sample_mode, NewNoteAction::Cut, DuplicateCheck::Off, DuplicateAction::Cut);
            let mut harness = Harness::with_module(module, 4, false, false);
            harness.row([note(60), Cell::default()]); let old = harness.foreground();
            harness.voices.accumulate::<FixedPath, starplayer::dsp::Linear>(harness.module.pcm(), &mut vec![starplayer::mixer::FixedFrame::default(); 65536], 8000);
            harness.channels.release_finished(&harness.voices);
            assert!(harness.voices.get(old).is_none(), "sample must finish in the real mixer");
            let mut context = TickContext::new(Frame(65536), &mut harness.voices, &mut harness.channels, RowClock::new(6), SongPosition::default(), 125);
            harness.processor.control_step(&mut context);
            if let Some(scope) = scope { harness.processor.clear_sound(scope, &mut context); }
            harness.row([Cell { instrument: 1, ..Cell::default() }, Cell::default()]);
            assert!(harness.channels.foreground(ChannelId(0)).is_some(), "sample mode {sample_mode}, scope {scope:?}");
        }
    }
}

#[cfg(feature = "interactive-audition")]
#[test]
fn no08_fresh_configured_instrument_only_preserves_native_initial_onsets() {
    use starplayer::core::{Command, InteractiveAction, InteractiveCommand};
    fn run(format: ModuleFormat, sample_mode: bool, interactive: bool) -> Vec<i16> {
        let module = module(format, &[[Cell { instrument: 1, ..Cell::default() }, Cell::default()]], sample_mode, NewNoteAction::Cut, DuplicateCheck::Off, DuplicateAction::Cut);
        let mut configuration = configuration(format, 4);
        if interactive { configuration.capabilities |= PlaybackCapabilities::INTERACTIVE_AUDITION; configuration.capacity.interactive_transport = true; configuration.capacity.live = 1; }
        let settings = EngineSettings { sample_rate_hz: 8000, channel_count: 2, ..EngineSettings::default() }.with_playback(configuration).unwrap();
        let mut engine: Engine<FixedPath, starplayer::dsp::Linear, StereoI16, Arc<Module>> = Engine::with_settings(settings);
        let mut control = engine.take_control().unwrap(); control.load_module(Arc::clone(&module)).unwrap();
        engine.set_source(Box::new(starplayer::NativeSequencer::new_with_articulation(module, 8000, QuirkSelection::FromDialect, settings.voice_capacity).unwrap()));
        if interactive {
            control.send(Command::Interactive(InteractiveCommand { identity: Default::default(), action: InteractiveAction::Stop })).unwrap();
            engine.render(&mut [0; 256]);
            control.send(Command::Interactive(InteractiveCommand { identity: Default::default(), action: InteractiveAction::Play })).unwrap();
        }
        let mut output = vec![0; 1024]; engine.render(&mut output); output
    }
    for format in FORMATS { for sample_mode in [false, true] {
        let native = run(format, sample_mode, false);
        if format == ModuleFormat::It { assert!(native.iter().any(|sample| *sample != 0), "native instrument-only C0 must sound"); }
        else { assert!(native.iter().all(|sample| *sample == 0), "XM has no remembered note on a fresh channel"); }
        assert!(run(format, sample_mode, true) == native, "fresh setup Stop changes {format:?}, sample mode {sample_mode}");
    } }
}
