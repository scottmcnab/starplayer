//! The public face of StarPlayer: re-exports of the engine surface plus format
//! autodetection over whichever format crates are enabled.
//!
//! This is *the* crate an embedder depends on. Everything below it is an implementation
//! detail whose layout may change; this crate's surface may not.
//!
//! The crate is `no_std` by default and stays that way: the `std` feature is opt-in and
//! is never reachable from any default feature set (AGENTS.md design goal 4). Standard
//! library access is pulled in explicitly below rather than by dropping `#![no_std]`, so
//! the bare-metal build cannot regress silently.
//!
//! Allowed dependency edges: `starplayer-core`, `starplayer-rt`, `starplayer-dsp`,
//! `starplayer-mixer`, `starplayer-model`, `starplayer-engine`, and — behind their own
//! features — `starplayer-{mod,s3m,mtm,xm,it}`, `starplayer-midi`,
//! `starplayer-telemetry`.
//!
//! # Quirk profiles and tracker dialects
//!
//! Two different questions, answered by two different types, and it is worth keeping them
//! apart.
//!
//! A **quirk profile** is what *you* choose. [`QuirkSet`](core::quirks::QuirkSet) is a
//! small `Copy` struct of named fields — a tempo model, a Paula clock, a handful of
//! booleans — and it has two named profiles. [`QuirkSet::canonical`](core::quirks::QuirkSet::canonical)
//! is the default: ProTracker 2.3D and Scream Tracker 3.21 as those programs actually
//! behaved. [`QuirkSet::starplayer_classic`](core::quirks::QuirkSet::starplayer_classic)
//! is the 1990s DOS StarPlayer's own behaviour where it differs and a modern default
//! should not have it — today that is its double-truncated tick length, which drifts
//! about 1.3 seconds over a four-minute song. Use `canonical()` to play modules
//! accurately; use `starplayer_classic()` to hear what the original sounded like. The
//! `quirks-starplayer` cargo feature makes the classic profile the per-dialect default for
//! a whole build.
//!
//! A **dialect** is what the *file* says. [`FormatDialect`](core::quirks::FormatDialect)
//! is what a loader concluded from the header alone — `CD61` is an Atari Octalyser MOD,
//! an S3M whose `Cwt/v` is below `0x1303` came from Scream Tracker 3.01 — and it is stored
//! in [`ModuleHeader::dialect`](model::ModuleHeader::dialect). Those trackers wrote the
//! same file format but replayed it differently, mostly in how `E6x` / `SBx` pattern loops
//! interact with pattern breaks and position jumps, so a module has to be played by its
//! own tracker's rules to sound right. You do not choose a dialect; the file does.
//!
//! The two meet in [`QuirkSelection`](core::quirks::QuirkSelection), which is the
//! precedence written into the type:
//!
//! ```no_run
//! use starplayer::core::quirks::{QuirkSelection, QuirkSet};
//! # let module: starplayer::rt::Arc<starplayer::model::Module> = unimplemented!();
//! // The usual case: let the file's dialect decide.
//! let sequencer = starplayer::mod_file::sequencer_with_quirks(module.clone(), 44_100, QuirkSelection::FromDialect);
//! // Or override it, for a compatibility menu or a regression test.
//! let classic = starplayer::mod_file::sequencer_with_quirks(module, 44_100, QuirkSelection::Override(QuirkSet::starplayer_classic()));
//! ```
//!
//! The chosen set also carries the [`TempoModelId`](core::TempoModelId), so one argument
//! selects both the effect behaviour and the tick length. It is resolved **once**, when
//! the sequencer is built, and is fixed for the lifetime of that loaded module: there is
//! no setter, because a channel whose effect memories were written under one rule and read
//! under another is not a tracker any tracker ever was. Load the module again to change
//! it.
//!
//! Every field names the `plans/product/03-accuracy-policy.md` entry it implements, and a
//! field with no policy entry is not allowed. That document is the reference for what
//! "accurate" means here and for every place StarPlayer knowingly differs from ProTracker,
//! Scream Tracker 3 or the oracles it is measured against.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

