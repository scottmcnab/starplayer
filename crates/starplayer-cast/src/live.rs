//! [`CastStreamBackend`] — an [`AudioBackend`] whose "device" is a Cast speaker.
//!
//! # The invariant a reviewer should check first
//!
//! **Encoding happens on the driver thread, after the render callback has returned, never
//! inside it.** The callback's whole job is to fill a scratch buffer that was sized when
//! the stream opened; quantising to `i16`, running the FLAC encoder and pushing bytes down
//! a channel all happen afterwards, on the driver thread, outside the audio realm. Design
//! goal 5 — no allocation, no locks, no panics inside `render()` — therefore holds here
//! exactly as it does under cpal.
//!
//! # Push, not pull
//!
//! This is a *push* backend in the shape of
//! [`ManualBackend`](starplayer_host::ManualBackend): `open` stores the callback and hands
//! back a paused [`Stream`]; [`Stream::play`] starts the driver thread, and from then on
//! the driver pulls blocks itself. There is no device clock to follow, so the driver paces
//! against the wall clock, keeping about [`STREAM_LEAD`] of audio ahead of real time. The
//! receiver adds two to five seconds of its own buffer on top of that; the lead here only
//! has to be enough that a hiccup in encoding or in the network does not starve it.
//!
//! # Back-pressure
//!
//! The chunk channel is a **bounded** `sync_channel`. If the HTTP side stalls — the
//! speaker stopped draining, the connection went away — the driver blocks rather than
//! growing a queue without limit, and each time it has to wait it records a non-fatal
//! error on the [`StreamHealth`] so the CLI can say "the speaker is not draining the
//! stream" instead of quietly eating memory.
//!
//! # Which thread may touch `Player`
//!
//! Only the control thread. `Player`'s command ring is SPSC and the control thread is its
//! sole producer, so `collect_garbage`, `stop`, `seek_frame` and everything else stay
//! there. The driver thread touches the render callback and nothing else; the heartbeat
//! thread touches neither.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use starplayer::mixer::{Dither, HostSample};
use starplayer_host::{AudioBackend, AudioSpec, DeviceInfo, HostError, RenderCallback, Stream, StreamControl, StreamHealth};

use crate::encode::CastEncoder;

/// The one "device" this backend offers.
pub const CAST_DEVICE_NAME: &str = "cast";

/// How far ahead of the wall clock the driver renders.
///
/// Two seconds: enough that an encode taking longer than usual, or a momentary stall on
/// the HTTP side, does not starve the receiver, and small enough that stopping the stream
/// does not leave seconds of already-encoded audio to play out.
pub const STREAM_LEAD: Duration = Duration::from_secs(2);

/// How long the driver sleeps when it is far enough ahead, or paused.
///
/// It never spins: every path through the loop either renders a block or sleeps.
const DRIVER_TICK: Duration = Duration::from_millis(20);

/// Frames per render when the spec expresses no preference.
///
/// A FLAC frame is 4096 frames, so this fills one exactly and keeps the encoder's
/// remainder buffer empty on every pass.
const DEFAULT_BLOCK_FRAMES: u32 = 4_096;

/// An [`AudioBackend`] that hands rendered audio to an encoder and a channel.
pub struct CastStreamBackend {
    spec: AudioSpec,
    /// Taken by `open`. A backend opens one stream in its life; a second `open` is a
    /// programming error rather than a runtime condition.
    payload: Option<DriverPayload>,
}

/// Everything the driver thread needs, held until [`StreamControl::play`] spawns it.
struct DriverPayload {
    encoder: Box<dyn CastEncoder + Send>,
    chunks: SyncSender<Vec<u8>>,
}

impl CastStreamBackend {
    /// A backend that renders at `spec`, encodes with `encoder`, and sends each encoded
    /// chunk to `chunks`.
    pub fn new(spec: AudioSpec, encoder: Box<dyn CastEncoder + Send>, chunks: SyncSender<Vec<u8>>) -> CastStreamBackend {
        CastStreamBackend { spec, payload: Some(DriverPayload { encoder, chunks }) }
    }
}

