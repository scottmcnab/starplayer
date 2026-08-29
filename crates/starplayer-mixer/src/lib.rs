//! The `VoicePool`, the voice render kernels, the bus graph, and the output formats
//! (i8 / i16 / i24 / i32 / f32, mono and stereo).
//!
//! Voice accumulation splits within a fixed `RENDER_QUANTUM` of 128 frames; the DSP graph
//! and the master bus only ever see whole quanta.
//!
//! Allowed dependency edges: `starplayer-core`, `starplayer-dsp`.
//!
//! # What M0 lands
//!
//! Enough of the above to render one voice and prove the block-size determinism
//! invariant, and no more (task A3):
//!
//! * [`sample`] — guard frames, and the offsets-plus-blob sample layout the loaders will
//!   build in M1.
//! * [`voice`] — [`Voice`], [`VoiceTag`] and the fixed-capacity generational
//!   [`VoicePool`].
//! * [`path`] — the float and fixed-point accumulators, as a monomorphised parameter.
//! * [`kernel`] — the voice render loop itself.
//! * [`output`] — accumulator frames to host samples.
//!
//! The **bus graph is not here yet**: M0 accumulates every voice into one stereo
//! accumulator. Per-channel buses and their insert chains land with the DSP graph, and
//! nothing in this crate's shape has to change for them — the kernel already writes into
//! a caller-supplied window.

#![no_std]
#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]

extern crate alloc;

pub mod kernel;
pub mod output;
pub mod path;
pub mod sample;
pub mod voice;

pub use kernel::{VoiceStatus, accumulate_voice};
pub use output::{MonoF32, MonoI16, OutputFormat, StereoF32, StereoI16};
pub use path::{FixedFrame, FixedPath, FloatFrame, FloatPath, MixPath, Stereo};
pub use sample::{GUARD_FRAMES, LoopSpan, SampleData, SampleRegion, append_guarded_sample};
pub use voice::{Voice, VoicePool, VoiceTag};
