//! [`WorkletBackend`] — the browser's `AudioWorkletProcessor` as an [`AudioBackend`].
//!
//! # Why an `AudioWorklet` fits a trait built for cpal (task D9, research point 1)
//!
//! It looks as though it should not. cpal owns a thread and *pulls* the callback from it; a
//! worklet is *pushed* — the browser calls `process()` on a thread nobody here owns, with
//! exactly 128 frames, and this crate's only entry point is a `process(frames)` export that
//! JavaScript invokes. But [`AudioBackend::open`] never promised to start a thread. It
//! promised to take ownership of a [`RenderCallback`] and call it, and which clock does the
//! calling is the backend's own business, mentioned nowhere in the trait. That is why
//! `starplayer_host::ManualBackend` — a push backend whose clock is a test — was already a
//! legitimate implementation before this one existed.
//!
//! So this is `ManualBackend` with a browser for a clock, and everything genuinely
//! browser-shaped is in [`AudioBackend::negotiate`], where it belongs: an `AudioContext`'s
//! sample rate is fixed at construction and cannot be renegotiated, so `negotiate` answers
//! with the context's rate whatever was asked for, and the engine and every module scan are
//! built from that answer (architecture §4.1). The block size is not a preference here
//! either — `RENDER_QUANTUM` is a promise the browser keeps — so it comes back named.
//!
//! # Real-time rules
//!
//! [`WorkletBackend::render`] is called from `process()`, which is the audio realm: it
//! stores the callback in a plain field rather than behind a lock, because the alternative —
//! `ManualBackend`'s `Mutex`, which is honest for a test harness — would be a lock inside
//! `render()` and design goal 5 forbids exactly that. Nothing here allocates.

use std::boxed::Box;
use std::string::{String, ToString};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::vec;
use std::vec::Vec;

use starplayer::engine::RENDER_QUANTUM;
use starplayer_host::{AudioBackend, AudioSpec, DeviceInfo, HostError, RenderCallback, Stream, StreamControl, StreamHealth};

/// The one device a worklet has: the `AudioContext` that constructed it.
pub const WORKLET_DEVICE_NAME: &str = "AudioWorklet";

/// The open stream's callback and its play/pause flag.
struct OpenStream {
    callback: RenderCallback,
    playing: Arc<AtomicBool>,
}

/// The handle [`Stream`] forwards `play` and `pause` to.
///
/// It shares only an atomic with the backend, never the callback: the callback stays where
/// `process()` can reach it without synchronisation, and a `Stream` travels wherever a
/// `Send` handle is wanted.
struct WorkletStreamControl {
    playing: Arc<AtomicBool>,
}

impl StreamControl for WorkletStreamControl {
    fn play(&self) -> Result<(), HostError> {
        self.playing.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn pause(&self) -> Result<(), HostError> {
        self.playing.store(false, Ordering::Relaxed);
        Ok(())
    }
}

/// The browser's audio graph, as a backend.
pub struct WorkletBackend {
    /// The `AudioContext`'s own rate. Not recoverable from the engine afterwards — the
    /// control clock's interval is `rate / 1000` rounded **down**, so 44 100 Hz comes back
    /// as 44 000 and detunes every module built from it.
    sample_rate_hz: u32,
    open_stream: Option<OpenStream>,
}

impl WorkletBackend {
    /// A backend for a context running at `sample_rate_hz`.
    pub fn new(sample_rate_hz: u32) -> WorkletBackend { WorkletBackend { sample_rate_hz, open_stream: None } }

    /// The rate the context was constructed at.
    pub const fn sample_rate_hz(&self) -> u32 { self.sample_rate_hz }

