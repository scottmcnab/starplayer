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

use starplayer::core::quirks::{QuirkSelection, QuirkSet};
use starplayer::core::{AtEnd, Error};

/// Re-exported so a caller naming a golden's kernel — the goldens driver, a cross-target
/// check — does not have to depend on `starplayer-core` directly.
pub use starplayer::core::Interpolator;
use starplayer::dsp::{Cubic, InsertKind, Interpolate, Linear, Nearest, ParamId, Sinc, build_insert};
use starplayer::engine::{
    ChannelTable, EndReason, Engine, EngineSettings, EngineWarnings, EventSource, InsertCommand, InsertTarget,
    InstrumentRack, MidiSource, ScanLimits, SongTimeline,
};
use starplayer::mixer::{FixedPath, FloatPath, I24, Limiter, MixPath, MonoF32, MonoI16, OutputFormat};
use starplayer::model::{Module, ModuleFormat, SampleEnhancer};
use starplayer::rt::Arc;
use starplayer::{NativeSequencer, ScannedSong, recommended_voice_capacity, scan_song};

/// One insert to install before a [`render_song`] render begins (M7-H7), so the CLI's
/// `--insert` flag and a test share exactly the same shape rather than each building an
/// effect its own way.
///
/// Installed, and every parameter set, **before the first frame renders** — never mid-song
/// — so the effect is present from sample zero whatever host block size the render walks
/// in, which is what keeps a render with `inserts` non-empty just as
/// buffer-size-independent as one with none.
#[derive(Clone, Debug)]
pub struct InsertSpec {
    /// Which bus.
    pub target: InsertTarget,
    /// Which of the four ordered slots.
    pub slot: u8,
    /// Which effect.
    pub kind: InsertKind,
    /// Parameters to set after installing, away from the effect's own defaults.
    pub params: Vec<(ParamId, i32)>,
}

/// Everything a [`render_song_with_options`] render can be asked for beyond the plain
/// song: inserts installed before the first frame, and a load-time sample enhancer
/// (M10-K5a's `starplayer::enhance`) applied to the module before it plays.
///
/// The enhancer runs on the loaded [`Module`] right after [`load_golden`], before it is
/// wrapped in the `Arc` every render path shares — so the scan that produces
/// [`song_timeline`]'s timing sees the enhanced module and nothing downstream of the load
/// has to know an enhancer ran at all.
#[derive(Clone, Copy, Default)]
pub struct RenderOptions<'a> {
    /// Inserts to install before the first frame. See [`InsertSpec`].
    pub inserts: &'a [InsertSpec],
    /// A sample enhancer to rebuild the module through before playback. `None` renders the
    /// module exactly as loaded.
    pub enhancer: Option<&'a dyn SampleEnhancer>,
}

// The per-tick trace is a diagnostic build only. Everything it needs sits behind the
// `trace` feature, so an ordinary `cargo test --workspace` — and every golden render —
// compiles the engine without the recorder in its render path.
#[cfg(feature = "trace")]
use starplayer::core::Frame;
#[cfg(feature = "trace")]
use starplayer::engine::{EndOfSongPolicy, SequencerSettings, Trace};
#[cfg(test)]
use starplayer::core::ExactFixedPoint;
#[cfg(any(feature = "trace", test))]
use starplayer::mixer::StereoI16;
use sha2::{Digest, Sha256};

pub mod fixtures;
pub mod wav;

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
    /// Stop after this many ticks. `None` follows the module until its end marker — which
    /// an XM or IT capture reaches only if its order list does not wrap, because those two
    /// follow their oracle's wrap (`trace_sequencer_settings`); a wrapping capture runs to
    /// [`MAX_CAPTURE_TICKS`] and reports [`TraceError::TickLimit`].
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
    Xm,
    It,
}

impl GoldenFormat {
    /// Every format the golden contract covers.
    pub const ALL: [GoldenFormat; 5] = [GoldenFormat::Mod, GoldenFormat::S3m, GoldenFormat::Mtm, GoldenFormat::Xm, GoldenFormat::It];

    /// The `goldens/<format>/` directory this format's hashes live in.
    pub const fn directory(self) -> &'static str {
        match self {
            GoldenFormat::Mod => "mod",
            GoldenFormat::S3m => "s3m",
            GoldenFormat::Mtm => "mtm",
            GoldenFormat::Xm => "xm",
            GoldenFormat::It => "it",
        }
    }

    /// The loaded-module format this fixture format must decode to.
    ///
    /// The golden contract's promise is that a fixture cannot be rendered through the
    /// wrong processor. Since the facade autodetects, that promise is now this check:
    /// [`load_golden`] loads through [`starplayer::load`] and refuses a module whose header
    /// format is not this one.
    pub const fn module_format(self) -> ModuleFormat {
        match self {
            GoldenFormat::Mod => ModuleFormat::Mod,
            GoldenFormat::S3m => ModuleFormat::S3m,
            GoldenFormat::Mtm => ModuleFormat::Mtm,
            GoldenFormat::Xm => ModuleFormat::Xm,
            GoldenFormat::It => ModuleFormat::It,
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
    /// A golden names a kernel this build has no implementation for.
    ///
    /// Every [`Interpolator`] has one since M7-task-H5, so nothing constructs this today.
    /// It stays because a build that features a kernel out — an embedded target that
    /// wants neither cubic nor sinc — needs somewhere to say so that is not a panic.
    /// The filename would promise audio the renderer cannot produce, so it refuses.
    UnimplementedInterpolator(Interpolator),
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::Load(error) => write!(formatter, "could not load module: {error}"),
            RenderError::CommandQueue => write!(formatter, "fresh offline engine rejected its module command"),
            RenderError::EngineWarnings(warnings) => write!(formatter, "offline render raised engine warnings: {warnings:?}"),
            RenderError::UnimplementedInterpolator(kernel) => write!(formatter, "the golden names {kernel:?}, which has no kernel in this build"),
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
    render_fixed_mono_with(format, bytes, host_block_frames, GOLDEN_INTERPOLATOR)
}

/// [`render_fixed_mono`] on a named kernel rather than the canonical one.
///
/// M7-task-H5 added the cubic and sinc goldens as **cross-target pins**, not as a second
/// canonical render: linear is still the kernel [`GOLDEN_INTERPOLATOR`] names and the one
/// the accuracy policy talks about. Everything else about the render — rate, duration,
/// mono fold-down, limiter — is identical, which is what makes the extra hashes worth
/// having: the only variable between them is the kernel.
pub fn render_fixed_mono_with(format: GoldenFormat, bytes: &[u8], host_block_frames: usize, interpolator: Interpolator) -> Result<Vec<i16>, RenderError> {
    render_golden::<FixedPath, MonoI16>(format, bytes, GOLDEN_RENDER_FRAMES, host_block_frames, interpolator)
}

/// Render the float path with the same rate, duration, interpolation, mono fold-down and
/// DSP bypass as [`render_fixed_mono`].
pub fn render_float_mono(format: GoldenFormat, bytes: &[u8], host_block_frames: usize) -> Result<Vec<f32>, RenderError> {
    render_golden::<FloatPath, MonoF32>(format, bytes, GOLDEN_RENDER_FRAMES, host_block_frames, GOLDEN_INTERPOLATOR)
}

/// SHA-256 over the canonical samples encoded as little-endian signed PCM words.
///
/// Hashing an explicit byte order, rather than the in-memory representation of `i16`, is
/// what lets x86-64, aarch64 and wasm32 compare the same digest.
pub fn canonical_sha256(format: GoldenFormat, bytes: &[u8], host_block_frames: usize) -> Result<[u8; 32], RenderError> {
    canonical_sha256_with(format, bytes, host_block_frames, GOLDEN_INTERPOLATOR)
}

/// [`canonical_sha256`] on a named kernel, which is what writes the extra cubic and sinc
/// goldens. The kernel is part of the golden's filename, so a hash and the kernel that
/// produced it cannot come apart.
pub fn canonical_sha256_with(format: GoldenFormat, bytes: &[u8], host_block_frames: usize, interpolator: Interpolator) -> Result<[u8; 32], RenderError> {
    let samples = render_fixed_mono_with(format, bytes, host_block_frames, interpolator)?;
    let mut hasher = Sha256::new();
    for sample in samples {
        hasher.update(sample.to_le_bytes());
    }
    let digest = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);
    Ok(hash)
}

