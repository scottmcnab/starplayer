//! [`HostEngine`] — the finite set of engine instantiations a native host is willing to
//! build, and the post-stage that turns whichever one is live into interleaved `f32`.
//!
//! The engine's mix path, interpolator and output format are **type parameters**
//! (architecture §7.1), so choosing one is a re-instantiation rather than a field write.
//! A host therefore holds an enum over the arms it will build, and selecting a
//! [`MixerMode`] is a `match` performed once when the stream opens — never something
//! `render()` branches on.
//!
//! Eight arms: two paths × two interpolators × mono and stereo. Depth and dither are not
//! arms; they are the post-stage in [`crate::depth`], which is what gives every arm all
//! five depths without forty instantiations.

use std::boxed::Box;
use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

use starplayer::core::{Interpolator, U0F16};
use starplayer::dsp::{Interpolate, Linear, Nearest};
use starplayer::engine::{
    ChannelTable, Engine, EngineHandle, EngineSettings, EngineWarnings, EventSource, MixPathKind, MixerMode, OutputDepth,
};
use starplayer::mixer::{Dither, FixedOut, FixedPath, FloatOut, FloatPath, MixPath, OutputFormat};
use starplayer::model::Module;
use starplayer::rt::Arc;
use starplayer::telemetry::TelemetryReader;
use starplayer::{MAX_VOICE_CAPACITY, core::Frame};

use crate::backend::HostError;
use crate::depth::{dither_for, quantize_fixed_sample, quantize_float_sample};
use crate::transport::{TRANSPORT_GAIN_UNITY, Transport};

/// Frames converted in one pass of the scratch buffers.
///
/// Backends deliver ragged block sizes, some of them very large: a PulseAudio sink will
/// happily ask for half a second at once. The scratch is allocated to this and the render
/// walks a long block in chunks, so nothing here depends on the host's block size and
/// nothing allocates when it changes.
pub const MAX_FRAMES_PER_RENDER: usize = 4_096;

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
    let telemetry = engine.telemetry_reader().expect("telemetry is enabled for the native host");
    (engine, control, telemetry)
}

macro_rules! render_arm {
    (float, $engine:ident, $frames:ident, $float:expr, $fixed:expr, $channels:expr) => {{
        let sample_count = $frames.saturating_mul($channels);
        if let Some(destination) = $float.get_mut(..sample_count) { $engine.render(destination); }
    }};
    (fixed, $engine:ident, $frames:ident, $float:expr, $fixed:expr, $channels:expr) => {{
        let sample_count = $frames.saturating_mul($channels);
        if let Some(destination) = $fixed.get_mut(..sample_count) { $engine.render(destination); }
    }};
}

macro_rules! define_arms {
    ($($variant:ident => ($path_kind:path, $interpolator_kind:path, $channels:literal, $path:ty, $interpolator:ty, $output:ty, $buffer:ident)),+ $(,)?) => {
        /// One built engine, of whichever instantiation the mixer mode selected.
        enum EngineArm {
            $($variant(Engine<$path, $interpolator, $output, Arc<Module>>)),+
        }

        impl EngineArm {
            fn build(mode: MixerMode, settings: EngineSettings) -> Result<(EngineArm, EngineHandle<Arc<Module>>, TelemetryReader), HostError> {
                match (mode.path, mode.interpolator, mode.channels) {
                    $(($path_kind, $interpolator_kind, $channels) => {
                        let (engine, control, telemetry) = build_engine_arm::<$path, $interpolator, $output>(settings);
                        Ok((EngineArm::$variant(engine), control, telemetry))
                    },)+
                    _ => Err(HostError::UnsupportedMixerMode(mode.describe().to_string())),
                }
            }

            fn replace_source(&mut self, source: Box<dyn EventSource>) -> Option<Box<dyn EventSource>> {
                match self { $(EngineArm::$variant(engine) => engine.replace_source(source),)+ }
            }

            fn render_native(&mut self, frames: usize, float: &mut [f32], fixed: &mut [i16], channels: usize) {
                let _ = channels;
                match self {
                    $(EngineArm::$variant(engine) => render_arm!($buffer, engine, frames, float, fixed, $channels),)+
                }
            }

            fn frame(&self) -> Frame { match self { $(EngineArm::$variant(engine) => engine.frame(),)+ } }
            fn source_frame(&self) -> Frame { match self { $(EngineArm::$variant(engine) => engine.source_frame(),)+ } }
            fn is_playing(&self) -> bool { match self { $(EngineArm::$variant(engine) => engine.is_playing(),)+ } }
            fn master_volume(&self) -> U0F16 { match self { $(EngineArm::$variant(engine) => engine.master_volume(),)+ } }
            fn warnings(&self) -> EngineWarnings { match self { $(EngineArm::$variant(engine) => engine.warnings(),)+ } }
            fn module(&self) -> Option<&Arc<Module>> { match self { $(EngineArm::$variant(engine) => engine.module(),)+ } }
        }
    };
}

