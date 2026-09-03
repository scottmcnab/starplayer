//! Browser AudioWorklet host for the real StarPlayer S3M engine.
//!
//! The worklet owns this wasm instance. A second, page-side wasm instance validates
//! files and supplies metadata without touching the render instance. Once validation
//! succeeds the original `ArrayBuffer` is transferred here; activation builds the
//! `Arc<Module>` and format-native sequencer outside `process()`, then hands the module through the
//! engine's typed command ring. Every buffer used by `process()` is allocated by `init`.
//!
//! # Why the module is decoded twice
//!
//! The two instances do not share memory, so an `Arc<Module>` built on the page cannot be
//! handed to the worklet — only bytes can cross, and they cross once, as a transferred
//! `ArrayBuffer`. Each instance therefore runs the facade's native format autodetection on
//! those bytes.
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
use starplayer::core::quirks::QuirkSelection;
use starplayer::core::{AtEnd, ChannelId, Command, Frame, Interpolator, U0F16};
use starplayer::dsp::{GainRamp, Interpolate, Linear, Nearest};
use starplayer::engine::{
    ChannelTable, Engine, EngineContext, EngineHandle, EngineSettings, EventSource, MixPathKind, MixerMode,
    OutputDepth, RENDER_QUANTUM, ScanLimits,
};
use starplayer::mixer::{Dither, FixedOut, FixedPath, FloatOut, FloatPath, HostSample, I24, MixPath, OutputFormat};
use starplayer::model::{Module, ModuleFormat};
use starplayer::rt::Arc;
use starplayer::telemetry::{SongEnd, Snapshot, TelemetryReader};
use starplayer::{MAX_VOICE_CAPACITY, NativeSequencer, ScannedSong};

pub use command::COMMAND_RING_CAPACITY;

/// Frames the wasm-owned planar buffer can expose in one call.
pub const MAX_FRAMES_PER_CALL: usize = 1024;
/// The largest channel count the browser player exposes.
pub const MAX_OUTPUT_CHANNELS: usize = 2;

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
const OPCODE_SET_MIXER_MODE: u8 = 7;
/// Seek to an elapsed position in the song. `argument` is a song frame.
const OPCODE_SEEK_FRAME: u8 = 8;
/// What to do at the detected loop point: `argument` 0 fade out, 1 continue, 2 stop;
/// `extra` is the fade length in frames.
const OPCODE_AT_END: u8 = 9;

/// Wire spelling of [`AtEnd`], chosen so the page's default repeat-off value is zero.
const AT_END_FADE_OUT: u32 = 0;
const AT_END_CONTINUE: u32 = 1;
const AT_END_STOP: u32 = 2;

/// Fade length used when the page asks for one without naming a length.
const DEFAULT_FADE_FRAMES: u32 = 10 * 48_000;

/// Layout exported to the worklet, then copied coherently to the SAB telemetry block.
const TELEMETRY_HEADER_WORDS: usize = 22;
const TELEMETRY_CHANNEL_WORDS: usize = 8;
const TELEMETRY_CHANNELS: usize = 64;
const TELEMETRY_WORDS: usize = TELEMETRY_HEADER_WORDS + TELEMETRY_CHANNELS * TELEMETRY_CHANNEL_WORDS;

/// Everything module activation hands back: the source the engine plays, the two cells the
/// host writes transport requests into, and the scan the source is playing against.
struct BuiltSource {
    source: Box<dyn EventSource>,
    seek_request: Rc<Cell<SeekRequest>>,
    at_end: Rc<Cell<AtEnd>>,
    scanned: Rc<ScannedSong>,
}

const DITHER_SEED: u32 = 0x5354_4152;

/// What building one typed arm hands back: the engine, its control handle, its reader.
type BuiltEngine<Path, Interp, Out> = (Engine<Path, Interp, Out, Arc<Module>>, EngineHandle<Arc<Module>>, TelemetryReader);

fn build_engine_arm<Path, Interp, Out>(settings: EngineSettings) -> BuiltEngine<Path, Interp, Out>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let mut engine = Engine::<Path, Interp, Out, Arc<Module>>::with_settings(settings);
    let control = engine.take_control().expect("a new engine owns its control handle");
    let telemetry = engine.telemetry_reader().expect("telemetry is enabled for the web host");
    (engine, control, telemetry)
}

macro_rules! render_web_arm {
    (float, $engine:ident, $frames:ident, $float_interleaved:ident, $fixed_interleaved:ident, $channel_count:expr) => {{
        let sample_count = $frames.saturating_mul($channel_count);
        if let Some(destination) = $float_interleaved.get_mut(..sample_count) { $engine.render(destination); }
    }};
    (fixed, $engine:ident, $frames:ident, $float_interleaved:ident, $fixed_interleaved:ident, $channel_count:expr) => {{
        let sample_count = $frames.saturating_mul($channel_count);
        if let Some(destination) = $fixed_interleaved.get_mut(..sample_count) { $engine.render(destination); }
    }};
}

macro_rules! define_web_engine {
    ($($variant:ident => ($path_kind:path, $interpolator_kind:path, $channels:literal, $path:ty, $interpolator:ty, $output:ty, $buffer:ident)),+ $(,)?) => {
        enum WebEngine {
            $($variant(Engine<$path, $interpolator, $output, Arc<Module>>)),+
        }

        impl WebEngine {
            fn build(mode: MixerMode, settings: EngineSettings) -> Result<(WebEngine, EngineHandle<Arc<Module>>, TelemetryReader), String> {
                match (mode.path, mode.interpolator, mode.channels) {
                    $(($path_kind, $interpolator_kind, $channels) => {
                        let (engine, control, telemetry) = build_engine_arm::<$path, $interpolator, $output>(settings);
                        Ok((WebEngine::$variant(engine), control, telemetry))
                    },)+
                    _ => Err(String::from("the web host supports only nearest/linear mono/stereo engine arms")),
                }
            }

            fn set_source(&mut self, source: Box<dyn EventSource>) {
                match self { $(WebEngine::$variant(engine) => engine.set_source(source),)+ }
            }

            fn render_native(&mut self, frames: usize, float_interleaved: &mut [f32], fixed_interleaved: &mut [i16]) {
                match self {
                    $(WebEngine::$variant(engine) => render_web_arm!($buffer, engine, frames, float_interleaved, fixed_interleaved, $channels),)+
                }
            }

            fn frame(&self) -> Frame { match self { $(WebEngine::$variant(engine) => engine.frame(),)+ } }
            fn source_frame(&self) -> Frame { match self { $(WebEngine::$variant(engine) => engine.source_frame(),)+ } }
            fn is_playing(&self) -> bool { match self { $(WebEngine::$variant(engine) => engine.is_playing(),)+ } }
            fn master_volume(&self) -> U0F16 { match self { $(WebEngine::$variant(engine) => engine.master_volume(),)+ } }
            #[cfg(test)]
            fn warnings(&self) -> starplayer::engine::EngineWarnings { match self { $(WebEngine::$variant(engine) => engine.warnings(),)+ } }

            #[cfg(test)]
            fn module(&self) -> Option<&Arc<Module>> { match self { $(WebEngine::$variant(engine) => engine.module(),)+ } }
        }
    };
}