/// Scan a loaded module and report the shape of its song: every row it plays, when, how
/// long one pass is, and whether it loops or ends.
///
/// A thin wrapper over [`starplayer::scan_song`], which is what the browser host calls
/// too, so an offline length and a browser progress slider cannot disagree. The scan runs
/// a **throwaway** sequencer; callers build a second one to play, from the quirks
/// [`scanned_song`] hands back.
pub fn song_timeline(module: &Arc<Module>, sample_rate_hz: u32) -> Result<SongTimeline, RenderError> {
    song_timeline_with_limits(module, sample_rate_hz, ScanLimits::for_rate(sample_rate_hz))
}

/// [`song_timeline`] with explicit limits, for a test that wants a short leash.
pub fn song_timeline_with_limits(module: &Arc<Module>, sample_rate_hz: u32, limits: ScanLimits) -> Result<SongTimeline, RenderError> {
    Ok(scanned_song_with_limits(module, sample_rate_hz, limits)?.timeline)
}

/// The scan *and* the quirks it ran under — what every render path here builds its
/// playback sequencer from. A MOD's `mod_timing` is decided by this call and by nothing
/// else, so nothing renders with unknown timing.
pub fn scanned_song(module: &Arc<Module>, sample_rate_hz: u32) -> Result<ScannedSong, RenderError> {
    scanned_song_with_limits(module, sample_rate_hz, ScanLimits::for_rate(sample_rate_hz))
}

/// [`scanned_song`] with explicit limits.
pub fn scanned_song_with_limits(module: &Arc<Module>, sample_rate_hz: u32, limits: ScanLimits) -> Result<ScannedSong, RenderError> {
    scan_song(module, sample_rate_hz, limits).map_err(RenderError::Load)
}

/// How much of a song an offline render should produce.
///
/// A module does not say how long it is, so a file render has to be told. The default is
/// what a media player's "export" button wants: play the song once, then fade out over ten
/// seconds into the start of the second pass. A song that *ends* — its order list runs out
/// ([`EndReason::Ended`]) or a stop marker fires ([`EndReason::Stopped`]) — gets no fade,
/// because there is nothing to fade away from; only an explicit `Bxx`/`Cxx`/`Dxx` loop
/// does.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RenderLength {
    /// Extra passes through the repeating section after the first. Zero plays it once.
    pub repeat_count: u32,
    /// What happens at a **loop point**. [`AtEnd::Stop`] cuts dead there with no fade;
    /// anything else fades, because a file has to end somewhere. A song that ends rather
    /// than loops is cut whatever this says — there is nothing to fade away from.
    pub at_end: AtEnd,
    /// Frames the fade lasts. Ignored when there is no fade.
    pub fade_frames: u64,
    /// A hard ceiling, so a pathological module cannot ask for an unbounded file.
    pub max_frames: u64,
}

impl RenderLength {
    /// Once through, then a ten-second fade, capped at an hour.
    pub const fn default_for(sample_rate_hz: u32) -> RenderLength {
        RenderLength {
            repeat_count: 0,
            at_end: AtEnd::FadeOut,
            fade_frames: sample_rate_hz as u64 * 10,
            max_frames: sample_rate_hz as u64 * 3_600,
        }
    }

    /// How many frames this length asks for, and how many of them are fade, given a scanned
    /// timeline.
    pub fn frames_for(&self, timeline: &SongTimeline) -> (usize, usize) {
        let loop_length = match timeline.end() {
            // A song that ends has nothing to repeat, so a repeat plays it again from the
            // top; a budget end never found the loop point, so the cap stands in for it.
            EndReason::Ended | EndReason::Stopped | EndReason::Budget => timeline.end_frame(),
            EndReason::Looped { .. } => timeline.loop_length_frames().unwrap_or(timeline.end_frame()),
        };
        let body = timeline.end_frame().saturating_add(loop_length.saturating_mul(self.repeat_count as u64));
        let fade = match (self.at_end, timeline.end()) {
            // An order list that ran out and a stop marker are the same kind of end: the
            // song is over on its own terms, so there is nothing to fade away from.
            (AtEnd::Stop, _) | (_, EndReason::Ended | EndReason::Stopped) => 0,
            _ => self.fade_frames,
        };
        let total = body.saturating_add(fade).min(self.max_frames);
        // A fade cannot be longer than the render it is applied to.
        (total as usize, fade.min(total) as usize)
    }
}

/// One output sample, scalable by a Q16.16 gain. The fade is the only place this crate
/// touches sample values, and it does it without a single transcendental call.
pub trait FadeSample: Copy + Default {
    /// This sample at `gain_q16 / 65_536` of its value.
    fn scaled_q16(self, gain_q16: u32) -> Self;
}

impl FadeSample for i16 {
    fn scaled_q16(self, gain_q16: u32) -> i16 { ((self as i64 * gain_q16 as i64) >> 16) as i16 }
}

impl FadeSample for f32 {
    fn scaled_q16(self, gain_q16: u32) -> f32 { self * (gain_q16 as f32 / 65_536.0) }
}

/// The CLI's `--depth 24` render (task D5): the same Q16.16 integer scale as `i16`'s
/// impl, applied to `I24`'s inner value rather than reaching for a float.
impl FadeSample for I24 {
    fn scaled_q16(self, gain_q16: u32) -> I24 { I24(((self.0 as i64 * gain_q16 as i64) >> 16) as i32) }
}

/// The CLI's `--depth 32` render (task D5).
impl FadeSample for i32 {
    fn scaled_q16(self, gain_q16: u32) -> i32 { ((self as i64 * gain_q16 as i64) >> 16) as i32 }
}

/// Render a whole song rather than a fixed segment: scan it, play it for as long as
/// [`RenderLength`] asks, and fade the tail.
///
/// The scan and the playback are two separate sequencers over the same `Arc<Module>`, so
/// nothing depends on `TrackerProcessor::reset` restoring every last bit of a processor's
/// state. The playback sequencer runs with [`AtEnd::Continue`] whatever the caller asked
/// for: the length decides where the render stops, and the fade decides how it ends.
pub fn render_song<Path, Interp, Out>(
    format: GoldenFormat,
    bytes: &[u8],
    sample_rate_hz: u32,
    host_block_frames: usize,
    length: RenderLength,
) -> Result<Vec<Out::Sample>, RenderError>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Out::Sample: FadeSample,
{
    render_song_with_options::<Path, Interp, Out>(format, bytes, sample_rate_hz, host_block_frames, length, &RenderOptions::default())
}

/// [`render_song`] with insert effects installed before the first frame renders (M7-H7):
/// the CLI's `--insert` flag and the offline exit-criterion tests share this entry point
/// rather than each poking at `Engine::take_insert_control` its own way.
pub fn render_song_with_inserts<Path, Interp, Out>(
    format: GoldenFormat,
    bytes: &[u8],
    sample_rate_hz: u32,
    host_block_frames: usize,
    length: RenderLength,
    inserts: &[InsertSpec],
) -> Result<Vec<Out::Sample>, RenderError>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Out::Sample: FadeSample,
{
    render_song_with_options::<Path, Interp, Out>(format, bytes, sample_rate_hz, host_block_frames, length, &RenderOptions { inserts, enhancer: None })
}

