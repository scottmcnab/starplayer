//! Native audio output: opens a `cpal` stream, owns the output ring that adapts the
//! host's block size to the engine's `RENDER_QUANTUM`, and pumps the control plane from
//! the main thread.
//!
//! A `std` crate by definition — it is a host, not part of the engine.
//!
//! Allowed dependency edges: `starplayer`, plus `cpal` once A3 lands.

#![forbid(unsafe_code)]
