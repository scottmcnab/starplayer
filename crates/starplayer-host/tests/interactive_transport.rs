#![cfg(feature = "interactive-audition")]
use starplayer::core::{AtEnd, InteractiveTransport, PlaybackCapabilities, PlaybackIdentity};
use starplayer::engine::{EngineSettings, MixerMode, PlaybackConfiguration, VoiceCapacity};
use starplayer_host::{AudioSpec, ManualBackend, ManualDriver, Player};

fn settings() -> EngineSettings {
    EngineSettings { sample_rate_hz: 44_100, channel_count: 4, ..EngineSettings::default() }.with_playback(PlaybackConfiguration {
        identity: PlaybackIdentity::default(), capabilities: PlaybackCapabilities::INTERACTIVE_AUDITION, instrument_count: 31,
        capacity: VoiceCapacity { native: 4, custom: 0, live: 2, interactive_transport: true },
    }).unwrap()
}
fn module(short: bool) -> Vec<u8> {
    let mut bytes = vec![0u8; 1084 + 1024 + 256];
    bytes[42..44].copy_from_slice(&128u16.to_be_bytes()); bytes[45] = 64;
    bytes[48..50].copy_from_slice(&128u16.to_be_bytes());
    bytes[950] = 1; bytes[1080..1084].copy_from_slice(b"M.K.");
    let note = starplayer::mod_file::ModCell { period: 428, instrument: 1, effect: 0xE, param: 0x91 };
    bytes[1084..1088].copy_from_slice(&note.to_bytes());
    if short { bytes[1100..1104].copy_from_slice(&starplayer::mod_file::ModCell { period: 0, instrument: 0, effect: 0xB, param: 0 }.to_bytes()); }
    bytes[2108..].fill(100);
    bytes
}
fn open() -> (ManualBackend, Player, ManualDriver) {
    let mut backend = ManualBackend::new();
    let player = Player::open_with_settings(&mut backend, None, AudioSpec::stereo(44_100), MixerMode::DEFAULT, settings()).unwrap();
    let driver = backend.driver().unwrap(); (backend, player, driver)
}
fn render(driver: &ManualDriver, frames: usize) -> Vec<f32> { let mut output = vec![0.0; frames * 2]; driver.render(&mut output); output }

#[test]
fn pause_keeps_voice_and_progress_and_jam_routing_cannot_replace_the_song() {
    let (_, mut player, driver) = open(); player.load(&module(false)).unwrap(); player.play().unwrap(); render(&driver, 256);
    player.pause().unwrap(); let held = render(&driver, 1024);
    let snapshot = *player.telemetry(); let identity = player.playback_identity();
    assert_eq!(snapshot.transport.interactive, Some(InteractiveTransport::Paused));
    assert_eq!(snapshot.transport.song_frame, 256);
    assert!(!snapshot.transport.natural_end);
    assert!(held.iter().any(|sample| *sample != 0.0));
    player.jam(true).unwrap(); render(&driver, 2048); player.jam(false).unwrap(); render(&driver, 128);
    assert_eq!(player.playback_identity(), identity);
    assert_eq!(player.telemetry().transport.song_frame, snapshot.transport.song_frame);
    assert_eq!(player.telemetry().transport.tick, snapshot.transport.tick);
    assert_eq!(player.telemetry().channels[0].active, snapshot.channels[0].active);
    player.resume().unwrap(); render(&driver, 1024);
    assert!(player.is_playing()); assert!(player.song_frame() > 256);
}

#[test]
fn stop_holds_cursor_and_old_effect_retrigger_cannot_resurrect_the_note() {
    let (_, mut player, driver) = open(); player.load(&module(false)).unwrap(); player.play().unwrap(); render(&driver, 256);
    player.clear_and_hold().unwrap(); render(&driver, 1024);
    assert_eq!(player.song_frame(), 256); assert!(!player.natural_end());
    player.play().unwrap(); let output = render(&driver, 2048);
    assert!(output.iter().all(|sample| *sample == 0.0));
    assert!(player.is_playing()); assert_eq!(player.telemetry().voices_active, 0);
}

