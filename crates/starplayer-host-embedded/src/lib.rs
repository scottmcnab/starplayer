//! The `no_std` host: [`EmbeddedPlayer`] drives the engine's fixed path straight into
//! `&mut [i16]`, with the control cadence that makes design goal 3 hold through a host.
//!
//! ```no_run
//! use starplayer::dsp::Linear;
//! use starplayer::rt::Arc;
//! use starplayer_host_embedded::EmbeddedPlayer;
//!
//! # fn main() -> Result<(), starplayer_host_embedded::Error> {
//! # let bytes: &[u8] = &[];
//! let module = Arc::new(starplayer::load(bytes)?);
//! let (mut render, mut control) = EmbeddedPlayer::<Linear>::open(module, 44_100)?;
//! control.play()?;
//! // …`render.render(&mut dma_buffer)` from the I2S refill, and poll `control.telemetry()`
//! // and `control.collect_garbage()` from a task.
//! # Ok(())
//! # }
//! ```
//!
//! The kernel is named even though [`Linear`](starplayer::dsp::Linear) is the default type
//! parameter: a default is used when a *type* is written out, never to infer one at a call
//! site, so `EmbeddedPlayer::open` on its own is ambiguous.
//!
//! # What it is
//!
//! A second host over the same engine, for a target that wants neither `std` nor a float
//! output stage. Every host before it — cpal, the AudioWorklet, `ManualBackend` — goes
//! through `starplayer_host::Player`, whose backend contract is
//! `Box<dyn FnMut(&mut [f32]) + Send>` and which converts the fixed path's native `i16` to
//! `f32` on the way out. An ESP32's I2S DMA refill wants
//! `Engine::<FixedPath, Linear, FixedOut<i16, 2>, Arc<Module>>::render(&mut [i16])` called
//! directly, and that is what [`RenderHalf::render`] is.
//!
//! What a host adds on top of `Engine::render`, and what this crate reproduces, is the
//! **control cadence**: commands are drained and the end of the song is armed only at
//! multiples of [`RENDER_QUANTUM`](starplayer::engine::RENDER_QUANTUM) frames *emitted*,
//! never at device-block boundaries, and the transport is settled after each whole quantum.
//! A stop, a seek or an end-of-song fade therefore lands on exactly one frame whatever the
//! DMA buffer size is — which is what keeps the engine's block-size invariant true of the
//! *host* and not merely of the engine. Retired modules and sequencers are moved out of the
//! engine's garbage channel onto a ring and dropped by [`ControlHalf::collect_garbage`],
//! never in the refill.
//!
//! # What it is not
//!
//! It is **not a port of `Player`**, and it does not refactor `starplayer-host`. There is
//! no `AudioBackend` here, no device negotiation, no output-depth post-stage, no insert
//! control, no live MIDI input and no scope taps: a firmware owns its own DMA callback and
//! its own tasks and calls into this crate from them. Nothing here names a board, a HAL or
//! an async runtime.
//!
//! # The duplication, and what it would take to remove it
//!
//! The transport module is `starplayer_host::transport` unchanged. The source module is
//! `starplayer_host::source` with `std::sync::Arc` replaced by [`starplayer_rt::Arc`] and
//! its two `AtomicU64`s replaced by seqlocks over 32-bit atomics — neither of the originals
//! exists on `riscv32imc-unknown-none-elf`, which has no compare-and-swap and, in fact, no
//! atomics at all, and the 64-bit atomic `portable-atomic` would synthesise is a critical
//! section. The cadence in [`RenderHalf::render`] is `starplayer_host::RenderState::render`
//! transposed from `f32` samples to `i16` ones, and counted in samples rather than frames
//! so that a device block which is not a whole number of frames cannot slide it.
//!
//! Sharing rather than copying would take a `std` **feature split** of `starplayer-host`:
//! that crate is `std` by construction (`Box<dyn FnMut + Send>`, `Mutex`, `String` errors,
//! device enumeration), and a crate in `xtask`'s `NO_STD_CRATES` may not depend on one.
//! M8's master plan puts that out of scope — "a `no_std` port of `starplayer-host` itself —
//! I1 records where its cadence logic could later be shared, and stops there" — so the
//! duplication is deliberate and is recorded here. Three things would move into a shared
//! `no_std` core if it is ever done: the transport, the seek mailbox, and the twenty lines
//! of `render` that walk a block quantum by quantum — and the mailbox would have to take
//! this crate's seqlock with it, because the std one's atomics do not exist on the targets
//! that need it.
//!
//! # Heap
//!
//! [`settings_for`] documents what one engine costs, in bytes, as a function of the
//! module's channel count and voice pool. Everything the engine allocates it allocates
//! there; nothing reachable from [`RenderHalf::render`] allocates at all.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod bench;
mod player;
mod seqlock;
mod source;
mod transport;

pub use player::{
    CONTROL_CADENCE_FRAMES, ControlHalf, EmbeddedPlayer, HOST_COMMAND_CAPACITY, OUTPUT_CHANNELS, RETIRED_CAPACITY,
    RenderHalf, TELEMETRY_DEPTH, settings_for,
};
pub use source::{
    AtEndSlot, BuiltSource, SeekKind, SeekMailbox, SeekRequest, SeekableModuleSource, SourceHandles, build_source,
    scan_module,
};
pub use transport::{DEFAULT_FADE_FRAMES, TRANSPORT_GAIN_UNITY, TRANSPORT_RAMP_FRAMES, Transport};

use starplayer::engine::EngineWarnings;

/// What can go wrong on this host's control side.
///
/// Short on purpose. There is no device to fail to open, no file system and no allocator
/// error to report: what is left is a module this build cannot play, a ring the caller has
/// overrun, and the two "you asked before there was anything to ask about" cases.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The module could not be loaded, scanned, or played by any processor in this build.
    Module(starplayer::core::Error),
    /// The command ring is full: [`HOST_COMMAND_CAPACITY`] commands are outstanding and the
    /// render half has not drained them. Nothing was queued; try again after a quantum.
    CommandQueueFull,
    /// A seek was asked for with no module loaded.
    NoModule,
    /// A freshly built engine did not hand over one of its control handles.
    ///
    /// Unreachable in practice — `Engine::with_settings` holds all of them until they are
    /// taken, and this crate takes each exactly once. It exists so that the construction
    /// path contains no `expect`, because a panic on a device is a reset.
    EngineHandleUnavailable,
    /// A bench render raised engine warnings, so the bytes it hashed are not the bytes the
    /// golden contract is about. See [`bench::render_digest`].
    EngineWarnings(EngineWarnings),
}

impl From<starplayer::core::Error> for Error {
    fn from(error: starplayer::core::Error) -> Error { Error::Module(error) }
}

impl core::fmt::Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Module(error) => write!(formatter, "could not play module: {error}"),
            Error::CommandQueueFull => write!(formatter, "the host command ring is full"),
            Error::NoModule => write!(formatter, "no module is loaded"),
            Error::EngineHandleUnavailable => write!(formatter, "a fresh engine did not hand over a control handle"),
            Error::EngineWarnings(warnings) => write!(formatter, "the bench render raised engine warnings: {warnings:?}"),
        }
    }
}

impl core::error::Error for Error {}
