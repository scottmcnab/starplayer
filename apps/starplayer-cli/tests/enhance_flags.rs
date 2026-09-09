//! M10-K5b: `render --enhance`, `play --enhance` and `info --enhance`, exercised through
//! the real CLI binary rather than through unit tests alone — the acceptance-level
//! counterpart to `golden_reproduction.rs` and `insert_flags.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

fn workspace_root() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("../..") }

fn reflex() -> PathBuf { workspace_root().join("crates/starplayer-s3m/tests/fixtures/REFLEX.S3M") }

fn sha256_of(path: &Path) -> [u8; 32] {
    let bytes = std::fs::read(path).expect("the rendered WAV reads back");
    let digest = Sha256::digest(&bytes);
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);
    hash
}

/// `--golden` and `--enhance` are mutually exclusive: an enhanced render is a different
/// configuration from the goldens (`plans/product/03-accuracy-policy.md` §5 item 5), so
/// clap refuses the combination before any rendering happens.
#[test]
fn golden_refuses_enhance() {
    let scratch = std::env::temp_dir().join(format!("starplayer-cli-enhance-golden-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let output = scratch.join("out.wav");

    let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli"))
        .args(["render", reflex().to_str().expect("a UTF-8 path"), "--golden", "--enhance", "sinc4x", "-o", output.to_str().expect("a UTF-8 path")])
        .output()
        .expect("the CLI binary runs");

    assert!(!cli_output.status.success(), "--golden and --enhance must not both be accepted");
    assert!(!output.exists(), "no file should have been written");
    let _ = std::fs::remove_dir_all(&scratch);
}

/// `render --enhance sinc4x+loop` writes a WAV whose hash differs from the plain render —
/// the acceptance-level form of `starplayer-offline`'s own
/// `render_song_with_options_and_a_sinc4x_enhancer_differs_and_is_block_size_independent`.
#[test]
fn an_enhanced_render_differs_from_the_plain_one() {
    let scratch = std::env::temp_dir().join(format!("starplayer-cli-enhance-audible-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let plain_output = scratch.join("plain.wav");
    let enhanced_output = scratch.join("enhanced.wav");

    let input = reflex();
    let run = |output: &Path, extra_args: &[&str]| {
        let mut args = vec!["render", input.to_str().expect("a UTF-8 path"), "-o", output.to_str().expect("a UTF-8 path"), "--max-seconds", "5"];
        args.extend_from_slice(extra_args);
        let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli")).args(&args).output().expect("the CLI binary runs");
        assert!(cli_output.status.success(), "render exited with {}: {}", cli_output.status, String::from_utf8_lossy(&cli_output.stderr));
    };

    run(&plain_output, &[]);
    run(&enhanced_output, &["--enhance", "sinc4x+loop"]);

    let plain_hash = sha256_of(&plain_output);
    let enhanced_hash = sha256_of(&enhanced_output);
    assert_ne!(plain_hash, enhanced_hash, "--enhance sinc4x+loop must audibly change the render");

    let _ = std::fs::remove_dir_all(&scratch);
}

/// M10-K5c's full chain renders through the real binary, and its output differs from both
/// the plain render and the `sinc4x` one — so every stage in `denoise+sinc4x+sbr` is
/// reaching the module rather than one of them silently being a no-op.
#[test]
fn the_full_enhancement_chain_renders_and_differs_from_the_upsampler_alone() {
    let scratch = std::env::temp_dir().join(format!("starplayer-cli-enhance-full-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let plain_output = scratch.join("plain.wav");
    let upsampled_output = scratch.join("upsampled.wav");
    let full_output = scratch.join("full.wav");

    let input = reflex();
    let run = |output: &Path, extra_args: &[&str]| {
        let mut args = vec!["render", input.to_str().expect("a UTF-8 path"), "-o", output.to_str().expect("a UTF-8 path"), "--max-seconds", "5"];
        args.extend_from_slice(extra_args);
        let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli")).args(&args).output().expect("the CLI binary runs");
        assert!(cli_output.status.success(), "render exited with {}: {}", cli_output.status, String::from_utf8_lossy(&cli_output.stderr));
    };

    run(&plain_output, &[]);
    run(&upsampled_output, &["--enhance", "sinc4x"]);
    run(&full_output, &["--enhance", "denoise+sinc4x+sbr"]);

    let plain_hash = sha256_of(&plain_output);
    let upsampled_hash = sha256_of(&upsampled_output);
    let full_hash = sha256_of(&full_output);
    assert_ne!(full_hash, plain_hash, "the full chain must change the render");
    assert_ne!(full_hash, upsampled_hash, "and must differ from sinc4x alone");

    let _ = std::fs::remove_dir_all(&scratch);
}

/// An unknown enhancer id is refused with a message naming the real choices, rather than
/// silently rendering as if `--enhance` had not been given.
#[test]
fn an_unknown_enhancer_id_is_refused() {
    let scratch = std::env::temp_dir().join(format!("starplayer-cli-enhance-unknown-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let output = scratch.join("out.wav");

    let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli"))
        .args(["render", reflex().to_str().expect("a UTF-8 path"), "--enhance", "reverb", "-o", output.to_str().expect("a UTF-8 path")])
        .output()
        .expect("the CLI binary runs");

    assert!(!cli_output.status.success(), "an unknown enhancer id must be refused");
    let stderr = String::from_utf8_lossy(&cli_output.stderr);
    assert!(stderr.contains("sinc4x"), "the error should name the real enhancer ids: {stderr}");
    let _ = std::fs::remove_dir_all(&scratch);
}

/// `info --enhance sinc4x` on REFLEX.S3M prints `×4` rows: every sample in the fixture
/// sits well under the 4x-at-44100Hz ceiling `info` applies (none), so every row scales.
#[test]
fn info_enhance_sinc4x_prints_the_scale_column() {
    let cli_output =
        Command::new(env!("CARGO_BIN_EXE_starplayer-cli")).args(["info", reflex().to_str().expect("a UTF-8 path"), "--enhance", "sinc4x"]).output().expect("the CLI binary runs");

    assert!(cli_output.status.success(), "info --enhance exited with {}: {}", cli_output.status, String::from_utf8_lossy(&cli_output.stderr));
    let stdout = String::from_utf8_lossy(&cli_output.stdout);
    assert!(stdout.contains("×4"), "info --enhance sinc4x should print a ×4 scale row:\n{stdout}");
}

/// A top-level `starplayer --list-enhancers` needs no subcommand at all, mirroring
/// `--list-effects`.
#[test]
fn top_level_list_enhancers_needs_no_subcommand() {
    let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli")).args(["--list-enhancers"]).output().expect("the CLI binary runs");
    assert!(cli_output.status.success(), "--list-enhancers exited with {}: {}", cli_output.status, String::from_utf8_lossy(&cli_output.stderr));
    let stdout = String::from_utf8_lossy(&cli_output.stdout);
    for id in ["denoise", "sinc4x", "sinc2x", "sbr", "loop"] {
        assert!(stdout.contains(id), "top-level --list-enhancers did not list `{id}`:\n{stdout}");
    }
}

/// `render --list-enhancers` needs no `file` or `-o`, and prints every enhancer's id.
#[test]
fn render_list_enhancers_needs_no_file_and_names_every_enhancer() {
    let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli")).args(["render", "--list-enhancers"]).output().expect("the CLI binary runs");
    assert!(cli_output.status.success(), "--list-enhancers exited with {}: {}", cli_output.status, String::from_utf8_lossy(&cli_output.stderr));
    let stdout = String::from_utf8_lossy(&cli_output.stdout);
    for id in ["denoise", "sinc4x", "sinc2x", "sbr", "loop"] {
        assert!(stdout.contains(id), "--list-enhancers did not list `{id}`:\n{stdout}");
    }
}