#[test]
fn held_seek_rebases_at_each_seek_without_dispatching_a_tick() {
    for stopped in [false, true] {
        let (_, mut player, driver) = open(); player.load(&module(false)).unwrap(); player.play().unwrap(); render(&driver, 256);
        if stopped { player.clear_and_hold().unwrap(); } else { player.pause().unwrap(); }
        render(&driver, 1024);
        for row in [3, 5] {
            player.seek_row(row).unwrap(); render(&driver, 1024);
            assert_eq!(player.telemetry().transport.row, row);
            assert_eq!(player.telemetry().transport.tick, 0);
            assert_eq!(player.telemetry().voices_active, 0);
        }
        let held = player.song_frame(); let before_resume = player.source_frame();
        player.resume().unwrap(); render(&driver, 128);
        assert_eq!(player.telemetry().transport.row, 5);
        assert_eq!(player.telemetry().transport.tick, 0);
        assert_eq!(player.song_frame(), held);
        assert_eq!(player.source_frame().0, before_resume.0 + 128);
        render(&driver, 896);
        assert_eq!(player.telemetry().transport.tick, 1);
    }
}

#[test]
fn invalid_held_seeks_do_not_clear_or_move_a_sounding_voice() {
    let (_, mut player, driver) = open(); player.load(&module(false)).unwrap(); player.play().unwrap(); render(&driver, 256); player.pause().unwrap(); render(&driver, 1024);
    let snapshot = *player.telemetry();
    for request in [0, 1, 2] {
        match request { 0 => player.seek_order(u16::MAX).unwrap(), 1 => player.seek_row(u16::MAX).unwrap(), _ => player.seek_frame(u64::MAX).unwrap() }
        render(&driver, 1024);
        assert_eq!(player.telemetry().transport.song_frame, snapshot.transport.song_frame);
    assert_eq!(player.telemetry().transport.tick, snapshot.transport.tick);
        assert_eq!(player.telemetry().channels[0].active, snapshot.channels[0].active);
    }
}

#[test]
fn pause_freezes_end_fade_and_resume_does_not_restart_it() {
    let (_, mut player, driver) = open(); player.load(&module(true)).unwrap(); player.set_at_end(AtEnd::FadeOut).unwrap(); player.set_fade_frames(4096).unwrap(); player.play().unwrap();
    while !player.is_fading() { render(&driver, 128); player.telemetry(); }
    render(&driver, 512); player.pause().unwrap(); render(&driver, 128);
    let held = player.song_frame(); render(&driver, 8192);
    assert!(player.is_fading()); assert!(!player.natural_end()); assert_eq!(player.song_frame(), held);
    player.resume().unwrap(); render(&driver, 512);
    assert!(player.is_fading()); assert!(player.song_frame() >= held);
    render(&driver, 4096);
    assert!(player.natural_end()); assert_eq!(player.interactive_transport(), Some(InteractiveTransport::Stopped)); assert!(!player.is_fading());
    let ended = player.song_frame(); render(&driver, 1024); assert_eq!(player.song_frame(), ended);
    player.play().unwrap(); render(&driver, 128); let debug_snapshot = player.telemetry().transport; assert_eq!(player.song_frame(), 0, "snapshot {debug_snapshot:?}, seek {:?}, playing {}", player.pending_seek(), player.is_playing()); assert!(!player.natural_end()); assert!(!player.is_fading());
}

#[test]
fn explicit_stop_cancels_natural_end_armed_rewind() {
    let (_, mut player, driver) = open(); player.load(&module(true)).unwrap(); player.set_at_end(AtEnd::FadeOut).unwrap(); player.set_fade_frames(256).unwrap(); player.play().unwrap();
    while !player.natural_end() { render(&driver, 128); player.telemetry(); }
    let ended = player.song_frame(); player.clear_and_hold().unwrap(); render(&driver, 128); player.play().unwrap(); render(&driver, 128);
    assert!(player.song_frame() >= ended); assert!(!player.natural_end());
}

#[test]
fn rebuild_preserves_session_pause_and_converts_song_time_to_new_rate() {
    let (mut backend, mut player, driver) = open(); player.load(&module(false)).unwrap(); player.play().unwrap(); render(&driver, 12032); player.pause().unwrap(); render(&driver, 1024);
    let old = player.playback_identity().unwrap(); let held = player.song_frame();
    player.reopen(&mut backend, None, AudioSpec::stereo(96_000)).unwrap();
    let driver = backend.driver().unwrap(); driver.render(&mut [0.0; 4096]);
    let rebuilt = player.playback_identity().unwrap(); assert_eq!(old.session, rebuilt.session); assert!(rebuilt.build.0 > old.build.0);
    assert_eq!(player.interactive_transport(), Some(InteractiveTransport::Paused));
    let converted = held * 96_000 / 44_100;
    assert_eq!(player.song_frame(), converted);
    let before_reload = player.playback_identity().unwrap(); player.load(&module(false)).unwrap();
    assert_ne!(player.playback_identity().unwrap().session, before_reload.session);
}

