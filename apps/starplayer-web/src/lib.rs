//! The browser player: a responsive web UI over the WASM build of the engine.
//!
//! Apps depend only on the facade. The AudioWorklet plumbing lives in
//! `starplayer-host-wasm` and arrives with task A4.

#![forbid(unsafe_code)]
