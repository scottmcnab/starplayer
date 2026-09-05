//! M2-C7 deliverable 6: a test-only global allocator that records every allocation made
//! while a thread-local "inside `render()`" flag is set, run over the whole corpus.
//!
//! Design goal 5 says no allocation, no locks and no panics inside `render()`. Until this
//! file existed the first of those was held **by inspection** — `Engine::render` carried a
//! `TODO(M2-task-C7)` saying exactly that. Inspection does not survive contact with a
//! "just for telemetry" `Vec::push`, so this arms a real allocator hook around every
//! `render()` call and pushes every module the repository can reach through it.
//!
//! # Why the hook records rather than panics
//!
//! Panicking from inside `GlobalAlloc::alloc` is not safe to do: the panic payload is
//! itself boxed, so the panic path re-enters the allocator, and unwinding out of an
//! allocation abandons the collection that asked for it mid-resize. The hook therefore
//! records the allocation — count, bytes, largest single request — lets it proceed, and
//! the test asserts the count is zero the moment `render()` returns. A violation is then
//! reported *with the module that caused it*, which an abort inside the allocator could
//! not do.
//!
//! # Why not `assert_no_alloc`
//!
//! See the C7 research-point answers in
//! `plans/engine/complete/M2-task-C7-fuzzing-and-rt-safety.md`. In short: the crate is a
//! thread-local counter plus a `GlobalAlloc` wrapper, which is the fifty lines below, and
//! its default violation handler aborts the process — which would say that *something*
//! allocated, but not which of seven hundred modules did it.
//!
//! # The one build this cannot run in
//!
//! `feature = "trace"` puts the per-tick recorder inside `render()`, and the recorder
//! appends a `TraceTick` per tick — it allocates by construction, which is what the hook
//! reported the first time `cargo xtask ci --job host-tests` reached the trace pass
//! (nineteen to forty-seven allocations a module, growing with the channel count). That is
//! a deliberate diagnostic build, not a real-time one: `trace` is in no default feature
//! set, `assert_trace_stays_off_the_default_workspace_build` proves the workspace never
//! resolves it, and `--job trace-zero-cost` proves the recorder compiles to nothing
//! without it. So this file compiles to nothing under `trace` rather than asserting
//! something that build never promised.

#![cfg(not(feature = "trace"))]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};

use starplayer::core::{ChannelId, ExactFixedPoint};
use starplayer::dsp::effects::GAIN_PARAM;
use starplayer::dsp::effects::eq::EQ_PEAK_GAIN_PARAM;
use starplayer::dsp::{InsertKind, Linear, build_insert};
use starplayer::engine::{Engine, EngineSettings, EventSource, InsertCommand, InsertTarget};
use starplayer::mixer::{FixedPath, StereoI16};
use starplayer::model::{Module, ModuleFormat};
use starplayer::rt::Arc;

// ── the allocator hook ──────────────────────────────────────────────────────────────

/// What the hook saw during one armed region, and whether it is armed at all.
///
/// **Everything is per thread.** `cargo test` runs the test functions in this binary on
/// parallel threads, and two of them arm the hook, so process-wide counters would let one
/// test's `thread::spawn` land in another's report. A global counter cannot answer "did
/// *this* `render()` allocate" at all; only a per-thread one can, and it is also the
/// honest model, because the claim being tested is about one audio thread.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct AllocationWatch {
    armed: bool,
    allocations: usize,
    bytes: usize,
    largest: usize,
}

thread_local! {
    /// Armed for exactly as long as a `render()` call is on this thread's stack. `const`
    /// initialised, `Copy` and destructor-free, so reading it never allocates and never
    /// has to lazily initialise anything.
    static WATCH: Cell<AllocationWatch> = const { Cell::new(AllocationWatch { armed: false, allocations: 0, bytes: 0, largest: 0 }) };
}

/// The system allocator, plus a per-thread counter that only moves while [`WATCH`] is
/// armed.
struct RenderGuardAllocator;

