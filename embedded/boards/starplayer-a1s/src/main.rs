//! StarPlayer on the AI-Thinker ESP32-Audio-Kit.
//!
//! Two builds out of one crate:
//!
//! * the **audio** build (no features) plays `REFLEX.S3M` from flash through the ES8388
//!   into the headphone jack, takes transport and volume
//!   commands from six push-buttons (M8-I5), optionally shows the sounding row on an
//!   ST7789 screen behind the `lcd` feature (M8-I5), and logs the transport once a
//!   second;
//! * the audio build with **`tone`** still renders that module, then replaces each outgoing
//!   quantum with a gated dual-mono diagnostic sine immediately before 32-bit I2S packing;
//! * the standalone **`engine-tone`** build replaces the boot module before playback with a
//!   one-channel native S3M whose 16-bit looping sine traverses the actual engine path;
//! * the standalone **`matched-tone`** build renders that same S3M, then replaces each outgoing
//!   quantum with a continuous 125 Hz Q15 sine matched to its measured left/right peaks;
//! * the standalone **`swapped-tone`** build makes the same comparison with only those peaks
//!   exchanged between left and right;
//! * the standalone **`reference-rate-tone`** build keeps the original matched comparison and
//!   changes only the complete A1S output path to 48 000 Hz;
//! * the **`bench`** build ([`bench`]) makes no sound and instead renders every golden
//!   fixture, printing its SHA-256 and its cost.
//!
//! # Boot order, and why it is this order
//!
//! 1. `esp_hal::init` at the maximum CPU clock. Nothing may touch GPIO0, 12 or 15 before
//!    this returns — they are boot straps (see [`board`]).
//! 2. The internal-DRAM heap. Everything the engine allocates, it allocates here.
//! 3. PSRAM is *mapped* and its size reported. It is deliberately **not** added to the
//!    general heap in the audio build — see "PSRAM and atomics" below.
//! 4. `esp_rtos::start`, then the board's own pins — including the keys and, under
//!    `lcd`, the display.
//! 5. An I2C scan, logged (research point 1), then the codec — configured but **muted**.
//! 6. The module image → [`Module::from_image`] (or the standalone diagnostic's native S3M
//!    built before playback) → [`EmbeddedPlayer`], set the board's capped engine level of
//!    1/4, then move the untouched I2S peripheral parts and renderer to core 1.
//! 7. Core 1 constructs the async I2S driver, prefills and starts its transfer, then completes
//!    the muted silence pre-roll. Core 0 waits for the startup result, logs and unmutes the
//!    headphone output while leaving the speaker amplifier disabled. Finally it spawns the keys
//!    task, control task and (under `lcd`) display task.
//!
//! # PSRAM and atomics
//!
//! esp-alloc's own documentation records the erratum: **on the ESP32, ESP32-S2 and
//! ESP32-S3 the atomic instructions do not work correctly on memory located in PSRAM.**
//! This engine puts atomics on the heap — `starplayer_rt::Arc`'s reference count, the
//! seqlocks `starplayer-host-embedded` carries its two frame clocks in, the telemetry
//! ring's sequence — so a heap region that could hand any of those out of PSRAM is a heap
//! that can silently corrupt a reference count. esp-alloc's plain `alloc` takes the first
//! region that fits, in registration order, so registering PSRAM at all makes that
//! reachable the moment internal DRAM is full.
//!
//! The audio build therefore maps PSRAM, reports it, and **does not register it**. The
//! `bench` build does register it, because the bench has no real-time path and explicitly
//! wants to measure PCM read out of PSRAM — and it asks for that memory by capability
//! (`alloc_caps(External)`) rather than letting a general allocation fall into it.
//!
//! M8-I6, which wants to hold an uploaded module in PSRAM, inherits this constraint: the
//! *sample data* may live there, the `Arc` that owns it may not.
//!
//! # Core pinning — corrected by M8-I5
//!
//! **The entire audio driver and refill now run on core 1**, with core 0 running the keys task,
//! the control task and (under `lcd`) the display task. M8-I3's write-up blamed `RenderHalf`
//! not being `Send` on `Engine`'s `Box<dyn EventSource>` and kept everything on core 0 as
//! a result. That diagnosis does not hold: `EventSource: Send` and `Insert: Send` are
//! already supertrait bounds in `starplayer-engine`/`starplayer-dsp`, so `RenderHalf` is
//! already `Send` — see the compile-time assertion and its doc comment beside
//! `RenderHalf` in `crates/starplayer-host-embedded/src/player.rs`, verified against this
//! exact `xtensa-esp32-none-elf` build, not only the host one. No engine change was
//! needed to move the refill to `esp_rtos::start_second_core`, mirroring
//! `../ampkeeper/esp32/firmware/src/esp32_main.rs`'s `CORE1_SPAWNER` pattern (simplified:
//! this board only ever runs one task on core 1, so the refill is spawned directly inside
//! the second core's entry closure rather than round-tripping a `SendSpawner` back to
//! core 0 through a `Signal`).
//!
//! Moving only an already-created DMA transfer was insufficient. esp-hal 1.1.2's
//! `ChannelTx::into_async` disables the DMA interrupt on `Cpu::other()` and binds its handler to
//! `Cpu::current()`, so calling [`audio::start`] on core 0 left core 1 dependent on wakeups from
//! the core that logs. [`audio::start`] now runs inside core 1's entry closure, matching Star FX
//! and keeping UART critical sections away from the DMA interrupt it owns. A timestamped run of
//! that arrangement still reached only 0:24 engine elapsed at +49.86 seconds, with both counters
//! zero, so interrupt affinity was not the main cause of the gating.

#![no_std]
#![no_main]
// `#[embassy_executor::task]` and `#[esp_rtos::main]` expand to a `TaskStorage` whose
// future type is an associated `impl Trait`. Nightly-only, which is one of the reasons the
// firmware is its own workspace on its own toolchain (M8 master-plan decision 1).
#![feature(impl_trait_in_assoc_type)]

#[cfg(all(feature = "bench", feature = "tone"))]
compile_error!("the A1S `tone` diagnostic needs the audio build and cannot be combined with `bench`");
#[cfg(all(feature = "voice-bench", any(feature = "bench", feature = "tone", feature = "engine-tone", feature = "matched-tone", feature = "swapped-tone", feature = "reference-rate-tone", feature = "lcd")))]
compile_error!("the A1S `voice-bench` personality may only be combined with `web` or `voice-bench-filtered`");
#[cfg(all(feature = "engine-tone", any(feature = "bench", feature = "tone", feature = "swapped-tone", feature = "web")))]
compile_error!("the standalone A1S `engine-tone` diagnostic cannot be combined with `bench`, `tone`, `swapped-tone` or `web`");
#[cfg(all(feature = "matched-tone", any(feature = "bench", feature = "tone", feature = "engine-tone", feature = "swapped-tone", feature = "web")))]
compile_error!("the standalone A1S `matched-tone` diagnostic cannot be combined with `bench`, `tone`, `engine-tone`, `swapped-tone` or `web`");
#[cfg(all(feature = "swapped-tone", any(feature = "bench", feature = "tone", feature = "engine-tone", feature = "matched-tone", feature = "web")))]
compile_error!("the standalone A1S `swapped-tone` diagnostic cannot be combined with `bench`, `tone`, `engine-tone`, `matched-tone` or `web`");
#[cfg(all(feature = "reference-rate-tone", any(feature = "bench", feature = "tone", feature = "engine-tone", feature = "matched-tone", feature = "swapped-tone", feature = "web")))]
compile_error!("the standalone A1S `reference-rate-tone` diagnostic cannot be combined with `bench`, `tone`, `engine-tone`, `matched-tone`, `swapped-tone` or `web`");

extern crate alloc;

