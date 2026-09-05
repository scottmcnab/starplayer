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
// The windowed-sinc table's generator and the regeneration test that gates it run in
// `f64` with `sin`, `sqrt` and a Bessel series. They exist only under `cargo test`; the
// shipped crate is `no_std` and reads the committed table as data.
#[cfg(test)]
extern crate std;

pub mod biquad;
pub mod delay_line;
pub mod effects;
pub mod filter;
pub mod frame;
pub mod insert;
pub mod interpolate;
pub mod lfo;
pub mod ramp;
pub mod sample;
pub mod sinc_table;
pub mod smooth;
pub mod tables;

// One `pub use` item per line (not `{A, B, C}`): M7's tasks all add modules to this file
// concurrently (M7 master plan, "Shared files"), and a one-item-per-line list makes every
// addition a pure insertion, so a merge is a union rather than a conflict.
pub use biquad::BiquadCoefficients;
pub use biquad::BiquadCoefficientsF32;
pub use delay_line::DelayLine;
pub use delay_line::StereoDelayLine;
pub use effects::InsertKind;
pub use effects::build_insert;
pub use filter::FILTER_FRACTION_BITS;
pub use filter::FILTER_PREAMP_BITS;
pub use filter::FilterCoefficients;
pub use filter::IT_RESONANCE_TABLE_Q24;
pub use filter::resonant_low_pass_f32;
pub use filter::resonant_low_pass_fixed;
pub use filter::resonate_f32;
pub use filter::resonate_fixed;
pub use frame::Stereo;
pub use insert::DSP_BLOCK_FRAMES;
pub use insert::Insert;
pub use insert::InsertDescriptor;
pub use insert::ParamId;
pub use insert::ParamSpec;
pub use insert::ParamUnit;
pub use interpolate::Cubic;
pub use interpolate::Interpolate;
pub use interpolate::Linear;
pub use interpolate::Nearest;
pub use interpolate::Sinc;
pub use interpolate::round_shift_nearest;
pub use lfo::Lfo;
pub use sinc_table::SINC_PHASES;
pub use sinc_table::SINC_TABLE_Q15;
pub use sinc_table::SINC_TAPS;
pub use ramp::GainRamp;
pub use sample::DspSample;
pub use sample::Q15_UNITY;
pub use smooth::SMOOTH_FRAMES;
pub use smooth::SmoothedParam;
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
