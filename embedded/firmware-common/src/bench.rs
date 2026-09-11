//! The on-device bench: one line per (fixture × interpolator × PCM location).
//!
//! M8's exit criterion is that the device's own SHA-256 of the ten-second fixed / mono /
//! linear render of every golden fixture equals the hash committed under `goldens/`, and
//! `plans/reference/embedded-budget.md`'s CPU table is cycles per output frame for each
//! interpolator with the PCM in flash, in PSRAM and in DRAM. Both come off the same run,
//! and this module is the part of it that is not board-specific: it drives
//! [`starplayer_host_embedded::bench`], counts cycles through whatever the board's cycle
//! counter is, and formats [`BenchRow`] so that the UART transcript can be pasted into
//! the budget document unedited.
//!
//! # What each row measures
//!
//! Two renders, deliberately:
//!
//! * the **digest** is [`render_digest`](starplayer_host_embedded::bench::render_digest)
//!   — mono, [`Limiter::Clamp`](starplayer::mixer::Limiter), no transport — because that
//!   is the configuration the committed hashes were taken in and a hash of anything else
//!   would compare against nothing;
//! * the **cycles** are [`render_frames`](starplayer_host_embedded::bench::render_frames)
//!   — stereo, through the caller's scratch buffer — because that is the shape the
//!   player actually renders in, and a cycles-per-frame figure taken in mono would
//!   understate the mixer by a factor the budget's reader could not recover.
//!
//! Both build their engine inside the timed call, which is why the plan renders whole
//! seconds: at [`GOLDEN_FRAMES`](crate::GOLDEN_FRAMES) the one-off construction is well
//! under a thousandth of the total and the figure is the steady-state one.
//!
//! # Real-time safety
//!
//! None of this is real-time code and none of it pretends to be. Both routines allocate
//! an engine; [`render_digest`] allocates a block-sized scratch. A bench build makes no
//! sound: it runs from a plain task at boot, prints, and stops.

use core::fmt::{Display, Formatter};

use starplayer::dsp::{Cubic, Linear, Nearest, Sinc};
use starplayer::mixer::Voice;
use starplayer::model::Module;
use starplayer::rt::Arc;
use starplayer_host_embedded::{Error, bench};

use crate::format::Hex;

/// Which interpolator a row was rendered with.
///
/// Named rather than generic at the call site so a board can loop over [`Kernel::ALL`]:
/// the four arms of [`digest`](Kernel::digest) are the four monomorphisations, and a
/// bench build is the only build in the tree that instantiates all of them. That is itself
/// a measurement: the audio build instantiates `Linear` alone, and the two builds' `.text`
/// differs by 87 608 bytes, because every interpolator monomorphises the whole
/// voice-accumulation path. (Their `.rodata` cannot be differenced the same way — the two
/// builds contain different *drivers* too. `plans/reference/embedded-budget.md` §1 has the
/// caveat in full.)
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Kernel {
    /// Nearest-neighbour: no interpolation at all.
    Nearest,
    /// Two-point linear — the goldens' kernel, and the one the player uses.
    Linear,
    /// Four-point Catmull-Rom.
    Cubic,
    /// Windowed-sinc over the 256-phase, 8-tap `SINC_TABLE_Q15`.
    Sinc,
}

impl Kernel {
    /// Every kernel, in the order a transcript lists them: cheapest first.
    pub const ALL: [Kernel; 4] = [Kernel::Nearest, Kernel::Linear, Kernel::Cubic, Kernel::Sinc];

    /// The lower-case name a transcript line carries, and the name `goldens/`'s filenames
    /// use.
    pub const fn name(self) -> &'static str {
        match self {
            Kernel::Nearest => "nearest",
            Kernel::Linear => "linear",
            Kernel::Cubic => "cubic",
            Kernel::Sinc => "sinc",
        }
    }

    /// SHA-256 of `frames` frames of `module`, rendered mono in blocks of `block_frames`.
    fn digest(self, module: &Arc<Module>, sample_rate_hz: u32, frames: usize, block_frames: usize) -> Result<[u8; 32], Error> {
        match self {
            Kernel::Nearest => bench::render_digest::<Nearest>(module, sample_rate_hz, frames, block_frames),
            Kernel::Linear => bench::render_digest::<Linear>(module, sample_rate_hz, frames, block_frames),
            Kernel::Cubic => bench::render_digest::<Cubic>(module, sample_rate_hz, frames, block_frames),
            Kernel::Sinc => bench::render_digest::<Sinc>(module, sample_rate_hz, frames, block_frames),
        }
    }

    /// `frames` frames of `module` rendered stereo through `scratch`, and how many came
    /// out.
    fn frames(self, module: &Arc<Module>, sample_rate_hz: u32, frames: usize, scratch: &mut [i16]) -> Result<usize, Error> {
        match self {
            Kernel::Nearest => bench::render_frames::<Nearest>(module, sample_rate_hz, frames, scratch),
            Kernel::Linear => bench::render_frames::<Linear>(module, sample_rate_hz, frames, scratch),
            Kernel::Cubic => bench::render_frames::<Cubic>(module, sample_rate_hz, frames, scratch),
            Kernel::Sinc => bench::render_frames::<Sinc>(module, sample_rate_hz, frames, scratch),
        }
    }
}

