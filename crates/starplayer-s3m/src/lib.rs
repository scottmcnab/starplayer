//! The Scream Tracker 3 S3M loader and its ST3 effect processor.
//!
//! S3M keeps its own pattern data and its own effect processor; it is the reference
//! format for the original DOS player and the first one this engine plays end to end.
//!
//! Allowed dependency edges: `starplayer-engine`, `starplayer-model`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
