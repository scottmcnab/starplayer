//! The M1-B3 verifications that need a whole engine: tick timing through the render loop,
//! the deterministic tie-break between two sources, the zero-advance guard with the mux in
//! place, and the garbage channel keeping `free()` off the audio thread.
//!
//! The block-size determinism invariant itself lives in `block_size_determinism.rs`, which
//! now includes a sequencer-driven variant.

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use starplayer_core::{Command, ExactFixedPoint, Frame, Step, U0F16, VoiceParams};
use starplayer_dsp::Linear;
use starplayer_engine::demo::{DEMO_SET_TEMPO, DemoCell, DemoPatternData, DemoProcessor};
use starplayer_engine::{
    ControlDriver, Engine, EngineContext, EngineSettings, EventSource, MAX_ZERO_ADVANCE, PatternSequencer, PcmSource,
    RENDER_QUANTUM, SequencerSettings,
};
use starplayer_mixer::{FixedPath, LoopSpan, SampleRegion, StereoI16, VoiceTag, append_guarded_sample};
use starplayer_rt::Arc as RtArc;

// ── fixtures ────────────────────────────────────────────────────────────────────────

/// 44100 Hz at 125 BPM is exactly 882 frames per tick; 250 BPM is exactly 441.
const FRAMES_PER_TICK_AT_125: u64 = 882;
const FRAMES_PER_TICK_AT_250: u64 = 441;

const SAMPLE_RATE_HZ: u32 = 44_100;

type TestEngine = Engine<FixedPath, Linear, StereoI16>;

fn waveform() -> Vec<i16> { (0..64).map(|index: i16| 2_500 + index * 350).collect() }

fn looping_blob() -> (Vec<i16>, SampleRegion) {
    let mut blob = Vec::new();
    let region = append_guarded_sample(&mut blob, &waveform(), LoopSpan::new(16, 64));
    (blob, region)
}

/// What a `Send` test source shares with the test that built it.
///
/// `EventSource` is `Send` because the engine carries its sources to the host's audio
/// thread, so `Rc<RefCell<..>>` is no longer a shareable log. Atomics rather than a
/// `Mutex`, because a test double that took a lock inside `dispatch` would be modelling
/// exactly the thing `render()` may never do (design goal 5).
struct DispatchLog {
    values: [AtomicU64; DispatchLog::CAPACITY],
    written: AtomicUsize,
}

impl DispatchLog {
    const CAPACITY: usize = 512;

    fn new() -> StdArc<DispatchLog> {
        StdArc::new(DispatchLog { values: [const { AtomicU64::new(0) }; DispatchLog::CAPACITY], written: AtomicUsize::new(0) })
    }

    fn push(&self, value: u64) {
        let index = self.written.fetch_add(1, Ordering::Relaxed);
        if let Some(slot) = self.values.get(index) {
            slot.store(value, Ordering::Relaxed);
        }
    }

    fn values(&self) -> Vec<u64> {
        let written = self.written.load(Ordering::Relaxed).min(DispatchLog::CAPACITY);
        self.values.iter().take(written).map(|slot| slot.load(Ordering::Relaxed)).collect()
    }

    fn frames(&self) -> Vec<Frame> { self.values().into_iter().map(Frame).collect() }

    fn marks(&self) -> Vec<u8> { self.values().into_iter().map(|value| value as u8).collect() }

    fn is_empty(&self) -> bool { self.written.load(Ordering::Relaxed) == 0 }
}

/// Wraps a source and records the frame of every dispatch, so the render loop's timing can
/// be asserted from outside without reaching into a boxed `dyn EventSource`.
struct Recording<Source> {
    inner: Source,
    log: StdArc<DispatchLog>,
}

impl<Source: EventSource> EventSource for Recording<Source> {
    fn next_event_frame(&self) -> Option<Frame> { self.inner.next_event_frame() }
    fn advance_to(&mut self, frame: Frame) { self.inner.advance_to(frame) }
    fn dispatch(&mut self, frame: Frame, context: &mut EngineContext<'_>) {
        self.log.push(frame.0);
        self.inner.dispatch(frame, context);
    }
}

/// A source that logs a mark whenever it is dispatched, at a fixed list of frames.
struct MarkingSource {
    frames: Vec<Frame>,
    next: usize,
    mark: u8,
    log: StdArc<DispatchLog>,
}

