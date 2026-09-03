//! Fixed-point arithmetic (Q0.16 / Q1.15 / Q32.32), the `Frame` / `Step` / `Note`
//! newtypes, `Event` / `TimedEvent`, `VoiceParams` and its dirty bits, the `TempoModel`
//! concept, the `QuirkSet` / `FormatDialect` replay-behaviour data ([`quirks`]), the
//! `FrameClock`, the `RowClock`, the period and waveform tables, and the shared `Error`
//! type ([`error`]).
//!
//! No IO and no side effects: everything here is pure arithmetic and plain data, so it
//! compiles unchanged for the audio thread, a bare-metal target and WASM.
//!
//! # Rules this crate keeps
//!
//! * **No floating point and no transcendental functions.** Not in the RT path, not
//!   anywhere in this crate. `sin`, `exp` and `powf` disagree between libms, so the
//!   fixed-point path would stop being bit-identical across x86, ARM and WASM
//!   (architecture §7.3). Sines come from [`tables`], the way trackers have always done
//!   it.
//! * **Everything saturates.** No operation here panics, wraps silently or divides by
//!   zero, because `render()` may not panic — a panic in an AudioWorklet kills audio for
//!   the page permanently (architecture §8).
//! * **No allocation.** Every type here is `Copy` plain data.
//!
//! Allowed dependency edges: **none**. `starplayer-core` is the root of the graph and
//! must never depend on another StarPlayer crate. The two external crates it uses,
//! `fixed` and `bitflags`, are `no_std` at the feature set this crate selects; see
//! the [`fixed`] module for what the `fixed` crate costs in transitive dependencies and
//! why it is still the right call.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod clock;
pub mod error;
pub mod event;
pub mod fixed;
pub mod frame;
pub mod note;
pub mod quirks;
pub mod random;
pub mod row_clock;
pub mod sample;
pub mod tables;
pub mod tempo;

pub use clock::FrameClock;
pub use error::Error;
pub use event::{
    AtEnd, ChannelId, Command, DirtyBits, Event, FilterParams, InstrumentId, Interpolator, SampleId,
    Target, TimedEvent, TriggerFlags, TriggerSpec, VoiceId, VoiceParam, VoiceParams,
};
pub use fixed::{I1F15, Q32_32, Step, U0F16};
pub use frame::Frame;
pub use note::{Note, Period};
pub use quirks::{
    BreakParameter, FormatDialect, ItLoopDialect, ModLoopDialect, PatternFlow, PaulaClock,
    QuirkSelection, QuirkSet, S3mLoopDialect,
};
pub use random::Xorshift32;
pub use row_clock::{RowAdvance, RowClock};
pub use sample::GUARD_FRAMES;
pub use tables::{PERIOD_TABLE, ST3_C4_SPEED, ST3_FREQUENCY_NUMERATOR, ST3_PERIOD_SCALE};
pub use tempo::{ExactFixedPoint, ItModern, St3Truncating, TempoModel, TempoModelId};
