//! `starplayer render` — deterministic offline rendering to a WAV file, via
//! `starplayer_offline::render_song`.
//!
//! `render_song` is generic over the mixer path, the interpolator and the output format —
//! compile-time parameters, deliberately (design goal 5: no `dyn` call in the mixing
//! loop) — so a runtime choice among them has to become one of a finite set of
//! monomorphised calls somewhere. [`render_dispatch`] is that `match`: one arm per
//! combination this command exposes, each just naming the types the CLI's flags asked
//! for.
//!
//! # `--golden` (task D5 research point 1)
//!
//! `render_song`'s length knobs (`--repeat`, `--fade`, `--at-end`, `--max-seconds`) are
//! built on `RenderLength::frames_for`, which — for `--at-end cut` — **stops the render at
//! the song's own end** rather than continuing past it (that is the whole point of "cut":
//! there is nothing to render after the song is over). The canonical golden segment does
//! the opposite: `render_fixed_mono` has no concept of a song ending at all — it just
//! keeps pulling `GOLDEN_RENDER_FRAMES` raw samples out of a sequencer running under
//! `EndOfSongPolicy::Loop`, which wraps back to the restart order and keeps generating
//! audio if the song's own pass is shorter than ten seconds.
//!
//! For four of the five owner S3Ms `--path fixed --depth 16 --mono --rate 44100
//! --max-seconds 10 --at-end cut` happens to reproduce a golden anyway, because their own
//! pass is *longer* than ten seconds: the render never reaches the natural end, so
//! `frames_for`'s cap at `max_frames` and the golden's raw frame count agree. `MOVEMENT.
//! S3M`'s pass is 9.32 s, so that recipe cuts its render 29,971 frames short of the
//! golden's window — a real discrepancy, not a rounding difference. Reaching the golden
//! for a song of *any* length through the general knobs alone would need a `--repeat`
//! count large enough to guarantee `body >= max_frames` regardless of the song's own
//! length, which is exactly the kind of flag surface this task's research point warns
//! against ("keeps the flag surface honest"). `--golden` is the dedicated switch instead:
//! it bypasses `render_song` entirely and calls `render_fixed_mono` /
//! `canonical_sha256` — the literal functions the goldens are generated from — so every
//! module reproduces its golden exactly, regardless of how long one pass of it is.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use starplayer::core::AtEnd;
use starplayer::dsp::{Linear, Nearest};
use starplayer::mixer::{FixedOut, FixedPath, FloatOut, FloatPath, I24};
use starplayer::model::ModuleFormat;
use starplayer_offline::wav::{WavSample, write_wav};
use starplayer_offline::{GoldenFormat, RenderLength};

use crate::archive;

/// Voice-accumulation path.
#[derive(clap::ValueEnum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum MixPathArg {
    /// `f32` accumulation — the desktop and browser default.
    Float,
    /// Integer accumulation — the canonical bit-exact path the goldens fingerprint.
    Fixed,
}

/// Resampling kernel.
#[derive(clap::ValueEnum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum InterpArg {
    /// Nearest-neighbour.
    Nearest,
    /// Linear. The default, and the golden-hash reference.
    Linear,
}

/// Output bit depth.
#[derive(clap::ValueEnum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum DepthArg {
    #[value(name = "16")]
    I16,
    #[value(name = "24")]
    I24,
    #[value(name = "32")]
    I32,
    #[value(name = "f32")]
    F32,
}

/// What happens at a detected loop point.
#[derive(clap::ValueEnum, Copy, Clone, Debug, PartialEq, Eq)]
pub enum AtEndArg {
    /// Fade over `--fade` seconds into the second pass. The default.
    Fade,
    /// Stop dead on the loop point; no fade.
    Cut,
}

