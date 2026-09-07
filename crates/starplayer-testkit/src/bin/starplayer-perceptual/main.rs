//! Score StarPlayer's float render against libopenmpt's, fixture by fixture (T10).
//!
//! The hashes and the per-tick traces are exact; neither can say "this sounds wrong".
//! This driver renders every fixture twice — once through `starplayer-offline`'s float
//! path, once through `openmpt123` — normalises the two to equal RMS and reports a
//! segmental SNR and a log-spectral distance. It is a **nightly report with a tolerance,
//! not a gate**: a score never fails the run, and only a tooling failure (no
//! `openmpt123`, an unreadable render, a committed fixture that will not render) exits
//! non-zero.
//!
//! `cargo xtask perceptual` is the entry point; it builds `openmpt123` from the pinned
//! source tarball and passes its path in.
//!
//! ## Why the libopenmpt render is headerless
//!
//! Deliverable 2 of the task file says "load the WAV". On a non-Windows host,
//! `openmpt123`'s WAV writer is `sndfile_stream_raii`, which only exists with libsndfile
//! (`openmpt123/openmpt123.cpp:238-254`: `raw` is unconditional, `wav` is Windows-only via
//! MMIO, and everything else falls through to `MPT_WITH_SNDFILE`). The dependency-free
//! build the task mandates therefore refuses `-o out.wav` with `file format handler 'wav'
//! not found`. `-o out.raw` is the same PCM without the RIFF wrapper, and since the rate,
//! channel count and sample format are ours to choose on the command line, there is
//! nothing in the header this comparison needs.

#![forbid(unsafe_code)]

mod analysis;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use starplayer::dsp::Linear;
use starplayer::mixer::{FloatPath, StereoF32};
use starplayer_offline::{GoldenFormat, RenderLength, fixtures, render_song};

use analysis::Scores;

/// The only rate this comparison runs at, on both sides.
const SAMPLE_RATE_HZ: u32 = 44_100;

/// Stereo, on both sides.
const CHANNEL_COUNT: usize = 2;

/// Host request size for StarPlayer's render. It cannot affect the samples — that is the
/// block-size-independence invariant — so this is only a chunking choice.
const HOST_BLOCK_FRAMES: usize = 128;

/// Where the report and the intermediate renders go when `--output` is not given.
const DEFAULT_OUTPUT_DIRECTORY: &str = "target/perceptual";

/// Two renders whose lengths differ by more than this are called out under the table:
/// libopenmpt's end-of-song is not always StarPlayer's, and trimming to the shorter hides
/// the difference from the scores.
const LENGTH_DIVERGENCE_SECONDS: f64 = 1.0;

/// One module to compare, with the bytes both engines are handed.
struct Fixture {
    name: String,
    format: GoldenFormat,
    extension: &'static str,
    bytes: Vec<u8>,
    /// A `--corpus` module is allowed to fail to load; a committed fixture is not.
    committed: bool,
}

/// One row of the report.
struct Row {
    name: String,
    frames: usize,
    starplayer_frames: usize,
    openmpt_frames: usize,
    scores: Scores,
}

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("starplayer-perceptual: {message}");
            eprintln!("usage: starplayer-perceptual --openmpt PATH [--output DIR] [--corpus PATH]...");
            return ExitCode::FAILURE;
        }
    };

    if !options.openmpt123.is_file() {
        eprintln!("starplayer-perceptual: `{}` is not an executable file", options.openmpt123.display());
        eprintln!("                       run `cargo xtask openmpt` to build it");
        return ExitCode::FAILURE;
    }

    let module_directory = options.output_directory.join("modules");
    if let Err(error) = std::fs::create_dir_all(&module_directory) {
        eprintln!("starplayer-perceptual: cannot create `{}`: {error}", module_directory.display());
        return ExitCode::FAILURE;
    }

    let mut corpus_fixtures = Vec::new();
    for path in &options.corpus {
        match load_corpus_fixture(path) {
            // A format StarPlayer has no render path for yet is a gap, not a failure:
            // F4/G5 point this at XM and IT modules before those have golden fixtures.
            Ok(None) => eprintln!("starplayer-perceptual: skipped {}: no StarPlayer render path for that format yet", path.display()),
            Ok(Some(fixture)) => corpus_fixtures.push(fixture),
            Err(message) => {
                // A corpus path that cannot even be read is the caller's mistake, not a
                // score, so it is worth failing on.
                eprintln!("starplayer-perceptual: {message}");
                return ExitCode::FAILURE;
            }
        }
    }

    let mut rows = Vec::new();
    for fixture in committed_fixtures().into_iter().chain(corpus_fixtures) {
        match compare_fixture(&fixture, &options.openmpt123, &options.output_directory, &module_directory) {
            Outcome::Scored(row) => rows.push(*row),
            Outcome::Skipped(reason) => eprintln!("starplayer-perceptual: skipped {}: {reason}", fixture.name),
            Outcome::ToolingFailure(reason) => {
                eprintln!("starplayer-perceptual: {}: {reason}", fixture.name);
                return ExitCode::FAILURE;
            }
        }
    }

    print_table(&rows);
    report_length_divergence(&rows);

    let report_path = options.output_directory.join("report.tsv");
    if let Err(error) = write_report(&report_path, &rows) {
        eprintln!("starplayer-perceptual: cannot write `{}`: {error}", report_path.display());
        return ExitCode::FAILURE;
    }
    println!("wrote {}", report_path.display());
    ExitCode::SUCCESS
}

