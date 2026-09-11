//! StarPlayer on the AI-Thinker ESP32-Audio-Kit.
//!
//! Two builds out of one crate:
//!
//! * the **audio** build (no features) plays `REFLEX.S3M` from flash through the ES8388
//!   into the headphone jack and the speaker outputs, takes transport and volume
//!   commands from six push-buttons (M8-I5), optionally shows the sounding row on an
//!   ST7789 screen behind the `lcd` feature (M8-I5), and logs the transport once a
//!   second;
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
//! 6. The module image → [`Module::from_image`] → [`EmbeddedPlayer`], and only then I2S,
//!    because the DMA ring is primed with real audio before the transfer starts.
//! 7. Unmute, enable the speaker amplifier, start the second core with the audio refill
//!    on it, and spawn the keys task, the control task and (under `lcd`) the display
//!    task on core 0.
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
//! **The audio refill now runs on core 1**, with core 0 running the keys task, the
//! control task and (under `lcd`) the display task. M8-I3's write-up blamed `RenderHalf`
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

#![no_std]
#![no_main]
// `#[embassy_executor::task]` and `#[esp_rtos::main]` expand to a `TaskStorage` whose
// future type is an associated `impl Trait`. Nightly-only, which is one of the reasons the
// firmware is its own workspace on its own toolchain (M8 master-plan decision 1).
#![feature(impl_trait_in_assoc_type)]

extern crate alloc;

#[cfg(not(feature = "bench"))]
mod audio;
#[cfg(feature = "bench")]
mod bench;
#[cfg(not(feature = "bench"))]
mod board;
#[cfg(not(feature = "bench"))]
mod es8388;
mod images;
#[cfg(not(feature = "bench"))]
mod keys;
#[cfg(all(not(feature = "bench"), feature = "lcd"))]
mod lcd;
#[cfg(all(not(feature = "bench"), feature = "web"))]
mod net;
#[cfg(all(not(feature = "bench"), feature = "web"))]
mod provisioning;
#[cfg(all(not(feature = "bench"), feature = "web"))]
mod psram;
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
use firmware_common::{Key, KeyEvent, NowPlaying};
#[cfg(not(feature = "bench"))]
use starplayer::core::U0F16;
#[cfg(not(feature = "bench"))]
use starplayer::dsp::Linear;
#[cfg(not(feature = "bench"))]
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
/// `27 800 + 184 × voices + 5 288 × channels` bytes
/// (`starplayer_host_embedded::settings_for`), which for `PETRI.S3M` — eight channels,
/// eight voices — is 71 576; the scanned song timeline, the `Arc`s and the sequencer sit
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
/// **The `web` build does not put its heap here at all.** It adds a great deal of `.bss`
/// — the WiFi driver's statics, embassy-net's socket storage, the web workers'
/// task-pool futures with their TCP buffers inside them — and every byte of that comes
/// out of the same main stack this array would. Measured: with the heap in `.bss` the
/// firmware does not link, because `.bss` alone reaches past `0x3ffe_0000` and the stack
/// has nowhere to start. So the `web` build's heap goes to [`RECLAIMED_HEAP_BYTES`] in
/// `dram2_seg` instead, and `dram_seg` carries `.bss` and the stack and nothing else.
#[cfg(not(feature = "web"))]
const HEAP_BYTES: usize = 120 * 1024;
#[cfg(feature = "web")]
const HEAP_BYTES: usize = RECLAIMED_HEAP_BYTES;

/// The `web` build's entire heap, in `dram2_seg`.
///
/// `dram2_seg` is the 98 768 bytes of DRAM above the ROM's own data and stacks
/// (`esp-hal`'s `ld/esp32/memory.x`), which the ESP-IDF heap reclaims and which esp-hal
/// leaves as an uninitialised section with no other user. It is ordinary internal DRAM —
/// atomics work there, unlike PSRAM — and, crucially, it is **not** `.bss` in `dram_seg`,
/// so it does not shorten the main stack. ampkeeper's classic-ESP32 build put 96 KiB of
/// its heap here for exactly this reason after a provisioning-mode allocation failure.
///
/// 96 KiB of the region's 98 768 bytes: all of it but a rounding margin. It is less than
/// the default build's 120 KiB, which is the real cost of the radio — the engine's own
/// requirement for `PETRI.S3M` is 71 576 bytes, so what is left over for the WiFi driver,
/// the TCP stack and an uploaded module's index vectors is about 25 KiB.
#[cfg(feature = "web")]
const RECLAIMED_HEAP_BYTES: usize = 96 * 1024;