define_arms! {
    FloatNearestMono => (MixPathKind::Float, Interpolator::None, 1, FloatPath, Nearest, FloatOut<f32, 1>, float),
    FloatNearestStereo => (MixPathKind::Float, Interpolator::None, 2, FloatPath, Nearest, FloatOut<f32, 2>, float),
    FloatLinearMono => (MixPathKind::Float, Interpolator::Linear, 1, FloatPath, Linear, FloatOut<f32, 1>, float),
    FloatLinearStereo => (MixPathKind::Float, Interpolator::Linear, 2, FloatPath, Linear, FloatOut<f32, 2>, float),
    FixedNearestMono => (MixPathKind::Fixed, Interpolator::None, 1, FixedPath, Nearest, FixedOut<i16, 1>, fixed),
    FixedNearestStereo => (MixPathKind::Fixed, Interpolator::None, 2, FixedPath, Nearest, FixedOut<i16, 2>, fixed),
    FixedLinearMono => (MixPathKind::Fixed, Interpolator::Linear, 1, FixedPath, Linear, FixedOut<i16, 1>, fixed),
    FixedLinearStereo => (MixPathKind::Fixed, Interpolator::Linear, 2, FixedPath, Linear, FixedOut<i16, 2>, fixed),
}

/// The engine a native host renders through, plus the buffers and the dither state that
/// turn it into the interleaved `f32` a backend takes.
///
/// Everything it needs is allocated by [`HostEngine::build`]; nothing in
/// [`HostEngine::render`] allocates, locks, or can panic.
pub struct HostEngine {
    arm: EngineArm,
    mode: MixerMode,
    dither: Dither,
    /// Native output of a float-path arm.
    float_scratch: Vec<f32>,
    /// Native output of a fixed-path arm.
    fixed_scratch: Vec<i16>,
}

impl HostEngine {
    /// Build the arm `mode` names, at the maxima a persistent host takes.
    ///
    /// A host that plays many modules through one engine cannot ask a processor how wide
    /// its pool should be, so it takes [`MAX_VOICE_CAPACITY`] and
    /// [`ChannelTable::MAX_CHANNELS`] once. Both are allocated here, and the mixer and the
    /// telemetry view walk only what is *active*, so a wider pool changes no rendered
    /// sample — only how much memory the host holds.
    pub fn build(mode: MixerMode, sample_rate_hz: u32) -> Result<(HostEngine, EngineHandle<Arc<Module>>, TelemetryReader), HostError> {
        let settings = EngineSettings {
            sample_rate_hz,
            voice_capacity: MAX_VOICE_CAPACITY,
            channel_count: ChannelTable::MAX_CHANNELS,
            ..EngineSettings::default()
        };
        let (arm, control, telemetry) = EngineArm::build(mode, settings)?;
        let samples = MAX_FRAMES_PER_RENDER * mode.channels as usize;
        let engine = HostEngine {
            arm,
            mode,
            dither: dither_for(mode),
            float_scratch: vec![0.0; samples],
            fixed_scratch: vec![0; samples],
        };
        Ok((engine, control, telemetry))
    }

