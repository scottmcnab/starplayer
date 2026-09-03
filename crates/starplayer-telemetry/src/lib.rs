//! The snapshot types every UI renders from: transport position, per-channel note,
//! instrument, volume, pan, VU level, the current command and its data, and the active
//! flag.
//!
//! This is a **first-class API rather than a debug hook**. It is what the web player, the
//! TUI homage and any future tracker editor render from, and the original understood that
//! too: its `ChannelData` carried `_CMDVal`, `_CMDData`, `_VUBarLevel` and `_ActiveFlag`
//! explicitly marked "for host program" / "info only"
//! (`plans/reference/original-s3mlib-analysis.md` §2).
//!
//! Allowed dependency edges: `starplayer-core`, `starplayer-rt`.
//!
//! # What is here, and what is M3's
//!
//! Architecture §9 splits telemetry in two, and this crate is the **first half**:
//!
//! * **(a) coherent scalar state** — [`Snapshot`]: about 2.5 KB of transport and
//!   per-channel scalars, published whole so a UI never pairs a row number from one tick
//!   with a note from the next;
//! * **(b) lossy audio taps** — scope waveforms, where tearing is invisible and a
//!   `Relaxed` write index is the right answer. That landed in M3-D6 as
//!   [`starplayer_rt::tap`] and `starplayer_engine::scope`, and it does **not** live here:
//!   it is not a snapshot, and nothing about it wants coherence.
//!
//! [`ChannelState::vu_level`] **stays here**, in (a). M1-B6 said it would move to (b) at
//! M3; D6 decided against it. It is one scalar per channel, the snapshot already carries
//! it for free, and every consumer reads it here — moving it would rewrite the 22-word
//! wire header and every reader in exchange for nothing (architecture §9).
//!
//! # Who depends on whom
//!
//! The engine publishes these types, so **`starplayer-engine` depends on this crate**,
//! behind its `telemetry` feature. The edge runs that way round rather than the other for
//! one reason: the snapshot is a *description of engine state*, and the engine is the only
//! thing that can fill it in coherently — a `TelemetrySink` trait in the engine that this
//! crate implemented would put a `dyn` call on the tick path and leave the snapshot types
//! split across two crates for every UI to reassemble. A stub-time comment in the engine's
//! manifest said the engine "deliberately does not depend on telemetry"; it was a guess
//! made before either crate had contents, and M1-B6 replaced it with the optional edge
//! recorded in architecture §11.
//!
//! Note what this crate therefore *cannot* see: `starplayer-model`, and so
//! [`EffectNames`](https://docs.rs/starplayer-model)'s table of English effect names. That
//! table already exists there next to the display-only `PatternCell` a UI renders a
//! pattern grid from, and it is **not duplicated here**. [`EffectDisplay`] carries the raw
//! code, the parameter and a `&'static str` name, and the format crate — the one that knew
//! what the bytes meant — resolves the name and reports the whole thing.
//!
//! # Publication
//!
//! [`TelemetryPublisher`] accumulates a tick's state and hands it to
//! [`starplayer_rt::snapshot`], which is a bounded SPSC channel of whole snapshots rather
//! than a triple buffer; that module documents why (short version: a triple buffer needs
//! `UnsafeCell`, every core crate here is `#![forbid(unsafe_code)]`, and the obvious
//! dependency is `std`-only). The writer never blocks and never allocates; the reader
//! drains to the newest and can never observe a torn value, because a snapshot moves
//! through the ring in one piece.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod publisher;
pub mod snapshot;
pub mod vu;

pub use publisher::{ChannelUpdate, TelemetryPublisher, TelemetryReader, telemetry_channel, telemetry_channel_with_depth};
pub use snapshot::{ChannelState, EffectDisplay, MAX_CHANNELS, Snapshot, SongEnd, TransportState, WarningFlags};
pub use vu::VuMeter;

/// Snapshots in flight before the writer starts dropping them.
///
/// `starplayer-rt`'s, re-exported so a host configuring a
/// [`telemetry_channel_with_depth`] need not name a second crate.
pub use starplayer_rt::DEFAULT_SNAPSHOT_DEPTH;
