//! The ProTracker MOD loader and its effect processor.
//!
//! MOD keeps its own pattern data and its own effect processor. It is never lowered into
//! another format — the original DOS player converted MOD and MTM to S3M before the
//! player saw them, and that is precisely why its MOD playback was inaccurate.
//!
//! Allowed dependency edges: `starplayer-engine`, `starplayer-model`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod loader;
pub mod pattern;
pub mod processor;
pub mod tables;

pub use loader::{LoadOptions, StereoSeparation, load, load_from, load_from_with_options, load_with_options, probe, probe_reader};
pub use pattern::{CELL_BYTES, ROWS, ModCell, PatternView};
pub use processor::{EffectCell, EffectNote, EffectSemantics, ModChannel, ModPatternData, ModProcessor, sequencer_for};
pub use tables::{AMIGA_CHANNEL_MAP, FINETUNE_REFERENCE_RATES, PROTRACKER_PERIODS};

pub use starplayer_core::Error;
pub use starplayer_model::Module;
