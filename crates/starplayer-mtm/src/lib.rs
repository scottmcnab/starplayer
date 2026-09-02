//! The MultiTracker MTM loader and its effect processor.
//!
//! MTM keeps its own three-byte pattern cells and its own processor and is never lowered
//! into S3M, however tempting the family resemblance is. Its command vocabulary enters
//! MOD's shared semantic effect core only after native decoding.
//!
//! Allowed dependency edges: `starplayer-core`, `starplayer-engine`, `starplayer-mixer`,
//! `starplayer-mod`, `starplayer-model`, `starplayer-rt`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod loader;
pub mod pattern;
pub mod processor;

pub use loader::{TempoMode, load, load_from, probe, probe_reader, tempo_mode, track_count};
pub use pattern::{CELL_BYTES, MtmCell, PatternView, ROWS};
pub use processor::{MtmPatternData, MtmProcessor, sequencer_for};

pub use starplayer_core::Error;
pub use starplayer_model::Module;