impl EventSource for MarkingSource {
    fn next_event_frame(&self) -> Option<Frame> { self.frames.get(self.next).copied() }
    fn advance_to(&mut self, _frame: Frame) {}
    fn dispatch(&mut self, frame: Frame, _context: &mut EngineContext<'_>) {
        while self.frames.get(self.next).is_some_and(|due| *due <= frame) {
            self.log.push(self.mark as u64);
            self.next += 1;
        }
    }
}

/// A source that always claims an event is due right now, forever. S3M speed 0, `A00`, a
/// MOD `E60` self-loop and a pattern break to the same row all produce exactly this.
struct StuckSource;

impl EventSource for StuckSource {
    fn next_event_frame(&self) -> Option<Frame> { Some(Frame::ZERO) }
    fn advance_to(&mut self, _frame: Frame) {}
    fn dispatch(&mut self, _frame: Frame, _context: &mut EngineContext<'_>) {}
}

// ── tick timing through the render loop ─────────────────────────────────────────────

/// The task's verification: a `Txx` on tick N moves tick **N+1**, and it does so at the
/// exact frame regardless of how the host slices the output.
#[test]
fn a_tempo_change_moves_the_next_tick_to_an_exact_frame_at_any_block_size() {
    let row_one_tick_zero = 6 * FRAMES_PER_TICK_AT_125;
    let expected: Vec<Frame> = [
        0,
        FRAMES_PER_TICK_AT_125,
        2 * FRAMES_PER_TICK_AT_125,
        3 * FRAMES_PER_TICK_AT_125,
        4 * FRAMES_PER_TICK_AT_125,
        5 * FRAMES_PER_TICK_AT_125,
        row_one_tick_zero,
        row_one_tick_zero + FRAMES_PER_TICK_AT_250,
        row_one_tick_zero + 2 * FRAMES_PER_TICK_AT_250,
        row_one_tick_zero + 3 * FRAMES_PER_TICK_AT_250,
    ]
    .map(Frame)
    .to_vec();

    for block_frames in [1usize, 3, 128, 8191] {
        let log = DispatchLog::new();
        let (blob, region) = looping_blob();

        let mut data = DemoPatternData::new(1, 4, 1);
        data.set(0, 0, 0, DemoCell::note(48));
        data.set(0, 1, 0, DemoCell::command(DEMO_SET_TEMPO, 250));

        let sequencer = PatternSequencer::new(
            ExactFixedPoint,
            data,
            DemoProcessor::new(region, Step::ONE),
            SequencerSettings { sample_rate_hz: SAMPLE_RATE_HZ, ..SequencerSettings::default() },
        );

        let mut engine: TestEngine = Engine::new(8);
        engine.set_pcm(blob);
        engine.set_source(Box::new(Recording { inner: sequencer, log: StdArc::clone(&log) }));

        // Enough frames to reach the fourth tick of row 1.
        let total_frames = (row_one_tick_zero + 4 * FRAMES_PER_TICK_AT_250) as usize;
        let mut output = vec![0i16; total_frames * 2];
        let mut written = 0;
        while written < output.len() {
            let end = (written + block_frames * 2).min(output.len());
            engine.render(&mut output[written..end]);
            written = end;
        }

        let dispatched = log.frames();
        assert_eq!(&dispatched[..expected.len()], &expected[..], "block size {block_frames}: ticks did not land on their exact frames");
        assert!(!engine.warnings().any(), "block size {block_frames}: a well-behaved sequencer raises no warnings");
    }
}

#[test]
fn the_sequencer_takes_the_control_clock_over_from_the_synthesised_one() {
    let (blob, region) = looping_blob();
    let mut engine: TestEngine = Engine::new(8);
    engine.set_pcm(blob);
    assert_eq!(engine.control_clock().driver(), ControlDriver::Synthesised);
    assert_eq!(engine.control_clock().interval_frames(), 44, "1 ms at 44.1 kHz");

    // With no tracker, the engine generates its own control ticks.
    let mut output = vec![0i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);
    assert_eq!(engine.control_clock().ticks(), 3, "128 frames at one tick per 44 is three ticks");

    let data = DemoPatternData::new(1, 4, 1);
    let sequencer = PatternSequencer::new(
        ExactFixedPoint,
        data,
        DemoProcessor::new(region, Step::ONE),
        SequencerSettings { sample_rate_hz: SAMPLE_RATE_HZ, first_tick_frame: Frame(RENDER_QUANTUM as u64), ..SequencerSettings::default() },
    );
    engine.set_source(Box::new(sequencer));

    engine.render(&mut output);
    assert_eq!(engine.control_clock().driver(), ControlDriver::Tracker, "the tracker tick is the control tick");
    assert_eq!(engine.control_clock().ticks(), 4, "one tracker tick, and no synthesised ones on top of it");
}

// ── rule 3: deterministic tie-breaking ──────────────────────────────────────────────

/// Two sources with events at the same frames must dispatch in the same order every time
/// and at every block size, or offline rendering stops matching real time.
#[test]
fn two_sources_at_the_same_frame_dispatch_in_the_same_order_every_run_and_every_block_size() {
    fn run(block_frames: usize) -> Vec<u8> {
        let log = DispatchLog::new();
        let coincident = vec![Frame(100), Frame(300), Frame(300), Frame(1_000)];

        let mut engine: TestEngine = Engine::new(4);
        engine
            .add_source(Box::new(MarkingSource { frames: coincident.clone(), next: 0, mark: b'A', log: StdArc::clone(&log) }))
            .map_err(|_| "slot 0")
            .expect("slot 0");
        engine
            .add_source(Box::new(MarkingSource { frames: coincident, next: 0, mark: b'B', log: StdArc::clone(&log) }))
            .map_err(|_| "slot 1")
            .expect("slot 1");

        let mut output = vec![0i16; 2_048 * 2];
        let mut written = 0;
        while written < output.len() {
            let end = (written + block_frames * 2).min(output.len());
            engine.render(&mut output[written..end]);
            written = end;
        }
        log.marks()
    }

    let reference = run(RENDER_QUANTUM);
    assert_eq!(reference, b"ABAABBAB".to_vec(), "slot order within every frame, including the doubled one at 300");

    for block_frames in [1usize, 8191] {
        for iteration in 0..100 {
            assert_eq!(run(block_frames), reference, "block size {block_frames}, run {iteration}: the dispatch order moved");
        }
    }
}

#[test]
fn removing_a_source_does_not_renumber_the_others() {
    let log = DispatchLog::new();
    let mut engine: TestEngine = Engine::new(4);

    let first = engine
        .add_source(Box::new(MarkingSource { frames: vec![Frame(10)], next: 0, mark: b'0', log: StdArc::clone(&log) }))
        .map_err(|_| "slot 0")
        .expect("slot 0");
    engine
        .add_source(Box::new(MarkingSource { frames: vec![Frame(10), Frame(20)], next: 0, mark: b'1', log: StdArc::clone(&log) }))
        .map_err(|_| "slot 1")
        .expect("slot 1");
    assert_eq!(engine.sources().len(), 2);

    assert!(engine.remove_source(first).is_some());
    assert!(engine.remove_source(first).is_none(), "a stale slot handle resolves to nothing");
    assert_eq!(engine.sources().len(), 1);
}

// ── rule 2: the zero-advance guard, with the mux in place ───────────────────────────

#[test]
fn a_stuck_source_alongside_a_healthy_one_is_forced_forward_rather_than_hanging() {
    let log = DispatchLog::new();
    let (blob, region) = looping_blob();

    let mut engine: TestEngine = Engine::new(8);
    engine.set_pcm(blob);
    let params = VoiceParams { step: Step::from_ratio(3, 2), volume: U0F16::MAX, ..VoiceParams::SILENT };
    engine.voices_mut().allocate(VoiceTag::default(), region, params, 0).expect("a fresh pool has room");

    engine.add_source(Box::new(StuckSource)).map_err(|_| "slot 0").expect("slot 0");
    engine
        .add_source(Box::new(MarkingSource { frames: vec![Frame(50), Frame(150)], next: 0, mark: b'H', log: StdArc::clone(&log) }))
        .map_err(|_| "slot 1")
        .expect("slot 1");

    // If the guard were missing this call would never return.
    let mut output = vec![0i16; RENDER_QUANTUM * 2 * 2];
    engine.render(&mut output);

    let warnings = engine.warnings();
    assert!(warnings.zero_advance_forced, "the guard must flag the breach for the host to see");
    assert!(warnings.event_limit_reached, "128 frames x {} dispatches also passes the per-block cap", MAX_ZERO_ADVANCE + 1);
    assert_eq!(engine.frame(), Frame(2 * RENDER_QUANTUM as u64), "the clock advanced a full frame at a time");
    assert!(output.iter().any(|sample| *sample != 0), "and audio was still produced throughout");
    assert_eq!(log.marks(), b"HH".to_vec(), "the healthy source still got both of its events");
}

// ── the garbage channel ─────────────────────────────────────────────────────────────

/// Counts its own destruction, so a test can prove *where* a module was dropped rather than
/// merely that it was.
struct CountingModule {
    pcm: Vec<i16>,
    drops: StdArc<AtomicUsize>,
}

impl PcmSource for CountingModule {
    fn pcm(&self) -> &[i16] { &self.pcm }
}

impl Drop for CountingModule {
    fn drop(&mut self) { self.drops.fetch_add(1, Ordering::SeqCst); }
}

type ModuleEngine = Engine<FixedPath, Linear, StereoI16, RtArc<CountingModule>>;

fn counting_module(drops: &StdArc<AtomicUsize>) -> RtArc<CountingModule> {
    RtArc::new(CountingModule { pcm: looping_blob().0, drops: StdArc::clone(drops) })
}

#[test]
fn a_retired_module_leaves_the_audio_thread_alive_and_dies_on_the_control_thread() {
    let first_drops = StdArc::new(AtomicUsize::new(0));
    let second_drops = StdArc::new(AtomicUsize::new(0));

    let mut engine: ModuleEngine = Engine::new(8);
    let mut control = engine.take_control().expect("the handle is there until it is taken");
    assert!(engine.take_control().is_none(), "and only once");

    control.load_module(counting_module(&first_drops)).map_err(|_| "queued").expect("the ring has room");
    control.load_module(counting_module(&second_drops)).map_err(|_| "queued").expect("the ring has room");
    assert_eq!(control.queued_commands(), 2);

    // One render pass drains both commands, swaps the module twice, and retires the first.
    let mut output = vec![0i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);

    assert_eq!(control.queued_commands(), 0);
    assert_eq!(first_drops.load(Ordering::SeqCst), 0, "the first module was NOT dropped inside render()");
    assert_eq!(second_drops.load(Ordering::SeqCst), 0, "and the loaded one is obviously still alive");
    assert_eq!(control.pending_garbage(), 1, "it went down the garbage channel instead");
    assert!(!engine.warnings().any(), "which is the normal path, not a warning");

    let retired = control.collect_garbage().expect("the first module came back to the control side");
    assert_eq!(first_drops.load(Ordering::SeqCst), 0, "collecting is not yet dropping");
    drop(retired);
    assert_eq!(first_drops.load(Ordering::SeqCst), 1, "the control thread runs the destructor");

    assert!(engine.module().is_some(), "the second module is what the engine plays");
}

#[test]
fn collect_all_garbage_drops_everything_waiting() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let mut engine: ModuleEngine = Engine::new(8);
    let mut control = engine.take_control().expect("the handle");

