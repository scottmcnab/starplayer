//! Deterministic offline rendering: the WAV writer, fixed-block-size render drivers and
//! the per-tick trace dump.
//!
//! This is where the buffer-size-independence invariant is exercised — rendering the same
//! module at host block sizes 1, 3, 64, 128, 4096 and 8191 must produce byte-identical
//! output.
//!
//! Allowed dependency edges: `starplayer`.

#![forbid(unsafe_code)]

use std::fmt;

use starplayer::core::{Error, ExactFixedPoint, Interpolator};
use starplayer::dsp::{Interpolate, Linear, Nearest};
use starplayer::engine::{Engine, EngineSettings, EngineWarnings, EventSource};
use starplayer::mixer::{FixedPath, FloatPath, Limiter, MixPath, MonoF32, MonoI16, OutputFormat};
use starplayer::model::Module;
use starplayer::rt::Arc;

// The per-tick trace is a diagnostic build only. Everything it needs sits behind the
// `trace` feature, so an ordinary `cargo test --workspace` — and every golden render —
// compiles the engine without the recorder in its render path.
#[cfg(feature = "trace")]
use starplayer::core::Frame;
#[cfg(feature = "trace")]
use starplayer::engine::{EndOfSongPolicy, PatternSequencer, SequencerSettings, Trace};
#[cfg(any(feature = "trace", test))]
use starplayer::mixer::StereoI16;
#[cfg(feature = "trace")]
use starplayer::model::ModuleFormat;

use sha2::{Digest, Sha256};

pub mod fixtures;

/// Output rate used by diagnostic traces and the future canonical golden renderer.
#[cfg(feature = "trace")]
pub const TRACE_SAMPLE_RATE_HZ: u32 = 44_100;

/// The only rate in the canonical audio-golden contract.
pub const GOLDEN_SAMPLE_RATE_HZ: u32 = 44_100;

/// Goldens fingerprint the first ten seconds of every fixture. A fixed-duration segment
/// makes regeneration bounded even for modules whose order list loops forever, while
/// still covering hundreds of tracker ticks and every active mixer stage.
pub const GOLDEN_RENDER_FRAMES: usize = GOLDEN_SAMPLE_RATE_HZ as usize * 10;

/// The default host request size used to regenerate a hash. It cannot affect the bytes;
/// tests repeat the render at the six invariant block sizes.
pub const GOLDEN_HOST_BLOCK_FRAMES: usize = 128;

/// Interpolator selected by the canonical render and its filename.
pub const GOLDEN_INTERPOLATOR: Interpolator = Interpolator::Linear;

/// Segment length for the float-versus-fixed perceptual regression check.
pub const SEGMENTAL_SNR_FRAMES: usize = 1_024;

/// Minimum accepted average segmental SNR between the linear, DSP-bypassed float and
/// fixed paths. 60 dB is roughly ten effective bits of agreement and leaves measured
/// headroom for multi-voice accumulator-order differences without masking an audible
/// path regression.
pub const MIN_FLOAT_FIXED_SEGMENTAL_SNR_DB: f64 = 60.0;

/// Guard against a malformed module whose control flow never reaches its end marker.
#[cfg(feature = "trace")]
pub const MAX_CAPTURE_TICKS: usize = 1_000_000;

/// Knobs for deterministic trace capture.
#[cfg(feature = "trace")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TraceOptions {
    /// Stop after this many ticks. `None` follows the module until its end marker.
    pub ticks: Option<usize>,
    /// Frames requested per host call. This cannot affect the resulting trace.
    pub host_block_frames: usize,
}

#[cfg(feature = "trace")]
impl Default for TraceOptions {
    fn default() -> TraceOptions { TraceOptions { ticks: None, host_block_frames: 128 } }
}

/// Failure to load or finish a diagnostic trace.
#[cfg(feature = "trace")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TraceError {
    /// No enabled native loader accepted the input.
    Load(Error),
    /// Playback did not terminate within [`MAX_CAPTURE_TICKS`].
    TickLimit,
}