#[cfg(not(feature = "bench"))]
mod audio;
#[cfg(feature = "bench")]
mod bench;
#[cfg(not(feature = "bench"))]
mod board;
#[cfg(not(feature = "bench"))]
mod es8388;
#[cfg(not(any(feature = "engine-tone", feature = "matched-tone", feature = "swapped-tone", feature = "reference-rate-tone")))]
mod images;
#[cfg(not(feature = "bench"))]
mod keys;
#[cfg(all(not(feature = "bench"), feature = "lcd"))]
mod lcd;
#[cfg(all(not(feature = "bench"), feature = "web"))]
mod net;
#[cfg(all(not(feature = "bench"), feature = "web"))]
mod provisioning;
#[cfg(all(not(feature = "bench"), any(feature = "web", feature = "voice-bench")))]
mod psram;
#[cfg(all(not(feature = "bench"), feature = "web"))]
mod psram_task;
#[cfg(all(not(feature = "bench"), feature = "web"))]
mod store;
#[cfg(all(not(feature = "bench"), feature = "web"))]
mod web;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_println::println;
use firmware_common::format::Kib;
#[cfg(not(feature = "bench"))]
use firmware_common::{Key, KeyEvent};
#[cfg(all(not(feature = "bench"), any(feature = "lcd", feature = "web", not(feature = "voice-bench"))))]
use firmware_common::NowPlaying;
#[cfg(not(feature = "bench"))]
use starplayer::core::U0F16;
#[cfg(not(feature = "bench"))]
use starplayer::dsp::Linear;
#[cfg(all(not(feature = "bench"), any(feature = "web", feature = "voice-bench")))]
use starplayer::engine::{EngineLayout, EngineSettings};
#[cfg(all(not(feature = "bench"), not(feature = "engine-tone"), not(feature = "matched-tone"), not(feature = "swapped-tone"), not(feature = "reference-rate-tone")))]
use starplayer::model::Module;
#[cfg(not(feature = "bench"))]
use starplayer::rt::Arc;
#[cfg(not(feature = "bench"))]
use starplayer_host_embedded::{ControlHalf, EmbeddedPlayer, RenderHalf};
#[cfg(not(feature = "bench"))]
use static_cell::StaticCell;

esp_app_desc!();

/// The internal-DRAM heap.
///
/// **120 KiB, and the number is a balance, not a guess.** The engine's own cost is
/// `27 808 + 168 × voices + 5 288 × channels` bytes
/// (`starplayer_host_embedded::settings_for`), which for `PETRI.S3M` — eight channels,
/// eight voices — is 71 456; the scanned song timeline, the `Arc`s and the sequencer sit
/// on top of that, and the `bench` build asks for a further 32 KiB of DRAM staging.
///
/// The balance is against the **stack**. This is a `.bss` array, and on the classic ESP32
/// the main stack is exactly the DRAM left between `_bss_end` and `0x3ffe_0000`, so every
/// byte here is a byte the stack does not get. At 160 KiB the firmware still *links* — and
/// leaves 6 472 bytes of stack, which is not enough to survive a boot. At 120 KiB the
/// stack is 47 432 bytes. Check it after any change that moves a large static:
///
/// ```text
/// xtensa-esp32-elf-nm target/xtensa-esp32-none-elf/release/starplayer-a1s \
///   | grep -E " (_bss_end|_stack_start)$"
/// ```
///
/// This array reserves heap *capacity* in `.bss`; later heap allocations consume that already
/// reserved region and do not move `_bss_end`. In particular, audio allocates its fixed 2 048-byte
/// packed descriptor only after `EmbeddedPlayer::open`, so player construction sees the full heap
/// and the descriptor costs no additional main-stack address space.
///
/// **The `web` build has two Internal-capability heap regions.** The first 96 KiB lives
/// in `dram2_seg`, where it does not shorten the main stack. M8-I8 moved the picoserve
/// worker futures from `.bss` to claim-only PSRAM and recovered enough ordinary DRAM for
/// a second 48 KiB `.bss` region. That raises the web heap from 96 KiB to 144 KiB for the
/// radio while retaining about 56 KiB of stack in the tightest `web,lcd` personality.
/// Both regions are ordinary internal DRAM, and PSRAM remains unregistered; see
/// [`WEB_RECLAIMED_HEAP_BYTES`] and [`WEB_INTERNAL_HEAP_BYTES`].
#[cfg(not(feature = "web"))]
const HEAP_BYTES: usize = 120 * 1024;
#[cfg(feature = "web")]
const HEAP_BYTES: usize = WEB_RECLAIMED_HEAP_BYTES + WEB_INTERNAL_HEAP_BYTES;

/// The first `web` heap region, in `dram2_seg`.
///
/// `dram2_seg` is the 98 768 bytes of DRAM above the ROM's own data and stacks
/// (`esp-hal`'s `ld/esp32/memory.x`), which the ESP-IDF heap reclaims and which esp-hal
/// leaves as an uninitialised section with no other user. It is ordinary internal DRAM —
/// atomics work there, unlike PSRAM — and, crucially, it is **not** `.bss` in `dram_seg`,
/// so it does not shorten the main stack. ampkeeper's classic-ESP32 build put 96 KiB of
/// its heap here for exactly this reason after a provisioning-mode allocation failure.
///
/// 96 KiB of the region's 98 768 bytes: all of it but a rounding margin. This region is
/// registered first so ordinary boot/player allocations consume it before the second
/// region takes space from the core-0 stack.
#[cfg(feature = "web")]
const WEB_RECLAIMED_HEAP_BYTES: usize = 96 * 1024;

/// The second `web` heap region, in ordinary `.bss` internal DRAM.
///
/// I8's exact links left 106 716 bytes of stack in `web` and 105 284 in `web,lcd` after
/// relocating picoserve futures to PSRAM. Giving 48 KiB back to the allocator leaves
/// about 57 KiB / 56 KiB respectively, still well above the enforced 32 KiB floor, and
/// supplies the internal-only dynamic allocations required by `esp_radio::wifi::new`.
#[cfg(feature = "web")]
const WEB_INTERNAL_HEAP_BYTES: usize = 48 * 1024;

/// The render half lives in a `static`: `size_of::<RenderHalf<Linear>>()` is 11 744 bytes
/// (I1 research point 3a), because the engine's telemetry publisher holds a working
/// `Snapshot` inline and this host holds another. Passing that down a call chain by value
/// would put two copies of it on the stack.
#[cfg(all(not(feature = "bench"), not(feature = "voice-bench")))]
static RENDER: StaticCell<RenderHalf<Linear>> = StaticCell::new();

/// Module construction runs as a separately polled task. The async main poll frame is
/// already close to core 0's linked stack limit; yielding before this task runs prevents
/// `ModuleBuilder` and image serialization frames from nesting underneath it.
#[cfg(feature = "voice-bench")]
static VOICE_BENCH_IMAGE_BUFFER: StaticCell<psram::Buffer> = StaticCell::new();
#[cfg(feature = "voice-bench")]
static VOICE_BENCH_MODULE_READY: embassy_sync::signal::Signal<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    Result<Arc<Module>, &'static str>,
> = embassy_sync::signal::Signal::new();
/// `init_with` constructs this pair directly in static storage. In particular, no
/// 11,744-byte `RenderHalf` value is copied through the setup task's stack frame.
#[cfg(feature = "voice-bench")]
static VOICE_BENCH_PLAYER: StaticCell<Option<(RenderHalf<Linear>, ControlHalf)>> = StaticCell::new();

#[cfg(feature = "voice-bench")]
struct VoiceBenchPlayer {
    render: &'static mut RenderHalf<Linear>,
    control: &'static mut ControlHalf,
}

#[cfg(feature = "voice-bench")]
static VOICE_BENCH_PLAYER_READY: embassy_sync::signal::Signal<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    Result<VoiceBenchPlayer, &'static str>,
> = embassy_sync::signal::Signal::new();

/// Core 1's stack. Audio start makes one small fallible heap allocation before driver construction;
/// the refill itself allocates and logs nothing. The synchronous prefill has one 512-byte quantum
/// scratch; steady refill uses a `ConstStaticCell` render buffer and one already-owned heap
/// descriptor instead of task stack. This remains deliberately far
/// smaller than core 0's stack, which carries the whole engine's call depth.
#[cfg(not(feature = "bench"))]
const CORE1_STACK_SIZE: usize = 8 * 1024;

// Const-initialized rather than built with `StaticCell::init`, the same reasoning
// ampkeeper's own `CORE1_STACK` comment gives: constructing an 8 KiB value through
// `StaticCell::init` could materialise it on core 0's caller stack before the copy into
// `.bss`. `ConstStaticCell::take` hands the static allocation directly to esp-rtos.
#[cfg(not(feature = "bench"))]
static CORE1_STACK: static_cell::ConstStaticCell<esp_hal::system::Stack<CORE1_STACK_SIZE>> =
    static_cell::ConstStaticCell::new(esp_hal::system::Stack::new());
#[cfg(not(feature = "bench"))]
static CORE1_EXECUTOR: StaticCell<esp_rtos::embassy::Executor> = StaticCell::new();

/// Carries the untouched I2S peripheral tokens across the one-time ownership handoff into core
/// 1's entry closure.
///
/// [`audio::Parts`] is not `Send` because esp-hal's peripheral tokens deliberately carry
/// non-`Send` markers. This crate is the one place `unsafe` is tolerated in the whole workspace
/// (M8 master-plan decision 6), and this is exactly the shape `esp_rtos::start_second_core`'s own
/// internal `SecondCoreStack` wrapper exists to cross (`esp-rtos` 0.3.0's `lib.rs`): each unique
/// peripheral token is moved **once**, into the closure that becomes core 1's program, with core
/// 0 never touching it again. The async driver is created only after that handoff, which also
/// binds its DMA interrupt to the correct core. `Send`'s safety property holds because there is
/// only ever one owner of each token, never concurrent access from two cores.
#[cfg(not(feature = "bench"))]
struct SendAudioParts(audio::Parts);