// SAFETY: both methods forward to `System` unchanged; the wrapper only reads and writes a
// destructor-free thread-local `Cell` of `Copy` data. Nothing it adds can allocate, so the
// hook cannot recurse into itself.
unsafe impl GlobalAlloc for RenderGuardAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        // SAFETY: `layout` is the caller's, forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // A `free()` inside the audio callback is the very stall the garbage channel
        // exists to prevent, so a deallocation counts as a violation too.
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
static ALLOCATOR: RenderGuardAllocator = RenderGuardAllocator;

/// What the hook saw during one armed region.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct AllocationReport {
    allocations: usize,
    bytes: usize,
    largest: usize,
}

impl AllocationReport {
    fn is_clean(self) -> bool { self.allocations == 0 }
}

/// Run `body` with the hook armed on this thread, and report what it saw.
///
/// The counters are reset on entry rather than on exit, so a violation is attributed to
/// this region and never to the previous one.
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

// ── the corpus ──────────────────────────────────────────────────────────────────────

/// A module to push through `render()`, and where it came from.
struct CorpusEntry {
    name: String,
    format: ModuleFormat,
    bytes: Vec<u8>,
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the workspace root is reachable from this crate")
}

/// Which loader owns these bytes, decided the way a host's format probe would.
fn classify(bytes: &[u8]) -> Option<ModuleFormat> {
    if starplayer::it::probe(bytes) {
        return Some(ModuleFormat::It);
    }
    if starplayer::s3m::probe(bytes) {
        return Some(ModuleFormat::S3m);
    }
    if starplayer::mtm::probe(bytes) {
        return Some(ModuleFormat::Mtm);
    }
    if starplayer::xm::probe(bytes) {
        return Some(ModuleFormat::Xm);
    }
    if starplayer::mod_file::probe(bytes) {
        return Some(ModuleFormat::Mod);
    }
    None
}

fn collect_directory(directory: &Path, entries: &mut Vec<CorpusEntry>) {
    let Ok(listing) = fs::read_dir(directory) else { return };
    let mut paths: Vec<PathBuf> = listing.filter_map(|entry| entry.ok().map(|entry| entry.path())).filter(|path| path.is_file()).collect();
    paths.sort();
    for path in paths {
        let Ok(bytes) = fs::read(&path) else { continue };
        let Some(format) = classify(&bytes) else { continue };
        let name = path.strip_prefix(repository_root()).unwrap_or(&path).display().to_string();
        entries.push(CorpusEntry { name, format, bytes });
    }
}

/// The cached pinned libxmp corpus, if `cargo xtask conformance --fetch-only` has run.
fn pinned_corpus_directory() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = fs::read_dir(repository_root().join("target/conformance/corpora"))
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .map(|path| path.join("test-dev/data"))
        .filter(|path| path.is_dir())
        .collect();
    candidates.sort();
    candidates.pop()
}

/// Everything this repository can reach: the committed fuzz seeds and crash regressions,
/// the owner's S3Ms, the synthesised golden fixtures, and the pinned libxmp corpus when
/// it is cached.
fn corpus() -> Vec<CorpusEntry> {
    let root = repository_root();
    let mut entries = vec![
        CorpusEntry { name: "fixtures::synthetic_mod".to_string(), format: ModuleFormat::Mod, bytes: starplayer_offline::fixtures::synthetic_mod() },
        CorpusEntry { name: "fixtures::synthetic_mtm".to_string(), format: ModuleFormat::Mtm, bytes: starplayer_offline::fixtures::synthetic_mtm() },
        CorpusEntry { name: "fixtures::synthetic_xm".to_string(), format: ModuleFormat::Xm, bytes: starplayer_offline::fixtures::synthetic_xm() },
        CorpusEntry { name: "fixtures::synthetic_it".to_string(), format: ModuleFormat::It, bytes: starplayer_offline::fixtures::synthetic_it() },
    ];

    for format in ["mod", "s3m", "mtm", "xm", "it"] {
        collect_directory(&root.join("fuzz/seeds").join(format), &mut entries);
        collect_directory(&root.join("fuzz/regressions").join(format), &mut entries);
    }
    collect_directory(&root.join("crates/starplayer-s3m/tests/fixtures"), &mut entries);

    match pinned_corpus_directory() {
        Some(directory) => collect_directory(&directory, &mut entries),
        None => assert!(
            std::env::var_os("STARPLAYER_REQUIRE_CORPUS").is_none(),
            "STARPLAYER_REQUIRE_CORPUS is set but the pinned corpus is not cached; run `cargo xtask conformance --fetch-only`",
        ),
    }
    entries
}