    for _ in 0..4 {
        control.load_module(counting_module(&drops)).map_err(|_| "queued").expect("the ring has room");
    }
    let mut output = vec![0i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);

    assert_eq!(drops.load(Ordering::SeqCst), 0, "three retirements, none of them on the audio thread");
    assert_eq!(control.collect_all_garbage(), 3);
    assert_eq!(drops.load(Ordering::SeqCst), 3);
    assert_eq!(control.pending_garbage(), 0);
}

#[test]
fn a_control_thread_that_stops_collecting_is_reported_rather_than_leaking_forever() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let settings = EngineSettings { voice_capacity: 4, garbage_capacity: 2, ..EngineSettings::default() };
    let mut engine: ModuleEngine = Engine::with_settings(settings);
    let mut control = engine.take_control().expect("the handle");

    for _ in 0..6 {
        control.load_module(counting_module(&drops)).map_err(|_| "queued").expect("the ring has room");
    }
    let mut output = vec![0i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);

    assert_eq!(control.pending_garbage(), 2, "the channel holds what it can");
    assert!(engine.warnings().retired_module_dropped, "and the overflow is a warning, not a silent leak or a hang");
    assert_eq!(drops.load(Ordering::SeqCst), 3, "the three that did not fit were dropped inline as a last resort");
}