// SAFETY: see the doc comment on `SendAudioParts` above.
#[cfg(not(feature = "bench"))]
unsafe impl Send for SendAudioParts {}

/// Key events flow from [`keys::keys_task`] (core 0, polling GPIO every 10 ms) to
/// [`control_task`] (core 0, the sole owner of [`ControlHalf`]) over this channel.
#[cfg(not(feature = "bench"))]
static KEY_EVENTS: keys::KeyEventChannel = keys::KeyEventChannel::new();

/// The latest [`NowPlaying`] the control task has computed, signalled to the display task
/// at up to 20 Hz. A `Signal` rather than a `Channel`: the display only ever wants the
/// newest frame, and a display task that fell behind must never queue up stale ones.
#[cfg(all(not(feature = "bench"), feature = "lcd"))]
static NOW_PLAYING: embassy_sync::signal::Signal<embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, NowPlaying> =
    embassy_sync::signal::Signal::new();

/// The core-0 stack is the linker-selected gap between `.bss` and the top of DRAM.
/// `stack-floor.x` enforces its minimum; this reports the exact per-personality value.
fn linked_main_stack_bytes() -> usize {
    unsafe extern "C" {
        static _stack_end: u8;
        static _stack_start: u8;
    }
    let stack_end = core::ptr::addr_of!(_stack_end) as usize;
    let stack_start = core::ptr::addr_of!(_stack_start) as usize;
    stack_start.saturating_sub(stack_end)
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()));

    esp_println::logger::init_logger_from_env();
    // The web build registers its 96 KiB dram2 region first, then its 48 KiB `.bss`
    // reserve. Both have the default Internal capability; PSRAM is never registered.
    #[cfg(not(feature = "web"))]
    esp_alloc::heap_allocator!(size: HEAP_BYTES);
    #[cfg(feature = "web")]
    {
        esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: WEB_RECLAIMED_HEAP_BYTES);
        esp_alloc::heap_allocator!(size: WEB_INTERNAL_HEAP_BYTES);
    }

    let timer = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG1);
    let software_interrupts = esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timer.timer0, software_interrupts.software_interrupt0);

    println!();
    println!("StarPlayer {} on the ESP32-A1S Audio Kit", env!("CARGO_PKG_VERSION"));
    println!("CPU  {} MHz   heap {}", esp_hal::clock::cpu_clock().as_mhz(), Kib(HEAP_BYTES));
    let main_stack_bytes = linked_main_stack_bytes();
    println!("STACK core0 linked {main_stack_bytes} bytes ({})", Kib(main_stack_bytes));

    // PSRAM: mapped and reported in both builds, registered only by the bench. See the
    // module docs, "PSRAM and atomics".
    let psram = esp_hal::psram::Psram::new(peripherals.PSRAM, esp_hal::psram::PsramConfig::default());
    let (psram_start, psram_size) = psram.raw_parts();
    println!("PSRAM {psram_size} bytes ({}) mapped at {psram_start:p}", Kib(psram_size));

    // esptool resets the board before the runner can reopen its serial connection. Keep
    // every voice-bench machine record and setup attempt behind a short capture window;
    // ordinary firmware personalities have no added boot delay.
    #[cfg(feature = "voice-bench")]
    Timer::after(Duration::from_secs(firmware_common::voice_bench::CAPTURE_GRACE_SECONDS)).await;

    #[cfg(not(any(feature = "bench", feature = "web", feature = "voice-bench")))]
    let _ = psram_start;

    // The `web` build takes the PSRAM region as an arena of its own instead of handing it
    // to the allocator — see `psram.rs` for the atomics erratum that forbids the latter.
    #[cfg(all(not(feature = "bench"), any(feature = "web", feature = "voice-bench")))]
    let mut psram_arena = psram::Arena::new(psram_start, psram_size);

    #[cfg(all(not(feature = "bench"), feature = "web"))]
    let network_psram_arena = match psram_arena.split_prefix(psram::NETWORK_BYTES) {
        Some(network) => network,
        None => {
            psram_arena = psram::Arena::empty();
            psram::Arena::empty()
        }
    };

    #[cfg(feature = "voice-bench")]
    let voice_bench_image_buffer = match psram_arena.claim(firmware_common::voice_bench::IMAGE_BUFFER_BYTES) {
        Some(buffer) => VOICE_BENCH_IMAGE_BUFFER.init(buffer),
        None => {
            emit_voice_bench_setup_reject("psram_claim", psram_arena.remaining());
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };
    #[cfg(feature = "voice-bench")]
    match voice_bench_module_task(voice_bench_image_buffer) {
        Ok(token) => spawner.spawn(token),
        Err(_) => {
            emit_voice_bench_setup_reject("module_task", psram_arena.remaining());
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    }
    #[cfg(feature = "voice-bench")]
    let voice_bench_module = match VOICE_BENCH_MODULE_READY.wait().await {
        Ok(module) => module,
        Err(message) => {
            emit_voice_bench_setup_reject(message, psram_arena.remaining());
            loop {
                Timer::after(Duration::from_secs(60)).await;
            }
        }
    };

    #[cfg(feature = "bench")]
    if psram_size > 0 {
        // SAFETY: `raw_parts` reports the region esp-hal has just mapped, exclusively —
        // nothing else in this firmware touches it — and it is valid for `psram_size`
        // bytes for the life of the program, which is `add_region`'s whole contract. This
        // is `esp_alloc::psram_allocator!`'s own expansion. It is inside `cfg(bench)`
        // because a **bench** build has no real-time path and no `Arc` it would mind
        // finding in PSRAM; the audio build must never reach this.
        unsafe {
            esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
                psram_start,
                psram_size,
                esp_alloc::MemoryCapability::External.into(),
            ));
        }
        println!("PSRAM registered as an External heap region (bench build only)");
    }

    #[cfg(feature = "bench")]
    {
        let _ = spawner;
        bench::run(&firmware_common::bench::BenchPlan::GOLDEN);
        loop {
            Timer::after(Duration::from_secs(60)).await;
        }
    }

    // The web build opens the flash **before** the second core starts: reading the
    // partition table and the stored credentials are flash reads with a ~3 KiB stack
    // frame each, and doing them while the audio refill is running would cost underruns
    // for no reason.
    #[cfg(all(not(feature = "bench"), feature = "web"))]
    let web_boot = {
        let mut store = match store::Store::take(peripherals.FLASH) {
            Ok(store) => store,
            Err(error) => {
                #[cfg(feature = "voice-bench")]
                emit_voice_bench_setup_reject("flash_store", psram_arena.remaining());
                #[cfg(not(feature = "voice-bench"))]
                println!("FATAL: the flash partitions are not what this firmware expects: {error:?}");
                #[cfg(feature = "voice-bench")]
                println!("VOICE_BENCH_SETUP flash partitions: {error:?}");
                loop {
                    Timer::after(Duration::from_secs(5)).await;
                }
            }
        };
        let credentials = store.load_wifi().await.ok().flatten();
        println!(
            "STORE config partition {}, modules partition {} slots",
            match &credentials {
                Some(credentials) => credentials.ssid.as_str(),
                None => "no network stored",
            },
            store.slot_count(),
        );
        let random = esp_hal::rng::Rng::new();
        let seed = u64::from(random.random()) | (u64::from(random.random()) << 32);
        // The format is corrected from the module's own header the moment `play` has one;
        // the compiled-in image is S3M, and saying so here keeps `Bridge::new` from
        // needing a module it does not yet have.
        let bridge = web::Bridge::new(store, &mut psram_arena, "S3M");
        println!(
            "PSRAM upload staging {}, {} left unclaimed",
            if bridge.has_psram() { "claimed" } else { "unavailable — uploads will be refused" },
            Kib(psram_arena.remaining()),
        );
        web::Boot { bridge, psram_arena: network_psram_arena, credentials, wifi: peripherals.WIFI, seed }
    };

    #[cfg(feature = "voice-bench")]
    let voice_bench_external_heap = psram_arena.remaining();

    #[cfg(not(feature = "bench"))]
    let started = {
        #[cfg(not(feature = "lcd"))]
        let board = board::Board::take(
            peripherals.I2C0,
            peripherals.GPIO33,
            peripherals.GPIO32,
            peripherals.GPIO21,
            peripherals.GPIO39,
            peripherals.GPIO36,
            peripherals.GPIO13,
            peripherals.GPIO19,
            peripherals.GPIO23,
            peripherals.GPIO18,
            peripherals.GPIO5,
        );
        #[cfg(feature = "lcd")]
        let board = board::Board::take(
            peripherals.I2C0,
            peripherals.GPIO33,
            peripherals.GPIO32,
            peripherals.GPIO21,
            peripherals.GPIO39,
            peripherals.GPIO36,
            peripherals.GPIO19,
            peripherals.GPIO23,
            peripherals.GPIO18,
            peripherals.GPIO5,
        );
        let audio_parts = audio::Parts {
            i2s0: peripherals.I2S0,
            dma: peripherals.DMA_I2S0,
            mclk: peripherals.GPIO0,
            bclk: peripherals.GPIO27,
            lrck: peripherals.GPIO25,
            dout: peripherals.GPIO26,
        };
        #[cfg(feature = "lcd")]
        let lcd_parts =
            lcd::Parts { spi2: peripherals.SPI2, sck: peripherals.GPIO14, mosi: peripherals.GPIO13, cs: peripherals.GPIO15, dc: peripherals.GPIO2, rst: peripherals.GPIO4 };
        match board {
            Ok(board) => {
                play(
                    spawner,
                    board,
                    audio_parts,
                    peripherals.CPU_CTRL,
                    software_interrupts.software_interrupt1,
                    #[cfg(feature = "lcd")]
                    lcd_parts,
                    #[cfg(feature = "web")]
                    web_boot,
                    #[cfg(feature = "voice-bench")]
                    voice_bench_module,
                    #[cfg(feature = "voice-bench")]
                    voice_bench_external_heap,
                )
                .await
            }
            Err(_) => Err("the I2C controller rejected its configuration"),
        }
    };

    #[cfg(not(feature = "bench"))]
    if let Err(message) = started {
        #[cfg(feature = "voice-bench")]
        emit_voice_bench_setup_reject(message, voice_bench_external_heap);
        #[cfg(not(feature = "voice-bench"))]
        // A boot that cannot make sound is not a boot that should pretend to. Say what
        // failed, loudly and repeatedly, rather than resetting into the same failure.
        loop {
            #[cfg(not(feature = "voice-bench"))]
            println!("FATAL: {message}");
            Timer::after(Duration::from_secs(5)).await;
        }
    }
}