#[cfg(feature = "note-overlap")]
#[test]
fn overlap_only_runtime_keeps_legacy_play_stop_and_jam() {
    let mut backend = ManualBackend::new();
    let settings = EngineSettings { channel_count: 64, ..EngineSettings::default() }.with_playback(PlaybackConfiguration {
        identity: PlaybackIdentity::default(), capabilities: PlaybackCapabilities::NOTE_OVERLAP, instrument_count: 31,
        capacity: VoiceCapacity { native: 272, custom: 4, live: 0, interactive_transport: false },
    }).unwrap();
    let mut player = Player::open_with_settings(&mut backend, None, AudioSpec::stereo(44_100), MixerMode::DEFAULT, settings).unwrap();
    let driver = backend.driver().unwrap(); player.load(&module(false)).unwrap(); player.play().unwrap(); render(&driver, 256);
    assert!(player.is_playing()); assert_eq!(player.interactive_transport(), None); assert!(player.pause().is_err());
    player.stop().unwrap(); render(&driver, 256); let stopped = player.source_frame(); render(&driver, 1024); assert_eq!(player.source_frame(), stopped);
    player.jam(true).unwrap(); assert!(player.take_event_sender().is_some());
}

#[test]
fn held_mid_fade_rebuild_preserves_fade_and_pending_tick_then_stop_cancels_it() {
    let (mut backend, mut player, driver) = open(); player.load(&module(true)).unwrap(); player.set_at_end(AtEnd::FadeOut).unwrap(); player.set_fade_frames(4096).unwrap(); player.play().unwrap();
    while !player.is_fading() { render(&driver, 128); player.telemetry(); }
    render(&driver, 512); player.pause().unwrap(); render(&driver, 128);
    let old = player.telemetry().transport.sequencer_cursor().unwrap();
    let mut mode = player.mixer_mode(); mode.interpolator = starplayer::core::Interpolator::Cubic;
    player.set_mixer_mode(&mut backend, None, mode).unwrap(); let driver = backend.driver().unwrap(); render(&driver, 256);
    let new = player.telemetry().transport.sequencer_cursor().unwrap();
    assert!(player.is_fading()); assert_eq!(new.row_clock, old.row_clock); assert_eq!(new.remaining_frames, old.remaining_frames); assert_eq!(new.fractional_remainder, old.fractional_remainder);
    assert_eq!(new.song_frame, old.song_frame); assert_eq!(new.pending_jump, old.pending_jump);
    player.resume().unwrap(); render(&driver, 512); assert!(player.is_fading());
    render(&driver, 4096); assert!(player.natural_end());
    player.clear_and_hold().unwrap(); render(&driver, 128); assert!(!player.is_fading()); assert!(!player.natural_end());
}

#[test]
fn commands_from_a_previous_build_cannot_change_the_host_shadow() {
    let (_, mut player, driver) = open(); player.load(&module(false)).unwrap(); let previous = player.playback_identity().unwrap();
    player.load(&module(false)).unwrap(); player.play().unwrap(); render(&driver, 256);
    assert!(player.submit_interactive(starplayer::core::InteractiveCommand { identity: previous, action: starplayer::core::InteractiveAction::Pause }).is_err());
    render(&driver, 128); assert_eq!(player.interactive_transport(), Some(InteractiveTransport::Playing));
}

