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