/// What happened to one fixture. Only the third variant is worth a non-zero exit.
enum Outcome {
    Scored(Box<Row>),
    Skipped(String),
    ToolingFailure(String),
}

#[derive(Debug)]
struct Options {
    openmpt123: PathBuf,
    output_directory: PathBuf,
    corpus: Vec<PathBuf>,
}

impl Options {
    /// `Ok(None)` means `--help` was asked for and printed.
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Option<Options>, String> {
        let arguments: Vec<String> = arguments.collect();
        let mut openmpt123: Option<PathBuf> = None;
        let mut output_directory = PathBuf::from(DEFAULT_OUTPUT_DIRECTORY);
        let mut corpus = Vec::new();
        let mut index = 0usize;
        while index < arguments.len() {
            match arguments[index].as_str() {
                "--help" | "-h" => {
                    println!("usage: starplayer-perceptual --openmpt PATH [--output DIR] [--corpus PATH]...");
                    println!();
                    println!("  --openmpt PATH   the openmpt123 built by `cargo xtask openmpt`");
                    println!("  --output DIR     where renders and report.tsv go [default: {DEFAULT_OUTPUT_DIRECTORY}]");
                    println!("  --corpus PATH    an extra module to compare; repeatable");
                    return Ok(None);
                }
                "--openmpt" => {
                    openmpt123 = Some(PathBuf::from(value_of(&arguments, index, "--openmpt")?));
                    index += 2;
                }
                "--output" => {
                    output_directory = PathBuf::from(value_of(&arguments, index, "--output")?);
                    index += 2;
                }
                "--corpus" => {
                    corpus.push(PathBuf::from(value_of(&arguments, index, "--corpus")?));
                    index += 2;
                }
                other => return Err(format!("unexpected argument `{other}`")),
            }
        }
        let openmpt123 = openmpt123.ok_or_else(|| "`--openmpt PATH` is required".to_string())?;
        Ok(Some(Options { openmpt123, output_directory, corpus }))
    }
}

fn value_of(arguments: &[String], index: usize, flag: &str) -> Result<String, String> {
    arguments.get(index + 1).cloned().ok_or_else(|| format!("`{flag}` needs a value"))
}

/// The golden contract's fixture set, which is what this comparison covers by default.
///
/// This mirrors the list in `starplayer-offline`'s `starplayer-goldens` binary — the five
/// owner S3Ms committed as bytes, and the synthetic MOD and MTM C6a generates because no
/// licence-safe module of either format can be committed. XM and IT arrive through
/// `--corpus` until F4/G5 give them golden fixtures of their own.
fn committed_fixtures() -> Vec<Fixture> {
    vec![
        Fixture { name: "mod/synthetic".to_string(), format: GoldenFormat::Mod, extension: "mod", bytes: fixtures::synthetic_mod(), committed: true },
        Fixture { name: "mtm/synthetic".to_string(), format: GoldenFormat::Mtm, extension: "mtm", bytes: fixtures::synthetic_mtm(), committed: true },
        Fixture { name: "it/synthetic".to_string(), format: GoldenFormat::It, extension: "it", bytes: fixtures::synthetic_it(), committed: true },
        Fixture { name: "s3m/petri".to_string(), format: GoldenFormat::S3m, extension: "s3m", bytes: include_bytes!("../../../../starplayer-s3m/tests/fixtures/PETRI.S3M").to_vec(), committed: true },
        Fixture { name: "s3m/reflex".to_string(), format: GoldenFormat::S3m, extension: "s3m", bytes: include_bytes!("../../../../starplayer-s3m/tests/fixtures/REFLEX.S3M").to_vec(), committed: true },
    ]
}