#[test]
fn seek_clears_song_before_reset_and_preserves_the_permanent_live_slot() {
    use starplayer::core::{ChannelId, Command, Frame, InteractiveAction, InteractiveCommand, Step, U0F16, VoiceParams};
    use starplayer::engine::{Engine, EngineContext, EventSource};
    use starplayer::mixer::{FixedPath, LoopSpan, SampleRegion, StereoI16, VoiceTag};
    struct Live { region: SampleRegion, started: bool }
    impl EventSource for Live {
        fn next_event_frame(&self) -> Option<Frame> { (!self.started).then_some(Frame::ZERO) }
        fn advance_to(&mut self, _frame: Frame) {}
        fn dispatch(&mut self, _frame: Frame, context: &mut EngineContext<'_>) {
            context.channels.trigger(ChannelId(64), context.voices, VoiceTag::default(), self.region, VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT }, 0).unwrap();
            self.started = true;
        }
    }
    let module = starplayer::rt::Arc::new(starplayer::load(&module(false)).unwrap());
    let handles = starplayer_host::SourceHandles::new(AtEnd::Continue);
    let settings = settings();
    let built = starplayer_host::build_source_with_settings(module.clone(), starplayer_host::SeekKind::None, Frame::ZERO, &handles, None, 0, settings).unwrap();
    let region = SampleRegion::looping(module.samples()[0].pcm_offset(), LoopSpan::new(0, 256).unwrap());
    let source = starplayer_host::interactive_sources(built.source, Box::new(Live { region, started: false }));
    let mut engine: Engine<FixedPath, starplayer::dsp::Linear, StereoI16, starplayer::rt::Arc<starplayer::model::Module>> = Engine::try_with_settings(settings).unwrap();
    let mut control = engine.take_control().unwrap(); control.load_module(module).unwrap(); engine.set_source(source);
    let identity = settings.playback_configuration().unwrap().identity;
    control.send(Command::Interactive(InteractiveCommand { identity, action: InteractiveAction::Play })).unwrap(); engine.render(&mut [0; 512]);
    let live = engine.channels().foreground(ChannelId(64)).unwrap(); let song = engine.channels().foreground(ChannelId(0)).unwrap();
    control.send(Command::Interactive(InteractiveCommand { identity, action: InteractiveAction::Pause })).unwrap(); engine.render(&mut [0; 256]);
    handles.seek.request(starplayer_host::SeekRequest { kind: starplayer_host::SeekKind::Row(3), frame: engine.source_frame() }); engine.render(&mut [0; 256]);
    assert_eq!(engine.channels().foreground(ChannelId(64)), Some(live)); assert!(engine.voices().get(live).is_some());
    assert_eq!(engine.channels().foreground(ChannelId(0)), None); assert!(engine.voices().get(song).is_none());
    assert_eq!(engine.interactive_transport(), Some(InteractiveTransport::Paused));
}

#[test]
fn held_mid_row_rebuild_retains_cursor_and_next_tick_at_both_rates() {
    for stopped in [false, true] {
        for rate in [44_100, 96_000] {
            let (mut backend, mut player, driver) = open();
            player.load(&module(false)).unwrap(); player.play().unwrap(); render(&driver, 12032);
            if stopped { player.clear_and_hold().unwrap(); } else { player.pause().unwrap(); }
            render(&driver, 128);
            let old = player.telemetry().transport.sequencer_cursor().unwrap();
            assert!(old.row_clock.tick_in_row > 0);
            let converted = old.converted(rate);
            player.reopen(&mut backend, None, AudioSpec::stereo(rate)).unwrap();
            let driver = backend.driver().unwrap(); render(&driver, 128);
            let restored = player.telemetry().transport.sequencer_cursor().unwrap();
            assert_eq!(restored.row_clock, converted.row_clock);
            assert_eq!(restored.remaining_frames, converted.remaining_frames);
            assert_eq!(restored.fractional_remainder, converted.fractional_remainder);
            assert_eq!(restored.arrival, converted.arrival);
            assert_eq!(restored.detector_state, converted.detector_state);
            let deadline = player.source_frame().0 + restored.remaining_frames;
            player.resume().unwrap();
            while player.source_frame().0 <= deadline { render(&driver, 128); player.telemetry(); }
            let next = player.telemetry().transport.sequencer_cursor().unwrap();
            assert_eq!(next.captured_frame.0, deadline);
            assert_ne!(next.row_clock.tick_in_row, old.row_clock.tick_in_row);
        }
    }
}

fn midi_file() -> Vec<u8> {
    let track = [0, 0xC0, 0, 0, 0xB0, 7, 32, 0, 0xB0, 10, 100, 0, 0xE0, 0, 96, 0, 0xB0, 64, 127,
        0, 0x90, 60, 100, 96, 0x90, 64, 100, 96, 0xFF, 0x2F, 0];
    let mut bytes = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk".to_vec();
    bytes.extend_from_slice(&(track.len() as u32).to_be_bytes()); bytes.extend_from_slice(&track); bytes
}