/// Bring the codec and I2S up, start playing, and start the second core with the audio
/// refill on it. Only the audio build compiles it.
#[cfg(not(feature = "bench"))]
async fn play(
    spawner: Spawner, mut board: board::Board<'static>, audio_parts: audio::Parts, cpu_control: esp_hal::peripherals::CPU_CTRL<'static>,
    software_interrupt1: esp_hal::interrupt::software::SoftwareInterrupt<'static, 1>,
    #[cfg(feature = "lcd")] lcd_parts: lcd::Parts,
    #[cfg(feature = "web")] web_boot: web::Boot,
    #[cfg(feature = "voice-bench")] voice_bench_module: Arc<Module>,
    #[cfg(feature = "voice-bench")] voice_bench_external_heap: usize,
) -> Result<(), &'static str> {
    #[cfg(all(feature = "web", not(feature = "voice-bench")))]
    let mut web_boot = web_boot;
    // Research point 1: what is actually on the control bus. Printed before anything is
    // configured, so a board that answers nowhere is diagnosable from the first boot.
    report_i2c_scan(&mut board);
    println!("JACK headphone_detect={}", if board.headphone_detect.is_low() { "inserted" } else { "empty" });

    // Headphone-only playback: the constructor starts GPIO21 low, and writing it again here
    // makes the listening policy independent of that implementation detail.
    board.power_amplifier.set_low();
    let mut codec = es8388::Es8388::new(board.i2c, board::ES8388_I2C_ADDRESS);
    codec.init_dac_only(es8388::outputs::HEADPHONE).map_err(|_| "the ES8388 did not answer — is this the AC101 revision?")?;
    println!("CODEC ES8388 at 0x{:02x}: DAC up, 32-bit Philips slave, MCLK/LRCK 256, headphone -12 dB, speaker minimum, muted", board::ES8388_I2C_ADDRESS);

    #[cfg(feature = "lcd")]
    let display = lcd::LcdDisplay::take(lcd_parts);
    #[cfg(feature = "lcd")]
    println!("LCD  ST7789 {}", if display.is_present() { "found" } else { "not found — continuing headless" });

    // Normal firmware borrows REFLEX straight from memory-mapped flash, PCM and all. The
    // standalone engine diagnostic instead allocates its controlled native S3M once here,
    // before either the player or audio task exists.
    #[cfg(all(not(feature = "voice-bench"), not(any(feature = "engine-tone", feature = "matched-tone", feature = "swapped-tone", feature = "reference-rate-tone"))))]
    let image = images::boot_module();
    #[cfg(all(not(feature = "voice-bench"), not(any(feature = "engine-tone", feature = "matched-tone", feature = "swapped-tone", feature = "reference-rate-tone"))))]
    #[cfg(not(feature = "web"))]
    let module = Arc::new(Module::try_from_image(image).map_err(|_| "the linked module image would not borrow — is it 4-byte aligned?")?);
    #[cfg(all(not(feature = "voice-bench"), feature = "web", not(any(feature = "engine-tone", feature = "matched-tone", feature = "swapped-tone", feature = "reference-rate-tone"))))]
    let module = match web_boot.bridge.initial_module() {
        Some(module) => module,
        None => Arc::new(Module::try_from_image(image).map_err(|_| "the linked module image would not borrow — is it 4-byte aligned?")?),
    };
    #[cfg(feature = "voice-bench")]
    let module = voice_bench_module;
    #[cfg(all(not(feature = "voice-bench"), not(any(feature = "engine-tone", feature = "matched-tone", feature = "swapped-tone", feature = "reference-rate-tone"))))]
    println!(
        "MODULE image={} bytes ({}) channels={} samples={}",
        image.len(),
        Kib(image.len()),
        module.header().channel_count,
        module.samples().len(),
    );
    #[cfg(any(feature = "engine-tone", feature = "matched-tone", feature = "swapped-tone", feature = "reference-rate-tone"))]
    let module = Arc::new(firmware_common::engine_tone_module().map_err(|_| "the engine-tone module would not build")?);
    #[cfg(feature = "engine-tone")]
    println!(
        "ENGINE-TONE output={} Hz source={} Hz signed16 amplitude={} loop={} frames song_loop=B00 interpolation=linear master=1/4",
        firmware_common::ENGINE_TONE_OUTPUT_FREQUENCY_HZ,
        firmware_common::ENGINE_TONE_REFERENCE_RATE_HZ,
        firmware_common::ENGINE_TONE_SOURCE_AMPLITUDE,
        firmware_common::ENGINE_TONE_LOOP_FRAMES,
    );
    #[cfg(feature = "matched-tone")]
    println!(
        "MATCHED-TONE output={} Hz peaks={}/{} generator=integer-Q15 continuous underlay=engine-tone interpolation=linear master=1/4",
        firmware_common::MATCHED_TONE_FREQUENCY_HZ,
        firmware_common::MATCHED_TONE_LEFT_PEAK,
        firmware_common::MATCHED_TONE_RIGHT_PEAK,
    );
    #[cfg(feature = "swapped-tone")]
    println!(
        "SWAPPED-TONE output={} Hz peaks={}/{} generator=integer-Q15 continuous underlay=engine-tone interpolation=linear master=1/4",
        firmware_common::MATCHED_TONE_FREQUENCY_HZ,
        firmware_common::SWAPPED_TONE_LEFT_PEAK,
        firmware_common::SWAPPED_TONE_RIGHT_PEAK,
    );
    #[cfg(feature = "reference-rate-tone")]
    println!(
        "REFERENCE-RATE-TONE rate={} Hz output={} Hz peaks={}/{} generator=integer-Q15 continuous underlay=engine-tone interpolation=linear master=1/4",
        OUTPUT_SAMPLE_RATE_HZ,
        firmware_common::MATCHED_TONE_FREQUENCY_HZ,
        firmware_common::MATCHED_TONE_LEFT_PEAK,
        firmware_common::MATCHED_TONE_RIGHT_PEAK,
    );

    #[cfg(all(not(feature = "web"), not(feature = "voice-bench")))]
    let (render, mut control) = EmbeddedPlayer::<Linear>::open(module, OUTPUT_SAMPLE_RATE_HZ)
        .map_err(|_| "this build cannot play that module")?;
    #[cfg(all(feature = "web", not(feature = "voice-bench")))]
    let (render, mut control) = {
        let initial_channel_count = module.header().channel_count;
        let settings = EngineSettings {
            sample_rate_hz: OUTPUT_SAMPLE_RATE_HZ,
            channel_count: web::PLAYBACK_CHANNEL_CAPACITY,
            voice_capacity: web::PLAYBACK_VOICE_CAPACITY,
            scope_taps: false,
            telemetry_depth: 1,
            layout: EngineLayout::MasterOnly,
            ..EngineSettings::default()
        };
        let (render, mut control) = EmbeddedPlayer::<Linear>::open_empty(OUTPUT_SAMPLE_RATE_HZ, settings)
            .map_err(|_| "this build cannot allocate the fixed web player")?;
        if web_boot.bridge.has_psram() {
            web_boot.bridge.load_initial(&mut control, module)?;
        } else {
            control.load(module).map_err(|_| "this build cannot play the linked module")?;
        }
        web::print_channel_warning(initial_channel_count);
        (render, control)
    };
    #[cfg(not(feature = "voice-bench"))]
    let render = RENDER.init(render);
    #[cfg(feature = "voice-bench")]
    match voice_bench_player_task(module) {
        Ok(token) => spawner.spawn(token),
        Err(_) => return Err("the voice benchmark player task would not spawn"),
    }
    #[cfg(feature = "voice-bench")]
    let VoiceBenchPlayer { render, control } = VOICE_BENCH_PLAYER_READY.wait().await?;
    println!("HEAP after open: {}", esp_alloc::HEAP.stats());

    control.set_master_volume(BOOT_MASTER_VOLUME).map_err(|_| "the command ring rejected the boot volume")?;
    control.play().map_err(|_| "the command ring rejected the first play")?;

    // Construct the whole async driver on core 1, where its DMA interrupt belongs. The codec
    // stays muted until construction, real-audio prefill, transfer start and the silent
    // descriptor handoffs have all succeeded. Nothing crosses back: the refill is built inside
    // the second core's entry closure and spawned on its executor, so core 0 never holds a
    // `SendSpawner` for it and this board never spawns a second task there.
    //
    // The untouched peripheral tokens need the documented ownership wrapper above.
    // `RenderHalf<Linear>` is already `Send` (see the module docs and the assertion beside its
    // definition), so `&'static mut RenderHalf` needs no wrapper.
    let core1_stack = CORE1_STACK.take();
    let audio_parts = SendAudioParts(audio_parts);
    esp_rtos::start_second_core(cpu_control, software_interrupt1, core1_stack, move || {
        // Bind the whole wrapper before touching `.0`, or edition-2021 disjoint capture would
        // reach through it and make the non-`Send` Parts field itself part of the closure.
        let audio_parts = audio_parts;
        let audio_parts = audio_parts.0;
        let transfer = match audio::start(audio_parts, OUTPUT_SAMPLE_RATE_HZ, render) {
            Ok(transfer) => transfer,
            Err(_) => {
                audio::report_start_failure();
                return;
            }
        };
        CORE1_EXECUTOR.init(esp_rtos::embassy::Executor::new()).run(|core1_spawner| {
            let token = audio::refill_task(transfer, render).expect("the refill task is the only thing core 1's executor ever spawns");
            core1_spawner.spawn(token);
        });
    });
    wait_for_audio_ready()?;
    println!("HEAP after audio start: {}", esp_alloc::HEAP.stats());
    println!(
        "I2S  {} Hz stereo 32-bit slots, MCLK on GPIO{}, DMA ring {} quanta ({} frames, {} ms)",
        OUTPUT_SAMPLE_RATE_HZ,
        board::PIN_I2S_MCLK,
        audio::DMA_RING_QUANTA,
        audio::DMA_RING_QUANTA * audio::QUANTUM_FRAMES,
        audio::DMA_RING_QUANTA * audio::QUANTUM_FRAMES * 1000 / OUTPUT_SAMPLE_RATE_HZ as usize,
    );
    let pre_roll_ms = audio::TX_PRIME_HANDOFFS * audio::DESCRIPTOR_FRAMES * 1000 / OUTPUT_SAMPLE_RATE_HZ as usize;
    println!("CORE1 audio refill running; pre-roll {} silent descriptor handoffs complete (~{} ms)", audio::TX_PRIME_HANDOFFS, pre_roll_ms);
    #[cfg(feature = "tone")]
    println!(
        "TONE diagnostic dual-mono sine {} amplitude={} gate={} frames (~998 ms) tone / {} frames (~998 ms) silence",
        firmware_common::DIAGNOSTIC_TONE_FREQUENCY,
        firmware_common::DIAGNOSTIC_TONE_AMPLITUDE,
        firmware_common::DIAGNOSTIC_TONE_ACTIVE_FRAMES,
        firmware_common::DIAGNOSTIC_TONE_SILENT_FRAMES,
    );

    // Sound only after the muted pre-roll has established descriptor accounting and the
    // steady refill owns the transfer. GPIO21 remains low and pair 1 remains disabled: this
    // boot is deliberately headphone-only after the first hardware run clipped at unity.
    codec.mute(false).map_err(|_| "the codec would not unmute")?;
    println!("PLAY master_volume=1/4 max=1/4 output=headphone speaker_pa=off");

    // The re-provision gesture, checked once, before the keys task exists. A boot nobody
    // is touching costs one GPIO read; a held key costs the five seconds it is held, with
    // the music playing throughout.
    #[cfg(feature = "web")]
    let reprovision = {
        let held = keys::held_at_boot(&board.keys, REPROVISION_KEY, Duration::from_secs(REPROVISION_HOLD_SECONDS)).await;
        if held {
            println!("KEYS {REPROVISION_KEY:?} held at boot — starting the captive portal");
        }
        held
    };

    #[cfg(feature = "web")]
    let web::Boot { bridge, mut psram_arena, credentials, wifi, seed } = web_boot;

    spawner.spawn(keys::keys_task(board.keys, KEY_EVENTS.sender()).map_err(|_| "the keys task would not spawn")?);
    #[cfg(not(feature = "web"))]
    spawner.spawn(control_task(control, (), #[cfg(feature = "voice-bench")] voice_bench_external_heap).map_err(|_| "the control task would not spawn")?);
    #[cfg(all(feature = "web", not(feature = "voice-bench")))]
    spawner.spawn(control_task(control, bridge, #[cfg(feature = "voice-bench")] voice_bench_external_heap).map_err(|_| "the control task would not spawn")?);
    #[cfg(feature = "lcd")]
    spawner.spawn(display_task(display).map_err(|_| "the display task would not spawn")?);

    // The network personality comes last: the codec, the DMA ring and core 1 are all up,
    // so the radio's own bring-up cannot delay the first sample.
    #[cfg(feature = "web")]
    {
        #[cfg(feature = "voice-bench")]
        if !firmware_common::voice_bench::web_network_preflight_passes(esp_alloc::HEAP.free()) {
            return Err("insufficient internal heap before the voice benchmark network start");
        }
        match credentials.filter(|_| !reprovision) {
            Some(credentials) => {
                println!("WIFI joining {}", credentials.ssid.as_str());
                let station = net::start(spawner, wifi, credentials, seed)?;
                web::start(spawner, station.stack, &mut psram_arena)?;
            }
            None => provisioning::run(spawner, wifi, seed, &mut psram_arena).await?,
        }
        spawner.spawn(reboot_task().map_err(|_| "the reboot watcher would not spawn")?);
    }
    #[cfg(all(feature = "web", feature = "voice-bench"))]
    spawner.spawn(control_task(control, bridge, voice_bench_external_heap).map_err(|_| "the control task would not spawn")?);
    Ok(())
}

/// The key held at boot to start the captive portal instead of joining the stored
/// network.
///
/// KEY2 in the six-key build. **KEY1 in the `lcd` build**, where KEY2 does not exist at
/// all — GPIO13 is the display's MOSI there (M8-I5) — so the gesture moves rather than
/// disappearing. KEY1 already carries a long-press meaning in that build (stop and
/// rewind), but only through the keys task, which does not exist yet when this is
/// checked, so the two cannot collide.
#[cfg(all(not(feature = "bench"), feature = "web"))]
const REPROVISION_KEY: Key = if cfg!(feature = "lcd") { Key::Key1 } else { Key::Key2 };

/// How long that key must be held.
#[cfg(all(not(feature = "bench"), feature = "web"))]
const REPROVISION_HOLD_SECONDS: u64 = 5;

/// Reset the board when something asks for it — `POST /api/reprovision`, or the portal's
/// `POST /save`.
///
/// The delay is what lets the answer reach the browser first: a reset inside the handler
/// would drop the connection before the response was flushed, and the owner would see a
/// failed request for an operation that in fact succeeded.
#[cfg(all(not(feature = "bench"), feature = "web"))]
#[embassy_executor::task]
async fn reboot_task() {
    web::wait_for_reboot().await;
    println!("RESET requested — restarting in two seconds");
    Timer::after(Duration::from_secs(2)).await;
    esp_hal::system::software_reset();
}

/// Probe the control bus and print what answered.
#[cfg(not(feature = "bench"))]
fn report_i2c_scan(board: &mut board::Board<'static>) {
    let found = es8388::Es8388::scan(&mut board.i2c);
    let mut any = false;
    for address in 0u8..128 {
        if found[usize::from(address) / 64] & (1u64 << (address % 64)) != 0 {
            let note = match address {
                board::ES8388_I2C_ADDRESS => " (ES8388)",
                board::AC101_I2C_ADDRESS => " (AC101 — this revision is out of scope)",
                _ => "",
            };
            println!("I2C  device at 0x{address:02x}{note}");
            any = true;
        }
    }
    if !any {
        println!("I2C  no device answered — check SDA GPIO{} / SCL GPIO{}", board::PIN_I2C_SDA, board::PIN_I2C_SCL);
    }
}

/// The A1S boot and maximum engine volume: exactly 1/4 of full scale.
///
/// The ES8388 headphone analog pair supplies another −12 dB, preserving the owner's accepted
/// nominal maximum while retaining two more digital signal bits. This remains board-local:
/// [`ControlHalf`] and the engine retain their unity defaults for every other host.
#[cfg(not(feature = "bench"))]
pub(crate) const MAX_MASTER_VOLUME: U0F16 = U0F16::from_bits(16_384);

#[cfg(not(feature = "bench"))]
const BOOT_MASTER_VOLUME: U0F16 = MAX_MASTER_VOLUME;

/// The accepted ES8388 output rate for every audible production A1S personality.
#[cfg(all(not(feature = "bench"), not(any(feature = "tone", feature = "engine-tone", feature = "matched-tone", feature = "swapped-tone"))))]
const A1S_AUDIBLE_SAMPLE_RATE_HZ: u32 = 48_000;

/// One board-selected output rate shared by player timing, I2S and presentation.
///
/// The four historical audio diagnostics remain pinned to firmware-common's golden 44.1 kHz rate
/// for reproducibility. Default, `lcd`, `web`, `web,lcd` and the accepted reference-rate image use
/// the A1S audible rate. Bench bypasses the audio path and uses the firmware-common constant
/// directly.
#[cfg(any(feature = "tone", feature = "engine-tone", feature = "matched-tone", feature = "swapped-tone"))]
const OUTPUT_SAMPLE_RATE_HZ: u32 = firmware_common::SAMPLE_RATE_HZ;
#[cfg(all(not(feature = "bench"), not(any(feature = "tone", feature = "engine-tone", feature = "matched-tone", feature = "swapped-tone"))))]
const OUTPUT_SAMPLE_RATE_HZ: u32 = A1S_AUDIBLE_SAMPLE_RATE_HZ;

#[cfg(feature = "voice-bench")]
const VOICE_BENCH_CHANNELS: usize = decimal_constant(env!("STARPLAYER_VOICE_BENCH_CHANNELS"));
#[cfg(feature = "voice-bench")]
const VOICE_BENCH_VOICES: usize = decimal_constant(env!("STARPLAYER_VOICE_BENCH_VOICES"));
#[cfg(feature = "voice-bench")]
const VOICE_BENCH_SECONDS: usize = decimal_constant(env!("STARPLAYER_VOICE_BENCH_SECONDS"));
#[cfg(feature = "voice-bench")]
const VOICE_BENCH_LOW: usize = decimal_constant(env!("STARPLAYER_VOICE_BENCH_LOW"));
#[cfg(feature = "voice-bench")]
const VOICE_BENCH_HIGH: usize = decimal_constant(env!("STARPLAYER_VOICE_BENCH_HIGH"));
#[cfg(feature = "voice-bench")]
const VOICE_BENCH_PHASE: &str = env!("STARPLAYER_VOICE_BENCH_PHASE");
#[cfg(feature = "voice-bench")]
const VOICE_BENCH_AXIS: &str = env!("STARPLAYER_VOICE_BENCH_AXIS");
#[cfg(feature = "voice-bench")]
const VOICE_BENCH_CASE: &str = env!("STARPLAYER_VOICE_BENCH_CASE");

#[cfg(feature = "voice-bench")]
fn emit_voice_bench_setup_reject(message: &str, external_heap: usize) {
    println!("VOICE_BENCH_SETUP {message}");
    println!(
        "VOICE_BENCH v=1 seq=0 kind=START case={} mode={} filtered={} channels={} voices={} rate=48000 descriptor_frames=256 duration_s={} phase={} axis={} low={} high={} dma_base={}",
        VOICE_BENCH_CASE,
        if cfg!(feature = "web") { "web" } else { "audio" },
        u8::from(cfg!(feature = "voice-bench-filtered")),
        VOICE_BENCH_CHANNELS,
        VOICE_BENCH_VOICES,
        VOICE_BENCH_SECONDS,
        VOICE_BENCH_PHASE,
        VOICE_BENCH_AXIS,
        VOICE_BENCH_LOW,
        VOICE_BENCH_HIGH,
        audio::dma_errors(),
    );
    println!(
        "VOICE_BENCH v=1 seq=1 kind=END case={} elapsed_ms=0 frames=0 active=0 peak_active=0 render_max_us=0 render_p50_us=0 render_p95_us=0 misses=0 underruns={} dma_errors={} warnings=0 steals=0 heap_internal={} heap_external={} web_http_ok=0 web_upload_ok=0 web_ws_ok=0 status=reject reason=setup",
        VOICE_BENCH_CASE,
        audio::underruns(),
        audio::dma_errors(),
        esp_alloc::HEAP.free(),
        external_heap,
    );
}

#[cfg(feature = "voice-bench")]
const fn decimal_constant(value: &str) -> usize {
    let bytes = value.as_bytes();
    let mut result = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        result = result * 10 + (bytes[index] - b'0') as usize;
        index += 1;
    }
    result
}

/// Build the deterministic workload in ordinary DRAM, serialise it once into a claimed
/// PSRAM buffer, then release the owned PCM before the engine is constructed. The
/// resulting module borrows both pattern bytes and sample PCM directly from PSRAM.
#[cfg(feature = "voice-bench")]
#[inline(never)]
fn build_voice_bench_module(buffer: &mut psram::Buffer) -> Result<Arc<Module>, &'static str> {
    let config = firmware_common::voice_bench::StressConfig {
        channels: VOICE_BENCH_CHANNELS,
        voices: VOICE_BENCH_VOICES,
        filtered: cfg!(feature = "voice-bench-filtered"),
    };
    let owned = firmware_common::voice_bench::stress_module(config).map_err(|_| "stress_module")?;
    let image = owned.to_image();
    if image.len() > buffer.capacity() {
        return Err("psram_image_capacity");
    }
    // SAFETY: this fresh claim has no borrowers. Every byte is initialized by the copy,
    // then the buffer is never written again for the life of the returned module.
    let destination = unsafe { buffer.as_mut() };
    destination.get_mut(..image.len()).ok_or("psram_bounds")?.copy_from_slice(&image);
    // SAFETY: the preceding copy initialized exactly this range, and it remains immutable.
    let view = unsafe { buffer.view(image.len()) }.ok_or("psram_view")?;
    let borrowed = Module::try_from_image(view).map_err(|_| "psram_image")?;
    drop(image);
    drop(owned);
    Ok(Arc::new(borrowed))
}

#[cfg(feature = "voice-bench")]
#[embassy_executor::task]
async fn voice_bench_module_task(buffer: &'static mut psram::Buffer) {
    VOICE_BENCH_MODULE_READY.signal(build_voice_bench_module(buffer));
}

/// Open and load the benchmark player on a clean executor stack, then leave both large
/// halves in static storage. Async main receives only two references after this task has
/// completed, so its poll frame cannot retain player construction temporaries at boot.
#[cfg(feature = "voice-bench")]
#[inline(never)]
fn build_voice_bench_player(module: Arc<Module>) -> Result<VoiceBenchPlayer, &'static str> {
    let settings = EngineSettings {
        sample_rate_hz: OUTPUT_SAMPLE_RATE_HZ,
        channel_count: VOICE_BENCH_CHANNELS,
        voice_capacity: VOICE_BENCH_VOICES,
        scope_taps: false,
        telemetry_depth: 1,
        layout: EngineLayout::MasterOnly,
        ..EngineSettings::default()
    };
    let player = VOICE_BENCH_PLAYER.init_with(|| EmbeddedPlayer::<Linear>::open_empty(OUTPUT_SAMPLE_RATE_HZ, settings).ok());
    let (render, control) = player.as_mut().ok_or("this build cannot allocate the fixed voice benchmark player")?;
    control.try_load(module).map_err(|_| "this build cannot prepare the voice benchmark")?;
    Ok(VoiceBenchPlayer { render, control })
}

