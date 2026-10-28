//! Combined native song/live acceptance. The external runner compares separate Cargo
//! feature configurations; runtime bypass alone is deliberately not feature-off proof.
#![cfg(all(feature = "mod", feature = "s3m", feature = "mtm", feature = "xm", feature = "it"))]
use starplayer::core::U0F16;
#[cfg(all(feature = "note-overlap", feature = "interactive-audition"))]
use starplayer::core::{ChannelId, Command, Frame};
use starplayer::core::quirks::QuirkSelection;
use starplayer::engine::{Engine, EngineSettings};
use starplayer::mixer::{FixedPath, StereoI16};
use starplayer::model::{InstrumentDef, Module, ModuleBuilder, ModuleFormat, ModuleHeader, NewNoteAction, SampleSpec};
use starplayer::rt::Arc;
const FORMATS: [ModuleFormat; 5] = [ModuleFormat::Mod, ModuleFormat::S3m, ModuleFormat::Mtm, ModuleFormat::Xm, ModuleFormat::It];
const BLOCKS: [usize; 6] = [1, 3, 64, 128, 4096, 8191];
const FRAMES: usize = 24576;
type TestEngine = Engine<FixedPath, starplayer::dsp::Linear, StereoI16, Arc<Module>>;

fn fixture(format: ModuleFormat) -> Arc<Module> { fixture_with_native_backgrounds(format, false) }
fn fixture_with_native_backgrounds(format: ModuleFormat, native_backgrounds: bool) -> Arc<Module> {
    let mut builder = ModuleBuilder::new();
    for index in 0..3 {
        let pcm: Vec<i16> = (0..65536).map(|frame| if (frame / (17 + index * 5)) % 2 == 0 { 1100 } else { -1100 }).collect();
        let specification = if index == 2 { SampleSpec::one_shot("native loop").with_forward_loop(0, 65536) } else { SampleSpec::one_shot("decay") };
        let sample = builder.add_sample(&pcm, specification).unwrap();
        let mut instrument = InstrumentDef::from_sample("NO acceptance", sample, U0F16::MAX);
        instrument.note_sample_map.fill(index as u16 + 1);
        for (note, transpose) in instrument.note_transpose_map.iter_mut().enumerate() { *transpose = note as u8; }
        // Mixed map alongside a wholly looping instrument; IT's looping NNA stays native.
        if index == 1 { instrument.note_sample_map[67] = 3; }
        instrument.new_note_action = if index == 2 || native_backgrounds && index == 0 { NewNoteAction::Continue } else { NewNoteAction::Cut };
        builder.add_instrument(instrument).unwrap();
    }
    let mut bytes = Vec::new();
    for row in 0..64 { for channel in 0..64 {
        let active = row < 18 && (channel == 0 || channel == 63);
        let instrument = if channel == 63 { 3 } else { [1, 2, 1, 1, 1, 2][row % 6] };
        let note_index = [60, 67, 60, 60, 60, 64][row % 6];
        let quiet = row % 6 == 3 || row % 6 == 4;
        let delayed = row == 7;
        match format {
            ModuleFormat::Mod | ModuleFormat::Mtm => {
                let (effect, parameter) = if delayed { (14, 0xd2) } else if quiet { (12, 23) } else if row == 6 { (14, 0xb3) } else { (0, 0) };
                if format == ModuleFormat::Mod {
                    bytes.extend(starplayer::mod_file::ModCell { period: if active { if note_index == 67 { 285 } else { 428 } } else { 0 }, instrument: if active { instrument } else { 0 }, effect: if active { effect } else { 0 }, param: parameter }.to_bytes());
                } else {
                    bytes.extend(starplayer::mtm::MtmCell { pitch: if active { if note_index == 67 { 31 } else { 24 } } else { 0 }, instrument: if active { instrument } else { 0 }, effect: if active { effect } else { 0 }, param: parameter }.to_bytes());
                }
            }
            ModuleFormat::S3m => bytes.extend([if active { if note_index == 67 { 0x47 } else { 0x40 } } else { 255 }, if active { instrument } else { 0 }, if quiet { 23 } else { 255 }, if active && delayed { 19 } else if active && row == 6 { 4 } else { 0 }, if delayed { 0xd2 } else if row == 6 { 0x1f } else { 0 }]),
            ModuleFormat::Xm => bytes.extend([if active { note_index + 1 } else { 0 }, if active { instrument } else { 0 }, if active && quiet { 0x27 } else { 0 }, if active && (delayed || row == 6) { 14 } else { 0 }, if delayed { 0xd2 } else if row == 6 { 0xb3 } else { 0 }]),
            ModuleFormat::It => bytes.extend([if active { note_index } else { 255 }, if active { instrument } else { 0 }, if quiet { 23 } else { 255 }, if active && delayed { 19 } else if active && row == 6 { 4 } else { 0 }, if delayed { 0xd2 } else if row == 6 { 0x1f } else { 0 }]),
        }
    } }
    builder.add_pattern(&bytes, 64, 64).unwrap(); builder.set_orders(&[0, starplayer::model::ORDER_END]);
    let mut header = ModuleHeader::new(format, 64); header.initial_speed = 3; header.initial_tempo = 137; header.flags.linear_slides = true;
    if format == ModuleFormat::It { header.format_extra = starplayer::it::ItFormatExtra { flags: 12, special: 0, old_instruments: false, has_midi_configuration: false }.encode(); }
    builder.set_header(header); Arc::new(builder.build().unwrap())
}
fn engine(module: &Arc<Module>, prepared: bool) -> (TestEngine, starplayer::engine::EngineHandle<Arc<Module>>) {
    let settings = EngineSettings { sample_rate_hz: 8000, channel_count: 64, voice_capacity: 256, ..EngineSettings::default() };
    #[cfg(feature = "note-overlap")]
    let settings = if prepared { settings.with_playback(starplayer::engine::PlaybackConfiguration { identity: Default::default(), capabilities: starplayer::core::PlaybackCapabilities::NOTE_OVERLAP, instrument_count: 3, capacity: starplayer::engine::VoiceCapacity { native: 256, custom: 64, live: 0, interactive_transport: false } }).unwrap() } else { settings };
    let mut engine = Engine::with_settings(settings);
    let mut control = engine.take_control().unwrap(); control.load_module(Arc::clone(module)).ok().unwrap();
    #[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
    let source = if prepared { starplayer::NativeSequencer::new_with_articulation(Arc::clone(module), 8000, QuirkSelection::FromDialect, settings.voice_capacity).unwrap() } else { starplayer::NativeSequencer::new_with_voice_capacity(Arc::clone(module), 8000, QuirkSelection::FromDialect, settings.voice_capacity).unwrap() };
    #[cfg(not(any(feature = "note-overlap", feature = "interactive-audition")))]
    let source = { let _ = prepared; starplayer::NativeSequencer::new_with_voice_capacity(Arc::clone(module), 8000, QuirkSelection::FromDialect, settings.voice_capacity).unwrap() };
    engine.set_source(Box::new(source)); (engine, control)
}
fn pcm(module: &Arc<Module>, choices: Option<(bool, [bool; 3])>, block: usize) -> Vec<i16> {
    let (mut engine, _control) = engine(module, choices.is_some());
    #[cfg(feature = "note-overlap")]
    if let Some((enabled, selections)) = choices { engine.restore_selections(Default::default(), enabled, &selections); }
    let mut output = vec![0; FRAMES * 2];
    for chunk in output.chunks_mut(block * 2) { engine.render(chunk); }
    output
}
#[test]
fn fresh_canonical_pairs_compare_to_separately_compiled_feature_off_pcm() {
    let artifacts = std::env::var_os("STARPLAYER_NO07_PCM").map(std::path::PathBuf::from);
    // The external runner proves this package graph has neither optional core flag.
    // Ordinary feature-unified consumers may intentionally have the larger variant.
    #[cfg(all(target_pointer_width = "64", not(any(feature = "note-overlap", feature = "interactive-audition"))))]
    if artifacts.is_some() { assert_eq!(std::mem::size_of::<starplayer::core::Command<Arc<Module>>>(), 16); }
    for format in FORMATS {
        let module = fixture(format); let native = pcm(&module, None, 128);
        assert!(native.iter().any(|sample| *sample != 0), "audible {format:?}");
        if let Some(directory) = &artifacts {
            let path = directory.join(format!("{format:?}.pcm"));
            let bytes: Vec<u8> = native.iter().flat_map(|sample| sample.to_le_bytes()).collect();
            if cfg!(not(any(feature = "note-overlap", feature = "interactive-audition"))) { std::fs::create_dir_all(directory).unwrap(); std::fs::write(path, &bytes).unwrap(); }
            else { assert!(std::fs::read(path).unwrap() == bytes, "separately compiled canonical {format:?}"); }
        }
        #[cfg(feature = "note-overlap")]
        let enabled = pcm(&module, Some((true, [true; 3])), 128);
        #[cfg(feature = "note-overlap")]
        assert!(enabled != native, "fixture must audibly overlap {format:?}");
        #[cfg(feature = "note-overlap")]
        if let Some(directory) = &artifacts { std::fs::write(directory.join(format!("{format:?}-overlap.pcm")), enabled.iter().flat_map(|sample| sample.to_le_bytes()).collect::<Vec<_>>()).unwrap(); }
        for block in BLOCKS {
            assert!(pcm(&module, None, block) == native, "native {format:?} block{block}");
            #[cfg(feature = "note-overlap")]
            {
                assert!(pcm(&module, Some((true, [true; 3])), block) == enabled, "enabled {format:?} block{block}");
                for choices in [(true, [false; 3]), (false, [true; 3]), (true, [false, false, true])] {
                    assert!(pcm(&module, Some(choices), block) == native, "canonical {format:?} {choices:?} block{block}");
                }
            }
        }
    }
}

