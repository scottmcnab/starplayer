//! The `VoicePool`, the voice render kernels, the bus graph, and the output formats
//! (i8 / i16 / i24 / i32 / f32, mono and stereo).
//!
//! Stereo mixing with a constant-power pan law, forward and ping-pong loops, nearest and
//! linear interpolation, click-free volume and pan ramping, a master section with a
//! table-driven soft limiter, and output conversion at every depth.
//!
//! Voice accumulation splits within a fixed `RENDER_QUANTUM` of 128 frames; the DSP graph
//! and the master bus only ever see whole quanta.
//!
//! Allowed dependency edges: `starplayer-core`, `starplayer-dsp`.
//!
//! # The modules
//!
//! * [`sample`] — guard frames, loop modes, and the offsets-plus-blob sample layout the
//!   loaders build.
//! * [`voice`] — [`Voice`], [`VoiceTag`], the gain ramps, and the fixed-capacity
//!   generational [`VoicePool`].
//! * [`gain`] — the constant-power pan law and the gain-unit space voices ramp in.
//! * [`path`] — the float and fixed-point accumulators, as a monomorphised parameter.
//! * [`kernel`] — the voice render loop itself: bounded runs, loop wrapping, ramping.
//! * [`master`] — master volume and the table-driven soft limiter.
//! * [`output`] — accumulator frames to host samples, at every depth, with dither.
//!
//! # What is still to come
//!
//! **Per-*channel* buses are not here yet.** Every voice accumulates into one stereo bus,
//! which is then handed to the master section. Per-channel insert chains land with the DSP
//! graph in M7, and nothing in this crate's shape has to change for them — the kernel
//! already writes into a caller-supplied window, so a bus is a window like any other.

#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

extern crate alloc;

pub mod gain;
pub mod kernel;
pub mod master;
pub mod output;
pub mod path;
pub mod sample;
pub mod voice;

pub use gain::{GAIN_UNITY, RAMP_FRAMES, pan_gains_q15, voice_gain_units};
pub use kernel::{VoiceStatus, accumulate_voice, folded_frame};
pub use master::{Limiter, MasterSettings};
pub use output::{Dither, FixedOut, FloatOut, HostSample, I24, MonoF32, MonoI16, OutputFormat, StereoF32, StereoI16};
pub use path::{FixedFrame, FixedPath, FloatFrame, FloatPath, MixPath, Stereo};
pub use sample::{GUARD_FRAMES, LoopMode, LoopSpan, SampleData, SampleRegion, append_guarded_sample};
pub use voice::{Voice, VoicePool, VoiceTag};
