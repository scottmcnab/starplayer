#![cfg(all(feature = "interactive-audition", feature = "xm", feature = "it"))]
use starplayer::core::{ChannelId, Event, Frame, InstrumentId, InteractiveAction, LiveDestination, Note, PlaybackCapabilities, Target, U0F16, VoiceId};
use starplayer::engine::{ChannelTable, ControlClock, EngineContext, EventSource, PlaybackConfiguration, VoiceCapacity, VoiceDomain, VoiceLifecycle};
use starplayer::engine::instrument::{external_event_channel, MidiSource, ExternalEventQueue};
use starplayer::mixer::VoicePool;
use starplayer::model::{AutoVibrato, AutoVibratoWaveform, Envelope, EnvelopePoint, EnvelopeSpan, InstrumentDef, Module, ModuleBuilder, ModuleFormat, ModuleHeader, NewNoteAction, SampleSpec};
use starplayer::rt::Arc;
const FORMATS: [ModuleFormat; 2] = [ModuleFormat::Xm, ModuleFormat::It];
fn module(format: ModuleFormat, looping: bool, nna: NewNoteAction, instruments: usize) -> Arc<Module> { detailed_module(format, looping, nna, instruments, true, 0) }
fn detailed_module(format: ModuleFormat, looping: bool, nna: NewNoteAction, instruments: usize, filter: bool, rate_scale: u8) -> Arc<Module> {
    waveform_module(format,looping,nna,instruments,filter,rate_scale,AutoVibratoWaveform::Sine)
}
fn waveform_module(format: ModuleFormat, looping: bool, nna: NewNoteAction, instruments: usize, filter: bool, rate_scale: u8, waveform: AutoVibratoWaveform) -> Arc<Module> {
    let mut builder = ModuleBuilder::new();
    let pcm: Vec<i16> = (0..32768).map(|index| if index % 37 < 19 { 1800 } else { -1800 }).collect();
    let mut specification = SampleSpec { rate_scale_log2: rate_scale, auto_vibrato: AutoVibrato { waveform, sweep: 8, depth: 4, rate: 4 }, ..SampleSpec::one_shot("native") };
    if looping { specification = specification.with_forward_loop(0, 32768); }
    let sample = builder.add_sample(&pcm, specification).unwrap();
    for _ in 0..instruments {
        let mut instrument = InstrumentDef::from_sample("native", sample, U0F16::MAX);
        instrument.note_sample_map.fill(1);
        for (note, mapped) in instrument.note_transpose_map.iter_mut().enumerate() { *mapped = note as u8; }
        instrument.volume_envelope = Some(Envelope { points: vec![EnvelopePoint { tick: 0, value: 64 }, EnvelopePoint { tick: 1, value: 48 }, EnvelopePoint { tick: 2, value: 32 }, EnvelopePoint { tick: 8, value: 0 }].into_boxed_slice(), sustain: Some(EnvelopeSpan { start: 1, end: 1 }), ..Envelope::default() });
        instrument.panning_envelope = Some(Envelope { points: vec![EnvelopePoint { tick: 0, value: 32 }, EnvelopePoint { tick: 8, value: 48 }].into_boxed_slice(), ..Envelope::default() });
        instrument.pitch_envelope = Some(Envelope { points: vec![EnvelopePoint { tick: 0, value: 32 }, EnvelopePoint { tick: 8, value: 48 }].into_boxed_slice(), ..Envelope::default() });
        instrument.pitch_envelope_is_filter = filter;
        instrument.initial_filter_cutoff = Some(60); instrument.initial_filter_resonance = Some(20);
        instrument.fadeout = 128; instrument.new_note_action = nna; instrument.random_volume_variation = 10; instrument.random_pan_variation = 8;
        builder.add_instrument(instrument).unwrap();
    }
    let cell = if format == ModuleFormat::Xm { vec![0; 5] } else { vec![255, 0, 255, 0, 0] };
    builder.add_pattern(&cell.repeat(64), 1, 64).unwrap(); builder.set_orders(&[0]);
    let mut header = ModuleHeader::new(format, 64); header.flags.linear_slides = true;
    if format == ModuleFormat::It { header.format_extra = starplayer::it::ItFormatExtra { flags: 12, special: 0, old_instruments: false, has_midi_configuration: false }.encode(); }
    builder.set_header(header); Arc::new(builder.build().unwrap())
}
fn configuration(instruments: usize) -> PlaybackConfiguration {
    let capabilities = PlaybackCapabilities::INTERACTIVE_AUDITION;
    #[cfg(feature = "note-overlap")] let capabilities = capabilities | PlaybackCapabilities::NOTE_OVERLAP;
    PlaybackConfiguration { identity: Default::default(), capabilities, instrument_count: instruments, capacity: VoiceCapacity { native: 256, custom: if cfg!(feature = "note-overlap") { 64 } else { 0 }, live: 16, interactive_transport: true } }
}
struct Harness { module: Arc<Module>, source: MidiSource<ExternalEventQueue>, voices: VoicePool, channels: ChannelTable, clock: ControlClock, frame: Frame }
impl Harness {
    fn new(module: Arc<Module>) -> Self {
        let config = configuration(module.instruments().len());
        let mut channels = ChannelTable::try_with_layout(starplayer::engine::LaneLayout::interactive(64).unwrap()).unwrap(); channels.prepare_playback(config).unwrap();
        let (_, feed) = external_event_channel(8);
        let rack = starplayer::prepare_live_rack(&module, 8000, config.capacity.physical()).unwrap();
        let mut result = Self { module, source: MidiSource::new(feed, rack, 8000), voices: VoicePool::new(config.capacity.physical()), channels, clock: ControlClock::new(8000, Frame::ZERO), frame: Frame(17) };
        result.action(InteractiveAction::SetLiveDestination(LiveDestination::Jam)); result
    }
    fn event(&mut self, lane: u16, event: Event) {
        let mut context = EngineContext::new(self.frame, &mut self.voices, &mut self.channels, &mut self.clock);
        self.source.rack_mut().apply(Target::Channel(ChannelId(lane)), event, &mut context);
    }
    fn action(&mut self, action: InteractiveAction) { let mut context = EngineContext::new(self.frame, &mut self.voices, &mut self.channels, &mut self.clock); self.source.interactive_action(action, &mut context); }
    fn on(&mut self, lane: u16, note: u8, velocity: u16) -> VoiceId { self.event(lane, Event::NoteOn { note: Note::new(note), velocity: U0F16::from_bits(velocity) }); self.channels.foreground(ChannelId(lane)).unwrap() }
    #[cfg(feature = "note-overlap")]
    fn off(&mut self, lane: u16, note: u8) { self.event(lane, Event::NoteOff { note: Note::new(note), velocity: U0F16::ZERO }); }
    fn step(&mut self, frame: u64) { self.frame = Frame(frame); let mut context = EngineContext::new(self.frame, &mut self.voices, &mut self.channels, &mut self.clock); self.source.post_source_articulation(self.frame, &mut context); }
    fn retiring(&self, voice: VoiceId) -> bool { self.channels.playback().unwrap().get(voice, &self.voices).is_some_and(|owned| owned.lifecycle == VoiceLifecycle::Retiring) }
    #[cfg(feature = "note-overlap")]
    fn overlap(&mut self, checked: bool, enabled: bool) { let playback = self.channels.playback_mut().unwrap(); playback.set_let_notes_ring(Default::default(), InstrumentId(0), checked).unwrap(); playback.set_note_overlap_enabled(Default::default(), enabled).unwrap(); }
}
#[test]
fn actual_live_native_onset_and_articulation_match_controlled_tracker_notes() {
    use starplayer::engine::{TickContext, RowRef, SongPosition, TrackerProcessor};
    use starplayer::core::{RowClock, quirks::QuirkSelection};
    for format in FORMATS {
        let module = module(format, true, NewNoteAction::Continue, 1);
        let mut live = Harness::new(Arc::clone(&module));
        let mut song_voices = VoicePool::new(339); let mut song_channels = ChannelTable::new(64);
        let mut processor: Box<dyn TrackerProcessor> = if format == ModuleFormat::Xm { Box::new(starplayer::xm::XmProcessor::new(Arc::clone(&module), 8000)) } else { Box::new(starplayer::it::ItProcessor::try_with_quirks_and_voice_capacity(Arc::clone(&module), 8000, QuirkSelection::FromDialect, 339).unwrap()) };
        // XM's live60 reference equals native48 pitch; IT's reference is60. Every
        // map entry in this fixture names sample1; map selection is tested separately.
        let row = if format == ModuleFormat::Xm { vec![49, 1, 0, 0, 0] } else { vec![60, 1, 255, 0, 0] };
        let mut context = TickContext::new(Frame(17), &mut song_voices, &mut song_channels, RowClock::new(6), SongPosition::default(), 125);
        let _ = processor.row(&mut context, RowRef { order: 0, pattern: 0, row: 0, bytes: &row });
        let voice = live.on(64, 60, u16::MAX); let song = song_channels.foreground(ChannelId(0)).unwrap();
        for tick in 0..6 {
            if tick != 0 { live.step(17 + tick * 160); let mut clock = RowClock::new(6); clock.tick_in_row = tick as u16; let mut context = TickContext::new(Frame(17 + tick * 160), &mut song_voices, &mut song_channels, clock, SongPosition::default(), 125); let _ = processor.tick(&mut context); }
            let actual = live.voices.get(voice).unwrap().params; let expected = song_voices.get(song).unwrap().params;
            assert_eq!((actual.step, actual.volume, actual.pan, actual.filter), (expected.step, expected.volume, expected.pan, expected.filter), "{format:?}, tick{tick}");
        }
        let before = live.voices.get(voice).unwrap().params; live.step(live.frame.0); // Only onset is suppressed, ordinary duplicate hooks are caller-owned.
        assert_ne!(before.step, starplayer::core::Step::ZERO);
    }
}
#[test]
fn all_live_pressure_uses_one_budget_and_every_press_admits_without_touching_song() {
    for format in FORMATS {
        let mut live = Harness::new(module(format, true, NewNoteAction::Continue, 1));
        let sample = live.module.sample(starplayer::core::SampleId(0)).unwrap();
        let native = live.channels.trigger(ChannelId(63), &mut live.voices, Default::default(), starplayer::engine::instrument::sample_region(sample), starplayer::core::VoiceParams::SILENT, 0).unwrap();
        for index in 0..100 { live.frame = Frame(index + 1); let voice = live.on(64, 60 + (index % 12) as u8, u16::MAX); assert_eq!(live.voices.get(voice).unwrap().position(), 0); assert!(!live.retiring(voice)); assert!(live.voices.get(native).is_some()); assert!(live.channels.playback().unwrap().active_count(VoiceDomain::Live, &live.voices) <= 16); assert!(live.channels.playback().unwrap().iter(&live.voices).filter(|owned| owned.domain == VoiceDomain::Live).count() <= 17); }
        live.channels.clear_scope(starplayer::engine::ClearScope::Live, &mut live.voices);
        for index in 0..30 { live.frame = Frame(200 + index); let voice = live.on(65, 60, u16::MAX); assert!(!live.retiring(voice)); }
        assert!(live.voices.get(native).is_some());
    }
}
#[cfg(feature = "note-overlap")]
#[test]
fn detached_held_notes_release_through_sustain_and_keep_passive_exact_identity() {
    for format in FORMATS {
        let mut live = Harness::new(module(format, false, NewNoteAction::Continue, 1)); live.overlap(false, false);
        let first = live.on(64, 60, u16::MAX); let identity = live.channels.playback().unwrap().get(first, &live.voices).unwrap().identity().unwrap();
        live.overlap(true, true); live.frame = Frame(18); let second = live.on(64, 67, u16::MAX);
        assert!(!live.retiring(first)); assert_eq!(live.channels.playback().unwrap().get(first, &live.voices).unwrap().domain, VoiceDomain::Live);
        live.event(64, Event::Controller { number: 64, value: U0F16::MAX }); live.off(64, 60);
        for tick in 1..5 { live.step(18 + tick * 160); }
        assert!(!live.retiring(first)); live.event(64, Event::Controller { number: 64, value: U0F16::ZERO });
        assert_eq!(live.channels.playback().unwrap().get(first, &live.voices).unwrap().identity(), Some(identity));
        for tick in 5..20 { live.step(18 + tick * 160); }
        assert!(live.retiring(first)); assert!(!live.retiring(second));
    }
}
#[cfg(feature = "note-overlap")]
#[test]
fn native_velocity_collisions_and_cc7_have_the_documented_duplicate_identity() {
    for format in FORMATS {
        let mut live = Harness::new(module(format, false, NewNoteAction::Continue, 1)); live.overlap(true, true);
        let first = live.on(64, 60, 32768); live.frame = Frame(18);
        live.event(64, Event::Controller { number: 7, value: U0F16::from_bits(12000) }); let second = live.on(64, 60, 32769);
        assert!(live.retiring(first), "adjacent raw velocities map to the same native base");
        let identity = live.channels.playback().unwrap().get(second, &live.voices).unwrap().identity().unwrap(); assert_eq!(identity.onset_volume, if format == ModuleFormat::Xm { 32 } else { 128 });
        live.frame = Frame(19); live.on(64, 60, 16000); assert!(!live.retiring(second), "different native base overlaps");
        let other = live.on(65, 60, 32768); live.on(64, 60, 32768); assert!(!live.retiring(other));
    }
}
#[test]
fn exact_high_instrument_ids_and_one_shot_button_release_are_preserved() {
    for format in FORMATS {
        let mut live = Harness::new(module(format, false, NewNoteAction::Continue, 257));
        live.action(InteractiveAction::PreviewInstrument(InstrumentId(256)));
        let voice = live.channels.foreground(ChannelId(80)).unwrap(); assert_eq!(live.voices.get(voice).unwrap().tag.instrument, 257);
        live.action(InteractiveAction::ReleasePreview);
        for tick in 1..20 { live.step(17 + tick * 160); }
        assert!(!live.retiring(voice), "one-shot button preview ignores button-up, envelope sustains natively");
    }
}

