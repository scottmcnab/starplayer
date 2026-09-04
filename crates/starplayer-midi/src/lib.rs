//! The MIDI byte codec, the Standard MIDI File parser, and the mapping between MIDI
//! messages and `starplayer_core::Event`.
//!
//! The musical layer is MIDI-*convertible*, not MIDI-*shaped*: this crate translates at
//! the boundary and never forces MIDI's value ranges onto the engine.
//!
//! Allowed dependency edges: `starplayer-core`; and, behind the `smf` feature only,
//! `starplayer-engine`.
//!
//! # Why `smf` reaches `starplayer-engine` (task E5 deviation)
//!
//! `plans/product/01-technical-architecture.md` §11's crate-layout table originally drew
//! this crate as `→ core` alone. That was written before task E4 landed
//! [`EventFeed`](starplayer_engine::EventFeed): §3.3 calls [`sequencer::SmfSequencer`]
//! "the other `EventFeed`", and a feed can only exist where the trait it implements is in
//! scope — `EventFeed` lives in `starplayer-engine`, which does not (and must not) depend
//! back on this crate, so the edge has to run this way. The byte codec itself
//! ([`codec`]) needs nothing beyond `starplayer-core` and stays available with no
//! features at all; only `smf` — the parser and the sequencer built on it — pulls in
//! `starplayer-engine`, and only for [`EventFeed`] and the `MIDI_CHANNEL_BASE` channel
//! mapping [`starplayer_engine::midi_channel`] already owns (master-plan decision 3).
//! `starplayer-engine` is itself `no_std + alloc` with an empty default feature set, so
//! this does not touch design goal 4.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod codec;

#[cfg(feature = "smf")]
pub mod sequencer;
#[cfg(feature = "smf")]
pub mod smf;

pub use codec::{MidiDecoder, encode, status_byte_data_len};

#[cfg(feature = "smf")]
pub use sequencer::SmfSequencer;
#[cfg(feature = "smf")]
pub use smf::{Division, Smf, TempoChange, parse_smf, probe};
