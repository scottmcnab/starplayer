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

use starplayer::core::ExactFixedPoint;
use starplayer::dsp::Linear;
use starplayer::engine::{Engine, EngineSettings, EventSource};
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
    if starplayer::s3m::probe(bytes) {
        return Some(ModuleFormat::S3m);
    }
    if starplayer::mtm::probe(bytes) {
        return Some(ModuleFormat::Mtm);
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
    let mut entries = Vec::new();

    entries.push(CorpusEntry { name: "fixtures::synthetic_mod".to_string(), format: ModuleFormat::Mod, bytes: starplayer_offline::fixtures::synthetic_mod() });
    entries.push(CorpusEntry { name: "fixtures::synthetic_mtm".to_string(), format: ModuleFormat::Mtm, bytes: starplayer_offline::fixtures::synthetic_mtm() });

    for format in ["mod", "s3m", "mtm"] {
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
        _ => None,
    }
}

fn source_for(format: ModuleFormat, module: Arc<Module>) -> Option<Box<dyn EventSource>> {
    match format {
        ModuleFormat::Mod => Some(Box::new(starplayer::mod_file::sequencer_for(module, SAMPLE_RATE_HZ, ExactFixedPoint))),
        ModuleFormat::S3m => Some(Box::new(starplayer::s3m::sequencer_for(module, SAMPLE_RATE_HZ, ExactFixedPoint))),
        ModuleFormat::Mtm => Some(Box::new(starplayer::mtm::sequencer_for(module, SAMPLE_RATE_HZ, ExactFixedPoint))),
        _ => None,
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
        voice_capacity: channel_count.max(1),
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