#[test]
fn exact_absolute_live_schedule_is_audible_and_identical_at_all_six_host_sizes() {
    use starplayer::core::{Command, InteractiveCommand, TimedEvent};
    use starplayer::engine::{Engine, EngineSettings, SourceMux};
    use starplayer::mixer::{FixedPath, StereoI16};
    fn run(format: ModuleFormat, block: usize) -> Vec<i16> {
        let module = module(format, true, NewNoteAction::Continue, 1);
        let settings = EngineSettings { sample_rate_hz: 8000, channel_count: 64, ..EngineSettings::default() }.with_playback(configuration(1)).unwrap();
        let mut engine: Engine<FixedPath, starplayer::dsp::Linear, StereoI16, Arc<Module>> = Engine::with_settings(settings);
        let mut handle = engine.take_control().unwrap(); handle.load_module(Arc::clone(&module)).ok().unwrap();
        let (mut producer, queue) = external_event_channel(256);
        let mut events=Vec::new();
        for (frame, lane, note) in [(17,64,60), (160,65,67), (257,64,64), (513,64,60), (1025,64,62), (2049,65,67), (4097,64,60)] {
            events.push(TimedEvent::on_channel(Frame(frame), ChannelId(lane), Event::NoteOn { note: Note::new(note), velocity: U0F16::MAX }));
            events.push(TimedEvent::on_channel(Frame(frame + 1), ChannelId(lane), Event::NoteOff { note: Note::new(note), velocity: U0F16::ZERO }));
        }
        for index in 0..64 {events.push(TimedEvent::on_channel(Frame(1027+index),ChannelId(64+(index%16) as u16),Event::NoteOn {note:Note::new(48+(index%24) as u8),velocity:U0F16::MAX}));}
        events.sort_by_key(|event|event.frame);for event in events {producer.send(event).unwrap();}
        let rack = starplayer::prepare_live_rack(&module, 8000, settings.voice_capacity).unwrap();
        let sequencer = starplayer::NativeSequencer::new_with_articulation(Arc::clone(&module), 8000, starplayer::core::quirks::QuirkSelection::FromDialect, settings.voice_capacity).unwrap();
        let mut mux = SourceMux::new(2); mux.insert(Box::new(sequencer)).ok().unwrap(); mux.insert(Box::new(MidiSource::new(queue, rack, 8000))).ok().unwrap(); engine.set_source(Box::new(mux));
        handle.send(Command::Interactive(InteractiveCommand { identity: Default::default(), action: InteractiveAction::SetLiveDestination(LiveDestination::Jam) })).unwrap();
        let mut output = Vec::new();
        for (action, frames) in [(None,256), (Some(InteractiveAction::Play),256), (Some(InteractiveAction::Pause),512), (Some(InteractiveAction::Stop),1024), (Some(InteractiveAction::Resume),2048), (Some(InteractiveAction::Stop),1024)] {
            if let Some(action) = action { handle.send(Command::Interactive(InteractiveCommand { identity: Default::default(), action })).unwrap(); }
            let mut segment = vec![0; frames * 2]; for chunk in segment.chunks_mut(block * 2) { engine.render(chunk); } output.extend(segment);
        }
        assert!(output[..34].iter().all(|sample| *sample == 0)); assert!(output[34..512].iter().any(|sample| *sample != 0), "before first Play");
        assert!(output[2048..4096].iter().any(|sample| *sample != 0), "fresh input after Stop");
        output
    }
    for format in FORMATS { let expected = run(format,128); for block in [1,3,64,128,4096,8191] { assert!(run(format,block) == expected, "{format:?} block{block}"); } }
}