/// [`render_song`] with the full [`RenderOptions`]: inserts, and optionally a load-time
/// sample enhancer (M10-K5b). `render_song` and `render_song_with_inserts` both delegate
/// here.
///
/// The enhancer, if any, rebuilds the loaded module **before** it is wrapped in the `Arc`
/// every render path shares, so [`song_timeline`]'s scan — and everything the fade and the
/// playback sequencer derive from it — sees exactly the module that plays.
pub fn render_song_with_options<Path, Interp, Out>(
    format: GoldenFormat,
    bytes: &[u8],
    sample_rate_hz: u32,
    host_block_frames: usize,
    length: RenderLength,
    options: &RenderOptions<'_>,
) -> Result<Vec<Out::Sample>, RenderError>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
    Out::Sample: FadeSample,
{
    let loaded = load_golden(format, bytes)?;
    let loaded = match options.enhancer {
        Some(enhancer) => loaded.enhanced(enhancer)?,
        None => loaded,
    };
    let module = Arc::new(loaded);
    let scanned = scanned_song(&module, sample_rate_hz)?;
    let (total_frames, fade_frames) = length.frames_for(&scanned.timeline);

    let settings = EngineSettings {
        sample_rate_hz,
        channel_count: module.header().channel_count as usize,
        voice_capacity: recommended_voice_capacity(&module).max(1),
        ..EngineSettings::default()
    };
    let mut engine: Engine<Path, Interp, Out, Arc<Module>> = Engine::with_settings(settings);
    let mut control = engine.take_control().ok_or(RenderError::CommandQueue)?;
    control.load_module(Arc::clone(&module)).map_err(|_| RenderError::CommandQueue)?;
    engine.set_source(playback_source(module, sample_rate_hz, scanned)?);
    engine.set_limiter(Limiter::Clamp);

    if !options.inserts.is_empty() {
        let mut insert_control = engine.take_insert_control().ok_or(RenderError::CommandQueue)?;
        for spec in options.inserts {
            let boxed = build_insert::<Path::Mono>(spec.kind, sample_rate_hz);
            insert_control.install(spec.target, spec.slot, boxed).map_err(|_| RenderError::CommandQueue)?;
            for &(param, value) in &spec.params {
                insert_control
                    .send(InsertCommand::SetParam { target: spec.target, slot: spec.slot, param, value })
                    .map_err(|_| RenderError::CommandQueue)?;
            }
        }
        // Dropped here rather than held: nothing after this point installs, removes or
        // parameterises another insert, so there is nothing left for the control side to
        // do — and holding it would only postpone the retired-insert collection this
        // render never produces any of anyway.
    }

    let output_samples = total_frames.saturating_mul(Out::CHANNELS);
    let block_samples = host_block_frames.max(1).saturating_mul(Out::CHANNELS).max(Out::CHANNELS);
    let mut output = vec![Out::Sample::default(); output_samples];
    for block in output.chunks_mut(block_samples) {
        engine.render(block);
    }
    let warnings = engine.warnings();
    if warnings.any() {
        return Err(RenderError::EngineWarnings(warnings));
    }

    apply_fade::<Out>(&mut output, total_frames, fade_frames);
    Ok(output)
}

/// The `i16` mono spelling, mirroring [`render_fixed_mono`].
pub fn render_song_fixed_mono(format: GoldenFormat, bytes: &[u8], sample_rate_hz: u32, host_block_frames: usize, length: RenderLength) -> Result<Vec<i16>, RenderError> {
    render_song::<FixedPath, Linear, MonoI16>(format, bytes, sample_rate_hz, host_block_frames, length)
}

/// Render a Standard MIDI File (task E5 deliverable 4), through the instruments of a
/// loaded module, from tick zero to the file's own length — [`Smf::length_frames`]
/// (`starplayer::midi::Smf`), the latest `end_of_track` tick across every track. Unlike
/// [`render_song`] an SMF carries no loop point of its own, so there is nothing for a
/// [`RenderLength`] to compute: the render is exactly the file's length, once, with no
/// fade.
///
/// `instruments_module_bytes` is any module this build can load; program numbers on the
/// file's sixteen MIDI channels index its instruments (master-plan decision 5). Building
/// the [`InstrumentRack`] and the playback [`Engine`] both happen here, the same shape
/// `tests/render_allocation.rs`'s `rendering_a_midi_source_allocates_nothing` proved
/// allocates nothing once `render()` is on the stack.
pub fn render_smf_song<Path, Interp, Out>(
    smf_bytes: &[u8],
    instruments_module_bytes: &[u8],
    sample_rate_hz: u32,
    host_block_frames: usize,
) -> Result<Vec<Out::Sample>, RenderError>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let smf = starplayer::midi::smf::parse_smf(smf_bytes).map_err(RenderError::Load)?;
    let module = Arc::new(starplayer::load(instruments_module_bytes)?);
    let sequencer = starplayer::midi::SmfSequencer::new(&smf, sample_rate_hz);
    let total_frames = sequencer.length_frames() as usize;

    let settings = EngineSettings {
        sample_rate_hz,
        // The MIDI lanes sit above a module's own (master-plan decision 3), so the
        // channel table has to be the full width regardless of the instruments module's
        // own channel count.
        channel_count: ChannelTable::MAX_CHANNELS,
        voice_capacity: recommended_voice_capacity(&module).max(16),
        ..EngineSettings::default()
    };
    let mut engine: Engine<Path, Interp, Out, Arc<Module>> = Engine::with_settings(settings);
    let mut control = engine.take_control().ok_or(RenderError::CommandQueue)?;
    control.load_module(Arc::clone(&module)).map_err(|_| RenderError::CommandQueue)?;
    let rack = InstrumentRack::for_module(&module, sample_rate_hz);
    engine.set_source(Box::new(MidiSource::new(sequencer, rack, sample_rate_hz)));
    engine.set_limiter(Limiter::Clamp);

    let output_samples = total_frames.saturating_mul(Out::CHANNELS);
    let block_samples = host_block_frames.max(1).saturating_mul(Out::CHANNELS).max(Out::CHANNELS);
    let mut output = vec![Out::Sample::default(); output_samples];
    for block in output.chunks_mut(block_samples) {
        engine.render(block);
    }
    let warnings = engine.warnings();
    if warnings.any() {
        return Err(RenderError::EngineWarnings(warnings));
    }
    Ok(output)
}

/// The `i16` mono spelling of [`render_smf_song`], mirroring [`render_song_fixed_mono`].
pub fn render_smf_song_fixed_mono(smf_bytes: &[u8], instruments_module_bytes: &[u8], sample_rate_hz: u32, host_block_frames: usize) -> Result<Vec<i16>, RenderError> {
    render_smf_song::<FixedPath, Linear, MonoI16>(smf_bytes, instruments_module_bytes, sample_rate_hz, host_block_frames)
}

/// A linear fade over the last `fade_frames` frames, in Q16.16 with no floating-point
/// curve and no transcendental call. The last frame is exactly zero.
fn apply_fade<Out>(output: &mut [Out::Sample], total_frames: usize, fade_frames: usize)
where
    Out: OutputFormat,
    Out::Sample: FadeSample,
{
    if fade_frames == 0 || total_frames == 0 {
        return;
    }
    let first_faded = total_frames.saturating_sub(fade_frames);
    for frame in first_faded..total_frames {
        let remaining = (total_frames - 1 - frame) as u64;
        let gain_q16 = (remaining * 65_536 / fade_frames as u64) as u32;
        let start = frame.saturating_mul(Out::CHANNELS);
        let Some(samples) = output.get_mut(start..start + Out::CHANNELS) else { continue };
        for sample in samples {
            *sample = sample.scaled_q16(gain_q16);
        }
    }
}

/// A playback sequencer for the module's own format with the scan's timeline installed,
/// the scan's own quirks resolved into it, and the loop point set to wrap rather than stop.
fn playback_source(module: Arc<Module>, sample_rate_hz: u32, scanned: ScannedSong) -> Result<Box<dyn EventSource>, RenderError> {
    let ScannedSong { timeline, quirks } = scanned;
    let mut sequencer = NativeSequencer::new(module, sample_rate_hz, QuirkSelection::Override(quirks)).map_err(RenderError::Load)?;
    sequencer.set_timeline(timeline);
    sequencer.set_at_end(AtEnd::Continue);
    Ok(Box::new(sequencer))
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
    golden_filename_for_configuration(module_stem, GOLDEN_INTERPOLATOR, None)
}

/// The configuration filename an alternate interpolator uses.
///
/// Linear is still the canonical kernel; the cubic and sinc goldens M7-task-H5 added for
/// `reflex.s3m` are cross-target pins beside it. This mapping is what keeps the two
/// apart: selecting another kernel names a different file rather than comparing its bytes
/// against the linear hash.
pub fn golden_filename_for_interpolator(module_stem: &str, interpolator: Interpolator) -> String {
    golden_filename_for_configuration(module_stem, interpolator, None)
}