#[test]
fn a_loaded_module_is_what_the_engine_plays() {
    let drops = StdArc::new(AtomicUsize::new(0));
    let (_blob, region) = looping_blob();

    let mut engine: ModuleEngine = Engine::new(8);
    let mut control = engine.take_control().expect("the handle");
    control.load_module(counting_module(&drops)).map_err(|_| "queued").expect("the ring has room");
    // The command drains at the top of the next quantum; a voice triggered before that
    // belongs to whatever was loaded before and is released with it.
    engine.render(&mut vec![0i16; RENDER_QUANTUM * 2]);

    let params = VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT };
    engine.voices_mut().allocate(VoiceTag::default(), region, params, 0).expect("a fresh pool has room");

    let mut output = vec![0i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);
    assert!(output.iter().any(|sample| *sample != 0), "the voice resolved against the loaded module's PCM");
}

#[test]
fn loading_a_module_releases_every_voice_the_previous_module_was_playing() {
    // A voice triggered from module A carries an offset into A's PCM. After the swap to
    // B it must not exist at all — left alone it would read B's samples at A's offsets,
    // which is the "weird sounds after loading another file" bug.
    let drops = StdArc::new(AtomicUsize::new(0));
    let (_blob, region) = looping_blob();
    let mut engine: ModuleEngine = Engine::new(8);
    let mut control = engine.take_control().expect("the handle");
    control.load_module(counting_module(&drops)).map_err(|_| "queued").expect("room");
    engine.render(&mut vec![0i16; RENDER_QUANTUM * 2]);

    let params = VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT };
    let voice = engine.voices_mut().allocate(VoiceTag { channel: 3, ..VoiceTag::default() }, region, params, 0).expect("room");
    engine.channels_mut().get_mut(starplayer_core::ChannelId(3)).expect("channel 3").foreground = Some(voice);
    engine.channels_mut().get_mut(starplayer_core::ChannelId(3)).expect("channel 3").muted = true;
    assert_eq!(engine.voices().voices_active(), 1);

    control.load_module(counting_module(&drops)).map_err(|_| "queued").expect("room");
    let mut output = vec![1i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);

    assert_eq!(engine.voices().voices_active(), 0, "the old module's voices are gone with it");
    assert!(engine.voices().get(voice).is_none(), "the old handle is stale");
    assert!(engine.channels().get(starplayer_core::ChannelId(3)).is_some_and(|lane| lane.foreground.is_none()), "the binding went too");
    assert!(engine.channels().get(starplayer_core::ChannelId(3)).is_some_and(|lane| lane.muted), "the host's mute flag survives a load");
    assert!(output.iter().all(|sample| *sample == 0), "nothing from the old module is audible");
    assert_eq!(control.collect_all_garbage(), 1, "the retired module still comes back down the garbage channel");
}

