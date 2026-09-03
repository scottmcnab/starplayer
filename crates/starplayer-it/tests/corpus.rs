//! Every `.it` in the pinned libxmp `test-dev` corpus, through this loader.
//!
//! The corpus is not committed: `cargo xtask conformance --fetch-only` caches it under
//! `target/conformance/corpora/libxmp-<revision>/`. Without that cache these tests print
//! a message and pass, so a fresh checkout is not broken by a missing download.
//!
//! Two populations, two assertions:
//!
//! * `data/`, `data/m/` and `openmpt/it/` are real modules — every one must load `Ok`.
//! * `data/f/` is libxmp's own fuzz-regression corpus. `Ok` and `Err` are both fine there;
//!   *returning* is the whole assertion, which is the same invariant `fuzz/` enforces.
//!
//! Run with `-- --nocapture` to see the counts, the dialect distribution and the sample
//! feature census.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use starplayer_it::{ItSampleHeader, sample};
use starplayer_model::FormatDialect;

/// Directories whose every `.it` must load.
const REQUIRED_DIRECTORIES: [&str; 3] = ["data", "data/m", "openmpt/it"];

/// The directory of inputs that must merely not panic.
const REGRESSION_DIRECTORY: &str = "data/f";

/// The one file whose dialect this test pins, because it is the only corpus member named
/// after the tracker that wrote it.
const SCHISM_FIXTURE: &str = "data/format_it_schism.it";

/// `target/conformance/corpora/libxmp-*/test-dev`, or `None` when the cache is absent.
fn corpus_root() -> Option<PathBuf> {
    let corpora = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/conformance/corpora");
    let entries = fs::read_dir(corpora).ok()?;
    let mut roots: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with("libxmp-")))
        .map(|path| path.join("test-dev"))
        .filter(|path| path.is_dir())
        .collect();
    roots.sort();
    roots.pop()
}

/// Every `*.it` directly inside `directory`, sorted, so a failure names the same input on
/// every machine.
fn it_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .filter(|path| path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("it")))
        .collect();
    paths.sort();
    paths
}

/// What one file's raw sample headers say, counted without going through the loader so
/// that the census reports the *file's* features rather than the loader's opinion of them.
#[derive(Default, Clone, Copy)]
struct SampleCensus {
    total: usize,
    compressed: usize,
    stereo: usize,
    sustain_looped: usize,
    ping_pong: usize,
    sixteen_bit: usize,
}

impl SampleCensus {
    fn add(&mut self, other: SampleCensus) {
        self.total += other.total;
        self.compressed += other.compressed;
        self.stereo += other.stereo;
        self.sustain_looped += other.sustain_looped;
        self.ping_pong += other.ping_pong;
        self.sixteen_bit += other.sixteen_bit;
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*bytes.get(offset)?, *bytes.get(offset + 1)?]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
        *bytes.get(offset + 2)?,
        *bytes.get(offset + 3)?,
    ]))
}

/// Walk a file's sample-header offset table and count the features the census reports.
fn census(bytes: &[u8]) -> SampleCensus {
    let mut census = SampleCensus::default();
    let (Some(orders), Some(instruments), Some(samples), Some(tracker_version)) =
        (read_u16(bytes, 0x20), read_u16(bytes, 0x22), read_u16(bytes, 0x24), read_u16(bytes, 0x28))
    else {
        return census;
    };
    let table = 0xC0 + orders as usize + 4 * instruments as usize;
    for index in 0..samples as usize {
        let Some(offset) = read_u32(bytes, table + 4 * index) else { continue };
        let offset = offset as usize;
        let Some(header_bytes) = bytes.get(offset..offset + sample::HEADER_LENGTH) else { continue };
        let Ok(header) = ItSampleHeader::parse(header_bytes) else { continue };
        census.total += 1;
        census.compressed += usize::from(header.is_compressed());
        census.stereo += usize::from(header.is_stereo(tracker_version));
        census.sustain_looped += usize::from(header.sustain_loops());
        census.ping_pong += usize::from(header.loop_is_ping_pong() || header.sustain_is_ping_pong());
        census.sixteen_bit += usize::from(header.is_sixteen_bit());
    }
    census
}

