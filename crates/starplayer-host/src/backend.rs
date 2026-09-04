//! The backend contract: what a device is, what a stream is, and the two calls a backend
//! has to answer.
//!
//! Everything here is deliberately plain: `String` names, `u32` rates, interleaved `f32`.
//! A backend is a *driver*, and a driver's whole job is to turn one of these into whatever
//! its platform wants. Nothing in this module knows what a module or an engine is.

use std::boxed::Box;
use std::fmt;
use std::string::String;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::vec::Vec;

/// The shape of an audio stream: what the caller asked for, or what the device agreed to.
///
/// The same type carries both directions on purpose. A host requests a rate and a block
/// size, the device answers with the ones it will actually use, and the answer is the one
/// the engine is built from — a `Player` built at 44 100 Hz feeding a 48 kHz device plays
/// every module a semitone and a half flat, which is the failure this type exists to make
/// impossible to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioSpec {
    /// Output rate in Hz.
    pub sample_rate_hz: u32,
    /// Interleaved output channels. StarPlayer's hosts render one or two.
    pub channels: u16,
    /// Frames the caller would like per callback, or `None` for the device's own choice.
    ///
    /// It is a *preference*, never a promise: every backend delivers ragged block sizes,
    /// which is precisely why the engine's output ring exists (architecture §1.4).
    pub preferred_block_frames: Option<u32>,
}

impl AudioSpec {
    /// 48 kHz stereo at the device's own block size.
    pub const DEFAULT: AudioSpec = AudioSpec { sample_rate_hz: 48_000, channels: 2, preferred_block_frames: None };

    /// `sample_rate_hz` in stereo at the device's own block size.
    pub const fn stereo(sample_rate_hz: u32) -> AudioSpec {
        AudioSpec { sample_rate_hz, channels: 2, preferred_block_frames: None }
    }

    /// Samples in `frames` frames of this spec.
    pub const fn samples_for(&self, frames: usize) -> usize { frames.saturating_mul(self.channels as usize) }
}

impl Default for AudioSpec {
    fn default() -> AudioSpec { AudioSpec::DEFAULT }
}

impl fmt::Display for AudioSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} Hz, {} ch", self.sample_rate_hz, self.channels)?;
        match self.preferred_block_frames {
            Some(frames) => write!(formatter, ", {frames} frames"),
            None => formatter.write_str(", device block size"),
        }
    }
}

/// One output device, as a chooser sees it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeviceInfo {
    /// The name a `--device` argument matches against.
    pub name: String,
    /// Whether this is the device a caller gets by passing `None`.
    pub is_default: bool,
    /// Output rates the device advertises, ascending and deduplicated. Empty means the
    /// backend could not say, not that the device has none.
    pub supported_rates: Vec<u32>,
    /// Which backend host this device came from — "ALSA", "PulseAudio", "WASAPI".
    pub backend: String,
}

/// What can go wrong between a host and its device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostError {
    /// The backend enumerated no output device at all.
    NoDevice,
    /// A `--device` name matched nothing.
    UnknownDevice(String),
    /// The device cannot be opened in any configuration this host can render.
    UnsupportedSpec { requested: AudioSpec, reason: String },
    /// The device was found and refused, or vanished mid-negotiation.
    Backend(String),
    /// The engine has no instantiation for the requested mixer mode.
    UnsupportedMixerMode(String),
    /// A module could not be loaded, scanned, or played by this build.
    Module(starplayer::core::Error),
    /// A Standard MIDI File could not be parsed.
    StandardMidiFile(starplayer::core::Error),
    /// The control plane could not accept a command because its ring was full.
    ControlQueueFull,
    /// A `Player` call that needs a module arrived before one was loaded.
    NoModule,
    /// A live event arrived before a live-input source was installed, or after the
    /// sender was taken away by [`Player::take_event_sender`](crate::Player::take_event_sender).
    NoEventQueue,
    /// The live-input queue is full: the caller is sending faster than the audio thread
    /// consumes. Counted rather than waited on — see [`crate::EventClock`].
    EventQueueFull,
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::NoDevice => formatter.write_str("no audio output device is available"),
            HostError::UnknownDevice(name) => write!(formatter, "no output device is called `{name}`"),
            HostError::UnsupportedSpec { requested, reason } => write!(formatter, "the device cannot play {requested}: {reason}"),
            HostError::Backend(message) => write!(formatter, "the audio backend failed: {message}"),
            HostError::UnsupportedMixerMode(message) => write!(formatter, "unsupported mixer mode: {message}"),
            HostError::Module(error) => write!(formatter, "could not play the module: {error}"),
            HostError::StandardMidiFile(error) => write!(formatter, "could not parse the Standard MIDI File: {error}"),
            HostError::ControlQueueFull => formatter.write_str("the host control queue is full"),
            HostError::NoModule => formatter.write_str("no module is loaded"),
            HostError::NoEventQueue => formatter.write_str("no live-input queue is installed"),
            HostError::EventQueueFull => formatter.write_str("the live-input queue is full"),
        }
    }
}

impl std::error::Error for HostError {}

impl From<starplayer::core::Error> for HostError {
    fn from(error: starplayer::core::Error) -> HostError { HostError::Module(error) }
}

/// What the backend's error callback has told us, readable from the control thread.
///
/// The audio realm cannot return a `Result`, so a stream that fails reports it by moving
/// two atomics. Both are written from whichever thread the backend calls its error
/// callback on, so nothing here allocates, locks, or formats a message.
#[derive(Debug, Default)]
pub struct StreamHealth {
    errors: AtomicU64,
    fatal: AtomicBool,
}

