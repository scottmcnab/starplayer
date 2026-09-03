//! Task F1 deliverable 5: every XM in the pinned libxmp corpus, through the loader.
//!
//! The corpus is fetched by `cargo xtask conformance --fetch-only` and cached under
//! `target/conformance/corpora/libxmp-<sha>/test-dev`. When it is not cached the test
//! prints why and passes, the way `starplayer-offline`'s `render_allocation.rs` does, so a
//! fresh checkout is not blocked on a network fetch. Set `STARPLAYER_REQUIRE_CORPUS` to
//! turn the skip into a failure.
//!
//! Two rules, and they differ by directory:
//!
//! * `data/`, `data/m/`, `data/p/` and `openmpt/xm/` are **real modules**: every one must
//!   load `Ok`. These are the files OpenMPT and libxmp were made to read, so a failure
//!   here is a loader bug, not a broken file.
//! * `data/f/` are libxmp's own **fuzz regressions**: `Ok` or `Err` are both acceptable
//!   and the assertion is only that control comes back. A panic, a hang or an out-of-
//!   memory is the failure.
//!
//! Only `*.xm` is walked. The corpus also holds `.oxm` files — XMs whose samples are Ogg
//! Vorbis streams, a ModPlug extension no format crate here implements — and they are not
//! this loader's business.
//!
//! One `*.xm` in `data/` is not an XM at all: `ice21_ambiguous.xm` is an Ice Tracker
//! module (`Ice!` at offset 0) that libxmp ships precisely to test format disambiguation,
//! and its *song title* is the string `Extended Modu: Necromanc…`. It must **not** probe
//! as an XM, which is what [`NOT_XM`] asserts.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use starplayer_model::FormatDialect;

/// Directories of real modules, relative to `test-dev`, and how many `*.xm` each holds at
/// the pinned revision. The counts are asserted so that a corpus that silently shrank —
/// a partial fetch, a moved directory — fails loudly instead of passing with nothing
/// checked.
const MODULE_DIRECTORIES: &[(&str, usize)] = &[("data", 49), ("data/m", 7), ("data/p", 1), ("openmpt/xm", 84)];

/// libxmp's fuzz-regression directory, where an `Err` is a perfectly good answer.
const REGRESSION_DIRECTORY: (&str, usize) = ("data/f", 8);

/// Files with an `.xm` extension that are **not** XMs, and must be refused by
/// [`starplayer_xm::probe`] rather than loaded. One, and it is deliberate: libxmp's
/// format-disambiguation fixture, an Ice Tracker module whose song title begins
/// `Extended Modu`.
const NOT_XM: &[&str] = &["ice21_ambiguous.xm"];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the workspace root is reachable from this crate")
}

/// The cached pinned libxmp corpus, if `cargo xtask conformance --fetch-only` has run.
fn pinned_corpus_directory() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = fs::read_dir(repository_root().join("target/conformance/corpora"))
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .map(|path| path.join("test-dev"))
        .filter(|path| path.is_dir())
        .collect();
    candidates.sort();
    candidates.pop()
}

/// Every `*.xm` directly inside `directory`, sorted, so a failure names the same file on
/// every machine.
fn xm_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("xm")))
        .collect();
    paths.sort();
    paths
}