fn dialect_name(dialect: FormatDialect) -> &'static str {
    match dialect {
        FormatDialect::ImpulseTracker => "ImpulseTracker",
        FormatDialect::SchismTracker => "SchismTracker",
        FormatDialect::OpenMptIt => "OpenMptIt",
        FormatDialect::ModPlugIt => "ModPlugIt",
        FormatDialect::Unknown => "Unknown",
        _ => "(not an IT dialect)",
    }
}

#[test]
fn every_corpus_module_loads_and_every_regression_input_returns() {
    let Some(root) = corpus_root() else {
        println!("starplayer-it corpus: target/conformance is absent; run `cargo xtask conformance --fetch-only`");
        return;
    };
    println!("starplayer-it corpus: {}", root.display());

    let mut per_directory: Vec<(&str, usize, SampleCensus)> = Vec::new();
    let mut dialects: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut totals = SampleCensus::default();
    let mut loaded = 0usize;

    for directory in REQUIRED_DIRECTORIES {
        let files = it_files(&root.join(directory));
        assert!(!files.is_empty(), "no .it files under {directory}");
        let mut directory_census = SampleCensus::default();
        for path in &files {
            let bytes = fs::read(path).expect("a corpus file is readable");
            assert!(starplayer_it::probe(&bytes), "{} does not probe as an IT", path.display());
            let module = starplayer_it::load(&bytes);
            let module = module.unwrap_or_else(|error| panic!("{} must load: {error:?}", path.display()));
            *dialects.entry(dialect_name(module.header().dialect)).or_default() += 1;
            directory_census.add(census(&bytes));
            loaded += 1;
        }
        totals.add(directory_census);
        per_directory.push((directory, files.len(), directory_census));
    }

    // The fuzz-regression corpus: `Ok` or `Err`, never a panic and never an allocation the
    // file cannot justify.
    let regressions = it_files(&root.join(REGRESSION_DIRECTORY));
    assert!(!regressions.is_empty(), "no .it files under {REGRESSION_DIRECTORY}");
    let mut regressions_ok = 0usize;
    for path in &regressions {
        let bytes = fs::read(path).expect("a corpus file is readable");
        let _ = starplayer_it::probe(&bytes);
        if starplayer_it::load(&bytes).is_ok() {
            regressions_ok += 1;
        }
    }

    println!();
    println!("| directory      | files | samples | compressed | stereo | sustain | ping-pong | 16-bit |");
    println!("|----------------|-------|---------|------------|--------|---------|-----------|--------|");
    for (directory, files, census) in &per_directory {
        println!(
            "| {directory:<14} | {files:>5} | {:>7} | {:>10} | {:>6} | {:>7} | {:>9} | {:>6} |",
            census.total, census.compressed, census.stereo, census.sustain_looped, census.ping_pong, census.sixteen_bit
        );
    }
    println!(
        "| {:<14} | {loaded:>5} | {:>7} | {:>10} | {:>6} | {:>7} | {:>9} | {:>6} |",
        "total", totals.total, totals.compressed, totals.stereo, totals.sustain_looped, totals.ping_pong, totals.sixteen_bit
    );
    println!();
    println!("| {REGRESSION_DIRECTORY:<14} | {:>5} | {regressions_ok} loaded Ok, {} returned Err |", regressions.len(), regressions.len() - regressions_ok);
    println!();
    println!("dialects:");
    for (dialect, count) in &dialects {
        println!("  {dialect:<16} {count}");
    }

    assert_eq!(loaded, per_directory.iter().map(|(_, files, _)| files).sum::<usize>());
}

#[test]
fn the_schism_fixture_reports_the_schism_dialect() {
    let Some(root) = corpus_root() else {
        println!("starplayer-it corpus: target/conformance is absent; run `cargo xtask conformance --fetch-only`");
        return;
    };
    let bytes = fs::read(root.join(SCHISM_FIXTURE)).expect("the Schism fixture is in the pinned corpus");
    let module = starplayer_it::load(&bytes).expect("the Schism fixture loads");

    assert_eq!(module.header().dialect, FormatDialect::SchismTracker);
}