    /// One block, as the browser's `process()` asks for it: interleaved `f32`, whatever
    /// length the caller passes.
    ///
    /// A paused stream — and a backend nothing has opened — fills `output` with silence and
    /// does not reach the callback, which is what a paused device does. Nothing here
    /// allocates, locks or can panic.
    pub fn render(&mut self, output: &mut [f32]) {
        match &mut self.open_stream {
            Some(stream) if stream.playing.load(Ordering::Relaxed) => (stream.callback)(output),
            _ => output.fill(0.0),
        }
    }
}

impl AudioBackend for WorkletBackend {
    fn devices(&self) -> Vec<DeviceInfo> {
        vec![DeviceInfo {
            name: WORKLET_DEVICE_NAME.to_string(),
            is_default: true,
            supported_rates: vec![self.sample_rate_hz],
            backend: String::from("AudioWorklet"),
        }]
    }

    fn negotiate(&self, device: Option<&str>, requested: AudioSpec) -> Result<AudioSpec, HostError> {
        if let Some(name) = device
            && name != WORKLET_DEVICE_NAME
        {
            return Err(HostError::UnknownDevice(name.to_string()));
        }
        if requested.channels != 1 && requested.channels != 2 {
            let reason = String::from("a worklet output is mono or stereo");
            return Err(HostError::UnsupportedSpec { requested, reason });
        }
        // The context's rate, never the requested one: an `AudioContext` is constructed at a
        // rate and cannot be renegotiated, so answering anything else would build the engine
        // and scan every module at a rate the browser will not play them at.
        Ok(AudioSpec {
            sample_rate_hz: self.sample_rate_hz,
            channels: requested.channels,
            preferred_block_frames: Some(RENDER_QUANTUM as u32),
        })
    }

    fn open(&mut self, device: Option<&str>, spec: AudioSpec, callback: RenderCallback) -> Result<Stream, HostError> {
        let spec = self.negotiate(device, spec)?;
        let playing = Arc::new(AtomicBool::new(false));
        // Whatever was open is replaced, and its callback — with the engine and the module
        // handles inside it — is dropped here. `open` is only ever reached from a worklet
        // message task, never from `process()`, so that `free()` is legal.
        self.open_stream = Some(OpenStream { callback, playing: Arc::clone(&playing) });
        Ok(Stream::new(spec, StreamHealth::new(), Box::new(WorkletStreamControl { playing })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_context_rate_is_the_answer_whatever_was_asked_for() {
        let backend = WorkletBackend::new(44_100);
        let negotiated = backend.negotiate(None, AudioSpec::stereo(48_000)).expect("a worklet takes stereo");
        assert_eq!(negotiated.sample_rate_hz, 44_100, "an AudioContext's rate is not negotiable");
        assert_eq!(negotiated.preferred_block_frames, Some(RENDER_QUANTUM as u32), "128 frames is a promise, not a preference");
        assert!(backend.negotiate(Some("some other card"), AudioSpec::DEFAULT).is_err());
        assert!(backend.negotiate(None, AudioSpec { channels: 6, ..AudioSpec::DEFAULT }).is_err());
    }

    #[test]
    fn a_stream_is_silent_until_it_is_played_and_a_second_open_replaces_the_first() {
        let mut backend = WorkletBackend::new(48_000);
        let stream = backend.open(None, AudioSpec::stereo(48_000), Box::new(|output: &mut [f32]| output.fill(0.5))).expect("it opens");

        let mut block = [1.0f32; 8];
        backend.render(&mut block);
        assert_eq!(block, [0.0; 8], "a stream comes back paused");

        stream.play().expect("play");
        backend.render(&mut block);
        assert_eq!(block, [0.5; 8]);

        let replacement = backend.open(None, AudioSpec::stereo(48_000), Box::new(|output: &mut [f32]| output.fill(0.25))).expect("it reopens");
        replacement.play().expect("play");
        backend.render(&mut block);
        assert_eq!(block, [0.25; 8], "the rebuilt engine is the one being rendered");

        // The retired handle still answers; it simply no longer reaches anything.
        stream.pause().expect("pause");
        backend.render(&mut block);
        assert_eq!(block, [0.25; 8]);
    }
}
