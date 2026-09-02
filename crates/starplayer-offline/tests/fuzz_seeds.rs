//! The stable-toolchain half of M2-C7: every committed fuzz seed and every committed
//! crash regression walked through all three loaders.
//!
//! `cargo fuzz` needs a nightly toolchain and libFuzzer, so the coverage-guided run lives
//! in its own CI job. This file is what `cargo test --workspace` runs on the pinned
//! toolchain: it replays the corpus that job starts from, so a crash the fuzzer once found
//! can never come back unnoticed, and a seed that stops loading is caught on every commit
//! rather than only when someone next fuzzes.
//!
//! * `fuzz/seeds/<format>/` — valid modules. Their own loader must **accept** them.
//! * `fuzz/regressions/<format>/` — inputs a fuzzer once crashed on. `Ok` or `Err` are both
//!   fine; not panicking is the whole assertion.
//!
//! Every input is additionally offered to the two loaders it does not belong to, because
//! a host that probes formats in the wrong order must not be able to crash a loader with
//! another format's bytes.

use std::fs;
use std::path::{Path, PathBuf};

/// The three native loaders, by the directory name their inputs live under.
const FORMATS: [&str; 3] = ["mod", "s3m", "mtm"];

fn fuzz_directory(kind: &str, format: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz").join(kind).join(format)
}

/// Every regular file directly inside `directory`, sorted, so a failure names the same
/// input on every machine.
fn inputs(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && path.file_name().is_some_and(|name| name != "README.md"))
        .collect();
    paths.sort();
    paths
}

/// Offer `bytes` to every loader and every probe. The assertion is that control returns.
fn every_loader_survives(bytes: &[u8]) {
    let _ = starplayer::mod_file::probe(bytes);
    let _ = starplayer::s3m::probe(bytes);
    let _ = starplayer::mtm::probe(bytes);
    let _ = starplayer::mod_file::load(bytes);
    let _ = starplayer::s3m::load(bytes);
    let _ = starplayer::mtm::load(bytes);
}

fn load_for(format: &str, bytes: &[u8]) -> Result<starplayer::model::Module, starplayer::core::Error> {
    match format {
        "mod" => starplayer::mod_file::load(bytes),
        "s3m" => starplayer::s3m::load(bytes),
        "mtm" => starplayer::mtm::load(bytes),
        other => panic!("unknown fuzz corpus format `{other}`"),
    }
}

#[test]
fn every_committed_seed_is_a_module_its_own_loader_accepts() {
    let mut checked = 0usize;
    for format in FORMATS {
        let directory = fuzz_directory("seeds", format);
        let seeds = inputs(&directory);
        assert!(!seeds.is_empty(), "no seeds under {}", directory.display());
        for seed in seeds {
            let bytes = fs::read(&seed).expect("a committed seed is readable");
            let module = load_for(format, &bytes);
            assert!(module.is_ok(), "{} must load as {format}: {:?}", seed.display(), module.err());
            every_loader_survives(&bytes);
            checked += 1;
        }
    }
    assert!(checked >= 20, "the seed corpus shrank to {checked} inputs");
}

#[test]
fn every_committed_crash_regression_returns_instead_of_panicking() {
    for format in FORMATS {
        for artifact in inputs(&fuzz_directory("regressions", format)) {
            let bytes = fs::read(&artifact).expect("a committed regression input is readable");
            let _ = load_for(format, &bytes);
            every_loader_survives(&bytes);
        }
    }
}

/// The seeds are the fuzzer's starting points, so a truncation of one is the shape the
/// fuzzer will spend most of its time on. Doing a deterministic pass here means the
/// pinned-toolchain job covers the truncation family even when nobody runs `cargo fuzz`.
#[test]
fn every_prefix_and_single_byte_edit_of_a_seed_returns_instead_of_panicking() {
    for format in FORMATS {
        for seed in inputs(&fuzz_directory("seeds", format)) {
            let bytes = fs::read(&seed).expect("a committed seed is readable");
            // Prefixes, on a stride that keeps the pass quick but still lands inside every
            // structural region: the header, the tables, the pattern body and the PCM.
            for length in (0..bytes.len()).step_by(97) {
                let _ = load_for(format, bytes.get(..length).unwrap_or_default());
            }
            // One flipped byte per 251-byte stride, at the two values most likely to be
            // read as a count: the maximum, and the one that overflows a signed compare.
            for offset in (0..bytes.len()).step_by(251) {
                for value in [0xFF, 0x80] {
                    let mut edited = bytes.clone();
                    if let Some(byte) = edited.get_mut(offset) {
                        *byte = value;
                    }
                    let _ = load_for(format, &edited);
                }
            }
        }
    }
}
