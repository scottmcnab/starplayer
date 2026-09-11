//! Design goal 5, measured rather than asserted: nothing reachable from
//! [`RenderHalf::render`] allocates — and what [`EmbeddedPlayer::open`] *does* allocate is
//! counted, because that number is the budget document I3 has to write.
//!
//! The allocator hook is `starplayer-offline`'s `tests/render_allocation.rs` pattern: a
//! `GlobalAlloc` wrapper over a per-thread `Cell` that is armed for exactly as long as a
//! call is on the stack. It **records** rather than panics, because panicking from inside
//! `GlobalAlloc::alloc` re-enters the allocator to box the payload; the assertion happens
//! the moment the armed region returns, so a violation is reported with the module that
//! caused it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use starplayer::dsp::Linear;
use starplayer::engine::EngineSettings;
use starplayer::mixer::Voice;
use starplayer::telemetry::Snapshot;
use starplayer::rt::Arc;
use starplayer_host_embedded::{EmbeddedPlayer, OUTPUT_CHANNELS, RenderHalf, settings_for};

// ── the allocator hook ──────────────────────────────────────────────────────────────

/// What the hook saw during one armed region, and whether it is armed at all.
///
/// Everything is per thread: `cargo test` runs these functions in parallel, so a
/// process-wide counter could not answer "did *this* call allocate" at all.
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

struct RecordingAllocator;

// SAFETY: both methods forward to `System` unchanged; the wrapper only reads and writes a
// destructor-free thread-local `Cell` of `Copy` data. Nothing it adds can allocate, so the
// hook cannot recurse into itself.
unsafe impl GlobalAlloc for RecordingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size(), true);
        // SAFETY: `layout` is the caller's, forwarded unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // A `free()` inside the DMA refill is the very stall the garbage channel exists to
        // prevent, so a deallocation counts as a violation too.
        record(layout.size(), false);
        // SAFETY: `pointer` and `layout` are the caller's, forwarded unchanged, and this
        // allocator only ever hands out blocks obtained from `System`.
        unsafe { System.dealloc(pointer, layout) }
    }
}

fn record(size: usize, allocating: bool) {
    let _ = WATCH.try_with(|watch| {
        let mut current = watch.get();
        if !current.armed {
            return;
        }
        current.allocations = current.allocations.saturating_add(1);
        if allocating {
            current.bytes = current.bytes.saturating_add(size);
            current.largest = current.largest.max(size);
        }
        watch.set(current);
    });
}

#[global_allocator]
static ALLOCATOR: RecordingAllocator = RecordingAllocator;

/// Run `body` with the hook armed on this thread, and report what it saw.
fn while_watching<T>(body: impl FnOnce() -> T) -> (T, AllocationWatch) {
    WATCH.with(|watch| watch.set(AllocationWatch { armed: true, ..AllocationWatch::default() }));
    let result = body();
    let seen = WATCH.with(|watch| {
        let seen = watch.get();
        watch.set(AllocationWatch::default());
        seen
    });
    (result, seen)
}

// ── the corpus ──────────────────────────────────────────────────────────────────────

fn corpus() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("fixtures::synthetic_mod", starplayer_offline::fixtures::synthetic_mod()),
        ("fixtures::synthetic_mtm", starplayer_offline::fixtures::synthetic_mtm()),
        ("fixtures::synthetic_xm", starplayer_offline::fixtures::synthetic_xm()),
        ("fixtures::synthetic_it", starplayer_offline::fixtures::synthetic_it()),
        ("PETRI.S3M", include_bytes!("../../starplayer-s3m/tests/fixtures/PETRI.S3M").to_vec()),
        ("REFLEX.S3M", include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M").to_vec()),
    ]
}

fn module(bytes: &[u8]) -> Arc<starplayer::model::Module> {
    Arc::new(starplayer::load(bytes).expect("the fixture loads"))
}

// ── design goal 5 ───────────────────────────────────────────────────────────────────

#[test]
fn rendering_through_the_embedded_host_allocates_nothing() {
    for (name, bytes) in corpus() {
        let (mut render, mut control) = EmbeddedPlayer::<Linear>::open(module(&bytes), 44_100).expect("it opens");
        control.play().expect("play");
        // Every block size the invariant is stated over, so a partial quantum, a partial
        // frame and a multi-quantum block are all on the stack with the hook armed.
        for block_frames in [1, 3, 64, 128, 4_096, 8_191] {
            let mut output = vec![0i16; block_frames * OUTPUT_CHANNELS];
            let (_, seen) = while_watching(|| render.render(&mut output));
            assert_eq!(seen.allocations, 0, "{name} at block size {block_frames} allocated: {seen:?}");
        }
        // A load retires a module and a sequencer through the ring; moving them must not
        // allocate either, and collecting them is the control side's business.
        control.load(module(&bytes)).expect("second load");
        let mut output = vec![0i16; 128 * OUTPUT_CHANNELS];
        for _ in 0..4 {
            let (_, seen) = while_watching(|| render.render(&mut output));
            assert_eq!(seen.allocations, 0, "{name} allocated while retiring a module: {seen:?}");
        }
        assert_eq!(control.collect_garbage(), 2, "{name}: the retirements came back to the control side");
    }
}

