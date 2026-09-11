//! The `bench` build: render every golden fixture and print what it cost.
//!
//! This transcript **is** `plans/reference/embedded-budget.md`'s data. It makes no sound,
//! touches neither the codec nor I2S, and stops when it is done. The owner runs
//!
//! ```text
//! cd embedded && cargo xtask flash --board a1s --features bench && cargo xtask monitor
//! ```
//!
//! captures the output, and pastes the `BENCH` lines into the budget document.
//!
//! # What is measured, and how
//!
//! One line per (fixture × interpolator × PCM location). The line's shape and the two
//! renders behind it belong to
//! [`firmware_common::bench`](starplayer_firmware_common::bench), so the C5 (M8-I4)
//! produces the same line from the same code and the two architectures' figures can be
//! compared. This module supplies the three board-specific things that crate cannot know:
//! the cycle counter, the staging buffers the non-flash locations are rendered from, and
//! the heap numbers.
//!
//! ## The cycle counter is derived, and deliberately
//!
//! The Xtensa `CCOUNT` special register (`xtensa_lx::timer::get_cycle_count`) is the
//! obvious source and is **32 bits**: at 240 MHz it wraps every 17.9 seconds. A row
//! brackets a ten-second *render*, which for the sinc kernel over a sample-heavy module is
//! not obviously faster than 0.56× real time — so a raw `CCOUNT` difference could silently
//! be off by a multiple of 2³². [`CycleClock`] therefore reads esp-hal's 64-bit
//! microsecond timer and scales it by the configured CPU frequency. At 240 MHz that is
//! exactly 240 cycles per microsecond, and the quantisation error over a two-second render
//! is under a millionth — far below anything the budget document claims.
//!
//! ## Where the PCM lives
//!
//! * `flash` — [`Module::from_image`] over the `#[repr(C, align(4))]` `include_bytes!`
//!   wrapper, PCM borrowed in place out of memory-mapped flash. This is the shipped path.
//! * `psram` — the image copied into external PSRAM first, then borrowed from there.
//! * `dram` — the image copied into internal DRAM first. Only images that fit the
//!   [`DRAM_STAGING_BYTES`] staging buffer are attempted; `PETRI.S3M` at 88 036 bytes does
//!   not, and the row says so. That refusal is itself a finding: the sample-heavy module
//!   this milestone exists to play **cannot** live in this chip's DRAM, which is why
//!   flash-resident PCM (M8-I2) was worth building.

use core::alloc::Layout;

use esp_alloc::MemoryCapability;
use esp_println::println;
use firmware_common::bench::{BenchOutcome, BenchPlan, CycleCounter, Kernel, PcmLocation, digest_row, voice_size};
use firmware_common::format::{Kib, Percent};
use starplayer::model::Module;
use starplayer::rt::Arc;

use crate::images;

/// Block size both renders use, in frames: one render quantum, the size the audio build's
/// DMA descriptors are.
const BLOCK_FRAMES: usize = 128;

/// Internal-DRAM staging for the `dram` rows: 32 KiB.
///
/// Big enough for `REFLEX.S3M` (14 984 bytes) and all four synthetic fixtures, and
/// deliberately *not* big enough for `PETRI.S3M` — 88 036 bytes of image would be a
/// quarter of this chip's entire internal DRAM, competing with the heap the engine itself
/// needs. A row that cannot fit says `skipped=` rather than quietly measuring something
/// else.
pub const DRAM_STAGING_BYTES: usize = 32 * 1024;

/// External-PSRAM staging for the `psram` rows: 128 KiB, which holds every fixture with
/// room to spare.
pub const PSRAM_STAGING_BYTES: usize = 128 * 1024;

/// esp-hal's 64-bit microsecond clock, scaled to CPU cycles.
///
/// See the module docs for why this rather than `CCOUNT`.
pub struct CycleClock {
    cycles_per_microsecond: u64,
}

impl CycleClock {
    /// Read the configured CPU frequency once and build a clock from it.
    pub fn new() -> CycleClock {
        CycleClock { cycles_per_microsecond: u64::from(esp_hal::clock::cpu_clock().as_mhz()) }
    }

    /// Cycles per second, for the "% of one core" column.
    pub const fn cycles_per_second(&self) -> u64 { self.cycles_per_microsecond * 1_000_000 }
}

impl CycleCounter for CycleClock {
    fn now(&self) -> u64 {
        esp_hal::time::Instant::now().duration_since_epoch().as_micros().saturating_mul(self.cycles_per_microsecond)
    }
}

/// A staging buffer in one memory region, reused by every fixture that fits it.
///
/// The two `unsafe` blocks in this file are both here, and both are here because
/// [`Module::from_image`] requires a `&'static [u8]` — it is the constructor that *proves*
/// the alignment wrapper is in place — while a bench that copied each fixture into a fresh
/// leaked allocation would exhaust 32 KiB of DRAM after two fixtures.
struct Staging {
    start: *mut u8,
    capacity: usize,
}