impl Display for Kernel {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> core::fmt::Result { formatter.write_str(self.name()) }
}

/// Where the module's sample data was read from while the row was rendered.
///
/// The mixer reads PCM at an arbitrary, rate-dependent stride — the least cache-friendly
/// access a program can make — so *where* the samples live is a first-order term in the
/// cycle count, and it is the term a future project has no way to guess. The three arms
/// are the three real choices on an ESP32: memory-mapped flash through the instruction
/// cache, PSRAM through the data cache, and internal DRAM.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PcmLocation {
    /// Borrowed straight out of the memory-mapped module image in flash — the M8-I2
    /// path, and the one a shipped firmware uses.
    Flash,
    /// The image copied into external PSRAM first, and borrowed from there.
    Psram,
    /// The image copied into internal DRAM first, and borrowed from there. Only the small
    /// fixtures fit; a board reports [`Skipped`](BenchOutcome::Skipped) for the rest.
    Dram,
}

impl PcmLocation {
    /// Every location, cheapest-to-reach first.
    pub const ALL: [PcmLocation; 3] = [PcmLocation::Flash, PcmLocation::Psram, PcmLocation::Dram];

    /// The lower-case name a transcript line carries.
    pub const fn name(self) -> &'static str {
        match self {
            PcmLocation::Flash => "flash",
            PcmLocation::Psram => "psram",
            PcmLocation::Dram => "dram",
        }
    }
}

impl Display for PcmLocation {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> core::fmt::Result { formatter.write_str(self.name()) }
}

/// How much work one row does.
///
/// Separated from the rows so a bring-up run can shorten everything at once — a first
/// power-on wants to know the UART works, not to wait four minutes — while the published
/// figures are always taken at [`BenchPlan::GOLDEN`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BenchPlan {
    /// Frames hashed for the digest. Must be [`GOLDEN_FRAMES`](crate::GOLDEN_FRAMES) for
    /// the hash to mean anything.
    pub digest_frames: usize,
    /// Frames rendered for the cycle count.
    pub cycle_frames: usize,
    /// Block size both renders use, in frames.
    pub block_frames: usize,
}

impl BenchPlan {
    /// The plan every published figure is taken at: ten seconds each way, in
    /// `RENDER_QUANTUM`-sized blocks.
    pub const GOLDEN: BenchPlan = BenchPlan {
        digest_frames: crate::GOLDEN_FRAMES,
        cycle_frames: crate::GOLDEN_FRAMES,
        block_frames: 128,
    };

    /// A tenth of the work, for a bring-up run. The digest it produces matches **no**
    /// committed hash, and [`BenchRow`] says so by printing `frames=` beside it.
    pub const QUICK: BenchPlan = BenchPlan {
        digest_frames: crate::GOLDEN_FRAMES / 10,
        cycle_frames: crate::GOLDEN_FRAMES / 10,
        block_frames: 128,
    };
}

/// A monotonic cycle counter.
///
/// A trait rather than a function pointer so the host tests can drive the runner with a
/// counter whose behaviour they choose, and so neither board has to agree with the other
/// about what a cycle counter is called: the A1S reads the Xtensa `CCOUNT` special
/// register, the C5 reads `mcycle`, and both are one instruction.
///
/// The contract is only that [`now`](CycleCounter::now) does not go backwards **within
/// one row**. A 32-bit counter at 240 MHz wraps every 17.9 seconds, so a board that has
/// only 32 bits must widen it — and this trait's `u64` is the reminder to.
pub trait CycleCounter {
    /// The counter now.
    fn now(&self) -> u64;
}