/// A `--corpus` module: name the processor by its extension, then read it.
///
/// `Ok(None)` is an extension no `GoldenFormat` covers.
/// That is a skip; only an unreadable path is an error.
fn load_corpus_fixture(path: &Path) -> Result<Option<Fixture>, String> {
    let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or_default().to_ascii_lowercase();
    let (format, canonical_extension) = match extension.as_str() {
        "mod" => (GoldenFormat::Mod, "mod"),
        "s3m" => (GoldenFormat::S3m, "s3m"),
        "mtm" => (GoldenFormat::Mtm, "mtm"),
        "xm" => (GoldenFormat::Xm, "xm"),
        "it" => (GoldenFormat::It, "it"),
        _ => return Ok(None),
    };
    let bytes = std::fs::read(path).map_err(|error| format!("cannot read `{}`: {error}", path.display()))?;
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("corpus").to_string();
    Ok(Some(Fixture { name: format!("corpus/{name}"), format, extension: canonical_extension, bytes, committed: false }))
}

/// Render one fixture through both engines and score them.
fn compare_fixture(fixture: &Fixture, openmpt123: &Path, output_directory: &Path, module_directory: &Path) -> Outcome {
    // Both engines are handed the same bytes: the fixture is written out so `openmpt123`
    // has a path, rather than pointing it at whatever is in the source tree.
    let stem = fixture.name.replace('/', "-");
    let module_path = module_directory.join(format!("{stem}.{}", fixture.extension));
    if let Err(error) = std::fs::write(&module_path, &fixture.bytes) {
        return Outcome::ToolingFailure(format!("cannot write `{}`: {error}", module_path.display()));
    }

    let length = RenderLength::default_for(SAMPLE_RATE_HZ);
    let starplayer = match render_song::<FloatPath, Linear, StereoF32>(fixture.format, &fixture.bytes, SAMPLE_RATE_HZ, HOST_BLOCK_FRAMES, length) {
        Ok(samples) => samples,
        Err(error) if !fixture.committed => return Outcome::Skipped(format!("StarPlayer cannot render it: {error}")),
        Err(error) => return Outcome::ToolingFailure(format!("StarPlayer cannot render it: {error}")),
    };

    // Both renders are kept next to the report. The point of a perceptual comparison is
    // that a human can listen to what the numbers are describing, and a bad score is only
    // investigable if the two signals it came from are still on disk.
    let starplayer_path = output_directory.join(format!("{stem}.starplayer.raw"));
    if let Err(error) = write_raw(&starplayer_path, &starplayer) {
        return Outcome::ToolingFailure(format!("cannot write `{}`: {error}", starplayer_path.display()));
    }

    let render_path = output_directory.join(format!("{stem}.openmpt.raw"));
    let openmpt = match render_with_openmpt123(openmpt123, &module_path, &render_path) {
        Ok(samples) => samples,
        Err(message) => return Outcome::ToolingFailure(message),
    };

    let starplayer_frames = starplayer.len() / CHANNEL_COUNT;
    let openmpt_frames = openmpt.len() / CHANNEL_COUNT;
    // libopenmpt stops at its own end-of-song, which is not always StarPlayer's, and
    // StarPlayer's default length adds a fade a looping song's libopenmpt render does not
    // have. Trimming to the shorter is the pragmatic rule; a divergence worth knowing
    // about is reported under the table rather than silently absorbed.
    let frames = starplayer_frames.min(openmpt_frames);
    let samples = frames * CHANNEL_COUNT;
    let Some(scores) = analysis::compare(&starplayer[..samples], &openmpt[..samples], CHANNEL_COUNT) else {
        return Outcome::Skipped("one of the two renders is silent or empty".to_string());
    };

    Outcome::Scored(Box::new(Row { name: fixture.name.clone(), frames, starplayer_frames, openmpt_frames, scores }))
}

