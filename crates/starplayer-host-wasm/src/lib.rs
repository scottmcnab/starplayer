//! Browser AudioWorklet host for the real StarPlayer S3M engine.
//!
//! The worklet owns this wasm instance. A second, page-side wasm instance validates
//! files and supplies metadata without touching the render instance. Once validation
//! succeeds the original `ArrayBuffer` is transferred here; activation builds the
//! `Arc<Module>` and S3M sequencer outside `process()`, then hands the module through the
//! engine's typed command ring. Every buffer used by `process()` is allocated by `init`.
//!
//! # Why the module is decoded twice
//!
//! The two instances do not share memory, so an `Arc<Module>` built on the page cannot be
//! handed to the worklet — only bytes can cross, and they cross once, as a transferred
//! `ArrayBuffer`. Each instance therefore runs `starplayer::s3m::load` on those bytes.
//! That is milliseconds of work and a second copy of the module, and it buys two things
//! worth more than either: an untrusted file is rejected on the page, before it can reach
//! the live audio graph at all, and the pattern view's `PatternCell` decoding runs on the
//! page thread rather than in the realm that has to hit a 2.7 ms deadline.
//!
//! The alternative — one instance with shared wasm memory — needs the `atomics` target
//! feature, which in turn needs a `std` rebuilt with `-Zbuild-std` on a nightly
//! toolchain, and would put the whole loader inside the audio realm. Rejected for a
//! stable toolchain and a clean realm boundary.

#![deny(unsafe_code)]

mod command;

use core::cell::{Cell, RefCell};
use std::boxed::Box;
use std::rc::Rc;
use std::vec;
use std::vec::Vec;

use command::{CommandRing, WireCommand};
use starplayer::core::{ChannelId, Command, ExactFixedPoint, Frame, U0F16};
use starplayer::dsp::{GainRamp, Linear};
use starplayer::engine::{Engine, EngineContext, EngineHandle, EngineSettings, EventSource, PatternSequencer, RENDER_QUANTUM};
use starplayer::mixer::{FloatPath, StereoF32};
use starplayer::model::Module;
use starplayer::rt::Arc;
use starplayer::s3m::{S3mPatternData, S3mProcessor};
use starplayer::telemetry::{Snapshot, TelemetryReader};

pub use command::COMMAND_RING_CAPACITY;

/// Frames the wasm-owned planar buffer can expose in one call.
pub const MAX_FRAMES_PER_CALL: usize = 1024;
/// The browser player is stereo.
pub const OUTPUT_CHANNELS: usize = 2;

/// The transient reservation forces wasm pages to be committed before any view is taken.
/// It is returned to the allocator immediately and then reused by module activation.
const HEAP_RESERVE_BYTES: usize = 16 * 1024 * 1024;
const TRANSPORT_RAMP_FRAMES: u32 = 64;
const TRANSPORT_GAIN_UNITY: i32 = 32_767;
/// Fraction of the held master peak that survives one quantum: about a 200 ms fall from
/// full scale to silence at 48 kHz, which reads well on a bar.
const MASTER_PEAK_DECAY_PER_QUANTUM: f32 = 0.94;

// Wire opcodes. Kept in one obvious block beside `ring.js`'s matching constants.
const OPCODE_PLAY: u8 = 1;
const OPCODE_STOP: u8 = 2;
const OPCODE_SEEK_ORDER: u8 = 3;
const OPCODE_SEEK_ROW: u8 = 4;
const OPCODE_MASTER_VOLUME: u8 = 5;
const OPCODE_MUTE_CHANNEL: u8 = 6;

/// Layout exported to the worklet, then copied coherently to the SAB telemetry block.
const TELEMETRY_HEADER_WORDS: usize = 18;
const TELEMETRY_CHANNEL_WORDS: usize = 8;
const TELEMETRY_CHANNELS: usize = 64;
const TELEMETRY_WORDS: usize = TELEMETRY_HEADER_WORDS + TELEMETRY_CHANNELS * TELEMETRY_CHANNEL_WORDS;

