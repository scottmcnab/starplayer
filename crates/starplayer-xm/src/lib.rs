//! The FastTracker II XM loader and its effect processor, including XM's instrument and
//! envelope model.
//!
//! Allowed dependency edges: `starplayer-engine`, `starplayer-model`, and
//! `starplayer-core` — the last is a legitimate transitive edge that `starplayer-model`
//! already depends on and partly re-exports, named here directly so that
//! [`Error`](starplayer_core::Error) and the fixed-point conversions can be used without
//! going through a re-export.
//!
//! # What is here (task F1 — the loader half)
//!
//! | Module | Contents |
//! |---|---|
//! | [`loader`] | [`load`], [`probe`], and the clamp-or-reject table for malformed files |
//! | [`header`] | the 80-byte file header, the tracker-name classification, and [`XmFormatExtra`] |
//! | [`instrument`] | the instrument header and its two envelopes |
//! | [`sample`] | the 40-byte sample header and the delta / stereo / ADPCM decoding |
//! | [`pattern`] | [`XmCell`], the pattern unpacker, and [`PatternView`] |
//! | [`data`] | [`XmPatternData`], the engine's read side over a loaded module |
//! | [`processor`] | [`XmProcessor`] — FastTracker 2's effects, envelopes and volume column |
//! | [`tables`] | FastTracker 2's own replay tables, transcribed from its binary |
//!
//! The processor half is task F2. Its reference is **FastTracker 2 itself**, read through
//! `8bitbubsy/ft2-clone`'s `src/ft2_replayer.c`, because the original DOS StarPlayer never
//! supported XM; see [`processor`] for what that means in practice.
//!
//! # The cell format
//!
//! An XM pattern in the module blob is a fixed-stride array of five-byte cells,
//! `[note, instrument, volume, effect, parameter]`, row-major, `rows × channels` of them,
//! holding the **file's own byte values** unchanged (design goal 7). A note of `0` is an
//! empty column, `1..=96` is C-0..B-7 and `97` is a key-off; an instrument of `0` is an
//! empty column; a volume of `0` is an empty column, `0x10..=0x50` sets a volume and
//! `0x60..=0xFF` is one of the volume column's own effects; the effect column has no empty
//! value, because `0x00` with a parameter of `0x00` *is* both "nothing" and "arpeggio 0".
//! See [`pattern`] for the encoding and [`unpack`] for what a malformed packed stream
//! does.
//!
//! # Reading a pattern
//!
//! ```no_run
//! # use starplayer_xm::{PatternView, XmCell};
//! # use starplayer_model::PatternId;
//! # fn example(module: &starplayer_model::Module) -> Option<XmCell> {
//! let view = PatternView::new(module, PatternId(0))?;
//! view.cell(/* row */ 0, /* channel */ 0)
//! # }
//! ```
//!
//! # Clamp or reject
//!
//! Every malformed field this loader tolerates, and every one it refuses, is tabulated in
//! the [`loader`] module documentation. The rule, as for S3M, is *clamp wherever a tracker
//! would have played the file, reject only where the file contradicts itself* — with one
//! extra defence, a budget on decoded pattern data, so a three-kilobyte file cannot ask
//! for twenty megabytes of cells.
//!
//! # Deliberate departures from a literal reading of the specification
//!
//! Three, each following OpenMPT's `Load_xm.cpp` and libxmp's `xm_load.c` rather than
//! `xm.txt`, because both reference loaders agree and the pinned corpus contains the files
//! that made them agree:
//!
//! * **`sample_header_size` is parsed but is not used as a stride.** FastTracker 2,
//!   OpenMPT and libxmp all step a fixed 40 bytes per sample header. Early Sk@le Tracker
//!   writes `0` (`IFULOVE.XM`) and `cybernostra weekend` writes `0x12`, and FastTracker 2
//!   reads the full 40-byte header — including the name — in both cases.
//! * **The instrument header's `size` *is* honoured**, because trackers genuinely disagree
//!   about it: 263 from FastTracker 2 and OpenMPT, 245 from ModPlug Tracker 1.0 alpha, 33
//!   for an empty FastTracker 2 instrument, 29 in `4-mat`'s `eternity.xm`.
//! * **ModPlug's stereo and ADPCM sample extensions are decoded, not refused**, because
//!   the pinned corpus contains both (`data/stereo.xm`, `data/test.xm` and
//!   `data/m/MRHPx-HBTN LUCiFER.xm`). A stereo sample is averaged into the mono frame the
//!   mixer plays.

#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

extern crate alloc;

pub mod data;
pub mod header;
pub mod instrument;
pub mod loader;
pub mod pattern;
pub mod processor;
pub mod sample;
pub mod tables;

pub use data::XmPatternData;
pub use header::{
    FT2_HEADER_SIZE, MAGIC, MAX_CHANNELS, MAX_INSTRUMENTS, MAX_PATTERNS, XmFormatExtra, XmHeader,
};
pub use instrument::{XmEnvelope, XmInstrumentHeader};
pub use loader::{load, load_from, probe, probe_reader};
pub use processor::{
    XmChannel, XmProcessor, recommended_voice_capacity, sequencer_for, sequencer_with_quirks,
};
pub use pattern::{
    CELL_BYTES, DEFAULT_ROWS, INSTRUMENT_NONE, MAX_ROWS, NOTE_KEY_OFF, NOTE_NONE, PatternView,
    VOLUME_NONE, XmCell, unpack,
};
pub use sample::XmSampleHeader;

// Re-exported so a host can name every type it needs from this one crate.
pub use starplayer_core::Error;
pub use starplayer_model::Module;