// ── the engine under the hook ───────────────────────────────────────────────────────

type CorpusEngine = Engine<FixedPath, Linear, StereoI16, Arc<Module>>;

const SAMPLE_RATE_HZ: u32 = 44_100;

/// Frames rendered per module: a quarter of a second, which at 125 BPM and speed 6 is
/// around thirteen tracker ticks — enough for the sequencer to advance rows, retrigger
/// voices and cross many `RENDER_QUANTUM` boundaries.
const FRAMES_PER_MODULE: usize = SAMPLE_RATE_HZ as usize / 4;

fn load(format: ModuleFormat, bytes: &[u8]) -> Option<Module> {
    match format {
        ModuleFormat::Mod => starplayer::mod_file::load(bytes).ok(),
        ModuleFormat::S3m => starplayer::s3m::load(bytes).ok(),
        ModuleFormat::Mtm => starplayer::mtm::load(bytes).ok(),
        ModuleFormat::Xm => starplayer::xm::load(bytes).ok(),
        ModuleFormat::It => starplayer::it::load(bytes).ok(),
    }
}

fn source_for(format: ModuleFormat, module: Arc<Module>) -> Option<Box<dyn EventSource>> {
    match format {
        ModuleFormat::Mod => Some(Box::new(starplayer::mod_file::sequencer_for(module, SAMPLE_RATE_HZ, ExactFixedPoint))),
        ModuleFormat::S3m => Some(Box::new(starplayer::s3m::sequencer_for(module, SAMPLE_RATE_HZ, ExactFixedPoint))),
        ModuleFormat::Mtm => Some(Box::new(starplayer::mtm::sequencer_for(module, SAMPLE_RATE_HZ, ExactFixedPoint))),
        ModuleFormat::Xm => Some(Box::new(starplayer::xm::sequencer_for(module, SAMPLE_RATE_HZ, ExactFixedPoint))),
        // IT is built through `sequencer_with_quirks` rather than `sequencer_for`, because
        // its tempo model is part of the dialect (accuracy policy §2) rather than the
        // caller's choice.
        ModuleFormat::It => Some(Box::new(starplayer::it::sequencer_with_quirks(module, SAMPLE_RATE_HZ, starplayer::core::quirks::QuirkSelection::FromDialect))),
    }
}

/// Build an engine on `entry` and render [`FRAMES_PER_MODULE`] frames with the hook armed,
/// at three host block sizes so the output ring's partial-quantum path is covered as well
/// as the aligned one.
fn render_under_the_hook(entry: &CorpusEntry) -> Option<AllocationReport> {
    let module = Arc::new(load(entry.format, &entry.bytes)?);
    let channel_count = module.header().channel_count as usize;
    let settings = EngineSettings {
        sample_rate_hz: SAMPLE_RATE_HZ,
        channel_count,
        // IT sounds several voices per channel, so the pool is the format's own answer.
        voice_capacity: starplayer::recommended_voice_capacity(&module).max(1),
        ..EngineSettings::default()
    };

    let mut worst = AllocationReport::default();
    for host_block_frames in [128usize, 37, 4096] {
        let mut engine: CorpusEngine = Engine::with_settings(settings);
        let mut control = engine.take_control().expect("a fresh engine owns its control handle");
        control.load_module(Arc::clone(&module)).map_err(|_| "full").expect("the ring has room");
        engine.set_source(source_for(entry.format, Arc::clone(&module))?);

        // Allocated before the hook is armed, exactly as a host allocates its output
        // buffer once and reuses it for every callback.
        let mut block = vec![0i16; host_block_frames * 2];
        let blocks = FRAMES_PER_MODULE.div_ceil(host_block_frames);
        let (_, report) = while_watching_for_allocations(|| {
            for _ in 0..blocks {
                engine.render(&mut block);
            }
        });
        if report.allocations > worst.allocations {
            worst = report;
        }
        // Retired handles die here, on the control thread, after the hook is disarmed —
        // which is the whole point of the garbage channel.
        control.collect_all_garbage();
    }
    Some(worst)
}