#[test]
fn smf_rebuild_retains_file_cursor_controllers_and_future_event_without_replay() {
    for stopped in [false, true] {
        for rate in [44_100, 96_000] {
            let (mut backend, mut player, driver) = open();
            let instruments = starplayer::rt::Arc::new(starplayer::load(&module(false)).unwrap());
            player.load_smf(&midi_file(), instruments).unwrap(); player.play().unwrap();
            let initial = render(&driver, 8192); assert!(initial.iter().any(|sample| *sample != 0.0));
            assert!(player.telemetry().channels[0].active); assert_eq!(player.telemetry().live.active, 0);
            if stopped { player.clear_and_hold().unwrap(); } else { player.pause().unwrap(); }
            render(&driver, 128);
            let old = player.telemetry().transport.smf_state().unwrap();
            assert_eq!(old.cursor.event_index, 6); assert_eq!(old.cursor.song_frame, 8192);
            assert_ne!(old.channels[0].volume, starplayer::core::U0F16::MAX);
            assert!(old.channels[0].pan.is_some()); assert_eq!(old.channels[0].sustain, !stopped);
            player.reopen(&mut backend, None, AudioSpec::stereo(rate)).unwrap();
            let driver = backend.driver().unwrap(); let held = render(&driver, 128);
            assert!(held.iter().all(|sample| *sample == 0.0));
            let restored = player.telemetry().transport.smf_state().unwrap();
            let converted = old.cursor.converted(rate);
            assert_eq!(restored.channels, old.channels);
            assert_eq!(restored.cursor.event_index, converted.event_index);
            assert_eq!(restored.cursor.song_frame, converted.song_frame);
            assert_eq!(restored.cursor.remaining_frames, converted.remaining_frames);
            assert_eq!(player.interactive_transport(), Some(if stopped { InteractiveTransport::Stopped } else { InteractiveTransport::Paused }));
            let deadline = player.source_frame().0 + converted.remaining_frames.unwrap();
            player.resume().unwrap();
            while player.source_frame().0 + 128 <= deadline {
                assert!(render(&driver, 128).iter().all(|sample| *sample == 0.0)); player.telemetry();
            }
            let sound = render(&driver, 2048); assert!(sound.iter().any(|sample| *sample != 0.0));
            assert_eq!(player.telemetry().transport.smf_state().unwrap().cursor.event_index, 7);
        }
    }
}

#[test]
fn permanent_live_slot_works_before_play_after_stop_and_across_routing_without_reload() {
    use starplayer::core::{Event, InstrumentId, InteractiveAction, LiveDestination, Note, U0F16};
    let (mut backend, mut player, driver) = open(); player.load(&module(false)).unwrap(); render(&driver,128);
    let identity = player.playback_identity();
    player.interactive(InteractiveAction::PreviewInstrument(InstrumentId(0))).unwrap();
    assert!(render(&driver,1024).iter().any(|sample| *sample != 0.0)); assert_eq!(player.song_frame(),0);
    player.interactive(InteractiveAction::StopPreview).unwrap(); let stopped = render(&driver,1024); assert!(stopped[128..].iter().all(|sample| *sample == 0.0)); assert_eq!(player.telemetry().voices_active,0);
    player.interactive(InteractiveAction::SetLiveDestination(LiveDestination::Selected(InstrumentId(0)))).unwrap();
    player.send_event(15,Event::NoteOn {note:Note::new(60),velocity:U0F16::MAX}).unwrap();
    assert!(render(&driver,4096).iter().any(|sample| *sample != 0.0));
    assert_eq!(player.playback_identity(),identity); assert_eq!(player.song_frame(),0);
    player.clear_and_hold().unwrap(); render(&driver,128);
    player.send_event(0,Event::NoteOn {note:Note::new(60),velocity:U0F16::MAX}).unwrap();
    assert!(render(&driver,8192).iter().any(|sample| *sample != 0.0));
    let spec = player.spec(); player.reopen(&mut backend,None,spec).unwrap(); let rebuilt = backend.driver().unwrap(); render(&rebuilt,128);
    assert_ne!(player.playback_identity(),identity); assert_eq!(player.telemetry().voices_active,0);
    player.send_event(0,Event::NoteOn {note:Note::new(60),velocity:U0F16::MAX}).unwrap();
    assert!(render(&rebuilt,4096).iter().any(|sample| *sample != 0.0),"routing survives rebuild with empty held generations");
}

#[test]
fn preview_after_natural_end_is_audible_without_restarting_song() {
    let (_,mut player,driver)=open(); player.load(&module(true)).unwrap();
    player.set_at_end(AtEnd::FadeOut).unwrap();player.set_fade_frames(256).unwrap();player.play().unwrap();
    for _ in 0..256 {render(&driver,128);player.telemetry();if player.natural_end(){break;}}
    assert!(player.natural_end());render(&driver,1024);player.telemetry();let ended=player.song_frame();
    player.interactive(starplayer::core::InteractiveAction::PreviewInstrument(starplayer::core::InstrumentId(0))).unwrap();
    assert!(render(&driver,2048).iter().any(|sample|*sample!=0.0));
    player.telemetry();assert!(player.natural_end());assert_eq!(player.song_frame(),ended);
}