#[derive(clap::Args, Debug)]
pub struct RenderArgs {
    /// Module file, or a ZIP archive containing one.
    pub file: PathBuf,
    /// Output WAV file.
    #[arg(short = 'o', long = "output")]
    pub output: PathBuf,
    /// Output sample rate in Hz.
    #[arg(long, default_value_t = 44_100, conflicts_with = "golden")]
    pub rate: u32,
    /// Output bit depth.
    #[arg(long, value_enum, default_value = "f32", conflicts_with = "golden")]
    pub depth: DepthArg,
    /// Fold stereo down to one channel. Stereo by default.
    #[arg(long, conflicts_with = "golden")]
    pub mono: bool,
    /// Voice-accumulation path.
    #[arg(long = "path", value_enum, default_value = "float", conflicts_with = "golden")]
    pub mix_path: MixPathArg,
    /// Resampling kernel.
    #[arg(long, value_enum, default_value = "linear", conflicts_with = "golden")]
    pub interp: InterpArg,
    /// Extra passes through the repeating section after the first. Zero plays it once.
    #[arg(long, default_value_t = 0, conflicts_with = "golden")]
    pub repeat: u32,
    /// Fade length in seconds at a detected loop point. Ignored by `--at-end cut` and by
    /// a song that ends on its own — there is nothing to fade away from. Defaults to ten
    /// seconds.
    #[arg(long, conflicts_with = "golden")]
    pub fade: Option<f64>,
    /// What happens at a detected loop point.
    #[arg(long = "at-end", value_enum, default_value = "fade", conflicts_with = "golden")]
    pub at_end: AtEndArg,
    /// Hard ceiling on the render length, in seconds. Defaults to one hour.
    #[arg(long, conflicts_with = "golden")]
    pub max_seconds: Option<f64>,
    /// Render exactly the canonical golden segment this build's `goldens/` fingerprint —
    /// fixed path, linear interpolation, mono 16-bit, 44.1 kHz, ten seconds, no fade.
    /// Conflicts with every other rendering knob; see this module's doc comment for why
    /// they cannot always express the same render.
    #[arg(long)]
    pub golden: bool,
    /// Which recognised entry of a ZIP archive to render.
    #[arg(long)]
    pub entry: Option<usize>,
}

pub fn run(args: RenderArgs) -> Result<(), String> {
    let bytes = archive::load_module_bytes(&args.file, args.entry)?;
    let format = starplayer::probe(&bytes)
        .and_then(golden_format)
        .ok_or_else(|| format!("{}: not a module format this build plays", args.file.display()))?;

    if args.golden {
        return run_golden(&args.output, format, &bytes);
    }

    let mut length = RenderLength::default_for(args.rate);
    length.repeat_count = args.repeat;
    if let Some(fade_seconds) = args.fade {
        length.fade_frames = seconds_to_frames(fade_seconds, args.rate);
    }
    length.at_end = match args.at_end {
        AtEndArg::Fade => AtEnd::FadeOut,
        // A cut render asks for no fade at all, whatever `--fade` said.
        AtEndArg::Cut => AtEnd::Stop,
    };
    if args.at_end == AtEndArg::Cut {
        length.fade_frames = 0;
    }
    if let Some(max_seconds) = args.max_seconds {
        length.max_frames = seconds_to_frames(max_seconds, args.rate);
    }

    let channels: u16 = if args.mono { 1 } else { 2 };
    let host_block_frames = starplayer_offline::GOLDEN_HOST_BLOCK_FRAMES;

    let (frames_written, hash) = render_dispatch(RenderTarget {
        mix_path: args.mix_path,
        interp: args.interp,
        depth: args.depth,
        channels,
        format,
        bytes: &bytes,
        rate: args.rate,
        host_block_frames,
        length,
        output: &args.output,
    })?;

    println!(
        "wrote {} · {frames_written} frames · {:.3} s at {} Hz to {}",
        describe(args.mix_path, args.interp, args.depth, channels),
        frames_written as f64 / args.rate.max(1) as f64,
        args.rate,
        args.output.display()
    );
    println!("sha256: {hash}");
    Ok(())
}

/// `--golden`: the literal golden-generation functions, not `render_song`.
fn run_golden(output: &Path, format: GoldenFormat, bytes: &[u8]) -> Result<(), String> {
    let host_block_frames = starplayer_offline::GOLDEN_HOST_BLOCK_FRAMES;
    let samples = starplayer_offline::render_fixed_mono(format, bytes, host_block_frames).map_err(|error| error.to_string())?;
    write_wav(output, starplayer_offline::GOLDEN_SAMPLE_RATE_HZ, 1, &samples).map_err(|error| error.to_string())?;
    let hash = starplayer_offline::canonical_sha256(format, bytes, host_block_frames).map_err(|error| error.to_string())?;

    println!(
        "wrote fixed · linear · 16-bit · mono · {} frames · {:.3} s at {} Hz to {}",
        samples.len(),
        samples.len() as f64 / starplayer_offline::GOLDEN_SAMPLE_RATE_HZ as f64,
        starplayer_offline::GOLDEN_SAMPLE_RATE_HZ,
        output.display()
    );
    println!("sha256: {}", starplayer_offline::sha256_hex(hash));
    Ok(())
}