#[test]
fn normal_preview_release_and_route_cleanup_have_different_scopes() {
    for format in FORMATS {
        let mut live = Harness::new(module(format, true, NewNoteAction::Continue, 2));
        live.action(InteractiveAction::PreviewInstrument(InstrumentId(0))); let preview = live.channels.foreground(ChannelId(80)).unwrap();
        live.action(InteractiveAction::ReleasePreview);
        assert!(!live.retiring(preview), "native release runs its envelope");
        live.action(InteractiveAction::PreviewInstrument(InstrumentId(1))); assert!(live.retiring(preview), "instrument change clears released backgrounds too");
        let preview = live.channels.foreground(ChannelId(80)).unwrap(); live.action(InteractiveAction::SetLiveDestination(LiveDestination::Selected(InstrumentId(0)))); assert!(live.retiring(preview));
        let input = live.on(64,60,u16::MAX); live.event(64,Event::Program(InstrumentId(1))); assert!(live.retiring(input));
    }
}

#[test]
fn metadata_and_map_selection_do_not_fall_back_from_an_unmapped_reference() {
    for format in FORMATS {
        let mut builder = ModuleBuilder::new();
        let first = builder.add_sample(&[1000;100], SampleSpec::one_shot("first")).unwrap();
        let second = builder.add_sample(&[2000;100], SampleSpec::one_shot("second").with_forward_loop(0,100)).unwrap();
        let mut instrument = InstrumentDef::from_sample("mapped", first, U0F16::MAX);
        instrument.note_sample_map.fill(0); instrument.note_sample_map[61] = second.0 + 1; instrument.note_transpose_map[61] = 72;
        builder.add_instrument(instrument).unwrap();
        let cell = if format == ModuleFormat::Xm { vec![0;5] } else { vec![255,0,255,0,0] };
        builder.add_pattern(&cell,1,1).unwrap(); builder.set_orders(&[0]); let mut header = ModuleHeader::new(format,1); header.flags.linear_slides = true;
        if format == ModuleFormat::It { header.format_extra = starplayer::it::ItFormatExtra { flags:12, special:0, old_instruments:false, has_midi_configuration:false }.encode(); }
        builder.set_header(header); let module = Arc::new(builder.build().unwrap());
        let capability = starplayer::live_instrument_capability(&module,InstrumentId(0));
        assert!(capability.playable && capability.looping); assert!(!capability.preview_playable && !capability.one_shot);
        let mut live = Harness::new(module); live.action(InteractiveAction::PreviewInstrument(InstrumentId(0))); assert_eq!(live.voices.voices_active(),0);
        let voice = live.on(64,61,u16::MAX); assert_eq!(live.voices.get(voice).unwrap().tag.sample,second.0+1);
        assert_eq!(live.voices.get(voice).unwrap().tag.note,if format == ModuleFormat::It {72} else {61});
    }
}

