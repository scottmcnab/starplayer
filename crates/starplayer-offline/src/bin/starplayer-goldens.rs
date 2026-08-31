//! Regenerate or verify the repository's canonical audio hashes.

use std::path::Path;
use std::process::ExitCode;

use starplayer_offline::{GOLDEN_HOST_BLOCK_FRAMES, canonical_s3m_sha256, golden_filename, sha256_hex};

struct Fixture {
    stem: &'static str,
    bytes: &'static [u8],
}

const FIXTURES: &[Fixture] = &[
    Fixture { stem: "armani", bytes: include_bytes!("../../../starplayer-s3m/tests/fixtures/ARMANI.S3M") },
    Fixture { stem: "movement", bytes: include_bytes!("../../../starplayer-s3m/tests/fixtures/MOVEMENT.S3M") },
    Fixture { stem: "nicetune", bytes: include_bytes!("../../../starplayer-s3m/tests/fixtures/NICETUNE.S3M") },
    Fixture { stem: "petri", bytes: include_bytes!("../../../starplayer-s3m/tests/fixtures/PETRI.S3M") },
    Fixture { stem: "reflex", bytes: include_bytes!("../../../starplayer-s3m/tests/fixtures/REFLEX.S3M") },
];

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
    for fixture in FIXTURES {
        let relative_path = Path::new("goldens").join("s3m").join(golden_filename(fixture.stem));
        let hash = match canonical_s3m_sha256(fixture.bytes, GOLDEN_HOST_BLOCK_FRAMES) {
            Ok(hash) => sha256_hex(hash),
            Err(error) => {
                eprintln!("starplayer-goldens: {}: {error}", fixture.stem);
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