// ── the rest of the control plane ───────────────────────────────────────────────────

#[test]
fn stopping_freezes_the_musical_clock_without_stopping_the_output_clock() {
    let log = DispatchLog::new();
    let mut engine: TestEngine = Engine::new(4);
    engine.set_source(Box::new(MarkingSource {
        frames: vec![Frame(64), Frame(300)],
        next: 0,
        mark: b'x',
        log: StdArc::clone(&log),
    }));
    let mut control = engine.take_control().expect("the handle");

    control.send(Command::Stop).map_err(|_| "queued").expect("the ring has room");
    let mut output = vec![0i16; RENDER_QUANTUM * 4 * 2];
    engine.render(&mut output);

    assert!(!engine.is_playing());
    assert_eq!(engine.frame(), Frame(4 * RENDER_QUANTUM as u64), "the output clock never stops");
    assert_eq!(engine.source_frame(), Frame::ZERO, "the musical clock did");
    assert!(log.is_empty(), "so nothing was dispatched, and nothing will be replayed in a burst on resume");

    control.send(Command::Play).map_err(|_| "queued").expect("the ring has room");
    engine.render(&mut output);
    assert_eq!(log.marks(), b"xx".to_vec(), "resuming picks up where the musical clock stopped, running frames 0..512");
    assert_eq!(engine.source_frame(), Frame(4 * RENDER_QUANTUM as u64), "512 musical frames, after 512 silent ones");
    assert_eq!(engine.frame(), Frame(8 * RENDER_QUANTUM as u64), "and 1024 output frames in total");
}

#[test]
fn stopping_silences_and_freezes_a_sounding_voice() {
    let (_blob, region) = looping_blob();
    let mut engine: TestEngine = Engine::new(4);
    engine.set_pcm(vec![500, -500, 1000, -1000, 500, -500, 1000, -1000]);
    let params = VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT };
    let voice = engine.voices_mut().allocate(VoiceTag::default(), region, params, 0).expect("a voice");
    let before = engine.voices().get(voice).map(|state| state.position()).expect("the voice exists");
    let mut control = engine.take_control().expect("the handle");
    control.send(Command::Stop).map_err(|_| "queued").expect("the ring has room");

    let mut output = vec![1i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);

    assert!(output.iter().all(|sample| *sample == 0), "a stopped transport is silent");
    assert_eq!(engine.voices().get(voice).map(|state| state.position()), Some(before), "the voice resumes from the same sample frame");
}