#[test]
fn render_allocates_nothing_for_any_module_in_the_corpus() {
    let corpus = corpus();
    assert!(corpus.len() >= 25, "the corpus collapsed to {} modules", corpus.len());

    let mut offenders = Vec::new();
    let mut rendered = 0usize;
    for entry in &corpus {
        let Some(report) = render_under_the_hook(entry) else { continue };
        rendered += 1;
        if !report.is_clean() {
            offenders.push(format!("{}: {} allocations, {} bytes, largest {}", entry.name, report.allocations, report.bytes, report.largest));
        }
    }

    println!("render allocation hook: rendered {rendered} of {} corpus modules", corpus.len());
    assert!(rendered >= 25, "only {rendered} corpus modules loaded");
    assert!(offenders.is_empty(), "render() allocated:\n{}", offenders.join("\n"));
}

/// The swap is the riskiest moment in the render loop: it releases every voice, forgets
/// every binding and pushes the retired `Arc<Module>` down the garbage channel. If any of
/// that allocated or freed, a host that changes tune would glitch.
/// Task G3's dense-IT check: the pinned corpus's three `data/m/*.it` modules rendered for
/// thirty seconds each, with the hook armed and the engine's warning flags read at the
/// end, reporting the peak number of voices sounding at once.
///
/// These are real Impulse Tracker songs rather than a fixture, so they are the only thing
/// in this repository that exercises New Note Actions, the duplicate check and voice
/// stealing under load. The test is skipped when the pinned corpus is not cached, exactly
/// as the corpus sweep above is.
#[test]
fn the_dense_it_modules_render_for_thirty_seconds_without_allocating() {
    const SECONDS: usize = 30;
    let Some(data) = pinned_corpus_directory() else {
        assert!(std::env::var_os("STARPLAYER_REQUIRE_CORPUS").is_none(), "STARPLAYER_REQUIRE_CORPUS is set but the pinned corpus is not cached");
        return;
    };
    let directory = data.join("m");
    let Ok(listing) = fs::read_dir(&directory) else { return };
    let mut paths: Vec<PathBuf> = listing
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("it")))
        .collect();
    paths.sort();
    assert_eq!(paths.len(), 3, "the pinned corpus carries three dense IT modules");

    for path in paths {
        let bytes = fs::read(&path).expect("a listed corpus file is readable");
        let module = Arc::new(starplayer::it::load(&bytes).expect("the dense IT loads"));
        let settings = EngineSettings {
            sample_rate_hz: SAMPLE_RATE_HZ,
            channel_count: (module.header().channel_count as usize).max(1),
            voice_capacity: starplayer::recommended_voice_capacity(&module).max(1),
            ..EngineSettings::default()
        };
        let mut engine: CorpusEngine = Engine::with_settings(settings);
        let mut control = engine.take_control().expect("a fresh engine has its control end");
        control.load_module(Arc::clone(&module)).expect("a fresh command queue accepts the module");
        let quirks = starplayer::core::quirks::QuirkSelection::FromDialect;
        engine.set_source(Box::new(starplayer::it::sequencer_with_quirks(Arc::clone(&module), SAMPLE_RATE_HZ, quirks)));

        let mut block = vec![0i16; 1024 * 2];
        let blocks = (SECONDS * SAMPLE_RATE_HZ as usize).div_ceil(1024);
        let (peak, report) = while_watching_for_allocations(|| {
            let mut peak = 0usize;
            for _ in 0..blocks {
                engine.render(&mut block);
                peak = peak.max(engine.voices().voices_active());
            }
            peak
        });
        let warnings = engine.warnings();
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        println!("{name}: {} channels, peak {peak} voices over {SECONDS}s", module.header().channel_count);
        assert!(report.is_clean(), "{name} allocated inside render(): {report:?}");
        assert!(!warnings.any(), "{name} raised an engine warning: {warnings:?}");
    }
}

