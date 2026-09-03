//! The Impulse Tracker IT loader, its effect processor and its new-note-action policy.
//!
//! The NNA policy lives here so that MOD and S3M are never contaminated by it.
//!
//! Allowed dependency edges: `starplayer-engine`, `starplayer-model`, and
//! `starplayer-core` — the last is a legitimate transitive edge that `starplayer-model`
//! already depends on and partly re-exports, named here directly so that
//! [`Error`](starplayer_core::Error) and the fixed-point conversions can be used without
//! going through a re-export. `starplayer-rt` supplies the [`Arc`](starplayer_rt::Arc) the
//! [`PatternData`](starplayer_engine::PatternData) seam holds a module through.
//!
//! # What is here (task G1 — the loader half)
//!
//! | Module | Contents |
//! |---|---|
//! | [`loader`] | [`load`], [`probe`], the two decoded-size budgets, and the clamp-or-reject table for malformed files |
//! | [`header`] | the song header, [`ItFormatExtra`], and the [`ItFormatData`] block the processor reads `Zxx` and `Sxx` out of |
//! | [`instrument`] | the `IMPI` header in both layouts, its three envelopes, and the conversion into [`InstrumentDef`](starplayer_model::InstrumentDef) |
//! | [`sample`] | the `IMPS` header and the raw-PCM decoding |
//! | [`compression`] | the IT 2.14 decompressor and its IT 2.15 double-delta variant |
//! | [`pattern`] | [`ItCell`], the pattern unpacker, [`PatternView`], and [`ItPatternData`] |
//!
//! The instrument runtime (G3) and the effect processor (G4) are separate tasks and are not
//! here yet: this crate loads an IT and hands the sequencer its rows, and nothing plays one.
//!
//! # Where each IT header field ends up
//!
//! The effect processor reads its inputs from the loaded
//! [`Module`](starplayer_model::Module), never from the file, so this is the whole contract
//! between the two halves of the crate:
//!
//! | File field | Where it lands |
//! |---|---|
//! | song name (`0x04`) | `header().title` |
//! | `OrdNum` (`0x20`) | `orders().len()`, with 254/255 mapped to `ORDER_MARKER`/`ORDER_END` |
//! | `InsNum` (`0x22`) | `instruments().len()` in instrument mode; in sample mode the instruments are one per sample instead |
//! | `SmpNum` (`0x24`) | `samples().len()`; sample *n* in a keyboard table is `SampleId(n - 1)` |
//! | `PatNum` (`0x26`) | `patterns().len()` |
//! | `Cwt/v` (`0x28`), `Cmwt` (`0x2A`) | `header().dialect`, and verbatim in [`ItFormatData`] |
//! | `Flags` (`0x2C`) bit 0 | `header().flags.stereo` |
//! | `Flags` bit 3 | `header().flags.linear_slides` |
//! | `Flags`, whole | [`ItFormatExtra::flags`] |
//! | `Special` (`0x2E`) low nibble | [`ItFormatExtra::special`] |
//! | `GV` (`0x30`) | `header().global_volume`, scaled from 0..128 |
//! | `MV` (`0x31`) | `header().master_volume`, scaled from 0..128 |
//! | `IS` (`0x32`), `IT` (`0x33`) | `header().initial_speed`, `header().initial_tempo` |
//! | `Sep` (`0x34`), `PWD` (`0x35`) | [`ItFormatData::stereo_separation`], [`ItFormatData::pitch_wheel_depth`] |
//! | `ChnPan` (`0x40`) | `header().default_pan`, and verbatim in [`ItFormatData::channel_pan_raw`] for surround and disabled |
//! | `ChnVol` (`0x80`) | `header().default_channel_volume` |
//! | the embedded MIDI configuration | [`ItFormatData::global_macro`] / [`ItFormatData::parametered_macro`] / [`ItFormatData::fixed_macro`] |
//! | instrument `NNA`/`DCT`/`DCA`, envelopes, filter, pitch-pan | the module's `InstrumentDef` for that instrument |
//! | sample `Vol`, `C5Speed`, loops, auto-vibrato, `DfP` | the module's `SampleIndex` for that sample |
//! | sample `GvL` | [`ItFormatData::sample_global_volume`] — the model has one volume field and IT has two |
//! | packed patterns | [`ItCell`]s in `Module::blob`, read through [`PatternView`] |
//!
//! What the song message (`Special` bit 0) holds is not loaded: nothing in the engine
//! displays it, and it is recoverable from the file whenever something does.
//!
//! # Reading a pattern
//!
//! ```no_run
//! # use starplayer_it::{ItCell, PatternView};
//! # use starplayer_model::PatternId;
//! # fn example(module: &starplayer_model::Module) -> Option<ItCell> {
//! let view = PatternView::new(module, PatternId(0))?;
//! view.cell(/* row */ 0, /* channel */ 0)
//! # }
//! ```

#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

extern crate alloc;

pub mod compression;
pub mod header;
pub mod instrument;
pub mod loader;
pub mod pattern;
pub mod sample;

pub use header::{
    ItFormatData, ItFormatExtra, ItHeader, MAX_CHANNELS, MIDI_CONFIGURATION_BYTES, PAN_SURROUND,
    pan_to_bipolar,
};
pub use instrument::{ItEnvelope, ItInstrument};
pub use loader::{load, load_from, probe, probe_reader};
pub use pattern::{
    COMMAND_NONE, DEFAULT_ROWS, INSTRUMENT_NONE, ItCell, ItPatternData, ItVolumeCommand, MAX_ROWS,
    NOTE_CUT, NOTE_FADE, NOTE_NONE, NOTE_OFF, PatternView, VOLUME_NONE, normalise_note, unpack,
    used_channels,
};
pub use sample::ItSampleHeader;

// Re-exported so a host can name every type it needs from this one crate.
pub use starplayer_core::Error;
pub use starplayer_model::Module;