fn golden_format(format: ModuleFormat) -> Option<GoldenFormat> {
    match format {
        ModuleFormat::Mod => Some(GoldenFormat::Mod),
        ModuleFormat::S3m => Some(GoldenFormat::S3m),
        ModuleFormat::Mtm => Some(GoldenFormat::Mtm),
        ModuleFormat::It => Some(GoldenFormat::It),
        ModuleFormat::Xm => None,
    }
}

fn seconds_to_frames(seconds: f64, rate: u32) -> u64 { (seconds.max(0.0) * rate as f64).round() as u64 }

fn describe(mix_path: MixPathArg, interp: InterpArg, depth: DepthArg, channels: u16) -> String {
    let mix_path = match mix_path { MixPathArg::Float => "float", MixPathArg::Fixed => "fixed" };
    let interp = match interp { InterpArg::Nearest => "nearest", InterpArg::Linear => "linear" };
    let depth = match depth { DepthArg::I16 => "16-bit", DepthArg::I24 => "24-bit", DepthArg::I32 => "32-bit", DepthArg::F32 => "f32" };
    let channels = if channels == 1 { "mono" } else { "stereo" };
    format!("{mix_path} · {interp} · {depth} · {channels}")
}

/// SHA-256 over `samples` encoded as little-endian bytes, hex-formatted — the same
/// encoding [`starplayer_offline::canonical_sha256`] hashes the golden segment with, so a
/// `--path fixed --depth 16 --mono` render's printed hash is directly comparable to a
/// committed golden file's contents.
fn pcm_sha256<S: WavSample>(samples: &[S]) -> String {
    let mut hasher = Sha256::new();
    let mut sample_bytes = Vec::with_capacity(S::BYTES_PER_SAMPLE as usize);
    for &sample in samples {
        sample_bytes.clear();
        sample.write_le(&mut sample_bytes);
        hasher.update(&sample_bytes);
    }
    let digest = hasher.finalize();

    use std::fmt::Write;
    let mut hexadecimal = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hexadecimal, "{byte:02x}");
    }
    hexadecimal
}

/// [`render_dispatch`]'s parameters, bundled so the runtime-to-compile-time dispatch
/// below takes one argument rather than ten.
struct RenderTarget<'a> {
    mix_path: MixPathArg,
    interp: InterpArg,
    depth: DepthArg,
    channels: u16,
    format: GoldenFormat,
    bytes: &'a [u8],
    rate: u32,
    host_block_frames: usize,
    length: RenderLength,
    output: &'a Path,
}

