//! Task D5 deliverable 3, the acceptance test: `starplayer render --golden` reproduces
//! every committed `goldens/s3m/*.sha256` fingerprint through the real CLI binary —
//! argument parsing, the WAV writer and all — for every owner S3M.
//!
//! `--golden` rather than the general `--path fixed --depth 16 --mono --max-seconds 10
//! --at-end cut` recipe: that recipe only reproduces a golden for a song whose own pass is
//! longer than the ten-second golden window. For a shorter song `--at-end cut` stops the
//! render at the song's own end — correct behaviour for a general-purpose renderer, and
//! short of the golden's raw, end-agnostic window. See
//! `apps/starplayer-cli/src/render.rs`'s module doc for the full account.

use std::path::{Path, PathBuf};
use std::process::Command;

use starplayer_offline::wav::read_wav;

/// Every owner S3M the golden contract covers, and the stem its golden file is named
/// after (`crates/starplayer-offline/src/bin/starplayer-goldens.rs`'s own fixture list).
const STEMS: &[(&str, &str)] = &[
    ("PETRI.S3M", "petri"),
    ("REFLEX.S3M", "reflex"),
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn golden_hash(stem: &str) -> String {
    let path = workspace_root().join("goldens/s3m").join(format!("{stem}__i16_mono_44100_linear.sha256"));
    let contents = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading golden `{}`: {error}", path.display()));
    contents.trim().to_string()
}

#[test]
fn the_cli_render_golden_reproduces_every_owner_s3m_golden() {
    let scratch_directory = std::env::temp_dir().join(format!("starplayer-cli-golden-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch_directory).expect("a scratch directory");

    for &(file_name, stem) in STEMS {
        let input = workspace_root().join("crates/starplayer-s3m/tests/fixtures").join(file_name);
        let output = scratch_directory.join(format!("{stem}.wav"));
        let expected_hash = golden_hash(stem);

        let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli"))
            .args(["render", input.to_str().expect("a UTF-8 fixture path"), "-o", output.to_str().expect("a UTF-8 output path"), "--golden"])
            .output()
            .expect("the CLI binary runs");
        assert!(cli_output.status.success(), "{stem}: `render --golden` exited with {}: {}", cli_output.status, String::from_utf8_lossy(&cli_output.stderr));

        let stdout = String::from_utf8_lossy(&cli_output.stdout);
        assert!(stdout.contains(&format!("sha256: {expected_hash}")), "{stem}: printed output does not carry the golden hash:\n{stdout}");

        let (header, samples) = read_wav::<i16>(&output).unwrap_or_else(|error| panic!("{stem}: reading the rendered WAV: {error}"));
        assert_eq!(header.channels, 1, "{stem}: a golden render must be mono");
        assert_eq!(header.sample_rate_hz, 44_100, "{stem}: a golden render must be 44.1 kHz");
        assert_eq!(samples.len(), starplayer_offline::GOLDEN_RENDER_FRAMES, "{stem}: a golden render must be exactly ten seconds");

        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        for sample in samples {
            hasher.update(sample.to_le_bytes());
        }
        let actual_hash = hex(&hasher.finalize());
        assert_eq!(actual_hash, expected_hash, "{stem}: the WAV file's own PCM does not match the committed golden");
    }

    let _ = std::fs::remove_dir_all(&scratch_directory);
}

/// The task file's own literal verification command: `PETRI.S3M`'s pass (41.1 s) is
/// longer than the ten-second window, so the general flag surface reaches the golden too.
#[test]
fn the_general_flag_recipe_also_reproduces_the_golden_for_a_song_longer_than_the_window() {
    let scratch_directory = std::env::temp_dir().join(format!("starplayer-cli-golden-flags-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch_directory).expect("a scratch directory");
    let input = workspace_root().join("crates/starplayer-s3m/tests/fixtures/PETRI.S3M");
    let output = scratch_directory.join("petri.wav");

    let status = Command::new(env!("CARGO_BIN_EXE_starplayer-cli"))
        .args([
            "render",
            input.to_str().expect("a UTF-8 fixture path"),
            "-o",
            output.to_str().expect("a UTF-8 output path"),
            "--path",
            "fixed",
            "--depth",
            "16",
            "--mono",
            "--rate",
            "44100",
            "--max-seconds",
            "10",
            "--at-end",
            "cut",
        ])
        .status()
        .expect("the CLI binary runs");
    assert!(status.success(), "render exited with {status}");

    let (_, samples) = read_wav::<i16>(&output).expect("the rendered WAV reads back");
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for sample in samples {
        hasher.update(sample.to_le_bytes());
    }
    let actual_hash = hex(&hasher.finalize());
    assert_eq!(actual_hash, golden_hash("petri"));

    let _ = std::fs::remove_dir_all(&scratch_directory);
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut hexadecimal = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hexadecimal, "{byte:02x}");
    }
    hexadecimal
}