/// What happened when a board tried to produce a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BenchOutcome {
    /// A row was rendered.
    Measured(BenchRow),
    /// The row could not be attempted, with a reason a reader can act on — "PCM will not
    /// fit in DRAM", "this build has no processor for that format".
    Skipped {
        /// Fixture name, as it would have appeared in the row.
        fixture: &'static str,
        /// Kernel, as it would have appeared in the row.
        kernel: Kernel,
        /// Location, as it would have appeared in the row.
        location: PcmLocation,
        /// Why.
        reason: &'static str,
    },
}

impl Display for BenchOutcome {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            BenchOutcome::Measured(row) => write!(formatter, "{row}"),
            BenchOutcome::Skipped { fixture, kernel, location, reason } => {
                write!(formatter, "BENCH {fixture} {kernel} {location} skipped={reason}")
            }
        }
    }
}

/// One measured row of the bench transcript.
///
/// Its [`Display`] is the line the owner pastes into the budget document, and it is
/// pinned by a test in this crate, because the document's tables are read off these
/// fields by hand. Every field is on one line, `key=value`, space-separated, with the
/// hash in the same lower-case hex `goldens/*.sha256` uses — so `grep BENCH` over a
/// captured log is the whole extraction tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchRow {
    /// The fixture's name, as `goldens/` spells it: `petri-s3m`, `synthetic-mod`, …
    pub fixture: &'static str,
    /// The interpolator.
    pub kernel: Kernel,
    /// Where the PCM was read from.
    pub location: PcmLocation,
    /// SHA-256 of the mono render.
    pub digest: [u8; 32],
    /// Frames hashed. Only a row that says `frames=441000` can be compared with
    /// `goldens/`.
    pub digest_frames: usize,
    /// Frames rendered for the cycle count.
    pub cycle_frames: usize,
    /// Cycles the stereo render took, engine construction included.
    pub cycles: u64,
}

impl BenchRow {
    /// Cycles per output frame, rounded down. Zero frames reports zero rather than
    /// dividing.
    pub const fn cycles_per_frame(&self) -> u64 {
        if self.cycle_frames == 0 { 0 } else { self.cycles / self.cycle_frames as u64 }
    }
}

impl Display for BenchRow {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "BENCH {} {} {} sha256={} frames={} cycle_frames={} cycles={} cycles_per_frame={}",
            self.fixture,
            self.kernel,
            self.location,
            Hex(&self.digest),
            self.digest_frames,
            self.cycle_frames,
            self.cycles,
            self.cycles_per_frame(),
        )
    }
}

/// Render one row: hash the mono render, then time the stereo one.
///
/// `scratch` is the caller's so that the cycle figure is taken at the block size the
/// device really uses — its I2S DMA buffer — and so that the measurement itself allocates
/// nothing beyond the engine.
// Eight arguments, and every one of them is a column in the row this produces or a
// resource the caller owns. Bundling them into a struct would mean a board built one
// throwaway value per row and the reader had to look somewhere else to see what a row is.
#[allow(clippy::too_many_arguments)]
pub fn digest_row<Counter: CycleCounter>(
    fixture: &'static str,
    kernel: Kernel,
    location: PcmLocation,
    module: &Arc<Module>,
    sample_rate_hz: u32,
    plan: &BenchPlan,
    counter: &Counter,
    scratch: &mut [i16],
) -> Result<BenchRow, Error> {
    let digest = kernel.digest(module, sample_rate_hz, plan.digest_frames, plan.block_frames)?;

    let started = counter.now();
    let cycle_frames = kernel.frames(module, sample_rate_hz, plan.cycle_frames, scratch)?;
    // `wrapping_sub` rather than a subtraction: a counter that wrapped inside the row
    // still yields the right difference as long as it wrapped once, and a panic on a
    // device is a reset.
    let cycles = counter.now().wrapping_sub(started);

    Ok(BenchRow { fixture, kernel, location, digest, digest_frames: plan.digest_frames, cycle_frames, cycles })
}

/// `size_of::<Voice>()`, for the budget document's RAM table.
///
/// A function rather than a constant because it is a *measurement* — the number the
/// device's own compiler produced for its own target — and the budget document's claim is
/// precisely that it was read off the device rather than off a host.
pub const fn voice_size() -> usize { core::mem::size_of::<Voice>() }

#[cfg(test)]
mod tests {
    use alloc::format;

    use starplayer::rt::Arc;

    use super::*;

    /// `REFLEX.S3M`, four channels and 4 480 PCM frames — the smallest real module in the
    /// tree, so a host test can render a couple of thousand frames of it in a blink.
    const REFLEX: &[u8] = include_bytes!("../../../crates/starplayer-s3m/tests/fixtures/REFLEX.S3M");