#[cfg(feature = "voice-bench")]
#[embassy_executor::task]
async fn voice_bench_player_task(module: Arc<Module>) {
    VOICE_BENCH_PLAYER_READY.signal(build_voice_bench_player(module));
}

/// One A1S master-volume step: 1/64 of full scale for usable headphone adjustment.
#[cfg(not(feature = "bench"))]
const VOLUME_STEP: U0F16 = U0F16::from_bits(1_024);

/// Maximum time core 0 waits for core 1's I2S startup and one-time muted pre-roll.
///
/// Eight descriptor periods take roughly 42 ms at the production 48 kHz rate and 46 ms in the
/// historical 44.1 kHz diagnostics. One second leaves ample scheduling margin and converts a
/// task that never starts into a muted boot error rather than an infinite spin.
#[cfg(not(feature = "bench"))]
const AUDIO_READY_TIMEOUT: esp_hal::time::Duration = esp_hal::time::Duration::from_secs(1);

/// Wait synchronously so `play` gains no nested async frame while core 1 starts audio.
///
/// A construction or DMA-start error is distinct from a task that never reports back; either
/// result returns while the codec is still muted.
#[cfg(not(feature = "bench"))]
fn wait_for_audio_ready() -> Result<(), &'static str> {
    let started = esp_hal::time::Instant::now();
    loop {
        match audio::startup_status() {
            audio::StartupStatus::Ready => return Ok(()),
            audio::StartupStatus::Failed => return Err("the I2S transmitter would not start"),
            audio::StartupStatus::Pending => {}
        }
        if started.elapsed() >= AUDIO_READY_TIMEOUT {
            return Err("the core-1 audio refill did not complete its pre-roll");
        }
        core::hint::spin_loop();
    }
}