#[test]
fn overflowing_recoverable_queue_releases_held_loop_and_reports_count() {
    use starplayer::core::TimedEvent;
    for format in FORMATS {
        let mut live = Harness::new(module(format,true,NewNoteAction::Continue,1));
        let voice = live.on(64,60,u16::MAX);
        let (mut producer, queue) = starplayer::engine::instrument::recoverable_event_channel(1);
        let rack = starplayer::prepare_live_rack(&live.module,8000,live.voices.capacity()).unwrap();
        let mut source = MidiSource::new(queue,rack,8000);
        // Use the prepared source's real generation table for the held loop.
        let mut context = EngineContext::new(Frame(20),&mut live.voices,&mut live.channels,&mut live.clock);
        source.interactive_action(InteractiveAction::SetLiveDestination(LiveDestination::Jam),&mut context);
        source.rack_mut().apply(Target::Channel(ChannelId(65)),Event::NoteOn {note:Note::new(60),velocity:U0F16::MAX},&mut context);
        let held = context.channels.foreground(ChannelId(65)).unwrap();
        producer.send(TimedEvent::global(Frame(21),Event::AllNotesOff)).unwrap();
        assert!(producer.send(TimedEvent::global(Frame(22),Event::AllNotesOff)).is_err());
        source.advance_to(Frame(23)); source.dispatch(Frame(23),&mut context);
        assert_eq!(source.rack().live_diagnostics().queue_overflows,1);
        for tick in 1..40 { context.frame=Frame(23+tick*160); source.post_source_articulation(context.frame,&mut context); }
        assert!(context.channels.playback().unwrap().get(held,context.voices).is_none_or(|owned| owned.lifecycle == VoiceLifecycle::Retiring));
        assert!(context.voices.get(voice).is_some());
    }
}

