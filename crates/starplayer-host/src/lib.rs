//! The backend-neutral half of a native host: what a device is, what a stream is, and the
//! [`Player`] that turns a file into sound through one.
//!
//! A `std` crate by definition — it is a host, not part of the engine. Its only dependency
//! is the facade, which is the rule for every host (architecture §11).
//!
//! ```no_run
//! use starplayer::engine::MixerMode;
//! use starplayer_host::{AudioSpec, ManualBackend, Player};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut backend = ManualBackend::new();
//! let mut player = Player::open(&mut backend, None, AudioSpec::stereo(48_000), MixerMode::DEFAULT)?;
//! player.load(&std::fs::read("song.s3m")?)?;
//! player.play()?;
//! // …then poll `player.telemetry()` and call `player.collect_garbage()` from time to time.
//! # Ok(())
//! # }
//! ```
//!
//! # The two backends, and why one trait covers both
//!
//! The abstraction was designed under the *harder* constraint. The only host that existed
//! before this crate runs in a browser AudioWorklet: a 128-frame quantum, a module that
//! arrives as a transferred `ArrayBuffer`, no threads at all, and a `process()` that may
//! not allocate or panic on pain of killing the page's audio permanently. Every shape in
//! here — the seek mailbox, the transport ramp, the enum over engine instantiations, the
//! two-step negotiate-then-open — is that host's shape, generalised. A native backend then
//! falls out nearly free, which is what M3's master plan predicted and what
//! `starplayer-host-cpal` cashed in.
//!
//! Task D9 then put the browser behind [`AudioBackend`] as well, and the question it had to
//! answer first was whether the trait survives a host that is *pushed* rather than *pulling*.
//! cpal owns a thread and calls the callback from it; an AudioWorklet is called by the
//! browser, on a thread nobody here owns, exactly 128 frames at a time, and the wasm host's
//! only entry point is a `process(frames)` export that JavaScript invokes. The honest
//! reading is that this is not a second kind of backend at all. **`open` does not start a
//! thread; it takes ownership of a callback and promises to call it.** Which clock does the
//! calling is the *backend's* business and appears nowhere in the trait — the proof already
//! sat in this crate before D9, because [`ManualBackend`] is a push backend too: it stores
//! the callback and its caller pumps it. `starplayer-host-wasm`'s `WorkletBackend` is
//! [`ManualBackend`] with a browser for a clock. A second "push-shaped" trait whose `open`
//! *returned* the callback would buy one thing — the host would not have to keep the
//! backend alive alongside the [`Player`] — and cost the two that matter: a host would have
//! two lifecycles to implement instead of one, and `negotiate`, which is where the wasm
//! backend earns its keep by answering with the `AudioContext`'s own rate rather than the
//! rate that was asked for, would have to be duplicated onto it. So: one trait, three
//! backends, and no `Stream` anywhere that means anything different.
//!
//! What stays in each backend crate is what is genuinely platform: cpal keeps device
//! enumeration and its `i16` conversion, and the wasm host keeps the wire command decoding,
//! the `SharedArrayBuffer` and `postMessage` transports, the scope-window copy, the planar
//! output buffer and the heap pre-reservation. Neither keeps a transport, an engine-arm
//! enum, a seek mailbox or a depth post-stage: those were the two copies D4 predicted D9
//! would delete, and it did.
//!
//! # Live input (task E6)
//!
//! A host does not only push audio out; it takes notes in. [`Player::midi_only`] binds the
//! loaded module's instruments to sixteen MIDI channels and installs the engine's
//! `MidiSource` in place of the module's own sequencer, and [`Player::send_event`] stamps
//! an event `source_frame + lead` and pushes it at that source's queue. [`EventSender`] is
//! the sending half, made `Send` so a `midir` callback thread can own it
//! (`starplayer-midi-native`), and [`EventClock`] carries the lead policy, the clock and
//! the counters both sides share. `src/events.rs` explains which of the engine's two clocks
//! the stamp is taken from and why the lead has a floor.
//!
//! # Real-time rules, and where they apply
//!
//! Everything reachable from a [`RenderCallback`] obeys design goal 5: no allocation, no
//! locks, no panics. That is [`HostEngine::render`], [`Transport`], [`SeekableModuleSource`]
//! and [`Player`]'s private audio half. Everything on [`Player`]'s own surface is the
//! opposite — it decodes, scans and allocates freely, because it runs on the caller's
//! thread. The boundary is the callback, and it is one function.

#![forbid(unsafe_code)]

mod backend;
mod depth;
mod engine;
mod events;
mod format;
mod manual;
mod player;
mod source;
mod transport;

pub use backend::{AudioBackend, AudioSpec, DeviceInfo, HostError, RenderCallback, Stream, StreamControl, StreamHealth};
pub use depth::{DITHER_SEED, dither_for, quantize_fixed_sample, quantize_float_sample};
pub use engine::{HostEngine, MAX_FRAMES_PER_RENDER, SUPPORTED_DEPTHS, supported_modes};
pub use events::{DEFAULT_EVENT_LEAD_FRAMES, EVENT_QUEUE_CAPACITY, EventClock, EventSender};
pub use format::format_seconds;
pub use manual::{MANUAL_DEVICE_NAME, MANUAL_RATES, ManualBackend, ManualDriver};
// E5 supersedes this re-export together with the module behind it.
pub use starplayer::midi::MidiDecoder;

/// Decode one already-framed channel voice message — a status byte and its data bytes,
/// the shape Web MIDI hands the page — into `(channel, Event)`.
///
/// A fresh [`MidiDecoder`] fed the three bytes in order; the first message it yields is
/// the answer, so a two-byte message (program change, channel pressure) is complete after
/// `data1` and never sees `data2`. System and real-time status bytes yield nothing.
pub fn message_to_event(status: u8, data1: u8, data2: u8) -> Option<(u8, starplayer::core::Event)> {
    let mut decoder = MidiDecoder::new();
    [status, data1, data2].into_iter().find_map(|byte| decoder.decode(byte))
}
pub use player::{CONTROL_CADENCE_FRAMES, HOST_COMMAND_CAPACITY, Player, RETIRED_CAPACITY, TELEMETRY_DEPTH};
pub use source::{
    AtEndSlot, BuiltSource, SeekKind, SeekMailbox, SeekRequest, SeekableModuleSource, SourceHandles, build_source,
    scan_module,
};
pub use transport::{DEFAULT_FADE_FRAMES, TRANSPORT_GAIN_UNITY, TRANSPORT_RAMP_FRAMES, Transport};
