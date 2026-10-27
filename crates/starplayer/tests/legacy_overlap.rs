#![cfg(all(feature = "note-overlap", feature = "mod", feature = "s3m", feature = "mtm"))]
use starplayer::core::{ChannelId, Frame, InstrumentId, OriginalNote, PlaybackCapabilities, RowClock, TriggerIdentity, U0F16};
use starplayer::core::quirks::QuirkSelection;
use starplayer::engine::{ChannelTable, Engine, EngineSettings, PlaybackConfiguration, SongPosition, TickContext, TrackerProcessor, VoiceCapacity, VoiceDomain, VoiceLifecycle};
use starplayer::mixer::{FixedPath, StereoI16, VoicePool};
use starplayer::model::{InstrumentDef, LoopMode, Module, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec};
use starplayer::rt::Arc;

const FORMATS: [ModuleFormat; 3] = [ModuleFormat::Mod, ModuleFormat::S3m, ModuleFormat::Mtm];
#[derive(Copy, Clone, Default)]
struct Cell { note: u16, instrument: u8, effect: u8, parameter: u8, volume: Option<u8> }
fn note(note: u16, instrument: u8) -> Cell { Cell { note, instrument, ..Cell::default() } }
fn effect(effect: u8, parameter: u8) -> Cell { Cell { effect, parameter, ..Cell::default() } }
fn encode(format: ModuleFormat, cells: [Cell; 2]) -> Vec<u8> {
    cells.into_iter().flat_map(|cell| match format {
        ModuleFormat::Mod => starplayer::mod_file::ModCell { period: cell.note, instrument: cell.instrument, effect: cell.effect, param: cell.parameter }.to_bytes().to_vec(),
        ModuleFormat::Mtm => starplayer::mtm::MtmCell { pitch: cell.note as u8, instrument: cell.instrument, effect: cell.effect, param: cell.parameter }.to_bytes().to_vec(),
        ModuleFormat::S3m => vec![if cell.note == 0 { 255 } else { cell.note as u8 }, cell.instrument, cell.volume.unwrap_or(255), cell.effect, cell.parameter],
        _ => unreachable!(),
    }).collect()
}
fn c(format: ModuleFormat) -> u16 { if format == ModuleFormat::Mod { 428 } else if format == ModuleFormat::Mtm { 24 } else { 0x40 } }
fn g(format: ModuleFormat) -> u16 { if format == ModuleFormat::Mod { 285 } else if format == ModuleFormat::Mtm { 31 } else { 0x47 } }
fn module(format: ModuleFormat, rows: &[[Cell; 2]], speed: u8) -> Arc<Module> {
    let mut builder = ModuleBuilder::new();
    for index in 0..3 {
        let mut specification = SampleSpec::one_shot("long PCM");
        if index == 2 { specification.loop_mode = LoopMode::Forward; specification.loop_end = 32_768; }
        let pcm: Vec<i16> = (0..32_768).map(|frame| if (frame / 19 + index) % 2 == 0 { 1500 } else { -1500 }).collect();
        let sample = builder.add_sample(&pcm, specification).unwrap();
        builder.add_instrument(InstrumentDef::from_sample("instrument", sample, U0F16::MAX)).unwrap();
    }
    let mut bytes = Vec::new();
    for row in 0..64 { bytes.extend(encode(format, rows.get(row).copied().unwrap_or_default())); }
    builder.add_pattern(&bytes, 64, 2).unwrap();
    builder.set_orders(&[0, starplayer::model::ORDER_END]);
    let mut header = ModuleHeader::new(format, 2); header.initial_speed = speed;
    header.flags.fast_volume_slides = true;
    builder.set_header(header);
    Arc::new(builder.build().unwrap())
}
fn configuration(custom: usize) -> PlaybackConfiguration {
    PlaybackConfiguration { identity: Default::default(), capabilities: PlaybackCapabilities::NOTE_OVERLAP, instrument_count: 3,
        capacity: VoiceCapacity { native: 2, custom, live: 0, interactive_transport: false } }
}
fn settings(custom: usize) -> EngineSettings {
    EngineSettings { sample_rate_hz: 8000, channel_count: 2, ..EngineSettings::default() }.with_playback(configuration(custom)).unwrap()
}
struct Harness { module: Arc<Module>, processor: Box<dyn TrackerProcessor>, voices: VoicePool, channels: ChannelTable }
impl Harness {
    fn new(format: ModuleFormat, custom: usize, checked: bool, enabled: bool) -> Self {
        let module = module(format, &[], 6);
        let processor: Box<dyn TrackerProcessor> = match format {
            ModuleFormat::Mod => Box::new(starplayer::mod_file::ModProcessor::new(Arc::clone(&module), 8000)),
            ModuleFormat::Mtm => Box::new(starplayer::mtm::MtmProcessor::new(Arc::clone(&module), 8000)),
            ModuleFormat::S3m => Box::new(starplayer::s3m::S3mProcessor::new(Arc::clone(&module), 8000)),
            _ => unreachable!(),
        };
        let mut channels = ChannelTable::new(2); channels.prepare_playback(configuration(custom)).unwrap();
        let playback = channels.playback_mut().unwrap();
        for instrument in 0..3 { playback.set_let_notes_ring(Default::default(), InstrumentId(instrument), checked).unwrap(); }
        playback.set_note_overlap_enabled(Default::default(), enabled).unwrap();
        Self { module, processor, channels, voices: VoicePool::new(configuration(custom).capacity.physical()) }
    }
    fn row(&mut self, cells: [Cell; 2], frame: u64) {
        let bytes = encode(self.module.header().format, cells);
        let mut context = TickContext::new(Frame(frame), &mut self.voices, &mut self.channels, RowClock::new(6), SongPosition::default(), 125);
        let _ = self.processor.row(&mut context, starplayer::engine::RowRef { order: 0, pattern: 0, row: 0, bytes: &bytes });
    }
    fn tick(&mut self, tick: u16, frame: u64) {
        let mut clock = RowClock::new(6); clock.tick_in_row = tick;
        let mut context = TickContext::new(Frame(frame), &mut self.voices, &mut self.channels, clock, SongPosition::default(), 125);
        let _ = self.processor.tick(&mut context);
    }
    fn foreground(&self) -> starplayer::core::VoiceId { self.channels.foreground(ChannelId(0)).unwrap() }
    fn identity(&self, voice: starplayer::core::VoiceId) -> TriggerIdentity { self.channels.playback().unwrap().get(voice, &self.voices).unwrap().identity().unwrap() }
    fn active(&self, domain: VoiceDomain) -> usize { self.channels.playback().unwrap().active_count(domain, &self.voices) }
    fn retiring(&self, voice: starplayer::core::VoiceId) -> bool { self.channels.playback().unwrap().get(voice, &self.voices).unwrap().lifecycle == VoiceLifecycle::Retiring }
}