#[test]
fn cents_and_bend_keep_current_modulation_and_scale_prepared_pcm_exactly_once() {
    for format in FORMATS { for rate_scale in [0,2] {
        let mut live = Harness::new(detailed_module(format,true,NewNoteAction::Continue,1,false,rate_scale));
        live.event(64,Event::NoteOn {note:Note::with_cents(60,50),velocity:U0F16::MAX}); let microtonal = live.channels.foreground(ChannelId(64)).unwrap();
        let plain = live.on(65,60,u16::MAX);
        assert!(live.voices.get(microtonal).unwrap().params.step > live.voices.get(plain).unwrap().params.step);
        for tick in 1..5 { live.step(17+tick*160); }
        let original = live.voices.get(microtonal).unwrap().params;
        live.event(64,Event::PitchBend(starplayer::core::I1F15::MAX));
        assert!(live.voices.get(microtonal).unwrap().params.step > original.step);
        live.event(64,Event::PitchBend(starplayer::core::I1F15::ZERO));
        assert_eq!(live.voices.get(microtonal).unwrap().params.step,original.step,"bend reset preserves cents and current envelope/autovibrato");
        assert_eq!(live.voices.get(microtonal).unwrap().params.volume,original.volume);
        let mut reference = Harness::new(detailed_module(format,true,NewNoteAction::Continue,1,false,0));
        reference.event(64,Event::NoteOn {note:Note::with_cents(60,50),velocity:U0F16::MAX}); let reference_voice = reference.channels.foreground(ChannelId(64)).unwrap();
        for tick in 1..5 { reference.step(17+tick*160); }
        assert_eq!(original.step.to_bits(),reference.voices.get(reference_voice).unwrap().params.step.to_bits()<<rate_scale);
    } }
}

