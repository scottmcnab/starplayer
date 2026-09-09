//! Load-time sample enhancers: [`SincUpsampler`], [`LoopSmoother`], [`Chain`] and the
//! [`CATALOGUE`] a host builds its user interface from.
//!
//! Allowed dependency edges: `starplayer-model`, `starplayer-dsp`.
//!
//! # What an enhancer is, and where it runs
//!
//! Tracker samples are mostly 8-bit and often 8-16 kHz. An **enhancer** is a transform on
//! one sample's decoded PCM, applied once at load time by
//! [`Module::enhanced`](starplayer_model::Module::enhanced). The real-time path is
//! untouched: the engine sees an ordinary module whose samples happen to store more frames
//! per second, and shifts the resample step and the sample offset by the
//! `rate_scale_log2` the rebuild recorded. Nothing here ever runs inside `render()`.
//!
//! Because it is a load-time transform, its output is a **regression contract**: the same
//! module enhanced the same way must hash the same on x86, ARM and WASM.
//! `tests/determinism.rs` pins the SHA-256 of one real-world S3M.
//!
//! # Why this crate is `no_std`
//!
//! The polyphase filter's coefficients are generated once, in `f64`, by an `#[ignore]`d
//! test and **committed as source** — `sin`, `sqrt` and a Bessel series differ in their
//! last bits between libm implementations, so building the table at run time would make
//! the output platform-dependent for exactly the reason architecture §7.3 bans
//! transcendental functions from the render path. With the table committed, the runtime
//! needs only `+ − × ÷` and casts, and `cargo xtask ci --job no-std-purity` compiles this
//! crate for a bare-metal target like every other core crate.
//!
//! # Determinism rules the implementations follow
//!
//! * `f64` accumulation in a fixed order — tap 0 to tap 63, no tree reduction, no FMA.
//! * Rounding half away from zero, through `as i64` casts, then saturation to `i16`.
//! * One sample at a time, written straight to `i16`: there is never a whole-module `f64`
//!   buffer.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
// The coefficient generator and the regeneration gate that keeps the committed table
// honest run in `f64` with `sin`, `sqrt` and a Bessel series. They exist only under
// `cargo test`; the shipped crate reads the table as data.
#[cfg(test)]
extern crate std;

pub mod catalogue;
pub mod chain;
pub mod loop_smooth;
pub mod polyphase;
pub mod upsample;

pub use catalogue::{CATALOGUE, DEFAULT_CROSSFADE_FRAMES, EnhancerDescriptor, NO_FLAG_BIT, enhancer_for_id, from_flags};
pub use chain::Chain;
pub use loop_smooth::LoopSmoother;
pub use polyphase::{UPSAMPLE_CUTOFF, UPSAMPLE_KAISER_BETA, UPSAMPLE_LEADING_TAPS, UPSAMPLE_PHASES, UPSAMPLE_TAPS, coefficient};
pub use upsample::{SincUpsampler, UpsampleFactor};

// Re-exported so a host can name the trait, the two PCM types and the rebuild from this
// one crate rather than reaching into the model as well.
pub use starplayer_model::{EnhancedPcm, SampleEnhancer, SamplePcm};
