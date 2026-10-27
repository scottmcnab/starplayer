#![cfg(feature = "note-overlap")]
use starplayer::core::{InstrumentId, InteractiveAction, InteractiveCommand, PlaybackCapabilities, PlaybackIdentity, U0F16};
use starplayer::engine::{EngineSettings, MixerMode, PlaybackConfiguration, VoiceCapacity};
use starplayer::model::{InstrumentDef, Module, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec};
use starplayer::rt::Arc;
use starplayer_host::{AudioSpec, ManualBackend, ManualDriver, Player};

fn settings() -> EngineSettings {
    EngineSettings { channel_count: 4, ..EngineSettings::default() }.with_playback(PlaybackConfiguration {
        identity: PlaybackIdentity::default(), capabilities: if cfg!(feature = "interactive-audition") { PlaybackCapabilities::all() } else { PlaybackCapabilities::NOTE_OVERLAP }, instrument_count: 0,
        capacity: VoiceCapacity { native: 4, custom: 4, live: if cfg!(feature = "interactive-audition") { 2 } else { 0 }, interactive_transport: cfg!(feature = "interactive-audition") },
    }).unwrap()
}
fn module(instruments: usize) -> Arc<Module> {
    let mut builder = ModuleBuilder::new();
    for _ in 0..instruments {
        let sample = builder.add_sample(&vec![1000; 32_768], SampleSpec::one_shot("one-shot")).unwrap();
        builder.add_instrument(InstrumentDef::from_sample("instrument", sample, U0F16::MAX)).unwrap();
    }
    let mut pattern = Vec::new();
    for row in 0..64 {
        pattern.extend(starplayer::mod_file::ModCell { period: if row % 2 == 0 { 428 } else { 285 }, instrument: 1, effect: 0, param: 0 }.to_bytes());
        pattern.extend([0; 12]);
    }
    builder.add_pattern(&pattern, 64, 4).unwrap(); builder.set_orders(&[0, starplayer::model::ORDER_END]);
    let mut header = ModuleHeader::new(ModuleFormat::Mod, 4); header.initial_speed = 1; builder.set_header(header);
    Arc::new(builder.build().unwrap())
}
fn render(driver: &ManualDriver, frames: usize) { driver.render(&mut vec![0.0; frames * 2]); }
fn check(player: &mut Player, instrument: u16) -> Result<(), starplayer_host::HostError> { player.interactive(InteractiveAction::SetLetNotesRing { instrument: InstrumentId(instrument), enabled: true }) }

#[test]
fn actual_module_bounds_replace_count_zero_preparation_and_reject_invalid_and_stale_ids() {
    let mut backend = ManualBackend::new();
    let mut player = Player::open_with_settings(&mut backend, None, AudioSpec::stereo(8000), MixerMode::DEFAULT, settings()).unwrap();
    let driver = backend.driver().unwrap();
    assert!(check(&mut player, 0).is_err());
    player.load_module(module(2)).unwrap(); let first = player.playback_identity().unwrap();
    assert!(check(&mut player, 1).is_ok()); assert!(check(&mut player, 2).is_err());
    player.interactive(InteractiveAction::SetNoteOverlapEnabled(true)).unwrap(); check(&mut player, 0).unwrap();
    player.play().unwrap(); render(&driver, 1024);
    assert!(player.telemetry().voices_active >= 2, "actual table records identity and enables overlap despite pre-load count 0");
    player.load_module(module(5)).unwrap();
    assert!(check(&mut player, 4).is_ok()); assert!(check(&mut player, 5).is_err());
    assert!(player.submit_interactive(InteractiveCommand { identity: first, action: InteractiveAction::SetNoteOverlapEnabled(true) }).is_err());
    player.play().unwrap(); render(&driver, 1024);
    assert_eq!(player.telemetry().voices_active, 1, "explicit load resets global and selections");
    check(&mut player, 0).unwrap(); player.interactive(InteractiveAction::SetNoteOverlapEnabled(true)).unwrap(); render(&driver, 1024);
    assert!(player.telemetry().voices_active >= 2);
    player.load_module(module(1)).unwrap(); assert!(check(&mut player, 1).is_err());
    player.play().unwrap(); render(&driver, 1024); assert_eq!(player.telemetry().voices_active, 1);
}

