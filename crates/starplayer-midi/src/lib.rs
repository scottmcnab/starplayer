//! The MIDI byte codec, the Standard MIDI File parser, and the mapping between MIDI
//! messages and `starplayer_core::Event`.
//!
//! The musical layer is MIDI-*convertible*, not MIDI-*shaped*: this crate translates at
//! the boundary and never forces MIDI's value ranges onto the engine.
//!
//! Allowed dependency edges: `starplayer-core`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