#[test]
fn instrument_note_volume_and_origin_control_replacement_at_the_onset_frame() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 8, true, true);
        harness.row([note(c(format), 1), Cell::default()], 17); let first_c = harness.foreground();
        harness.row([note(g(format), 1), note(c(format), 1)], 177); let first_g = harness.foreground();
        let other_channel = harness.channels.foreground(ChannelId(1)).unwrap();
        assert_eq!(harness.active(VoiceDomain::Custom), 1, "{format:?} C survives G");
        harness.row([note(c(format), 1), Cell::default()], 337);
        assert!(harness.retiring(first_c), "same-volume C replaces earlier C at the exact row callback");
        assert!(!harness.retiring(first_g)); assert!(!harness.retiring(other_channel));
        let loud_c = harness.foreground();
        let mut quiet = note(c(format), 1);
        if format == ModuleFormat::S3m { quiet.volume = Some(23); } else { quiet.effect = 0xC; quiet.parameter = 23; }
        harness.row([quiet, Cell::default()], 497); let quiet_c = harness.foreground();
        assert_eq!(harness.identity(quiet_c).onset_volume, 23); assert!(!harness.retiring(loud_c));
        harness.row([quiet, Cell::default()], 657); assert!(harness.retiring(quiet_c)); assert!(!harness.retiring(loud_c));
        harness.row([note(c(format), 2), Cell::default()], 817);
        assert!(harness.voices.get(loud_c).is_some(), "snare survives a different instrument kick");
    }
}

