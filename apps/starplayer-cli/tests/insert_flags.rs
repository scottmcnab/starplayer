//! M7-H7: `render --insert` and `--list-effects`, exercised through the real CLI binary
//! rather than `render.rs`'s own unit tests — the acceptance-level counterpart to
//! `golden_reproduction.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

use starplayer_offline::wav::read_wav;

fn workspace_root() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("../..") }

fn reflex() -> PathBuf { workspace_root().join("crates/starplayer-s3m/tests/fixtures/REFLEX.S3M") }

/// `--golden` and `--insert` are mutually exclusive (the eleven goldens are DSP-bypassed
/// by policy §5.5): clap refuses the combination before any rendering happens.
#[test]
fn golden_refuses_insert() {
    let scratch = std::env::temp_dir().join(format!("starplayer-cli-insert-golden-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let output = scratch.join("out.wav");

    let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli"))
        .args(["render", reflex().to_str().expect("a UTF-8 path"), "--golden", "--insert", "1:reverb", "-o", output.to_str().expect("a UTF-8 path")])
        .output()
        .expect("the CLI binary runs");

    assert!(!cli_output.status.success(), "--golden and --insert must not both be accepted");
    assert!(!output.exists(), "no file should have been written");
    let _ = std::fs::remove_dir_all(&scratch);
}

/// `render --insert` on a real channel changes the rendered audio against the same render
/// with no insert at all — the acceptance-level form of the audibility claim
/// `starplayer-host`'s and `starplayer-offline`'s own unit tests make at their level.
#[test]
fn an_insert_changes_the_rendered_audio() {
    let scratch = std::env::temp_dir().join(format!("starplayer-cli-insert-audible-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let plain_output = scratch.join("plain.wav");
    let reverb_output = scratch.join("reverb.wav");

    let input = reflex();
    let run = |output: &Path, extra_args: &[&str]| {
        let mut args = vec!["render", input.to_str().expect("a UTF-8 path"), "-o", output.to_str().expect("a UTF-8 path"), "--max-seconds", "5"];
        args.extend_from_slice(extra_args);
        let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli")).args(&args).output().expect("the CLI binary runs");
        assert!(cli_output.status.success(), "render exited with {}: {}", cli_output.status, String::from_utf8_lossy(&cli_output.stderr));
    };

    run(&plain_output, &[]);
    run(&reverb_output, &["--insert", "1:reverb:room=60,mix=50"]);

    let (_, plain_samples) = read_wav::<f32>(&plain_output).expect("the plain render reads back");
    let (_, reverb_samples) = read_wav::<f32>(&reverb_output).expect("the reverb render reads back");
    assert_eq!(plain_samples.len(), reverb_samples.len(), "an insert must not change the render's length");
    assert_ne!(plain_samples, reverb_samples, "installing a reverb on channel 1 must audibly change the render");

    let _ = std::fs::remove_dir_all(&scratch);
}

/// A top-level `starplayer --list-effects` needs no subcommand at all.
#[test]
fn top_level_list_effects_needs_no_subcommand() {
    let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli")).args(["--list-effects"]).output().expect("the CLI binary runs");
    assert!(cli_output.status.success(), "--list-effects exited with {}: {}", cli_output.status, String::from_utf8_lossy(&cli_output.stderr));
    let stdout = String::from_utf8_lossy(&cli_output.stdout);
    for name in ["gain", "eq", "delay", "chorus", "reverb", "compressor"] {
        assert!(stdout.contains(name), "top-level --list-effects did not list `{name}`:\n{stdout}");
    }
}

/// `render --list-effects` needs no `file` or `-o`, and prints every effect's name.
#[test]
fn list_effects_needs_no_file_and_names_every_effect() {
    let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli")).args(["render", "--list-effects"]).output().expect("the CLI binary runs");
    assert!(cli_output.status.success(), "--list-effects exited with {}: {}", cli_output.status, String::from_utf8_lossy(&cli_output.stderr));
    let stdout = String::from_utf8_lossy(&cli_output.stdout);
    for name in ["gain", "eq", "delay", "chorus", "reverb", "compressor"] {
        assert!(stdout.contains(name), "--list-effects did not list `{name}`:\n{stdout}");
    }
}

/// An unknown effect name is refused with a message naming the real choices, rather than
/// silently rendering as if `--insert` had not been given.
#[test]
fn an_unknown_effect_name_is_refused() {
    let scratch = std::env::temp_dir().join(format!("starplayer-cli-insert-unknown-test-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).expect("a scratch directory");
    let output = scratch.join("out.wav");

    let cli_output = Command::new(env!("CARGO_BIN_EXE_starplayer-cli"))
        .args(["render", reflex().to_str().expect("a UTF-8 path"), "--insert", "1:flanger", "-o", output.to_str().expect("a UTF-8 path")])
        .output()
        .expect("the CLI binary runs");

    assert!(!cli_output.status.success(), "an unknown effect name must be refused");
    let stderr = String::from_utf8_lossy(&cli_output.stderr);
    assert!(stderr.contains("gain"), "the error should name the real effects: {stderr}");
    let _ = std::fs::remove_dir_all(&scratch);
}