#[test]
fn same_session_rebuild_retains_actual_bounds_and_selections_after_count_zero_open() {
    let mut backend = ManualBackend::new();
    let mut player = Player::open_with_settings(&mut backend, None, AudioSpec::stereo(8000), MixerMode::DEFAULT, settings()).unwrap();
    let driver = backend.driver().unwrap(); player.load_module(module(2)).unwrap();
    check(&mut player, 0).unwrap(); player.interactive(InteractiveAction::SetNoteOverlapEnabled(true)).unwrap();
    player.play().unwrap(); render(&driver, 1024); let before = player.playback_identity().unwrap();
    player.reopen(&mut backend, None, AudioSpec::stereo(8000)).unwrap();
    let rebuilt = player.playback_identity().unwrap(); assert_eq!(before.session, rebuilt.session); assert!(rebuilt.build.0 > before.build.0);
    assert!(check(&mut player, 1).is_ok()); assert!(check(&mut player, 2).is_err());
    // Legacy reopen intentionally starts stopped; interactive rebuild keeps its mode.
    player.play().unwrap();
    let driver = backend.driver().unwrap(); render(&driver, 1024);
    assert!(player.telemetry().voices_active >= 2, "replayed selections/global remain effective");
}

#[test]
fn smf_instrument_modules_install_their_actual_instrument_bounds() {
    const SMF: &[u8] = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk\0\0\0\x0C\0\x90\x3C\x7F\x60\x80\x3C\0\0\xFF\x2F\0";
    let mut backend = ManualBackend::new();
    let mut player = Player::open_with_settings(&mut backend, None, AudioSpec::stereo(8000), MixerMode::DEFAULT, settings()).unwrap();
    let driver = backend.driver().unwrap();
    player.load_smf(SMF, module(2)).unwrap(); assert!(check(&mut player, 1).is_ok()); assert!(check(&mut player, 2).is_err()); render(&driver, 128);
    player.load_smf(SMF, module(4)).unwrap(); assert!(check(&mut player, 3).is_ok()); assert!(check(&mut player, 4).is_err()); render(&driver, 128);
}