type S3mSequencer = PatternSequencer<ExactFixedPoint, S3mProcessor, S3mPatternData>;
type WebEngine = Engine<FloatPath, Linear, StereoF32, Arc<Module>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SeekKind {
    #[default]
    None,
    Order(u16),
    Row(u16),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SeekRequest {
    kind: SeekKind,
    frame: Frame,
}

/// A concrete S3M source with a host-owned seek mailbox.
///
/// `Engine` intentionally stores `dyn EventSource`, so its generic command handler cannot
/// downcast to `PatternSequencer`. This wrapper is the safe host-specific bridge: the
/// command remains a typed `Command`, but order/row seeks become one fixed-size mailbox
/// write. The wrapper consumes it at an event boundary and restarts the sequencer clock
/// on the engine's monotonic source timeline. No allocation occurs in `process()`.
struct SeekableS3mSource {
    sequencer: S3mSequencer,
    request: Rc<Cell<SeekRequest>>,
}

impl EventSource for SeekableS3mSource {
    fn next_event_frame(&self) -> Option<Frame> {
        let request = self.request.get();
        match request.kind {
            SeekKind::None => self.sequencer.next_event_frame(),
            _ => Some(match self.sequencer.next_event_frame() {
                Some(next) => core::cmp::min(next, request.frame),
                None => request.frame,
            }),
        }
    }

    fn advance_to(&mut self, frame: Frame) { self.sequencer.advance_to(frame); }

    fn dispatch(&mut self, frame: Frame, context: &mut EngineContext<'_>) {
        let request = self.request.get();
        if !matches!(request.kind, SeekKind::None) && frame >= request.frame {
            self.request.set(SeekRequest::default());
            match request.kind {
                SeekKind::Order(order) => { let _ = self.sequencer.seek_order(order); }
                SeekKind::Row(row) => self.sequencer.seek_row(row),
                SeekKind::None => {}
            }
            self.sequencer.restart_clock_at(frame);
        }
        if self.sequencer.next_event_frame().is_some_and(|next| next <= frame) {
            self.sequencer.dispatch(frame, context);
        }
    }
}

struct Host {
    engine: WebEngine,
    /// The `AudioContext`'s own rate, kept because it cannot be recovered from the engine:
    /// the control clock's interval is `rate / 1000` rounded **down**, so 44 100 Hz comes
    /// back as 44 000 and detunes every module built from it.
    sample_rate_hz: u32,
    control: EngineHandle<Arc<Module>>,
    telemetry: TelemetryReader,
    commands: CommandRing,
    seek_request: Option<Rc<Cell<SeekRequest>>>,
    interleaved: Vec<f32>,
    planar: Vec<f32>,
    telemetry_words: Box<[i32]>,
    transport_gain: GainRamp,
    pending_engine_stop: bool,
    commands_rejected: u32,
    quanta_rendered: u64,
    module_generation: u32,
    retired_modules_collected: u32,
    last_peak: f32,
}

impl Host {
    fn new(sample_rate_hz: u32) -> Host {
        let mut engine = WebEngine::with_settings(EngineSettings {
            sample_rate_hz,
            voice_capacity: 64,
            channel_count: 32,
            ..EngineSettings::default()
        });
        let control = engine.take_control().expect("a new engine owns its control handle");
        let telemetry = engine.telemetry_reader().expect("telemetry is enabled for the web host");
        Host {
            engine,
            sample_rate_hz,
            control,
            telemetry,
            commands: CommandRing::new(),
            seek_request: None,
            interleaved: vec![0.0; MAX_FRAMES_PER_CALL * OUTPUT_CHANNELS],
            planar: vec![0.0; MAX_FRAMES_PER_CALL * OUTPUT_CHANNELS],
            telemetry_words: vec![0; TELEMETRY_WORDS].into_boxed_slice(),
            transport_gain: GainRamp::steady(TRANSPORT_GAIN_UNITY),
            pending_engine_stop: false,
            commands_rejected: 0,
            quanta_rendered: 0,
            module_generation: 0,
            retired_modules_collected: 0,
            last_peak: 0.0,
        }
    }

    /// Decode, construct and queue a module. Called from the worklet's message handler,
    /// never from `process()`; a failed decode leaves the previous module and source live.
    fn load_module(&mut self, bytes: &[u8]) -> Result<u32, String> {
        let loaded = starplayer::s3m::load(bytes).map_err(|error| error.to_string())?;
        let module = Arc::new(loaded);
        let mut sequencer = starplayer::s3m::sequencer_for(Arc::clone(&module), self.sample_rate_hz, ExactFixedPoint);
        // A new sequencer's tick clock starts at frame zero, but the engine's musical
        // clock is monotonic and has been running since `init`. Without this the first
        // tick of the new module is due thousands of frames in the past, and the engine
        // burns ticks trying to catch up until it gives up and raises
        // `zero_advance_forced`. Start the clock where the engine actually is.
        sequencer.restart_clock_at(self.engine.source_frame());
        let request = Rc::new(Cell::new(SeekRequest::default()));
        let source = SeekableS3mSource { sequencer, request: Rc::clone(&request) };

        self.control.load_module(Arc::clone(&module)).map_err(|_| String::from("the engine command ring is full"))?;
        self.engine.set_source(Box::new(source));
        self.seek_request = Some(request);
        self.module_generation = self.module_generation.wrapping_add(1).max(1);
        Ok(self.module_generation)
    }

    fn enqueue(&mut self, command: WireCommand) -> bool { self.commands.push(command) }

    fn decode(command: WireCommand) -> Option<Command<Arc<Module>>> {
        match command.opcode {
            OPCODE_PLAY => Some(Command::Play),
            OPCODE_STOP => Some(Command::Stop),
            OPCODE_SEEK_ORDER => Some(Command::SeekOrder(command.argument)),
            OPCODE_SEEK_ROW => Some(Command::SeekRow(command.argument)),
            OPCODE_MASTER_VOLUME => Some(Command::SetMasterVolume(U0F16::from_bits(command.argument))),
            OPCODE_MUTE_CHANNEL => Some(Command::MuteChannel {
                channel: ChannelId(command.argument),
                muted: command.extra != 0,
            }),
            _ => None,
        }
    }

    fn drain_commands(&mut self) {
        for _ in 0..COMMAND_RING_CAPACITY {
            let Some(wire) = self.commands.pop() else { break };
            let Some(command) = Self::decode(wire) else {
                self.commands_rejected = self.commands_rejected.saturating_add(1);
                continue;
            };
            match command {
                // Fade first; the typed engine stop is queued when the ramp reaches zero.
                Command::Stop => {
                    self.transport_gain.glide_to(0, TRANSPORT_RAMP_FRAMES);
                    self.pending_engine_stop = true;
                }
                Command::Play => {
                    self.pending_engine_stop = false;
                    self.transport_gain.glide_to(TRANSPORT_GAIN_UNITY, TRANSPORT_RAMP_FRAMES);
                    if self.control.send(Command::Play).is_err() {
                        self.commands_rejected = self.commands_rejected.saturating_add(1);
                    }
                }
                // The dyn source boundary cannot safely downcast. Route only these two
                // typed variants through the wrapper's fixed mailbox.
                Command::SeekOrder(order) => self.request_seek(SeekKind::Order(order)),
                Command::SeekRow(row) => self.request_seek(SeekKind::Row(row)),
                other => {
                    if self.control.send(other).is_err() {
                        self.commands_rejected = self.commands_rejected.saturating_add(1);
                    }
                }
            }
        }
    }

    fn request_seek(&mut self, kind: SeekKind) {
        if let Some(request) = &self.seek_request {
            request.set(SeekRequest { kind, frame: self.engine.source_frame() });
        } else {
            self.commands_rejected = self.commands_rejected.saturating_add(1);
        }
    }

    fn process(&mut self, frames: usize) -> f32 {
        self.drain_commands();
        let frames = frames.min(MAX_FRAMES_PER_CALL);
        let sample_count = frames.saturating_mul(OUTPUT_CHANNELS);
        if let Some(interleaved) = self.interleaved.get_mut(..sample_count) {
            self.engine.render(interleaved);
        }

        let mut peak = 0.0f32;
        for frame in 0..frames {
            let gain = self.transport_gain.advance() as f32 / TRANSPORT_GAIN_UNITY as f32;
            let left = self.interleaved.get(frame * 2).copied().unwrap_or(0.0) * gain;
            let right = self.interleaved.get(frame * 2 + 1).copied().unwrap_or(0.0) * gain;
            if let Some(sample) = self.planar.get_mut(frame) { *sample = left; }
            if let Some(sample) = self.planar.get_mut(MAX_FRAMES_PER_CALL + frame) { *sample = right; }
            peak = peak.max(left.abs()).max(right.abs());
        }

        if self.pending_engine_stop && !self.transport_gain.is_ramping() {
            if self.control.send(Command::Stop).is_ok() {
                self.pending_engine_stop = false;
            } else {
                self.commands_rejected = self.commands_rejected.saturating_add(1);
            }
        }

        self.quanta_rendered = self.quanta_rendered.wrapping_add(1);
        // Peak-hold with decay, as the original walked `_VUBarLevel` down every tick: a
        // bare 2.7 ms quantum peak flickers, and a quantum that lands between transients
        // reads as silence.
        self.last_peak = peak.max(self.last_peak * MASTER_PEAK_DECAY_PER_QUANTUM);
        let snapshot = *self.telemetry.read();
        self.pack_telemetry(snapshot);
        peak
    }

    fn pack_telemetry(&mut self, snapshot: Snapshot) {
        let warnings = (snapshot.warnings.zero_advance_forced as i32)
            | ((snapshot.warnings.event_limit_reached as i32) << 1)
            | ((snapshot.warnings.retired_module_dropped as i32) << 2)
            | ((snapshot.warnings.unsupported_command as i32) << 3);
        let header = [
            snapshot.sequence as i32,
            snapshot.publishes_dropped as i32,
            snapshot.channel_count as i32,
            snapshot.voices_active as i32,
            snapshot.transport.order as i32,
            snapshot.transport.pattern as i32,
            snapshot.transport.row as i32,
            snapshot.transport.tick as i32,
            snapshot.transport.speed as i32,
            snapshot.transport.tempo_bpm as i32,
            snapshot.transport.global_volume.to_bits() as i32,
            warnings,
            self.engine.frame().0 as i32,
            self.control.pending_garbage() as i32,
            self.module_generation as i32,
            (self.engine.is_playing() && !self.pending_engine_stop) as i32,
            // The master peak is a lossy audio tap (architecture §9): a torn or stale
            // reading costs a UI nothing, so it rides the same block as scalars for now.
            (self.last_peak.clamp(0.0, 1.0) * 65_535.0) as i32,
            self.retired_modules_collected as i32,
        ];
        if let Some(destination) = self.telemetry_words.get_mut(..TELEMETRY_HEADER_WORDS) {
            destination.copy_from_slice(&header);
        }

        for (index, channel) in snapshot.channels.iter().enumerate() {
            let base = TELEMETRY_HEADER_WORDS + index * TELEMETRY_CHANNEL_WORDS;
            let flags = (channel.active as i32) | ((channel.muted as i32) << 1);
            let values = [
                channel.note.map(|note| note.semitone as i32).unwrap_or(-1),
                channel.instrument as i32,
                channel.volume.to_bits() as i32,
                channel.pan.to_bits() as i32,
                channel.effect.code as i32,
                channel.effect.param as i32,
                channel.vu_level.to_bits() as i32,
                flags,
            ];
            if let Some(destination) = self.telemetry_words.get_mut(base..base + TELEMETRY_CHANNEL_WORDS) {
                destination.copy_from_slice(&values);
            }
        }
    }

    /// Drop every retired `Arc<Module>` the render path has handed back, and keep a
    /// running total. The page asserts on the total rather than on one call's return
    /// value: collection is polled from a message task, so which task sees the retired
    /// handle is a scheduling detail, but the cumulative count is not.
    fn collect_garbage(&mut self) -> usize {
        let collected = self.control.collect_all_garbage();
        self.retired_modules_collected = self.retired_modules_collected.saturating_add(collected as u32);
        collected
    }

    fn dropped_commands(&self) -> u32 {
        self.commands.dropped().saturating_add(self.commands_rejected)
    }
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

fn with_host<T>(fallback: T, action: impl FnOnce(&mut Host) -> T) -> T {
    HOST.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => match slot.as_mut() {
            Some(host) => action(host),
            None => fallback,
        },
        Err(_) => fallback,
    })
}