#[test]
fn swapping_a_module_inside_render_allocates_nothing() {
    let first = Arc::new(starplayer::mod_file::load(&starplayer_offline::fixtures::synthetic_mod()).expect("the synthesised MOD loads"));
    let second = Arc::new(starplayer::mtm::load(&starplayer_offline::fixtures::synthetic_mtm()).expect("the synthesised MTM loads"));

    let settings = EngineSettings { sample_rate_hz: SAMPLE_RATE_HZ, channel_count: 8, voice_capacity: 8, ..EngineSettings::default() };
    let mut engine: CorpusEngine = Engine::with_settings(settings);
    let mut control = engine.take_control().expect("a fresh engine owns its control handle");
    control.load_module(Arc::clone(&first)).map_err(|_| "full").expect("the ring has room");
    engine.set_source(source_for(ModuleFormat::Mod, Arc::clone(&first)).expect("a MOD sequencer"));

    let mut block = vec![0i16; 128 * 2];
    engine.render(&mut block);

    control.load_module(Arc::clone(&second)).map_err(|_| "full").expect("the ring has room");
    let (_, report) = while_watching_for_allocations(|| engine.render(&mut block));

    assert!(report.is_clean(), "swapping a module inside render() allocated: {report:?}");
    assert_eq!(control.pending_garbage(), 1, "the retired module went down the garbage channel");
    assert!(!engine.warnings().retired_module_dropped, "and did not have to be dropped inline");
    assert_eq!(control.collect_all_garbage(), 1, "the control thread is where it dies");
}

/// M7-H1: the insert graph is inside `render()` too.
///
/// A gain insert on **every** channel bus and one on the master, with a stream of
/// `SetParam`s arriving while the render runs — which is the shape a host's automation
/// takes. Nothing here may allocate: the effects were built on this thread before the hook
/// was armed, the parameter changes are `i32`s on a preallocated ring, and the smoothing
/// they start is arithmetic on state the effect already owns.
#[test]
fn rendering_with_an_insert_on_every_channel_allocates_nothing() {
    let module = Arc::new(starplayer::mod_file::load(&starplayer_offline::fixtures::synthetic_mod()).expect("the synthesised MOD loads"));
    let channel_count = (module.header().channel_count as usize).max(1);
    let settings = EngineSettings {
        sample_rate_hz: SAMPLE_RATE_HZ,
        channel_count,
        voice_capacity: starplayer::recommended_voice_capacity(&module).max(1),
        // Wide enough to install every chain before the first render, so the installs are
        // not what the drain limit is spent on.
        insert_command_capacity: 128,
        ..EngineSettings::default()
    };
    let mut engine: CorpusEngine = Engine::with_settings(settings);
    let mut control = engine.take_control().expect("a fresh engine owns its control handle");
    let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
    control.load_module(Arc::clone(&module)).map_err(|_| "full").expect("the ring has room");
    engine.set_source(source_for(ModuleFormat::Mod, Arc::clone(&module)).expect("a MOD sequencer"));

    // Every `Box::new` happens here, on the control thread, before the hook is armed.
    for channel in 0..channel_count {
        let insert = build_insert::<i32>(InsertKind::Gain, SAMPLE_RATE_HZ);
        inserts.install(InsertTarget::Channel(ChannelId(channel as u16)), 0, insert).map_err(|_| "full").expect("the ring has room");
    }
    inserts.install(InsertTarget::Master, 0, build_insert::<i32>(InsertKind::Gain, SAMPLE_RATE_HZ)).map_err(|_| "full").expect("the ring has room");

    let mut block = vec![0i16; 128 * 2];
    let blocks = FRAMES_PER_MODULE.div_ceil(128);
    let (_, report) = while_watching_for_allocations(|| {
        for index in 0..blocks {
            // A slider being dragged: one parameter change per host callback, walking the
            // channels so every chain is touched.
            let target = InsertTarget::Channel(ChannelId((index % channel_count) as u16));
            let value = -100 - (index % 40) as i32 * 100;
            let _ = inserts.send(InsertCommand::SetParam { target, slot: 0, param: GAIN_PARAM, value });
            engine.render(&mut block);
        }
    });

    assert!(report.is_clean(), "render() with the insert graph live allocated: {report:?}");
    assert!(!engine.warnings().any(), "the insert graph raised an engine warning: {:?}", engine.warnings());
    assert_eq!(inserts.pending_garbage(), 0, "nothing was retired, so nothing is waiting");
    control.collect_all_garbage();
    inserts.collect_all_garbage();
}