#[test]
fn undersized_public_runtime_refuses_before_allocating_or_binding_any_generation() {
    use starplayer::engine::instrument::live::{RackRuntime,RackScope};
    for format in FORMATS {
        let mut live = Harness::new(module(format,false,NewNoteAction::Continue,1));
        let runtime: Box<dyn RackRuntime> = if format == ModuleFormat::Xm {Box::new(starplayer::xm::processor::live::XmLiveRuntime::try_new(1,8000).unwrap())} else {Box::new(starplayer::it::processor::live::ItLiveRuntime::try_new(1,8000).unwrap())};
        let rack = starplayer::engine::InstrumentRack::try_prepared(&live.module,8000,live.voices.capacity(),Some(runtime),true,RackScope::Live).unwrap();
        let old = live.source.replace_rack(rack); drop(old); live.action(InteractiveAction::SetLiveDestination(LiveDestination::Jam));
        live.event(64,Event::NoteOn {note:Note::new(60),velocity:U0F16::MAX});
        assert_eq!(live.voices.voices_active(),0); assert!(live.channels.foreground(ChannelId(64)).is_none());
        let runtime: Box<dyn RackRuntime> = if format == ModuleFormat::Xm {Box::new(starplayer::xm::processor::live::XmLiveRuntime::try_new(live.voices.capacity(),8000).unwrap())} else {Box::new(starplayer::it::processor::live::ItLiveRuntime::try_new(live.voices.capacity(),8000).unwrap())};
        let rack = starplayer::engine::InstrumentRack::try_prepared(&live.module,8000,1,Some(runtime),true,RackScope::Live).unwrap();
        drop(live.source.replace_rack(rack)); live.action(InteractiveAction::SetLiveDestination(LiveDestination::Jam));
        live.event(64,Event::NoteOn {note:Note::new(60),velocity:U0F16::MAX});
        assert_eq!(live.voices.voices_active(),0); assert!(live.channels.foreground(ChannelId(64)).is_none());
    }
}

#[test]
fn same_frame_cadence_does_not_step_the_native_onset_twice() {
    for format in FORMATS {
        let mut live = Harness::new(module(format,true,NewNoteAction::Continue,1)); let voice = live.on(64,60,u16::MAX);
        let onset = live.voices.get(voice).unwrap().params; live.step(17); assert_eq!(live.voices.get(voice).unwrap().params,onset);
        live.step(177); assert!(live.voices.get(voice).unwrap().params.volume < onset.volume);
    }
}

#[test]
fn live_random_swing_never_changes_future_native_song_random_choices() {
    use starplayer::engine::{TickContext,RowRef,SongPosition,TrackerProcessor};
    use starplayer::core::{RowClock,quirks::QuirkSelection};
    fn run(audition: bool) -> Vec<starplayer::core::VoiceParams> {
        let module = module(ModuleFormat::It,true,NewNoteAction::Continue,1); let mut live = Harness::new(Arc::clone(&module));
        let mut processor = starplayer::it::ItProcessor::try_with_quirks_and_voice_capacity(module,8000,QuirkSelection::FromDialect,live.voices.capacity()).unwrap();
        let mut output = Vec::new();
        for index in 0..20 {
            live.frame=Frame(17+index*160);
            if audition { for note in [60,64,67] { live.on(64,note,u16::MAX); } live.step(live.frame.0+1); }
            let row = [60+(index%8) as u8,1,255,0,0];
            let mut context = TickContext::new(live.frame,&mut live.voices,&mut live.channels,RowClock::new(6),SongPosition::default(),125);
            let _ = processor.row(&mut context,RowRef {order:0,pattern:0,row:0,bytes:&row});
            let voice=live.channels.foreground(ChannelId(0)).unwrap(); output.push(live.voices.get(voice).unwrap().params);
        }
        output
    }
    let native=run(false); assert!(native.windows(2).any(|pair|pair[0].volume!=pair[1].volume || pair[0].pan!=pair[1].pan)); assert_eq!(run(true),native);
}

#[test]
fn cc7_is_latched_for_each_note_and_never_rewrites_existing_native_output() {
    for format in FORMATS {
        let module=module(format,true,NewNoteAction::Continue,1);let mut live=Harness::new(Arc::clone(&module));let mut reference=Harness::new(module);
        let voice=live.on(64,60,u16::MAX);let reference_voice=reference.on(64,60,u16::MAX);
        live.event(64,Event::Controller {number:7,value:U0F16::ZERO});
        for tick in 1..5 {live.step(17+tick*160);reference.step(17+tick*160);assert_eq!(live.voices.get(voice).unwrap().params.volume,reference.voices.get(reference_voice).unwrap().params.volume);}
        let quiet=live.on(64,67,u16::MAX);assert_eq!(live.voices.get(quiet).unwrap().params.volume,U0F16::ZERO);
    }
}