impl AudioBackend for CastStreamBackend {
    fn devices(&self) -> Vec<DeviceInfo> {
        vec![DeviceInfo {
            name: String::from(CAST_DEVICE_NAME),
            is_default: true,
            supported_rates: vec![self.spec.sample_rate_hz],
            backend: String::from("cast"),
        }]
    }

    /// The stream is entirely ours to define, so the only thing to negotiate is that the
    /// caller asked for something this backend can encode.
    fn negotiate(&self, device: Option<&str>, requested: AudioSpec) -> Result<AudioSpec, HostError> {
        if let Some(name) = device
            && name != CAST_DEVICE_NAME
        {
            return Err(HostError::UnknownDevice(name.to_string()));
        }
        if requested.channels != 1 && requested.channels != 2 {
            let reason = String::from("a cast stream is mono or stereo");
            return Err(HostError::UnsupportedSpec { requested, reason });
        }
        Ok(AudioSpec { preferred_block_frames: Some(requested.preferred_block_frames.unwrap_or(DEFAULT_BLOCK_FRAMES)), ..requested })
    }

    fn open(&mut self, device: Option<&str>, spec: AudioSpec, callback: RenderCallback) -> Result<Stream, HostError> {
        let spec = self.negotiate(device, spec)?;
        let payload = self.payload.take().ok_or_else(|| HostError::Backend(String::from("this cast backend has already opened its one stream")))?;
        let health = StreamHealth::new();
        let control = CastStreamControl {
            shared: Arc::new(DriverFlags { playing: AtomicBool::new(false), stopped: AtomicBool::new(false) }),
            pending: Mutex::new(Some(PendingDriver { spec, callback, payload, health: Arc::clone(&health) })),
            thread: Mutex::new(None),
        };
        Ok(Stream::new(spec, health, Box::new(control)))
    }
}

/// The two flags the driver thread reads: whether to render, and whether to stop.
struct DriverFlags {
    playing: AtomicBool,
    stopped: AtomicBool,
}

/// What `open` prepared and the first `play` consumes.
struct PendingDriver {
    spec: AudioSpec,
    callback: RenderCallback,
    payload: DriverPayload,
    health: Arc<StreamHealth>,
}

/// The [`Stream`]'s handle: starts the driver on the first `play`, stops and joins it when
/// the stream is dropped.
struct CastStreamControl {
    shared: Arc<DriverFlags>,
    pending: Mutex<Option<PendingDriver>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl StreamControl for CastStreamControl {
    fn play(&self) -> Result<(), HostError> {
        self.shared.playing.store(true, Ordering::SeqCst);
        let pending = self.pending.lock().map_err(|_| HostError::Backend(String::from("the cast stream's state is poisoned")))?.take();
        if let Some(pending) = pending {
            let flags = Arc::clone(&self.shared);
            let handle = std::thread::Builder::new()
                .name(String::from("starplayer-cast-driver"))
                .spawn(move || drive(pending, &flags))
                .map_err(|error| HostError::Backend(format!("could not start the cast driver thread: {error}")))?;
            let mut thread = self.thread.lock().map_err(|_| HostError::Backend(String::from("the cast stream's state is poisoned")))?;
            *thread = Some(handle);
        }
        Ok(())
    }

    fn pause(&self) -> Result<(), HostError> {
        self.shared.playing.store(false, Ordering::SeqCst);
        Ok(())
    }
}

impl Drop for CastStreamControl {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        if let Ok(mut thread) = self.thread.lock()
            && let Some(handle) = thread.take()
        {
            let _ = handle.join();
        }
    }
}