#[cfg(feature = "interactive-audition")]
mod cleanup {
    use super::*;
    use starplayer::core::{AtEnd, ChannelId, Command, Frame, Step, VoiceParams};
    use starplayer::engine::{Engine, EngineContext, EventSource, SourceMux, VoiceDomain, VoiceLifecycle};
    use starplayer::mixer::{FixedPath, SampleRegion, StereoI16, VoiceTag};
    use starplayer_host::{SeekKind, SeekRequest, SourceHandles, build_source_with_settings};
    type TestEngine = Engine<FixedPath, starplayer::dsp::Linear, StereoI16, Arc<Module>>;
    struct Live { pending: bool, region: SampleRegion }
    impl EventSource for Live {
        fn next_event_frame(&self) -> Option<Frame> { self.pending.then_some(Frame(7)) }
        fn advance_to(&mut self, _: Frame) {}
        fn dispatch(&mut self, _: Frame, context: &mut EngineContext<'_>) {
            if self.pending {
                context.channels.trigger(ChannelId(64), context.voices, VoiceTag { instrument: 2, sample: 2, ..Default::default() }, self.region, VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT }, 0);
                self.pending = false;
            }
        }
    }
    fn setup(interactive: bool) -> (TestEngine, starplayer::engine::EngineHandle<Arc<Module>>, SourceHandles) {
        let module = module(2);
        let mut settings = settings(); settings.sample_rate_hz = 8000;
        if !interactive {
            settings.playback.as_mut().unwrap().capabilities = PlaybackCapabilities::NOTE_OVERLAP;
            settings.playback.as_mut().unwrap().capacity.live = 0;
            settings.playback.as_mut().unwrap().capacity.interactive_transport = false;
            settings.voice_capacity = settings.playback.unwrap().capacity.physical();
        }
        settings.set_instrument_count(2);
        let handles = SourceHandles::new(AtEnd::Stop);
        let built = build_source_with_settings(Arc::clone(&module), SeekKind::None, Frame::ZERO, &handles, None, 0, settings).unwrap();
        let mut mux = SourceMux::new(2); mux.insert(built.source).ok().unwrap();
        if interactive {
            let sample = &module.samples()[1];
            mux.insert(Box::new(Live { pending: true, region: SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames()) })).ok().unwrap();
        }
        let mut engine = TestEngine::with_settings(settings); engine.set_source(Box::new(mux));
        engine.restore_selections(Default::default(), true, &[true, false]);
        let mut control = engine.take_control().unwrap(); control.load_module(module).ok().unwrap();
        if interactive { control.send(Command::Interactive(InteractiveCommand { identity: Default::default(), action: InteractiveAction::Play })).ok().unwrap(); }
        engine.render(&mut [0; 768]);
        assert!(engine.channels().playback().unwrap().active_count(VoiceDomain::Custom, engine.voices()) != 0);
        (engine, control, handles)
    }
    fn command(control: &mut starplayer::engine::EngineHandle<Arc<Module>>, action: InteractiveAction) { control.send(Command::Interactive(InteractiveCommand { identity: Default::default(), action })).ok().unwrap(); }
    fn live(engine: &TestEngine) -> usize { engine.channels().playback().unwrap().active_count(VoiceDomain::Live, engine.voices()) }
    #[test]
    fn bypass_stop_and_all_sound_off_clear_exactly_their_owned_scopes() {
        let (mut engine, mut control, _) = setup(true);
        command(&mut control, InteractiveAction::SetNoteOverlapEnabled(false)); engine.render(&mut [0; 256]);
        assert_eq!(engine.channels().playback().unwrap().active_count(VoiceDomain::Custom, engine.voices()), 0);
        assert_eq!(engine.channels().playback().unwrap().let_notes_ring(InstrumentId(0)), Some(true));
        assert_eq!(live(&engine), 1); assert!(engine.channels().foreground(ChannelId(0)).is_some());
        command(&mut control, InteractiveAction::Stop); engine.render(&mut [0; 256]);
        assert_eq!(engine.voices().voices_active(), 0);
        let (mut engine, mut control, _) = setup(true);
        command(&mut control, InteractiveAction::Pause); command(&mut control, InteractiveAction::AllSoundOff); engine.render(&mut [0; 256]);
        assert_eq!(engine.voices().voices_active(), 0); assert_eq!(engine.interactive_transport(), Some(starplayer::core::InteractiveTransport::Paused));
    }
    #[test]
    fn interactive_seek_retires_song_tails_preserves_live_and_invalid_seek_preserves_both() {
        let (mut engine, mut control, handles) = setup(true);
        command(&mut control, InteractiveAction::Pause); engine.render(&mut [0; 256]);
        let custom = engine.channels().playback().unwrap().active_count(VoiceDomain::Custom, engine.voices());
        handles.seek.request(SeekRequest { kind: SeekKind::Row(u16::MAX), frame: engine.source_frame() }); engine.render(&mut [0; 256]);
        assert_eq!(engine.channels().playback().unwrap().active_count(VoiceDomain::Custom, engine.voices()), custom); assert_eq!(live(&engine), 1);
        handles.seek.request(SeekRequest { kind: SeekKind::Row(10), frame: engine.source_frame() }); engine.render(&mut [0; 256]);
        assert_eq!(engine.channels().playback().unwrap().active_count(VoiceDomain::Custom, engine.voices()), 0);
        assert_eq!(engine.channels().playback().unwrap().active_count(VoiceDomain::Native, engine.voices()), 0); assert_eq!(live(&engine), 1);
    }
    #[test]
    fn legacy_seek_keeps_sounding_custom_tail_ownership_until_bypass() {
        let (mut engine, mut control, handles) = setup(false);
        let tail = engine.channels().playback().unwrap().iter(engine.voices()).find(|owned| owned.domain == VoiceDomain::Custom && owned.lifecycle == VoiceLifecycle::Background).unwrap().voice;
        handles.seek.request(SeekRequest { kind: SeekKind::Row(10), frame: engine.source_frame() }); engine.render(&mut [0; 256]);
        assert!(engine.channels().playback().unwrap().get(tail, engine.voices()).is_some(), "legacy processor reset never discards custom ownership");
        command(&mut control, InteractiveAction::SetNoteOverlapEnabled(false)); engine.render(&mut [0; 256]);
        assert!(engine.voices().get(tail).is_none());
    }
}