#[allow(unsafe_code, reason = "`#[wasm_bindgen]` expands to unsafe ABI shims")]
mod exports {
    use super::*;
    use wasm_bindgen::prelude::{JsValue, wasm_bindgen};

    /// Allocate the engine, rings and all steady-state render buffers.
    #[wasm_bindgen]
    pub fn init(sample_rate: f32, _channels: u32) {
        drop(core::hint::black_box(vec![0u8; HEAP_RESERVE_BYTES]));
        let sample_rate_hz = sample_rate.round().clamp(8_000.0, 384_000.0) as u32;
        HOST.with(|cell| {
            if let Ok(mut slot) = cell.try_borrow_mut() {
                *slot = Some(Host::new(sample_rate_hz));
            }
        });
    }

    /// Decode and activate one validated S3M byte buffer outside `process()`.
    #[wasm_bindgen]
    pub fn load_module(bytes: &[u8]) -> Result<u32, JsValue> {
        HOST.with(|cell| {
            let mut slot = cell.try_borrow_mut().map_err(|_| JsValue::from_str("the worklet host is busy"))?;
            let host = slot.as_mut().ok_or_else(|| JsValue::from_str("the worklet host is not initialized"))?;
            host.load_module(bytes).map_err(|message| JsValue::from_str(&message))
        })
    }