/// A format that has a canonical golden render.
///
/// The variant, the loader, the sequencer and the `goldens/<format>/` directory are
/// chosen together here so a fixture cannot be rendered through the wrong processor.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GoldenFormat {
    Mod,
    S3m,
    Mtm,
}

impl GoldenFormat {
    /// Every format the golden contract covers.
    pub const ALL: [GoldenFormat; 3] = [GoldenFormat::Mod, GoldenFormat::S3m, GoldenFormat::Mtm];

    /// The `goldens/<format>/` directory this format's hashes live in.
    pub const fn directory(self) -> &'static str {
        match self {
            GoldenFormat::Mod => "mod",
            GoldenFormat::S3m => "s3m",
            GoldenFormat::Mtm => "mtm",
        }
    }
}

impl fmt::Display for GoldenFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result { formatter.write_str(self.directory()) }
}

/// Failure to produce a canonical audio render.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The format's loader rejected the input.
    Load(Error),
    /// The fresh engine's command queue could not accept its module.
    CommandQueue,
    /// The engine's safety guards fired during the supposedly canonical segment.
    EngineWarnings(EngineWarnings),
    /// [`GOLDEN_INTERPOLATOR`] names a kernel this build has no implementation for.
    /// The filename would promise audio the renderer cannot produce, so it refuses.
    UnimplementedInterpolator(Interpolator),
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::Load(error) => write!(formatter, "could not load module: {error}"),
            RenderError::CommandQueue => write!(formatter, "fresh offline engine rejected its module command"),
            RenderError::EngineWarnings(warnings) => write!(formatter, "offline render raised engine warnings: {warnings:?}"),
            RenderError::UnimplementedInterpolator(kernel) => write!(formatter, "GOLDEN_INTERPOLATOR names {kernel:?}, which has no kernel in this build"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<Error> for RenderError {
    fn from(error: Error) -> RenderError { RenderError::Load(error) }
}

#[cfg(feature = "trace")]
impl fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TraceError::Load(error) => write!(formatter, "could not load module: {error}"),
            TraceError::TickLimit => write!(formatter, "trace exceeded the safety limit of {MAX_CAPTURE_TICKS} ticks"),
        }
    }
}

#[cfg(feature = "trace")]
impl std::error::Error for TraceError {}

#[cfg(feature = "trace")]
impl From<Error> for TraceError {
    fn from(error: Error) -> TraceError { TraceError::Load(error) }
}

#[cfg(feature = "trace")]
type TraceEngine = Engine<FixedPath, Linear, StereoI16, Arc<Module>>;

/// Render the canonical fixed-path segment at an arbitrary host block size.
///
/// The path is exactly the filename contract: fixed-point voice accumulation, the kernel
/// [`GOLDEN_INTERPOLATOR`] names, mono `i16`, 44.1 kHz, no dither, and the nonlinear
/// master limiter bypassed in favour of a transparent clamp. Per-channel DSP is currently
/// a no-op; when the graph lands, this entry point remains its explicit bypass boundary.
pub fn render_fixed_mono(format: GoldenFormat, bytes: &[u8], host_block_frames: usize) -> Result<Vec<i16>, RenderError> {
    render_golden::<FixedPath, MonoI16>(format, bytes, GOLDEN_RENDER_FRAMES, host_block_frames)
}

/// Render the float path with the same rate, duration, interpolation, mono fold-down and
/// DSP bypass as [`render_fixed_mono`].
pub fn render_float_mono(format: GoldenFormat, bytes: &[u8], host_block_frames: usize) -> Result<Vec<f32>, RenderError> {
    render_golden::<FloatPath, MonoF32>(format, bytes, GOLDEN_RENDER_FRAMES, host_block_frames)
}

