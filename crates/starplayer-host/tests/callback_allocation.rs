//! Design goal 5, for the *host* half of the audio path.
//!
//! `crates/starplayer-offline/tests/render_allocation.rs` proves that `Engine::render`
//! allocates nothing, over the whole module corpus. That is the engine's half. This is the
//! other one: between the device and `render()` sits the host's callback — it drains a
//! command ring, reads a telemetry snapshot, arms a fade, walks a per-frame gain, quantises
//! to the output depth and swaps sources and modules. Every one of those is somewhere a
//! `Vec::push` could hide, and an allocation there stalls the audio callback exactly as one
//! inside `render()` would.
//!
//! The hook is the same shape as the offline one and for the same reasons: it **records**
//! rather than panicking, because panicking from inside `GlobalAlloc::alloc` re-enters the
//! allocator, and it is per thread, because `cargo test` runs these functions in parallel.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use starplayer::core::{AtEnd, ChannelId, Interpolator, U0F16};
use starplayer::engine::{MixPathKind, MixerMode, OutputDepth, RENDER_QUANTUM};
use starplayer_host::{AudioSpec, ManualBackend, Player};

const FIXTURE: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/PETRI.S3M");
const REFLEX: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");

// ── the allocator hook ──────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct AllocationWatch {
    armed: bool,
    allocations: usize,
    bytes: usize,
    largest: usize,
}

thread_local! {
    static WATCH: Cell<AllocationWatch> = const { Cell::new(AllocationWatch { armed: false, allocations: 0, bytes: 0, largest: 0 }) };
}

struct CallbackGuardAllocator;

// SAFETY: both methods forward to `System` unchanged; the wrapper only reads and writes a
// destructor-free thread-local `Cell` of `Copy` data, so nothing it adds can allocate and
// it cannot recurse into itself.
unsafe impl GlobalAlloc for CallbackGuardAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: `layout` is the caller's, forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // A `free()` in the callback is the very stall the retirement rings exist to
        // prevent, so a deallocation counts as a violation too.
        record(layout.size());
        // SAFETY: `pointer` and `layout` are the caller's, forwarded unchanged, and this
        // allocator only ever hands out blocks obtained from `System`.
        unsafe { System.dealloc(pointer, layout) }
    }
}

fn record(size: usize) {
    let _ = WATCH.try_with(|watch| {
        let mut current = watch.get();
        if !current.armed {
            return;
        }
        current.allocations = current.allocations.saturating_add(1);
        current.bytes = current.bytes.saturating_add(size);
        current.largest = current.largest.max(size);
        watch.set(current);
    });
}

#[global_allocator]
static ALLOCATOR: CallbackGuardAllocator = CallbackGuardAllocator;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct AllocationReport {
    allocations: usize,
    bytes: usize,
    largest: usize,
}

impl AllocationReport {
    fn is_clean(self) -> bool { self.allocations == 0 }
}

fn while_watching_for_allocations<T>(body: impl FnOnce() -> T) -> (T, AllocationReport) {
    WATCH.with(|watch| watch.set(AllocationWatch { armed: true, ..AllocationWatch::default() }));
    let result = body();
    let seen = WATCH.with(|watch| {
        let seen = watch.get();
        watch.set(AllocationWatch::default());
        seen
    });
    (result, AllocationReport { allocations: seen.allocations, bytes: seen.bytes, largest: seen.largest })
}

// ── the callback under the hook ─────────────────────────────────────────────────────

#[test]
fn the_hook_itself_reports_an_allocation_made_inside_the_armed_region() {
    let (_, clean) = while_watching_for_allocations(|| std::hint::black_box(1u32 + 1));
    assert!(clean.is_clean(), "an armed region that allocates nothing reports nothing: {clean:?}");

    let (_, dirty) = while_watching_for_allocations(|| {
        let mut growing: Vec<u64> = Vec::new();
        for value in 0..4u64 {
            growing.push(value);
        }
        std::hint::black_box(growing.len())
    });
    assert!(!dirty.is_clean(), "a Vec::push inside the armed region must be reported");
}