    /// Decode one SAB/fallback wire record into the wasm-side fixed command ring.
    #[wasm_bindgen]
    pub fn enqueue_command(opcode: u32, argument: u32, extra: u32) -> bool {
        let command = WireCommand { opcode: opcode as u8, argument: argument as u16, extra: extra as u16 };
        with_host(false, |host| host.enqueue(command))
    }

    #[wasm_bindgen]
    pub fn process(frames: u32) -> f32 { with_host(0.0, |host| host.process(frames as usize)) }

    #[wasm_bindgen]
    pub fn output_ptr() -> u32 { with_host(0, |host| host.planar.as_ptr() as usize as u32) }

    #[wasm_bindgen]
    pub fn output_len() -> u32 { (MAX_FRAMES_PER_CALL * OUTPUT_CHANNELS) as u32 }

    #[wasm_bindgen]
    pub fn output_channel_stride() -> u32 { MAX_FRAMES_PER_CALL as u32 }

    #[wasm_bindgen]
    pub fn telemetry_ptr() -> u32 { with_host(0, |host| host.telemetry_words.as_ptr() as usize as u32) }

    #[wasm_bindgen]
    pub fn telemetry_len() -> u32 { TELEMETRY_WORDS as u32 }

    #[wasm_bindgen]
    pub fn render_quantum() -> u32 { RENDER_QUANTUM as u32 }

