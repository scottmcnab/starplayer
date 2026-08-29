//! Real-time plumbing shared by every host: the SPSC ring, the triple buffer, the
//! seqlock, the lossy telemetry tap ring, and the `portable-atomic` shim used on targets
//! that lack native compare-and-swap.
//!
//! Everything here is wait-free on the audio side. No allocation, no locks and no panics
//! may occur on a path reachable from `render()`.
//!
//! Allowed dependency edges: `starplayer-core`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