#[test]
fn a_ragged_render_that_ends_mid_frame_allocates_nothing_either() {
    let (mut render, mut control) = EmbeddedPlayer::<Linear>::open(module(&corpus()[5].1), 44_100).expect("it opens");
    control.play().expect("play");
    let mut output = vec![0i16; 2_001];
    for _ in 0..8 {
        let (_, seen) = while_watching(|| render.render(&mut output));
        assert_eq!(seen.allocations, 0, "a block that is not a whole number of frames allocated: {seen:?}");
    }
}

// ── the heap budget (research point 3, and deliverable 1's formula) ─────────────────

/// The per-voice and per-channel costs, derived rather than asserted from a table: build
/// two engines that differ in exactly one setting and difference them.
///
/// This is research point 3's answer, and the numbers deliverable 1's formula quotes.
#[test]
fn the_heap_cost_of_an_engine_is_linear_in_the_voice_and_channel_counts() {
    let base = EngineSettings { sample_rate_hz: 44_100, voice_capacity: 32, channel_count: 16, ..EngineSettings::default() };
    let measure = |settings: EngineSettings| {
        let (_, seen) = while_watching(|| EmbeddedPlayer::<Linear>::open_empty(44_100, settings).expect("it opens"));
        seen.bytes
    };

    let baseline = measure(base);
    let more_voices = measure(EngineSettings { voice_capacity: base.voice_capacity + 32, ..base });
    let more_channels = measure(EngineSettings { channel_count: base.channel_count + 16, ..base });

    let per_voice = (more_voices - baseline) / 32;
    let per_channel = (more_channels - baseline) / 16;
    let fixed = baseline - per_voice * base.voice_capacity - per_channel * base.channel_count;
    println!(
        "embedded host heap: {baseline} bytes at {} voices / {} channels\n  {per_voice} bytes per voice (size_of::<Voice>() = {})\n  {per_channel} bytes per channel\n  {fixed} bytes fixed (size_of::<Snapshot>() = {})",
        base.voice_capacity,
        base.channel_count,
        size_of::<Voice>(),
        size_of::<Snapshot>(),
    );

    // Research point 3: a `Voice` carries a `PathFilter<f32>` that the fixed path never
    // reads — two `f32` of delay line and a `FilterCoefficients<f32>` — and that dead
    // state is inside every one of these bytes. The pool's slot is a little wider than the
    // voice itself, because it also carries the free-list link.
    assert!((size_of::<Voice>()..size_of::<Voice>() + 32).contains(&per_voice), "the voice pool is the only per-voice allocation: {per_voice} against {}", size_of::<Voice>());
    // Ranges rather than equalities, so that a field added to a `Voice` or a `Channel` is a
    // review conversation about the budget rather than a red test — but a *structural*
    // change, such as a second per-channel ring, moves these and should.
    assert!((4_500..6_000).contains(&per_channel), "the per-channel cost moved: {per_channel} bytes");
    assert!((20_000..36_000).contains(&fixed), "the fixed cost moved: {fixed} bytes");

    // Linear in both, which is what makes the formula in `settings_for` a formula.
    let doubled = measure(EngineSettings { voice_capacity: 64, channel_count: 32, ..base });
    assert_eq!(doubled, fixed + per_voice * 64 + per_channel * 32, "the cost is not linear in the two counts");

    // And the formula `settings_for`'s documentation quotes is *this* formula, so a change
    // to any of the three constants fails here and is fixed in both places at once.
    assert_eq!((fixed, per_voice, per_channel), (27_800, 184, 5_288), "the heap formula in `settings_for`'s documentation is out of date");

    // A `RenderHalf` is not small **by value**: the engine's telemetry publisher holds a
    // working `Snapshot` inline and this host holds another. A firmware that moves one down
    // a call chain rather than into a `static` pays for it in stack.
    println!("size_of::<RenderHalf<Linear>>() = {}", size_of::<RenderHalf<Linear>>());
}

/// A real module's engine, sized by [`settings_for`], with the numbers printed for the
/// budget document.
#[test]
fn a_real_module_reports_what_it_costs() {
    for (name, bytes) in corpus() {
        let module = module(&bytes);
        let settings = settings_for(&module, 44_100);
        let (_, seen) = while_watching(|| EmbeddedPlayer::<Linear>::open_empty(44_100, settings).expect("it opens"));
        println!(
            "{name}: {} channels, {} voices, {} bytes of engine, {} bytes of PCM+patterns",
            settings.channel_count,
            settings.voice_capacity,
            seen.bytes,
            module.pcm().len() * 2,
        );
        assert_eq!(seen.bytes, 27_800 + 184 * settings.voice_capacity + 5_288 * settings.channel_count, "{name} did not follow the documented formula");
    }
}