/// How long the control task waits between polls of the key-event channel and the
/// once-per-tick bookkeeping (garbage collection, the display refresh). 10 ms: fine
/// enough that a key's `Hold` repeat cadence (200 ms,
/// `firmware_common::keys::HOLD_REPEAT_INTERVAL_MS`) is never held up behind this loop.
#[cfg(not(feature = "bench"))]
const CONTROL_TICK: Duration = Duration::from_millis(10);

/// The display refreshes at 20 Hz — every 5th control tick — never faster, and never from
/// the audio task (the task file's own rule; [`display_task`] only ever reads what this
/// task signals it).
#[cfg(all(not(feature = "bench"), any(feature = "lcd", feature = "web")))]
const DISPLAY_REFRESH_TICKS: u32 = 5;

/// The UART transport line prints once a second — every 100th control tick.
#[cfg(not(feature = "bench"))]
const LOG_TICKS: u32 = 100;

/// Apply one [`KeyEvent`] to the transport or the volume.
///
/// `key1_hold_actioned` is the five-key `lcd` build's "KEY1 long-press = stop" latch: a
/// `Hold` fires repeatedly every 200 ms once past the threshold (the same mechanism
/// KEY3–KEY6 want for their own repeat), so this remembers whether *this* hold has already
/// stopped the transport, and clears on `Release` — the same shape
/// `RenderHalf::awaiting_engine_stop` uses to keep a level-triggered signal from firing on
/// every poll.
#[cfg(not(feature = "bench"))]
fn apply_key_event(control: &mut ControlHalf, event: KeyEvent, key1_hold_actioned: &mut bool) {
    let current_order = control.telemetry().transport.order;
    match event {
        KeyEvent::Press(Key::Key1) => {
            let _ = if control.is_playing() { control.stop() } else { control.play() };
        }
        #[cfg(feature = "lcd")]
        KeyEvent::Hold(Key::Key1, _) => {
            if !*key1_hold_actioned {
                *key1_hold_actioned = true;
                let _ = control.stop();
                let _ = control.seek_order(0);
            }
        }
        KeyEvent::Release(Key::Key1) => *key1_hold_actioned = false,

        #[cfg(not(feature = "lcd"))]
        KeyEvent::Press(Key::Key2) => {
            let _ = control.stop();
            let _ = control.seek_order(0);
        }

        KeyEvent::Press(Key::Key3) | KeyEvent::Hold(Key::Key3, _) => {
            let _ = control.seek_order(current_order.saturating_sub(1));
        }
        KeyEvent::Press(Key::Key4) | KeyEvent::Hold(Key::Key4, _) => {
            let _ = control.seek_order(current_order.saturating_add(1));
        }
        KeyEvent::Press(Key::Key5) | KeyEvent::Hold(Key::Key5, _) => {
            let volume = firmware_common::lower_master_volume(control.master_volume(), VOLUME_STEP, MAX_MASTER_VOLUME);
            let _ = control.set_master_volume(volume);
        }
        KeyEvent::Press(Key::Key6) | KeyEvent::Hold(Key::Key6, _) => {
            let volume = firmware_common::raise_master_volume(control.master_volume(), VOLUME_STEP, MAX_MASTER_VOLUME);
            let _ = control.set_master_volume(volume);
        }
        _ => {}
    }
}