#[test]
fn passive_unchecked_bypassed_and_looping_starts_keep_exact_identity() {
    for format in FORMATS {
        for (checked, enabled) in [(false, true), (true, false)] {
            let mut harness = Harness::new(format, 4, checked, enabled);
            harness.row([note(c(format), 1), Cell::default()], 13); let old = harness.foreground();
            assert_eq!(harness.identity(old).onset_volume, 64);
            let playback = harness.channels.playback_mut().unwrap();
            playback.set_let_notes_ring(Default::default(), InstrumentId(0), true).unwrap();
            playback.set_note_overlap_enabled(Default::default(), true).unwrap();
            harness.row([note(c(format), 1), Cell::default()], 173);
            assert!(harness.retiring(old), "{format:?} enable then repeat matches passive identity");
        }
        let mut harness = Harness::new(format, 4, true, true);
        harness.row([note(c(format), 3), Cell::default()], 0); let looping = harness.foreground();
        assert_eq!(harness.identity(looping).instrument, InstrumentId(2));
        harness.row([note(g(format), 1), Cell::default()], 160);
        assert!(harness.voices.get(looping).is_none()); assert_eq!(harness.active(VoiceDomain::Custom), 0);
    }
}

#[test]
fn delayed_notes_capture_current_base_volume_on_actual_start_and_retriggers_keep_it() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 8, true, true);
        let mut first = note(c(format), 1);
        if format == ModuleFormat::S3m { first.volume = Some(21); } else { first.effect = 0xC; first.parameter = 21; }
        harness.row([first, Cell::default()], 11); let old = harness.foreground();
        let mut delayed = note(g(format), 0);
        delayed.effect = if format == ModuleFormat::S3m { 19 } else { 0xE }; delayed.parameter = 0xD2;
        harness.row([delayed, Cell::default()], 171);
        assert_eq!(harness.foreground(), old); assert_eq!(harness.active(VoiceDomain::Custom), 0);
        harness.tick(1, 331); assert_eq!(harness.foreground(), old);
        harness.tick(2, 491); let onset = harness.foreground();
        assert_ne!(onset, old); assert_eq!(harness.identity(onset).onset_volume, 21); assert_eq!(harness.active(VoiceDomain::Custom), 1);
        let onset_identity = harness.identity(onset);
        let retrigger = if format == ModuleFormat::S3m { effect(17, 0x91) } else { effect(0xE, 0x91) };
        harness.row([retrigger, Cell::default()], 651); harness.tick(1, 811);
        let retriggered = harness.foreground(); assert_eq!(harness.identity(retriggered), onset_identity);
        assert_eq!(harness.active(VoiceDomain::Custom), 1, "effect retrigger never creates a tail");
    }
}

#[test]
fn onset_fine_slides_exclude_tremor_and_later_effects() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 4, true, true);
        let mut onset = note(c(format), 1);
        if format == ModuleFormat::S3m { onset.volume = Some(20); onset.effect = 4; onset.parameter = 0x1F; }
        else { onset.effect = 0xE; onset.parameter = 0xB3; }
        harness.row([onset, Cell::default()], 51); let voice = harness.foreground();
        assert_eq!(harness.identity(voice).onset_volume, if format == ModuleFormat::S3m { 21 } else { 61 });
        let slide = if format == ModuleFormat::S3m { effect(4, 0x01) } else { effect(0xA, 0x01) };
        let identity = harness.identity(voice); harness.row([slide, Cell::default()], 211); harness.tick(1, 371);
        assert_eq!(harness.identity(voice), identity);
        if format == ModuleFormat::S3m {
            let mut tremor = note(c(format), 1); tremor.volume = Some(20); tremor.effect = 9; tremor.parameter = 0x01;
            harness.row([tremor, Cell::default()], 691); assert_eq!(harness.identity(harness.foreground()).onset_volume, 20);
            harness.row([tremor, Cell::default()], 851);
            assert_eq!(harness.voices.get(harness.foreground()).unwrap().params.volume.to_bits(), 0, "I01 toggles off on this onset tick");
            assert_eq!(harness.identity(harness.foreground()).onset_volume, 20, "tick-zero tremor silence does not enter identity");
        }
    }
}