impl StreamHealth {
    /// A stream that has reported nothing.
    pub fn new() -> Arc<StreamHealth> { Arc::new(StreamHealth::default()) }

    /// Record one error. `fatal` means the stream will not recover and has to be rebuilt.
    pub fn record(&self, fatal: bool) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        if fatal {
            self.fatal.store(true, Ordering::Relaxed);
        }
    }

    /// Errors the backend has reported since the stream opened.
    pub fn error_count(&self) -> u64 { self.errors.load(Ordering::Relaxed) }

    /// Whether the stream has died and has to be reopened.
    pub fn is_failed(&self) -> bool { self.fatal.load(Ordering::Relaxed) }

    /// Whether anything at all has gone wrong.
    pub fn is_healthy(&self) -> bool { self.error_count() == 0 }
}

/// The backend half of an open stream: everything [`Stream`] forwards.
///
/// Object-safe and `Send`, because the stream handle lives on the control thread while the
/// callback it started runs somewhere else entirely.
pub trait StreamControl: Send {
    /// Start, or resume, calling the data callback.
    fn play(&self) -> Result<(), HostError>;

    /// Stop calling the data callback without tearing the stream down.
    fn pause(&self) -> Result<(), HostError>;
}

/// An open output stream.
///
/// Dropping it closes the stream and joins the callback, so the callback — and the engine
/// it owns — is gone by the time `drop` returns. [`Stream::close`] is the same thing said
/// out loud.
pub struct Stream {
    spec: AudioSpec,
    health: Arc<StreamHealth>,
    control: Box<dyn StreamControl>,
}

impl Stream {
    /// Wrap a backend's control handle. Backends call this; hosts do not.
    pub fn new(spec: AudioSpec, health: Arc<StreamHealth>, control: Box<dyn StreamControl>) -> Stream {
        Stream { spec, health, control }
    }

    /// What the device actually agreed to — **not** what was asked for.
    pub fn spec(&self) -> AudioSpec { self.spec }

    /// What the backend's error callback has reported.
    pub fn health(&self) -> &Arc<StreamHealth> { &self.health }

    /// Start, or resume, the data callback.
    pub fn play(&self) -> Result<(), HostError> { self.control.play() }

    /// Stop the data callback without closing the stream.
    pub fn pause(&self) -> Result<(), HostError> { self.control.pause() }

    /// Close the stream and wait for its callback to finish.
    pub fn close(self) { drop(self) }
}

impl fmt::Debug for Stream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Stream")
            .field("spec", &self.spec)
            .field("errors", &self.health.error_count())
            .finish()
    }
}

/// Interleaved `f32` frames, of whatever length the device asked for.
///
/// The callback is the audio realm: design goal 5 applies to every line it reaches. It is
/// `Send` and `'static` because it is about to be handed to a thread the host does not own.
pub type RenderCallback = Box<dyn FnMut(&mut [f32]) + Send>;

/// One platform audio API.
///
/// Two calls, and the split between them is the whole design. [`AudioBackend::negotiate`]
/// answers "what would you actually give me?" *before* anything expensive exists, because
/// the answer decides the rate the engine is built at and the rate the song is scanned at
/// (architecture §4.1). [`AudioBackend::open`] then takes a callback that is already
/// correct for that answer. A single call that negotiated and opened at once would force
/// the engine to be built for a guess and corrected afterwards, which is a rebuild of the
/// whole voice pool on the first callback.
pub trait AudioBackend {
    /// Every output device this backend can see, default first where the platform says so.
    ///
    /// Never fails: a backend that cannot enumerate reports an empty list, because "no
    /// devices" and "enumeration broke" look identical to the person reading `--list-devices`
    /// and neither is worth a `Result` at this level.
    fn devices(&self) -> Vec<DeviceInfo>;

    /// What `device` would actually give for `requested`, without opening anything.
    fn negotiate(&self, device: Option<&str>, requested: AudioSpec) -> Result<AudioSpec, HostError>;

    /// Open `device` at `spec` — which must be a spec [`AudioBackend::negotiate`] returned —
    /// and start feeding `callback`.
    ///
    /// The stream comes back **paused**: nothing is rendered until [`Stream::play`].
    fn open(&mut self, device: Option<&str>, spec: AudioSpec, callback: RenderCallback) -> Result<Stream, HostError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_says_how_many_samples_a_block_holds() {
        assert_eq!(AudioSpec::stereo(44_100).samples_for(128), 256);
        assert_eq!(AudioSpec { channels: 1, ..AudioSpec::DEFAULT }.samples_for(128), 128);
    }

    #[test]
    fn stream_health_starts_clean_and_latches_a_fatal_error() {
        let health = StreamHealth::new();
        assert!(health.is_healthy() && !health.is_failed());
        health.record(false);
        assert_eq!(health.error_count(), 1);
        assert!(!health.is_failed(), "a recoverable underrun is not a dead stream");
        health.record(true);
        assert!(health.is_failed() && health.error_count() == 2);
    }

    #[test]
    fn a_spec_reads_back_the_way_a_command_line_says_it() {
        assert_eq!(AudioSpec::stereo(48_000).to_string(), "48000 Hz, 2 ch, device block size");
        assert_eq!(AudioSpec { preferred_block_frames: Some(256), ..AudioSpec::stereo(44_100) }.to_string(), "44100 Hz, 2 ch, 256 frames");
    }
}
