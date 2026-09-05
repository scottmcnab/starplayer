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
//! * [`simd`] — the bus-summation and master-volume kernels, scalar and vector (M7-H6).
//!
//! # Per-channel buses (M7-H1)
//!
//! [`VoicePool::accumulate_masked`] sums each voice into the bus of its `tag.channel`
//! rather than into one shared accumulator, through a [`BusSegment`] view over the
//! engine's channel-major bus array. A voice whose channel has no bus goes to the spill
//! lane, which the engine sums in after every bus. The buses are **always on**: there is
//! no "no inserts, old path" branch to keep in sync, and M7 master-plan decision 1 records
//! why the fixed goldens do not move for it.

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
pub mod simd;
pub mod voice;

pub use gain::{GAIN_UNITY, RAMP_FRAMES, pan_gains_q15, voice_gain_units};
pub use kernel::{VoiceStatus, accumulate_voice, folded_frame};
pub use master::{Limiter, MasterSettings};
pub use output::{Dither, FixedOut, FloatOut, HostSample, I24, MonoF32, MonoI16, OutputFormat, StereoF32, StereoI16};
pub use path::{FixedFrame, FixedPath, FloatFrame, FloatPath, MixPath, Stereo};
pub use sample::{GUARD_FRAMES, LoopMode, LoopSpan, PRE_ROLL_FRAMES, SampleData, SampleRegion, append_guarded_sample};
pub use voice::{BusSegment, PathFilter, Voice, VoiceFilter, VoicePool, VoiceTag};