#[test]
fn raw_mod_periods_that_share_a_native_note_do_not_collapse() {
    let mut harness = Harness::new(ModuleFormat::Mod, 4, true, true);
    harness.row([note(428, 1), Cell::default()], 0); let old = harness.foreground();
    harness.row([note(427, 1), Cell::default()], 160);
    assert_eq!(harness.identity(old).note, OriginalNote::ModPeriod(428));
    assert_eq!(harness.identity(harness.foreground()).note, OriginalNote::ModPeriod(427));
    assert!(!harness.retiring(old));
}

#[test]
fn blank_sample_only_portamento_and_invalid_starts_never_detach() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 4, true, true);
        harness.row([note(c(format), 1), Cell::default()], 0);
        harness.row([Cell::default(); 2], 160);
        harness.row([note(0, 2), Cell::default()], 320);
        let mut porta = note(g(format), 1); porta.effect = if format == ModuleFormat::S3m { 7 } else { 3 }; porta.parameter = 2;
        harness.row([porta, Cell::default()], 480); harness.tick(1, 640);
        assert_eq!(harness.active(VoiceDomain::Custom), 0);
        let mut empty = Harness::new(format, 4, true, true);
        empty.row([note(c(format), 99), Cell::default()], 0);
        assert_eq!(empty.active(VoiceDomain::Custom), 0); assert_eq!(empty.active(VoiceDomain::Native), 0);
    }
}

fn render(module: Arc<Module>, configuration: Option<(bool, [bool; 3])>, block: usize) -> Vec<i16> {
    render_muted(module, configuration, block, false)
}
fn render_muted(module: Arc<Module>, configuration: Option<(bool, [bool; 3])>, block: usize, muted: bool) -> Vec<i16> {
    let prepared = configuration.is_some();
    let engine_settings = if prepared { settings(4) } else { EngineSettings { sample_rate_hz: 8000, channel_count: 2, voice_capacity: 2, ..EngineSettings::default() } };
    let mut engine: Engine<FixedPath, starplayer::dsp::Linear, StereoI16, Arc<Module>> = Engine::with_settings(engine_settings);
    let mut control = engine.take_control().unwrap(); control.load_module(Arc::clone(&module)).ok().unwrap();
    if muted { control.send(starplayer::core::Command::MuteChannel { channel: ChannelId(0), muted: true }).ok().unwrap(); }
    engine.set_source(Box::new(starplayer::NativeSequencer::new_with_voice_capacity(module, 8000, QuirkSelection::FromDialect, engine_settings.voice_capacity).unwrap()));
    if let Some((enabled, selections)) = configuration {
        engine.restore_selections(Default::default(), enabled, &selections);
    }
    let mut output = vec![0; 12_000];
    for chunk in output.chunks_mut(block * 2) { engine.render(chunk); }
    output
}

#[test]
fn enabled_pcm_is_block_independent_and_all_three_passive_configurations_are_canonical() {
    for format in FORMATS {
        let rows = [[note(c(format), 1), Cell::default()], [note(g(format), 1), note(c(format), 1)], [note(c(format), 1), Cell::default()], [note(c(format), 2), Cell::default()], [note(c(format), 3), Cell::default()]];
        let module = module(format, &rows, 1);
        let canonical = render(Arc::clone(&module), None, 128);
        let enabled = render(Arc::clone(&module), Some((true, [true; 3])), 128);
        assert_ne!(enabled, canonical, "{format:?} enabled fixture actually overlaps");
        for block in [1, 3, 64, 128, 4096, 8191] {
            assert_eq!(render(Arc::clone(&module), Some((true, [true; 3])), block), enabled, "{format:?} active overlap block {block}");
            for configuration in [(true, [false; 3]), (false, [true; 3]), (true, [false, false, true])] {
                assert_eq!(render(Arc::clone(&module), Some(configuration), block), canonical, "{format:?} passive {configuration:?} block {block}");
            }
        }
    }
}

