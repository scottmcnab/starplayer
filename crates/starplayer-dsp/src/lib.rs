//! Sample interpolators, parameter ramping, the IT resonant filter, biquads, the
//! reverb / chorus / delay / compressor effects, and the SIMD backends
//! (scalar / sse2 / neon / simd128).
//!
//! The fixed-point path uses tables only — no transcendental functions — so its output
//! is bit-identical on x86, ARM and WASM.
//!
//! Allowed dependency edges: `starplayer-core`.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod interpolate;

pub use interpolate::{Interpolate, Linear, Nearest};
