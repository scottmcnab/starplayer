//! Task D5: "every error is a one-line message and a non-zero exit; no panics on bad
//! input (feed it a fuzz seed)". This drives the CLI over every committed fuzz seed and a
//! handful of hand-built bad inputs and checks only for the one thing that must never
//! happen: a Rust panic reaching the process boundary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cli_with<const N: usize>(args: [&str; N]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_starplayer-cli"));
    command.args(args);
    command
}

fn assert_no_panic(mut command: Command, label: &str) {
    let output = command.output().expect("the CLI binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked at"), "{label}: the CLI panicked:\n{stderr}");
}

#[test]
fn every_committed_fuzz_seed_is_handled_without_panicking() {
    for format in ["mod", "s3m", "mtm"] {
        let directory = workspace_root().join("fuzz/seeds").join(format);
        let Ok(entries) = std::fs::read_dir(&directory) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.file_name().is_some_and(|name| name == "README.md") {
                continue;
            }
            let path_string = path.to_str().expect("a UTF-8 fixture path").to_string();
            assert_no_panic(cli_with(["info", &path_string]), &format!("info {}", path.display()));
            assert_no_panic(cli_with(["trace", &path_string, "--ticks", "4"]), &format!("trace {}", path.display()));
        }
    }
}

#[test]
fn garbage_truncated_and_missing_input_fail_cleanly_rather_than_panicking() {
    let scratch_directory = std::env::temp_dir().join(format!("starplayer-cli-robustness-{}", std::process::id()));
    std::fs::create_dir_all(&scratch_directory).expect("a scratch directory");

    let empty = scratch_directory.join("empty.s3m");
    std::fs::write(&empty, []).expect("writes an empty file");
    let garbage = scratch_directory.join("garbage.mod");
    std::fs::write(&garbage, [0xFFu8; 37]).expect("writes garbage bytes");
    let truncated_zip = scratch_directory.join("truncated.zip");
    std::fs::write(&truncated_zip, b"PK\x03\x04not really a zip archive").expect("writes a fake zip header");
    let missing = scratch_directory.join("does-not-exist.s3m");

    for path in [&empty, &garbage, &truncated_zip, &missing] {
        let path_string = path.to_str().expect("a UTF-8 path").to_string();
        let output = cli_with(["info", &path_string]).output().expect("the CLI binary runs");
        assert!(!output.status.success(), "{}: bad input unexpectedly succeeded", path.display());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("panicked at"), "{}: the CLI panicked:\n{stderr}", path.display());
        assert_eq!(stderr.lines().count(), 1, "{}: the error should be one line, was:\n{stderr}", path.display());
    }

    let _ = std::fs::remove_dir_all(&scratch_directory);
}