#[test]
fn all_exact_duplicates_are_retired_and_checkbox_edits_apply_to_future_notes() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 8, false, false);
        let mut earlier = Vec::new();
        for frame in [0, 160, 320] {
            harness.row([note(c(format), 1), Cell::default()], frame); earlier.push(harness.foreground());
            harness.channels.detach_custom_foreground(ChannelId(0), &harness.voices).unwrap();
        }
        let playback = harness.channels.playback_mut().unwrap();
        playback.set_let_notes_ring(Default::default(), InstrumentId(0), true).unwrap();
        playback.set_note_overlap_enabled(Default::default(), true).unwrap();
        harness.row([note(c(format), 1), Cell::default()], 480);
        assert!(earlier.iter().all(|voice| harness.retiring(*voice)), "{format:?} scans every match");
        let foreground = harness.foreground();
        harness.channels.playback_mut().unwrap().set_let_notes_ring(Default::default(), InstrumentId(0), false).unwrap();
        harness.row([note(g(format), 1), Cell::default()], 640);
        assert!(harness.voices.get(foreground).is_none(), "unticked outgoing instrument follows native replacement");
    }
}

#[test]
fn custom_pressure_recycles_oldest_retirement_without_stealing_another_foreground() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 1, true, true);
        harness.row([note(c(format), 1), note(c(format), 2)], 0);
        let unrelated = harness.channels.foreground(ChannelId(1)).unwrap(); let oldest = harness.foreground();
        harness.row([note(g(format), 1), Cell::default()], 160); let second = harness.foreground();
        harness.row([note(c(format), 2), Cell::default()], 320); let third = harness.foreground();
        assert!(harness.retiring(oldest)); assert!(!harness.retiring(second));
        harness.row([note(g(format), 2), Cell::default()], 480);
        assert!(harness.voices.get(oldest).is_none(), "domain's oldest already-retiring generation reclaimed");
        assert!(harness.retiring(second)); assert!(!harness.retiring(third));
        assert_eq!(harness.channels.foreground(ChannelId(1)), Some(unrelated));
        assert_eq!(harness.active(VoiceDomain::Custom), 1);
        assert_eq!(harness.channels.playback().unwrap().overlap_diagnostics().custom_evictions, 2);
        let mut no_budget = Harness::new(format, 0, true, true);
        no_budget.row([note(c(format), 1), Cell::default()], 0); let old = no_budget.foreground();
        no_budget.row([note(g(format), 1), Cell::default()], 160);
        assert!(no_budget.voices.get(old).is_none());
        assert_eq!(no_budget.channels.playback().unwrap().overlap_diagnostics().refused_detachments, 1);
    }
}

#[test]
fn pending_mod_swap_is_discarded_only_after_a_successful_custom_detach() {
    for custom in [0, 4] {
        let mut harness = Harness::new(ModuleFormat::Mod, custom, true, true);
        harness.row([note(428, 1), Cell::default()], 0); let old = harness.foreground();
        harness.row([note(0, 3), Cell::default()], 160);
        assert!(harness.voices.get(old).unwrap().pending_region().unwrap().loop_span().is_some());
        harness.row([note(285, 2), Cell::default()], 320);
        if custom != 0 {
            let tail = harness.voices.get(old).unwrap();
            assert_eq!(tail.pending_region(), None); assert_eq!(tail.region().loop_span(), None);
            assert_eq!(tail.tag.instrument, 1); assert_eq!(harness.identity(old).instrument, InstrumentId(0));
        } else { assert!(harness.voices.get(old).is_none()); }
    }
}