/// What the control task carries for the web, which in a build without it is nothing.
///
/// A type alias rather than a `#[cfg]` parameter: `#[embassy_executor::task]` rewrites
/// the function signature into a `TaskStorage`, and a parameter that exists in one build
/// and not another is a shape the macro should not have to reason about.
#[cfg(all(not(feature = "bench"), feature = "web"))]
type WebBridge = web::Bridge;
#[cfg(all(not(feature = "bench"), not(feature = "web")))]
type WebBridge = ();

#[cfg(feature = "voice-bench")]
type ControlTaskControl = &'static mut ControlHalf;
#[cfg(all(not(feature = "bench"), not(feature = "voice-bench")))]
type ControlTaskControl = ControlHalf;

/// The control task: the sole owner of [`ControlHalf`]. Drains key events and turns them
/// into transport/volume commands, collects the render half's garbage, signals the
/// display task its next frame (`lcd` only, at [`DISPLAY_REFRESH_TICKS`]), and prints the
/// transport line once a second — everything the audio refill (now on core 1) may not do.
#[cfg(not(feature = "bench"))]
#[embassy_executor::task]
async fn control_task(mut control: ControlTaskControl, bridge: WebBridge, #[cfg(feature = "voice-bench")] voice_bench_external_heap: usize) {
    // One binding, two builds: in a build without `web` the parameter is `()` and exists
    // only so that `#[embassy_executor::task]` sees one signature rather than two.
    #[cfg(feature = "web")]
    let mut bridge = bridge;
    #[cfg(not(feature = "web"))]
    let () = bridge;

    let receiver: keys::KeyEventReceiver = KEY_EVENTS.receiver();
    let mut key1_hold_actioned = false;
    let mut tick: u32 = 0;
    #[cfg(feature = "voice-bench")]
    let benchmark_started = esp_hal::time::Instant::now();
    #[cfg(feature = "voice-bench")]
    let benchmark_start_frame = control.output_frame().0;
    #[cfg(feature = "voice-bench")]
    let benchmark_dma_base = audio::dma_errors();
    #[cfg(feature = "voice-bench")]
    let mut benchmark_sequence = 0u32;
    #[cfg(feature = "voice-bench")]
    let mut benchmark_peak_active = 0u16;
    #[cfg(feature = "voice-bench")]
    let mut benchmark_minimum_heap = esp_alloc::HEAP.free();
    #[cfg(feature = "voice-bench")]
    let mut benchmark_plateau_failed = false;
    #[cfg(feature = "voice-bench")]
    let mut benchmark_web_load_failed = false;
    #[cfg(feature = "voice-bench")]
    let mut benchmark_previous_web_counts = [0u32; 3];
    #[cfg(feature = "voice-bench")]
    let mut benchmark_web_last_advance_ms = [0u64; 3];
    #[cfg(all(feature = "voice-bench", feature = "web"))]
    let benchmark_web_base = web::voice_bench_load_counts();
    #[cfg(all(feature = "voice-bench", not(feature = "web")))]
    let benchmark_web_base = [0u32; 3];
    #[cfg(feature = "voice-bench")]
    let mut benchmark_finished = false;
    #[cfg(feature = "voice-bench")]
    println!(
        "VOICE_BENCH v=1 seq={} kind=START case={} mode={} filtered={} channels={} voices={} rate={} descriptor_frames={} duration_s={} phase={} axis={} low={} high={} dma_base={}",
        benchmark_sequence,
        VOICE_BENCH_CASE,
        if cfg!(feature = "web") { "web" } else { "audio" },
        u8::from(cfg!(feature = "voice-bench-filtered")),
        VOICE_BENCH_CHANNELS,
        VOICE_BENCH_VOICES,
        OUTPUT_SAMPLE_RATE_HZ,
        audio::DESCRIPTOR_FRAMES,
        VOICE_BENCH_SECONDS,
        VOICE_BENCH_PHASE,
        VOICE_BENCH_AXIS,
        VOICE_BENCH_LOW,
        VOICE_BENCH_HIGH,
        benchmark_dma_base,
    );
    #[cfg(feature = "voice-bench")]
    {
        benchmark_sequence = benchmark_sequence.wrapping_add(1);
    }
    loop {
        Timer::after(CONTROL_TICK).await;
        tick = tick.wrapping_add(1);

        #[cfg(feature = "voice-bench")]
        {
            benchmark_minimum_heap = benchmark_minimum_heap.min(esp_alloc::HEAP.free());
        }

        while let Ok(event) = receiver.try_receive() {
            apply_key_event(&mut control, event, &mut key1_hold_actioned);
        }

        // The web's own commands and jobs, on the same task and therefore under the same
        // single owner of `ControlHalf`.
        #[cfg(feature = "web")]
        bridge.poll(&mut control).await;

        #[cfg(feature = "voice-bench")]
        {
            benchmark_minimum_heap = benchmark_minimum_heap.min(esp_alloc::HEAP.free());
        }

        #[cfg(any(feature = "lcd", feature = "web"))]
        if tick % DISPLAY_REFRESH_TICKS == 0 {
            let snapshot = *control.telemetry();
            let title = control.module().map(|module| module.header().title.as_ref()).unwrap_or("");
            let view = NowPlaying::from_snapshot(&snapshot, OUTPUT_SAMPLE_RATE_HZ, title, control.master_volume());
            #[cfg(feature = "lcd")]
            NOW_PLAYING.signal(view);
            #[cfg(feature = "web")]
            bridge.publish(&mut control, &view);
            let _ = view;
        }

        if tick % LOG_TICKS == 0 {
            let retired = control.collect_garbage();
            let snapshot = *control.telemetry();
            #[cfg(feature = "voice-bench")]
            {
                let _ = retired;
                benchmark_peak_active = benchmark_peak_active.max(snapshot.voices_active);
                let timing = audio::render_timing();
                let elapsed_ms = benchmark_started.elapsed().as_millis();
                let frames = control.output_frame().0.saturating_sub(benchmark_start_frame);
                let warnings = u8::from(control.warnings().any());
                if elapsed_ms >= firmware_common::voice_bench::VOICE_PLATEAU_SETTLE_MS
                    && snapshot.voices_active as usize != VOICE_BENCH_VOICES
                {
                    benchmark_plateau_failed = true;
                }
                #[cfg(feature = "web")]
                let web_counts = web::voice_bench_load_counts();
                #[cfg(not(feature = "web"))]
                let web_counts = [0u32; 3];
                let web_counts = [
                    web_counts[0].wrapping_sub(benchmark_web_base[0]),
                    web_counts[1].wrapping_sub(benchmark_web_base[1]),
                    web_counts[2].wrapping_sub(benchmark_web_base[2]),
                ];
                for index in 0..web_counts.len() {
                    if web_counts[index] > benchmark_previous_web_counts[index] {
                        benchmark_web_last_advance_ms[index] = elapsed_ms;
                    } else if cfg!(feature = "web")
                        && elapsed_ms >= firmware_common::voice_bench::WEB_LOAD_SETTLE_MS
                        && web_counts[index] < benchmark_previous_web_counts[index]
                    {
                        benchmark_web_load_failed = true;
                    }
                }
                if cfg!(feature = "web") && elapsed_ms >= firmware_common::voice_bench::WEB_LOAD_SETTLE_MS {
                    for index in 0..web_counts.len() {
                        if web_counts[index] == 0
                            || elapsed_ms.saturating_sub(benchmark_web_last_advance_ms[index])
                                > firmware_common::voice_bench::WEB_LOAD_STALL_MS
                        {
                            benchmark_web_load_failed = true;
                        }
                    }
                }
                benchmark_previous_web_counts = web_counts;
                let finished_now = elapsed_ms >= (VOICE_BENCH_SECONDS as u64).saturating_mul(1_000);
                let kind = if finished_now { "END" } else { "SAMPLE" };
                let pass = snapshot.voices_active as usize == VOICE_BENCH_VOICES
                    && benchmark_peak_active as usize >= VOICE_BENCH_VOICES
                    && !benchmark_plateau_failed
                    && !benchmark_web_load_failed
                    && timing.maximum_us <= 4_266
                    && timing.misses == 0
                    && audio::underruns() == 0
                    && audio::dma_errors() == benchmark_dma_base
                    && warnings == 0
                    && control.voice_steals() > 0
                    && benchmark_minimum_heap >= 8 * 1024
                    && (!cfg!(feature = "web") || web_counts.iter().all(|count| *count > 0));
                if !benchmark_finished {
                    if finished_now {
                        println!(
                            "VOICE_BENCH v=1 seq={} kind={} case={} elapsed_ms={} frames={} active={} peak_active={} render_max_us={} render_p50_us={} render_p95_us={} misses={} underruns={} dma_errors={} warnings={} steals={} heap_internal={} heap_external={} web_http_ok={} web_upload_ok={} web_ws_ok={} status={} reason={}",
                            benchmark_sequence, kind, VOICE_BENCH_CASE, elapsed_ms, frames, snapshot.voices_active,
                            benchmark_peak_active, timing.maximum_us, timing.p50_us, timing.p95_us, timing.misses,
                            audio::underruns(), audio::dma_errors(), warnings, control.voice_steals(), benchmark_minimum_heap,
                            voice_bench_external_heap, web_counts[0], web_counts[1], web_counts[2],
                            if pass { "pass" } else { "reject" }, if pass { "none" } else { "criteria" },
                        );
                        benchmark_finished = true;
                    } else {
                        println!(
                            "VOICE_BENCH v=1 seq={} kind={} case={} elapsed_ms={} frames={} active={} peak_active={} render_max_us={} render_p50_us={} render_p95_us={} misses={} underruns={} dma_errors={} warnings={} steals={} heap_internal={} heap_external={} web_http_ok={} web_upload_ok={} web_ws_ok={}",
                            benchmark_sequence, kind, VOICE_BENCH_CASE, elapsed_ms, frames, snapshot.voices_active,
                            benchmark_peak_active, timing.maximum_us, timing.p50_us, timing.p95_us, timing.misses,
                            audio::underruns(), audio::dma_errors(), warnings, control.voice_steals(), benchmark_minimum_heap,
                            voice_bench_external_heap, web_counts[0], web_counts[1], web_counts[2],
                        );
                    }
                    benchmark_sequence = benchmark_sequence.wrapping_add(1);
                }
            }
            #[cfg(not(feature = "voice-bench"))]
            {
            let title = control.module().map(|module| module.header().title.as_ref()).unwrap_or("");
            let view = NowPlaying::from_snapshot(&snapshot, OUTPUT_SAMPLE_RATE_HZ, title, control.master_volume());
            // What one descriptor of audio actually costs to render, against the 5 333 us it is
            // worth. This is the number that decides whether a slow song is the transport or the
            // renderer, and inferring it from the transport counters cannot separate rendering
            // from the refill's legitimate waiting.
            #[cfg(feature = "render-timing")]
            {
                let timing = audio::render_timing();
                println!(
                    "RENDER p50={}us p95={}us max={}us over_budget={} (budget 5333us)",
                    timing.p50_us, timing.p95_us, timing.maximum_us, timing.misses,
                );
            }
            println!(
                "{view}  peak={} underruns={} dma_errors={} offered={} written={} pushes={} retired={} rejected={}",
                control.peak(),
                audio::underruns(),
                audio::dma_errors(),
                audio::steady_available_bytes(),
                audio::steady_written_bytes(),
                audio::steady_push_calls(),
                retired,
                control.commands_rejected(),
            );
            }
        }
    }
}

/// Redraw the screen whenever the control task signals a new [`NowPlaying`]. `lcd` only.
///
/// This task never touches [`ControlHalf`] — the control task is its sole owner — and
/// never reads faster than the control task signals (at most [`DISPLAY_REFRESH_TICKS`]
/// per second, i.e. 20 Hz), which is what keeps the display's SPI traffic off the audio
/// path (now on core 1 regardless) and bounded on core 0.
#[cfg(all(not(feature = "bench"), feature = "lcd"))]
#[embassy_executor::task]
async fn display_task(mut display: lcd::LcdDisplay) {
    let mut screen = firmware_common::Screen::new();
    loop {
        let view = NOW_PLAYING.wait().await;
        let _ = screen.render(&mut display, &view);
    }
}
