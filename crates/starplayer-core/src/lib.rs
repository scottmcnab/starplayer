//! Fixed-point arithmetic (Q16.16 / Q32.32), the `Frame` / `Step` / `Note` newtypes,
//! `Event` / `TimedEvent`, `VoiceParams` and its dirty bits, the `TempoModel` concept,
//! the `RowClock`, the period and waveform tables, and the shared `Error` type.
//!
//! No IO and no side effects: everything here is pure arithmetic and plain data, so it
//! compiles unchanged for the audio thread, a bare-metal target and WASM.
//!
//! Allowed dependency edges: **none**. `starplayer-core` is the root of the graph and
//! must never depend on another StarPlayer crate.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
