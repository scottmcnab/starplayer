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