/// M7-H3: the same, with the three stateful effects installed.
///
/// The gain insert allocates nothing to build and has no state, so the H1 test above could
/// not have caught an effect that allocated a ring buffer lazily on its first `process`, or
/// that grew a `Vec` while re-cooking. An EQ, a delay and a chorus between them cover both:
/// the delay and the chorus each allocate a ring at construction and must never touch the
/// allocator again, and the EQ re-cooks three RBJ biquads every sixteen frames throughout.
/// The sweep queued below keeps all three re-cooking for the whole render rather than
/// settling into the branch where nothing moves.
#[test]
fn rendering_with_an_eq_a_delay_and_a_chorus_allocates_nothing() {
    let module = Arc::new(starplayer::mod_file::load(&starplayer_offline::fixtures::synthetic_mod()).expect("the synthesised MOD loads"));
    let channel_count = (module.header().channel_count as usize).max(1);
    let settings = EngineSettings {
        sample_rate_hz: SAMPLE_RATE_HZ,
        channel_count,
        voice_capacity: starplayer::recommended_voice_capacity(&module).max(1),
        insert_command_capacity: 128,
        ..EngineSettings::default()
    };
    let mut engine: CorpusEngine = Engine::with_settings(settings);
    let mut control = engine.take_control().expect("a fresh engine owns its control handle");
    let mut inserts = engine.take_insert_control().expect("a fresh engine owns its insert handle");
    control.load_module(Arc::clone(&module)).map_err(|_| "full").expect("the ring has room");
    engine.set_source(source_for(ModuleFormat::Mod, Arc::clone(&module)).expect("a MOD sequencer"));

    // Every ring buffer this test will ever use is allocated here, before the hook is armed.
    let equalised = InsertTarget::Channel(ChannelId(0));
    inserts.install(equalised, 0, build_insert::<i32>(InsertKind::Eq, SAMPLE_RATE_HZ)).map_err(|_| "full").expect("the ring has room");
    let delayed = InsertTarget::Channel(ChannelId((channel_count.saturating_sub(1)) as u16));
    inserts.install(delayed, 1, build_insert::<i32>(InsertKind::Delay, SAMPLE_RATE_HZ)).map_err(|_| "full").expect("the ring has room");
    inserts.install(InsertTarget::Master, 0, build_insert::<i32>(InsertKind::Chorus, SAMPLE_RATE_HZ)).map_err(|_| "full").expect("the ring has room");

    let mut block = vec![0i16; 128 * 2];
    let blocks = FRAMES_PER_MODULE.div_ceil(128);
    let (_, report) = while_watching_for_allocations(|| {
        for index in 0..blocks {
            // A slider being dragged on the EQ's bell, so its cooking branch is live the
            // whole way through rather than settling after the first ramp.
            let value = -2_400 + (index % 49) as i32 * 100;
            let _ = inserts.send(InsertCommand::SetParam { target: equalised, slot: 0, param: EQ_PEAK_GAIN_PARAM, value });
            engine.render(&mut block);
        }
    });

    assert!(report.is_clean(), "render() with the H3 effects live allocated: {report:?}");
    assert!(!engine.warnings().any(), "the insert graph raised an engine warning: {:?}", engine.warnings());
    control.collect_all_garbage();
    inserts.collect_all_garbage();
}

/// The hook has to be able to fail, or its silence means nothing. This is C7's deliberate
/// `Vec::push` violation, made permanent instead of performed once by hand.
#[test]
fn the_hook_itself_reports_an_allocation_made_inside_the_armed_region() {
    let (_, clean) = while_watching_for_allocations(|| std::hint::black_box(1u32 + 1));
    assert!(clean.is_clean(), "an armed region that allocates nothing reports nothing: {clean:?}");

    let (_, dirty) = while_watching_for_allocations(|| {
        // The shape C7 asks for: a growing collection on a path that "only" gathers
        // diagnostics. `vec![]` would be one allocation the optimiser could hoist; a loop
        // of pushes is what such a path actually looks like.
        let mut growing: Vec<u64> = Vec::new();
        for value in 0..4u64 {
            growing.push(value);
        }
        std::hint::black_box(growing.len())
    });
    assert!(!dirty.is_clean(), "a Vec::push inside the armed region must be reported");
    assert!(dirty.bytes >= 8, "and its size recorded: {dirty:?}");
}