/// Every mixer arm, every depth, and every block size that exercises the ring's partial
/// path — with a module playing and the transport ramping.
#[test]
fn the_render_callback_allocates_nothing_on_any_arm_at_any_depth() {
    let mut offenders = Vec::new();
    for path in [MixPathKind::Float, MixPathKind::Fixed] {
        for interpolator in [Interpolator::None, Interpolator::Linear] {
            for depth in [OutputDepth::F32, OutputDepth::I32, OutputDepth::I24, OutputDepth::I16, OutputDepth::I8] {
                for dither in [false, true] {
                    let mode = MixerMode { path, interpolator, depth, dither, channels: 2 };
                    let report = drive_once(mode, &[128, 37, 4_096]);
                    if !report.is_clean() {
                        offenders.push(format!("{mode:?}: {report:?}"));
                    }
                }
            }
        }
    }
    assert!(offenders.is_empty(), "the render callback allocated:\n{}", offenders.join("\n"));
}

fn drive_once(mode: MixerMode, block_sizes: &[usize]) -> AllocationReport {
    let spec = AudioSpec::stereo(44_100);
    let mut backend = ManualBackend::new();
    let mut player = Player::open(&mut backend, None, spec, mode).expect("the arm exists");
    let driver = backend.driver().expect("driver");
    player.load(FIXTURE).expect("the fixture loads");
    player.play().expect("play");

    let mut worst = AllocationReport::default();
    for block_frames in block_sizes {
        // Allocated before the hook is armed, exactly as a backend allocates its buffer once
        // and reuses it for every callback.
        let mut block = vec![0.0f32; spec.samples_for(*block_frames)];
        let (_, report) = while_watching_for_allocations(|| {
            for _ in 0..8 {
                driver.render(&mut block);
            }
        });
        if report.allocations > worst.allocations {
            worst = report;
        }
    }
    player.collect_garbage();
    worst
}

/// The riskiest moment in the callback: the module swap. It replaces the source, pushes the
/// retired one onto the retirement ring, moves the retired module out of the engine's
/// garbage channel and onto the same ring, and never drops either.
#[test]
fn swapping_a_module_inside_the_callback_allocates_nothing_and_drops_nothing_there() {
    let spec = AudioSpec::stereo(44_100);
    let mut backend = ManualBackend::new();
    let mut player = Player::open(&mut backend, None, spec, MixerMode::DEFAULT).expect("it opens");
    let driver = backend.driver().expect("driver");
    player.load(FIXTURE).expect("first load");
    player.play().expect("play");

    let mut block = vec![0.0f32; spec.samples_for(RENDER_QUANTUM)];
    driver.render(&mut block);

    // Decoded, scanned and built on this thread — the callback only ever moves the result.
    player.load(REFLEX).expect("second load");
    let (_, report) = while_watching_for_allocations(|| {
        driver.render(&mut block);
        driver.render(&mut block);
    });

    assert!(report.is_clean(), "swapping a module inside the callback allocated: {report:?}");
    assert_eq!(player.pending_garbage(), 2, "the retired module and its sequencer are both waiting on the control thread");
    assert!(!player.warnings().retired_module_dropped, "and neither had to be dropped inline");
    assert_eq!(player.collect_garbage(), 2, "the control thread is where they die");
}

/// The transport's own paths: Play, Stop, the seek mailbox, the fade arming at the end of
/// the song, and the typed engine stop the ramp lands on.
#[test]
fn the_transport_and_the_end_of_song_allocate_nothing() {
    let spec = AudioSpec::stereo(22_050);
    let mut backend = ManualBackend::new();
    let mut player = Player::open(&mut backend, None, spec, MixerMode::DEFAULT).expect("it opens");
    let driver = backend.driver().expect("driver");
    player.load(FIXTURE).expect("the fixture loads");
    player.set_at_end(AtEnd::Stop).expect("stop at the end");
    player.play().expect("play");

    let length = player.song_length().expect("scanned") as usize;
    let mut block = vec![0.0f32; spec.samples_for(1_024)];
    let blocks = (length + 4 * RENDER_QUANTUM).div_ceil(1_024);

    player.stop().expect("stop");
    player.play().expect("play again");
    player.seek_frame(length as u64 / 4).expect("seek");
    player.mute(ChannelId(3), true).expect("mute");
    player.set_master_volume(U0F16::from_bits(30_000)).expect("master volume");

    let (_, report) = while_watching_for_allocations(|| {
        for _ in 0..blocks {
            driver.render(&mut block);
        }
    });
    assert!(report.is_clean(), "playing a song to its end allocated in the callback: {report:?}");
    assert!(!player.is_playing(), "the song reached its end and the transport stopped");
    player.collect_garbage();
}