#[test]
fn every_module_in_the_pinned_corpus_loads_and_every_fuzz_regression_returns() {
    let Some(corpus) = pinned_corpus_directory() else {
        assert!(
            std::env::var_os("STARPLAYER_REQUIRE_CORPUS").is_none(),
            "STARPLAYER_REQUIRE_CORPUS is set but the pinned corpus is not cached; run `cargo xtask conformance --fetch-only`",
        );
        println!("skipped: the pinned libxmp corpus is not cached; run `cargo xtask conformance --fetch-only`");
        return;
    };

    let mut dialects: BTreeMap<String, usize> = BTreeMap::new();
    let mut loaded = 0usize;
    let mut not_xm = 0usize;
    let mut rows: Vec<(String, usize, usize, usize)> = Vec::new();

    for (directory, expected) in MODULE_DIRECTORIES {
        let files = xm_files(&corpus.join(directory));
        assert_eq!(files.len(), *expected, "{directory} holds {} `*.xm` files, not the pinned {expected}", files.len());

        for path in &files {
            let bytes = fs::read(path).expect("a corpus file is readable");
            let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            if NOT_XM.contains(&name.as_str()) {
                assert!(!starplayer_xm::probe(&bytes), "{directory}/{name} is not an XM and must not probe as one");
                assert!(starplayer_xm::load(&bytes).is_err(), "{directory}/{name} is not an XM and must not load as one");
                not_xm += 1;
                continue;
            }
            assert!(starplayer_xm::probe(&bytes), "{directory}/{name} does not probe as an XM");

            let module = starplayer_xm::load(&bytes);
            let module = match module {
                Ok(module) => module,
                Err(error) => panic!("{directory}/{name} must load: {error:?}"),
            };
            *dialects.entry(format!("{:?}", module.header().dialect)).or_default() += 1;
            loaded += 1;
            rows.push((
                format!("{directory}/{name}"),
                module.header().channel_count as usize,
                module.patterns().len(),
                module.samples().len(),
            ));
        }
    }

    let (regressions, expected_regressions) = REGRESSION_DIRECTORY;
    let regression_files = xm_files(&corpus.join(regressions));
    assert_eq!(regression_files.len(), expected_regressions, "{regressions} holds {} `*.xm` files, not the pinned {expected_regressions}", regression_files.len());

    let mut accepted = 0usize;
    let mut refused = 0usize;
    for path in &regression_files {
        let bytes = fs::read(path).expect("a corpus file is readable");
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        // The whole assertion is that control returns: `Ok` and `Err` are both fine.
        match starplayer_xm::load(&bytes) {
            Ok(module) => {
                accepted += 1;
                // A module that came back has to be one the rest of the engine can index.
                for index in 0..module.patterns().len() {
                    let _ = starplayer_xm::PatternView::new(&module, starplayer_model::PatternId(index as u16));
                }
                println!("  {regressions}/{name}: Ok");
            }
            Err(error) => {
                refused += 1;
                println!("  {regressions}/{name}: {error}");
            }
        }
    }

    println!();
    println!("XM corpus at {}", corpus.display());
    println!("  {:<44} {:>4} {:>4} {:>4}", "file", "chn", "pat", "smp");
    for (name, channels, patterns, samples) in &rows {
        println!("  {name:<44} {channels:>4} {patterns:>4} {samples:>4}");
    }
    println!();
    for (directory, expected) in MODULE_DIRECTORIES {
        println!("  {directory:<16} {expected:>3} `*.xm` file(s), all Ok");
    }
    println!("  {:<16} {not_xm:>3} `*.xm` file(s) that are not XMs at all, all refused: {}", "of which", NOT_XM.join(", "));
    println!("  {regressions:<16} {expected_regressions:>3} fuzz regression(s): {accepted} Ok, {refused} Err, 0 panics");
    println!("  {loaded} modules loaded in total");
    println!();
    println!("  dialect distribution");
    for (dialect, count) in &dialects {
        println!("    {dialect:<16} {count:>3}");
    }

    assert_eq!(loaded + not_xm, MODULE_DIRECTORIES.iter().map(|(_, count)| *count).sum::<usize>());
    assert_eq!(not_xm, NOT_XM.len(), "every known non-XM was seen exactly once");
    assert!(dialects.contains_key(&format!("{:?}", FormatDialect::FastTracker2)), "the corpus has FastTracker 2 files");
    assert!(dialects.contains_key(&format!("{:?}", FormatDialect::OpenMptXm)), "and OpenMPT ones");
    assert!(dialects.contains_key(&format!("{:?}", FormatDialect::MilkyTracker)), "and MilkyTracker ones");
}
