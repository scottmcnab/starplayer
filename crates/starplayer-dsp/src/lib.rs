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

pub mod filter;
pub mod interpolate;
pub mod ramp;

pub use filter::{
    FILTER_FRACTION_BITS, FILTER_PREAMP_BITS, FilterCoefficients, IT_RESONANCE_TABLE_Q24, resonant_low_pass_f32,
    resonant_low_pass_fixed, resonate_f32, resonate_fixed,
};
pub use interpolate::{Interpolate, Linear, Nearest, round_shift_nearest};
pub use ramp::GainRamp;