    /// The mixer mode this engine was built for.
    pub const fn mode(&self) -> MixerMode { self.mode }

    /// Output channels, which is [`MixerMode::channels`].
    pub const fn channels(&self) -> usize { self.mode.channels as usize }

    /// Swap the playing source and hand the retired one back rather than dropping it —
    /// see [`Engine::replace_source`].
    pub fn replace_source(&mut self, source: Box<dyn EventSource>) -> Option<Box<dyn EventSource>> {
        self.arm.replace_source(source)
    }

    /// The output clock.
    pub fn frame(&self) -> Frame { self.arm.frame() }

    /// The musical clock event sources report against.
    pub fn source_frame(&self) -> Frame { self.arm.source_frame() }

    /// Whether the musical clock is running.
    pub fn is_playing(&self) -> bool { self.arm.is_playing() }

    /// The master volume the control plane last set.
    pub fn master_volume(&self) -> U0F16 { self.arm.master_volume() }

    /// Sticky warnings raised by the render loop.
    pub fn warnings(&self) -> EngineWarnings { self.arm.warnings() }

    /// The module the control plane last swapped in.
    pub fn module(&self) -> Option<&Arc<Module>> { self.arm.module() }

    /// Render `output` — interleaved, `channels()` per frame, any length — applying
    /// `transport`'s per-frame gain and the mode's output depth on the way out.
    ///
    /// Reports the block's peak, which is what a VU meter wants and what the browser host
    /// has always returned from `process`.
    pub fn render(&mut self, output: &mut [f32], transport: &mut Transport) -> f32 {
        let channels = self.channels();
        let mut peak = 0.0f32;
        for chunk in output.chunks_mut(MAX_FRAMES_PER_RENDER.saturating_mul(channels)) {
            peak = peak.max(self.render_chunk(chunk, transport, channels));
        }
        peak
    }

    /// One chunk of at most [`MAX_FRAMES_PER_RENDER`] frames, which is what the scratch
    /// buffers hold.
    fn render_chunk(&mut self, output: &mut [f32], transport: &mut Transport, channels: usize) -> f32 {
        let frames = output.len() / channels.max(1);
        self.arm.render_native(frames, &mut self.float_scratch, &mut self.fixed_scratch, channels);

        let mut peak = 0.0f32;
        for frame in 0..frames {
            let gain_units = transport.advance();
            for channel in 0..channels {
                let index = frame.saturating_mul(channels).saturating_add(channel);
                let (sample, _) = match self.mode.path {
                    MixPathKind::Float => {
                        let native = self.float_scratch.get(index).copied().unwrap_or(0.0);
                        let gained = native * gain_units as f32 / TRANSPORT_GAIN_UNITY as f32;
                        quantize_float_sample(gained, self.mode.depth, &mut self.dither)
                    }
                    MixPathKind::Fixed => {
                        let native = self.fixed_scratch.get(index).copied().unwrap_or(0) as i32;
                        let gained = ((native as i64 * gain_units as i64) / TRANSPORT_GAIN_UNITY as i64) as i32;
                        quantize_fixed_sample(gained, self.mode.depth, &mut self.dither)
                    }
                };
                if let Some(destination) = output.get_mut(index) { *destination = sample; }
                peak = peak.max(sample.abs());
            }
        }
        peak
    }
}

/// A human-readable list of the arms this host builds, for a `--help` or an error message.
pub fn supported_modes() -> String {
    String::from("float or fixed path, nearest or linear interpolation, mono or stereo; any of the five depths")
}

/// Every depth a host can ask for on any arm.
pub const SUPPORTED_DEPTHS: [OutputDepth; 5] =
    [OutputDepth::F32, OutputDepth::I32, OutputDepth::I24, OutputDepth::I16, OutputDepth::I8];