/// SHA-256 over the canonical samples encoded as little-endian signed PCM words.
///
/// Hashing an explicit byte order, rather than the in-memory representation of `i16`, is
/// what lets x86-64, aarch64 and wasm32 compare the same digest.
pub fn canonical_sha256(format: GoldenFormat, bytes: &[u8], host_block_frames: usize) -> Result<[u8; 32], RenderError> {
    let samples = render_fixed_mono(format, bytes, host_block_frames)?;
    let mut hasher = Sha256::new();
    for sample in samples {
        hasher.update(sample.to_le_bytes());
    }
    let digest = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);
    Ok(hash)
}

/// The S3M-only spelling C6 shipped, kept so existing callers and tests read unchanged.
pub fn render_s3m_fixed_mono(bytes: &[u8], host_block_frames: usize) -> Result<Vec<i16>, RenderError> {
    render_fixed_mono(GoldenFormat::S3m, bytes, host_block_frames)
}

/// The S3M-only spelling of [`render_float_mono`].
pub fn render_s3m_float_mono(bytes: &[u8], host_block_frames: usize) -> Result<Vec<f32>, RenderError> {
    render_float_mono(GoldenFormat::S3m, bytes, host_block_frames)
}

/// The S3M-only spelling of [`canonical_sha256`].
pub fn canonical_s3m_sha256(bytes: &[u8], host_block_frames: usize) -> Result<[u8; 32], RenderError> {
    canonical_sha256(GoldenFormat::S3m, bytes, host_block_frames)
}

/// Lower-case hexadecimal representation used by `.sha256` files and cross-target logs.
pub fn sha256_hex(hash: [u8; 32]) -> String {
    use std::fmt::Write;

    let mut hexadecimal = String::with_capacity(64);
    for byte in hash {
        let _ = write!(hexadecimal, "{byte:02x}");
    }
    hexadecimal
}

/// Configuration-encoded filename for one module stem.
pub fn golden_filename(module_stem: &str) -> String {
    golden_filename_for_interpolator(module_stem, GOLDEN_INTERPOLATOR)
}

/// The configuration filename an alternate interpolator would use.
///
/// Only linear is rendered canonically in C6. This mapping exists to make the filename
/// transition testable: selecting another kernel creates a missing new golden instead of
/// comparing its bytes against the linear hash.
pub fn golden_filename_for_interpolator(module_stem: &str, interpolator: Interpolator) -> String {
    let label = match interpolator {
        Interpolator::None => "nearest",
        Interpolator::Linear => "linear",
        Interpolator::Cubic => "cubic",
        Interpolator::Sinc => "sinc",
    };
    format!("{module_stem}__i16_mono_44100_{label}.sha256")
}

/// Average segmental SNR in dB, treating `fixed` as the reference signal.
///
/// Segments below −80 dBFS RMS are excluded: silence has no meaningful SNR and would let
/// long quiet introductions dominate the result. Exact matches are capped at 120 dB so
/// the average stays finite. Returns `None` when the lengths differ, the segment length is
/// zero, or every segment is silent.
pub fn segmental_snr_db(fixed: &[i16], float: &[f32], segment_frames: usize) -> Option<f64> {
    if fixed.len() != float.len() || segment_frames == 0 {
        return None;
    }

    let mut total_db = 0.0f64;
    let mut compared_segments = 0usize;
    for (fixed_segment, float_segment) in fixed.chunks(segment_frames).zip(float.chunks(segment_frames)) {
        let mut signal_energy = 0.0f64;
        let mut error_energy = 0.0f64;
        for (&fixed_sample, &float_sample) in fixed_segment.iter().zip(float_segment.iter()) {
            let reference = fixed_sample as f64 * (1.0 / 32_767.0);
            let error = reference - float_sample as f64;
            signal_energy += reference * reference;
            error_energy += error * error;
        }
        let frame_count = fixed_segment.len().max(1) as f64;
        if signal_energy / frame_count < 1.0e-8 {
            continue;
        }
        let segment_db = if error_energy == 0.0 {
            120.0
        } else {
            (10.0 * (signal_energy / error_energy).log10()).clamp(-20.0, 120.0)
        };
        total_db += segment_db;
        compared_segments += 1;
    }

    if compared_segments == 0 { None } else { Some(total_db / compared_segments as f64) }
}

