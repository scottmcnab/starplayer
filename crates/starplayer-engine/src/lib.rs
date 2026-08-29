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
//!
//! # What M0 lands
//!
//! The skeleton, and the test that pins it (task A3):
//!
//! ```text
//! for each whole RENDER_QUANTUM:
//!     voice accumulation, split at event boundaries within the quantum
//!     per-channel DSP inserts   (whole quantum, no-op for now)
//!     master bus                (whole quantum, no-op for now)
//!     output conversion → ring
//! host block is served from the ring
//! ```
//!
//! `tests/block_size_determinism.rs` renders the same scenario at host block sizes 1, 3,
//! 64, 128, 4096 and 8191 and asserts byte-identical output on both mixing paths. That
//! test is the deliverable, not a check on it: it is what makes the split-for-mixing /
//! quantise-for-DSP rule enforceable before any format code exists to break it.
//!
//! Not here yet: the command queue, channel binding, instruments, telemetry, and the
//! sequencer. Each arrives with the milestone that gives it a second implementation to
//! justify its shape.

#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

extern crate alloc;

pub mod engine;
pub mod ring;
pub mod source;

pub use engine::{Engine, EngineWarnings, MAX_EVENTS_PER_BLOCK, MAX_ZERO_ADVANCE, RENDER_QUANTUM};
pub use ring::OutputRing;
pub use source::{EngineContext, EventSource, ScriptedAction, ScriptedSource, SilentSource};

use starplayer_mixer::{FixedPath, FloatPath, StereoF32, StereoI16};

/// The desktop and browser default: `f32` accumulation, interleaved stereo `f32` out.
pub type FloatEngine<Interp> = Engine<FloatPath, Interp, StereoF32>;

/// The canonical bit-exact path: integer accumulation, interleaved stereo `i16` out.
/// This is what the goldens fingerprint and what the embedded target uses
/// (architecture §7.3).
pub type FixedEngine<Interp> = Engine<FixedPath, Interp, StereoI16>;