/// The driver thread: render, quantise, encode, send, pace, repeat.
fn drive(pending: PendingDriver, flags: &DriverFlags) {
    let PendingDriver { spec, mut callback, payload, health } = pending;
    let DriverPayload { mut encoder, chunks } = payload;

    let channels = spec.channels.max(1) as usize;
    let block_frames = spec.preferred_block_frames.unwrap_or(DEFAULT_BLOCK_FRAMES).max(1) as usize;
    // Both scratches are sized once, here, and never reallocated: the callback writes into
    // `rendered` and the quantised copy goes into `quantised`.
    let mut rendered = vec![0.0f32; block_frames * channels];
    let mut quantised = vec![0i16; block_frames * channels];
    // Dither is off for the same reason `starplayer-host-cpal` turns it off: the mixer
    // mode's depth post-stage has already decided the signal's resolution, and dithering a
    // second time would add noise describing precision that is not there.
    let mut dither = Dither::OFF;

    let started = Instant::now();
    let mut frames_rendered: u64 = 0;
    let sample_rate_hz = spec.sample_rate_hz.max(1) as f64;

    while !flags.stopped.load(Ordering::SeqCst) {
        if !flags.playing.load(Ordering::SeqCst) {
            std::thread::sleep(DRIVER_TICK);
            continue;
        }
        // Pacing: how much audio exists, against how much time has passed. Sleeping when
        // far enough ahead is what keeps this from rendering the whole session at once.
        let rendered_seconds = frames_rendered as f64 / sample_rate_hz;
        let elapsed_seconds = started.elapsed().as_secs_f64();
        if rendered_seconds > elapsed_seconds + STREAM_LEAD.as_secs_f64() {
            std::thread::sleep(DRIVER_TICK);
            continue;
        }

        rendered.fill(0.0);
        callback(&mut rendered);
        for (destination, &sample) in quantised.iter_mut().zip(rendered.iter()) {
            *destination = <i16 as HostSample>::from_unit_f32(sample, &mut dither);
        }
        frames_rendered += block_frames as u64;

        match encoder.push(&quantised) {
            Ok(bytes) => {
                if !send_chunk(&chunks, bytes, &health, flags) {
                    break;
                }
            }
            Err(_) => {
                health.record(true);
                break;
            }
        }
    }

    // The tail: whatever the encoder held back, and then the channel closing, which is what
    // ends the chunked HTTP response.
    if let Ok(bytes) = encoder.finish() {
        let _ = send_chunk(&chunks, bytes, &health, flags);
    }
    drop(chunks);
}