/// Dispatch the canonical render on [`GOLDEN_INTERPOLATOR`].
///
/// This `match` is the whole point of the constant: the same value that names the golden
/// file selects the kernel that produces its bytes, so the two cannot disagree. Adding a
/// kernel to `starplayer-dsp` means adding one arm here, and nothing else in this crate
/// may name an interpolator type.
fn render_golden<Path, Out>(format: GoldenFormat, bytes: &[u8], frames: usize, host_block_frames: usize) -> Result<Vec<Out::Sample>, RenderError>
where
    Path: MixPath,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    match GOLDEN_INTERPOLATOR {
        Interpolator::None => render_with_kernel::<Path, Nearest, Out>(format, bytes, frames, host_block_frames),
        Interpolator::Linear => render_with_kernel::<Path, Linear, Out>(format, bytes, frames, host_block_frames),
        kernel => Err(RenderError::UnimplementedInterpolator(kernel)),
    }
}

fn render_with_kernel<Path, Interp, Out>(format: GoldenFormat, bytes: &[u8], frames: usize, host_block_frames: usize) -> Result<Vec<Out::Sample>, RenderError>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let module = Arc::new(load_golden(format, bytes)?);
    let channel_count = module.header().channel_count as usize;
    let settings = EngineSettings {
        sample_rate_hz: GOLDEN_SAMPLE_RATE_HZ,
        channel_count,
        voice_capacity: channel_count.max(1),
        ..EngineSettings::default()
    };
    let mut engine: Engine<Path, Interp, Out, Arc<Module>> = Engine::with_settings(settings);
    let mut control = engine.take_control().ok_or(RenderError::CommandQueue)?;
    control.load_module(Arc::clone(&module)).map_err(|_| RenderError::CommandQueue)?;
    engine.set_source(golden_source(format, module));
    engine.set_limiter(Limiter::Clamp);

    let output_samples = frames.saturating_mul(Out::CHANNELS);
    let block_samples = host_block_frames.max(1).saturating_mul(Out::CHANNELS).max(Out::CHANNELS);
    let mut output = vec![Out::Sample::default(); output_samples];
    for block in output.chunks_mut(block_samples) {
        engine.render(block);
    }
    let warnings = engine.warnings();
    if warnings.any() { Err(RenderError::EngineWarnings(warnings)) } else { Ok(output) }
}

fn load_golden(format: GoldenFormat, bytes: &[u8]) -> Result<Module, Error> {
    match format {
        GoldenFormat::Mod => starplayer::mod_file::load(bytes),
        GoldenFormat::S3m => starplayer::s3m::load(bytes),
        GoldenFormat::Mtm => starplayer::mtm::load(bytes),
    }
}

fn golden_source(format: GoldenFormat, module: Arc<Module>) -> Box<dyn EventSource> {
    match format {
        GoldenFormat::Mod => Box::new(starplayer::mod_file::sequencer_for(module, GOLDEN_SAMPLE_RATE_HZ, ExactFixedPoint)),
        GoldenFormat::S3m => Box::new(starplayer::s3m::sequencer_for(module, GOLDEN_SAMPLE_RATE_HZ, ExactFixedPoint)),
        GoldenFormat::Mtm => Box::new(starplayer::mtm::sequencer_for(module, GOLDEN_SAMPLE_RATE_HZ, ExactFixedPoint)),
    }
}

/// Autodetect a supported module and capture its stable per-tick trace with that format's
/// native processor.
#[cfg(feature = "trace")]
pub fn trace_module(bytes: &[u8], options: TraceOptions) -> Result<Trace, TraceError> {
    trace_loaded(Arc::new(starplayer::load(bytes)?), options)
}

/// Load an S3M and capture its stable per-tick trace.
#[cfg(feature = "trace")]
pub fn trace_s3m(bytes: &[u8], options: TraceOptions) -> Result<Trace, TraceError> {
    trace_loaded(Arc::new(starplayer::s3m::load(bytes)?), options)
}

