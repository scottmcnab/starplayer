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

/// Scream Tracker 3 loading, native pattern access and sequencer construction.
#[cfg(feature = "s3m")]
pub use starplayer_s3m as s3m;
