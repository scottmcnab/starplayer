//! Regenerate or verify the repository's canonical audio hashes.

use std::path::Path;
use std::process::ExitCode;

use starplayer_offline::{
    GOLDEN_HOST_BLOCK_FRAMES, GOLDEN_INTERPOLATOR, GoldenFixture, Interpolator, canonical_sha256_with, golden_fixtures,
    golden_filename_for_interpolator, sha256_hex,
};

/// What every fixture is hashed on: the one kernel the accuracy policy calls canonical.
const CANONICAL: &[Interpolator] = &[GOLDEN_INTERPOLATOR];

/// `reflex.s3m` is hashed on all three implemented kernels. One fixture is enough: the
/// pins exist to catch a kernel that is not bit-identical across x86, ARM and WASM, and a
/// kernel that drifts drifts on any module.
const EVERY_KERNEL: &[Interpolator] = &[GOLDEN_INTERPOLATOR, Interpolator::Cubic, Interpolator::Sinc];

/// Kernels one fixture is hashed on.
///
/// The fixtures themselves live in `starplayer_offline::golden_fixtures`, shared with
/// `starplayer-module-image`; which kernels each is *pinned* on is this binary's business
/// alone. Task F2 added the XM fixture, whose envelopes, key-off, fadeout and ping-pong
/// loop no other golden can reach; task H5 added `reflex`'s cubic and sinc hashes
/// alongside its linear one.
fn kernels_for(fixture: &GoldenFixture) -> &'static [Interpolator] {
    match fixture.stem {
        "reflex" => EVERY_KERNEL,
        _ => CANONICAL,
    }
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
    for fixture in golden_fixtures() {
        for &kernel in kernels_for(&fixture) {
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
