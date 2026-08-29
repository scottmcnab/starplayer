//! Build orchestration for the StarPlayer workspace.
//!
//! `cargo xtask ci` runs exactly the matrix that `.github/workflows/ci.yml` runs, so an
//! agent can verify a change locally without pushing. CI invokes the same subcommands
//! one job at a time (`cargo xtask ci --job <job>`), which keeps the two definitions
//! from drifting apart — there is only one definition.
//!
//! No dependencies: `std` only, argv parsed by hand.

#![forbid(unsafe_code)]

use std::process::{Command, ExitCode};

/// Bare-metal target used for both the `no_std` check and the no-std purity guard.
/// `std` is unavailable here, so any crate that reaches for it fails to compile.
const BARE_METAL_TARGET: &str = "riscv32imc-unknown-none-elf";

const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// Every crate that must stay `no_std`. The facade is last so a purity failure inside it
/// is reported after the crate that actually caused it.
const NO_STD_CRATES: &[&str] = &[
    "starplayer-core",
    "starplayer-rt",
    "starplayer-dsp",
    "starplayer-mixer",
    "starplayer-model",
    "starplayer-engine",
    "starplayer-mod",
    "starplayer-s3m",
    "starplayer-mtm",
    "starplayer-xm",
    "starplayer-it",
    "starplayer-midi",
    "starplayer-telemetry",
    "starplayer",
];

const JOBS: &[&str] = &["host-tests", "wasm-build", "no-std-check", "clippy", "no-std-purity"];

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let subcommand = arguments.first().map(String::as_str);

    let succeeded = match subcommand {
        Some("ci") => run_ci(&arguments[1..]),
        Some("goldens") => not_implemented("goldens", "regenerating golden renders (M1)"),
        Some("wasm") => not_implemented("wasm", "packaging the web build (M0-A4)"),
        Some("help") | Some("--help") | Some("-h") | None => {
            print_usage();
            true
        }
        Some(other) => {
            eprintln!("xtask: unknown subcommand `{other}`\n");
            print_usage();
            false
        }
    };

    if succeeded { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn print_usage() {
    println!("usage: cargo xtask <subcommand>");
    println!();
    println!("subcommands:");
    println!("  ci [--job <job>]   run the CI matrix locally, or a single job of it");
    println!("  goldens            regenerate golden renders (not implemented)");
    println!("  wasm               package the web build (not implemented)");
    println!();
    println!("ci jobs:");
    for job in JOBS {
        println!("  {job}");
    }
}

fn not_implemented(subcommand: &str, what: &str) -> bool {
    println!("xtask {subcommand}: not implemented — {what}");
    true
}

fn run_ci(arguments: &[String]) -> bool {
    let requested_job = match parse_job_argument(arguments) {
        Ok(job) => job,
        Err(message) => {
            eprintln!("xtask ci: {message}");
            return false;
        }
    };

    let jobs: Vec<&str> = match requested_job {
        Some(job) => vec![job],
        None => JOBS.to_vec(),
    };

    let mut failed_jobs: Vec<&str> = Vec::new();
    for job in &jobs {
        println!("\n=== xtask ci: {job} ===");
        let succeeded = match *job {
            "host-tests" => job_host_tests(),
            "wasm-build" => job_wasm_build(),
            "no-std-check" => job_no_std_check(),
            "clippy" => job_clippy(),
            "no-std-purity" => job_no_std_purity(),
            other => {
                eprintln!("xtask ci: unknown job `{other}`");
                false
            }
        };
        if !succeeded {
            failed_jobs.push(job);
        }
    }

    println!();
    if failed_jobs.is_empty() {
        println!("xtask ci: {} job(s) passed", jobs.len());
        true
    } else {
        println!("xtask ci: FAILED — {}", failed_jobs.join(", "));
        false
    }
}

fn parse_job_argument(arguments: &[String]) -> Result<Option<&str>, String> {
    let mut requested_job = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--job" => {
                let value = arguments.get(index + 1).ok_or("`--job` needs a value")?;
                if !JOBS.contains(&value.as_str()) {
                    return Err(format!("unknown job `{value}`; known jobs: {}", JOBS.join(", ")));
                }
                requested_job = Some(value.as_str());
                index += 2;
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(requested_job)
}

// ── jobs ────────────────────────────────────────────────────────────────────────────

fn job_host_tests() -> bool {
    cargo(&["test", "--workspace"])
}

/// The web build has to keep working on every commit, both for the facade the app
/// consumes and for the AudioWorklet glue that wraps it.
fn job_wasm_build() -> bool {
    cargo(&["build", "--target", WASM_TARGET, "-p", "starplayer"])
        && cargo(&["build", "--target", WASM_TARGET, "-p", "starplayer-host-wasm"])
}

/// Each `no_std` crate must compile for a bare-metal target with its features stripped
/// back to nothing. This is the minimal-configuration half of the portability check.
///
/// There is deliberately no *clippy* run on the bare-metal target. Clippy's findings are
/// target-independent apart from code behind `#[cfg(target_arch = ...)]` and friends, and
/// there is none of that yet, so the host clippy job already lints every line the
/// bare-metal build compiles. Revisit when `starplayer-dsp` grows its cfg-gated SIMD
/// backends: at that point the scalar and simd128 arms stop being covered by host clippy,
/// and a `cargo clippy --target riscv32imc-unknown-none-elf --workspace` run belongs here
/// (measured at well under a second on top of the checks this job already does).
fn job_no_std_check() -> bool {
    let mut all_succeeded = true;
    for crate_name in NO_STD_CRATES {
        let succeeded = cargo(&[
            "check",
            "--target", BARE_METAL_TARGET,
            "--no-default-features",
            "-p", crate_name,
        ]);
        all_succeeded &= succeeded;
    }
    all_succeeded
}

fn job_clippy() -> bool {
    cargo(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])
}

