//! The shared [`Module`] representation — two owned blobs plus `u32` offsets rather than
//! nested references — along with [`SampleIndex`], [`Envelope`], [`InstrumentDef`] and the
//! display-only [`PatternCell`].
//!
//! Offsets rather than references are what let sample data be *borrowed* from
//! memory-mapped flash on an embedded target, and what make `Arc<Module>` trivially
//! `Send + Sync` and the whole module hashable for golden tests (architecture §6).
//!
//! Allowed dependency edges: `starplayer-core`.
//!
//! # What is here, and what is deliberately not
//!
//! | Here | Not here |
//! |---|---|
//! | Samples, their loops and their guard-frame layout | Any format parsing (that is each format's own crate) |
//! | Instruments, envelopes, NNA settings as **data** | Envelope or NNA *behaviour* |
//! | Patterns' **native** bytes, unparsed, plus their extents | A shared executable pattern-cell type |
//! | A display-only [`PatternCell`] and the English [`EffectNames`] | Sample decompression (IT, M6) |
//!
//! The second row of that table is design goal 7. Each format keeps its native pattern
//! data and its own effect processor; the original lowered MOD and MTM into S3M before
//! the player saw them, and that is exactly why its MOD playback was inaccurate. The
//! model bounds-checks pattern regions and never looks inside one.
//!
//! # Building a module
//!
//! [`ModuleBuilder`] is the only way to make a [`Module`], and it is where fuzz
//! resistance is concentrated: a loader that mis-parses a field gets an [`Error`], not a
//! module that makes the mixer read the wrong memory.
//!
//! ```
//! use starplayer_model::{InstrumentDef, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec};
//! use starplayer_core::U0F16;
//!
//! let mut builder = ModuleBuilder::new();
//! let sample = builder.add_sample(&[0, 1000, 2000, 1000], SampleSpec::one_shot("bass"))?;
//! builder.add_instrument(InstrumentDef::from_sample("bass", sample, U0F16::MAX))?;
//! let pattern = builder.add_pattern(&[0x00], 64, 4)?;
//! builder.set_orders(&[pattern.0, starplayer_model::ORDER_END]);
//! builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 4));
//! let module = builder.build()?;
//!
//! assert_eq!(module.sample_pcm(sample).map(<[i16]>::len), Some(4 + starplayer_core::GUARD_FRAMES));
//! # Ok::<(), starplayer_core::Error>(())
//! ```
//!
//! # Decisions this crate records
//!
//! **Samples are normalised to `i16` at load time** (task B1, research point 1). Every
//! format's 8-bit, delta-coded and (later) compressed sample data is decoded and widened
//! once, by the loader, so the mixer sees one width and the guard-frame contract has one
//! shape. The alternative — keeping the source width and making the mixer generic over it
//! — would halve the memory an 8-bit MOD costs on embedded, at the price of doubling the
//! monomorphised kernel count and of an interpolator that has to know the source width.
//! It is revisited at M8, when the embedded memory budget is a real number rather than a
//! guess; nothing in the public API here changes if it does, because a loader already
//! hands the builder decoded frames.
//!
//! **The blob and the PCM are two allocations, not one** (research point 2). One
//! allocation would let a future mmap path map a single region, but it forces `pcm` to be
//! byte-aligned rather than `i16`-aligned and makes every sample access go through a
//! cast. Two allocations keep `pcm: Box<[i16]>` exactly as the mixer wants it, and the
//! mmap case is better served later by making each blob independently *borrowable* —
//! which the offsets-not-references layout already allows — than by fusing them now.
//!
//! **`Error` lives in `starplayer-core`** rather than here. Architecture §11 lists it
//! under `core`, and it has three users with no crate in common: the format crates, this
//! builder, and the platform byte sources behind [`ModuleReader`]. It is re-exported here
//! so a loader can write `use starplayer_model::Error`.
//!
//! **`GUARD_FRAMES` lives in `starplayer-core`** for the same reason: this crate *writes*
//! the guard frames and `starplayer-mixer` *reads* them, and there is no dependency edge
//! between the two. `starplayer-mixer` re-exports it.
//!
//! # Sample sustain loops
//!
//! IT's [`SustainLoop`](sample::SustainLoop) is a second loop, played instead of the
//! sample's ordinary loop until the note is released. Whenever a sample declares one,
//! [`ModuleBuilder::add_sample`] stores the sample's **whole** body rather than truncating
//! it at a loop end — the ordinary loop and the sustain loop may each lie anywhere inside
//! it — and fills the guard with silence rather than a wrapped or reflected continuation,
//! because neither loop's end has to sit at the stored length any more. This is task E1;
//! nothing plays a sustain loop yet, and no format loader produces one before G1 (IT).

#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

extern crate alloc;

pub mod builder;
pub mod enhance;
pub mod header;
pub mod instrument;
pub mod module;
pub mod pattern;
pub mod reader;
pub mod sample;
pub mod text;

pub use builder::{ModuleBuilder, ping_pong_reflect};
pub use enhance::{EnhancedPcm, SampleEnhancer, SamplePcm};
pub use header::{ModuleFlags, ModuleFormat, ModuleHeader};
pub use instrument::{
    DuplicateAction, DuplicateCheck, Envelope, EnvelopePoint, EnvelopeSpan, InstrumentDef,
    NOTE_MAP_LENGTH, NewNoteAction,
};
pub use module::{Module, ORDER_END, ORDER_MARKER, OrderEntry};
pub use pattern::{
    EffectCell, EffectNames, NoteCell, PatternCell, PatternId, PatternIndex, it_command_code, s3m_command_code,
    xm_command_code,
};
pub use reader::ModuleReader;
pub use sample::{
    AutoVibrato, AutoVibratoWaveform, DEFAULT_REFERENCE_RATE_HZ, LoopMode, MAX_RATE_SCALE_LOG2,
    SampleIndex, SampleSpec, SustainLoop,
};
pub use text::{cp437_char, decode_cp437};

// Re-exported so a format crate can name every type it needs from this one crate.
pub use starplayer_core::quirks::FormatDialect;
pub use starplayer_core::{Error, GUARD_FRAMES, InstrumentId, SampleId};
