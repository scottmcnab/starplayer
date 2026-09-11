//! StarPlayer on the ESP32-C5 — the RISC-V bench, and nothing else.
//!
//! This chip has no audio codec, no I2S, no PSRAM on the devkit in hand (I4 research
//! resolution) — there is no "audio" build to mirror the A1S's. Two builds out of one
//! crate instead:
//!
//! * the **default** build (no features) boots, prints the chip and its clock, and idles.
//!   A fast link-only smoke build with no module image in it at all.
//! * the **`bench`** build ([`bench`]) links all six golden fixtures and renders every one
//!   of them through every interpolator, printing a `BENCH …` line per row in exactly the
//!   format `starplayer-a1s`'s bench build does (`firmware_common::bench` is the shared
//!   code both boards drive) — so the two architectures' cycle counts are directly
//!   comparable. **This is the whole point of the board**, and it is the build the
//!   verification section and the owner's flash step both mean by "the C5 bench".
//!
//! # No embassy, no `esp-rtos`, deliberately
//!
//! The A1S needs `esp-rtos`'s cooperative scheduler because it has a DMA refill task and a
//! once-a-second control task running concurrently with I2S. This board renders six
//! fixtures once at boot and then idles forever — there is nothing to schedule. Skipping
//! `embassy-executor` (and the `nightly` feature it would need for `#[embassy_executor::task]`'s
//! `impl_trait_in_assoc_type`) is what makes I4 research point 2's plain-`rustup`-builds
//! finding possible: `#[esp_hal::main]` is a synchronous, non-async entry point, and
//! nothing in this crate's dependency graph asks for a nightly language feature.
//!
//! # Boot order
//!
//! 1. `esp_hal::init` at the maximum CPU clock.
//! 2. The heap ([`HEAP_BYTES`]).
//! 3. `bench` build only: [`bench::run`] over every golden fixture, then idle. Default
//!    build: idle immediately.

#![no_std]
#![no_main]

extern crate alloc;

#[cfg(feature = "bench")]
mod bench;
#[cfg(feature = "bench")]
mod images;

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_println::println;
use firmware_common::format::Kib;

esp_app_desc!();

/// The heap.
///
/// This chip's unified instruction/data RAM (`esp-hal`'s `ld/esp32c5/memory.x`) is
/// materially larger than the classic ESP32's DRAM segment the A1S balances against — on
/// the order of 313 KiB in the primary region plus a further 64 KiB reclaimed after the
/// second-stage bootloader hands off, none of it shared with code (code executes out of
/// the flash-mapped ROM region, not out of this RAM, except for the small `#[ram]`
/// sections `esp-hal` itself places).
///
/// 176 KiB, sized against the same engine heap formula the A1S measured
/// (`starplayer_host_embedded::settings_for`: `27 800 + 184 × voices + 5 288 × channels`)
/// rather than against a device reading, because there is no board in hand to read from —
/// see `plans/reference/embedded-budget.md`'s C5 rows. `PETRI.S3M` (8 channels, 8 voices)
/// is the formula's largest fixture at 71 576 bytes, measured on a **32-bit** host and
/// therefore already an upper bound on this 32-bit target (I1 research point 3a: the
/// formula was measured against a 64-bit `Snapshot`, and a 32-bit one is smaller — the A1S
/// found exactly this margin in its own favour). The [`bench`] build's own `DRAM_STAGING_BYTES`
/// (32 KiB) is carved out of this **same** heap, exactly as the A1S's is (both boards'
/// `Staging::claim` calls `esp_alloc::HEAP.alloc_caps`, not a separate static) — so the
/// budget that has to hold is `DRAM_STAGING_BYTES` *and* `PETRI.S3M`'s engine at once, not
/// either alone: 32 768 + 71 576 = 104 344 bytes at the formula's upper bound. 176 KiB
/// (180 224 bytes) leaves close to double that as headroom against allocator overhead and
/// against the formula being an upper bound rather than this target's real figure — a
/// margin chosen deliberately generous because a heap that is merely *adequate* here would
/// be a bench build that OOMs on exactly the fixture the milestone's exit criterion is
/// stated in terms of, on a board with no way to attach a debugger.
const HEAP_BYTES: usize = 176 * 1024;

#[esp_hal::main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()));
    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(size: HEAP_BYTES);

    println!();
    println!("StarPlayer {} on the ESP32-C5 (RISC-V, no audio hardware)", env!("CARGO_PKG_VERSION"));
    println!("CPU  {} MHz   heap {}", esp_hal::clock::cpu_clock().as_mhz(), Kib(HEAP_BYTES));

    #[cfg(feature = "bench")]
    bench::run(&firmware_common::bench::BenchPlan::GOLDEN);

    #[cfg(not(feature = "bench"))]
    println!("default build: no module linked — rebuild with --features bench to render the golden fixtures");

    println!("idle");
    loop {
        core::hint::spin_loop();
    }
}
