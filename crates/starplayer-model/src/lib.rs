//! The shared `Module` representation — one blob plus `u32` offsets rather than nested
//! references — along with `Sample`, `Envelope`, `InstrumentDef` and the display-only
//! `PatternCell`.
//!
//! Offsets rather than references are what let sample data be *borrowed* from
//! memory-mapped flash on an embedded target.
//!
//! Allowed dependency edges: `starplayer-core`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