define_web_engine! {
    FloatNearestMono => (MixPathKind::Float, Interpolator::None, 1, FloatPath, Nearest, FloatOut<f32, 1>, float),
    FloatNearestStereo => (MixPathKind::Float, Interpolator::None, 2, FloatPath, Nearest, FloatOut<f32, 2>, float),
    FloatLinearMono => (MixPathKind::Float, Interpolator::Linear, 1, FloatPath, Linear, FloatOut<f32, 1>, float),
    FloatLinearStereo => (MixPathKind::Float, Interpolator::Linear, 2, FloatPath, Linear, FloatOut<f32, 2>, float),
    FixedNearestMono => (MixPathKind::Fixed, Interpolator::None, 1, FixedPath, Nearest, FixedOut<i16, 1>, fixed),
    FixedNearestStereo => (MixPathKind::Fixed, Interpolator::None, 2, FixedPath, Nearest, FixedOut<i16, 2>, fixed),
    FixedLinearMono => (MixPathKind::Fixed, Interpolator::Linear, 1, FixedPath, Linear, FixedOut<i16, 1>, fixed),
    FixedLinearStereo => (MixPathKind::Fixed, Interpolator::Linear, 2, FixedPath, Linear, FixedOut<i16, 2>, fixed),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SeekKind {
    #[default]
    None,
    Order(u16),
    Row(u16),
    /// An elapsed position in the song, in frames from the start of the current pass.
    Frame(u64),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SeekRequest {
    kind: SeekKind,
    frame: Frame,
}

/// A format-native tracker source with a host-owned seek mailbox.
///
/// `Engine` intentionally stores `dyn EventSource`, so its generic command handler cannot
/// downcast to `PatternSequencer`. This wrapper is the safe host-specific bridge: the
/// command remains a typed `Command`, but order/row seeks become one fixed-size mailbox
/// write. The wrapper consumes it at an event boundary and restarts the sequencer clock
/// on the engine's monotonic source timeline. No allocation occurs in `process()`.
struct SeekableModuleSource {
    sequencer: NativeSequencer,
    request: Rc<Cell<SeekRequest>>,
    /// The host's repeat setting, read at each event boundary rather than written straight
    /// into the sequencer: the page can change it while `process()` is between quanta.
    at_end: Rc<Cell<AtEnd>>,
    applied_at_end: AtEnd,
}

impl EventSource for SeekableModuleSource {
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
        let at_end = self.at_end.get();
        if at_end != self.applied_at_end {
            self.sequencer.set_at_end(at_end);
            self.applied_at_end = at_end;
        }
        let request = self.request.get();
        if !matches!(request.kind, SeekKind::None) && frame >= request.frame {
            self.request.set(SeekRequest::default());
            // An order the module does not have, or a frame past the end of the scan, is
            // a page-side mistake and not something the audio realm can report: the
            // sequencer stays where it was, which is the only answer that keeps playing.
            match request.kind {
                SeekKind::Order(order) => { let _ = self.sequencer.seek_order_at(order, frame); }
                SeekKind::Row(row) => self.sequencer.seek_row(row),
                SeekKind::Frame(song_frame) => { let _ = self.sequencer.seek_frame(song_frame, frame); }
                SeekKind::None => {}
            }
            self.sequencer.restart_clock_at(frame);
        }
        if self.sequencer.next_event_frame().is_some_and(|next| next <= frame) {
            self.sequencer.dispatch(frame, context);
        }
    }
}

fn dither_for(mode: MixerMode) -> Dither {
    if mode.dither { Dither::seeded(DITHER_SEED) } else { Dither::OFF }
}

fn quantize_float_sample(value: f32, depth: OutputDepth, dither: &mut Dither) -> (f32, Option<i16>) {
    match depth {
        OutputDepth::F32 => (<f32 as HostSample>::from_unit_f32(value, dither), None),
        OutputDepth::I32 => (<i32 as HostSample>::from_unit_f32(value, dither) as f32 * (1.0 / 2_147_483_520.0), None),
        OutputDepth::I24 => (<I24 as HostSample>::from_unit_f32(value, dither).0 as f32 * (1.0 / 8_388_607.0), None),
        OutputDepth::I16 => {
            let quantized = <i16 as HostSample>::from_unit_f32(value, dither);
            (quantized as f32 * (1.0 / 32_767.0), Some(quantized))
        }
        OutputDepth::I8 => (<i8 as HostSample>::from_unit_f32(value, dither) as f32 * (1.0 / 127.0), None),
    }
}

fn quantize_fixed_sample(value: i32, depth: OutputDepth, dither: &mut Dither) -> (f32, Option<i16>) {
    match depth {
        OutputDepth::F32 => (<f32 as HostSample>::from_i16_scale(value, dither), None),
        OutputDepth::I32 => (<i32 as HostSample>::from_i16_scale(value, dither) as f32 * (1.0 / 2_147_483_520.0), None),
        OutputDepth::I24 => (<I24 as HostSample>::from_i16_scale(value, dither).0 as f32 * (1.0 / 8_388_607.0), None),
        OutputDepth::I16 => {
            let quantized = <i16 as HostSample>::from_i16_scale(value, dither);
            (quantized as f32 * (1.0 / 32_767.0), Some(quantized))
        }
        OutputDepth::I8 => (<i8 as HostSample>::from_i16_scale(value, dither) as f32 * (1.0 / 127.0), None),
    }
}

/// Scan a module on a throwaway sequencer, so the playback one never has to be rewound.
///
/// Runs in the worklet's message task, never in `process()` — like module decoding, which
/// is the other thing here that allocates and takes milliseconds.
fn scan_for(module: &Arc<Module>, sample_rate_hz: u32) -> Result<ScannedSong, String> {
    starplayer::scan_song(module, sample_rate_hz, ScanLimits::for_rate(sample_rate_hz))
        .map_err(|_| String::from("the module format has no web-audio processor"))
}

/// Build the playback source, scanning the song first unless the caller already has the
/// scan — a mixer-mode rebuild does, because the timeline depends on the rate and the
/// module's dialect and on nothing the mixer mode chooses.
fn source_for(
    module: Arc<Module>,
    sample_rate_hz: u32,
    start: SeekKind,
    frame: Frame,
    at_end: AtEnd,
    cached_scan: Option<Rc<ScannedSong>>,
) -> Result<BuiltSource, String> {
    let scanned = match cached_scan {
        Some(scanned) => scanned,
        None => Rc::new(scan_for(&module, sample_rate_hz)?),
    };
    // The scan decides the quirks — for a MOD it is the scan, not the header, that settles
    // CIA against VBlank — and the playback sequencer must be built from exactly the set
    // the timeline was measured under. The web player still exposes no override of its
    // own: this override comes from the scan, not from the UI.
    let quirks = QuirkSelection::Override(scanned.quirks);
    let mut sequencer = NativeSequencer::new(module, sample_rate_hz, quirks)
        .map_err(|_| String::from("the module format has no web-audio processor"))?;
    sequencer.set_timeline(scanned.timeline.clone());
    sequencer.set_at_end(at_end);
    match start {
        SeekKind::Frame(song_frame) => { let _ = sequencer.seek_frame(song_frame, frame); }
        SeekKind::Order(order) => { let _ = sequencer.seek_order_at(order, frame); }
        SeekKind::Row(row) => sequencer.seek_row(row),
        SeekKind::None => { let _ = sequencer.seek_order_at(0, frame); }
    }
    sequencer.restart_clock_at(frame);
    let request = Rc::new(Cell::new(SeekRequest::default()));
    let at_end_cell = Rc::new(Cell::new(at_end));
    let source = SeekableModuleSource {
        sequencer,
        request: Rc::clone(&request),
        at_end: Rc::clone(&at_end_cell),
        applied_at_end: at_end,
    };
    Ok(BuiltSource { source: Box::new(source), seek_request: request, at_end: at_end_cell, scanned })
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
    /// The repeat setting, shared with the live source so a change lands at the next event
    /// boundary rather than inside a render.
    at_end: Rc<Cell<AtEnd>>,
    /// How long the transport fade lasts when the song reaches its loop point.
    fade_frames: u32,
    /// Whether that fade is running, so it is armed once rather than every quantum.
    fading: bool,
    /// Frames of the fade already rendered. The fade gain is computed from this position
    /// on every frame rather than accumulated, so it is exact over any length: a
    /// `GainRamp` is built for 64-frame transport clicks, and over a five-second fade its
    /// integer increment rounds to zero — the gain would hold at unity and then snap to
    /// silence, which is what shipped first.
    fade_elapsed: u32,
    current_module: Option<Arc<Module>>,
    /// The scan for `current_module` at this rate — timeline *and* the quirks it was
    /// measured under — kept so a mixer-mode rebuild reuses it. The quirks have to ride
    /// along: rebuilding a MOD's sequencer from the header dialect alone would lose the
    /// scan's CIA-versus-VBlank verdict and play the song at a different speed.
    song_scan: Option<Rc<ScannedSong>>,
    active_mode: MixerMode,
    float_interleaved: Vec<f32>,
    fixed_interleaved: Vec<i16>,
    quantized_i16: Vec<i16>,
    planar: Vec<f32>,
    telemetry_words: Box<[i32]>,
    transport_gain: GainRamp,
    pending_engine_stop: bool,
    /// Whether the queued engine stop is the end of the song rather than the Stop button,
    /// and so must rewind to frame zero once it lands.
    pending_end_rewind: bool,
    commands_rejected: u32,
    quanta_rendered: u64,
    module_generation: u32,
    retired_modules_collected: u32,
    last_peak: f32,
    dither: Dither,
}

impl Host {
    #[cfg(test)]
    fn new(sample_rate_hz: u32) -> Host {
        Host::with_mode(sample_rate_hz, MixerMode::DEFAULT).expect("the default mixer mode has a web engine arm")
    }

    fn with_mode(sample_rate_hz: u32, active_mode: MixerMode) -> Result<Host, String> {
        // The worklet builds its engine before it has seen a module and plays every module
        // through it, so it cannot ask a processor for its recommended capacity: it takes
        // the maxima. Both are allocated once, and the mixer and the telemetry view walk
        // only what is *active*, so a wider pool and a longer channel table change no
        // rendered sample — only how much memory the worklet holds.
        let settings = EngineSettings {
            sample_rate_hz,
            voice_capacity: MAX_VOICE_CAPACITY,
            channel_count: ChannelTable::MAX_CHANNELS,
            ..EngineSettings::default()
        };
        let (engine, control, telemetry) = WebEngine::build(active_mode, settings)?;
        Ok(Host {
            engine,
            sample_rate_hz,
            control,
            telemetry,
            commands: CommandRing::new(),
            seek_request: None,
            at_end: Rc::new(Cell::new(AtEnd::Continue)),
            fade_frames: DEFAULT_FADE_FRAMES,
            fading: false,
            fade_elapsed: 0,
            current_module: None,
            song_scan: None,
            active_mode,
            float_interleaved: vec![0.0; MAX_FRAMES_PER_CALL * MAX_OUTPUT_CHANNELS],
            fixed_interleaved: vec![0; MAX_FRAMES_PER_CALL * MAX_OUTPUT_CHANNELS],
            quantized_i16: vec![0; MAX_FRAMES_PER_CALL * MAX_OUTPUT_CHANNELS],
            planar: vec![0.0; MAX_FRAMES_PER_CALL * MAX_OUTPUT_CHANNELS],
            telemetry_words: vec![0; TELEMETRY_WORDS].into_boxed_slice(),
            transport_gain: GainRamp::steady(TRANSPORT_GAIN_UNITY),
            pending_engine_stop: false,
            pending_end_rewind: false,
            commands_rejected: 0,
            quanta_rendered: 0,
            module_generation: 0,
            retired_modules_collected: 0,
            last_peak: 0.0,
            dither: dither_for(active_mode),
        })
    }

    /// Decode, construct and queue a module. Called from the worklet's message handler,
    /// never from `process()`; a failed decode leaves the previous module and source live.
    fn load_module(&mut self, bytes: &[u8]) -> Result<u32, String> { self.load_module_with_options(bytes, false) }

    fn load_module_with_options(&mut self, bytes: &[u8], headphone_friendly_mod_panning: bool) -> Result<u32, String> {
        let loaded = match starplayer::probe(bytes) {
            Some(ModuleFormat::Mod) => starplayer::mod_file::load_with_options(bytes, starplayer::mod_file::LoadOptions {
                stereo_separation: starplayer::mod_file::StereoSeparation::percent(if headphone_friendly_mod_panning { 60 } else { 100 }),
            }),
            Some(ModuleFormat::S3m) => starplayer::s3m::load(bytes),
            Some(ModuleFormat::Mtm) => starplayer::mtm::load(bytes),
            _ => Err(starplayer::core::Error::BadMagic),
        }.map_err(|error| error.to_string())?;
        let module = Arc::new(loaded);
        // A new sequencer's tick clock starts at frame zero, but the engine's musical
        // clock is monotonic and has been running since `init`. Without this the first
        // tick of the new module is due thousands of frames in the past, and the engine
        // burns ticks trying to catch up until it gives up and raises
        // `zero_advance_forced`. Start the clock where the engine actually is.
        let built = source_for(Arc::clone(&module), self.sample_rate_hz, SeekKind::None, self.engine.source_frame(), self.at_end.get(), None)?;

        self.control.load_module(Arc::clone(&module)).map_err(|_| String::from("the engine command ring is full"))?;
        self.engine.set_source(built.source);
        self.seek_request = Some(built.seek_request);
        self.at_end = built.at_end;
        self.song_scan = Some(built.scanned);
        self.fading = false;
        self.pending_end_rewind = false;
        self.current_module = Some(module);
        self.module_generation = self.module_generation.wrapping_add(1).max(1);
        Ok(self.module_generation)
    }

    /// Rebuild the typed engine in a worklet message task, retaining the same module Arc
    /// and the order whose audio is currently sounding.
    fn set_mixer_mode(&mut self, mode: MixerMode) -> Result<u32, String> {
        if mode == self.active_mode {
            return Ok(mode.to_wire());
        }
        // The rebuilt engine is the same persistent engine, so it takes the same maxima.
        let settings = EngineSettings {
            sample_rate_hz: self.sample_rate_hz,
            voice_capacity: MAX_VOICE_CAPACITY,
            channel_count: ChannelTable::MAX_CHANNELS,
            ..EngineSettings::default()
        };
        let snapshot = *self.telemetry.read();
        // The sounding *song frame*, not just the order: a rebuild in the middle of a bar
        // should come back where the ear left it, and the timeline can say where that is.
        let sounding = match self.song_scan.as_ref() {
            Some(_) => SeekKind::Frame(snapshot.transport.song_frame),
            None => SeekKind::Order(snapshot.transport.order),
        };
        let was_playing = self.engine.is_playing() && !self.pending_engine_stop;
        let master_volume = self.engine.master_volume();
        let (mut engine, mut control, telemetry) = WebEngine::build(mode, settings)?;
        let mut seek_request = None;

        let mut at_end_cell = Rc::clone(&self.at_end);
        if let Some(module) = self.current_module.as_ref() {
            let built = source_for(
                Arc::clone(module),
                self.sample_rate_hz,
                sounding,
                engine.source_frame(),
                self.at_end.get(),
                self.song_scan.clone(),
            )?;
            control.load_module(Arc::clone(module)).map_err(|_| String::from("the rebuilt engine command ring is full"))?;
            engine.set_source(built.source);
            seek_request = Some(built.seek_request);
            at_end_cell = built.at_end;
            self.song_scan = Some(built.scanned);
        }
        control.send(Command::SetMasterVolume(master_volume)).map_err(|_| String::from("the rebuilt engine command ring is full"))?;
        for (channel_index, channel) in snapshot.channels.iter().enumerate() {
            if channel.muted {
                let command = Command::MuteChannel { channel: ChannelId(channel_index as u16), muted: true };
                control.send(command).map_err(|_| String::from("the rebuilt engine command ring is full"))?;
            }
        }
        if !was_playing {
            control.send(Command::Stop).map_err(|_| String::from("the rebuilt engine command ring is full"))?;
        }

        self.engine = engine;
        self.control = control;
        self.telemetry = telemetry;
        self.seek_request = seek_request;
        self.at_end = at_end_cell;
        self.fading = false;
        self.active_mode = mode;
        self.dither = dither_for(mode);
        self.transport_gain = GainRamp::steady(if was_playing { TRANSPORT_GAIN_UNITY } else { 0 });
        self.pending_engine_stop = false;
        self.pending_end_rewind = false;
        Ok(mode.to_wire())
    }

    fn enqueue(&mut self, command: WireCommand) -> bool { self.commands.push(command) }

    fn decode(command: WireCommand) -> Option<Command<Arc<Module>>> {
        match command.opcode {
            OPCODE_PLAY => Some(Command::Play),
            OPCODE_STOP => Some(Command::Stop),
            OPCODE_SEEK_ORDER => Some(Command::SeekOrder(command.argument as u16)),
            OPCODE_SEEK_ROW => Some(Command::SeekRow(command.argument as u16)),
            OPCODE_SEEK_FRAME => Some(Command::SeekFrame(command.argument as u64)),
            OPCODE_AT_END => match command.argument {
                AT_END_FADE_OUT => Some(Command::SetAtEnd(AtEnd::FadeOut)),
                AT_END_CONTINUE => Some(Command::SetAtEnd(AtEnd::Continue)),
                AT_END_STOP => Some(Command::SetAtEnd(AtEnd::Stop)),
                _ => None,
            },
            OPCODE_MASTER_VOLUME => Some(Command::SetMasterVolume(U0F16::from_bits(command.argument as u16))),
            OPCODE_MUTE_CHANNEL => Some(Command::MuteChannel {
                channel: ChannelId(command.argument as u16),
                muted: command.extra != 0,
            }),
            // This opcode is handled synchronously by the worklet message handler; it
            // must never reach the render-path staging ring.
            OPCODE_SET_MIXER_MODE => None,
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
            // `Command` has no room for the fade length, so the wire record's `extra` is
            // read here rather than being folded into the typed command.
            if let Command::SetAtEnd(at_end) = command {
                self.set_at_end(at_end, wire.extra);
                continue;
            }
            match command {
                // Fade first; the typed engine stop is queued when the ramp reaches zero.
                Command::Stop => self.begin_transport_stop(),
                Command::Play => {
                    self.pending_engine_stop = false;
                    // Play during a fade-out, or after the song ran out of order list,
                    // means "again", not "louder": the song is over, so it goes back to the
                    // top, which also clears the sticky `end_reached` the fade or the stop
                    // was armed from.
                    if self.fading || self.pending_end_rewind {
                        self.fading = false;
                        self.pending_end_rewind = false;
                        self.request_seek(SeekKind::Frame(0));
                    }
                    self.transport_gain.glide_to(TRANSPORT_GAIN_UNITY, TRANSPORT_RAMP_FRAMES);
                    if self.control.send(Command::Play).is_err() {
                        self.commands_rejected = self.commands_rejected.saturating_add(1);
                    }
                }
                // The dyn source boundary cannot safely downcast. Route only these two
                // typed variants through the wrapper's fixed mailbox.
                Command::SeekOrder(order) => self.request_seek(SeekKind::Order(order)),
                Command::SeekRow(row) => self.request_seek(SeekKind::Row(row)),
                Command::SeekFrame(song_frame) => self.request_seek(SeekKind::Frame(song_frame)),
                other => {
                    if self.control.send(other).is_err() {
                        self.commands_rejected = self.commands_rejected.saturating_add(1);
                    }
                }
            }
        }
    }

    /// Ramp the transport down over the usual 64 frames and queue the typed engine stop for
    /// when the ramp lands.
    ///
    /// The Stop button's whole body, shared with the end of a song so the two stop
    /// identically — click-free, and through one code path rather than two.
    fn begin_transport_stop(&mut self) {
        self.transport_gain.glide_to(0, TRANSPORT_RAMP_FRAMES);
        self.pending_engine_stop = true;
    }

    fn request_seek(&mut self, kind: SeekKind) {
        if let Some(request) = &self.seek_request {
            request.set(SeekRequest { kind, frame: self.engine.source_frame() });
        } else {
            self.commands_rejected = self.commands_rejected.saturating_add(1);
        }
    }

    /// Set the repeat behaviour, and its fade length when one was named.
    ///
    /// Choosing `Continue` while the transport is already fading takes the fade back: the
    /// gain glides home and the queued stop is cancelled, so the repeat button is a toggle
    /// rather than a one-way door.
    fn set_at_end(&mut self, at_end: AtEnd, fade_frames: u32) {
        if fade_frames > 0 {
            self.fade_frames = fade_frames;
        }
        self.at_end.set(at_end);
        if at_end != AtEnd::FadeOut && self.fading {
            // Take the fade back from where it has got to, not from unity: the transport
            // ramp picks up at the faded level and glides home over the usual 64 frames.
            let faded = self.faded_gain(self.transport_gain.current());
            self.fading = false;
            self.transport_gain = GainRamp::steady(faded);
            self.transport_gain.glide_to(TRANSPORT_GAIN_UNITY, TRANSPORT_RAMP_FRAMES);
        }
    }

    /// `gain` scaled by where the song fade has got to: untouched before the fade starts,
    /// zero on its last frame, linear in between. Position-based, so it is exact whatever
    /// the fade length and however the host blocks fall.
    fn faded_gain(&self, gain: i32) -> i32 {
        if !self.fading || self.fade_frames == 0 {
            return gain;
        }
        let remaining = self.fade_frames.saturating_sub(self.fade_elapsed) as i64;
        (gain as i64 * remaining / self.fade_frames as i64) as i32
    }

    fn process(&mut self, frames: usize) -> f32 {
        self.drain_commands();
        let frames = frames.min(MAX_FRAMES_PER_CALL);
        self.engine.render_native(frames, &mut self.float_interleaved, &mut self.fixed_interleaved);

        // The song has been heard through once. What that means depends on *how* it ends
        // (task D2), and only the two cases below do anything at all:
        //
        // * it **loops** — something in the module jumps back — and the page asked for a
        //   fade, so the second pass plays under a fading transport. The ramp starts up to
        //   one host block (at most 1024 frames, ~21 ms at 48 kHz) after the loop point;
        //   for a fade measured in seconds that is inaudible, and it keeps the arming
        //   decision out of the per-frame loop.
        // * it **ends** — the order list ran out, or a stop marker fired — and the page
        //   asked for anything but Repeat. There is no second pass to fade into and the
        //   sequencer has already stopped itself on the end frame, so the transport stops
        //   the way the Stop button stops it and rewinds for the next Play. (Before D2 this
        //   case armed the fade too, and an `F00` ended in ten seconds of silence.)
        //
        // Only while the transport is actually running: once the fade or the stop has
        // landed and the engine has stopped, no tick publishes again until the next Play
        // consumes the rewind, so the snapshot keeps saying `end_reached` and would
        // otherwise re-arm over silence — and leave `fading` stuck for the next real loop
        // point.
        let snapshot = *self.telemetry.read();
        let transport_running = self.engine.is_playing() && !self.pending_engine_stop;
        if transport_running && snapshot.transport.end_reached && !self.fading {
            let loops = snapshot.transport.song_end == SongEnd::Loops;
            match self.at_end.get() {
                AtEnd::FadeOut if loops => {
                    self.fading = true;
                    self.fade_elapsed = 0;
                }
                AtEnd::FadeOut | AtEnd::Stop if !loops => {
                    self.begin_transport_stop();
                    self.pending_end_rewind = true;
                }
                _ => {}
            }
        }

        let mut peak = 0.0f32;
        for frame in 0..frames {
            let transport_units = self.transport_gain.advance();
            let gain_units = self.faded_gain(transport_units);
            if self.fading {
                self.fade_elapsed = self.fade_elapsed.saturating_add(1);
            }
            for channel in 0..self.active_mode.channels as usize {
                let interleaved_index = frame.saturating_mul(self.active_mode.channels as usize).saturating_add(channel);
                let (output, quantized_i16) = match self.active_mode.path {
                    MixPathKind::Float => {
                        let native = self.float_interleaved.get(interleaved_index).copied().unwrap_or(0.0);
                        let gained = native * gain_units as f32 / TRANSPORT_GAIN_UNITY as f32;
                        quantize_float_sample(gained, self.active_mode.depth, &mut self.dither)
                    }
                    MixPathKind::Fixed => {
                        let native = self.fixed_interleaved.get(interleaved_index).copied().unwrap_or(0) as i32;
                        let gained = ((native as i64 * gain_units as i64) / TRANSPORT_GAIN_UNITY as i64) as i32;
                        quantize_fixed_sample(gained, self.active_mode.depth, &mut self.dither)
                    }
                };
                if let Some(quantized) = quantized_i16
                    && let Some(destination) = self.quantized_i16.get_mut(interleaved_index)
                {
                    *destination = quantized;
                }
                let planar_index = channel.saturating_mul(MAX_FRAMES_PER_CALL).saturating_add(frame);
                if let Some(destination) = self.planar.get_mut(planar_index) { *destination = output; }
                peak = peak.max(output.abs());
            }
        }

        // The fade has run its course. A song that faded out is over, not paused: stop the
        // engine and rewind it so the next Play starts from the top rather than from the
        // silence at the end. The transport gain itself was never touched, so Play needs
        // no ramp back up.
        if self.fading && self.fade_elapsed >= self.fade_frames {
            if self.control.send(Command::Stop).is_ok() {
                self.fading = false;
                self.request_seek(SeekKind::Frame(0));
            } else {
                self.commands_rejected = self.commands_rejected.saturating_add(1);
            }
        }

        if self.pending_engine_stop && !self.transport_gain.is_ramping() {
            if self.control.send(Command::Stop).is_ok() {
                self.pending_engine_stop = false;
                // A song that stopped because it *ended* is over, not paused: rewind it so
                // the next Play starts from the top. A Stop the user asked for keeps its
                // place, which is why this rides its own flag.
                if core::mem::take(&mut self.pending_end_rewind) {
                    self.request_seek(SeekKind::Frame(0));
                }
            } else {
                self.commands_rejected = self.commands_rejected.saturating_add(1);
            }
        }

        self.quanta_rendered = self.quanta_rendered.wrapping_add(1);
        // Peak-hold with decay, as the original walked `_VUBarLevel` down every tick: a
        // bare 2.7 ms quantum peak flickers, and a quantum that lands between transients
        // reads as silence.
        self.last_peak = peak.max(self.last_peak * MASTER_PEAK_DECAY_PER_QUANTUM);
        self.pack_telemetry(snapshot);
        peak
    }

    fn pack_telemetry(&mut self, snapshot: Snapshot) {
        let song_flags = ((snapshot.transport.song_end != SongEnd::Unknown) as i32)
            | (((snapshot.transport.song_end == SongEnd::Loops) as i32) << 1)
            | ((snapshot.transport.end_reached as i32) << 2)
            | ((self.fading as i32) << 3);
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
            self.active_mode.to_wire() as i32,
            snapshot.transport.song_frame.min(i32::MAX as u64) as i32,
            snapshot.transport.song_length_frames.min(i32::MAX as u64) as i32,
            song_flags,
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
    pub fn init(sample_rate: f32, mixer_mode_wire: u32) -> bool {
        drop(core::hint::black_box(vec![0u8; HEAP_RESERVE_BYTES]));
        let sample_rate_hz = sample_rate.round().clamp(8_000.0, 384_000.0) as u32;
        let mode = MixerMode::from_wire(mixer_mode_wire).unwrap_or(MixerMode::DEFAULT);
        let mut initialized = false;
        HOST.with(|cell| {
            if let Ok(mut slot) = cell.try_borrow_mut()
                && let Ok(host) = Host::with_mode(sample_rate_hz, mode)
            {
                *slot = Some(host);
                initialized = true;
            }
        });
        initialized
    }

    /// Decode and activate one validated native module byte buffer outside `process()`.
    #[wasm_bindgen]
    pub fn load_module(bytes: &[u8]) -> Result<u32, JsValue> {
        HOST.with(|cell| {
            let mut slot = cell.try_borrow_mut().map_err(|_| JsValue::from_str("the worklet host is busy"))?;
            let host = slot.as_mut().ok_or_else(|| JsValue::from_str("the worklet host is not initialized"))?;
            host.load_module(bytes).map_err(|message| JsValue::from_str(&message))
        })
    }

    /// Decode and activate a module with web-player loading preferences. The option is
    /// deliberately MOD-specific; S3M and MTM continue through their native loaders.
    #[wasm_bindgen]
    pub fn load_module_with_options(bytes: &[u8], headphone_friendly_mod_panning: bool) -> Result<u32, JsValue> {
        HOST.with(|cell| {
            let mut slot = cell.try_borrow_mut().map_err(|_| JsValue::from_str("the worklet host is busy"))?;
            let host = slot.as_mut().ok_or_else(|| JsValue::from_str("the worklet host is not initialized"))?;
            host.load_module_with_options(bytes, headphone_friendly_mod_panning).map_err(|message| JsValue::from_str(&message))
        })
    }

    /// Decode one SAB/fallback wire record into the wasm-side fixed command ring.
    #[wasm_bindgen]
    pub fn enqueue_command(opcode: u32, argument: u32, extra: u32) -> bool {
        let command = WireCommand { opcode: opcode as u8, argument, extra };
        with_host(false, |host| host.enqueue(command))
    }

    /// Rebuild the selected typed engine outside `process()`.
    #[wasm_bindgen]
    pub fn set_mixer_mode(wire: u32) -> Result<u32, JsValue> {
        let mode = MixerMode::from_wire(wire).ok_or_else(|| JsValue::from_str("invalid mixer mode wire value"))?;
        HOST.with(|cell| {
            let mut slot = cell.try_borrow_mut().map_err(|_| JsValue::from_str("the worklet host is busy"))?;
            let host = slot.as_mut().ok_or_else(|| JsValue::from_str("the worklet host is not initialized"))?;
            host.set_mixer_mode(mode).map_err(|message| JsValue::from_str(&message))
        })
    }

    #[wasm_bindgen]
    pub fn process(frames: u32) -> f32 { with_host(0.0, |host| host.process(frames as usize)) }

    #[wasm_bindgen]
    pub fn output_ptr() -> u32 { with_host(0, |host| host.planar.as_ptr() as usize as u32) }

    #[wasm_bindgen]
    pub fn output_len() -> u32 { (MAX_FRAMES_PER_CALL * MAX_OUTPUT_CHANNELS) as u32 }

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
    use std::collections::BTreeSet;

    const FIXTURE: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");

    fn minimal_mod() -> Vec<u8> {
        const SAMPLE_FRAMES: usize = 256;
        let mut bytes = vec![0; 1084 + 64 * 4 * 4 + SAMPLE_FRAMES];
        bytes[..10].copy_from_slice(b"native mod");
        bytes[42..44].copy_from_slice(&((SAMPLE_FRAMES / 2) as u16).to_be_bytes());
        bytes[45] = 64;
        bytes[950] = 1;
        bytes[1080..1084].copy_from_slice(b"M.K.");
        bytes[1084..1088].copy_from_slice(&starplayer::mod_file::ModCell { period: 428, instrument: 1, effect: 0, param: 0 }.to_bytes());
        let sample_offset = 1084 + 64 * 4 * 4;
        for (index, byte) in bytes[sample_offset..].iter_mut().enumerate() { *byte = if index & 1 == 0 { 0x7F } else { 0x80 }; }
        bytes
    }

    /// [`minimal_mod`] with a `B00` on its very last row, so the song genuinely loops
    /// rather than merely running out of order list — which task D2 reads as an end.
    fn looping_mod() -> Vec<u8> {
        let mut bytes = minimal_mod();
        let last_cell = 1084 + 63 * 4 * 4;
        let jump = starplayer::mod_file::ModCell { period: 0, instrument: 0, effect: 0xB, param: 0x00 };
        bytes[last_cell..last_cell + 4].copy_from_slice(&jump.to_bytes());
        bytes
    }

    fn minimal_mtm() -> Vec<u8> {
        const SAMPLE_FRAMES: usize = 256;
        const SAMPLE_HEADER: usize = 66;
        const ORDER_TABLE: usize = SAMPLE_HEADER + 37;
        const TRACK_DATA: usize = ORDER_TABLE + 128;
        const PATTERN_TABLE: usize = TRACK_DATA + 192;
        const SAMPLE_DATA: usize = PATTERN_TABLE + 64;
        let mut bytes = vec![0; SAMPLE_DATA + SAMPLE_FRAMES];
        bytes[..4].copy_from_slice(b"MTM\x10");
        bytes[4..14].copy_from_slice(b"native mtm");
        bytes[24..26].copy_from_slice(&1u16.to_le_bytes());
        bytes[30] = 1;
        bytes[32] = 64;
        bytes[33] = 1;
        bytes[34] = 8;
        bytes[SAMPLE_HEADER..SAMPLE_HEADER + 6].copy_from_slice(b"sample");
        bytes[SAMPLE_HEADER + 22..SAMPLE_HEADER + 26].copy_from_slice(&(SAMPLE_FRAMES as u32).to_le_bytes());
        bytes[SAMPLE_HEADER + 35] = 64;
        bytes[TRACK_DATA..TRACK_DATA + 3].copy_from_slice(&starplayer::mtm::MtmCell { pitch: 12, instrument: 1, effect: 0, param: 0 }.to_bytes());
        bytes[PATTERN_TABLE..PATTERN_TABLE + 2].copy_from_slice(&1u16.to_le_bytes());
        for (index, byte) in bytes[SAMPLE_DATA..].iter_mut().enumerate() { *byte = if index & 1 == 0 { 0xFF } else { 0x00 }; }
        bytes
    }

    fn mode(path: MixPathKind, interpolator: Interpolator, depth: OutputDepth, dither: bool, channels: u8) -> MixerMode {
        MixerMode { path, interpolator, depth, dither, channels }
    }

    fn rendered_planar(mode: MixerMode, quanta: usize) -> Vec<f32> {
        let mut host = Host::with_mode(48_000, mode).expect("test mode has an engine arm");
        assert!(host.load_module(FIXTURE).is_ok());
        let mut output = Vec::with_capacity(quanta * RENDER_QUANTUM * mode.channels as usize);
        for _ in 0..quanta {
            host.process(RENDER_QUANTUM);
            for channel in 0..mode.channels as usize {
                let first = channel * MAX_FRAMES_PER_CALL;
                output.extend_from_slice(&host.planar[first..first + RENDER_QUANTUM]);
            }
        }
        output
    }

    #[test]
    fn all_eight_typed_engine_arms_build_and_render_a_quantum() {
        for path in [MixPathKind::Float, MixPathKind::Fixed] {
            for interpolator in [Interpolator::None, Interpolator::Linear] {
                for channels in [1, 2] {
                    let mode = mode(path, interpolator, OutputDepth::F32, false, channels);
                    let mut host = Host::with_mode(48_000, mode).expect("the documented arm exists");
                    assert!(host.load_module(FIXTURE).is_ok());
                    host.process(RENDER_QUANTUM);
                    assert_eq!(host.active_mode, mode);
                    assert_eq!(host.telemetry_words[18] as u32, mode.to_wire());
                }
            }
        }
    }

    #[test]
    fn switching_mode_keeps_the_sounding_order_generation_and_same_module_arc() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        host.process(RENDER_QUANTUM);
        assert!(host.enqueue(WireCommand { opcode: OPCODE_SEEK_ORDER, argument: 1, extra: 0 }));
        for _ in 0..10 { host.process(RENDER_QUANTUM); }
        assert_eq!(host.telemetry.read().transport.order, 1);
        let generation = host.module_generation;
        let retired = host.retired_modules_collected;
        let module = Arc::clone(host.current_module.as_ref().expect("the host retains its module"));

        let retro = mode(MixPathKind::Fixed, Interpolator::None, OutputDepth::I8, false, 2);
        assert_eq!(host.set_mixer_mode(retro), Ok(retro.to_wire()));
        for _ in 0..10 { host.process(RENDER_QUANTUM); }

        assert_eq!(host.telemetry.read().transport.order, 1, "the rebuilt sequencer seeks to the sounding order");
        assert!(host.telemetry.read().transport.song_frame > 0, "and to the song frame that was sounding, not to the top");
        assert_eq!(host.module_generation, generation, "a mode switch is not a module reload");
        assert_eq!(host.retired_modules_collected, retired, "the same Arc is not retired through the audio channel");
        assert_eq!(host.collect_garbage(), 0, "no module was retired by the switch");
        assert!(Arc::ptr_eq(host.current_module.as_ref().expect("module retained"), &module));
        assert!(Arc::ptr_eq(host.engine.module().expect("new engine loaded the module"), &module));
    }

    #[test]
    fn fixed_i16_post_quantisation_is_bit_exact_with_the_engine_output() {
        let fixed_i16 = mode(MixPathKind::Fixed, Interpolator::Linear, OutputDepth::I16, true, 2);
        let mut host = Host::with_mode(48_000, fixed_i16).expect("fixed stereo arm exists");
        assert!(host.load_module(FIXTURE).is_ok());
        let mut heard = false;
        for _ in 0..200 {
            host.process(RENDER_QUANTUM);
            let native = &host.fixed_interleaved[..RENDER_QUANTUM * 2];
            let quantized = &host.quantized_i16[..RENDER_QUANTUM * 2];
            assert_eq!(quantized, native, "fixed I16 uses the engine's native samples unchanged");
            heard |= native.iter().any(|sample| *sample != 0);
        }
        assert!(heard, "the comparison covered non-silent output");
    }

    /// The fixture opens quietly, so a short render only reaches a couple of dozen codes.
    /// Two thousand quanta reach a peak of about 0.39 full scale, where the same passage at
    /// full depth takes thousands of distinct values and the 8-bit one still cannot.
    #[test]
    fn i8_output_uses_no_more_than_256_values() {
        let quanta = 2_000;
        let eight_bit = rendered_planar(mode(MixPathKind::Float, Interpolator::Linear, OutputDepth::I8, false, 2), quanta);
        let values: BTreeSet<u32> = eight_bit.iter().map(|sample| sample.to_bits()).collect();
        assert!(values.len() <= 256, "8-bit output produced {} distinct values", values.len());
        assert!(values.len() > 1, "the fixture produced more than one 8-bit value");
        assert!(eight_bit.iter().any(|sample| *sample != 0.0), "the comparison covered non-silent output");

        let full_depth = rendered_planar(mode(MixPathKind::Float, Interpolator::Linear, OutputDepth::F32, false, 2), quanta);
        let full_values: BTreeSet<u32> = full_depth.iter().map(|sample| sample.to_bits()).collect();
        assert!(full_values.len() > 256, "the same passage at full depth takes only {} distinct values", full_values.len());
    }

    #[test]
    fn dither_changes_output_and_is_deterministic() {
        let dithered_mode = mode(MixPathKind::Float, Interpolator::Linear, OutputDepth::I8, true, 2);
        let first = rendered_planar(dithered_mode, 100);
        let second = rendered_planar(dithered_mode, 100);
        let undithered = rendered_planar(MixerMode { dither: false, ..dithered_mode }, 100);
        assert_eq!(first, second, "the seeded TPDF stream repeats exactly");
        assert_ne!(first, undithered, "enabling dither changes reduced-depth output");
    }

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
    fn a_mod_uses_the_native_processor_and_reaches_the_real_engine() {
        let mut host = Host::new(48_000);
        assert_eq!(host.load_module(&minimal_mod()), Ok(1));
        assert_eq!(host.current_module.as_ref().map(|module| module.header().format), Some(ModuleFormat::Mod));
        assert!((0..20).any(|_| host.process(RENDER_QUANTUM) > 0.0), "the MOD sequencer should trigger sample audio");
        assert!(host.telemetry.read().sequence > 0);
    }

    #[test]
    fn mod_headphone_option_changes_only_mod_initial_panning() {
        let mut host = Host::new(48_000);
        let mod_bytes = minimal_mod();
        assert_eq!(host.load_module_with_options(&mod_bytes, false), Ok(1));
        let hard: Vec<i16> = host.current_module.as_ref().expect("MOD retained").header().default_pan.iter().map(|pan| pan.to_bits()).collect();
        assert_eq!(hard, [-32_767, 32_767, 32_767, -32_767]);
        assert_eq!(host.load_module_with_options(&mod_bytes, true), Ok(2));
        let headphone: Vec<i16> = host.current_module.as_ref().expect("MOD retained").header().default_pan.iter().map(|pan| pan.to_bits()).collect();
        assert_eq!(headphone, [-19_660, 19_660, 19_660, -19_660]);

        assert_eq!(host.load_module_with_options(FIXTURE, false), Ok(3));
        let s3m_authentic = host.current_module.as_ref().expect("S3M retained").header().default_pan.to_vec();
        assert_eq!(host.load_module_with_options(FIXTURE, true), Ok(4));
        assert_eq!(host.current_module.as_ref().expect("S3M retained").header().default_pan.as_ref(), s3m_authentic.as_slice());

        let mtm_bytes = minimal_mtm();
        assert_eq!(host.load_module_with_options(&mtm_bytes, false), Ok(5));
        let mtm_authentic = host.current_module.as_ref().expect("MTM retained").header().default_pan.to_vec();
        assert_eq!(host.load_module_with_options(&mtm_bytes, true), Ok(6));
        assert_eq!(host.current_module.as_ref().expect("MTM retained").header().default_pan.as_ref(), mtm_authentic.as_slice());
    }

    #[test]
    fn an_mtm_uses_the_native_processor_and_reaches_the_real_engine() {
        let mut host = Host::new(48_000);
        assert_eq!(host.load_module(&minimal_mtm()), Ok(1));
        assert_eq!(host.current_module.as_ref().map(|module| module.header().format), Some(ModuleFormat::Mtm));
        assert!((0..20).any(|_| host.process(RENDER_QUANTUM) > 0.0), "the MTM sequencer should trigger sample audio");
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
    fn a_loaded_module_is_scanned_and_its_length_reaches_the_telemetry() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        let scanned = Rc::clone(host.song_scan.as_ref().expect("activation scans the module"));
        let timeline = &scanned.timeline;
        assert!(timeline.end_frame() > 48_000, "REFLEX is longer than a second");
        // REFLEX's order list runs out; nothing in it jumps backwards (task D2).
        assert_eq!(timeline.end(), starplayer::engine::EndReason::Ended, "REFLEX ends");

        host.process(RENDER_QUANTUM);
        assert_eq!(host.telemetry_words[20] as u64, timeline.end_frame(), "the song length rides in word 20");
        assert_eq!(host.telemetry_words[21] & 0b11, 0b01, "length known, and the song does not end by looping");
        assert_eq!(host.telemetry_words[21] & 0b1000, 0, "nothing is fading");

        // …and a module that really loops says so, which is the bit the page's "add the
        // fade to the displayed length" decision keys on.
        let mut host = Host::new(48_000);
        assert!(host.load_module(&looping_mod()).is_ok());
        host.process(RENDER_QUANTUM);
        assert_eq!(host.telemetry_words[21] & 0b11, 0b11, "length known, and the song ends by looping");
    }

    #[test]
    fn a_frame_seek_moves_the_song_clock_through_the_mailbox() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        for _ in 0..10 { host.process(RENDER_QUANTUM); }

        let scanned = Rc::clone(host.song_scan.as_ref().expect("the module was scanned"));
        let timeline = &scanned.timeline;
        let target = timeline.end_frame() / 2;
        let expected = *timeline.mark_at_frame(target).expect("a frame inside the song resolves");
        assert!(host.enqueue(WireCommand { opcode: OPCODE_SEEK_FRAME, argument: target as u32, extra: 0 }));
        for _ in 0..10 { host.process(RENDER_QUANTUM); }

        let snapshot = *host.telemetry.read();
        assert_eq!(snapshot.transport.order, expected.order, "the seek landed on the scanned order");
        assert!(snapshot.transport.song_frame >= expected.frame, "elapsed picks up where the scan says that row is");
        assert!(snapshot.transport.song_frame < expected.frame + 48_000, "and not somewhere else entirely");
        assert!(!host.engine.warnings().unsupported_command, "a frame seek is routed, not flagged");
        assert_eq!(host.telemetry_words[19] as u64, snapshot.transport.song_frame, "the elapsed frame rides in word 19");
    }

    #[test]
    fn the_at_end_opcode_carries_the_mode_and_its_fade_length() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        assert_eq!(host.at_end.get(), AtEnd::Continue, "repeat is on by default");

        assert!(host.enqueue(WireCommand { opcode: OPCODE_AT_END, argument: AT_END_FADE_OUT, extra: 4_096 }));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.at_end.get(), AtEnd::FadeOut);
        assert_eq!(host.fade_frames, 4_096);

        assert!(host.enqueue(WireCommand { opcode: OPCODE_AT_END, argument: AT_END_STOP, extra: 0 }));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.at_end.get(), AtEnd::Stop);
        assert_eq!(host.fade_frames, 4_096, "an unnamed fade length leaves the previous one alone");

        let rejected = host.dropped_commands();
        assert!(host.enqueue(WireCommand { opcode: OPCODE_AT_END, argument: 99, extra: 0 }));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.at_end.get(), AtEnd::Stop, "an unknown mode is rejected rather than guessed");
        assert_eq!(host.dropped_commands(), rejected + 1);
    }

    #[test]
    fn a_song_that_reaches_its_loop_point_under_fade_out_ramps_down_and_rewinds() {
        let module = looping_mod();
        let mut host = Host::new(48_000);
        assert!(host.load_module(&module).is_ok());
        let end_frame = host.song_scan.as_ref().expect("the module was scanned").timeline.end_frame();
        assert!(host.enqueue(WireCommand { opcode: OPCODE_AT_END, argument: AT_END_FADE_OUT, extra: 4_096 }));
        assert!(host.enqueue(WireCommand { opcode: OPCODE_PLAY, argument: 0, extra: 0 }));

        let mut fade_seen = false;
        let mut stopped_at = None;
        for quantum in 0..(end_frame as usize / RENDER_QUANTUM + 200) {
            host.process(RENDER_QUANTUM);
            fade_seen |= host.telemetry_words[21] & 0b1000 != 0;
            if !host.engine.is_playing() {
                stopped_at = Some(quantum);
                break;
            }
        }
        assert!(fade_seen, "the fade was armed and reported");
        let stopped_at = stopped_at.expect("the transport stopped once the fade finished");
        assert!(stopped_at as u64 * RENDER_QUANTUM as u64 >= end_frame, "it did not stop before the loop point");
        assert!(!host.fading, "the fade is finished, not stuck");
        assert_eq!(host.seek_request.as_ref().map(|request| request.get().kind), Some(SeekKind::Frame(0)), "a faded-out song rewinds for the next Play");
    }

    /// Task D2, the owner's case: with Repeat off, a song whose order list simply runs out
    /// stops at its end frame instead of playing on under a five-second fade.
    #[test]
    fn a_song_that_runs_out_of_order_list_stops_at_its_end_instead_of_fading() {
        let module = minimal_mod();
        let mut host = Host::new(48_000);
        assert!(host.load_module(&module).is_ok());
        let end_frame = host.song_scan.as_ref().expect("the module was scanned").timeline.end_frame();
        assert_eq!(host.song_scan.as_ref().expect("scanned").timeline.end(), starplayer::engine::EndReason::Ended);
        assert!(host.enqueue(WireCommand { opcode: OPCODE_AT_END, argument: AT_END_FADE_OUT, extra: 48_000 * 5 }));
        assert!(host.enqueue(WireCommand { opcode: OPCODE_PLAY, argument: 0, extra: 0 }));

        let mut fade_seen = false;
        let mut stopped_at = None;
        for quantum in 0..(end_frame as usize / RENDER_QUANTUM + 200) {
            host.process(RENDER_QUANTUM);
            fade_seen |= host.telemetry_words[21] & 0b1000 != 0;
            if !host.engine.is_playing() {
                stopped_at = Some(quantum);
                break;
            }
        }
        assert!(!fade_seen, "a song that ends has nothing to fade into");
        assert!(!host.fading);
        let stopped_at = stopped_at.expect("the transport stopped");
        let stopped_frame = stopped_at as u64 * RENDER_QUANTUM as u64;
        assert!(stopped_frame >= end_frame, "it did not stop before the end of the song");
        // The 64-frame transport glide plus the quantum the arming decision is taken in.
        assert!(stopped_frame < end_frame + 4 * RENDER_QUANTUM as u64, "and it stopped there, not five seconds later: {stopped_frame} vs {end_frame}");
        assert_eq!(host.seek_request.as_ref().map(|request| request.get().kind), Some(SeekKind::Frame(0)), "a song that ended rewinds for the next Play");

        // A stopped engine whose snapshot still says `end_reached` must not stop again.
        let rejected = host.dropped_commands();
        for _ in 0..20 { host.process(RENDER_QUANTUM); }
        assert!(!host.pending_engine_stop);
        assert!(!host.pending_end_rewind);
        assert_eq!(host.dropped_commands(), rejected, "nothing re-armed over the silence");

        // Play after an end-stop is "again", not "resume": the pending rewind is consumed,
        // the transport gain glides back to unity and the sticky end flag clears.
        assert!(host.enqueue(WireCommand { opcode: OPCODE_PLAY, argument: 0, extra: 0 }));
        for _ in 0..8 { host.process(RENDER_QUANTUM); }
        assert!(host.engine.is_playing(), "the song plays again");
        assert_eq!(host.transport_gain.target(), TRANSPORT_GAIN_UNITY, "at unity, not at the faded-out level");
        assert_eq!(host.seek_request.as_ref().map(|request| request.get().kind), Some(SeekKind::None), "the rewind was consumed");
        assert_eq!(host.telemetry_words[21] & 0b100, 0, "and the sticky end flag went with it");
        assert!((host.telemetry_words[19] as u64) < end_frame / 4, "it restarted from the top, not from the end");
    }

    #[test]
    fn a_faded_out_song_fades_again_on_its_next_pass() {
        let module = looping_mod();
        let mut host = Host::new(48_000);
        assert!(host.load_module(&module).is_ok());
        let end_frame = host.song_scan.as_ref().expect("the module was scanned").timeline.end_frame();
        assert!(host.enqueue(WireCommand { opcode: OPCODE_AT_END, argument: AT_END_FADE_OUT, extra: 4_096 }));
        let budget = end_frame as usize / RENDER_QUANTUM + 200;

        let play_through = |host: &mut Host| {
            assert!(host.enqueue(WireCommand { opcode: OPCODE_PLAY, argument: 0, extra: 0 }));
            let mut fade_seen = false;
            for _ in 0..budget {
                host.process(RENDER_QUANTUM);
                fade_seen |= host.telemetry_words[21] & 0b1000 != 0;
                if !host.engine.is_playing() { break; }
            }
            assert!(!host.engine.is_playing(), "the fade landed and the transport stopped");
            fade_seen
        };

        assert!(play_through(&mut host), "the first pass fades");
        // Stopped, with the snapshot still saying the end was reached: nothing may re-arm.
        for _ in 0..20 { host.process(RENDER_QUANTUM); }
        assert!(!host.fading, "a stopped transport does not fade over silence");
        assert!(!host.pending_engine_stop);
        assert_eq!(host.telemetry_words[21] & 0b1000, 0, "and does not report a fade");

        assert!(play_through(&mut host), "the second pass fades again from the top");
        assert!(!host.fading);
    }

    #[test]
    fn choosing_continue_while_a_fade_runs_takes_the_fade_back() {
        let mut host = Host::new(48_000);
        assert!(host.load_module(FIXTURE).is_ok());
        host.at_end.set(AtEnd::FadeOut);
        host.fading = true;
        host.fade_elapsed = host.fade_frames / 2;
        assert_eq!(host.faded_gain(TRANSPORT_GAIN_UNITY), TRANSPORT_GAIN_UNITY / 2, "half way through, the fade is at half gain");

        assert!(host.enqueue(WireCommand { opcode: OPCODE_AT_END, argument: AT_END_CONTINUE, extra: 0 }));
        host.process(RENDER_QUANTUM);
        assert_eq!(host.at_end.get(), AtEnd::Continue);
        assert!(!host.fading);
        assert!(!host.pending_engine_stop, "no stop is queued");
        assert_eq!(host.transport_gain.target(), TRANSPORT_GAIN_UNITY, "and the gain heads back to unity");
    }

    #[test]
    fn the_song_fade_attenuates_the_output_all_the_way_to_silence() {
        let module = looping_mod();
        let mut host = Host::new(48_000);
        assert!(host.load_module(&module).is_ok());
        let end_frame = host.song_scan.as_ref().expect("the module was scanned").timeline.end_frame();
        let fade_frames: u32 = 48_000;
        assert!(host.enqueue(WireCommand { opcode: OPCODE_AT_END, argument: AT_END_FADE_OUT, extra: fade_frames }));
        assert!(host.enqueue(WireCommand { opcode: OPCODE_PLAY, argument: 0, extra: 0 }));

        let mut fading_peaks = Vec::new();
        let mut playing_words = Vec::new();
        for _ in 0..((end_frame as usize + fade_frames as usize) / RENDER_QUANTUM + 50) {
            let peak = host.process(RENDER_QUANTUM);
            if host.fading {
                fading_peaks.push(peak);
                playing_words.push(host.telemetry_words[15]);
            }
            if !host.engine.is_playing() { break; }
        }
        let fade_quanta = fade_frames as usize / RENDER_QUANTUM;
        assert!(fading_peaks.len() >= fade_quanta - 1 && fading_peaks.len() <= fade_quanta + 1, "the fade ran for its whole length: {} quanta", fading_peaks.len());
        assert!(playing_words.iter().all(|word| *word == 1), "the transport reports playing for the whole fade");

        let quarter = fading_peaks.len() / 4;
        let loudest = |peaks: &[f32]| peaks.iter().copied().fold(0.0f32, f32::max);
        let first = loudest(&fading_peaks[..quarter]);
        let last = loudest(&fading_peaks[fading_peaks.len() - quarter..]);
        assert!(first > 0.0, "the fixture makes sound going into the fade");
        assert!(last < first * 0.3, "the last quarter of the fade is well down on the first: {last} vs {first}");
        assert!(!host.engine.is_playing(), "and the transport stopped when the fade landed");
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
