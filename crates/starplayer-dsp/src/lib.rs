//! Sample interpolators, parameter ramping, the IT resonant filter, biquads, the
//! reverb / chorus / delay / compressor effects, and the SIMD backends
//! (scalar / sse2 / neon / simd128).
//!
//! The fixed-point path uses tables only — no transcendental functions — so its output
//! is bit-identical on x86, ARM and WASM.
//!
//! Allowed dependency edges: `starplayer-core`.
//!
//! # The insert graph (M7-H1)
//!
//! [`Insert`] is one effect in a channel or master chain, generic over [`DspSample`] so a
//! single body serves the float and the fixed mixing path. [`Stereo`] lives here rather
//! than in `starplayer-mixer` for the same reason: effects need it and this crate sits
//! below the mixer. [`SmoothedParam`] is how an audible parameter change gets from one
//! value to another without clicking.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod effects;
pub mod filter;
pub mod frame;
pub mod insert;
pub mod interpolate;
pub mod ramp;
pub mod sample;
pub mod smooth;

pub use effects::{InsertKind, build_insert};
pub use filter::{
    FILTER_FRACTION_BITS, FILTER_PREAMP_BITS, FilterCoefficients, IT_RESONANCE_TABLE_Q24, resonant_low_pass_f32,
    resonant_low_pass_fixed, resonate_f32, resonate_fixed,
};
pub use frame::Stereo;
pub use insert::{DSP_BLOCK_FRAMES, Insert, InsertDescriptor, ParamId, ParamSpec, ParamUnit};
pub use interpolate::{Interpolate, Linear, Nearest, round_shift_nearest};
pub use ramp::GainRamp;
pub use sample::{DspSample, Q15_UNITY};
pub use smooth::{SMOOTH_FRAMES, SmoothedParam};