/// Send one chunk, waiting if the consumer is behind. `false` means give up: the receiver
/// is gone, or the stream has been stopped while this chunk was waiting for room.
///
/// A full channel is a real event worth reporting — it means the speaker is not draining
/// the stream — so it is recorded as a non-fatal error the first time one chunk has to
/// wait. The wait is a retry loop rather than a plain blocking `send` so that dropping the
/// [`Stream`] cannot deadlock against a consumer that has stopped reading.
fn send_chunk(chunks: &SyncSender<Vec<u8>>, bytes: Vec<u8>, health: &StreamHealth, flags: &DriverFlags) -> bool {
    if bytes.is_empty() {
        return true;
    }
    let mut pending = bytes;
    let mut reported = false;
    loop {
        match chunks.try_send(pending) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                if !reported {
                    health.record(false);
                    reported = true;
                }
                if flags.stopped.load(Ordering::SeqCst) {
                    return false;
                }
                pending = returned;
                std::thread::sleep(DRIVER_TICK);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::{FlacEncoder, WavEncoder};
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc::sync_channel;

    /// Render until `wanted_frames` frames have been produced or `deadline` passes, and
    /// return `(chunks, frames the callback filled)`.
    fn run_driver(encoder: Box<dyn CastEncoder + Send>, spec: AudioSpec, wanted_frames: u64) -> (Vec<u8>, u64) {
        let (sender, receiver) = sync_channel::<Vec<u8>>(256);
        let filled = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&filled);
        let channels = spec.channels as usize;

        let mut backend = CastStreamBackend::new(spec, encoder, sender);
        let stream = backend
            .open(None, spec, Box::new(move |output: &mut [f32]| {
                let frames = output.len() / channels;
                let start = counter.fetch_add(frames as u64, Ordering::SeqCst);
                for (index, sample) in output.iter_mut().enumerate() {
                    // A slow ramp that stays inside ±1.0 and is not silence, so a decoder
                    // has something to disagree with us about.
                    *sample = (((start as usize * channels + index) % 2_000) as f32 / 2_000.0) - 0.5;
                }
            }))
            .expect("the cast backend opens");
        assert_eq!(stream.spec().sample_rate_hz, spec.sample_rate_hz);

        stream.play().expect("play starts the driver");
        let mut bytes = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while filled.load(Ordering::SeqCst) < wanted_frames && Instant::now() < deadline {
            if let Ok(chunk) = receiver.recv_timeout(Duration::from_millis(100)) {
                bytes.extend_from_slice(&chunk);
            }
        }
        // Dropping the stream stops the driver and flushes `finish` into the channel.
        drop(stream);
        while let Ok(chunk) = receiver.recv_timeout(Duration::from_millis(500)) {
            bytes.extend_from_slice(&chunk);
        }
        (bytes, filled.load(Ordering::SeqCst))
    }

    #[test]
    fn a_paused_cast_stream_renders_nothing_until_it_is_played() {
        let spec = AudioSpec { sample_rate_hz: 8_000, channels: 2, preferred_block_frames: Some(256) };
        let (sender, receiver) = sync_channel::<Vec<u8>>(4);
        let mut backend = CastStreamBackend::new(spec, Box::new(WavEncoder::new(8_000, 2)), sender);
        let stream = backend.open(None, spec, Box::new(|output: &mut [f32]| output.fill(0.25))).unwrap();

        assert!(receiver.recv_timeout(Duration::from_millis(200)).is_err(), "the stream comes back paused, as the backend contract promises");
        drop(stream);
    }

    #[test]
    fn the_chunks_of_a_live_wav_stream_carry_every_frame_the_callback_filled() {
        let spec = AudioSpec { sample_rate_hz: 8_000, channels: 2, preferred_block_frames: Some(512) };
        let (bytes, frames) = run_driver(Box::new(WavEncoder::new(8_000, 2)), spec, 8_000);

        assert!(frames >= 8_000, "the driver renders ahead of the wall clock, so a second of audio arrives at once");
        assert_eq!(&bytes[0..4], b"RIFF");
        let payload_bytes = bytes.len() - 44;
        assert_eq!(payload_bytes / (2 * 2), frames as usize, "every frame the callback filled reached the channel");
    }

    #[test]
    fn the_chunks_of_a_live_flac_stream_decode_to_the_frame_count_that_went_in() {
        use flacenc::component::Decode;

        let spec = AudioSpec { sample_rate_hz: 8_000, channels: 2, preferred_block_frames: Some(FLAC_FRAME_FRAMES) };
        let (bytes, frames) = run_driver(Box::new(FlacEncoder::new(8_000, 2).unwrap()), spec, 8_000);

        assert_eq!(&bytes[0..4], b"fLaC");
        let (_, stream_info) = flacenc::component::parser::stream_info::<()>(&bytes[8..]).expect("STREAMINFO parses");
        assert_eq!(stream_info.total_samples(), 0, "a live stream's length is unknown");

        let mut rest = &bytes[8 + 34..];
        let mut decoded_frames = 0usize;
        let mut decoded_samples = 0usize;
        let mut parse_frame = flacenc::component::parser::frame::<()>(&stream_info, true);
        while !rest.is_empty() {
            let Ok((remaining, frame)) = parse_frame(rest) else { break };
            decoded_frames += frame.block_size();
            decoded_samples += frame.decode().len();
            rest = remaining;
        }
        assert!(rest.is_empty(), "{} bytes were left unparsed", rest.len());
        assert_eq!(decoded_frames, frames as usize);
        assert_eq!(decoded_samples, frames as usize * 2);
    }

    /// The block size the FLAC live test renders at, so every pass fills exactly one frame.
    const FLAC_FRAME_FRAMES: u32 = crate::encode::FLAC_BLOCK_SIZE as u32;

    #[test]
    fn the_backend_offers_one_device_and_refuses_a_name_that_is_not_it() {
        let spec = AudioSpec::stereo(44_100);
        let (sender, _receiver) = sync_channel::<Vec<u8>>(1);
        let backend = CastStreamBackend::new(spec, Box::new(WavEncoder::new(44_100, 2)), sender);
        assert_eq!(backend.devices().len(), 1);
        assert_eq!(backend.devices()[0].name, CAST_DEVICE_NAME);
        assert!(backend.negotiate(Some("speakers"), spec).is_err());
        assert!(backend.negotiate(Some(CAST_DEVICE_NAME), spec).is_ok());
        let eight_channel = AudioSpec { channels: 8, ..spec };
        assert!(backend.negotiate(None, eight_channel).is_err(), "a cast stream is mono or stereo");
    }
}
