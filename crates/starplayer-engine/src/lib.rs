//! The render loop: the `EventSource` and `Instrument` concepts, channel-to-voice
//! binding, the `RENDER_QUANTUM` adapter, the command queue, and the telemetry publisher.
//!
//! Tracker ticks land on exact output sample frames — the loop splits each block at event
//! boundaries and never renders across one. No allocation, no locks and no panics inside
//! `render()`; retired `Arc<Module>` values go back over a garbage channel to be dropped
//! off the audio thread.
//!
//! Allowed dependency edges: `starplayer-core`, `starplayer-rt`, `starplayer-dsp`,
//! `starplayer-mixer`, `starplayer-model`.

#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

extern crate alloc;