#[test]
fn muting_a_channel_silences_its_ringing_voice_and_unmuting_resumes_mid_note() {
    // Two engines, one voice each on channel 1, rendered in lock-step: one is muted for
    // the middle quantum, the other never is. The muted quantum must be silent, and after
    // the unmute the two outputs must be byte-identical — the muted voice kept moving.
    let (blob, region) = looping_blob();
    let params = VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT };
    let tag = VoiceTag { channel: 1, ..VoiceTag::default() };
    let mut muted: TestEngine = Engine::new(8);
    let mut reference: TestEngine = Engine::new(8);
    for engine in [&mut muted, &mut reference] {
        engine.set_pcm(blob.clone());
        let voice = engine.voices_mut().allocate(tag, region, params, 0).expect("a voice");
        engine.voices_mut().get_mut(voice).expect("live").settle_gains();
    }
    let mut control = muted.take_control().expect("the handle");
    fn quantum(engine: &mut TestEngine) -> Vec<i16> { let mut output = vec![0i16; RENDER_QUANTUM * 2]; engine.render(&mut output); output }

    let (first_muted, first_reference) = (quantum(&mut muted), quantum(&mut reference));
    assert_eq!(first_muted, first_reference, "before the mute the two engines agree");
    assert!(first_reference.iter().any(|sample| *sample != 0), "the reference voice is audible");

    control.send(Command::MuteChannel { channel: starplayer_core::ChannelId(1), muted: true }).map_err(|_| "queued").expect("room");
    let silent = quantum(&mut muted);
    let audible = quantum(&mut reference);
    assert!(silent.iter().all(|sample| *sample == 0), "a muted channel contributes nothing");
    assert!(audible.iter().any(|sample| *sample != 0));
    assert!(muted.channels().get(starplayer_core::ChannelId(1)).is_some_and(|lane| lane.muted));
    assert_eq!(muted.voices().voices_active(), 1, "the muted voice is still alive");

    control.send(Command::MuteChannel { channel: starplayer_core::ChannelId(1), muted: false }).map_err(|_| "queued").expect("room");
    assert_eq!(quantum(&mut muted), quantum(&mut reference), "after the unmute the voice is exactly where it would have been");
    assert_eq!(quantum(&mut muted), quantum(&mut reference));
}

#[test]
fn master_volume_arrives_and_unsupported_commands_are_flagged_rather_than_ignored() {
    let mut engine: TestEngine = Engine::new(4);
    let mut control = engine.take_control().expect("the handle");
    control.send(Command::SetMasterVolume(U0F16::from_bits(1_234))).map_err(|_| "queued").expect("room");
    control.send(Command::SeekOrder(3)).map_err(|_| "queued").expect("room");

    let mut output = vec![0i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);

    assert_eq!(engine.master_volume(), U0F16::from_bits(1_234));
    assert!(engine.warnings().unsupported_command, "SeekOrder needs the concrete sequencer, and says so");
}

#[test]
fn a_full_command_ring_hands_the_command_back_rather_than_dropping_it() {
    let settings = EngineSettings { voice_capacity: 4, command_capacity: 2, ..EngineSettings::default() };
    let mut engine: TestEngine = Engine::with_settings(settings);
    let mut control = engine.take_control().expect("the handle");

    assert!(control.send(Command::Play).is_ok());
    assert!(control.send(Command::Play).is_ok());
    assert_eq!(control.command_capacity(), 2);
    assert!(matches!(control.send(Command::Stop), Err(Command::Stop)), "the caller keeps what it queued");

    let mut output = vec![0i16; RENDER_QUANTUM * 2];
    engine.render(&mut output);
    assert_eq!(control.queued_commands(), 0);
    assert!(control.send(Command::Stop).is_ok(), "and room reappears once the audio thread has drained");
}

/// The control half has to cross a thread boundary — it is created next to the engine and
/// then handed to the UI thread, a loader task or a `postMessage` handler.
#[test]
fn the_control_handle_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<starplayer_engine::EngineHandle<RtArc<CountingModule>>>();
    assert_send::<starplayer_engine::EngineHandle<()>>();
}