/// The no-std purity guard, and the enforcement point for AGENTS.md design goal 4:
/// *no default feature transitively enables `std`.*
///
/// Three complementary checks, because each catches what the others miss:
///
/// 1. Compile every `no_std` crate for the bare-metal target with its **default**
///    features on. `std` does not exist for that target, so any use of it — direct, or
///    dragged in by a dependency's default feature — is a hard compile error. This is
///    the strong signal, and the one that catches a stray `use std::...` in our code.
/// 2. Ask cargo which dependency features the default resolution actually turns on, for
///    the same target, and reject any `std` among them. This catches a crate that wires
///    a dependency's `std` into a default before any code uses it — a latent violation
///    check 1 cannot see.
/// 3. Read every manifest's own `default = [...]` list and reject `"std"` and any
///    `".../std"` entry. Check 2 never sees the root crate's own feature list, so this
///    is what closes the loop on the crate being checked.
fn job_no_std_purity() -> bool {
    let mut all_succeeded = true;

    for crate_name in NO_STD_CRATES {
        all_succeeded &= cargo(&["check", "--target", BARE_METAL_TARGET, "-p", crate_name]);
    }
    for crate_name in NO_STD_CRATES {
        all_succeeded &= assert_resolved_features_exclude_std(crate_name);
    }
    all_succeeded &= assert_manifest_defaults_exclude_std();

    all_succeeded
}

fn assert_resolved_features_exclude_std(crate_name: &str) -> bool {
    let arguments = ["tree", "--edges", "features", "--target", BARE_METAL_TARGET, "-p", crate_name];
    println!("     cargo {}", arguments.join(" "));

    let output = match Command::new(cargo_binary()).args(arguments).output() {
        Ok(output) => output,
        Err(error) => {
            eprintln!("     cargo tree failed to start: {error}");
            return false;
        }
    };
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        eprintln!("     cargo tree failed for `{crate_name}`");
        return false;
    }

    let tree = String::from_utf8_lossy(&output.stdout);
    let offenders: Vec<&str> = tree.lines()
        .map(trim_tree_glyphs)
        .filter(|line| line.ends_with("feature \"std\""))
        .collect();

    if offenders.is_empty() {
        return true;
    }
    eprintln!("     no-std purity violation: `{crate_name}` enables `std` by default:");
    for offender in offenders {
        eprintln!("       {offender}");
    }
    false
}

/// Check 3: no manifest in the workspace lists `std` — its own, or a dependency's — in
/// its `default` feature set.
fn assert_manifest_defaults_exclude_std() -> bool {
    println!("     scanning manifests for `std` in a default feature set");

    let mut manifests: Vec<std::path::PathBuf> = Vec::new();
    for directory in ["crates", "apps"] {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!("     cannot read `{directory}`: {error}");
                return false;
            }
        };
        for entry in entries.flatten() {
            manifests.push(entry.path().join("Cargo.toml"));
        }
    }
    manifests.sort();

    let mut all_succeeded = true;
    for manifest in &manifests {
        let text = match std::fs::read_to_string(manifest) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("     cannot read `{}`: {error}", manifest.display());
                all_succeeded = false;
                continue;
            }
        };
        for entry in default_feature_entries(&text) {
            if entry == "std" || entry.ends_with("/std") {
                eprintln!("     no-std purity violation: `{}` has `{entry}` in its default features", manifest.display());
                all_succeeded = false;
            }
        }
    }
    all_succeeded
}

/// Collect the entries of the `default = [...]` list in a manifest's `[features]`
/// section. Comments and whitespace are ignored; the list may span several lines.
fn default_feature_entries(manifest_text: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut in_features_section = false;
    let mut collecting = false;

    for raw_line in manifest_text.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_features_section = line == "[features]";
            collecting = false;
            continue;
        }
        if !in_features_section {
            continue;
        }
        let payload = if !collecting {
            let Some(rest) = line.strip_prefix("default") else { continue };
            let Some(rest) = rest.trim_start().strip_prefix('=') else { continue };
            collecting = true;
            rest.trim_start().trim_start_matches('[')
        } else {
            line
        };
        let closed = payload.contains(']');
        for token in payload.trim_end_matches(']').split(',') {
            let token = token.trim().trim_matches('"');
            if !token.is_empty() {
                entries.push(token.to_string());
            }
        }
        if closed {
            collecting = false;
        }
    }
    entries
}

/// `cargo tree` prefixes each line with box-drawing glyphs; strip them so the feature
/// name can be matched against the end of the line.
fn trim_tree_glyphs(line: &str) -> &str {
    line.trim_start_matches(|character: char| {
        character.is_whitespace() || "│├└─".contains(character)
    })
    .trim_end()
    .trim_end_matches("(*)")
    .trim_end()
}

// ── process plumbing ────────────────────────────────────────────────────────────────

fn cargo_binary() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn cargo(arguments: &[&str]) -> bool {
    println!("     cargo {}", arguments.join(" "));
    match Command::new(cargo_binary()).args(arguments).status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("     cargo {} exited with {status}", arguments.join(" "));
            false
        }
        Err(error) => {
            eprintln!("     cargo failed to start: {error}");
            false
        }
    }
}