/// Render then write the WAV file, dispatching the compile-time mixer path,
/// interpolator, sample type and channel count that the runtime arguments named. Returns
/// the number of frames written and the hex SHA-256 of its PCM payload.
fn render_dispatch(target: RenderTarget<'_>) -> Result<(usize, String), String> {
    let RenderTarget { mix_path, interp, depth, channels, format, bytes, rate, host_block_frames, length, output } = target;
    macro_rules! render_and_write {
        ($path:ty, $out:ty, $interp:ty, $channels:expr) => {{
            let samples = starplayer_offline::render_song::<$path, $interp, $out>(format, bytes, rate, host_block_frames, length).map_err(|error| error.to_string())?;
            write_wav(output, rate, channels, &samples).map_err(|error| error.to_string())?;
            (samples.len() / ($channels as usize), pcm_sha256(&samples))
        }};
    }

    let result = match (mix_path, interp, depth, channels) {
        (MixPathArg::Float, InterpArg::Nearest, DepthArg::I16, 1) => render_and_write!(FloatPath, FloatOut<i16, 1>, Nearest, 1),
        (MixPathArg::Float, InterpArg::Nearest, DepthArg::I16, 2) => render_and_write!(FloatPath, FloatOut<i16, 2>, Nearest, 2),
        (MixPathArg::Float, InterpArg::Nearest, DepthArg::I24, 1) => render_and_write!(FloatPath, FloatOut<I24, 1>, Nearest, 1),
        (MixPathArg::Float, InterpArg::Nearest, DepthArg::I24, 2) => render_and_write!(FloatPath, FloatOut<I24, 2>, Nearest, 2),
        (MixPathArg::Float, InterpArg::Nearest, DepthArg::I32, 1) => render_and_write!(FloatPath, FloatOut<i32, 1>, Nearest, 1),
        (MixPathArg::Float, InterpArg::Nearest, DepthArg::I32, 2) => render_and_write!(FloatPath, FloatOut<i32, 2>, Nearest, 2),
        (MixPathArg::Float, InterpArg::Nearest, DepthArg::F32, 1) => render_and_write!(FloatPath, FloatOut<f32, 1>, Nearest, 1),
        (MixPathArg::Float, InterpArg::Nearest, DepthArg::F32, 2) => render_and_write!(FloatPath, FloatOut<f32, 2>, Nearest, 2),

        (MixPathArg::Float, InterpArg::Linear, DepthArg::I16, 1) => render_and_write!(FloatPath, FloatOut<i16, 1>, Linear, 1),
        (MixPathArg::Float, InterpArg::Linear, DepthArg::I16, 2) => render_and_write!(FloatPath, FloatOut<i16, 2>, Linear, 2),
        (MixPathArg::Float, InterpArg::Linear, DepthArg::I24, 1) => render_and_write!(FloatPath, FloatOut<I24, 1>, Linear, 1),
        (MixPathArg::Float, InterpArg::Linear, DepthArg::I24, 2) => render_and_write!(FloatPath, FloatOut<I24, 2>, Linear, 2),
        (MixPathArg::Float, InterpArg::Linear, DepthArg::I32, 1) => render_and_write!(FloatPath, FloatOut<i32, 1>, Linear, 1),
        (MixPathArg::Float, InterpArg::Linear, DepthArg::I32, 2) => render_and_write!(FloatPath, FloatOut<i32, 2>, Linear, 2),
        (MixPathArg::Float, InterpArg::Linear, DepthArg::F32, 1) => render_and_write!(FloatPath, FloatOut<f32, 1>, Linear, 1),
        (MixPathArg::Float, InterpArg::Linear, DepthArg::F32, 2) => render_and_write!(FloatPath, FloatOut<f32, 2>, Linear, 2),

        (MixPathArg::Fixed, InterpArg::Nearest, DepthArg::I16, 1) => render_and_write!(FixedPath, FixedOut<i16, 1>, Nearest, 1),
        (MixPathArg::Fixed, InterpArg::Nearest, DepthArg::I16, 2) => render_and_write!(FixedPath, FixedOut<i16, 2>, Nearest, 2),
        (MixPathArg::Fixed, InterpArg::Nearest, DepthArg::I24, 1) => render_and_write!(FixedPath, FixedOut<I24, 1>, Nearest, 1),
        (MixPathArg::Fixed, InterpArg::Nearest, DepthArg::I24, 2) => render_and_write!(FixedPath, FixedOut<I24, 2>, Nearest, 2),
        (MixPathArg::Fixed, InterpArg::Nearest, DepthArg::I32, 1) => render_and_write!(FixedPath, FixedOut<i32, 1>, Nearest, 1),
        (MixPathArg::Fixed, InterpArg::Nearest, DepthArg::I32, 2) => render_and_write!(FixedPath, FixedOut<i32, 2>, Nearest, 2),
        (MixPathArg::Fixed, InterpArg::Nearest, DepthArg::F32, 1) => render_and_write!(FixedPath, FixedOut<f32, 1>, Nearest, 1),
        (MixPathArg::Fixed, InterpArg::Nearest, DepthArg::F32, 2) => render_and_write!(FixedPath, FixedOut<f32, 2>, Nearest, 2),

        (MixPathArg::Fixed, InterpArg::Linear, DepthArg::I16, 1) => render_and_write!(FixedPath, FixedOut<i16, 1>, Linear, 1),
        (MixPathArg::Fixed, InterpArg::Linear, DepthArg::I16, 2) => render_and_write!(FixedPath, FixedOut<i16, 2>, Linear, 2),
        (MixPathArg::Fixed, InterpArg::Linear, DepthArg::I24, 1) => render_and_write!(FixedPath, FixedOut<I24, 1>, Linear, 1),
        (MixPathArg::Fixed, InterpArg::Linear, DepthArg::I24, 2) => render_and_write!(FixedPath, FixedOut<I24, 2>, Linear, 2),
        (MixPathArg::Fixed, InterpArg::Linear, DepthArg::I32, 1) => render_and_write!(FixedPath, FixedOut<i32, 1>, Linear, 1),
        (MixPathArg::Fixed, InterpArg::Linear, DepthArg::I32, 2) => render_and_write!(FixedPath, FixedOut<i32, 2>, Linear, 2),
        (MixPathArg::Fixed, InterpArg::Linear, DepthArg::F32, 1) => render_and_write!(FixedPath, FixedOut<f32, 1>, Linear, 1),
        (MixPathArg::Fixed, InterpArg::Linear, DepthArg::F32, 2) => render_and_write!(FixedPath, FixedOut<f32, 2>, Linear, 2),

        (_, _, _, channels) => return Err(format!("internal error: unhandled channel count {channels}")),
    };
    Ok(result)
}