/// The render half lives in a `static`: `size_of::<RenderHalf<Linear>>()` is 11 744 bytes
/// (I1 research point 3a), because the engine's telemetry publisher holds a working
/// `Snapshot` inline and this host holds another. Passing that down a call chain by value
/// would put two copies of it on the stack.
#[cfg(not(feature = "bench"))]
static RENDER: StaticCell<RenderHalf<Linear>> = StaticCell::new();

/// Core 1's stack. Only the audio refill task runs there — no allocation, no logging, one
/// small local scratch buffer (`audio::fill`'s `[i16; QUANTUM_FRAMES * 2]`, 512 bytes) —
/// so this is deliberately far smaller than core 0's, which carries the whole engine's
/// call depth.
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

/// Carries [`audio::AudioTransfer`] across the one-time ownership handoff into core 1's
/// entry closure.
///
/// `AudioTransfer` is not `Send`: esp-hal's DMA transfer type addresses its descriptor
/// ring through raw pointers (`*mut DmaDescriptor`, `*const u8`, and `Async`'s own marker
/// `PhantomData<*const ()>`), none of which carry `unsafe impl Send`. This crate is the
/// one place `unsafe` is tolerated in the whole workspace (M8 master-plan decision 6), and
/// this is exactly the shape `esp_rtos::start_second_core`'s own internal
/// `SecondCoreStack` wrapper exists to cross (`esp-rtos` 0.3.0's `lib.rs`): a value moved
/// **once**, into the closure that becomes core 1's entire program, with core 0 never
/// touching it again afterwards. `Send`'s actual safety property — no two threads ever
/// read or write the same memory concurrently — holds because there is only ever one
/// owner at a time, never two.
#[cfg(not(feature = "bench"))]
struct SendTransfer(audio::AudioTransfer);

