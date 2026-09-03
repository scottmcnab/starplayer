//! [`ManualBackend`] — an [`AudioBackend`] with no device behind it, driven by the caller.
//!
//! It exists for three reasons, and the first is the important one:
//!
//! 1. **It is the abstraction's second implementation.** AGENTS.md design goal 8 says a
//!    trait is not committed until its second real implementation exists, and the browser
//!    retrofit is a follow-up. This is a real one: it goes through exactly the calls cpal
//!    goes through, in the same order, with the same callback.
//! 2. **It makes the invariants testable without a sound card.** Buffer-size independence
//!    is design goal 3, and proving it needs a stream that can be asked for 1 frame and
//!    then 8191. No device will do that on request; this one does.
//! 3. **A headless machine still has a host.** CI, a container and a build box have no
//!    audio device, and "the tests only run where speakers do" is not a test suite.

use std::boxed::Box;
use std::string::{String, ToString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::vec;
use std::vec::Vec;

use crate::backend::{AudioBackend, AudioSpec, DeviceInfo, HostError, RenderCallback, Stream, StreamControl, StreamHealth};

/// The one device [`ManualBackend`] offers.
pub const MANUAL_DEVICE_NAME: &str = "manual";

/// The rates the manual device claims, so a caller exercising rate negotiation has
/// something to negotiate against.
pub const MANUAL_RATES: [u32; 4] = [22_050, 44_100, 48_000, 96_000];

/// The callback and its play/pause flag, shared between the [`Stream`] and the
/// [`ManualDriver`] that pumps it.
struct ManualStream {
    callback: Mutex<RenderCallback>,
    playing: AtomicBool,
    health: Arc<StreamHealth>,
}

impl StreamControl for ManualHandle {
    fn play(&self) -> Result<(), HostError> {
        self.stream.playing.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn pause(&self) -> Result<(), HostError> {
        self.stream.playing.store(false, Ordering::SeqCst);
        Ok(())
    }
}

/// The control handle the [`Stream`] forwards `play` and `pause` to.
struct ManualHandle {
    stream: Arc<ManualStream>,
}

/// Pumps an open manual stream: the caller's stand-in for a device's clock.
///
/// A `Mutex` around the callback, deliberately. This is not an audio thread and never
/// pretends to be one; the lock is what makes a callback that a real backend would own
/// exclusively safe to call from a test. Everything *inside* the callback is still held to
/// the real-time rules, which is exactly what the allocation test measures.
#[derive(Clone)]
pub struct ManualDriver {
    stream: Arc<ManualStream>,
}

impl ManualDriver {
    /// Render one block of interleaved `f32`, as a device callback would.
    ///
    /// A paused stream fills `output` with silence and does not call the callback, which is
    /// what a paused device does.
    pub fn render(&self, output: &mut [f32]) {
        if !self.stream.playing.load(Ordering::SeqCst) {
            output.fill(0.0);
            return;
        }
        let mut callback = self.stream.callback.lock().expect("the manual callback is not poisoned");
        callback(output);
    }

    /// Render `frames` frames in blocks of `block_frames`, as a device with that block size
    /// would. Returns the interleaved output.
    pub fn render_blocks(&self, spec: AudioSpec, frames: usize, block_frames: usize) -> Vec<f32> {
        let mut output = vec![0.0f32; spec.samples_for(frames)];
        let block_samples = spec.samples_for(block_frames.max(1)).max(spec.channels as usize);
        for block in output.chunks_mut(block_samples) {
            self.render(block);
        }
        output
    }

    /// Report a backend error, the way a device that underran would.
    pub fn fail(&self, fatal: bool) { self.stream.health.record(fatal); }
}

/// A backend with one device, whose clock is the caller.
#[derive(Default)]
pub struct ManualBackend {
    open_stream: Option<Arc<ManualStream>>,
}

impl ManualBackend {
    /// A backend with nothing open.
    pub fn new() -> ManualBackend { ManualBackend::default() }

    /// The driver for the stream this backend last opened.
    pub fn driver(&self) -> Option<ManualDriver> {
        self.open_stream.as_ref().map(|stream| ManualDriver { stream: Arc::clone(stream) })
    }
}

impl AudioBackend for ManualBackend {
    fn devices(&self) -> Vec<DeviceInfo> {
        vec![DeviceInfo {
            name: MANUAL_DEVICE_NAME.to_string(),
            is_default: true,
            supported_rates: MANUAL_RATES.to_vec(),
            backend: String::from("manual"),
        }]
    }

    fn negotiate(&self, device: Option<&str>, requested: AudioSpec) -> Result<AudioSpec, HostError> {
        if let Some(name) = device
            && name != MANUAL_DEVICE_NAME
        {
            return Err(HostError::UnknownDevice(name.to_string()));
        }
        if requested.channels != 1 && requested.channels != 2 {
            let reason = String::from("the manual device is mono or stereo");
            return Err(HostError::UnsupportedSpec { requested, reason });
        }
        // The nearest advertised rate, so a caller asking for something exotic gets the
        // same "you asked, the device answered" shape a real device gives.
        let rate = MANUAL_RATES
            .iter()
            .copied()
            .min_by_key(|rate| rate.abs_diff(requested.sample_rate_hz))
            .unwrap_or(48_000);
        Ok(AudioSpec { sample_rate_hz: rate, ..requested })
    }

    fn open(&mut self, device: Option<&str>, spec: AudioSpec, callback: RenderCallback) -> Result<Stream, HostError> {
        let spec = self.negotiate(device, spec)?;
        let health = StreamHealth::new();
        let stream = Arc::new(ManualStream {
            callback: Mutex::new(callback),
            playing: AtomicBool::new(false),
            health: Arc::clone(&health),
        });
        self.open_stream = Some(Arc::clone(&stream));
        Ok(Stream::new(spec, health, Box::new(ManualHandle { stream })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paused_stream_renders_silence_and_a_playing_one_reaches_the_callback() {
        let mut backend = ManualBackend::new();
        let spec = backend.negotiate(None, AudioSpec::stereo(44_100)).expect("the manual device takes 44.1 kHz");
        let stream = backend.open(None, spec, Box::new(|output: &mut [f32]| output.fill(0.5))).expect("it opens");
        let driver = backend.driver().expect("the backend keeps the driver");

        let mut block = [0.0f32; 8];
        driver.render(&mut block);
        assert_eq!(block, [0.0; 8], "a stream comes back paused");

        stream.play().expect("play");
        driver.render(&mut block);
        assert_eq!(block, [0.5; 8]);

        stream.pause().expect("pause");
        driver.render(&mut block);
        assert_eq!(block, [0.0; 8]);
    }

    #[test]
    fn negotiation_answers_with_the_nearest_advertised_rate_and_rejects_a_name_it_has_not_got() {
        let backend = ManualBackend::new();
        assert_eq!(backend.negotiate(None, AudioSpec::stereo(44_100)).map(|spec| spec.sample_rate_hz), Ok(44_100));
        assert_eq!(backend.negotiate(None, AudioSpec::stereo(45_000)).map(|spec| spec.sample_rate_hz), Ok(44_100));
        assert_eq!(backend.negotiate(None, AudioSpec::stereo(192_000)).map(|spec| spec.sample_rate_hz), Ok(96_000));
        assert_eq!(backend.negotiate(Some("no such device"), AudioSpec::DEFAULT), Err(HostError::UnknownDevice(String::from("no such device"))));
    }

    #[test]
    fn the_health_flag_carries_what_the_backend_reported() {
        let mut backend = ManualBackend::new();
        let stream = backend.open(None, AudioSpec::stereo(48_000), Box::new(|_| {})).expect("it opens");
        let driver = backend.driver().expect("driver");
        assert!(stream.health().is_healthy());
        driver.fail(true);
        assert!(stream.health().is_failed() && stream.health().error_count() == 1);
    }
}
