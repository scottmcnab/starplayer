//! The backend-neutral half of a native host: what a device is, what a stream is, and the
//! [`Player`] that turns a file into sound through one.
//!
//! A `std` crate by definition — it is a host, not part of the engine. Its only dependency
//! is the facade, which is the rule for every host (architecture §11).
//!
//! ```no_run
//! use starplayer::engine::MixerMode;
//! use starplayer_host::{AudioSpec, ManualBackend, Player};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut backend = ManualBackend::new();
//! let mut player = Player::open(&mut backend, None, AudioSpec::stereo(48_000), MixerMode::DEFAULT)?;
//! player.load(&std::fs::read("song.s3m")?)?;
//! player.play()?;
//! // …then poll `player.telemetry()` and call `player.collect_garbage()` from time to time.
//! # Ok(())
//! # }
//! ```
//!
//! # The split, and why the browser host is not behind it yet
//!
//! The abstraction was designed under the *harder* constraint. The only host that existed
//! before this crate runs in a browser AudioWorklet: a 128-frame quantum, a module that
//! arrives as a transferred `ArrayBuffer`, no threads at all, and a `process()` that may
//! not allocate or panic on pain of killing the page's audio permanently. Every shape in
//! here — the seek mailbox, the transport ramp, the enum over engine instantiations, the
//! two-step negotiate-then-open — is that host's shape, generalised. A native backend then
//! falls out nearly free, which is what M3's master plan predicted and what
//! `starplayer-host-cpal` cashed in.
//!
//! The browser host is **not** retrofitted behind [`AudioBackend`] yet; the owner deferred
//! that to a follow-up (2026-09-03) so that cpal, the CLI and the TUI were not blocked on
//! the riskier half. What it *does* share today is everything that had to move for the
//! engine's sources to become `Send`: [`SeekableModuleSource`] and its mailbox, and the
//! output-depth post-stage. What it still has its own copy of is the transport
//! ([`Transport`]) and the engine-arm enum ([`HostEngine`]), because both live inside its
//! `process()` and moving them is a rewrite of that function rather than a re-import. The
//! retrofit deletes both copies.
//!
//! # Real-time rules, and where they apply
//!
//! Everything reachable from a [`RenderCallback`] obeys design goal 5: no allocation, no
//! locks, no panics. That is [`HostEngine::render`], [`Transport`], [`SeekableModuleSource`]
//! and [`Player`]'s private audio half. Everything on [`Player`]'s own surface is the
//! opposite — it decodes, scans and allocates freely, because it runs on the caller's
//! thread. The boundary is the callback, and it is one function.

#![forbid(unsafe_code)]

mod backend;
mod depth;
mod engine;
mod manual;
mod player;
mod source;
mod transport;

pub use backend::{AudioBackend, AudioSpec, DeviceInfo, HostError, RenderCallback, Stream, StreamControl, StreamHealth};
pub use depth::{DITHER_SEED, dither_for, quantize_fixed_sample, quantize_float_sample};
pub use engine::{HostEngine, MAX_FRAMES_PER_RENDER, SUPPORTED_DEPTHS, supported_modes};
pub use manual::{MANUAL_DEVICE_NAME, MANUAL_RATES, ManualBackend, ManualDriver};
pub use player::{CONTROL_CADENCE_FRAMES, HOST_COMMAND_CAPACITY, Player, RETIRED_CAPACITY, TELEMETRY_DEPTH};
pub use source::{
    AtEndSlot, BuiltSource, SeekKind, SeekMailbox, SeekRequest, SeekableModuleSource, SourceHandles, build_source,
    scan_module,
};
pub use transport::{DEFAULT_FADE_FRAMES, TRANSPORT_GAIN_UNITY, TRANSPORT_RAMP_FRAMES, Transport};