#[cfg(all(feature = "note-overlap", feature = "interactive-audition"))]
#[test]
fn all_formats_share_absolute_song_live_transport_and_bypass_schedule() {
    use starplayer::core::{Event, InstrumentId, InteractiveAction, InteractiveCommand, LiveDestination, Note, TimedEvent};
    use starplayer::engine::{SourceMux, VoiceDomain};
    use starplayer::engine::instrument::{external_event_channel, MidiSource};
    use std::sync::{Arc as Shared, atomic::{AtomicUsize, Ordering}};
    struct Audit { next: u64, maxima: Shared<[AtomicUsize; 4]> }
    impl starplayer::engine::EventSource for Audit {
        fn next_event_frame(&self) -> Option<Frame> { Some(Frame(self.next)) }
        fn advance_to(&mut self, _: Frame) {}
        fn dispatch(&mut self, _: Frame, context: &mut starplayer::engine::EngineContext<'_>) {
            let playback = context.channels.playback().unwrap();
            for (index, domain) in [VoiceDomain::Native, VoiceDomain::Custom, VoiceDomain::Live].into_iter().enumerate() { self.maxima[index].fetch_max(playback.active_count(domain, context.voices), Ordering::Relaxed); }
            let identities = playback.iter(context.voices).filter(|owned| owned.identity().is_some()).count();
            self.maxima[3].fetch_max(identities, Ordering::Relaxed); self.next += 128;
        }
    }
    fn run<Path, Output>(format: ModuleFormat, block: usize) -> (Vec<Output::Sample>, [usize; 4])
    where Path: starplayer::mixer::MixPath, Output: starplayer::mixer::OutputFormat<Accumulator = Path::Accumulator>, Output::Sample: PartialEq {
        let module = fixture(format);
        let mut settings = EngineSettings::interactive_web(Default::default(), 3).unwrap(); settings.sample_rate_hz = 8000;
        assert_eq!(settings.voice_capacity, 339);
        let mut engine: Engine<Path, starplayer::dsp::Linear, Output, Arc<Module>> = Engine::with_settings(settings);
        let mut control = engine.take_control().unwrap(); control.load_module(Arc::clone(&module)).ok().unwrap();
        let (mut producer, feed) = external_event_channel(256);
        let mut events = Vec::new();
        // Includes sub-quantum input, equal/different pre-CC7 velocities, all17 lanes,
        // and saturation immediately after Stop. Times never depend on render calls.
        for (frame, note, velocity) in [(17,60,32768), (161,67,32768), (259,60,32769), (513,60,16000), (777,60,32768), (1025,67,32768), (2051,60,32768), (8193,64,32768)] {
            events.push(TimedEvent::on_channel(Frame(frame), ChannelId(64), Event::NoteOn { note: Note::new(note), velocity: U0F16::from_bits(velocity) }));
        }
        events.push(TimedEvent::on_channel(Frame(257), ChannelId(64), Event::Controller { number: 7, value: U0F16::from_bits(12000) }));
        for index in 0..80 { events.push(TimedEvent::on_channel(Frame(4097 + index), ChannelId(64 + (index % 16) as u16), Event::NoteOn { note: Note::new(48 + (index % 24) as u8), velocity: U0F16::MAX })); }
        events.sort_by_key(|event| event.frame); for event in events { producer.send(event).unwrap(); }
        let rack = starplayer::prepare_live_rack(&module, 8000, 339).unwrap();
        let source = starplayer::NativeSequencer::new_with_articulation(Arc::clone(&module), 8000, QuirkSelection::FromDialect, 339).unwrap();
        let maxima = Shared::new(std::array::from_fn(|_| AtomicUsize::new(0)));
        let mut mux = SourceMux::new(3); mux.insert(Box::new(source)).ok().unwrap(); mux.insert(Box::new(MidiSource::new(feed, rack, 8000))).ok().unwrap(); mux.insert(Box::new(Audit { next: 0, maxima: Shared::clone(&maxima) })).ok().unwrap(); engine.set_source(Box::new(mux));
        let mut command = |action| control.send(Command::Interactive(InteractiveCommand { identity: Default::default(), action })).unwrap();
        command(InteractiveAction::SetLiveDestination(LiveDestination::Jam));
        // Start unchecked and bypassed: identities must already exist before enable.
        let schedule = [(256, InteractiveAction::Play), (512, InteractiveAction::SetLetNotesRing { instrument: InstrumentId(0), enabled: true }), (512, InteractiveAction::SetNoteOverlapEnabled(true)), (768, InteractiveAction::PreviewInstrument(InstrumentId(1))), (1024, InteractiveAction::Pause), (2048, InteractiveAction::Resume), (3072, InteractiveAction::SetNoteOverlapEnabled(false)), (4096, InteractiveAction::Stop), (6144, InteractiveAction::Play), (8192, InteractiveAction::SetNoteOverlapEnabled(true)), (12288, InteractiveAction::StopPreview), (16384, InteractiveAction::AllSoundOff)];
        let mut output = Vec::new(); let mut frame = 0;
        for (deadline, action) in schedule.into_iter().chain(std::iter::once((FRAMES, InteractiveAction::Pause))) {
            let mut segment = vec![Output::Sample::default(); (deadline - frame) * 2]; for chunk in segment.chunks_mut(block * 2) { engine.render(chunk); } output.extend(segment); frame = deadline; command(action);
        }
        assert!(output[34..512].iter().any(|sample| *sample != Output::Sample::default()), "before Play {format:?}");
        assert!(output[8194..12000].iter().any(|sample| *sample != Output::Sample::default()), "after Stop {format:?}");
        (output, std::array::from_fn(|index| maxima[index].load(Ordering::Relaxed)))
    }
    for format in FORMATS {
        let expected = run::<FixedPath, StereoI16>(format, 128); assert!(expected.1[0] > 0 && expected.1[1] > 0 && expected.1[2] > 0 && expected.1[3] > 0, "actual song/custom/live/identity {format:?} {:?}", expected.1);
        assert!(expected.1[0] <= 256 && expected.1[1] <= 64 && expected.1[2] <= 16);
        for block in BLOCKS { let actual = run::<FixedPath, StereoI16>(format, block); assert!(actual == expected, "absolute integrated schedule {format:?} block{block}"); }
        let expected_float = run::<starplayer::mixer::FloatPath, starplayer::mixer::StereoF32>(format, 128);
        let expected_bits: Vec<_> = expected_float.0.iter().map(|sample| sample.to_bits()).collect();
        for block in BLOCKS {
            let actual = run::<starplayer::mixer::FloatPath, starplayer::mixer::StereoF32>(format, block);
            assert_eq!(actual.1, expected_float.1);
            assert!(actual.0.iter().map(|sample| sample.to_bits()).eq(expected_bits.iter().copied()), "float bits {format:?} block{block}");
        }
    }
}