#[cfg(feature = "note-overlap")]
#[test]
fn bypass_clears_only_custom_live_backgrounds_and_cc120_is_lane_scoped() {
    let mut live=Harness::new(module(ModuleFormat::It,false,NewNoteAction::Continue,1));
    let native=live.on(64,60,u16::MAX);live.frame=Frame(18);let custom=live.on(64,64,u16::MAX);
    live.overlap(true,true);live.frame=Frame(19);let foreground=live.on(64,67,u16::MAX);let other=live.on(65,60,u16::MAX);
    assert!(!live.channels.playback().unwrap().get(native,&live.voices).unwrap().custom_detached);
    assert!(live.channels.playback().unwrap().get(custom,&live.voices).unwrap().custom_detached);
    let scope=live.channels.playback_mut().unwrap().set_note_overlap_enabled(Default::default(),false).unwrap().unwrap();
    live.channels.clear_scope(scope,&mut live.voices);let mut context=EngineContext::new(live.frame,&mut live.voices,&mut live.channels,&mut live.clock);live.source.clear_scope(scope,&mut context);
    assert!(live.retiring(custom));assert!(!live.retiring(native));assert!(!live.retiring(foreground));assert!(!live.retiring(other));
    live.event(64,Event::Controller {number:120,value:U0F16::ZERO});assert!(live.retiring(native));assert!(live.retiring(foreground));assert!(!live.retiring(other));
    let fresh=live.on(64,69,u16::MAX);assert!(!live.retiring(fresh));
}

#[test]
fn retired_and_stale_public_runtime_operations_cannot_advance_random_articulation() {
    use starplayer::engine::instrument::{NoteParams,live::RackRuntime};
    for format in FORMATS {
        let module=waveform_module(format,true,NewNoteAction::Continue,1,false,0,AutoVibratoWaveform::Random);
        let mut reference=Harness::new(Arc::clone(&module)); let mut disturbed=Harness::new(Arc::clone(&module));
        let make_runtime=|| -> Box<dyn RackRuntime> { if format==ModuleFormat::Xm {Box::new(starplayer::xm::processor::live::XmLiveRuntime::try_new(reference.voices.capacity(),8000).unwrap())} else {Box::new(starplayer::it::processor::live::ItLiveRuntime::try_new(reference.voices.capacity(),8000).unwrap())} };
        let mut runtimes=[make_runtime(),make_runtime()];
        let params=NoteParams {note:Note::new(60),velocity:U0F16::MAX,raw_velocity:U0F16::MAX,pan_override:None};
        for (index,harness) in [&mut reference,&mut disturbed].into_iter().enumerate() {
            let mut context=EngineContext::new(Frame(1),&mut harness.voices,&mut harness.channels,&mut harness.clock);
            let voice=runtimes[index].note_on(&module,InstrumentId(0),ChannelId(64),params,&mut context).unwrap();
            context.channels.retire_voice(voice,context.voices);
            if index==1 { for _ in 0..17 { runtimes[index].step(&module,voice,&mut context); runtimes[index].bend(&module,voice,params.note,100,&mut context); runtimes[index].release(&module,voice,&mut context); } }
            context.voices.release(voice);
            if index==1 { for _ in 0..17 { runtimes[index].step(&module,voice,&mut context); runtimes[index].bend(&module,voice,params.note,100,&mut context); runtimes[index].release(&module,voice,&mut context); } }
            runtimes[index].note_on(&module,InstrumentId(0),ChannelId(64),params,&mut context).unwrap();
        }
        for tick in 0..12 {
            for (index,harness) in [&mut reference,&mut disturbed].into_iter().enumerate() { let mut context=EngineContext::new(Frame(160*tick+1),&mut harness.voices,&mut harness.channels,&mut harness.clock); let voice=context.channels.foreground(ChannelId(64)).unwrap(); runtimes[index].step(&module,voice,&mut context); }
            let expected=reference.voices.get(reference.channels.foreground(ChannelId(64)).unwrap()).unwrap().params;
            let actual=disturbed.voices.get(disturbed.channels.foreground(ChannelId(64)).unwrap()).unwrap().params;
            assert_eq!((actual.step,actual.volume,actual.pan,actual.filter),(expected.step,expected.volume,expected.pan,expected.filter),"{format:?} tick{tick}");
        }
    }
}