    /// A counter that advances by a fixed amount per call, so a row's arithmetic is
    /// checkable without a clock.
    struct FakeCounter {
        step: core::cell::Cell<u64>,
        now: core::cell::Cell<u64>,
    }

    impl FakeCounter {
        fn new(step: u64) -> FakeCounter { FakeCounter { step: core::cell::Cell::new(step), now: core::cell::Cell::new(1_000) } }
    }

    impl CycleCounter for FakeCounter {
        fn now(&self) -> u64 {
            let now = self.now.get();
            self.now.set(now.wrapping_add(self.step.get()));
            now
        }
    }

    fn reflex() -> Arc<starplayer::model::Module> { Arc::new(starplayer::load(REFLEX).expect("REFLEX.S3M loads")) }

    #[test]
    fn a_row_prints_the_line_the_budget_document_is_read_from() {
        let row = BenchRow {
            fixture: "petri-s3m",
            kernel: Kernel::Linear,
            location: PcmLocation::Flash,
            digest: [0xab; 32],
            digest_frames: 441_000,
            cycle_frames: 441_000,
            cycles: 882_000_000,
        };
        assert_eq!(
            format!("{row}"),
            "BENCH petri-s3m linear flash \
             sha256=abababababababababababababababababababababababababababababababab \
             frames=441000 cycle_frames=441000 cycles=882000000 cycles_per_frame=2000"
        );
    }

    #[test]
    fn a_skipped_row_says_why_and_stays_greppable() {
        let outcome = BenchOutcome::Skipped {
            fixture: "petri-s3m",
            kernel: Kernel::Sinc,
            location: PcmLocation::Dram,
            reason: "image does not fit in internal DRAM",
        };
        assert_eq!(format!("{outcome}"), "BENCH petri-s3m sinc dram skipped=image does not fit in internal DRAM");
    }

    #[test]
    fn cycles_per_frame_does_not_divide_by_zero() {
        let row = BenchRow {
            fixture: "x", kernel: Kernel::Linear, location: PcmLocation::Flash,
            digest: [0; 32], digest_frames: 0, cycle_frames: 0, cycles: 17,
        };
        assert_eq!(row.cycles_per_frame(), 0);
    }

    #[test]
    fn every_kernel_and_location_has_a_distinct_name() {
        let mut names = alloc::vec::Vec::new();
        for kernel in Kernel::ALL {
            names.push(kernel.name());
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Kernel::ALL.len());

        let mut locations = alloc::vec::Vec::new();
        for location in PcmLocation::ALL {
            locations.push(location.name());
        }
        locations.sort_unstable();
        locations.dedup();
        assert_eq!(locations.len(), PcmLocation::ALL.len());
    }

    #[test]
    fn a_row_renders_a_real_module_through_every_kernel() {
        let module = reflex();
        let counter = FakeCounter::new(7);
        let plan = BenchPlan { digest_frames: 512, cycle_frames: 512, block_frames: 128 };
        let mut scratch = [0i16; 256];

        for kernel in Kernel::ALL {
            let row = digest_row("reflex-s3m", kernel, PcmLocation::Flash, &module, crate::SAMPLE_RATE_HZ, &plan, &counter, &mut scratch)
                .expect("a bench row renders");
            assert_eq!(row.kernel, kernel);
            assert_eq!(row.digest_frames, 512);
            assert_eq!(row.cycle_frames, 512);
            // The two `now()` calls are 7 apart by construction.
            assert_eq!(row.cycles, 7);
            assert_ne!(row.digest, [0u8; 32]);
        }
    }

    #[test]
    fn the_linear_row_reproduces_the_committed_golden_hash() {
        // The whole milestone in one host-side test: the runner, driven the way a board
        // drives it, over the ten seconds `goldens/s3m/reflex__i16_mono_44100_linear.sha256`
        // was taken over. If this passes on the host and fails on the device, the device
        // is the finding; if it fails here, the runner is.
        let expected = include_str!("../../../goldens/s3m/reflex__i16_mono_44100_linear.sha256");
        let module = reflex();
        let counter = FakeCounter::new(1);
        let plan = BenchPlan { digest_frames: crate::GOLDEN_FRAMES, cycle_frames: 128, block_frames: 128 };
        let mut scratch = [0i16; 256];

        let row = digest_row("reflex-s3m", Kernel::Linear, PcmLocation::Flash, &module, crate::SAMPLE_RATE_HZ, &plan, &counter, &mut scratch)
            .expect("the golden row renders");
        assert!(expected.trim().starts_with(&format!("{}", Hex(&row.digest))), "device digest {} is not in {expected}", Hex(&row.digest));
    }
}
