//! The `bench` build: render every golden fixture and print what it cost, on RISC-V.
//!
//! Structurally this is `starplayer-a1s/src/bench.rs` with the codec-adjacent concerns
//! removed and the cycle counter swapped for the chip this board actually has. The **line
//! format is unchanged** — both boards drive
//! [`firmware_common::bench::digest_row`](starplayer_firmware_common::bench::digest_row)
//! and print its [`Display`](core::fmt::Display), so a `BENCH …` line from this board and
//! one from the A1S are directly comparable, which is the entire point of I4.
//!
//! # The cycle counter needs no derivation here
//!
//! The A1S reads esp-hal's 64-bit microsecond timer and scales it, because the Xtensa
//! `CCOUNT` register is only 32 bits and wraps inside a ten-second render. RISC-V's `mcycle`
//! CSR is architecturally 64 bits everywhere (on `riscv32imac` the low and high halves are
//! two separate 32-bit CSRs, `mcycle` and `mcycleh`), and [`riscv::register::mcycle::read64`]
//! already does the standard "read hi, read lo, re-read hi, retry if it changed" dance to
//! assemble a consistent 64-bit value across a `mcycle` wrap — no derivation needed, and no
//! wraparound this board could hit in a single boot's lifetime. This is I4's deliverable:
//! `CycleClock::now` is one function call.
//!
//! # Where the PCM lives
//!
//! * `flash` — [`Module::from_image`] over the `#[repr(C, align(4))]` `include_bytes!`
//!   wrapper, PCM borrowed in place out of memory-mapped flash.
//! * `psram` — **always skipped.** This devkit has no PSRAM fitted (I4 research
//!   resolution); the row says so rather than silently measuring nothing.
//! * `dram` — the image copied into this chip's unified RAM first, then borrowed from
//!   there. Only images that fit [`DRAM_STAGING_BYTES`] are attempted, exactly as the A1S
//!   restricts its `dram` row — `PETRI.S3M` does not fit either board's budget.

use core::alloc::Layout;

use esp_alloc::MemoryCapability;
use esp_println::println;
use firmware_common::bench::{BenchOutcome, BenchPlan, CycleCounter, Kernel, PcmLocation, digest_row, voice_size};
use firmware_common::format::{Kib, Percent};
use starplayer::model::Module;
use starplayer::rt::Arc;

use crate::images;

/// Block size both renders use, in frames: one render quantum, same as the A1S.
const BLOCK_FRAMES: usize = 128;

/// Internal-RAM staging for the `dram` rows: 32 KiB, the same figure the A1S uses and for
/// the same reason — big enough for `REFLEX.S3M` and the four synthetic fixtures,
/// deliberately not big enough for `PETRI.S3M` (88 036 bytes), which stays a `flash`-only
/// measurement on both boards.
pub const DRAM_STAGING_BYTES: usize = 32 * 1024;

/// A RISC-V `mcycle`-backed cycle counter. See the module docs for why no derivation is
/// needed here.
pub struct CycleClock {
    cycles_per_second: u64,
}

impl CycleClock {
    /// Read the configured CPU frequency once and build a clock from it.
    pub fn new() -> CycleClock {
        CycleClock { cycles_per_second: u64::from(esp_hal::clock::cpu_clock().as_mhz()) * 1_000_000 }
    }

    /// Cycles per second, for the "% of one core" column.
    pub const fn cycles_per_second(&self) -> u64 { self.cycles_per_second }
}

impl CycleCounter for CycleClock {
    fn now(&self) -> u64 { riscv::register::mcycle::read64() }
}

/// A staging buffer in internal RAM, reused by every fixture that fits it.
///
/// See `starplayer-a1s/src/bench.rs`'s copy of this type for the full reasoning: the two
/// `unsafe` blocks below are both here because [`Module::from_image`] requires a
/// `&'static [u8]`, and a bench that copied each fixture into a fresh leaked allocation
/// would exhaust the staging buffer after two fixtures.
struct Staging {
    start: *mut u8,
    capacity: usize,
}

impl Staging {
    /// Claim `capacity` bytes of internal RAM, 4-byte aligned, for the life of the
    /// program. `None` when the heap cannot satisfy it.
    fn claim(capacity: usize) -> Option<Staging> {
        // 4 is `starplayer_model::image::IMAGE_ALIGNMENT` — what makes the PCM borrow succeed.
        let layout = Layout::from_size_align(capacity, 4).ok()?;
        // SAFETY: `alloc_caps` has `GlobalAlloc::alloc`'s contract — a non-zero-sized
        // layout, and a pointer that is either null or valid for `capacity` bytes with the
        // requested alignment. `capacity` is a non-zero constant at the one call site
        // below. The allocation is never freed: a bench firmware runs its rows once and
        // then idles until the owner resets it, so there is no lifetime to manage.
        let start = unsafe { esp_alloc::HEAP.alloc_caps(MemoryCapability::Internal.into(), layout) };
        if start.is_null() { None } else { Some(Staging { start, capacity }) }
    }