#[test]
fn prepared_simple_sample_bend_preserves_cents_and_pcm_rate_scaling() {
    for scale in [0,2] {
        let mut live=Harness::new(detailed_module(ModuleFormat::S3m,true,NewNoteAction::Cut,1,false,scale));
        live.event(64,Event::PitchBend(starplayer::core::I1F15::MAX));
        let note=Note::with_cents(60,50);
        live.event(64,Event::NoteOn {note,velocity:U0F16::MAX});
        let bent=live.channels.foreground(ChannelId(64)).unwrap();
        let onset=live.voices.get(bent).unwrap().params.step;
        live.event(65,Event::NoteOn {note,velocity:U0F16::MAX});
        let centered=live.channels.foreground(ChannelId(65)).unwrap();
        live.event(65,Event::PitchBend(starplayer::core::I1F15::MAX));
        assert_eq!(onset,live.voices.get(centered).unwrap().params.step);
        live.event(64,Event::PitchBend(starplayer::core::I1F15::ZERO));
        live.event(65,Event::PitchBend(starplayer::core::I1F15::ZERO));
        assert_eq!(live.voices.get(bent).unwrap().params.step,live.voices.get(centered).unwrap().params.step);
        assert!(onset>live.voices.get(bent).unwrap().params.step);
    }
}

#[test]
fn capability_uses_native_xm_tuning_for_reference_preview_and_other_map_keys() {
    let mut builder = ModuleBuilder::new();
    let sample = builder.add_sample(&[1000; 128], SampleSpec { relative_note: -80, ..SampleSpec::one_shot("low tuning") }).unwrap();
    let mut instrument = InstrumentDef::from_sample("mapped", sample, U0F16::MAX);
    instrument.note_sample_map.fill(0); instrument.note_sample_map[60] = 1; instrument.note_sample_map[100] = 1;
    builder.add_instrument(instrument).unwrap();
    builder.add_pattern(&[0; 5 * 64], 1, 64).unwrap(); builder.set_orders(&[0]); builder.set_header(ModuleHeader::new(ModuleFormat::Xm, 1));
    let module = Arc::new(builder.build().unwrap());
    let capability = starplayer::live_instrument_capability(&module, InstrumentId(0));
    assert!(capability.playable && capability.one_shot);
    assert!(!capability.preview_playable && !capability.preview_looping);
    let mut live = Harness::new(module);
    live.action(InteractiveAction::PreviewInstrument(InstrumentId(0)));
    assert!(live.channels.foreground(ChannelId(80)).is_none());
    live.on(64, 100, 65_535);
    assert!(live.channels.foreground(ChannelId(64)).is_some());
}


#[test]
fn xm_sample_loop_eligibility_is_independent_of_live_pitch_viability() {
    use starplayer::engine::{TickContext, RowRef, SongPosition, TrackerProcessor};
    use starplayer::core::RowClock;
    for case in 0..3 {
        let mut builder = ModuleBuilder::new();
        let one_shot = builder.add_sample(&[1000; 128], SampleSpec::one_shot("one shot")).unwrap();
        let looping = builder.add_sample(&[2000; 128], SampleSpec::one_shot("looping").with_forward_loop(0, 128)).unwrap();
        let mut instrument = InstrumentDef::from_sample("mapped", one_shot, U0F16::MAX);
        instrument.note_sample_map.fill(0);
        instrument.note_sample_map[0] = if case == 2 { looping.0 + 1 } else { one_shot.0 + 1 };
        if case != 0 { instrument.note_sample_map[60] = if case == 1 { looping.0 + 1 } else { one_shot.0 + 1 }; }
        builder.add_instrument(instrument).unwrap();
        builder.add_pattern(&[1, 1, 0, 0, 0], 1, 1).unwrap(); builder.set_orders(&[0]);
        builder.set_header(ModuleHeader::new(ModuleFormat::Xm, 1));
        let module = Arc::new(builder.build().unwrap());
        let capability = starplayer::live_instrument_capability(&module, InstrumentId(0));
        assert!(capability.one_shot, "native mapped PCM stays overlap-eligible case{case}");
        assert_eq!(capability.looping, case != 0);
        assert_eq!(capability.playable, case != 0);
        assert_eq!(capability.preview_playable, case != 0);
        assert_eq!(capability.preview_looping, case == 1);
        // Native cell1 selects map0 and has a valid native period. The live adapter
        // subtracts12 for input0, which cannot sound; that does not invalidate songPCM.
        let mut voices = VoicePool::new(339); let mut channels = ChannelTable::new(1);
        let mut processor = starplayer::xm::XmProcessor::new(Arc::clone(&module), 8000);
        let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(6), SongPosition::default(), 125);
        let _ = processor.row(&mut context, RowRef { order: 0, pattern: 0, row: 0, bytes: &[1, 1, 0, 0, 0] });
        let song = channels.foreground(ChannelId(0)).expect("native lowest map key starts a song voice");
        assert_ne!(voices.get(song).unwrap().params.step, starplayer::core::Step::ZERO);
        let mut live = Harness::new(module);
        live.event(64, Event::NoteOn { note: Note::new(0), velocity: U0F16::MAX });
        assert!(live.channels.foreground(ChannelId(64)).is_none());
    }
}
