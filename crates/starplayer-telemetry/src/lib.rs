//! The snapshot types every UI renders from: channel state, VU levels, the current
//! command and its data, the active flag, and the scope taps.
//!
//! This is a first-class API rather than a debug hook — the web UI, the TUI and any
//! future tracker editor all read these types.
//!
//! Allowed dependency edges: `starplayer-core`, `starplayer-rt`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
