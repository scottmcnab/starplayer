//! `bench_render` — how much wall-clock time a minute of audio costs, scalar against
//! `simd` (M7-H6 deliverable 4).
//!
//! ```text
//! cargo run -p starplayer-offline --release --example bench_render
//! cargo run -p starplayer-offline --release --example bench_render --features simd
//! ```
//!
//! No `criterion`, deliberately: this is one number per configuration, compared between
//! two builds of the same binary, and a benchmark harness would add a dependency to a
//! workspace that has almost none for a statistical treatment nothing here needs.
//!
//! # What it measures, and how it removes what it does not
//!
//! Each case renders **sixty seconds** of a module with a reverb on channel 0, an
//! equaliser on channel 1 and a compressor on the master bus, on both mix paths. That is
//! the shape M7's exit criterion describes and the one where the vector kernels are
//! reachable at all: a render with no inserts installed only exercises the bus summation
//! and the master volume.
//!
//! `render_song_with_inserts` scans the song before it renders a frame, and building the
//! engine allocates the voice pool and every delay line. Neither is what this is timing,
//! so every case is measured **twice** — once at one second and once at sixty-one — and
//! the reported rate is the fifty-nine seconds between them over the difference of the two
//! times. Anything that happens once per call cancels.
//!
//! `reflex.s3m` is one of the repository owner's own S3Ms and is committed. The dense `.it`
//! comes from the pinned conformance corpus, which is a cache rather than a checkout: if it
//! is not there the case is skipped and says so, rather than failing.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use starplayer::core::ChannelId;
use starplayer::dsp::{InsertKind, Linear, ParamId};
use starplayer::engine::InsertTarget;
use starplayer::mixer::{FixedPath, FloatPath, MonoF32, MonoI16};
use starplayer::core::AtEnd;
use starplayer_offline::{GoldenFormat, InsertSpec, RenderLength, render_song_with_inserts};

/// The rate every case renders at.
const SAMPLE_RATE_HZ: u32 = 44_100;

/// Host block the render is walked in — a whole quantum, which is what a real host asks
/// for and what the goldens use.
const HOST_BLOCK_FRAMES: usize = 128;

/// The long render, in seconds.
const LONG_SECONDS: u64 = 61;

/// The short render whose time is subtracted, in seconds.
const SHORT_SECONDS: u64 = 1;

/// `reflex.s3m`, committed next to the S3M loader's other fixtures.
const REFLEX: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");

/// The dense IT from the pinned corpus, relative to the workspace root.
const DENSE_IT: &str = "target/conformance/corpora/libxmp-6ec0ba21b1b28f91e22b68a51d59207c6bbf6139/test-dev/data/m/Fight2.it";

/// Which build this binary is.
const CONFIGURATION: &str = if cfg!(feature = "simd") { "simd" } else { "scalar" };

/// A reverb, an equaliser and a compressor — the graph M7's exit criterion describes.
///
/// `--no-inserts` renders the same modules with an empty graph instead, which is what
/// isolates the voice path, the bus summation and the master bus from the effects: the
/// difference between the two runs is what the DSP graph costs, and therefore the ceiling
/// on what vectorising an effect can win.
fn inserts() -> Vec<InsertSpec> {
    if std::env::args().any(|argument| argument == "--no-inserts") {
        return Vec::new();
    }
    vec![
        InsertSpec { target: InsertTarget::Channel(ChannelId(0)), slot: 0, kind: InsertKind::Reverb, params: vec![(ParamId(3), 40)] },
        InsertSpec { target: InsertTarget::Channel(ChannelId(1)), slot: 0, kind: InsertKind::Eq, params: vec![(ParamId(0), 1)] },
        InsertSpec { target: InsertTarget::Master, slot: 0, kind: InsertKind::Compressor, params: Vec::new() },
    ]
}

