//! The `VoicePool`, the voice render kernels, the bus graph, and the output formats
//! (i8 / i16 / i24 / i32 / f32, mono and stereo).
//!
//! Voice accumulation splits within a fixed `RENDER_QUANTUM` of 128 frames; the DSP graph
//! and the master bus only ever see whole quanta.
//!
//! Allowed dependency edges: `starplayer-core`, `starplayer-dsp`.

#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

extern crate alloc;
