//! The Scream Tracker 3 S3M loader and its ST3 effect processor.
//!
//! S3M keeps its own pattern data and its own effect processor; it is the reference
//! format for the original DOS player and the first one this engine plays end to end.
//!
//! Allowed dependency edges: `starplayer-engine`, `starplayer-model`, and
//! `starplayer-core` — the last is a legitimate transitive edge that `starplayer-model`
//! already depends on and partly re-exports, named here directly so that
//! [`Error`](starplayer_core::Error) and the fixed-point conversions can be used without
//! going through a re-export.
//!
//! # What is here (task B2 — the loader half)
//!
//! | Module | Contents |
//! |---|---|
//! | [`loader`] | [`load`], [`probe`], and the clamp-or-reject table for malformed files |
//! | [`header`] | the file header, the channel-settings array, and the default-panning derivation |
//! | [`sample`] | the 80-byte sample header and the PCM widening |
//! | [`pattern`] | [`S3mCell`], the pattern unpacker, and [`PatternView`] |
//!
//! The effect processor is task B4 and is not here yet.
//!
//! # Where each ST3 header field ends up
//!
//! The effect processor reads its inputs from the loaded
//! [`Module`](starplayer_model::Module), never from the file, so this is the whole
//! contract between the two halves of the crate:
//!
//! | File field | Where it lands |
//! |---|---|
//! | title (`0x00`) | `header().title` |
//! | `Ordnum` (`0x20`) | `orders().len()`, with 254/255 mapped to `ORDER_MARKER`/`ORDER_END` |
//! | `Insnum` (`0x22`) | `instruments().len()`; instrument *n* in a cell is `InstrumentId(n - 1)` |
//! | `Patnum` (`0x24`) | `patterns().len()` |
//! | `generalflags` (`0x26`) bit 4 | `header().flags.amiga_limits` |
//! | `generalflags` bit 6 | `header().flags.fast_volume_slides`, together with `Cwt/v == 0x1300` |
//! | `generalflags` low byte, whole | [`S3mFormatExtra::general_flags`] |
//! | `Cwt/v` (`0x28`) | [`S3mFormatExtra::tracker_version`] |
//! | `ffi` (`0x2A`) | consumed by the loader; see [`sample::FILE_FORMAT_SIGNED`] |
//! | `globalvol` (`0x30`) | `header().global_volume`, scaled from 0..64 |
//! | `initialspd` (`0x31`) | `header().initial_speed` |
//! | `initialBPM` (`0x32`) | `header().initial_tempo` |
//! | `mastervol` (`0x33`) | `header().master_volume` scaled from 0..127, bit 7 in `flags.stereo`, and the raw byte in [`S3mFormatExtra::master_volume`] |
//! | `defaultpan` (`0x35`) and the pan block | `header().default_pan`, via [`header::default_pan_nibbles`] |
//! | channel settings (`0x40`) | `header().channel_count`, and the panning derivation |
//! | sample `vol`, `C2Spd`, loop points | the module's `SampleIndex` for that sample |
//! | packed patterns | [`S3mCell`]s in `Module::blob`, read through [`PatternView`] |
//!
//! [`S3mFormatExtra`] is the decoder for
//! [`ModuleHeader::format_extra`](starplayer_model::ModuleHeader::format_extra):
//!
//! ```no_run
//! # use starplayer_s3m::S3mFormatExtra;
//! # fn example(module: &starplayer_model::Module) {
//! let extra = S3mFormatExtra::from_header(module.header());
//! let written_by_st3_00 = extra.tracker_version == 0x1300;
//! # let _ = written_by_st3_00;
//! # }
//! ```
//!
//! # Reading a pattern
//!
//! ```no_run
//! # use starplayer_s3m::{PatternView, S3mCell};
//! # use starplayer_model::PatternId;
//! # fn example(module: &starplayer_model::Module) -> Option<S3mCell> {
//! let view = PatternView::new(module, PatternId(0))?;
//! view.cell(/* row */ 0, /* channel */ 0)
//! # }
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

extern crate alloc;

pub mod header;
pub mod loader;
pub mod pattern;
pub mod processor;
pub mod sample;

pub use header::{
    MAX_CHANNELS, PAN_CENTRE, PAN_LEFT, PAN_RIGHT, S3mFormatExtra, S3mHeader, default_pan_nibbles,
    pan_nibble_to_bipolar,
};
pub use loader::{load, load_from, probe, probe_reader};
pub use pattern::{
    COMMAND_NONE, INSTRUMENT_NONE, NOTE_CUT, NOTE_NONE, PatternView, ROWS, S3mCell, VOLUME_NONE, unpack,
};
pub use processor::{S3mChannel, S3mPatternData, S3mProcessor, sequencer_for, sequencer_with_quirks};
pub use sample::S3mSampleHeader;

// Re-exported so a host can name every type it needs from this one crate.
pub use starplayer_core::Error;
pub use starplayer_model::Module;
