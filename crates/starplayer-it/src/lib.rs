//! The Impulse Tracker IT loader, its effect processor and its new-note-action policy.
//!
//! The NNA policy lives here so that MOD and S3M are never contaminated by it.
//!
//! Allowed dependency edges: `starplayer-engine`, `starplayer-model`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