#[test]
fn adopted_mod_swap_uses_actual_sounding_instrument_for_survival() {
    let mut harness = Harness::new(ModuleFormat::Mod, 4, false, true);
    harness.channels.playback_mut().unwrap().set_let_notes_ring(Default::default(), InstrumentId(1), true).unwrap();
    harness.row([note(428, 3), Cell::default()], 0); let old = harness.foreground();
    harness.row([note(0, 2), Cell::default()], 160);
    let mut output = [starplayer::mixer::FixedFrame::default(); 128];
    for _ in 0..512 {
        harness.voices.accumulate::<FixedPath, starplayer::dsp::Linear>(harness.module.pcm(), &mut output, 8000);
        if harness.voices.get(old).unwrap().tag.instrument == 2 { break; }
    }
    let sounding = harness.voices.get(old).unwrap();
    assert_eq!(sounding.tag.instrument, 2); assert_eq!(sounding.tag.sample, 2); assert_eq!(sounding.region().loop_span(), None);
    assert_eq!(harness.identity(old).instrument, InstrumentId(2), "original onset stays immutable");
    harness.row([note(285, 1), Cell::default()], 65_536);
    assert_eq!(harness.active(VoiceDomain::Custom), 1, "checked actual sample 2 survives despite unchecked original instrument 3");
}

#[test]
fn offsets_preserve_protracker_word_fallback_and_do_not_detach_on_nonstarts() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 4, true, true);
        harness.row([note(c(format), 1), Cell::default()], 0); let old = harness.foreground();
        let mut invalid = note(g(format), 1); invalid.effect = if format == ModuleFormat::S3m { 15 } else { 9 }; invalid.parameter = 0xFF;
        harness.row([invalid, Cell::default()], 160);
        if format == ModuleFormat::Mod {
            assert!(harness.voices.get(old).is_some()); assert_eq!(harness.active(VoiceDomain::Custom), 1);
            assert_eq!(harness.voices.get(harness.foreground()).unwrap().region().length_frames(), 2, "ProTracker really starts its retained-pointer one-word fallback");
            assert_eq!(harness.identity(harness.foreground()).onset_volume, 64);
            continue;
        }
        assert!(harness.voices.get(old).is_none()); assert_eq!(harness.active(VoiceDomain::Custom), 0);
        if let Some(foreground) = harness.channels.foreground(ChannelId(0)) {
            assert_eq!(harness.channels.playback().unwrap().get(foreground, &harness.voices).unwrap().identity(), None);
        }
    }
}

#[test]
fn eligibility_checks_tagged_sample_metadata_even_when_pcm_offsets_alias() {
    let module = module(ModuleFormat::Mod, &[], 6);
    let mut image = module.to_image();
    let looping = &module.samples()[2];
    let mut needle = Vec::new();
    needle.extend(looping.pcm_offset().to_le_bytes()); needle.extend(looping.length_frames().to_le_bytes());
    needle.push(1); needle.extend(0u32.to_le_bytes()); needle.extend(looping.loop_end().to_le_bytes());
    let position = image.windows(needle.len()).position(|window| window == needle).unwrap();
    image[position..position + 4].copy_from_slice(&module.samples()[0].pcm_offset().to_le_bytes());
    let aliased = Module::from_image_copied(&image).unwrap();
    assert_eq!(aliased.samples()[0].pcm_offset(), aliased.samples()[2].pcm_offset());
    let region = starplayer::mixer::SampleRegion::one_shot(aliased.samples()[0].pcm_offset(), 32_768);
    let params = starplayer::core::VoiceParams::SILENT;
    let one_shot = starplayer::mixer::Voice::new(starplayer::mixer::VoiceTag { instrument: 1, sample: 1, ..Default::default() }, region, params, 0);
    let looping_tag = starplayer::mixer::Voice::new(starplayer::mixer::VoiceTag { instrument: 3, sample: 3, ..Default::default() }, region, params, 0);
    assert!(starplayer::engine::overlap::eligible_one_shot(&one_shot, &aliased));
    assert!(!starplayer::engine::overlap::eligible_one_shot(&looping_tag, &aliased));
}