/// Allocation on another thread is not this thread's violation. Without this the hook
/// would be useless under `cargo test`, which runs test functions in parallel threads.
#[test]
fn allocation_on_another_thread_is_not_a_violation() {
    let (_, report) = while_watching_for_allocations(|| {
        std::thread::spawn(|| vec![7u8; 1 << 20].len()).join().expect("the helper thread finished")
    });
    // `spawn` and `join` allocate on *this* thread, so the assertion is about the
    // megabyte the other thread asked for, not about a clean report.
    assert!(report.largest < 1 << 20, "another thread's megabyte was attributed to this one: {report:?}");
}

/// Task E4's musical path and E7's two-source mux under the same hook.
///
/// `MidiSource` brings two things the tracker path does not have — an `InstrumentRack`
/// holding `Box<dyn Instrument>`s, and events arriving from another thread over an SPSC
/// ring — and both are new opportunities to allocate inside the callback. The rack is
/// built off the audio thread, the ring is allocated once, and the queue's one-event
/// lookahead is a `Copy` field, so `render()` must stay clean with a live keyboard playing
/// through it.
#[test]
fn rendering_a_tracker_and_midi_source_mux_allocates_nothing() {
    use starplayer::core::{Event, Frame, Note, TimedEvent, U0F16};
    use starplayer::engine::{InstrumentRack, MidiSource, SourceMux, external_event_channel, midi_channel};

    let module = Arc::new(starplayer::it::load(&starplayer_offline::fixtures::synthetic_it()).expect("the synthesised IT loads"));
    let settings = EngineSettings {
        sample_rate_hz: SAMPLE_RATE_HZ,
        // The MIDI lanes sit above a module's, so the table has to be the full width.
        channel_count: starplayer::engine::ChannelTable::MAX_CHANNELS,
        voice_capacity: starplayer::recommended_voice_capacity(&module).saturating_add(16),
        ..EngineSettings::default()
    };

    for host_block_frames in [128usize, 37, 4096] {
        let mut engine: CorpusEngine = Engine::with_settings(settings);
        let mut control = engine.take_control().expect("a fresh engine owns its control handle");
        control.load_module(Arc::clone(&module)).map_err(|_| "full").expect("the ring has room");

        // Everything the audio thread will touch is allocated here, before it goes over:
        // the rack's boxed instruments, the event ring, and the source itself.
        let rack = InstrumentRack::for_module(&module, SAMPLE_RATE_HZ);
        let (mut producer, queue) = external_event_channel(256);
        let midi_source = MidiSource::new(queue, rack, SAMPLE_RATE_HZ);
        let tracker_source = source_for(ModuleFormat::It, Arc::clone(&module)).expect("an IT sequencer");
        let mut mux = SourceMux::new(2);
        assert!(mux.insert(tracker_source).is_ok(), "slot 0 is free");
        assert!(mux.insert(Box::new(midi_source)).is_ok(), "slot 1 is free");
        engine.set_source(Box::new(mux));

        // A chord per eighth of the render, held and released — enough note-ons, note-offs
        // and controller writes that a per-event allocation could not hide.
        let step_frames = (FRAMES_PER_MODULE / 16) as u64;
        for index in 0..8u64 {
            let channel = midi_channel((index % 4) as u8);
            let note = Note::new(48 + (index as u8 * 5) % 36);
            let on = Event::NoteOn { note, velocity: U0F16::MAX };
            let off = Event::NoteOff { note, velocity: U0F16::ZERO };
            producer.send(TimedEvent::on_channel(Frame(index * step_frames + 7), channel, on)).expect("room in the ring");
            producer.send(TimedEvent::on_channel(Frame(index * step_frames + step_frames / 2), channel, Event::PitchBend(starplayer::core::I1F15::from_bits(9_000)))).expect("room");
            producer.send(TimedEvent::on_channel(Frame((index + 1) * step_frames - 3), channel, off)).expect("room in the ring");
        }

        let mut block = vec![0i16; host_block_frames * 2];
        let blocks = FRAMES_PER_MODULE.div_ceil(host_block_frames);
        let (_, report) = while_watching_for_allocations(|| {
            for _ in 0..blocks {
                engine.render(&mut block);
            }
        });
        assert!(report.is_clean(), "render() allocated with a MIDI source at block size {host_block_frames}: {report:?}");
        assert!(engine.voices().voices_active() > 0 || engine.frame() > Frame(0), "the source drove the engine");
        control.collect_all_garbage();
    }
}