/// A length that renders exactly `seconds` of audio: no fade, and enough repeats that the
/// hard cap is what actually decides where the render stops.
fn length_of(seconds: u64) -> RenderLength {
    RenderLength { repeat_count: u32::MAX, at_end: AtEnd::Continue, fade_frames: 0, max_frames: SAMPLE_RATE_HZ as u64 * seconds }
}

/// Render `seconds` of audio on the float path and return how long it took.
fn time_float(format: GoldenFormat, bytes: &[u8], seconds: u64) -> Result<Duration, String> {
    let started = Instant::now();
    let rendered = render_song_with_inserts::<FloatPath, Linear, MonoF32>(format, bytes, SAMPLE_RATE_HZ, HOST_BLOCK_FRAMES, length_of(seconds), &inserts())
        .map_err(|error| format!("{error:?}"))?;
    // Read the output back so nothing above can be optimised away as unused.
    if rendered.len() as u64 != SAMPLE_RATE_HZ as u64 * seconds {
        return Err(format!("expected {} frames, rendered {}", SAMPLE_RATE_HZ as u64 * seconds, rendered.len()));
    }
    Ok(started.elapsed())
}

/// The same on the fixed path.
fn time_fixed(format: GoldenFormat, bytes: &[u8], seconds: u64) -> Result<Duration, String> {
    let started = Instant::now();
    let rendered = render_song_with_inserts::<FixedPath, Linear, MonoI16>(format, bytes, SAMPLE_RATE_HZ, HOST_BLOCK_FRAMES, length_of(seconds), &inserts())
        .map_err(|error| format!("{error:?}"))?;
    if rendered.len() as u64 != SAMPLE_RATE_HZ as u64 * seconds {
        return Err(format!("expected {} frames, rendered {}", SAMPLE_RATE_HZ as u64 * seconds, rendered.len()));
    }
    Ok(started.elapsed())
}

/// Frames of audio produced per wall-clock second, and how many times faster than real
/// time that is.
fn report(label: &str, long: Duration, short: Duration) {
    let frames = SAMPLE_RATE_HZ as f64 * (LONG_SECONDS - SHORT_SECONDS) as f64;
    let elapsed = long.saturating_sub(short).as_secs_f64().max(f64::MIN_POSITIVE);
    println!("  {label:<28} {:>12.0} frames/s   {:>7.1}x real time   ({:.3} s for {} s of audio)", frames / elapsed, frames / elapsed / SAMPLE_RATE_HZ as f64, elapsed, LONG_SECONDS - SHORT_SECONDS);
}

fn run_case(name: &str, format: GoldenFormat, bytes: &[u8]) {
    println!("{name}:");
    match (time_float(format, bytes, SHORT_SECONDS), time_float(format, bytes, LONG_SECONDS)) {
        (Ok(short), Ok(long)) => report("float path", long, short),
        (Err(error), _) | (_, Err(error)) => println!("  float path                   failed: {error}"),
    }
    match (time_fixed(format, bytes, SHORT_SECONDS), time_fixed(format, bytes, LONG_SECONDS)) {
        (Ok(short), Ok(long)) => report("fixed path", long, short),
        (Err(error), _) | (_, Err(error)) => println!("  fixed path                   failed: {error}"),
    }
}

fn main() {
    println!("bench_render ({CONFIGURATION} build), {SAMPLE_RATE_HZ} Hz, {HOST_BLOCK_FRAMES}-frame host blocks");
    if inserts().is_empty() {
        println!("no inserts installed (--no-inserts): the voice path, the bus summation and the master bus only");
    } else {
        println!("reverb on channel 0, equaliser on channel 1, compressor on the master bus");
    }
    println!();

    run_case("reflex.s3m", GoldenFormat::S3m, REFLEX);

    let dense = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(DENSE_IT);
    match std::fs::read(&dense) {
        Ok(bytes) => run_case("Fight2.it (pinned corpus)", GoldenFormat::It, &bytes),
        Err(error) => println!("Fight2.it (pinned corpus): skipped — {} ({error})", dense.display()),
    }
}
