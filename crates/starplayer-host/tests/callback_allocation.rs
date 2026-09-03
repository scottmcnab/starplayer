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

const FIXTURE: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/NICETUNE.S3M");
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