#[test]
fn both_normal_loop_modes_sustain_and_unmapped_regions_are_ineligible() {
    let mut builder = ModuleBuilder::new();
    for index in 0..4 {
        let mut specification = SampleSpec::one_shot("eligibility");
        if index < 2 { specification.loop_mode = if index == 0 { LoopMode::Forward } else { LoopMode::PingPong }; specification.loop_end = 64; }
        if index == 2 { specification.sustain_loop = Some(starplayer::model::SustainLoop { mode: LoopMode::Forward, start: 0, end: 64 }); }
        builder.add_sample(&[100; 64], specification).unwrap();
    }
    builder.set_header(ModuleHeader::new(ModuleFormat::Mod, 1));
    let module = builder.build().unwrap();
    for (index, sample) in module.samples().iter().enumerate() {
        // Even an already-released sustain presented as a one-shot remains ineligible.
        let region = starplayer::mixer::SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames());
        let voice = starplayer::mixer::Voice::new(starplayer::mixer::VoiceTag { sample: index as u16 + 1, ..Default::default() }, region, starplayer::core::VoiceParams::SILENT, 0);
        assert_eq!(starplayer::engine::overlap::eligible_one_shot(&voice, &module), index == 3);
    }
    let unmapped = starplayer::mixer::Voice::new(Default::default(), starplayer::mixer::SampleRegion::one_shot(0, 64), starplayer::core::VoiceParams::SILENT, 0);
    assert!(!starplayer::engine::overlap::eligible_one_shot(&unmapped, &module));
}

#[test]
fn detached_tails_keep_origin_mute_routing() {
    for format in FORMATS {
        let rows = [[note(c(format), 1), note(c(format), 2)], [note(g(format), 1), Cell::default()], [note(c(format), 1), Cell::default()]];
        let reference_rows = [[Cell::default(), note(c(format), 2)], [Cell::default(); 2], [Cell::default(); 2]];
        let muted = render_muted(module(format, &rows, 1), Some((true, [true; 3])), 3, true);
        let reference = render(module(format, &reference_rows, 1), None, 128);
        assert_eq!(muted, reference, "{format:?} every tail stays on its original muted lane");
    }
}

#[test]
fn mod_and_mtm_eax_inherited_volume_and_s3m_tick_zero_retrigger_volume_finalize_after_effects() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 4, true, true);
        let mut first = note(c(format), 1);
        if format == ModuleFormat::S3m {
            first.volume = Some(20); first.effect = 17; first.parameter = 0x11;
            harness.row([first, Cell::default()], 0); harness.tick(1, 160);
            let mut incoming = note(g(format), 1); incoming.effect = 17; incoming.parameter = 0x11;
            harness.row([incoming, Cell::default()], 320);
            assert_eq!(harness.identity(harness.foreground()).onset_volume, 63, "Q11's tick-zero volume operation precedes capture");
        } else {
            first.effect = 0xC; first.parameter = 20;
            harness.row([first, Cell::default()], 0);
            let mut incoming = note(g(format), 0); incoming.effect = 0xE; incoming.parameter = 0xA3;
            harness.row([incoming, Cell::default()], 160);
            assert_eq!(harness.identity(harness.foreground()).onset_volume, 23, "EA3 includes the onset fine-up operation after inherited Cxx");
        }
    }
}

#[test]
fn existing_s3m_fast_slide_flag_gap_does_not_invent_an_onset_volume_change() {
    let mut harness = Harness::new(ModuleFormat::S3m, 4, true, true);
    assert!(harness.module.header().flags.fast_volume_slides);
    let incoming = Cell { note: 0x40, instrument: 1, volume: Some(20), effect: 4, parameter: 0x10 };
    harness.row([incoming, Cell::default()], 0);
    // Existing S3mProcessor::static_volume_slide schedules normal Dxy only for minor
    // ticks even when this header flag is set. This pins the current gap, not ST3's
    // canonical fast-slide behavior; its repair is a separate accuracy follow-up.
    let voice = harness.foreground(); assert_eq!(harness.identity(voice).onset_volume, 20);
    harness.tick(1, 160); assert_eq!(harness.identity(voice).onset_volume, 20);
}