/// Render `module_path` with `openmpt123` and read back the interleaved float samples.
///
/// The flags are the comparison contract: 44.1 kHz stereo, 32-bit float, linear
/// interpolation (`--filter 2` selects the two-tap kernel), unity gain, no dither, and no
/// repeat, so the render stops at libopenmpt's own end-of-song.
fn render_with_openmpt123(openmpt123: &Path, module_path: &Path, render_path: &Path) -> Result<Vec<f32>, String> {
    let output = Command::new(openmpt123)
        .args([
            "--quiet", "--banner", "0", "--no-progress", "--no-meters", "--no-details",
            "--samplerate", "44100", "--channels", "2", "--float",
            "--gain", "0", "--filter", "2", "--dither", "0", "--repeat", "0",
            "--batch", "--force", "-o",
        ])
        .arg(render_path)
        .arg("--")
        .arg(module_path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run `{}`: {error}", openmpt123.display()))?;
    if !output.status.success() {
        return Err(format!(
            "openmpt123 exited with {}\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let bytes = std::fs::read(render_path).map_err(|error| format!("cannot read `{}`: {error}", render_path.display()))?;
    if bytes.is_empty() || bytes.len() % (4 * CHANNEL_COUNT) != 0 {
        return Err(format!("`{}` is not a whole number of stereo float frames ({} bytes)", render_path.display(), bytes.len()));
    }
    Ok(bytes.chunks_exact(4).map(|word| f32::from_le_bytes([word[0], word[1], word[2], word[3]])).collect())
}

fn print_table(rows: &[Row]) {
    println!();
    println!("{:<20} {:>10} {:>12} {:>10} {:>10}", "module", "frames", "seg_snr_db", "lsd_db", "rms_ratio");
    for row in rows {
        println!(
            "{:<20} {:>10} {:>12.2} {:>10.2} {:>10.3}",
            row.name, row.frames, row.scores.segmental_snr_db, row.scores.log_spectral_distance_db, row.scores.rms_ratio
        );
    }
    println!();
}

/// Name every fixture whose two renders are more than [`LENGTH_DIVERGENCE_SECONDS`] apart.
fn report_length_divergence(rows: &[Row]) {
    for row in rows {
        let difference = (row.starplayer_frames as f64 - row.openmpt_frames as f64).abs() / SAMPLE_RATE_HZ as f64;
        if difference > LENGTH_DIVERGENCE_SECONDS {
            println!(
                "length divergence: {} — StarPlayer {:.2} s, libopenmpt {:.2} s, compared {:.2} s",
                row.name,
                row.starplayer_frames as f64 / SAMPLE_RATE_HZ as f64,
                row.openmpt_frames as f64 / SAMPLE_RATE_HZ as f64,
                row.frames as f64 / SAMPLE_RATE_HZ as f64
            );
        }
    }
}

/// Interleaved 32-bit float samples, little-endian and headerless — the same shape
/// `openmpt123 -o <file>.raw` writes, so the two renders load into anything identically.
fn write_raw(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes)
}

fn write_report(path: &Path, rows: &[Row]) -> std::io::Result<()> {
    let mut text = String::from("module\tframes\tseg_snr_db\tlsd_db\trms_ratio\n");
    for row in rows {
        text.push_str(&format!(
            "{}\t{}\t{:.3}\t{:.3}\t{:.4}\n",
            row.name, row.frames, row.scores.segmental_snr_db, row.scores.log_spectral_distance_db, row.scores.rms_ratio
        ));
    }
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_committed_fixture_names_a_format_and_carries_bytes() {
        let fixtures = committed_fixtures();
        assert_eq!(fixtures.len(), 5, "the golden contract has five fixtures");
        for fixture in &fixtures {
            assert!(!fixture.bytes.is_empty(), "{} carries bytes", fixture.name);
            assert!(fixture.committed, "{} is a committed fixture", fixture.name);
            assert!(fixture.name.starts_with(fixture.extension), "{} is named after its format", fixture.name);
        }
    }

    #[test]
    fn an_unsupported_corpus_extension_is_skipped_rather_than_guessed() {
        // Skipped before the file is even read, so a module in a format this checkout has
        // no processor for cannot fail the run just because the corpus names it. XM and IT
        // are both covered now, so the case needs a format neither milestone reached.
        assert!(matches!(load_corpus_fixture(Path::new("does-not-exist.669")), Ok(None)));
    }

    #[test]
    fn a_corpus_path_of_a_known_format_that_does_not_exist_is_an_error() {
        let Err(error) = load_corpus_fixture(Path::new("does-not-exist.s3m")) else { panic!("the file is missing") };
        assert!(error.contains("cannot read"), "{error}");
    }

    #[test]
    fn the_openmpt_path_is_required() {
        let error = Options::parse(["--output".to_string(), "somewhere".to_string()].into_iter()).expect_err("no --openmpt");
        assert!(error.contains("--openmpt"), "{error}");
    }

    #[test]
    fn corpus_paths_accumulate() {
        let arguments = ["--openmpt", "bin/openmpt123", "--corpus", "a.xm", "--corpus", "b.it"].map(str::to_string);
        let options = Options::parse(arguments.into_iter()).expect("the arguments parse").expect("not --help");
        assert_eq!(options.corpus, vec![PathBuf::from("a.xm"), PathBuf::from("b.it")]);
        assert_eq!(options.openmpt123, PathBuf::from("bin/openmpt123"));
    }
}