/// Core event, timing and fixed-point types used by hosts.
pub use starplayer_core as core;
/// Interpolators used to select an engine instantiation.
pub use starplayer_dsp as dsp;
/// The render engine and tracker sequencing surface.
pub use starplayer_engine as engine;
/// Mixer paths and host output formats.
pub use starplayer_mixer as mixer;
/// Loaded-module and display-only pattern types.
pub use starplayer_model as model;
/// Real-time ownership primitives, including the portable [`Arc`](starplayer_rt::Arc).
pub use starplayer_rt as rt;
/// Coherent UI snapshots.
#[cfg(feature = "telemetry")]
pub use starplayer_telemetry as telemetry;

/// Scream Tracker 3 loading, native pattern access and sequencer construction.
#[cfg(feature = "s3m")]
pub use starplayer_s3m as s3m;

/// ProTracker MOD loading, native pattern access and sequencer construction.
///
/// `mod` is a Rust keyword, so the facade spells the namespace `mod_file`; the generic
/// [`load`] and [`probe`] entry points normally mean callers do not need this name.
#[cfg(feature = "mod")]
pub use starplayer_mod as mod_file;

/// MultiTracker loading, native pattern access and sequencer construction.
#[cfg(feature = "mtm")]
pub use starplayer_mtm as mtm;

/// Identify a module using the probes for the format capabilities compiled into this
/// facade. No extension or S3M lowering participates in this decision.
pub fn probe(bytes: &[u8]) -> Option<starplayer_model::ModuleFormat> {
    let _ = bytes;
    #[cfg(feature = "s3m")]
    if starplayer_s3m::probe(bytes) { return Some(starplayer_model::ModuleFormat::S3m); }
    #[cfg(feature = "mtm")]
    if starplayer_mtm::probe(bytes) { return Some(starplayer_model::ModuleFormat::Mtm); }
    #[cfg(feature = "mod")]
    if starplayer_mod::probe(bytes) { return Some(starplayer_model::ModuleFormat::Mod); }
    None
}

/// Autodetect and load a module through its native format crate.
pub fn load(bytes: &[u8]) -> Result<starplayer_model::Module, starplayer_core::Error> {
    match probe(bytes) {
        #[cfg(feature = "s3m")]
        Some(starplayer_model::ModuleFormat::S3m) => starplayer_s3m::load(bytes),
        #[cfg(feature = "mod")]
        Some(starplayer_model::ModuleFormat::Mod) => starplayer_mod::load(bytes),
        #[cfg(feature = "mtm")]
        Some(starplayer_model::ModuleFormat::Mtm) => starplayer_mtm::load(bytes),
        _ => Err(starplayer_core::Error::BadMagic),
    }
}

#[cfg(all(test, feature = "mod", feature = "mtm"))]
mod tests {
    use alloc::vec;
    use super::*;

    fn mtm_with_a_mod_tag_collision() -> alloc::vec::Vec<u8> {
        const TRACK_COUNT: usize = 5;
        const TRACK_OFFSET: usize = 66 + 128;
        const PATTERN_OFFSET: usize = TRACK_OFFSET + TRACK_COUNT * 192;
        let mut bytes = vec![0; PATTERN_OFFSET + 64];
        bytes[..4].copy_from_slice(b"MTM\x10");
        bytes[24..26].copy_from_slice(&(TRACK_COUNT as u16).to_le_bytes());
        bytes[32] = 64;
        bytes[33] = 1;
        bytes[34] = 8;
        // Offset 1080 lies in the fifth stored MTM track and is unconstrained pattern
        // data. It is also the weak signature offset used by the MOD probe.
        bytes[1080..1084].copy_from_slice(b"M.K.");
        bytes
    }

    #[test]
    fn strong_mtm_magic_wins_over_a_legal_offset_1080_mod_tag_collision() {
        let bytes = mtm_with_a_mod_tag_collision();
        assert!(starplayer_mod::probe(&bytes));
        assert!(starplayer_mtm::probe(&bytes));
        assert_eq!(probe(&bytes), Some(starplayer_model::ModuleFormat::Mtm));
        assert_eq!(load(&bytes).map(|module| module.header().format), Ok(starplayer_model::ModuleFormat::Mtm));
    }
}
