//! Regenerate or verify the repository's canonical audio hashes.

use std::path::Path;
use std::process::ExitCode;

use starplayer_offline::{
    GOLDEN_HOST_BLOCK_FRAMES, GOLDEN_INTERPOLATOR, GoldenFormat, Interpolator, canonical_sha256_with, fixtures,
    golden_filename_for_interpolator, sha256_hex,
};

struct Fixture {
    format: GoldenFormat,
    stem: &'static str,
    bytes: Vec<u8>,
    /// Kernels this fixture is hashed on. Every fixture carries the canonical one; the
    /// two extra `reflex` hashes are M7-task-H5's cross-target pins for the wide kernels,
    /// not a second canonical render.
    kernels: &'static [Interpolator],
}

/// What every fixture is hashed on: the one kernel the accuracy policy calls canonical.
const CANONICAL: &[Interpolator] = &[GOLDEN_INTERPOLATOR];

/// `reflex.s3m` is hashed on all three implemented kernels. One fixture is enough: the
/// pins exist to catch a kernel that is not bit-identical across x86, ARM and WASM, and a
/// kernel that drifts drifts on any module.
const EVERY_KERNEL: &[Interpolator] = &[GOLDEN_INTERPOLATOR, Interpolator::Cubic, Interpolator::Sinc];

/// Every fixture in the canonical contract.
///
/// The S3Ms are the repository owner's own modules and are committed as bytes. MOD,
/// MTM and XM have no licence-safe module to commit, so C6a synthesises theirs from a
/// committed generator instead — see `starplayer_offline::fixtures` for why that route was
/// chosen over hashing the pinned libxmp corpus. Task F2 added the XM one, whose fixture
/// covers the envelopes, the key-off, the fadeout and the ping-pong loop no other golden
/// can reach. Task H5 added `reflex`'s cubic and sinc hashes alongside its linear one.
fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture { format: GoldenFormat::Mod, stem: "synthetic", bytes: fixtures::synthetic_mod(), kernels: CANONICAL },
        Fixture { format: GoldenFormat::Mtm, stem: "synthetic", bytes: fixtures::synthetic_mtm(), kernels: CANONICAL },
        Fixture { format: GoldenFormat::Xm, stem: "synthetic", bytes: fixtures::synthetic_xm(), kernels: CANONICAL },
        Fixture { format: GoldenFormat::It, stem: "synthetic", bytes: fixtures::synthetic_it(), kernels: CANONICAL },
        Fixture { format: GoldenFormat::S3m, stem: "petri", bytes: include_bytes!("../../../starplayer-s3m/tests/fixtures/PETRI.S3M").to_vec(), kernels: CANONICAL },
        Fixture { format: GoldenFormat::S3m, stem: "reflex", bytes: include_bytes!("../../../starplayer-s3m/tests/fixtures/REFLEX.S3M").to_vec(), kernels: EVERY_KERNEL },
    ]
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Mode {
    Regenerate,
    Check,
    Print,
}

fn main() -> ExitCode {
    let mode = match parse_mode() {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("starplayer-goldens: {message}");
            return ExitCode::FAILURE;
        }
    };

    let mut succeeded = true;
    for fixture in fixtures() {
        for &kernel in fixture.kernels {
            let file_name = golden_filename_for_interpolator(fixture.stem, kernel);
            let relative_path = Path::new("goldens").join(fixture.format.directory()).join(file_name);
            let hash = match canonical_sha256_with(fixture.format, &fixture.bytes, GOLDEN_HOST_BLOCK_FRAMES, kernel) {
                Ok(hash) => sha256_hex(hash),
                Err(error) => {
                    eprintln!("starplayer-goldens: {}/{} on {kernel:?}: {error}", fixture.format, fixture.stem);
                    succeeded = false;
                    continue;
                }
            };

            let fixture_succeeded = match mode {
                Mode::Regenerate => write_hash(&relative_path, &hash),
                Mode::Check => check_hash(&relative_path, &hash),
                Mode::Print => {
                    println!("{}  {}", hash, relative_path.display());
                    true
                }
            };
            succeeded &= fixture_succeeded;
        }
    }

    if succeeded { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn parse_mode() -> Result<Mode, String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.as_slice() {
        [] => Ok(Mode::Regenerate),
        [flag] if flag == "--check" => Ok(Mode::Check),
        [flag] if flag == "--print" => Ok(Mode::Print),
        _ => Err("usage: starplayer-goldens [--check|--print]".to_string()),
    }
}

fn write_hash(path: &Path, hash: &str) -> bool {
    let Some(parent) = path.parent() else {
        eprintln!("starplayer-goldens: `{}` has no parent directory", path.display());
        return false;
    };
    if let Err(error) = std::fs::create_dir_all(parent) {
        eprintln!("starplayer-goldens: cannot create `{}`: {error}", parent.display());
        return false;
    }
    if let Err(error) = std::fs::write(path, format!("{hash}\n")) {
        eprintln!("starplayer-goldens: cannot write `{}`: {error}", path.display());
        return false;
    }
    println!("wrote {}  {}", hash, path.display());
    true
}

fn check_hash(path: &Path, actual: &str) -> bool {
    let expected = match std::fs::read_to_string(path) {
        Ok(expected) => expected,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("starplayer-goldens: missing golden `{}` (run `cargo xtask goldens`)", path.display());
            return false;
        }
        Err(error) => {
            eprintln!("starplayer-goldens: cannot read `{}`: {error}", path.display());
            return false;
        }
    };
    let expected = expected.trim();
    if expected != actual {
        eprintln!("starplayer-goldens: mismatch `{}`", path.display());
        eprintln!("  expected {expected}");
        eprintln!("    actual {actual}");
        return false;
    }
    println!("ok    {}  {}", actual, path.display());
    true
}