/// The filename one rendered configuration hashes under: the module stem, the fixed
/// `i16_mono_44100` shape every golden shares, the interpolator, and — when a load-time
/// sample enhancer (M10-K5a) ran — its name behind an `_enh-` marker.
///
/// An enhanced render is a **different configuration** from the plain goldens
/// (`plans/product/03-accuracy-policy.md` §5 item 5), so it must never collide with one:
/// `stem__i16_mono_44100_<kernel>.sha256` when `enhancer_name` is `None`, else
/// `stem__i16_mono_44100_<kernel>_enh-<name>.sha256`. [`golden_filename`] and
/// [`golden_filename_for_interpolator`] both delegate here with `None`. No enhanced golden
/// is committed; this function only names what a future one would be called.
pub fn golden_filename_for_configuration(module_stem: &str, interpolator: Interpolator, enhancer_name: Option<&str>) -> String {
    let label = match interpolator {
        Interpolator::None => "nearest",
        Interpolator::Linear => "linear",
        Interpolator::Cubic => "cubic",
        Interpolator::Sinc => "sinc",
    };
    match enhancer_name {
        None => format!("{module_stem}__i16_mono_44100_{label}.sha256"),
        Some(name) => format!("{module_stem}__i16_mono_44100_{label}_enh-{name}.sha256"),
    }
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

/// Dispatch a golden render on the [`Interpolator`] its filename names.
///
/// This `match` is the whole point of naming the kernel in the filename: the same value
/// that picks the file picks the code that produces its bytes, so the two cannot
/// disagree. Adding a kernel to `starplayer-dsp` means adding one arm here, and nothing
/// else in this crate may name an interpolator type.
fn render_golden<Path, Out>(format: GoldenFormat, bytes: &[u8], frames: usize, host_block_frames: usize, interpolator: Interpolator) -> Result<Vec<Out::Sample>, RenderError>
where
    Path: MixPath,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    match interpolator {
        Interpolator::None => render_with_kernel::<Path, Nearest, Out>(format, bytes, frames, host_block_frames),
        Interpolator::Linear => render_with_kernel::<Path, Linear, Out>(format, bytes, frames, host_block_frames),
        Interpolator::Cubic => render_with_kernel::<Path, Cubic, Out>(format, bytes, frames, host_block_frames),
        Interpolator::Sinc => render_with_kernel::<Path, Sinc, Out>(format, bytes, frames, host_block_frames),
    }
}

