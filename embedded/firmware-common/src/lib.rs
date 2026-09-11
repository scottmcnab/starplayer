//! Board-independent firmware logic.
//!
//! Everything here compiles for the host as well as for Xtensa and RISC-V, and everything
//! here is tested on the host. A board crate owns pins, clocks, DMA and an executor; this
//! crate owns the three things that are the same on every board:
//!
//! * [`bench`] — the on-device measurement the milestone exists to produce. It drives
//!   [`starplayer_host_embedded::bench`] over a list of module images, for each
//!   interpolator and each place the PCM can live, and formats one line per row. The
//!   *format* of that line is the budget document's input, so it is pinned by a test here
//!   rather than discovered by reading a UART.
//! * [`now_playing`] — the view model M8-I5's display and M8-I6's web page both render.
//!   A [`Snapshot`](starplayer_telemetry::Snapshot) is 2 616 bytes and carries 64
//!   channels whatever the module has; a screen wants six fields.
//! * [`format`] — `core::fmt` helpers (a SHA-256 as hex, a byte count as KiB, a ratio as
//!   a fixed-point percentage) that exist so no board has to reach for `alloc::format!`
//!   on a logging path.
//!
//! # Why a separate crate rather than a module in the board
//!
//! The bench runner is the part of the firmware whose *output* is a published
//! measurement, and the C5 board (M8-I4) must produce byte-identical lines from the same
//! code or the two boards' figures cannot be compared. Sharing it is the point. Being
//! host-testable is the dividend: `cargo test -p starplayer-firmware-common` runs the
//! whole bench pipeline over a real module, so a change that breaks the runner is caught
//! by a CI job rather than by an owner with a serial cable.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod bench;
pub mod format;
pub mod now_playing;

pub use bench::{BenchRow, CycleCounter, Kernel, PcmLocation, digest_row};
pub use now_playing::NowPlaying;

/// The sample rate every StarPlayer firmware runs at.
///
/// Not a knob. It is the rate the committed goldens were rendered at (M8 master-plan
/// decision 2), so a device that ran at any other rate could not compare its hash with
/// `goldens/` — which is the milestone's exit criterion.
pub const SAMPLE_RATE_HZ: u32 = 44_100;

/// Frames in the ten-second golden render: `SAMPLE_RATE_HZ × 10`.
pub const GOLDEN_FRAMES: usize = 441_000;