/// The hook has to be able to see another thread's allocation as *not* this one's, or it
/// would be useless under `cargo test`'s parallel harness.
#[test]
fn allocation_on_another_thread_is_not_a_violation() {
    let (_, report) = while_watching_for_allocations(|| {
        std::thread::spawn(|| vec![7u8; 1 << 20].len()).join().expect("the helper thread finished")
    });
    assert!(report.largest < 1 << 20, "another thread's megabyte was attributed to this one: {report:?}");
}

#[cfg(all(feature = "note-overlap", feature = "interactive-audition"))]
#[test]
fn interactive_pause_clear_seek_and_rebuild_replay_allocate_nothing_in_callback() {
    let mut backend = ManualBackend::new();
    let spec = AudioSpec::stereo(44_100);
    let settings = starplayer::engine::EngineSettings::interactive_web(Default::default(), 256).unwrap();
    let mut player = Player::open_with_settings(&mut backend, None, spec, MixerMode::DEFAULT, settings).unwrap();
    let driver = backend.driver().unwrap();
    player.load(include_bytes!("../../starplayer-s3m/tests/fixtures/PETRI.S3M")).unwrap(); player.play().unwrap();
    let mut output = [0.0; 2048];
    driver.render(&mut output);
    for action in [starplayer::core::InteractiveAction::Pause, starplayer::core::InteractiveAction::Stop, starplayer::core::InteractiveAction::Resume, starplayer::core::InteractiveAction::AllSoundOff] {
        player.interactive(action).unwrap();
        let (_, report) = while_watching_for_allocations(|| driver.render(&mut output));
        assert!(report.is_clean(), "{action:?}: {report:?}");
    }
    player.pause().unwrap(); player.seek_row(3).unwrap();
    let (_, report) = while_watching_for_allocations(|| driver.render(&mut output)); assert!(report.is_clean(), "seek: {report:?}");
    player.telemetry();
    let mut mode = player.mixer_mode(); mode.interpolator = starplayer::core::Interpolator::Cubic;
    player.set_mixer_mode(&mut backend, None, mode).unwrap(); let driver = backend.driver().unwrap();
    let (_, report) = while_watching_for_allocations(|| driver.render(&mut output)); assert!(report.is_clean(), "replay: {report:?}");
    let midi = b"MThd\0\0\0\x06\0\0\0\x01\0\x60MTrk\0\0\0\x08\0\x90\x3C\x64\x60\xFF\x2F\0";
    let instruments = starplayer::rt::Arc::clone(player.module().unwrap());
    player.load_smf(midi, instruments).unwrap(); player.play().unwrap(); driver.render(&mut output);
    player.pause().unwrap(); driver.render(&mut output); player.telemetry();
    player.reopen(&mut backend, None, AudioSpec::stereo(48_000)).unwrap(); let driver = backend.driver().unwrap();
    let (_, report) = while_watching_for_allocations(|| driver.render(&mut output)); assert!(report.is_clean(), "SMF rebuild: {report:?}");
    player.resume().unwrap();
    let (_, report) = while_watching_for_allocations(|| driver.render(&mut output)); assert!(report.is_clean(), "SMF resume: {report:?}");
}

