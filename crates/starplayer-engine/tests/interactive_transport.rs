#![cfg(feature = "interactive-audition")]

use std::sync::{Arc, atomic::{AtomicU64, AtomicUsize, Ordering}};
use starplayer_core::{AtEnd, ChannelId, Command, ExactFixedPoint, Frame, FrameClock, InteractiveAction, InteractiveCommand, InteractiveTransport, PlaybackCapabilities, PlaybackIdentity, Step, U0F16, VoiceParams};
use starplayer_dsp::Linear;
use starplayer_engine::{ChannelTable, ClearScope, Engine, EngineContext, EngineSettings, EventSource, PatternSequencer, PlaybackConfiguration, RowRef, SequencerSettings, SourceMux, TickContext, TickOutcome, TrackerProcessor, VoiceCapacity, VoiceDomain, VoiceLifecycle};
use starplayer_engine::demo::DemoPatternData;
use starplayer_mixer::{FixedPath, LoopSpan, SampleRegion, StereoI16, VoicePool, VoiceTag, append_guarded_sample};

type TestEngine = Engine<FixedPath, Linear, StereoI16>;
const IDENTITY: PlaybackIdentity = PlaybackIdentity { session: starplayer_core::SessionId(1), build: starplayer_core::BuildGeneration(2) };
struct Log { entries: [AtomicU64; 512], length: AtomicUsize }
impl Log {
    fn new() -> Arc<Self> { Arc::new(Self { entries: [const { AtomicU64::new(0) }; 512], length: AtomicUsize::new(0) }) }
    fn push(&self, frame: Frame, kind: u64) { let index = self.length.fetch_add(1, Ordering::Relaxed); if let Some(entry) = self.entries.get(index) { entry.store(frame.0 * 10 + kind, Ordering::Relaxed); } }
    fn values(&self) -> Vec<u64> { self.entries.iter().take(self.length.load(Ordering::Relaxed).min(512)).map(|entry| entry.load(Ordering::Relaxed)).collect() }
}
struct Processor { log: Arc<Log>, region: SampleRegion }
impl TrackerProcessor for Processor {
    fn reset(&mut self) {}
    fn row(&mut self, context: &mut TickContext<'_>, _row: RowRef<'_>) -> TickOutcome {
        self.log.push(context.frame, 1);
        context.channels.trigger(ChannelId(0), context.voices, VoiceTag::default(), self.region, VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT }, 0);
        context.outcome()
    }
    fn tick(&mut self, context: &mut TickContext<'_>) -> TickOutcome { self.log.push(context.frame, 1); context.outcome() }
    fn control_step(&mut self, context: &mut TickContext<'_>) { self.log.push(context.frame, 2); }
}
struct Live { log: Arc<Log>, events: Vec<Frame>, next: usize, region: SampleRegion }
impl EventSource for Live {
    fn next_event_frame(&self) -> Option<Frame> { self.events.get(self.next).copied() }
    fn advance_to(&mut self, _frame: Frame) {}
    fn dispatch(&mut self, frame: Frame, context: &mut EngineContext<'_>) {
        while self.next_event_frame().is_some_and(|next| next <= frame) {
            self.log.push(frame, 3); self.next += 1;
            context.channels.trigger(ChannelId(64), context.voices, VoiceTag::default(), self.region, VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT }, 0);
        }
    }
    fn post_source_articulation(&mut self, frame: Frame, _context: &mut EngineContext<'_>) { self.log.push(frame, 4); }
}
fn settings() -> EngineSettings {
    EngineSettings { sample_rate_hz: 44_100, channel_count: 1, ..EngineSettings::default() }.with_playback(PlaybackConfiguration {
        identity: IDENTITY, capabilities: PlaybackCapabilities::INTERACTIVE_AUDITION, instrument_count: 1,
        capacity: VoiceCapacity { native: 1, custom: 0, live: 2, interactive_transport: true },
    }).unwrap()
}
fn setup(events: Vec<Frame>, rows: u16, speed: u8) -> (TestEngine, starplayer_engine::EngineHandle<()>, Arc<Log>) {
    let log = Log::new();
    let mut pcm = Vec::new();
    let region = append_guarded_sample(&mut pcm, &[4000; 256], LoopSpan::new(0, 256));
    let mut sequencer = PatternSequencer::new(ExactFixedPoint, DemoPatternData::new(1, rows, 1), Processor { log: log.clone(), region }, SequencerSettings { sample_rate_hz: 44_100, initial_tempo_bpm: 130, initial_speed: speed, ..SequencerSettings::default() });
    sequencer.set_at_end(AtEnd::Stop);
    let mut mux = SourceMux::new(2);
    assert!(mux.insert(Box::new(sequencer)).is_ok());
    assert!(mux.insert(Box::new(Live { log: log.clone(), events, next: 0, region })).is_ok());
    let mut engine = TestEngine::try_with_settings(settings()).unwrap();
    engine.set_pcm(pcm); engine.set_source(Box::new(mux));
    let handle = engine.take_control().unwrap();
    (engine, handle, log)
}
fn command(handle: &mut starplayer_engine::EngineHandle<()>, action: InteractiveAction) { handle.send(Command::Interactive(InteractiveCommand { identity: IDENTITY, action })).unwrap(); }
fn render(engine: &mut TestEngine, frames: usize, block: usize) -> Vec<i16> { let mut output = vec![0; frames * 2]; for chunk in output.chunks_mut(block * 2) { engine.render(chunk); } output }

#[test]
fn pause_holds_sequence_but_steps_song_before_live_and_live_articulation() {
    let (mut engine, mut handle, log) = setup(vec![Frame(848), Frame(900)], 8, 6);
    command(&mut handle, InteractiveAction::Play); render(&mut engine, 256, 128);
    command(&mut handle, InteractiveAction::Pause); let paused = render(&mut engine, 2560, 3);
    assert!(paused.iter().any(|sample| *sample != 0));
    assert_eq!(engine.frame(), engine.source_frame());
    assert_eq!(engine.interactive_transport(), Some(InteractiveTransport::Paused));
    let values = log.values();
    assert!(values.windows(3).any(|window| window == [8482, 8483, 8484]), "held song, live event, live articulation: {values:?}");
    assert_eq!(values.iter().filter(|value| **value % 10 == 1).count(), 1);
    command(&mut handle, InteractiveAction::Resume); render(&mut engine, 2048, 64);
    let ticks: Vec<_> = log.values().into_iter().filter(|value| value % 10 == 1).collect();
    assert_eq!(ticks, [1, 34081, 42561]);
    assert_eq!(log.values().iter().filter(|value| **value == 34084).count(), 1);
}

#[test]
fn stopped_live_path_runs_before_first_play_and_immediately_after_clear() {
    let (mut engine, mut handle, log) = setup(vec![Frame(1), Frame(257)], 8, 6);
    assert!(!engine.is_playing());
    assert!(render(&mut engine, 256, 1).iter().any(|sample| *sample != 0));
    command(&mut handle, InteractiveAction::Stop);
    assert!(render(&mut engine, 256, 3).iter().any(|sample| *sample != 0));
    assert_eq!(engine.channels().playback().unwrap().active_count(VoiceDomain::Live, engine.voices()), 1);
    assert!(!log.values().iter().any(|value| value % 10 == 1 || value % 10 == 2));
    command(&mut handle, InteractiveAction::AllSoundOff); render(&mut engine, 128, 64);
    assert_eq!(engine.voices().voices_active(), 0);
    assert_eq!(engine.interactive_transport(), Some(InteractiveTransport::Stopped));
}

#[test]
fn fractional_pause_schedule_is_byte_identical_at_every_required_host_size() {
    fn run(block: usize) -> (Vec<i16>, Vec<u64>) {
        let (mut engine, mut handle, log) = setup(vec![Frame(849), Frame(3075), Frame(4999)], 8, 6);
        let mut pcm = Vec::new();
        for (action, frames) in [(InteractiveAction::Play, 256), (InteractiveAction::Pause, 128), (InteractiveAction::Resume, 128), (InteractiveAction::Pause, 2432), (InteractiveAction::Resume, 1024), (InteractiveAction::Stop, 128), (InteractiveAction::Play, 4096)] {
            command(&mut handle, action); pcm.extend(render(&mut engine, frames, block));
        }
        (pcm, log.values())
    }
    let expected = run(128);
    for block in [1, 3, 64, 128, 4096, 8191] { assert_eq!(run(block), expected, "host block {block}"); }
}

#[test]
fn natural_exhaustion_keeps_one_live_step_at_every_fractional_boundary() {
    let (mut engine, mut handle, log) = setup(vec![], 1, 1);
    command(&mut handle, InteractiveAction::Play); render(&mut engine, 5000, 128);
    let actual: Vec<_> = log.values().into_iter().filter(|value| value % 10 == 4).map(|value| value / 10).collect();
    let mut clock = FrameClock::new(ExactFixedPoint, 44_100, Frame::ZERO);
    let mut expected = vec![0];
    while clock.advance_tick(130, 1).0 < 5120 { expected.push(clock.pending_tick_frame().0); }
    assert_eq!(actual, expected);
}

#[test]
fn translating_pending_deadlines_never_resets_fractional_phase() {
    for model in [starplayer_core::TempoModelId::ExactFixedPoint, starplayer_core::TempoModelId::St3Truncating, starplayer_core::TempoModelId::ItModern] {
        let mut uninterrupted = FrameClock::new(model, 44_100, Frame::ZERO);
        let mut held = FrameClock::new(model, 44_100, Frame::ZERO);
        let mut translation = 0;
        for index in 0..10_000 {
            if index % 7 == 0 { held.translate(137); translation += 137; }
            assert_eq!(held.advance_tick(143, 6).0, uninterrupted.advance_tick(143, 6).0 + translation);
            assert_eq!(held.fractional_remainder(), uninterrupted.fractional_remainder());
        }
    }
}

#[test]
fn retirement_capacity_admits_consecutive_input_without_cross_domain_steals() {
    let configuration = settings().playback_configuration().unwrap();
    let mut channels = ChannelTable::try_with_layout(starplayer_engine::LaneLayout::interactive(1).unwrap()).unwrap();
    channels.prepare_playback(configuration).unwrap();
    let mut voices = VoicePool::new(configuration.capacity.physical());
    let region = SampleRegion::one_shot(0, 1000);
    let song = channels.trigger(ChannelId(0), &mut voices, VoiceTag::default(), region, VoiceParams::SILENT, 0).unwrap();
    for lane in [64, 65] { channels.trigger(ChannelId(lane), &mut voices, VoiceTag::default(), region, VoiceParams::SILENT, 0).unwrap(); }
    channels.clear_scope(ClearScope::Live, &mut voices);
    assert_eq!(channels.playback().unwrap().active_count(VoiceDomain::Live, &voices), 0);
    for _ in 0..32 {
        let victim = if channels.playback().unwrap().active_count(VoiceDomain::Live, &voices) >= 2 { channels.playback().unwrap().oldest(VoiceDomain::Live, false, &voices) } else { None };
        assert!(channels.prepare_admission(VoiceDomain::Live, victim, &mut voices));
        channels.detach_foreground(ChannelId(64));
        channels.trigger(ChannelId(64), &mut voices, VoiceTag::default(), region, VoiceParams::SILENT, 0).unwrap();
        assert!(voices.get(song).is_some());
        assert!(channels.playback().unwrap().iter(&voices).filter(|owned| owned.domain == VoiceDomain::Live).count() <= 3);
    }
    assert!(channels.playback().unwrap().iter(&voices).any(|owned| owned.lifecycle == VoiceLifecycle::Retiring));
}

#[cfg(feature = "telemetry")]
#[test]
fn direct_engine_publishes_one_coherent_snapshot_per_cadence_and_held_cursor() {
    let (mut engine, mut handle, _) = setup(vec![Frame(848)], 8, 6);
    let mut reader = engine.telemetry_reader().unwrap();
    command(&mut handle, InteractiveAction::Play); render(&mut engine, 256, 128); reader.read();
    command(&mut handle, InteractiveAction::Pause); render(&mut engine, 128, 128);
    let paused = *reader.read();
    assert_eq!(paused.transport.interactive, Some(InteractiveTransport::Paused));
    assert_eq!(paused.transport.song_frame, 256);
    render(&mut engine, 512, 128);
    let held = *reader.read();
    assert_eq!(held.sequence, paused.sequence + 1);
    assert_eq!(held.transport.song_frame, 256);
    assert_eq!(held.live.active, 1);
    assert!(!held.transport.natural_end);
    command(&mut handle, InteractiveAction::Stop); render(&mut engine, 128, 128);
    let stopped = *reader.read();
    assert_eq!(stopped.transport.interactive, Some(InteractiveTransport::Stopped));
    assert_eq!(stopped.live.active, 0);
    assert!(!stopped.transport.end_reached);
}

struct Tail { level: i32 }
impl starplayer_dsp::Insert<i32> for Tail {
    fn process(&mut self, frames: &mut [starplayer_dsp::Stereo<i32>]) { assert_eq!(frames.len(), 128); for frame in frames { frame.left += self.level; frame.right += self.level; } }
    fn set_param(&mut self, _id: starplayer_dsp::ParamId, _value: i32) {}
    fn param(&self, _id: starplayer_dsp::ParamId) -> Option<i32> { None }
    fn reset(&mut self) { self.level = 0; }
    fn descriptor(&self) -> &'static starplayer_dsp::InsertDescriptor { &starplayer_dsp::effects::gain::GAIN_DESCRIPTOR }
}
#[test]
fn dsp_clear_bridges_the_old_output_for_64_frames_without_muting_fresh_live_input() {
    fn run(tail: i32) -> (Vec<i16>, Vec<i16>) {
        let (mut engine, mut handle, _) = setup(vec![Frame(129)], 8, 6);
        let mut inserts = engine.take_insert_control().unwrap();
        assert!(inserts.send(starplayer_engine::InsertCommand::Install { target: starplayer_engine::InsertTarget::Master, slot: 0, insert: Box::new(Tail { level: tail }) }).is_ok());
        let before = render(&mut engine, 128, 128);
        command(&mut handle, InteractiveAction::AllSoundOff);
        let after = render(&mut engine, 256, 3);
        (before, after)
    }
    let (before, after) = run(4000);
    let (_, clean) = run(0);
    assert_eq!(after[0], before[before.len() - 2]);
    assert_eq!(&after[128..], &clean[128..]);
    assert!(after[128..].iter().any(|sample| *sample != 0));
}

#[test]
fn no08_sub_frame_configured_cadence_always_progresses() {
    for micros in [0, 1, 22] {
        let mut configured = settings(); configured.articulation_interval_micros = micros;
        let mut engine = TestEngine::with_settings(configured);
        render(&mut engine, 256, 3);
        assert!(!engine.warnings().zero_advance_forced, "interval {micros}");
        assert!(!engine.warnings().event_limit_reached);
        assert_eq!(engine.control_clock().ticks(), 256);
    }
}

#[cfg(feature = "telemetry")]
#[test]
fn no08_command_burst_publishes_final_transport_live_and_cursor() {
    let (mut engine, mut handle, _) = setup(vec![Frame(1)], 8, 6);
    let mut reader = engine.telemetry_reader().unwrap();
    command(&mut handle, InteractiveAction::Play); render(&mut engine, 256, 128); reader.read();
    for _ in 0..5 { command(&mut handle, InteractiveAction::Resume); }
    command(&mut handle, InteractiveAction::AllSoundOff);
    command(&mut handle, InteractiveAction::Pause);
    render(&mut engine, 128, 128);
    let final_state = *reader.read();
    assert_eq!(final_state.transport.interactive, Some(InteractiveTransport::Paused));
    assert_eq!(final_state.live.active, 0);
    assert_eq!(final_state.transport.song_frame, 256);
    assert_eq!(final_state.transport.sequencer_cursor().unwrap().captured_frame, Frame(256));
}

#[cfg(feature = "telemetry")]
#[test]
fn no08_first_tracker_attachment_keeps_logical_cursor_and_fractional_remainder() {
    for state in [InteractiveTransport::Playing, InteractiveTransport::Paused, InteractiveTransport::Stopped] {
        let (mut engine, mut handle, _) = setup(vec![], 16, 6);
        let mut reader = engine.telemetry_reader().unwrap();
        command(&mut handle, InteractiveAction::Play); render(&mut engine, 1152, 128); reader.read();
        command(&mut handle, InteractiveAction::Pause); render(&mut engine, 128, 128);
        let saved = reader.read().transport.sequencer_cursor().unwrap();
        assert_ne!(saved.fractional_remainder, 0);
        let preparation_frame = engine.source_frame();
        let log = Log::new();
        let mut fresh = PatternSequencer::new(ExactFixedPoint, DemoPatternData::new(1, 16, 1), Processor { log: log.clone(), region: SampleRegion::one_shot(0, 0) }, SequencerSettings { sample_rate_hz: 44_100, initial_tempo_bpm: 130, initial_speed: 6, ..SequencerSettings::default() });
        assert!(fresh.restore_cursor(saved, preparation_frame));
        let live_deadline = preparation_frame.saturating_add(4096 + if state == InteractiveTransport::Paused { 17 } else { 145 });
        let mut prepared = SourceMux::new(2);
        prepared.insert(Box::new(fresh)).ok().unwrap();
        prepared.insert(Box::new(Live { log: log.clone(), events: vec![live_deadline], next: 0, region: SampleRegion::one_shot(0, 0) })).ok().unwrap();
        render(&mut engine, 4096, 128); reader.read();
        if state != InteractiveTransport::Paused {
            command(&mut handle, if state == InteractiveTransport::Playing { InteractiveAction::Play } else { InteractiveAction::Stop });
            render(&mut engine, 128, 128); reader.read();
        }
        let attachment_frame = engine.source_frame();
        engine.set_source(Box::new(prepared));
        command(&mut handle, match state { InteractiveTransport::Playing => InteractiveAction::Play, InteractiveTransport::Paused => InteractiveAction::Pause, InteractiveTransport::Stopped => InteractiveAction::Stop });
        render(&mut engine, 128, 128);
        let restored = reader.read().transport.sequencer_cursor().unwrap();
        assert_eq!(restored.song_frame, saved.song_frame, "{state:?}");
        assert_eq!(restored.remaining_frames, saved.remaining_frames, "{state:?}");
        assert_eq!(restored.fractional_remainder, saved.fractional_remainder);
        if state != InteractiveTransport::Playing { command(&mut handle, InteractiveAction::Resume); }
        let resume_frame = if state == InteractiveTransport::Playing { attachment_frame } else { engine.source_frame() };
        render(&mut engine, 2048, 128);
        assert!(log.values().contains(&(live_deadline.0 * 10 + 3)), "first attachment must not translate live timestamps");
        let ticks: Vec<_> = log.values().into_iter().filter(|entry| entry % 10 == 1).collect();
        assert_eq!(ticks[0] / 10, resume_frame.0 + saved.remaining_frames);
    }
}