// SAFETY: see the doc comment on `SendTransfer` above.
#[cfg(not(feature = "bench"))]
unsafe impl Send for SendTransfer {}

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

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()));

    esp_println::logger::init_logger_from_env();
    // The `web` build's heap is in `dram2_seg`, not in `.bss` — see `HEAP_BYTES`.
    #[cfg(not(feature = "web"))]
    esp_alloc::heap_allocator!(size: HEAP_BYTES);
    #[cfg(feature = "web")]
    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: HEAP_BYTES);

    let timer = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG1);
    let software_interrupts = esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timer.timer0, software_interrupts.software_interrupt0);

    println!();
    println!("StarPlayer {} on the ESP32-A1S Audio Kit", env!("CARGO_PKG_VERSION"));
    println!("CPU  {} MHz   heap {}", esp_hal::clock::cpu_clock().as_mhz(), Kib(HEAP_BYTES));

    // PSRAM: mapped and reported in both builds, registered only by the bench. See the
    // module docs, "PSRAM and atomics".
    let psram = esp_hal::psram::Psram::new(peripherals.PSRAM, esp_hal::psram::PsramConfig::default());
    let (psram_start, psram_size) = psram.raw_parts();
    println!("PSRAM {psram_size} bytes ({}) mapped at {psram_start:p}", Kib(psram_size));

    #[cfg(not(any(feature = "bench", feature = "web")))]
    let _ = psram_start;

    // The `web` build takes the PSRAM region as an arena of its own instead of handing it
    // to the allocator — see `psram.rs` for the atomics erratum that forbids the latter.
    #[cfg(all(not(feature = "bench"), feature = "web"))]
    let mut psram_arena = psram::Arena::new(psram_start, psram_size);

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
                println!("FATAL: the flash partitions are not what this firmware expects: {error:?}");
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
        web::Boot { bridge, credentials, wifi: peripherals.WIFI, seed }
    };

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
                )
                .await
            }
            Err(_) => Err("the I2C controller rejected its configuration"),
        }
    };

    #[cfg(not(feature = "bench"))]
    if let Err(message) = started {
        // A boot that cannot make sound is not a boot that should pretend to. Say what
        // failed, loudly and repeatedly, rather than resetting into the same failure.
        loop {
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
) -> Result<(), &'static str> {
    // Research point 1: what is actually on the control bus. Printed before anything is
    // configured, so a board that answers nowhere is diagnosable from the first boot.
    report_i2c_scan(&mut board);
    println!("JACK headphone_detect={}", if board.headphone_detect.is_low() { "inserted" } else { "empty" });

    let mut codec = es8388::Es8388::new(board.i2c, board::ES8388_I2C_ADDRESS);
    codec.init_dac_only(es8388::outputs::ALL).map_err(|_| "the ES8388 did not answer — is this the AC101 revision?")?;
    println!("CODEC ES8388 at 0x{:02x}: DAC up, 16-bit Philips slave, MCLK/LRCK 256, muted", board::ES8388_I2C_ADDRESS);

    #[cfg(feature = "lcd")]
    let display = lcd::LcdDisplay::take(lcd_parts);
    #[cfg(feature = "lcd")]
    println!("LCD  ST7789 {}", if display.is_present() { "found" } else { "not found — continuing headless" });

    // The module: borrowed straight out of memory-mapped flash, PCM and all.
    let image = images::boot_module();
    let module = Arc::new(Module::from_image(image).map_err(|_| "the linked module image would not borrow — is it 4-byte aligned?")?);
    println!(
        "MODULE image={} bytes ({}) channels={} samples={}",
        image.len(),
        Kib(image.len()),
        module.header().channel_count,
        module.samples().len(),
    );

    let (render, mut control) = EmbeddedPlayer::<Linear>::open(Arc::clone(&module), firmware_common::SAMPLE_RATE_HZ)
        .map_err(|_| "this build cannot play that module")?;
    let render = RENDER.init(render);
    println!("HEAP after open: {}", esp_alloc::HEAP.stats());

    control.play().map_err(|_| "the command ring rejected the first play")?;

    // I2S last, and with the render half in hand: `audio::start` primes the whole DMA ring
    // with real audio before the circular transfer begins, so the first sample the codec
    // sees is music rather than 23 ms of silence.
    let transfer = audio::start(audio_parts, firmware_common::SAMPLE_RATE_HZ, render)
        .map_err(|_| "the I2S transmitter would not start")?;
    println!(
        "I2S  44100 Hz stereo 16-bit, MCLK on GPIO{}, DMA ring {} quanta ({} frames, {} ms)",
        board::PIN_I2S_MCLK,
        audio::DMA_RING_QUANTA,
        audio::DMA_RING_QUANTA * audio::QUANTUM_FRAMES,
        audio::DMA_RING_QUANTA * audio::QUANTUM_FRAMES * 1000 / firmware_common::SAMPLE_RATE_HZ as usize,
    );

    // Sound, in this order: the DMA is already running, so unmuting the codec now cannot
    // catch it with an empty ring, and the amplifier comes up after the codec so the
    // speakers never hear the unmute transient.
    codec.mute(false).map_err(|_| "the codec would not unmute")?;
    board.power_amplifier.set_high();
    println!("PLAY");

    // The refill runs on core 1 — see the module docs, "Core pinning — corrected by
    // M8-I5". Nothing crosses back: the whole task is built inside the second core's own
    // entry closure and spawned on its own executor, so core 0 never holds a `SendSpawner`
    // for it and this board never spawns a second task there.
    //
    // `transfer` (`audio::AudioTransfer`) has to cross into that closure and is not
    // `Send`: esp-hal's DMA transfer type addresses its descriptor ring through raw
    // pointers with no `unsafe impl Send`, the exact shape `esp_rtos::start_second_core`'s
    // own internal `SecondCoreStack` wrapper exists to cross (`esp-rtos` 0.3.0's own
    // `lib.rs`). `render` needs no such wrapper — `RenderHalf<Linear>` is `Send` (see the
    // module docs and the assertion beside its definition), so `&'static mut RenderHalf`
    // is `Send` too.
    let core1_stack = CORE1_STACK.take();
    let transfer = SendTransfer(transfer);
    esp_rtos::start_second_core(cpu_control, software_interrupt1, core1_stack, move || {
        // `let transfer = transfer;` before touching `.0` forces the closure to capture
        // the *whole* `SendTransfer` by move rather than edition-2021 disjoint capture
        // reaching in and capturing the `AudioTransfer` field directly — which would
        // recreate exactly the `Send` error `SendTransfer` exists to route around, since
        // the field itself carries no `unsafe impl Send`.
        let transfer = transfer;
        let transfer = transfer.0;
        CORE1_EXECUTOR.init(esp_rtos::embassy::Executor::new()).run(|core1_spawner| {
            let token = audio::refill_task(transfer, render).expect("the refill task is the only thing core 1's executor ever spawns");
            core1_spawner.spawn(token);
        });
    });
    println!("CORE1 audio refill running");

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
    let web::Boot { bridge, credentials, wifi, seed } = web_boot;

    spawner.spawn(keys::keys_task(board.keys, KEY_EVENTS.sender()).map_err(|_| "the keys task would not spawn")?);
    #[cfg(not(feature = "web"))]
    spawner.spawn(control_task(control, ()).map_err(|_| "the control task would not spawn")?);
    #[cfg(feature = "web")]
    spawner.spawn(control_task(control, bridge).map_err(|_| "the control task would not spawn")?);
    #[cfg(feature = "lcd")]
    spawner.spawn(display_task(display).map_err(|_| "the display task would not spawn")?);

    // The network personality comes last: the codec, the DMA ring and core 1 are all up,
    // so the radio's own bring-up cannot delay the first sample.
    #[cfg(feature = "web")]
    {
        match credentials.filter(|_| !reprovision) {
            Some(credentials) => {
                println!("WIFI joining {}", credentials.ssid.as_str());
                let station = net::start(spawner, wifi, credentials, seed)?;
                web::start(spawner, station.stack)?;
            }
            None => provisioning::run(spawner, wifi, seed).await?,
        }
        spawner.spawn(reboot_task().map_err(|_| "the reboot watcher would not spawn")?);
    }
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

/// One master-volume step: `1/16` of full scale, the task file's own figure for KEY5/KEY6.
#[cfg(not(feature = "bench"))]
const VOLUME_STEP: U0F16 = U0F16::from_bits(u16::MAX / 16);

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
            let volume = U0F16::from_bits(control.master_volume().to_bits().saturating_sub(VOLUME_STEP.to_bits()));
            let _ = control.set_master_volume(volume);
        }
        KeyEvent::Press(Key::Key6) | KeyEvent::Hold(Key::Key6, _) => {
            let volume = U0F16::from_bits(control.master_volume().to_bits().saturating_add(VOLUME_STEP.to_bits()));
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

/// The control task: the sole owner of [`ControlHalf`]. Drains key events and turns them
/// into transport/volume commands, collects the render half's garbage, signals the
/// display task its next frame (`lcd` only, at [`DISPLAY_REFRESH_TICKS`]), and prints the
/// transport line once a second — everything the audio refill (now on core 1) may not do.
#[cfg(not(feature = "bench"))]
#[embassy_executor::task]
async fn control_task(mut control: ControlHalf, bridge: WebBridge) {
    // One binding, two builds: in a build without `web` the parameter is `()` and exists
    // only so that `#[embassy_executor::task]` sees one signature rather than two.
    #[cfg(feature = "web")]
    let mut bridge = bridge;
    #[cfg(not(feature = "web"))]
    let () = bridge;

    let receiver: keys::KeyEventReceiver = KEY_EVENTS.receiver();
    let mut key1_hold_actioned = false;
    let mut tick: u32 = 0;
    loop {
        Timer::after(CONTROL_TICK).await;
        tick = tick.wrapping_add(1);

        while let Ok(event) = receiver.try_receive() {
            apply_key_event(&mut control, event, &mut key1_hold_actioned);
        }

        // The web's own commands and jobs, on the same task and therefore under the same
        // single owner of `ControlHalf`.
        #[cfg(feature = "web")]
        bridge.poll(&mut control).await;

        #[cfg(any(feature = "lcd", feature = "web"))]
        if tick % DISPLAY_REFRESH_TICKS == 0 {
            let snapshot = *control.telemetry();
            let title = control.module().map(|module| module.header().title.as_ref()).unwrap_or("");
            let view = NowPlaying::from_snapshot(&snapshot, firmware_common::SAMPLE_RATE_HZ, title, control.master_volume());
            #[cfg(feature = "lcd")]
            NOW_PLAYING.signal(view);
            #[cfg(feature = "web")]
            bridge.publish(&mut control, &view);
            let _ = view;
        }

        if tick % LOG_TICKS == 0 {
            let retired = control.collect_garbage();
            let snapshot = *control.telemetry();
            let title = control.module().map(|module| module.header().title.as_ref()).unwrap_or("");
            let view = NowPlaying::from_snapshot(&snapshot, firmware_common::SAMPLE_RATE_HZ, title, control.master_volume());
            println!(
                "{view}  peak={} underruns={} dma_errors={} retired={} rejected={}",
                control.peak(),
                audio::underruns(),
                audio::dma_errors(),
                retired,
                control.commands_rejected(),
            );
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