#[cfg(feature = "note-overlap")]
#[test]
fn active_legacy_overlap_pressure_bypass_and_actual_instrument_table_swaps_are_callback_safe() {
    use starplayer::core::{InstrumentId, InteractiveAction, PlaybackCapabilities, PlaybackIdentity};
    use starplayer::engine::{EngineSettings, PlaybackConfiguration, VoiceCapacity};
    use starplayer::model::{InstrumentDef, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec};
    fn module(format: ModuleFormat, instruments: usize) -> starplayer::rt::Arc<starplayer::model::Module> {
        let mut builder = ModuleBuilder::new();
        for _ in 0..instruments {
            let sample = builder.add_sample(&vec![1000; 65_536], SampleSpec::one_shot("long sample")).unwrap();
            builder.add_instrument(InstrumentDef::from_sample("instrument", sample, U0F16::MAX)).unwrap();
        }
        let mut pattern = Vec::new();
        for row in 0..64 {
            for channel in 0..4 {
                let instrument = if channel == 0 { 1 } else { 0 };
                match format {
                    ModuleFormat::Mod => pattern.extend(starplayer::mod_file::ModCell { period: if channel == 0 { [428, 285, 339, 320][row % 4] } else { 0 }, instrument, effect: 0, param: 0 }.to_bytes()),
                    ModuleFormat::Mtm => pattern.extend(starplayer::mtm::MtmCell { pitch: if channel == 0 { [24, 31, 28, 29][row % 4] } else { 0 }, instrument, effect: 0, param: 0 }.to_bytes()),
                    ModuleFormat::S3m => pattern.extend([if channel == 0 { [0x40, 0x47, 0x44, 0x45][row % 4] } else { 255 }, instrument, 255, 0, 0]),
                    _ => unreachable!(),
                }
            }
        }
        builder.add_pattern(&pattern, 64, 4).unwrap(); builder.set_orders(&[0, starplayer::model::ORDER_END]);
        let mut header = ModuleHeader::new(format, 4); header.initial_speed = 1; builder.set_header(header);
        starplayer::rt::Arc::new(builder.build().unwrap())
    }
    let settings = EngineSettings { channel_count: 4, ..EngineSettings::default() }.with_playback(PlaybackConfiguration {
        identity: PlaybackIdentity::default(), capabilities: if cfg!(feature = "interactive-audition") { PlaybackCapabilities::all() } else { PlaybackCapabilities::NOTE_OVERLAP }, instrument_count: 0,
        capacity: VoiceCapacity { native: 4, custom: 2, live: if cfg!(feature = "interactive-audition") { 2 } else { 0 }, interactive_transport: cfg!(feature = "interactive-audition") },
    }).unwrap();
    let mut backend = ManualBackend::new();
    let mut player = Player::open_with_settings(&mut backend, None, AudioSpec::stereo(44_100), MixerMode::DEFAULT, settings).unwrap();
    let driver = backend.driver().unwrap(); let mut output = [0.0; 2048];
    for (format, count) in [(ModuleFormat::Mod, 2), (ModuleFormat::S3m, 3), (ModuleFormat::Mtm, 1)] {
        player.load_module(module(format, count)).unwrap();
        player.interactive(InteractiveAction::SetLetNotesRing { instrument: InstrumentId(0), enabled: true }).unwrap();
        player.interactive(InteractiveAction::SetNoteOverlapEnabled(true)).unwrap(); player.play().unwrap();
        let (_, report) = while_watching_for_allocations(|| { for _ in 0..12 { driver.render(&mut output); } });
        assert!(report.is_clean(), "{format:?} table install, enabled detachment, eviction and duplicate scans: {report:?}");
        assert!(player.telemetry().voices_active >= 2, "enabled overlap actually ran for {format:?}");
        player.interactive(InteractiveAction::SetNoteOverlapEnabled(false)).unwrap();
        let (_, report) = while_watching_for_allocations(|| driver.render(&mut output));
        assert!(report.is_clean(), "{format:?} bypass retirement: {report:?}");
        player.collect_garbage();
    }
}

#[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
#[test]
fn burst_prepared_loads_wait_for_retirement_room_without_callback_drops() {
    #[cfg(feature = "note-overlap")]
    use starplayer::core::{InstrumentId, InteractiveAction};
    let mut backend = ManualBackend::new();
    let settings = starplayer::engine::EngineSettings::default().with_playback(starplayer::engine::PlaybackConfiguration {
        identity: Default::default(), capabilities: starplayer::engine::compiled_playback_capabilities(), instrument_count: 0,
        capacity: starplayer::engine::VoiceCapacity { native: 64, custom: if cfg!(feature = "note-overlap") { 4 } else { 0 }, live: if cfg!(feature = "interactive-audition") { 2 } else { 0 }, interactive_transport: cfg!(feature = "interactive-audition") },
    }).unwrap();
    let mut player = Player::open_with_settings(&mut backend, None, AudioSpec::stereo(44_100), MixerMode::DEFAULT, settings).unwrap();
    let driver = backend.driver().unwrap();
    for _ in 0..12 { player.load(include_bytes!("../../../fuzz/seeds/mod/synthetic-golden.mod")).unwrap(); }
    #[cfg(feature = "note-overlap")]
    {
        player.interactive(InteractiveAction::SetLetNotesRing { instrument: InstrumentId(0), enabled: true }).unwrap();
        player.interactive(InteractiveAction::SetNoteOverlapEnabled(true)).unwrap();
    }
    player.play().unwrap();
    let mut output = [0.0; 8192];
    let (_, report) = while_watching_for_allocations(|| driver.render(&mut output));
    assert!(report.is_clean(), "full retirement ring must defer a complete prepared Load: {report:?}");
    assert!(!player.warnings().retired_module_dropped);
    assert!(!player.is_playing(), "later commands remain ordered behind the deferred loads");
    assert!(player.collect_garbage() != 0);
    let (_, report) = while_watching_for_allocations(|| driver.render(&mut output));
    assert!(report.is_clean(), "deferred source/table/module swaps: {report:?}");
    assert!(player.is_playing()); assert!(!player.warnings().retired_module_dropped);
    player.collect_garbage();
}