    #[wasm_bindgen]
    pub fn dropped_commands() -> u32 { with_host(0, |host| host.dropped_commands()) }

    #[wasm_bindgen]
    pub fn quanta_rendered() -> f64 { with_host(0.0, |host| host.quanta_rendered as f64) }

    /// Drop retired module Arcs from a worklet message task, never from `process()`.
    #[wasm_bindgen]
    pub fn collect_garbage() -> u32 { with_host(0, |host| host.collect_garbage() as u32) }

    #[wasm_bindgen]
    pub fn pending_garbage() -> u32 { with_host(0, |host| host.control.pending_garbage() as u32) }

    /// Retired module handles dropped off the audio callback since `init`.
    #[wasm_bindgen]
    pub fn retired_modules_collected() -> u32 { with_host(0, |host| host.retired_modules_collected) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");

    #[test]
    fn a_real_s3m_reaches_the_real_engine_and_renders() {
        let mut host = Host::new(48_000);
        assert_eq!(host.load_module(FIXTURE), Ok(1));
        let mut heard = false;
        for _ in 0..200 {
            heard |= host.process(RENDER_QUANTUM) > 0.0;
        }
        assert!(heard, "the S3M sequencer should trigger sample audio");
        assert!(host.telemetry.read().sequence > 0);
    }

    #[test]
    fn the_sequencer_is_built_at_the_context_s_own_sample_rate() {
        let mut host = Host::new(44_100);
        assert_eq!(host.sample_rate_hz, 44_100, "not the control clock's rounded-down 44 000");
        assert!(host.load_module(FIXTURE).is_ok());
        assert!((0..200).any(|_| host.process(RENDER_QUANTUM) > 0.0));
    }

    #[test]
    fn a_bad_load_keeps_the_previous_module_usable() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        assert!(host.load_module(b"not an s3m").is_err());
        assert_eq!(host.module_generation, 1);
        for _ in 0..10 { host.process(RENDER_QUANTUM); }
        assert!(host.telemetry.read().sequence > 0);
    }

    #[test]
    fn a_replaced_module_is_counted_when_it_returns_down_the_garbage_channel() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        host.process(RENDER_QUANTUM);
        assert_eq!(host.retired_modules_collected, 0, "nothing has been replaced yet");
        assert!(host.load_module(FIXTURE).is_ok());
        host.process(RENDER_QUANTUM);
        assert_eq!(host.collect_garbage(), 1, "the first module was handed back once the swap landed");
        assert_eq!(host.retired_modules_collected, 1);
        assert_eq!(host.control.pending_garbage(), 0);
    }

    #[test]
    fn swapping_a_module_mid_song_does_not_force_the_musical_clock_forward() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        host.enqueue(WireCommand { opcode: OPCODE_PLAY, argument: 0, extra: 0 });
        // Long enough that a clock left at zero would be more than `MAX_ZERO_ADVANCE`
        // ticks behind the engine when the second module arrives.
        for _ in 0..1_500 { host.process(RENDER_QUANTUM); }
        assert!(!host.engine.warnings().zero_advance_forced, "the first module plays cleanly");

        assert!(host.load_module(FIXTURE).is_ok());
        for _ in 0..100 { host.process(RENDER_QUANTUM); }
        assert!(!host.engine.warnings().zero_advance_forced, "and so does the one loaded over the top of it");
    }

    #[test]
    fn seek_commands_use_the_fixed_mailbox_without_flagging_engine_unsupported() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        assert!(host.enqueue(WireCommand { opcode: OPCODE_SEEK_ORDER, argument: 1, extra: 0 }));
        host.process(RENDER_QUANTUM);
        assert!(!host.engine.warnings().unsupported_command);
    }
}
