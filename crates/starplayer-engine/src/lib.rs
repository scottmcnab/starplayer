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
//! # What M1 adds (task B3)
//!
//! * [`sequencer`] — the [`PatternSequencer`] timing spine: order list → pattern → row →
//!   tick, order advance, pattern break, position jump and end-of-song, driving a
//!   format-supplied [`TrackerProcessor`].
//! * [`channel`] — [`Channel`] and [`ChannelTable`], the channel-to-voice binding.
//! * [`command`] — the SPSC command ring and the garbage channel that keeps `free()` off
//!   the audio thread.
//! * [`control`] — the [`ControlClock`] envelopes will advance on.
//! * [`SourceMux`] — several sources at once with a deterministic tie-break.
//! * [`demo`] — a four-byte toy tracker format, so all of the above is testable before any
//!   loader exists.
//!
//! Not here yet: effect interpretation (B4), telemetry publishing (B6), instruments (M4),
//! the DSP graph, and background voices (M6).

#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

extern crate alloc;

pub mod channel;
pub mod command;
pub mod control;
pub mod demo;
pub mod engine;
pub mod ring;
pub mod sequencer;
pub mod source;

pub use channel::{Channel, ChannelTable};
pub use command::{
    DEFAULT_COMMAND_CAPACITY, DEFAULT_GARBAGE_CAPACITY, EngineHandle, MAX_COMMANDS_PER_QUANTUM, PcmSource,
};
pub use control::{ControlClock, ControlDriver, DEFAULT_CONTROL_INTERVAL_MICROS};
pub use engine::{
    DEFAULT_SAMPLE_RATE_HZ, Engine, EngineSettings, EngineWarnings, MAX_EVENTS_PER_BLOCK, MAX_ZERO_ADVANCE,
    RENDER_QUANTUM,
};
pub use ring::OutputRing;
pub use sequencer::{
    EndOfSongPolicy, Jump, OrderEntry, PatternData, PatternSequencer, RowRef, SequencerSettings, SongPosition,
    TickContext, TickOutcome, TrackerProcessor,
};
pub use source::{EngineContext, EventSource, ScriptedAction, ScriptedSource, SilentSource, SourceMux, SourceSlot};

use starplayer_mixer::{FixedPath, FloatPath, StereoF32, StereoI16};

/// The desktop and browser default: `f32` accumulation, interleaved stereo `f32` out.
pub type FloatEngine<Interp> = Engine<FloatPath, Interp, StereoF32>;

/// The canonical bit-exact path: integer accumulation, interleaved stereo `i16` out.
/// This is what the goldens fingerprint and what the embedded target uses
/// (architecture §7.3).
pub type FixedEngine<Interp> = Engine<FixedPath, Interp, StereoI16>;