#[cfg(feature = "note-overlap")]
#[test]
fn active_xm_it_overlap_articulation_pressure_and_bypass_are_callback_safe() {
    use starplayer::core::{InstrumentId, InteractiveAction, PlaybackCapabilities, PlaybackIdentity};
    use starplayer::engine::{EngineSettings, PlaybackConfiguration, VoiceCapacity};
    use starplayer::model::{InstrumentDef, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec};
    fn module(format: ModuleFormat, instruments: usize) -> starplayer::rt::Arc<starplayer::model::Module> {
        let mut builder = ModuleBuilder::new();
        for _ in 0..instruments {
            let sample = builder.add_sample(&vec![1000; 65_536], SampleSpec::one_shot("long sample")).unwrap();
            let mut instrument = InstrumentDef::from_sample("instrument", sample, U0F16::MAX);
            instrument.note_sample_map.fill(sample.0 + 1);
            for (note, mapped) in instrument.note_transpose_map.iter_mut().enumerate() { *mapped = note as u8; }
            builder.add_instrument(instrument).unwrap();
        }
        let mut pattern = Vec::new();
        for row in 0..64 {
            for channel in 0..4 {
                let instrument = if channel == 0 { 1 } else { 0 };
                match format {
                    ModuleFormat::Xm => pattern.extend(starplayer::xm::XmCell { note: if channel == 0 { [49, 56, 53, 54][row % 4] } else { 0 }, instrument, ..starplayer::xm::XmCell::EMPTY }.to_bytes()),
                    ModuleFormat::It => pattern.extend(starplayer::it::ItCell { note: if channel == 0 { [48, 55, 52, 53][row % 4] } else { starplayer::it::NOTE_NONE }, instrument, ..starplayer::it::ItCell::EMPTY }.to_bytes()),
                    _ => unreachable!(),
                }
            }
        }
        builder.add_pattern(&pattern, 64, 4).unwrap(); builder.set_orders(&[0, starplayer::model::ORDER_END]);
        let mut header = ModuleHeader::new(format, 4); header.initial_speed = 1;
        if format == ModuleFormat::It { header.format_extra = starplayer::it::ItFormatExtra { flags: 12, special: 0, old_instruments: false, has_midi_configuration: false }.encode(); }
        builder.set_header(header);
        starplayer::rt::Arc::new(builder.build().unwrap())
    }
    let settings = EngineSettings { channel_count: 4, ..EngineSettings::default() }.with_playback(PlaybackConfiguration {
        identity: PlaybackIdentity::default(), capabilities: if cfg!(feature = "interactive-audition") { PlaybackCapabilities::all() } else { PlaybackCapabilities::NOTE_OVERLAP }, instrument_count: 0,
        capacity: VoiceCapacity { native: 256, custom: 2, live: if cfg!(feature = "interactive-audition") { 2 } else { 0 }, interactive_transport: cfg!(feature = "interactive-audition") },
    }).unwrap();
    let mut backend = ManualBackend::new();
    let mut player = Player::open_with_settings(&mut backend, None, AudioSpec::stereo(44_100), MixerMode::DEFAULT, settings).unwrap();
    let driver = backend.driver().unwrap(); let mut output = [0.0; 2048];
    for (format, count) in [(ModuleFormat::Xm, 2), (ModuleFormat::It, 3)] {
        player.load_module(module(format, count)).unwrap();
        player.interactive(InteractiveAction::SetLetNotesRing { instrument: InstrumentId(0), enabled: true }).unwrap();
        player.interactive(InteractiveAction::SetNoteOverlapEnabled(true)).unwrap(); player.play().unwrap();
        let (_, report) = while_watching_for_allocations(|| { for _ in 0..12 { driver.render(&mut output); } });
        assert!(report.is_clean(), "{format:?} table install, enabled detachment, eviction and duplicate scans: {report:?}");
        assert!(player.telemetry().voices_active >= 2, "enabled overlap actually ran for {format:?}");
        player.interactive(InteractiveAction::SetNoteOverlapEnabled(false)).unwrap();
        let (_, report) = while_watching_for_allocations(|| driver.render(&mut output));
        assert!(report.is_clean(), "{format:?} bypass retirement: {report:?}");
        player.collect_garbage();
    }
}