/// Task E5's arm: `SmfSequencer` feeding the same `MidiSource` the test above proved
/// clean, this time from a parsed Standard MIDI File rather than a live event ring.
/// Parsing the file and converting it to frames both allocate, off the audio thread,
/// before the watched region begins — exactly as building the previous test's rack and
/// ring does.
#[test]
fn rendering_an_smf_source_allocates_nothing() {
    use starplayer::core::Frame;
    use starplayer::engine::{InstrumentRack, MidiSource};
    use starplayer::midi::SmfSequencer;
    use starplayer::midi::smf::parse_smf;

    // One track, four channels, eight note on/off pairs and a controller message, all
    // with explicit status bytes and small delta-times — enough channel voice traffic
    // that a per-event allocation could not hide, without needing running status here
    // (that shape is `starplayer-midi`'s own test suite's job).
    let mut track = vec![0x00, 0x90, 60, 100, 0x00, 0xB0, 7, 100, 0x03, 0x80, 60, 0];
    for (index, channel_status) in [0x91u8, 0x92, 0x93].into_iter().enumerate() {
        let note = 65 + index as u8 * 5;
        track.extend_from_slice(&[0x00, channel_status, note, 100, 0x06, channel_status - 0x10, note, 0]);
    }
    track.extend_from_slice(&[0x00, 0xFF, 0x2F, 0x00]); // end of track
    let mut smf_bytes = Vec::new();
    smf_bytes.extend_from_slice(b"MThd");
    smf_bytes.extend_from_slice(&6u32.to_be_bytes());
    smf_bytes.extend_from_slice(&0u16.to_be_bytes()); // format 0
    smf_bytes.extend_from_slice(&1u16.to_be_bytes()); // one track
    smf_bytes.extend_from_slice(&96u16.to_be_bytes()); // 96 PPQN
    smf_bytes.extend_from_slice(b"MTrk");
    smf_bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
    smf_bytes.extend_from_slice(&track);

    let module = Arc::new(starplayer::it::load(&starplayer_offline::fixtures::synthetic_it()).expect("the synthesised IT loads"));
    let settings = EngineSettings {
        sample_rate_hz: SAMPLE_RATE_HZ,
        channel_count: starplayer::engine::ChannelTable::MAX_CHANNELS,
        voice_capacity: starplayer::recommended_voice_capacity(&module).max(16),
        ..EngineSettings::default()
    };
    let smf = parse_smf(&smf_bytes).expect("the hand-assembled file parses");

    for host_block_frames in [128usize, 37, 4096] {
        let mut engine: CorpusEngine = Engine::with_settings(settings);
        let mut control = engine.take_control().expect("a fresh engine owns its control handle");
        control.load_module(Arc::clone(&module)).map_err(|_| "full").expect("the ring has room");

        // Everything the audio thread will touch is allocated here, before it goes over:
        // the rack's boxed instruments, the sequencer's converted frame list, and the
        // source itself.
        let rack = InstrumentRack::for_module(&module, SAMPLE_RATE_HZ);
        let sequencer = SmfSequencer::new(&smf, SAMPLE_RATE_HZ);
        engine.set_source(Box::new(MidiSource::new(sequencer, rack, SAMPLE_RATE_HZ)));

        let mut block = vec![0i16; host_block_frames * 2];
        let blocks = FRAMES_PER_MODULE.div_ceil(host_block_frames);
        let (_, report) = while_watching_for_allocations(|| {
            for _ in 0..blocks {
                engine.render(&mut block);
            }
        });
        assert!(report.is_clean(), "render() allocated with an SMF source at block size {host_block_frames}: {report:?}");
        assert!(engine.voices().voices_active() > 0 || engine.frame() > Frame(0), "the source drove the engine");
        control.collect_all_garbage();
    }
}