#[test]
fn no08_pause_after_natural_end_keeps_next_play_rewind() {
    for at_end in [AtEnd::Stop, AtEnd::FadeOut] {
        for resume in [false, true] {
            let (_, mut player, driver) = open(); player.load(&module(true)).unwrap();
            if at_end == AtEnd::Stop {
                let mut ending = module(true);
                ending[1100..1104].copy_from_slice(&starplayer::mod_file::ModCell { period: 0, instrument: 0, effect: 0xF, param: 0 }.to_bytes());
                player.load(&ending).unwrap();
            }
            player.set_at_end(at_end).unwrap(); player.set_fade_frames(256).unwrap(); player.play().unwrap();
            for _ in 0..1000 { render(&driver, 128); player.telemetry(); if player.natural_end() { break; } }
            assert!(player.natural_end());
            player.pause().unwrap(); render(&driver, 128);
            if resume { player.resume().unwrap(); } else { player.play().unwrap(); }
            render(&driver, 128);
            assert_eq!(player.song_frame(), 0);
            assert!(!player.natural_end());
            render(&driver, 8 * 128);
            assert!(player.is_playing()); assert!(!player.natural_end());
        }
    }
}

#[test]
fn no08_musically_invalid_seeks_preserve_ringing_inserts_and_voices() {
    use starplayer::dsp::{InsertKind, effects::delay::{DELAY_TIME_PARAM, DELAY_FEEDBACK_PARAM, DELAY_MIX_PARAM}};
    use starplayer::engine::InsertTarget;
    fn run(request: Option<u8>) -> (Vec<f32>, starplayer::telemetry::Snapshot) {
        let (_, mut player, driver) = open(); player.load(&module(false)).unwrap();
        for target in [InsertTarget::Master, InsertTarget::Channel(starplayer::core::ChannelId(0))] {
            player.install_insert(target, 0, InsertKind::Delay).unwrap();
            player.set_insert_param(target, 0, DELAY_TIME_PARAM, 10).unwrap();
            player.set_insert_param(target, 0, DELAY_FEEDBACK_PARAM, 75).unwrap();
            player.set_insert_param(target, 0, DELAY_MIX_PARAM, 70).unwrap();
        }
        player.play().unwrap(); render(&driver, 2048); player.telemetry(); player.pause().unwrap(); render(&driver, 128); player.telemetry();
        player.interactive(starplayer::core::InteractiveAction::PreviewInstrument(starplayer::core::InstrumentId(0))).unwrap(); render(&driver, 1024); player.telemetry();
        match request { Some(0) => player.seek_order(999).unwrap(), Some(1) => player.seek_row(999).unwrap(), Some(2) => player.seek_frame(99999999).unwrap(), Some(_) => player.seek_row(3).unwrap(), None => () }
        let output = render(&driver, 2048); let snapshot = *player.telemetry(); (output, snapshot)
    }
    let expected = run(None);
    assert!(expected.0.iter().any(|sample| *sample != 0.0));
    for kind in 0..3 { let actual = run(Some(kind)); assert!(actual.0 == expected.0, "invalid kind {kind} changed DSP output"); assert_eq!(actual.1.channels[0].active, expected.1.channels[0].active); assert_eq!(actual.1.live.active, expected.1.live.active); }
    let valid = run(Some(3)); assert_ne!(valid.0, expected.0); assert_eq!(valid.1.live.active, expected.1.live.active); assert!(!valid.1.channels[0].active);
}

#[test]
fn no08_retirement_deferred_fresh_load_starts_stopped_at_zero_with_one_initial_tick() {
    let (_, mut player, driver) = open();
    for _ in 0..12 { player.load(&module(false)).unwrap(); }
    render(&driver, 8192);
    assert!(player.pending_control_commands() > 0, "test must reach actual retirement pressure");
    player.collect_garbage(); render(&driver, 128);
    assert_eq!(player.song_frame(), 0);
    player.collect_garbage(); render(&driver, 128);
    assert_eq!(player.song_frame(), 0);
    player.play().unwrap(); render(&driver, 128);
    let snapshot = player.telemetry();
    assert_eq!(snapshot.transport.song_frame, 0);
    assert_eq!(snapshot.transport.tick, 0);
}