#[cfg(all(feature = "note-overlap", feature = "interactive-audition"))]
#[test]
fn real_it_rows_and_live_rack_fill_all_339_slots_without_cross_domain_stealing() {
    use starplayer::core::{Event, InteractiveAction, InteractiveCommand, LiveDestination, Note, RowClock, TimedEvent};
    use starplayer::engine::{ClearScope, EngineContext, EventSource, RowRef, SongPosition, SourceMux, TickContext, TrackerProcessor, VoiceDomain};
    use starplayer::engine::instrument::{external_event_channel, MidiSource};
    use std::sync::{Arc as Shared, atomic::{AtomicUsize, Ordering}};
    struct NativeRows { processor: starplayer::it::ItProcessor, rows: Vec<Vec<u8>>, stage: usize }
    impl NativeRows {
        fn row(&mut self, index: usize, frame: Frame, context: &mut EngineContext<'_>) {
            let mut tick = TickContext::new(frame, context.voices, context.channels, RowClock::new(6), SongPosition::default(), 137);
            let _ = self.processor.row(&mut tick, RowRef { order: 0, pattern: 0, row: 0, bytes: &self.rows[index] });
        }
    }
    impl EventSource for NativeRows {
        fn next_event_frame(&self) -> Option<Frame> { [17, 19, 21].get(self.stage).copied().map(Frame) }
        fn advance_to(&mut self, _: Frame) {}
        fn dispatch(&mut self, frame: Frame, context: &mut EngineContext<'_>) {
            match self.stage {
                0 => { for _ in 0..256 { self.row(0, frame, context); } for index in 1..=65 { self.row(index, frame, context); } }
                1 => self.row(66, frame, context),
                2 => {
                    // Clear only the native origin; C on lane0 and all L stay intact.
                    context.channels.clear_scope(ClearScope::Lane(ChannelId(63)), context.voices);
                    for _ in 0..255 { self.row(0, frame, context); }
                }
                _ => unreachable!(),
            }
            self.stage += 1;
        }
    }
    struct Audit { next: u64, observed: Shared<[AtomicUsize; 8]> }
    impl EventSource for Audit {
        fn next_event_frame(&self) -> Option<Frame> { (self.next <= 24).then_some(Frame(self.next)) }
        fn advance_to(&mut self, _: Frame) {}
        fn dispatch(&mut self, _: Frame, context: &mut EngineContext<'_>) {
            let playback = context.channels.playback().unwrap();
            let counts = [VoiceDomain::Native, VoiceDomain::Custom, VoiceDomain::Live].map(|domain| playback.active_count(domain, context.voices));
            for (index, count) in counts.into_iter().enumerate() { self.observed[index].fetch_max(count, Ordering::Relaxed); }
            if counts == [256, 64, 16] { self.observed[3].fetch_max(context.voices.voices_active(), Ordering::Relaxed); }
            for (voice, _) in context.voices.iter() { self.observed[4].fetch_max(voice.index() as usize, Ordering::Relaxed); }
            for (index, domain) in [VoiceDomain::Native, VoiceDomain::Custom, VoiceDomain::Live].into_iter().enumerate() { self.observed[5 + index].fetch_max(playback.iter(context.voices).filter(|owned| owned.domain == domain).count(), Ordering::Relaxed); }
            self.next += 1;
        }
    }
    let module = fixture_with_native_backgrounds(ModuleFormat::It, true);
    let mut rows = Vec::new();
    for index in 0..67 {
        let mut row = [255, 0, 255, 0, 0].repeat(64);
        let lane = if index == 0 { 63 } else { 0 };
        row[lane * 5..lane * 5 + 5].copy_from_slice(&[48 + (index / 64) as u8, if index == 0 { 1 } else { 2 }, if index == 0 { 64 } else { (index % 64) as u8 }, 0, 0]); rows.push(row);
    }
    let mut settings = EngineSettings::interactive_web(Default::default(), 3).unwrap(); settings.sample_rate_hz = 8000;
    let mut engine: Engine<FixedPath, starplayer::dsp::Linear, StereoI16, Arc<Module>> = Engine::with_settings(settings);
    let mut control = engine.take_control().unwrap(); control.load_module(Arc::clone(&module)).ok().unwrap(); engine.restore_selections(Default::default(), true, &[false, true, false]);
    let (mut producer, feed) = external_event_channel(128);
    for index in 0..17 { producer.send(TimedEvent::on_channel(Frame(if index < 16 { 18 } else { 20 }), ChannelId(64 + (index % 16) as u16), Event::NoteOn { note: Note::new(48 + (index % 24) as u8), velocity: U0F16::MAX })).unwrap(); }
    let rack = starplayer::prepare_live_rack(&module, 8000, 339).unwrap();
    let processor = starplayer::it::ItProcessor::try_with_quirks_and_voice_capacity(Arc::clone(&module), 8000, QuirkSelection::FromDialect, 339).unwrap();
    let observed = Shared::new(std::array::from_fn(|_| AtomicUsize::new(0)));
    let mut mux = SourceMux::new(3); mux.insert(Box::new(NativeRows { processor, rows, stage: 0 })).ok().unwrap(); mux.insert(Box::new(MidiSource::new(feed, rack, 8000))).ok().unwrap(); mux.insert(Box::new(Audit { next: 17, observed: Shared::clone(&observed) })).ok().unwrap(); engine.set_source(Box::new(mux));
    control.send(Command::Interactive(InteractiveCommand { identity: Default::default(), action: InteractiveAction::SetLiveDestination(LiveDestination::Jam) })).unwrap();
    let mut output = [0; 256]; engine.render(&mut output);
    assert_eq!(std::array::from_fn::<_, 8, _>(|index| observed[index].load(Ordering::Relaxed)), [256, 64, 16, 339, 338, 257, 65, 17]);
    assert!(output.iter().any(|sample| *sample != 0));
}
