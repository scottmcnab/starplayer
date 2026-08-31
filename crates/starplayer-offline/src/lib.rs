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

use starplayer::core::{Error, ExactFixedPoint, Frame, Interpolator};
use starplayer::dsp::Linear;
use starplayer::engine::{EndOfSongPolicy, Engine, EngineSettings, EngineWarnings, EventSource, PatternSequencer, SequencerSettings, Trace};
use starplayer::mixer::{FixedPath, FloatPath, Limiter, MixPath, MonoF32, MonoI16, OutputFormat, StereoI16};
use starplayer::model::Module;
use starplayer::rt::Arc;

use sha2::{Digest, Sha256};

/// Output rate used by diagnostic traces and the future canonical golden renderer.
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
pub const MAX_CAPTURE_TICKS: usize = 1_000_000;

/// Knobs for deterministic trace capture.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TraceOptions {
    /// Stop after this many ticks. `None` follows the module until its end marker.
    pub ticks: Option<usize>,
    /// Frames requested per host call. This cannot affect the resulting trace.
    pub host_block_frames: usize,
}

impl Default for TraceOptions {
    fn default() -> TraceOptions { TraceOptions { ticks: None, host_block_frames: 128 } }
}

/// Failure to load or finish a diagnostic trace.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TraceError {
    /// The S3M loader rejected the input.
    Load(Error),
    /// Playback did not terminate within [`MAX_CAPTURE_TICKS`].
    TickLimit,
}

/// Failure to produce a canonical audio render.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// The S3M loader rejected the input.
    Load(Error),
    /// The fresh engine's command queue could not accept its module.
    CommandQueue,
    /// The engine's safety guards fired during the supposedly canonical segment.
    EngineWarnings(EngineWarnings),
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::Load(error) => write!(formatter, "could not load S3M module: {error}"),
            RenderError::CommandQueue => write!(formatter, "fresh offline engine rejected its module command"),
            RenderError::EngineWarnings(warnings) => write!(formatter, "offline render raised engine warnings: {warnings:?}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<Error> for RenderError {
    fn from(error: Error) -> RenderError { RenderError::Load(error) }
}

impl fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TraceError::Load(error) => write!(formatter, "could not load S3M module: {error}"),
            TraceError::TickLimit => write!(formatter, "trace exceeded the safety limit of {MAX_CAPTURE_TICKS} ticks"),
        }
    }
}

impl std::error::Error for TraceError {}

impl From<Error> for TraceError {
    fn from(error: Error) -> TraceError { TraceError::Load(error) }
}

type TraceEngine = Engine<FixedPath, Linear, StereoI16, Arc<Module>>;

/// Render the canonical fixed-path segment at an arbitrary host block size.
///
/// The path is exactly the filename contract: fixed-point voice accumulation, linear
/// interpolation, mono `i16`, 44.1 kHz, no dither, and the nonlinear master limiter
/// bypassed in favour of a transparent clamp. Per-channel DSP is currently a no-op; when
/// the graph lands, this entry point remains its explicit bypass boundary.
pub fn render_s3m_fixed_mono(bytes: &[u8], host_block_frames: usize) -> Result<Vec<i16>, RenderError> {
    render_s3m::<FixedPath, MonoI16>(bytes, GOLDEN_RENDER_FRAMES, host_block_frames)
}

/// Render the float path with the same rate, duration, interpolation, mono fold-down and
/// DSP bypass as [`render_s3m_fixed_mono`].
pub fn render_s3m_float_mono(bytes: &[u8], host_block_frames: usize) -> Result<Vec<f32>, RenderError> {
    render_s3m::<FloatPath, MonoF32>(bytes, GOLDEN_RENDER_FRAMES, host_block_frames)
}

/// SHA-256 over the canonical samples encoded as little-endian signed PCM words.
///
/// Hashing an explicit byte order, rather than the in-memory representation of `i16`, is
/// what lets x86-64, aarch64 and wasm32 compare the same digest.
pub fn canonical_s3m_sha256(bytes: &[u8], host_block_frames: usize) -> Result<[u8; 32], RenderError> {
    let samples = render_s3m_fixed_mono(bytes, host_block_frames)?;
    let mut hasher = Sha256::new();
    for sample in samples {
        hasher.update(sample.to_le_bytes());
    }
    let digest = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);
    Ok(hash)
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

fn render_s3m<Path, Out>(bytes: &[u8], frames: usize, host_block_frames: usize) -> Result<Vec<Out::Sample>, RenderError>
where
    Path: MixPath,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let module = Arc::new(starplayer::s3m::load(bytes)?);
    let channel_count = module.header().channel_count as usize;
    let settings = EngineSettings {
        sample_rate_hz: GOLDEN_SAMPLE_RATE_HZ,
        channel_count,
        voice_capacity: channel_count.max(1),
        ..EngineSettings::default()
    };
    let mut engine: Engine<Path, Linear, Out, Arc<Module>> = Engine::with_settings(settings);
    let mut control = engine.take_control().ok_or(RenderError::CommandQueue)?;
    control.load_module(Arc::clone(&module)).map_err(|_| RenderError::CommandQueue)?;
    engine.set_source(Box::new(starplayer::s3m::sequencer_for(module, GOLDEN_SAMPLE_RATE_HZ, ExactFixedPoint)));
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

/// Load an S3M and capture its stable per-tick trace.
///
/// M1 has one production loader/effect processor, S3M. MOD and MTM route through the same
/// entry point when their native processors land in C3/C4; no format is lowered to S3M.
pub fn trace_s3m(bytes: &[u8], options: TraceOptions) -> Result<Trace, TraceError> {
    let module = Arc::new(starplayer::s3m::load(bytes)?);
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
    let sequencer = PatternSequencer::new(
        ExactFixedPoint,
        starplayer::s3m::S3mPatternData(Arc::clone(&module)),
        starplayer::s3m::S3mProcessor::new(module, TRACE_SAMPLE_RATE_HZ),
        sequencer_settings,
    );
    engine.set_source(Box::new(sequencer));

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

    fn trace_with_block_size(host_block_frames: usize) -> Trace {
        trace_s3m(REFLEX, TraceOptions { ticks: Some(24), host_block_frames }).expect("REFLEX traces")
    }

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
}