impl Staging {
    /// Claim `capacity` bytes with the given capability, 4-byte aligned, for the life of
    /// the program.
    ///
    /// Returns `None` when the region cannot satisfy it — which for `External` is the
    /// normal answer on a module with no PSRAM fitted, and is reported as a skipped row
    /// rather than as a failure.
    fn claim(capability: MemoryCapability, capacity: usize) -> Option<Staging> {
        // 4 is what a module image is written and checked at (`starplayer_model::image`'s
        // `IMAGE_ALIGN`), and is what makes the borrow of its PCM as `&[i16]` succeed.
        let layout = Layout::from_size_align(capacity, 4).ok()?;
        // SAFETY: `alloc_caps` has `GlobalAlloc::alloc`'s contract — a non-zero-sized
        // layout, and a pointer that is either null or valid for `capacity` bytes with the
        // requested alignment. `capacity` is a non-zero constant at both call sites. The
        // allocation is never freed, which is intended: a bench firmware runs its rows once
        // and then idles until the owner resets it, so there is no lifetime to manage and
        // nothing to leak into.
        let start = unsafe { esp_alloc::HEAP.alloc_caps(capability.into(), layout) };
        if start.is_null() { None } else { Some(Staging { start, capacity }) }
    }

    /// Copy `image` into the buffer and hand back a `'static` view of it.
    ///
    /// The caller must have dropped any [`Module`] built from a previous call before
    /// making another: the buffer is reused, and a module borrows its blob and its PCM
    /// straight out of it. Every call site in this file builds a module, uses it, and lets
    /// it fall out of scope before the next `load`.
    fn load(&mut self, image: &[u8]) -> Option<&'static [u8]> {
        if image.len() > self.capacity {
            return None;
        }
        // SAFETY: `start` came from an allocation of `capacity` bytes that is still live
        // (nothing frees it), `image.len() <= capacity`, and the memory is initialised by
        // the copy on the next line before anything reads it. The `'static` lifetime is
        // sound because the allocation outlives the program; the *aliasing* obligation —
        // that no `Module` still borrows the previous contents when this overwrites them —
        // is the caller's, and is stated above.
        let destination = unsafe { core::slice::from_raw_parts_mut(self.start, image.len()) };
        destination.copy_from_slice(image);
        Some(destination)
    }
}

/// Run the whole bench and print it.
///
/// `plan` is [`BenchPlan::GOLDEN`] for a published run; a bring-up that only wants to see
/// the UART work can pass [`BenchPlan::QUICK`], and the `frames=` field on every line then
/// says the hashes match no committed golden.
pub fn run(plan: &BenchPlan) {
    let clock = CycleClock::new();

    println!();
    println!("=== StarPlayer A1S bench ===");
    println!("BUILD profile={} kernels=nearest,linear,cubic,sinc", if cfg!(debug_assertions) { "dev" } else { "release" });
    println!("CPU  {} MHz  ({} cycles/s)", esp_hal::clock::cpu_clock().as_mhz(), clock.cycles_per_second());
    println!("SIZE voice={} bytes  render_half={} bytes", voice_size(), core::mem::size_of::<starplayer_host_embedded::RenderHalf>());
    report_heap("boot");

    let mut dram = Staging::claim(MemoryCapability::Internal, DRAM_STAGING_BYTES);
    let mut psram = Staging::claim(MemoryCapability::External, PSRAM_STAGING_BYTES);
    println!(
        "STAGING dram={} psram={}",
        if dram.is_some() { "ok" } else { "unavailable" },
        if psram.is_some() { "ok" } else { "unavailable (no PSRAM region registered)" },
    );
    report_heap("after staging");

    let mut scratch = [0i16; BLOCK_FRAMES * 2];

    for (name, image) in images::GOLDEN_FIXTURES {
        println!("IMAGE {name} bytes={} ({})", image.len(), Kib(image.len()));
        for location in PcmLocation::ALL {
            // One module per location, built once and rendered by all four kernels, so the
            // staging buffer is written once per location rather than once per row.
            let staged = match location {
                PcmLocation::Flash => Some(image),
                PcmLocation::Psram => psram.as_mut().and_then(|staging| staging.load(image)),
                PcmLocation::Dram => dram.as_mut().and_then(|staging| staging.load(image)),
            };
            let Some(staged) = staged else {
                for kernel in Kernel::ALL {
                    println!("{}", BenchOutcome::Skipped { fixture: name, kernel, location, reason: "no staging buffer large enough" });
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

/// Print one heap line: free, used and the high-water mark, per region.
fn report_heap(label: &str) {
    let stats = esp_alloc::HEAP.stats();
    println!("HEAP [{label}] {stats}");
}