fn render_with_kernel<Path, Interp, Out>(format: GoldenFormat, bytes: &[u8], frames: usize, host_block_frames: usize) -> Result<Vec<Out::Sample>, RenderError>
where
    Path: MixPath,
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = Path::Accumulator>,
{
    let module = Arc::new(load_golden(format, bytes)?);
    let settings = EngineSettings {
        sample_rate_hz: GOLDEN_SAMPLE_RATE_HZ,
        channel_count: module.header().channel_count as usize,
        voice_capacity: recommended_voice_capacity(&module).max(1),
        ..EngineSettings::default()
    };
    let mut engine: Engine<Path, Interp, Out, Arc<Module>> = Engine::with_settings(settings);
    let mut control = engine.take_control().ok_or(RenderError::CommandQueue)?;
    control.load_module(Arc::clone(&module)).map_err(|_| RenderError::CommandQueue)?;
    let quirks = scanned_song(&module, GOLDEN_SAMPLE_RATE_HZ)?.quirks;
    engine.set_source(golden_source(module, quirks)?);
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

/// Load a fixture through the facade's autodetection, then check it is the format the
/// caller named.
///
/// The check is what keeps [`GoldenFormat`]'s promise now that the loader is shared: a
/// fixture filed under `goldens/mtm/` that probes as a MOD is a corpus mistake, and it
/// reports one rather than quietly hashing the wrong processor's output.
fn load_golden(format: GoldenFormat, bytes: &[u8]) -> Result<Module, Error> {
    let module = starplayer::load(bytes)?;
    if module.header().format == format.module_format() { Ok(module) } else { Err(Error::BadMagic) }
}

/// The canonical golden render's source. It takes the scan's quirks like every other
/// render path here: the fixed-length hash is a regression contract, and a MOD whose
/// timing the scan resolves differently must be hashed the way it would be played.
fn golden_source(module: Arc<Module>, quirks: QuirkSet) -> Result<Box<dyn EventSource>, RenderError> {
    let sequencer = NativeSequencer::new(module, GOLDEN_SAMPLE_RATE_HZ, QuirkSelection::Override(quirks)).map_err(RenderError::Load)?;
    Ok(Box::new(sequencer))
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

/// Load an XM and capture its stable per-tick trace through the FastTracker 2 processor.
#[cfg(feature = "trace")]
pub fn trace_xm(bytes: &[u8], options: TraceOptions) -> Result<Trace, TraceError> {
    trace_loaded(Arc::new(starplayer::xm::load(bytes)?), options)
}

/// Capture the per-tick trace of a module that is **already loaded**.
///
/// The entry point for anything that transforms a module before playing it — M10-K5a's
/// sample enhancers, whose proof is that an enhanced module's traced positions are the
/// plain module's shifted by the enhancement's own factor, and nothing else in the trace
/// moves at all.
#[cfg(feature = "trace")]
pub fn trace_loaded_module(module: Arc<Module>, options: TraceOptions) -> Result<Trace, TraceError> {
    trace_loaded(module, options)
}

#[cfg(feature = "trace")]
fn trace_loaded(module: Arc<Module>, options: TraceOptions) -> Result<Trace, TraceError> {
    let engine_settings = EngineSettings {
        sample_rate_hz: TRACE_SAMPLE_RATE_HZ,
        channel_count: module.header().channel_count as usize,
        voice_capacity: recommended_voice_capacity(&module).max(1),
        ..EngineSettings::default()
    };
    let mut engine: TraceEngine = Engine::with_settings(engine_settings);
    let mut control = engine.take_control().expect("a fresh offline engine owns its control handle");
    control.load_module(Arc::clone(&module)).map_err(|_| TraceError::Load(Error::Invalid("module command queue is full")))?;

    let sequencer_settings = trace_sequencer_settings(&module);
    let sequencer = NativeSequencer::with_settings(module, QuirkSelection::FromDialect, sequencer_settings)
        .map_err(|_| TraceError::Load(Error::Invalid("format has no offline native processor")))?;
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

/// The end-of-song rule a capture follows — the **oracle's** rule, per format, rather than
/// the player's.
///
/// A trace exists to be compared against another player's dump, so it has to end where
/// that player ends. Task D2 gave the *player* a different rule — running out of order
/// list is the end of the song, and the loop detector plus
/// [`AtEnd`](starplayer::core::AtEnd) decide what happens next — and nothing here touches
/// it: this is the trace path alone, and the scan, the goldens and every host still build
/// their sequencers through each format crate's own `sequencer_settings`.
///
/// | Format | Rule | Reference |
/// |---|---|---|
/// | MOD, S3M, MTM | the order list running out ends the capture | unchanged; the 47 pinned cases of those three formats are measured against exactly this |
/// | XM | wrap to the header's restart position | `ft2_replayer.c`'s `getNextPos`, `if (++song.songPos >= song.songLength) song.songPos = song.songLoopStart;` — and libxmp's dumps for `PatLoop-Break.xm` and `PatLoop-Weird.xm` record three passes of that wrap |
/// | IT | wrap to order zero, past the `0xFF` end marker | `ITTECH.TXT`: the order list ends at `255` and Impulse Tracker restarts from the top, which is what OpenMPT and libxmp both do |
///
/// A wrapping capture no longer stops on its own, so a caller that asked for
/// [`TraceOptions::ticks`] `None` runs to [`MAX_CAPTURE_TICKS`] and reports
/// [`TraceError::TickLimit`] — the same outcome any module that jumps backwards has always
/// had. The conformance harness always names a tick budget.
#[cfg(feature = "trace")]
fn trace_sequencer_settings(module: &Module) -> SequencerSettings {
    let (end_of_song, restart_order) = match module.header().format {
        // `PatternSequencer::new` starts the song at `restart_order`, so this also says
        // where the capture begins. Every XM in the pinned corpus has a restart position
        // of zero, and the XM *player* already reads the same field into the same setting
        // (`starplayer_xm::processor::sequencer_settings`), so the trace and the player
        // agree about a non-zero one rather than disagreeing.
        ModuleFormat::Xm => (EndOfSongPolicy::WrapKeepingBreakRow, starplayer::xm::XmFormatExtra::from_header(module.header()).restart_position),
        ModuleFormat::It => (EndOfSongPolicy::WrapKeepingBreakRow, 0),
        _ => (EndOfSongPolicy::Stop, 0),
    };
    SequencerSettings {
        sample_rate_hz: TRACE_SAMPLE_RATE_HZ,
        first_tick_frame: Frame::ZERO,
        initial_speed: module.header().initial_speed,
        initial_tempo_bpm: module.header().initial_tempo,
        restart_order,
        end_of_song,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFLEX: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");
    const PETRI: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/PETRI.S3M");
    const GOLDEN_CORPUS: &[(&str, &[u8])] = &[
        ("PETRI.S3M", PETRI),
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
            voice_capacity: recommended_voice_capacity(&module).max(1),
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

    /// M10-K5b: an enhanced configuration's filename pins visibly apart from a plain one's,
    /// so a later pin of an enhanced golden cannot collide with an existing plain one.
    #[test]
    fn golden_filename_for_configuration_names_an_enhanced_configuration_separately() {
        assert_eq!(golden_filename_for_configuration("reflex", Interpolator::Linear, None), "reflex__i16_mono_44100_linear.sha256");
        assert_eq!(golden_filename_for_configuration("reflex", Interpolator::Linear, None), golden_filename("reflex"));
        assert_eq!(
            golden_filename_for_configuration("reflex", Interpolator::Linear, Some("sinc4x+loop=64")),
            "reflex__i16_mono_44100_linear_enh-sinc4x+loop=64.sha256"
        );
        assert_eq!(
            golden_filename_for_configuration("reflex", Interpolator::None, Some("sinc2x")),
            "reflex__i16_mono_44100_nearest_enh-sinc2x.sha256"
        );
    }

    /// The block-size determinism invariant, with **Impulse Tracker's New Note Actions
    /// active**.
    ///
    /// `starplayer-engine`'s own `block_size_determinism` test cannot reach a format crate
    /// — the dependency runs the other way — so the IT half of the invariant lives here,
    /// where the facade is available. `fixtures::synthetic_it()`'s channel zero retriggers
    /// a `Continue` instrument every four rows, so background voices accumulate and are
    /// stolen, and every one of them has to advance identically at every host block size.
    #[test]
    fn the_synthetic_it_renders_identically_at_every_host_block_size_with_nna_active() {
        let bytes = fixtures::synthetic_it();
        let reference = render_fixed_mono(GoldenFormat::It, &bytes, GOLDEN_HOST_BLOCK_FRAMES).expect("the synthetic IT renders");
        assert!(reference.iter().any(|sample| *sample != 0), "the fixture has to actually make sound");
        let reference_hash = canonical_sha256(GoldenFormat::It, &bytes, GOLDEN_HOST_BLOCK_FRAMES).expect("the synthetic IT hashes");

        // A `Continue` New Note Action really did leave voices sounding behind their
        // channel: without that this would only be testing one voice per channel.
        let module = Arc::new(starplayer::it::load(&bytes).expect("the synthetic IT loads"));
        assert_eq!(recommended_voice_capacity(&module), starplayer::it::VIRTUAL_CHANNELS);
        assert!(peak_voices_of(&module) > module.header().channel_count as usize, "the fixture reaches more voices than it has channels");

        for host_block_frames in [1, 3, 64, 128, 4096, 8191] {
            assert_eq!(
                render_fixed_mono(GoldenFormat::It, &bytes, host_block_frames).expect("the synthetic IT renders"),
                reference,
                "host block size {host_block_frames} changed the IT PCM",
            );
            assert_eq!(
                canonical_sha256(GoldenFormat::It, &bytes, host_block_frames).expect("the synthetic IT hashes"),
                reference_hash,
                "host block size {host_block_frames} changed the IT hash",
            );
        }
    }

    /// The largest number of voices sounding at once while `module` plays, foreground and
    /// New Note Action background alike.
    fn peak_voices_of(module: &Arc<Module>) -> usize {
        use starplayer::core::Frame;
        use starplayer::engine::{ChannelTable, ControlClock, EngineContext, EventSource};
        use starplayer::mixer::VoicePool;
        let quirks = QuirkSelection::Override(scanned_song(module, 44_100).expect("it scans").quirks);
        let mut sequencer = starplayer::it::sequencer_with_quirks(Arc::clone(module), 44_100, quirks);
        let mut voices = VoicePool::new(starplayer::it::VIRTUAL_CHANNELS);
        let mut channels = ChannelTable::new((module.header().channel_count as usize).max(1));
        let mut control = ControlClock::new(44_100, Frame::ZERO);
        let mut peak = 0usize;
        for _ in 0..20_000u32 {
            let Some(frame) = sequencer.next_event_frame() else { break };
            let mut context = EngineContext::new(frame, &mut voices, &mut channels, &mut control);
            sequencer.dispatch(frame, &mut context);
            peak = peak.max(voices.voices_active());
        }
        peak
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
        let synthetic_xm = fixtures::synthetic_xm();
        let synthetic_it = fixtures::synthetic_it();
        let corpus: &[(GoldenFormat, &str, &[u8])] = &[
            (GoldenFormat::Mod, "synthetic", &synthetic_mod),
            (GoldenFormat::S3m, "REFLEX.S3M", REFLEX),
            (GoldenFormat::Mtm, "synthetic", &synthetic_mtm),
            (GoldenFormat::Xm, "synthetic", &synthetic_xm),
            (GoldenFormat::It, "synthetic", &synthetic_it),
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

    /// A short render, so the six block sizes stay quick: five seconds of song with a
    /// one-second tail.
    fn short_length(sample_rate_hz: u32) -> RenderLength {
        RenderLength {
            repeat_count: 0,
            at_end: AtEnd::FadeOut,
            fade_frames: sample_rate_hz as u64,
            max_frames: sample_rate_hz as u64 * 5,
        }
    }

    /// The convenience that sizes an engine from a module header must agree with the
    /// processor that will actually play it — that is the whole point of it, and the two
    /// live in different crates, so nothing but a test keeps them together.
    #[test]
    fn the_facade_voice_capacity_convenience_agrees_with_the_processor_that_plays_the_module() {
        for (format, name, bytes) in song_corpus() {
            let module = Arc::new(load_golden(format, &bytes).expect("the fixture loads"));
            let quirks = QuirkSelection::Override(scanned_song(&module, GOLDEN_SAMPLE_RATE_HZ).expect("the fixture scans").quirks);
            let sequencer = NativeSequencer::new(Arc::clone(&module), GOLDEN_SAMPLE_RATE_HZ, quirks).expect("the fixture has a processor");
            assert_eq!(recommended_voice_capacity(&module), sequencer.recommended_voice_capacity(), "{format} {name}");
            assert_eq!(sequencer.format(), format.module_format(), "{format} {name}");
        }
    }

    fn song_corpus() -> Vec<(GoldenFormat, &'static str, Vec<u8>)> {
        vec![
            (GoldenFormat::Mod, "synthetic", fixtures::synthetic_mod()),
            (GoldenFormat::S3m, "REFLEX.S3M", REFLEX.to_vec()),
            (GoldenFormat::Mtm, "synthetic", fixtures::synthetic_mtm()),
            (GoldenFormat::Xm, "synthetic", fixtures::synthetic_xm()),
            (GoldenFormat::It, "synthetic", fixtures::synthetic_it()),
        ]
    }

    /// Play a real sequencer to a stop with no mixing, and report the frame of the last
    /// tick it dispatched. This is the live dispatch path — the same `begin_row` the engine
    /// runs — so agreeing with the scan is a real claim about playback, not about a copy of
    /// it.
    /// A frame far enough into the engine's monotonic timeline that seeking back to the
    /// start of a song still leaves a positive origin.
    const SEEK_TEST_ORIGIN: u64 = 1 << 40;

    fn last_live_tick_frame(format: GoldenFormat, module: &Arc<Module>, sample_rate_hz: u32, scanned: &ScannedSong, seek_to: Option<u64>) -> Option<u64> {
        use starplayer::core::Frame;
        use starplayer::engine::{ChannelTable, ControlClock, EngineContext, PatternData};
        use starplayer::mixer::VoicePool;
        let timeline = &scanned.timeline;

        macro_rules! drive {
            ($sequencer:expr) => {{
                let mut sequencer = $sequencer;
                sequencer.set_timeline(timeline.clone());
                sequencer.set_at_end(AtEnd::Stop);
                if let Some(song_frame) = seek_to {
                    // Well clear of the song's own length, so the rebased origin is a
                    // subtraction rather than a saturation and `song_frame` stays
                    // comparable with the timeline's frames.
                    let now = Frame(SEEK_TEST_ORIGIN);
                    sequencer.seek_frame(song_frame, now)?;
                    sequencer.restart_clock_at(now);
                }
                let channel_count = (sequencer.data().channel_count() as usize).max(1);
                let mut voices = VoicePool::new(channel_count);
                let mut channels = ChannelTable::new(channel_count);
                let mut control = ControlClock::new(sample_rate_hz, Frame::ZERO);
                let mut last = None;
                for _ in 0..2_000_000u32 {
                    let Some(frame) = sequencer.next_event_frame() else { break };
                    let mut context = EngineContext::new(frame, &mut voices, &mut channels, &mut control);
                    sequencer.dispatch(frame, &mut context);
                    last = Some(sequencer.song_frame(frame));
                }
                last
            }};
        }

        // The playback sequencer is built from the scan's own quirks, exactly as every
        // host does it, so the live run and the scan cannot disagree about `mod_timing`.
        let quirks = QuirkSelection::Override(scanned.quirks);
        let module = Arc::clone(module);
        match format {
            GoldenFormat::Mod => drive!(starplayer::mod_file::sequencer_with_quirks(module, sample_rate_hz, quirks)),
            GoldenFormat::S3m => drive!(starplayer::s3m::sequencer_with_quirks(module, sample_rate_hz, quirks)),
            GoldenFormat::Mtm => drive!(starplayer::mtm::sequencer_with_quirks(module, sample_rate_hz, quirks)),
            GoldenFormat::Xm => drive!(starplayer::xm::sequencer_with_quirks(module, sample_rate_hz, quirks)),
            GoldenFormat::It => drive!(starplayer::it::sequencer_with_quirks(module, sample_rate_hz, quirks)),
        }
    }

    #[test]
    fn a_scanned_timeline_describes_a_song_the_live_sequencer_plays_the_same_way() {
        for (format, name, bytes) in song_corpus() {
            let module = Arc::new(load_golden(format, &bytes).expect("the fixture loads"));
            let scanned = scanned_song(&module, GOLDEN_SAMPLE_RATE_HZ).expect("the fixture scans");
            let timeline = &scanned.timeline;
            assert!(timeline.end_frame() > 0, "{format} {name}: a playable module is longer than nothing");
            assert!(!timeline.marks().is_empty(), "{format} {name}: the scan recorded rows");

            let live = last_live_tick_frame(format, &module, GOLDEN_SAMPLE_RATE_HZ, &scanned, None);
            assert_eq!(live, Some(timeline.end_frame()), "{format} {name}: the live detector fired somewhere else");

            // …and again from halfway in, which is what a progress-slider drag does.
            let halfway = timeline.end_frame() / 2;
            let seeked = last_live_tick_frame(format, &module, GOLDEN_SAMPLE_RATE_HZ, &scanned, Some(halfway));
            assert_eq!(seeked, Some(timeline.end_frame()), "{format} {name}: the loop point moved after a seek");
        }
    }

    #[test]
    fn scanning_the_same_module_twice_gives_the_same_timeline() {
        let module = Arc::new(starplayer::s3m::load(REFLEX).expect("REFLEX loads"));
        let first = song_timeline(&module, GOLDEN_SAMPLE_RATE_HZ).expect("REFLEX scans");
        let second = song_timeline(&module, GOLDEN_SAMPLE_RATE_HZ).expect("REFLEX scans twice");
        assert_eq!(first, second, "the scan is deterministic");
    }

    #[test]
    fn a_scanned_song_is_rendered_identically_at_every_host_block_size() {
        for (format, name, bytes) in song_corpus() {
            let length = short_length(GOLDEN_SAMPLE_RATE_HZ);
            let reference = render_song_fixed_mono(format, &bytes, GOLDEN_SAMPLE_RATE_HZ, GOLDEN_HOST_BLOCK_FRAMES, length).expect("the fixture renders");
            assert!(reference.iter().any(|sample| *sample != 0), "{format} {name}: the render is not silent");
            for host_block_frames in [1, 3, 64, 128, 4096, 8191] {
                let rendered = render_song_fixed_mono(format, &bytes, GOLDEN_SAMPLE_RATE_HZ, host_block_frames, length).expect("the fixture renders");
                assert_eq!(rendered, reference, "{format} {name}: host block size {host_block_frames} changed the song render");
            }
        }
    }

    /// The synthetic MOD's order list simply runs out, which task D2 reads as the end of
    /// the song rather than a loop. This is the same fixture with a `B00` on its very last
    /// row, so it genuinely repeats — and at exactly the same frame, which is what makes
    /// the two renders below comparable.
    fn looping_synthetic_mod() -> Vec<u8> {
        const HEADER_BYTES: usize = 1084;
        const CELL_BYTES: usize = 4;
        const CHANNELS: usize = 4;
        const ROWS: usize = 64;
        let mut bytes = fixtures::synthetic_mod();
        let last_cell = HEADER_BYTES + ((ROWS + ROWS - 1) * CHANNELS) * CELL_BYTES;
        let jump = starplayer::mod_file::ModCell { period: 0, instrument: 0, effect: 0xB, param: 0x00 };
        bytes[last_cell..last_cell + CELL_BYTES].copy_from_slice(&jump.to_bytes());
        bytes
    }

    /// Task D2: a song whose order list merely runs out is rendered once, with no fade —
    /// there is nothing to fade away from.
    #[test]
    fn a_song_that_runs_out_of_order_list_renders_one_pass_with_no_fade() {
        let bytes = fixtures::synthetic_mod();
        let module = Arc::new(starplayer::mod_file::load(&bytes).expect("the fixture loads"));
        let timeline = song_timeline(&module, GOLDEN_SAMPLE_RATE_HZ).expect("the fixture scans");
        assert_eq!(timeline.end(), EndReason::Ended, "the fixture's order list runs out");
        assert_eq!(timeline.loop_length_frames(), None);

        let length = RenderLength { repeat_count: 0, at_end: AtEnd::FadeOut, fade_frames: GOLDEN_SAMPLE_RATE_HZ as u64, max_frames: GOLDEN_SAMPLE_RATE_HZ as u64 * 60 };
        assert_eq!(length.frames_for(&timeline), (timeline.end_frame() as usize, 0), "one pass, and not one frame of fade");
        let rendered = render_song_fixed_mono(GoldenFormat::Mod, &bytes, GOLDEN_SAMPLE_RATE_HZ, GOLDEN_HOST_BLOCK_FRAMES, length).expect("it renders");
        assert_eq!(rendered.len() as u64, timeline.end_frame());
        assert!(rendered.iter().rev().take(64).any(|sample| *sample != 0), "it is cut at full level rather than faded");

        // The `B00` twin is the same song with an explicit loop, and it ends at the same
        // frame — the only difference is that the module asked to be heard again.
        let looping = Arc::new(starplayer::mod_file::load(&looping_synthetic_mod()).expect("the twin loads"));
        let looping_timeline = song_timeline(&looping, GOLDEN_SAMPLE_RATE_HZ).expect("the twin scans");
        assert!(matches!(looping_timeline.end(), EndReason::Looped { .. }), "the twin loops");
        assert_eq!(looping_timeline.end_frame(), timeline.end_frame(), "at the same frame");
    }

    #[test]
    fn a_faded_render_ends_in_silence_and_a_cut_one_ends_on_the_loop_point() {
        let bytes = looping_synthetic_mod();
        let module = Arc::new(starplayer::mod_file::load(&bytes).expect("the fixture loads"));
        let timeline = song_timeline(&module, GOLDEN_SAMPLE_RATE_HZ).expect("the fixture scans");
        assert!(matches!(timeline.end(), EndReason::Looped { .. }), "the fixture loops");

        let faded_length = RenderLength { repeat_count: 0, at_end: AtEnd::FadeOut, fade_frames: GOLDEN_SAMPLE_RATE_HZ as u64, max_frames: GOLDEN_SAMPLE_RATE_HZ as u64 * 60 };
        let faded = render_song_fixed_mono(GoldenFormat::Mod, &bytes, GOLDEN_SAMPLE_RATE_HZ, GOLDEN_HOST_BLOCK_FRAMES, faded_length).expect("it renders");
        assert_eq!(faded.len() as u64, timeline.end_frame() + GOLDEN_SAMPLE_RATE_HZ as u64);
        assert_eq!(faded.last().copied(), Some(0), "the last frame of a fade is exactly zero");
        assert!(faded.iter().rev().take(64).all(|sample| sample.abs() < 512), "the fade really is a fade");

        let cut_length = RenderLength { repeat_count: 0, at_end: AtEnd::Stop, ..faded_length };
        let cut = render_song_fixed_mono(GoldenFormat::Mod, &bytes, GOLDEN_SAMPLE_RATE_HZ, GOLDEN_HOST_BLOCK_FRAMES, cut_length).expect("it renders");
        assert_eq!(cut.len() as u64, timeline.end_frame(), "a cut render is exactly one pass");

        let repeated_length = RenderLength { repeat_count: 1, ..cut_length };
        let repeated = render_song_fixed_mono(GoldenFormat::Mod, &bytes, GOLDEN_SAMPLE_RATE_HZ, GOLDEN_HOST_BLOCK_FRAMES, repeated_length).expect("it renders");
        let loop_length = timeline.loop_length_frames().expect("a looping song has a loop length");
        assert_eq!(repeated.len() as u64, timeline.end_frame() + loop_length);
        assert_eq!(repeated.get(..cut.len()).expect("the repeat contains the first pass"), cut.as_slice(), "a repeat appends rather than changing the first pass");
    }

    /// Task D5: the CLI's `--depth 24` and `--depth 32` renders need [`FadeSample`] on
    /// [`I24`] and `i32`, which nothing before this task exercised. A fade that reaches
    /// silence at both depths is the same claim `a_faded_render_ends_in_silence_and_a_cut_
    /// one_ends_on_the_loop_point` already makes for `i16`.
    #[test]
    fn a_faded_render_reaches_silence_at_24_and_32_bit_depth_too() {
        use starplayer::mixer::FixedOut;

        let bytes = looping_synthetic_mod();
        let timeline = song_timeline(&Arc::new(starplayer::mod_file::load(&bytes).expect("the fixture loads")), GOLDEN_SAMPLE_RATE_HZ).expect("the fixture scans");
        let length = RenderLength { repeat_count: 0, at_end: AtEnd::FadeOut, fade_frames: GOLDEN_SAMPLE_RATE_HZ as u64, max_frames: GOLDEN_SAMPLE_RATE_HZ as u64 * 60 };

        let twenty_four_bit = render_song::<FixedPath, Linear, FixedOut<I24, 1>>(GoldenFormat::Mod, &bytes, GOLDEN_SAMPLE_RATE_HZ, GOLDEN_HOST_BLOCK_FRAMES, length).expect("it renders");
        assert_eq!(twenty_four_bit.len() as u64, timeline.end_frame() + GOLDEN_SAMPLE_RATE_HZ as u64);
        assert_eq!(twenty_four_bit.last().copied(), Some(I24(0)), "the last frame of a 24-bit fade is exactly zero");

        let thirty_two_bit = render_song::<FixedPath, Linear, FixedOut<i32, 1>>(GoldenFormat::Mod, &bytes, GOLDEN_SAMPLE_RATE_HZ, GOLDEN_HOST_BLOCK_FRAMES, length).expect("it renders");
        assert_eq!(thirty_two_bit.len() as u64, timeline.end_frame() + GOLDEN_SAMPLE_RATE_HZ as u64);
        assert_eq!(thirty_two_bit.last().copied(), Some(0), "the last frame of a 32-bit fade is exactly zero");
    }

    /// Task D2's acceptance case. `PETRI.S3M`'s order list simply runs out, so the song
    /// *ends* rather than looping, and there is no loop length to report. The frame count
    /// and duration pinned here are the ones measured on 2026-09-07, when the fixture set
    /// was trimmed to `PETRI.S3M` and `REFLEX.S3M`.
    #[test]
    fn petri_ends_when_its_order_list_runs_out_at_the_length_it_always_had() {
        let module = Arc::new(starplayer::s3m::load(PETRI).expect("PETRI loads"));
        let timeline = song_timeline(&module, GOLDEN_SAMPLE_RATE_HZ).expect("PETRI scans");

        assert_eq!(timeline.end(), EndReason::Ended, "nothing in PETRI jumps backwards");
        assert_eq!(timeline.end_frame(), 1_814_400, "the frame count measured on 2026-09-07");
        assert!((timeline.duration_seconds() - 41.142857).abs() < 0.001, "0:41, the duration measured on 2026-09-07");
        assert_eq!(timeline.loop_length_frames(), None);
    }

    #[test]
    fn a_render_length_is_clamped_to_its_ceiling() {
        let bytes = looping_synthetic_mod();
        let module = Arc::new(starplayer::mod_file::load(&bytes).expect("the fixture loads"));
        let timeline = song_timeline(&module, GOLDEN_SAMPLE_RATE_HZ).expect("the fixture scans");
        let length = RenderLength { repeat_count: 1_000_000, at_end: AtEnd::FadeOut, fade_frames: 1_000, max_frames: 10_000 };
        assert_eq!(length.frames_for(&timeline), (10_000, 1_000), "the ceiling wins over the repeat count");

        // REFLEX's order list runs out, so however many repeats are asked for there is no
        // fade to append to them.
        let reflex = Arc::new(starplayer::s3m::load(REFLEX).expect("REFLEX loads"));
        let reflex_timeline = song_timeline(&reflex, GOLDEN_SAMPLE_RATE_HZ).expect("REFLEX scans");
        assert_eq!(reflex_timeline.end(), EndReason::Ended);
        assert_eq!(length.frames_for(&reflex_timeline), (10_000, 0), "a song that ends is cut, not faded");
    }

    /// Research point: no format processor may keep a shadow copy of speed or tempo that
    /// survives `TrackerProcessor::reset` and overrides the mark on the first tick after a
    /// seek. MOD's deferred CIA `pending_tempo` is the suspect; it is cleared in `reset`.
    #[test]
    fn a_seek_restores_the_scanned_timing_rather_than_the_processors() {
        use starplayer::core::Frame;
        use starplayer::core::quirks::QuirkSelection;
        use starplayer::engine::{ChannelTable, ControlClock, EngineContext, PatternData};
        use starplayer::mixer::VoicePool;

        for (format, name, bytes) in song_corpus() {
            let module = Arc::new(load_golden(format, &bytes).expect("the fixture loads"));
            let scanned = scanned_song(&module, GOLDEN_SAMPLE_RATE_HZ).expect("the fixture scans");
            let timeline = &scanned.timeline;

            macro_rules! check {
                ($sequencer:expr) => {{
                    let mut sequencer = $sequencer;
                    sequencer.set_timeline(timeline.clone());
                    let channel_count = (sequencer.data().channel_count() as usize).max(1);
                    let mut voices = VoicePool::new(channel_count);
                    let mut channels = ChannelTable::new(channel_count);
                    let mut control = ControlClock::new(GOLDEN_SAMPLE_RATE_HZ, Frame::ZERO);

                    for mark in timeline.marks().iter().step_by(17) {
                        let now = Frame(SEEK_TEST_ORIGIN);
                        let seeked = sequencer.seek_frame(mark.frame, now).expect("a scanned frame resolves");
                        assert_eq!(&seeked, mark, "{format} {name}: seeking to a mark's frame lands on that mark");
                        sequencer.restart_clock_at(now);
                        assert_eq!(sequencer.song_frame(now), mark.frame, "{format} {name}: elapsed is continuous across the seek");
                        assert_eq!(sequencer.row_clock().speed, mark.speed, "{format} {name}: speed at {}", mark.frame);
                        assert_eq!(sequencer.tempo_bpm(), mark.tempo_bpm, "{format} {name}: tempo at {}", mark.frame);

                        // What a seek restores is the speed and the tempo and nothing
                        // else: the processor's global volume (`Vxx`), effect memories and
                        // sample positions come back as `TrackerProcessor::reset` left
                        // them, not as they were when the song last played this row.
                        // Matching OpenMPT's `eAdjust` state replay is separate work.
                        //
                        // Run the row's first tick: whatever the processor kept across
                        // `reset` must not have replaced the sequencer's values.
                        let frame = sequencer.next_event_frame().expect("a seeked sequencer is ready");
                        let mut context = EngineContext::new(frame, &mut voices, &mut channels, &mut control);
                        sequencer.dispatch(frame, &mut context);
                        let visit = sequencer.last_row_visit().expect("the first tick after a seek starts a row");
                        assert_eq!(visit.mark.speed, mark.speed, "{format} {name}: the first tick after a seek ran at the scanned speed");
                        assert_eq!(visit.mark.tempo_bpm, mark.tempo_bpm, "{format} {name}: and the scanned tempo");
                    }
                }};
            }

            let quirks = QuirkSelection::Override(scanned.quirks);
            let handle = Arc::clone(&module);
            match format {
                GoldenFormat::Mod => check!(starplayer::mod_file::sequencer_with_quirks(handle, GOLDEN_SAMPLE_RATE_HZ, quirks)),
                GoldenFormat::S3m => check!(starplayer::s3m::sequencer_with_quirks(handle, GOLDEN_SAMPLE_RATE_HZ, quirks)),
                GoldenFormat::Mtm => check!(starplayer::mtm::sequencer_with_quirks(handle, GOLDEN_SAMPLE_RATE_HZ, quirks)),
                GoldenFormat::Xm => check!(starplayer::xm::sequencer_with_quirks(handle, GOLDEN_SAMPLE_RATE_HZ, quirks)),
                GoldenFormat::It => check!(starplayer::it::sequencer_with_quirks(handle, GOLDEN_SAMPLE_RATE_HZ, quirks)),
            }
        }
    }

    /// Research point: how expensive is a scan? Reported rather than asserted tightly —
    /// the browser runs it in the worklet's message task, like module decoding, so what
    /// matters is that it is milliseconds and not seconds.
    #[test]
    fn scanning_reflex_costs_milliseconds() {
        use std::time::Instant;

        let module = Arc::new(starplayer::s3m::load(REFLEX).expect("REFLEX loads"));
        let started = Instant::now();
        let timeline = song_timeline(&module, GOLDEN_SAMPLE_RATE_HZ).expect("REFLEX scans");
        let elapsed = started.elapsed();

        let ticks: u64 = timeline.end_frame() / 882;
        let ticks_per_second = ticks as f64 / elapsed.as_secs_f64().max(1.0e-9);
        println!(
            "REFLEX.S3M: {:.1} s of song, ~{ticks} ticks, scanned in {:.1} ms ({:.0} ticks/s)",
            timeline.duration_seconds(),
            elapsed.as_secs_f64() * 1_000.0,
            ticks_per_second
        );
        assert!(elapsed.as_secs() < 5, "a scan that takes seconds does not belong in a message task");
    }

    #[test]
    fn the_golden_formats_and_their_directories_are_distinct() {
        assert_eq!(GoldenFormat::ALL.map(GoldenFormat::directory), ["mod", "s3m", "mtm", "xm", "it"]);
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

    // ── M7-H7: `render_song_with_inserts`, the CLI's `--insert` code path ───────────

    /// An insert installed through `render_song_with_inserts` is present from the very
    /// first frame — never mid-song — so the render stays buffer-size-independent exactly
    /// as `render_song` with no inserts does (design goal 3).
    #[test]
    fn a_render_with_an_insert_installed_is_still_block_size_independent() {
        use starplayer::dsp::InsertKind;
        use starplayer::dsp::effects::reverb::REVERB_MIX_PARAM;
        use starplayer::engine::InsertTarget;

        let inserts = [InsertSpec {
            target: InsertTarget::Channel(starplayer::core::ChannelId(0)),
            slot: 0,
            kind: InsertKind::Reverb,
            params: vec![(REVERB_MIX_PARAM, 50)],
        }];
        let length = RenderLength { max_frames: 4 * GOLDEN_SAMPLE_RATE_HZ as u64, fade_frames: 0, ..RenderLength::default_for(GOLDEN_SAMPLE_RATE_HZ) };

        let reference = render_song_with_inserts::<FixedPath, Linear, MonoI16>(GoldenFormat::S3m, REFLEX, GOLDEN_SAMPLE_RATE_HZ, 128, length, &inserts)
            .expect("REFLEX renders with a reverb installed");
        assert!(reference.iter().any(|sample| *sample != 0), "an inserted reverb still produced audible output");

        for host_block_frames in [1, 3, 64, 128, 4_096, 8_191] {
            let rendered = render_song_with_inserts::<FixedPath, Linear, MonoI16>(
                GoldenFormat::S3m,
                REFLEX,
                GOLDEN_SAMPLE_RATE_HZ,
                host_block_frames,
                length,
                &inserts,
            )
            .expect("REFLEX renders with a reverb installed");
            assert_eq!(rendered, reference, "host block size {host_block_frames} changed a render with an insert installed");
        }
    }

    /// `render_song` with an empty insert slice is exactly `render_song_with_inserts` with
    /// none — the two must not silently diverge now that one is defined in terms of the
    /// other.
    #[test]
    fn render_song_with_no_inserts_matches_render_song() {
        let length = RenderLength { max_frames: 2 * GOLDEN_SAMPLE_RATE_HZ as u64, fade_frames: 0, ..RenderLength::default_for(GOLDEN_SAMPLE_RATE_HZ) };
        let plain = render_song::<FixedPath, Linear, MonoI16>(GoldenFormat::S3m, REFLEX, GOLDEN_SAMPLE_RATE_HZ, 128, length).expect("renders");
        let via_inserts =
            render_song_with_inserts::<FixedPath, Linear, MonoI16>(GoldenFormat::S3m, REFLEX, GOLDEN_SAMPLE_RATE_HZ, 128, length, &[]).expect("renders");
        assert_eq!(plain, via_inserts);
    }

    // ── M10-K5b: `render_song_with_options` and the load-time enhancer ──────────────

    /// An enhancer that hands every sample back unchanged, so a rebuilt module still
    /// renders exactly like the one it came from. `starplayer-model`'s own equivalent is
    /// `#[cfg(test)]`-private, so this crate — which is the one place a real fixture can be
    /// both loaded and enhanced — keeps its own.
    struct IdentityEnhancer;

    impl SampleEnhancer for IdentityEnhancer {
        fn name(&self) -> String { String::from("identity") }
        fn enhance(&self, sample: starplayer::model::SamplePcm<'_>) -> starplayer::model::EnhancedPcm {
            starplayer::model::EnhancedPcm::unchanged(sample)
        }
    }

    #[test]
    fn render_song_with_options_and_an_identity_enhancer_matches_render_song() {
        let length = RenderLength { max_frames: 2 * GOLDEN_SAMPLE_RATE_HZ as u64, fade_frames: 0, ..RenderLength::default_for(GOLDEN_SAMPLE_RATE_HZ) };
        let plain = render_song::<FixedPath, Linear, MonoI16>(GoldenFormat::S3m, REFLEX, GOLDEN_SAMPLE_RATE_HZ, 128, length).expect("plain renders");

        let identity = IdentityEnhancer;
        let options = RenderOptions { inserts: &[], enhancer: Some(&identity as &dyn SampleEnhancer) };
        let enhanced = render_song_with_options::<FixedPath, Linear, MonoI16>(GoldenFormat::S3m, REFLEX, GOLDEN_SAMPLE_RATE_HZ, 128, length, &options)
            .expect("identity-enhanced renders");
        assert_eq!(plain, enhanced, "an identity enhancer must not change a byte");
    }

    /// `sinc4x` changes the rendered bytes, and the change is block-size independent —
    /// design goal 3 holds for an enhanced render exactly as it does for a plain one.
    #[test]
    fn render_song_with_options_and_a_sinc4x_enhancer_differs_and_is_block_size_independent() {
        let length = RenderLength { max_frames: 2 * GOLDEN_SAMPLE_RATE_HZ as u64, fade_frames: 0, ..RenderLength::default_for(GOLDEN_SAMPLE_RATE_HZ) };
        let plain = render_song::<FixedPath, Linear, MonoI16>(GoldenFormat::S3m, REFLEX, GOLDEN_SAMPLE_RATE_HZ, 128, length).expect("plain renders");

        let enhancer = starplayer_enhance::SincUpsampler::new(starplayer_enhance::UpsampleFactor::Four);
        let options = RenderOptions { inserts: &[], enhancer: Some(&enhancer as &dyn SampleEnhancer) };
        let reference = render_song_with_options::<FixedPath, Linear, MonoI16>(GoldenFormat::S3m, REFLEX, GOLDEN_SAMPLE_RATE_HZ, 128, length, &options)
            .expect("sinc4x-enhanced renders");
        assert_ne!(reference, plain, "a sinc4x-enhanced render must differ from the plain one");

        for host_block_frames in [1, 3, 64, 128, 4_096, 8_191] {
            let rendered =
                render_song_with_options::<FixedPath, Linear, MonoI16>(GoldenFormat::S3m, REFLEX, GOLDEN_SAMPLE_RATE_HZ, host_block_frames, length, &options)
                    .expect("sinc4x-enhanced renders");
            assert_eq!(rendered, reference, "host block size {host_block_frames} changed a sinc4x-enhanced render");
        }
    }
}
