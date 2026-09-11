//! StarPlayer on the AI-Thinker ESP32-Audio-Kit.
//!
//! Two builds out of one crate:
//!
//! * the **audio** build (no features) plays `PETRI.S3M` from flash through the ES8388
//!   into the headphone jack and the speaker outputs, and logs the transport once a
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
//! 4. `esp_rtos::start`, then the board's own pins.
//! 5. An I2C scan, logged (research point 1), then the codec — configured but **muted**.
//! 6. The module image → [`Module::from_image`] → [`EmbeddedPlayer`], and only then I2S,
//!    because the DMA ring is primed with real audio before the transfer starts.
//! 7. Unmute, enable the speaker amplifier, spawn the refill and the control task.
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
//! # Core pinning — research point 6
//!
//! The audio task runs on **core 0**, with the control task, and not on a second-core
//! executor as the task file's deliverable sketches. The blocker is a type, not the
//! chip: `Engine` holds `Box<dyn EventSource>` — no `+ Send` — so `RenderHalf` is not
//! `Send` and cannot be handed to `esp_rtos`'s second-core `SendSpawner`. The two ways out
//! are (a) `Box<dyn EventSource + Send>` in `starplayer-engine`, which the task file puts
//! out of scope ("any change to the engine crates: stop and write it up"), or (b) building
//! the whole player *inside* the second core's entry closure so nothing crosses, which
//! moves the control half to core 1 as well and wants the command channel M8-I5 and M8-I6
//! will need anyway. Neither is needed for this milestone's exit criterion: there is no
//! WiFi and no display yet, so core 0 has nothing to be starved by. The underrun counter
//! is printed every second so the owner's run measures what single-core costs.

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

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_println::println;
use firmware_common::format::Kib;
#[cfg(not(feature = "bench"))]
use firmware_common::NowPlaying;
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
const HEAP_BYTES: usize = 120 * 1024;

/// The render half lives in a `static`: `size_of::<RenderHalf<Linear>>()` is 11 744 bytes
/// (I1 research point 3a), because the engine's telemetry publisher holds a working
/// `Snapshot` inline and this host holds another. Passing that down a call chain by value
/// would put two copies of it on the stack.
#[cfg(not(feature = "bench"))]
static RENDER: StaticCell<RenderHalf<Linear>> = StaticCell::new();

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()));

    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(size: HEAP_BYTES);

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

    #[cfg(feature = "bench")]
    let _ = psram_start;

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

    #[cfg(not(feature = "bench"))]
    let started = {
        let board = board::Board::take(
            peripherals.I2C0,
            peripherals.GPIO33,
            peripherals.GPIO32,
            peripherals.GPIO21,
            peripherals.GPIO39,
        );
        let audio_parts = audio::Parts {
            i2s0: peripherals.I2S0,
            dma: peripherals.DMA_I2S0,
            mclk: peripherals.GPIO0,
            bclk: peripherals.GPIO27,
            lrck: peripherals.GPIO25,
            dout: peripherals.GPIO26,
        };
        match board {
            Ok(board) => play(spawner, board, audio_parts).await,
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

/// Bring the codec and I2S up and start playing. Only the audio build compiles it.
#[cfg(not(feature = "bench"))]
async fn play(spawner: Spawner, mut board: board::Board<'static>, audio_parts: audio::Parts) -> Result<(), &'static str> {
    // Research point 1: what is actually on the control bus. Printed before anything is
    // configured, so a board that answers nowhere is diagnosable from the first boot.
    report_i2c_scan(&mut board);
    println!("JACK headphone_detect={}", if board.headphone_detect.is_low() { "inserted" } else { "empty" });

    let mut codec = es8388::Es8388::new(board.i2c, board::ES8388_I2C_ADDRESS);
    codec.init_dac_only(es8388::outputs::ALL).map_err(|_| "the ES8388 did not answer — is this the AC101 revision?")?;
    println!("CODEC ES8388 at 0x{:02x}: DAC up, 16-bit Philips slave, MCLK/LRCK 256, muted", board::ES8388_I2C_ADDRESS);

    // The module: borrowed straight out of memory-mapped flash, PCM and all.
    let image = images::petri_s3m();
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

    spawner.spawn(audio::refill_task(transfer, render).map_err(|_| "the refill task would not spawn")?);
    spawner.spawn(control_task(control).map_err(|_| "the control task would not spawn")?);
    Ok(())
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

/// Once a second: retire what the render loop handed back, and say where the song is.
///
/// Everything the refill may not do. `collect_garbage` is the reason it exists at all — a
/// retired module or sequencer is dropped **here**, on a task that can afford a
/// deallocation, never in the DMA refill (design goal 5).
#[cfg(not(feature = "bench"))]
#[embassy_executor::task]
async fn control_task(mut control: ControlHalf) {
    loop {
        Timer::after(Duration::from_secs(1)).await;
        let retired = control.collect_garbage();
        let view = NowPlaying::from_snapshot(control.telemetry(), firmware_common::SAMPLE_RATE_HZ);
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