#[cfg(feature = "interactive-audition")]
#[test]
fn actual_loaded_native_live_runtimes_and_bounded_controls_allocate_and_free_nothing() {
    use starplayer::core::{Event, InstrumentId, InteractiveAction, LiveDestination, Note};
    for format in [starplayer::model::ModuleFormat::Xm,starplayer::model::ModuleFormat::It] {
        use starplayer::model::{ModuleBuilder, ModuleHeader, ModuleFormat, SampleSpec, InstrumentDef, Envelope, EnvelopePoint};
        let mut builder = ModuleBuilder::new(); let sample = builder.add_sample(&[1500;32768],SampleSpec::one_shot("live")).unwrap();
        let mut instrument = InstrumentDef::from_sample("live",sample,U0F16::MAX); instrument.note_sample_map.fill(1);
        for (note,mapped) in instrument.note_transpose_map.iter_mut().enumerate() { *mapped = note as u8; }
        instrument.new_note_action = starplayer::model::NewNoteAction::Continue;
        instrument.volume_envelope = Some(Envelope {points:vec![EnvelopePoint {tick:0,value:64},EnvelopePoint {tick:100,value:0}].into_boxed_slice(),..Envelope::default()});
        builder.add_instrument(instrument).unwrap();
        let cells = if format == ModuleFormat::Xm {vec![49,1,0,0,0,56,1,0,0,0,49,1,0x27,0,0,49,1,0x27,0,0]} else {vec![60,1,255,0,0,67,1,255,0,0,60,1,23,0,0,60,1,23,0,0]}; builder.add_pattern(&cells,4,1).unwrap(); builder.set_orders(&[0]);
        let mut header = ModuleHeader::new(format,1); header.flags.linear_slides=true;
        if format == ModuleFormat::It { header.format_extra=starplayer::it::ItFormatExtra {flags:12,special:0,old_instruments:false,has_midi_configuration:false}.encode(); }
        builder.set_header(header); let module = starplayer::rt::Arc::new(builder.build().unwrap());
        let mut backend = ManualBackend::new();
        let settings = starplayer::engine::EngineSettings::default().with_playback(starplayer::engine::PlaybackConfiguration {
            identity:Default::default(),capabilities:starplayer::engine::compiled_playback_capabilities(),instrument_count:0,
            capacity:starplayer::engine::VoiceCapacity {native:256,custom:if cfg!(feature="note-overlap") {4} else {0},live:16,interactive_transport:true},
        }).unwrap();
        let mut player = Player::open_with_settings(&mut backend,None,AudioSpec::stereo(44_100),MixerMode::DEFAULT,settings).unwrap();
        let driver = backend.driver().unwrap(); player.load_module(module).unwrap();
        player.interactive(InteractiveAction::SetLiveDestination(LiveDestination::Selected(InstrumentId(0)))).unwrap();
        #[cfg(feature = "note-overlap")]
        { player.interactive(InteractiveAction::SetLetNotesRing { instrument: InstrumentId(0), enabled: true }).unwrap(); player.interactive(InteractiveAction::SetNoteOverlapEnabled(true)).unwrap(); }
        player.play().unwrap();
        let mut output = [0.0;512]; let mut heard = false;
        let (_, report) = while_watching_for_allocations(|| {
            for index in 0..80 {
                if index == 40 { let _ = player.pause(); }
                if index == 60 { let _ = player.resume(); }
                let _ = player.send_event(0,Event::NoteOn {note:Note::new(60+(index%12) as u8),velocity:U0F16::MAX});
                driver.render(&mut output); heard |= output.iter().any(|sample| *sample != 0.0);
            }
            let _ = player.interactive(InteractiveAction::PreviewInstrument(InstrumentId(u16::MAX)));
            let _ = player.interactive(InteractiveAction::StopPreview); driver.render(&mut output);
            let _ = player.interactive(InteractiveAction::Stop); driver.render(&mut output);
        });
        assert!(report.is_clean(),"native live runtime, pressure, reject and clears: {report:?}");
        assert!(player.event_clock().sent() > 0); assert!(heard,"the actual native runtime must sound under the allocation hook");
    }
}