/// Load a MOD and capture its stable per-tick trace through the ProTracker processor.
#[cfg(feature = "trace")]
pub fn trace_mod(bytes: &[u8], options: TraceOptions) -> Result<Trace, TraceError> {
    trace_loaded(Arc::new(starplayer::mod_file::load(bytes)?), options)
}

/// Load an MTM and capture its stable per-tick trace through the MultiTracker processor.
#[cfg(feature = "trace")]
pub fn trace_mtm(bytes: &[u8], options: TraceOptions) -> Result<Trace, TraceError> {
    trace_loaded(Arc::new(starplayer::mtm::load(bytes)?), options)
}

#[cfg(feature = "trace")]
fn trace_loaded(module: Arc<Module>, options: TraceOptions) -> Result<Trace, TraceError> {
    let channel_count = module.header().channel_count as usize;
    let engine_settings = EngineSettings {
        sample_rate_hz: TRACE_SAMPLE_RATE_HZ,
        channel_count,
        voice_capacity: channel_count.max(1),
        ..EngineSettings::default()
    };
    let mut engine: TraceEngine = Engine::with_settings(engine_settings);
    let mut control = engine.take_control().expect("a fresh offline engine owns its control handle");
    control.load_module(Arc::clone(&module)).map_err(|_| TraceError::Load(Error::Invalid("module command queue is full")))?;

    let sequencer_settings = SequencerSettings {
        sample_rate_hz: TRACE_SAMPLE_RATE_HZ,
        first_tick_frame: Frame::ZERO,
        initial_speed: module.header().initial_speed,
        initial_tempo_bpm: module.header().initial_tempo,
        restart_order: 0,
        end_of_song: EndOfSongPolicy::Stop,
    };
    let source: Box<dyn EventSource> = match module.header().format {
        ModuleFormat::S3m => Box::new(PatternSequencer::new(
            ExactFixedPoint,
            starplayer::s3m::S3mPatternData(Arc::clone(&module)),
            starplayer::s3m::S3mProcessor::new(module, TRACE_SAMPLE_RATE_HZ),
            sequencer_settings,
        )),
        ModuleFormat::Mod => Box::new(PatternSequencer::new(
            ExactFixedPoint,
            starplayer::mod_file::ModPatternData(Arc::clone(&module)),
            starplayer::mod_file::ModProcessor::new(module, TRACE_SAMPLE_RATE_HZ),
            sequencer_settings,
        )),
        ModuleFormat::Mtm => Box::new(PatternSequencer::new(
            ExactFixedPoint,
            starplayer::mtm::MtmPatternData(Arc::clone(&module)),
            starplayer::mtm::MtmProcessor::new(module, TRACE_SAMPLE_RATE_HZ),
            sequencer_settings,
        )),
        _ => return Err(TraceError::Load(Error::Invalid("format has no offline native processor"))),
    };
    engine.set_source(source);

    let requested_ticks = options.ticks.unwrap_or(MAX_CAPTURE_TICKS);
    if requested_ticks == 0 {
        return Ok(engine.take_trace());
    }
    let block_frames = options.host_block_frames.max(1);
    let mut output = vec![0i16; block_frames.saturating_mul(2).max(2)];
    while engine.trace().ticks.len() < requested_ticks && engine.sources().next_event_frame().is_some() {
        engine.render(&mut output);
    }

    let source_still_running = engine.sources().next_event_frame().is_some();
    let mut trace = engine.take_trace();
    if let Some(ticks) = options.ticks {
        trace.truncate(ticks);
    } else if source_still_running && trace.ticks.len() >= MAX_CAPTURE_TICKS {
        return Err(TraceError::TickLimit);
    }
    Ok(trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFLEX: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");
    const GOLDEN_CORPUS: &[(&str, &[u8])] = &[
        ("ARMANI.S3M", include_bytes!("../../starplayer-s3m/tests/fixtures/ARMANI.S3M")),
        ("MOVEMENT.S3M", include_bytes!("../../starplayer-s3m/tests/fixtures/MOVEMENT.S3M")),
        ("NICETUNE.S3M", include_bytes!("../../starplayer-s3m/tests/fixtures/NICETUNE.S3M")),
        ("PETRI.S3M", include_bytes!("../../starplayer-s3m/tests/fixtures/PETRI.S3M")),
        ("REFLEX.S3M", REFLEX),
    ];

    #[cfg(feature = "trace")]
    fn minimal_mod() -> Vec<u8> {
        let mut bytes = vec![0; 1084 + 64 * 4 * 4 + 256];
        bytes[..10].copy_from_slice(b"native mod");
        bytes[42..44].copy_from_slice(&128u16.to_be_bytes());
        bytes[45] = 64;
        bytes[950] = 1;
        bytes[1080..1084].copy_from_slice(b"M.K.");
        bytes[1084..1088].copy_from_slice(&starplayer::mod_file::ModCell { period: 428, instrument: 1, effect: 0, param: 0 }.to_bytes());
        bytes
    }

    fn minimal_mtm() -> Vec<u8> {
        const SAMPLE_HEADER: usize = 66;
        const ORDER_TABLE: usize = SAMPLE_HEADER + 37;
        const TRACK_DATA: usize = ORDER_TABLE + 128;
        const PATTERN_TABLE: usize = TRACK_DATA + 192;
        const SAMPLE_DATA: usize = PATTERN_TABLE + 64;
        let mut bytes = vec![0; SAMPLE_DATA + 256];
        bytes[..4].copy_from_slice(b"MTM\x10");
        bytes[4..14].copy_from_slice(b"native mtm");
        bytes[24..26].copy_from_slice(&1u16.to_le_bytes());
        bytes[30] = 1;
        bytes[32] = 64;
        bytes[33] = 1;
        bytes[34] = 8;
        bytes[SAMPLE_HEADER..SAMPLE_HEADER + 6].copy_from_slice(b"sample");
        bytes[SAMPLE_HEADER + 22..SAMPLE_HEADER + 26].copy_from_slice(&256u32.to_le_bytes());
        bytes[SAMPLE_HEADER + 35] = 64;
        bytes[TRACK_DATA..TRACK_DATA + 3].copy_from_slice(&starplayer::mtm::MtmCell { pitch: 12, instrument: 1, effect: 0, param: 0 }.to_bytes());
        bytes[PATTERN_TABLE..PATTERN_TABLE + 2].copy_from_slice(&1u16.to_le_bytes());
        for (index, byte) in bytes[SAMPLE_DATA..].iter_mut().enumerate() { *byte = if index & 1 == 0 { 0xFF } else { 0x00 }; }
        bytes
    }

    #[cfg(feature = "trace")]
    fn mtm_with_a_mod_tag_collision() -> Vec<u8> {
        const TRACK_COUNT: usize = 5;
        const TRACK_OFFSET: usize = 66 + 128;
        const PATTERN_OFFSET: usize = TRACK_OFFSET + TRACK_COUNT * 192;
        let mut bytes = vec![0; PATTERN_OFFSET + 64];
        bytes[..4].copy_from_slice(b"MTM\x10");
        bytes[24..26].copy_from_slice(&(TRACK_COUNT as u16).to_le_bytes());
        bytes[32] = 64;
        bytes[33] = 1;
        bytes[34] = 8;
        bytes[1080..1084].copy_from_slice(b"M.K.");
        bytes
    }

    #[cfg(feature = "trace")]
    fn trace_with_block_size(host_block_frames: usize) -> Trace {
        trace_s3m(REFLEX, TraceOptions { ticks: Some(24), host_block_frames }).expect("REFLEX traces")
    }

    fn render_mtm_with_block_size(bytes: &[u8], host_block_frames: usize) -> Vec<i16> {
        let module = Arc::new(starplayer::mtm::load(bytes).expect("valid MTM"));
        let settings = EngineSettings {
            sample_rate_hz: 44_100,
            channel_count: module.header().channel_count as usize,
            voice_capacity: module.header().channel_count.max(1) as usize,
            ..EngineSettings::default()
        };
        let mut engine: Engine<FixedPath, Linear, StereoI16, Arc<Module>> = Engine::with_settings(settings);
        let mut control = engine.take_control().expect("fresh control");
        control.load_module(Arc::clone(&module)).expect("fresh command queue");
        engine.set_source(Box::new(starplayer::mtm::sequencer_for(module, 44_100, ExactFixedPoint)));
        engine.set_limiter(Limiter::Clamp);
        let mut output = vec![0; 44_100 * 2];
        for block in output.chunks_mut(host_block_frames.max(1) * 2) { engine.render(block); }
        assert!(!engine.warnings().any());
        output
    }

    #[cfg(feature = "trace")]
    #[test]
    fn a_trace_is_repeatable_and_host_block_size_independent() {
        let first = trace_with_block_size(128).to_text();
        assert_eq!(trace_with_block_size(128).to_text(), first, "two runs are byte-identical");
        for host_block_frames in [1, 3, 64, 128, 4096, 8191] {
            assert_eq!(trace_with_block_size(host_block_frames).to_text(), first, "host block size {host_block_frames} changed the trace");
        }
    }

    #[test]
    fn the_canonical_render_and_hash_are_repeatable_at_every_host_block_size() {
        assert_eq!(GOLDEN_INTERPOLATOR, Interpolator::Linear, "the canonical kernel is the one the filenames name");
        assert_eq!(golden_filename("reflex"), "reflex__i16_mono_44100_linear.sha256");
        assert_eq!(golden_filename_for_interpolator("reflex", Interpolator::Linear), golden_filename("reflex"));
        assert_eq!(golden_filename_for_interpolator("reflex", Interpolator::None), "reflex__i16_mono_44100_nearest.sha256");
        let reference = render_s3m_fixed_mono(REFLEX, GOLDEN_HOST_BLOCK_FRAMES).expect("REFLEX renders");
        assert!(reference.iter().any(|sample| *sample != 0), "the canonical segment is not silent");
        let reference_hash = canonical_s3m_sha256(REFLEX, GOLDEN_HOST_BLOCK_FRAMES).expect("REFLEX hashes");
        assert_eq!(canonical_s3m_sha256(REFLEX, GOLDEN_HOST_BLOCK_FRAMES).expect("REFLEX hashes twice"), reference_hash);

        for host_block_frames in [1, 3, 64, 128, 4096, 8191] {
            assert_eq!(render_s3m_fixed_mono(REFLEX, host_block_frames).expect("REFLEX renders"), reference, "host block size {host_block_frames} changed the canonical PCM");
            assert_eq!(canonical_s3m_sha256(REFLEX, host_block_frames).expect("REFLEX hashes"), reference_hash, "host block size {host_block_frames} changed the canonical hash");
        }
    }

    #[test]
    fn the_float_path_stays_above_the_stated_segmental_snr_on_the_corpus() {
        for &(name, bytes) in GOLDEN_CORPUS {
            let fixed = render_s3m_fixed_mono(bytes, GOLDEN_HOST_BLOCK_FRAMES).expect("fixed render");
            let float = render_s3m_float_mono(bytes, GOLDEN_HOST_BLOCK_FRAMES).expect("float render");
            assert!(float.iter().all(|sample| sample.is_finite()), "{name}: float render contains NaN or infinity");
            let snr = segmental_snr_db(&fixed, &float, SEGMENTAL_SNR_FRAMES).expect("the module has audible segments");
            assert!(snr >= MIN_FLOAT_FIXED_SEGMENTAL_SNR_DB, "{name}: {snr:.2} dB is below the {MIN_FLOAT_FIXED_SEGMENTAL_SNR_DB:.2} dB contract");
        }
    }

    #[cfg(feature = "trace")]
    #[test]
    fn native_mod_trace_is_repeatable_and_host_block_size_independent() {
        let module = minimal_mod();
        let first = trace_module(&module, TraceOptions { ticks: Some(24), host_block_frames: 128 }).expect("MOD traces").to_text();
        assert!(first.contains("per=000428"), "the trace carries native MOD periods: {first}");
        for host_block_frames in [1, 3, 64, 128, 4096, 8191] {
            let trace = trace_mod(&module, TraceOptions { ticks: Some(24), host_block_frames }).expect("native MOD trace").to_text();
            assert_eq!(trace, first, "host block size {host_block_frames} changed the MOD trace");
        }
    }

    #[cfg(feature = "trace")]
    #[test]
    fn native_mtm_trace_is_repeatable_and_host_block_size_independent() {
        let module = minimal_mtm();
        let first = trace_module(&module, TraceOptions { ticks: Some(24), host_block_frames: 128 }).expect("MTM traces").to_text();
        assert!(first.contains("per=000856"), "the trace carries native MTM periods: {first}");
        for host_block_frames in [1, 3, 64, 128, 4096, 8191] {
            let trace = trace_mtm(&module, TraceOptions { ticks: Some(24), host_block_frames }).expect("native MTM trace").to_text();
            assert_eq!(trace, first, "host block size {host_block_frames} changed the MTM trace");
        }
    }

    #[cfg(feature = "trace")]
    #[test]
    fn generic_trace_dispatch_prefers_strong_mtm_magic_over_a_mod_tag_collision() {
        let module = mtm_with_a_mod_tag_collision();
        assert!(starplayer::mod_file::probe(&module), "the fixture reaches the weak MOD probe");
        assert_eq!(starplayer::probe(&module), Some(ModuleFormat::Mtm));
        let options = TraceOptions { ticks: Some(4), host_block_frames: 128 };
        assert_eq!(trace_module(&module, options).expect("generic MTM trace"), trace_mtm(&module, options).expect("native MTM trace"));
    }

    #[test]
    fn every_golden_format_renders_audibly_and_hashes_identically_at_every_block_size() {
        let synthetic_mod = fixtures::synthetic_mod();
        let synthetic_mtm = fixtures::synthetic_mtm();
        let corpus: &[(GoldenFormat, &str, &[u8])] = &[
            (GoldenFormat::Mod, "synthetic", &synthetic_mod),
            (GoldenFormat::S3m, "REFLEX.S3M", REFLEX),
            (GoldenFormat::Mtm, "synthetic", &synthetic_mtm),
        ];
        for &(format, name, bytes) in corpus {
            let reference = render_fixed_mono(format, bytes, GOLDEN_HOST_BLOCK_FRAMES).expect("the fixture renders");
            assert_eq!(reference.len(), GOLDEN_RENDER_FRAMES, "{format} {name}: the canonical segment is ten seconds of mono");
            assert!(reference.iter().any(|sample| *sample != 0), "{format} {name}: the canonical segment is not silent");
            let reference_hash = canonical_sha256(format, bytes, GOLDEN_HOST_BLOCK_FRAMES).expect("the fixture hashes");
            for host_block_frames in [1, 3, 64, 128, 4096, 8191] {
                assert_eq!(canonical_sha256(format, bytes, host_block_frames).expect("the fixture hashes"), reference_hash, "{format} {name}: host block size {host_block_frames} changed the canonical hash");
            }
        }
    }

    #[test]
    fn the_golden_formats_and_their_directories_are_distinct() {
        assert_eq!(GoldenFormat::ALL.map(GoldenFormat::directory), ["mod", "s3m", "mtm"]);
        assert_eq!(GoldenFormat::S3m.to_string(), "s3m");
    }

    #[test]
    fn native_mtm_audio_is_host_block_size_independent() {
        let module = minimal_mtm();
        let reference = render_mtm_with_block_size(&module, 128);
        assert!(reference.iter().any(|sample| *sample != 0), "the native MTM render is audible");
        for host_block_frames in [1, 3, 64, 128, 4096, 8191] {
            assert_eq!(render_mtm_with_block_size(&module, host_block_frames), reference, "host block size {host_block_frames} changed MTM PCM");
        }
    }
}
