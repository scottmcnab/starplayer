//! The MultiTracker MTM loader and its effect processor.
//!
//! MTM keeps its own pattern data and its own effect processor and is never lowered into
//! S3M, however tempting the family resemblance is.
//!
//! Allowed dependency edges: `starplayer-engine`, `starplayer-model`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
