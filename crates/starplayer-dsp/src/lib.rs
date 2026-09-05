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

pub mod biquad;
pub mod delay_line;
pub mod filter;
pub mod interpolate;
pub mod lfo;
pub mod ramp;
pub mod sample;
pub mod tables;

// Grouped one `pub use` item per line (not `{A, B, C}`): H1, H2 and H5 all add modules to
// this file concurrently (M7 master plan, "Shared files"), and a one-item-per-line list
// makes every addition a pure insertion, so the merge across all three is a union rather
// than a conflict.
pub use biquad::BiquadCoefficients;
pub use biquad::BiquadCoefficientsF32;
pub use delay_line::DelayLine;
pub use delay_line::StereoDelayLine;
pub use filter::FILTER_FRACTION_BITS;
pub use filter::FILTER_PREAMP_BITS;
pub use filter::FilterCoefficients;
pub use filter::IT_RESONANCE_TABLE_Q24;
pub use filter::resonant_low_pass_f32;
pub use filter::resonant_low_pass_fixed;
pub use filter::resonate_f32;
pub use filter::resonate_fixed;
pub use interpolate::Interpolate;
pub use interpolate::Linear;
pub use interpolate::Nearest;
pub use interpolate::round_shift_nearest;
pub use lfo::Lfo;
pub use ramp::GainRamp;
pub use sample::DspSample;
pub use tables::CENTI_DB_MAX;
pub use tables::CENTI_DB_MIN;
pub use tables::cos_f32;
pub use tables::cos_q15;
pub use tables::db_to_gain_f32;
pub use tables::db_to_gain_q15;
pub use tables::exp_neg_f32;
pub use tables::exp_neg_q24;
pub use tables::gain_to_centi_db;
pub use tables::gain_to_centi_db_f32;
pub use tables::log2_f32;
pub use tables::log2_q16;
pub use tables::pow2_f32;
pub use tables::pow2_q24;
pub use tables::shelf_amplitude_f32;
pub use tables::shelf_amplitude_q24;
pub use tables::sin_f32;
pub use tables::sin_q15;
pub use tables::time_constant_f32;
pub use tables::time_constant_q24;