#[test]
fn tails_keep_parameters_until_natural_completion_and_slot_reuse_reclaims_identity() {
    for format in FORMATS {
        let mut harness = Harness::new(format, 4, true, true);
        harness.row([note(c(format), 1), Cell::default()], 0); let tail = harness.foreground();
        let parameters = harness.voices.get(tail).unwrap().params;
        harness.row([note(g(format), 1), Cell::default()], 160);
        let slide = if format == ModuleFormat::S3m { effect(4, 0x01) } else { effect(0xA, 0x01) };
        harness.row([slide, Cell::default()], 320); harness.tick(1, 480);
        assert_eq!(harness.voices.get(tail).unwrap().params, parameters, "{format:?} subsequent channel effects target foreground only");
        let mut output = [starplayer::mixer::FixedFrame::default(); 1024];
        for _ in 0..128 { harness.voices.accumulate::<FixedPath, starplayer::dsp::Linear>(harness.module.pcm(), &mut output, 8000); }
        harness.channels.release_finished(&harness.voices);
        assert_eq!(harness.channels.playback().unwrap().iter(&harness.voices).count(), 0);
        harness.row([note(c(format), 2), Cell::default()], 131_072);
        assert!(harness.voices.get(tail).is_none()); assert_eq!(harness.identity(harness.foreground()).instrument, InstrumentId(1));
    }
}

#[test]
fn loaded_legacy_fixtures_match_canonical_pcm_in_all_three_recording_configurations() {
    fn run(module: Arc<Module>, policy: Option<u8>, block: usize) -> (Vec<i16>, bool) {
        let mut engine_settings = EngineSettings { sample_rate_hz: 8000, channel_count: 64, voice_capacity: 64, ..EngineSettings::default() };
        if policy.is_some() {
            engine_settings = engine_settings.with_playback(PlaybackConfiguration { instrument_count: module.instruments().len(),
                capacity: VoiceCapacity { native: 64, custom: 4, live: 0, interactive_transport: false }, ..configuration(4) }).unwrap();
        }
        let mut engine: Engine<FixedPath, starplayer::dsp::Linear, StereoI16, Arc<Module>> = Engine::with_settings(engine_settings);
        let mut control = engine.take_control().unwrap(); control.load_module(Arc::clone(&module)).ok().unwrap();
        engine.set_source(Box::new(starplayer::NativeSequencer::new_with_voice_capacity(Arc::clone(&module), 8000, QuirkSelection::FromDialect, engine_settings.voice_capacity).unwrap()));
        if let Some(policy) = policy {
            let selections: Vec<bool> = module.instruments().iter().map(|instrument| match policy {
                0 => false,
                1 => true,
                _ => instrument.sample.and_then(|sample| module.sample(sample)).is_some_and(|sample| sample.loop_mode().is_looping() || sample.sustain_loop().is_some()),
            }).collect();
            engine.restore_selections(Default::default(), policy != 1, &selections);
        }
        let mut output = vec![0; 32_000]; let mut recorded = false;
        for chunk in output.chunks_mut(block * 2) {
            engine.render(chunk);
            recorded |= engine.channels().playback().is_some_and(|playback| playback.iter(engine.voices()).any(|owned| owned.identity().is_some()));
        }
        (output, recorded)
    }
    for bytes in [
        include_bytes!("../../../fuzz/seeds/mod/synthetic-golden.mod").as_slice(),
        include_bytes!("../../../fuzz/seeds/s3m/minimal.s3m").as_slice(),
        include_bytes!("../../../fuzz/seeds/mtm/synthetic-golden.mtm").as_slice(),
    ] {
        let module = Arc::new(starplayer::load(bytes).unwrap());
        let canonical = run(Arc::clone(&module), None, 128).0;
        for policy in 0..3 {
            for block in [1, 3, 64, 128, 4096, 8191] {
                let (actual, recorded) = run(Arc::clone(&module), Some(policy), block);
                assert!(recorded, "{:?} actually records identity in passive configuration {policy}", module.header().format);
                assert_eq!(actual, canonical, "{:?} loaded fixture passive configuration {policy} block {block}", module.header().format);
            }
        }
    }
}