    /// Copy `image` into the buffer and hand back a `'static` view of it.
    ///
    /// The caller must have dropped any [`Module`] built from a previous call before
    /// making another — see the A1S's copy of this method for the full aliasing argument.
    fn load(&mut self, image: &[u8]) -> Option<&'static [u8]> {
        if image.len() > self.capacity {
            return None;
        }
        // SAFETY: `start` came from a `capacity`-byte allocation that is still live
        // (nothing frees it), `image.len() <= capacity`, and the memory is initialised by
        // the copy on the next line before anything reads it. The `'static` lifetime is
        // sound because the allocation outlives the program; the *aliasing* obligation —
        // that no `Module` still borrows the previous contents when this overwrites them —
        // is the caller's, discharged by the explicit `drop(module)` in [`run`].
        let destination = unsafe { core::slice::from_raw_parts_mut(self.start, image.len()) };
        destination.copy_from_slice(image);
        Some(destination)
    }
}

/// Run the whole bench and print it.
pub fn run(plan: &BenchPlan) {
    let clock = CycleClock::new();

    println!();
    println!("=== StarPlayer C5 bench (RISC-V, no audio hardware) ===");
    println!("BUILD profile={} kernels=nearest,linear,cubic,sinc", if cfg!(debug_assertions) { "dev" } else { "release" });
    println!("CPU  {} MHz  ({} cycles/s)", esp_hal::clock::cpu_clock().as_mhz(), clock.cycles_per_second());
    println!("SIZE voice={} bytes  render_half={} bytes", voice_size(), core::mem::size_of::<starplayer_host_embedded::RenderHalf>());
    report_heap("boot");

    let mut dram = Staging::claim(DRAM_STAGING_BYTES);
    println!(
        "STAGING dram={} psram=unavailable (this devkit has no PSRAM)",
        if dram.is_some() { "ok" } else { "unavailable" },
    );
    report_heap("after staging");

    let mut scratch = [0i16; BLOCK_FRAMES * 2];

    for (name, image) in images::GOLDEN_FIXTURES {
        println!("IMAGE {name} bytes={} ({})", image.len(), Kib(image.len()));
        for location in PcmLocation::ALL {
            let staged = match location {
                PcmLocation::Flash => Some(image),
                PcmLocation::Psram => None,
                PcmLocation::Dram => dram.as_mut().and_then(|staging| staging.load(image)),
            };
            let Some(staged) = staged else {
                let reason = if location == PcmLocation::Psram { "this devkit has no PSRAM" } else { "no staging buffer large enough" };
                for kernel in Kernel::ALL {
                    println!("{}", BenchOutcome::Skipped { fixture: name, kernel, location, reason });
                }
                continue;
            };
            let module = match Module::from_image(staged) {
                Ok(module) => Arc::new(module),
                Err(_) => {
                    for kernel in Kernel::ALL {
                        println!("{}", BenchOutcome::Skipped { fixture: name, kernel, location, reason: "the image would not borrow" });
                    }
                    continue;
                }
            };

            for kernel in Kernel::ALL {
                match digest_row(name, kernel, location, &module, firmware_common::SAMPLE_RATE_HZ, plan, &clock, &mut scratch) {
                    Ok(row) => {
                        let load = Percent {
                            numerator: row.cycles_per_frame() * u64::from(firmware_common::SAMPLE_RATE_HZ),
                            denominator: clock.cycles_per_second(),
                        };
                        println!("{row} core_load={load}");
                    }
                    Err(_) => println!(
                        "{}",
                        BenchOutcome::Skipped { fixture: name, kernel, location, reason: "the render raised engine warnings or the module would not play" }
                    ),
                }
            }
            // Explicit, and load-bearing: the next `staging.load` overwrites the bytes this
            // module borrows.
            drop(module);
        }
        report_heap(name);
    }

    println!("=== bench complete ===");
}

/// Print one heap line: free, used and the high-water mark.
fn report_heap(label: &str) {
    let stats = esp_alloc::HEAP.stats();
    println!("HEAP [{label}] {stats}");
}
