//! Build orchestration for the StarPlayer workspace.
//!
//! `cargo xtask ci` runs exactly the matrix that `.github/workflows/ci.yml` runs, so an
//! agent can verify a change locally without pushing. CI invokes the same subcommands
//! one job at a time (`cargo xtask ci --job <job>`), which keeps the two definitions
//! from drifting apart — there is only one definition.
//!
//! No dependencies: `std` only, argv parsed by hand.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
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

/// Optional features that pull in a whole extra crate and therefore need their own
/// bare-metal check: the loop above only ever compiles the *minimal* configuration, so a
/// dependency reachable only through a feature would never be built for the bare-metal
/// target at all.
///
/// `starplayer-engine/telemetry` is the first of these — it turns on the optional
/// `starplayer-telemetry` edge (architecture §11, M1-B6).
const FEATURE_ENABLED_NO_STD_CHECKS: &[(&str, &str)] = &[("starplayer-engine", "telemetry")];

const JOBS: &[&str] = &["host-tests", "wasm-build", "no-std-check", "clippy", "no-std-purity"];

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let subcommand = arguments.first().map(String::as_str);

    let succeeded = match subcommand {
        Some("ci") => run_ci(&arguments[1..]),
        Some("goldens") => not_implemented("goldens", "regenerating golden renders (M1)"),
        Some("wasm") => run_wasm(&arguments[1..]),
        Some("serve") => run_serve(&arguments[1..]),
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
    println!("  wasm [--serve]     build and package the web player into apps/starplayer-web/dist");
    println!("  serve [--port N] [--host ADDR]   serve that directory with the COOP/COEP headers SharedArrayBuffer needs");
    println!("                     --host 0.0.0.0 exposes it to the LAN; add --tls [--tls-san ip,ip] there, since AudioWorklet");
    println!("                     and SharedArrayBuffer only exist in a secure context (https or localhost)");
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

/// The workspace's tests, plus a second pass over `starplayer-engine` with `telemetry`
/// on.
///
/// The second pass is not redundant: `crates/starplayer-engine/tests/telemetry.rs` is
/// `#![cfg(feature = "telemetry")]` in its entirety, so the default-feature run compiles it
/// to nothing. `telemetry` is not in any default feature set — it must not be, since the
/// engine is embeddable without a UI — so without this line the whole of M1-B6's
/// engine-side verification would silently never run.
fn job_host_tests() -> bool {
    cargo(&["test", "--workspace"])
        && cargo(&["test", "-p", "starplayer-engine", "--features", "telemetry"])
}

/// The web build has to keep working on every commit, both for the facade the app
/// consumes and for the AudioWorklet glue that wraps it.
fn job_wasm_build() -> bool {
    cargo(&["build", "--target", WASM_TARGET, "-p", "starplayer"])
        && cargo(&["build", "--target", WASM_TARGET, "-p", "starplayer-host-wasm"])
        && cargo(&["build", "--target", WASM_TARGET, "-p", "starplayer-web"])
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
    for (crate_name, feature) in FEATURE_ENABLED_NO_STD_CHECKS {
        all_succeeded &= cargo(&[
            "check",
            "--target", BARE_METAL_TARGET,
            "--no-default-features",
            "--features", feature,
            "-p", crate_name,
        ]);
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

// ── wasm packaging ──────────────────────────────────────────────────────────────────

/// Everything the web build reads from and writes to, relative to the workspace root.
const WEB_SOURCE_DIRECTORY: &str = "apps/starplayer-web/www";
const WEB_OUTPUT_DIRECTORY: &str = "apps/starplayer-web/dist";
const DEV_SERVER_SCRIPT: &str = "apps/starplayer-web/dev-server.mjs";
const FIXTURE_SOURCE_DIRECTORY: &str = "crates/starplayer-s3m/tests/fixtures";
/// Where those fixtures are served from, relative to the document root.
const PACKAGED_MODULE_DIRECTORY: &str = "modules";

/// The crate compiled to wasm, and the file names `wasm-bindgen` derives from it.
const WASM_CRATE: &str = "starplayer-host-wasm";
const WASM_ARTIFACT: &str = "starplayer_host_wasm.wasm";
const BINDGEN_GLUE: &str = "starplayer_host_wasm.js";

/// The page-side loader is a second wasm instance, generated as an ES module.
const PAGE_WASM_CRATE: &str = "starplayer-web";
const PAGE_WASM_ARTIFACT: &str = "starplayer_web.wasm";

/// The single file `AudioWorklet.addModule()` is pointed at.
const WORKLET_BUNDLE: &str = "starplayer-worklet.js";

/// The pieces concatenated into that bundle, after the `wasm-bindgen` glue. Order
/// matters: `ring.js` installs the protocol the processor then uses.
const WORKLET_BUNDLE_PARTS: &[&str] = &["ring.js", "worklet-processor.js"];

/// Bundled *ahead* of the wasm-bindgen glue, because the glue constructs a `TextDecoder`
/// at the top of its IIFE and `AudioWorkletGlobalScope` does not have one.
const WORKLET_BUNDLE_PRELUDE: &str = "worklet-prelude.js";

/// Sources that exist only to be bundled and are never loaded on their own. `ring.js` is
/// not among them: the page loads it too, because the protocol has two ends.
const WORKLET_ONLY_SOURCES: &[&str] = &["worklet-processor.js", "worklet-prelude.js"];

const DEFAULT_SERVE_PORT: u16 = 8080;
/// Loopback only, unless `serve --host` says otherwise.
const DEFAULT_SERVE_HOST: &str = "127.0.0.1";

/// `cargo build` → `wasm-bindgen` → worklet-scope massaging → a servable directory.
///
/// `wasm-pack` would do the first two steps and is deliberately not used: it adds an
/// npm-package generator this project has no use for, another binary to install, and a
/// second place where the `wasm-bindgen` version is decided. `cargo build` plus
/// `wasm-bindgen-cli` is the whole job, and the version pin then lives in exactly one
/// place — `[workspace.dependencies]` — which this function checks the CLI against.
fn run_wasm(arguments: &[String]) -> bool {
    let mut serve_afterwards = false;
    let mut port = DEFAULT_SERVE_PORT;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--serve" => {
                serve_afterwards = true;
                index += 1;
            }
            "--port" => {
                let Some(value) = arguments.get(index + 1) else {
                    eprintln!("xtask wasm: `--port` needs a value");
                    return false;
                };
                let Ok(parsed) = value.parse::<u16>() else {
                    eprintln!("xtask wasm: `{value}` is not a port number");
                    return false;
                };
                port = parsed;
                index += 2;
            }
            other => {
                eprintln!("xtask wasm: unexpected argument `{other}`");
                return false;
            }
        }
    }

    let root = workspace_root();

    if !check_bindgen_version(&root) {
        return false;
    }
    if !cargo(&[
        "build", "--release", "--target", WASM_TARGET,
        "-p", WASM_CRATE,
        "-p", PAGE_WASM_CRATE,
    ]) {
        return false;
    }

    let output_directory = root.join(WEB_OUTPUT_DIRECTORY);
    // A stale artefact from an earlier layout is worse than a slow build: the page would
    // load it and the failure would look like a bug in the new code.
    if output_directory.exists()
        && let Err(error) = std::fs::remove_dir_all(&output_directory) {
        eprintln!("xtask wasm: cannot clear `{}`: {error}", output_directory.display());
        return false;
    }
    if let Err(error) = std::fs::create_dir_all(&output_directory) {
        eprintln!("xtask wasm: cannot create `{}`: {error}", output_directory.display());
        return false;
    }

    let wasm_artifact = root.join("target").join(WASM_TARGET).join("release").join(WASM_ARTIFACT);
    println!("     wasm-bindgen --target no-modules {}", wasm_artifact.display());
    let bindgen_succeeded = Command::new("wasm-bindgen")
        .args(["--target", "no-modules", "--no-typescript", "--out-dir"])
        .arg(&output_directory)
        .arg(&wasm_artifact)
        .status();
    match bindgen_succeeded {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!("     wasm-bindgen exited with {status}");
            return false;
        }
        Err(error) => {
            eprintln!("     wasm-bindgen failed to start: {error}");
            eprintln!("     install it with `cargo install wasm-bindgen-cli --version {}`", pinned_bindgen_version(&root).unwrap_or_default());
            return false;
        }
    }

    if !build_worklet_bundle(&root, &output_directory) {
        return false;
    }
    if !build_page_loader(&root, &output_directory) {
        return false;
    }
    if !copy_web_sources(&root, &output_directory) {
        return false;
    }
    if !copy_fixture_modules(&root, &output_directory) {
        return false;
    }
    if !report_output(&output_directory) {
        return false;
    }

    println!();
    println!("xtask wasm: packaged into {}", output_directory.display());
    if serve_afterwards {
        return serve(&root, port, DEFAULT_SERVE_HOST, None);
    }
    println!("     run `cargo xtask serve` and open http://localhost:{DEFAULT_SERVE_PORT}/");
    true
}

/// Generate the independent main-thread loader instance. Keeping this separate from the
/// worklet means malformed-file validation and PatternCell display decoding never run in
/// the live render instance.
fn build_page_loader(root: &Path, output_directory: &Path) -> bool {
    let wasm_artifact = root.join("target").join(WASM_TARGET).join("release").join(PAGE_WASM_ARTIFACT);
    println!("     wasm-bindgen --target web {}", wasm_artifact.display());
    match Command::new("wasm-bindgen")
        .args(["--target", "web", "--no-typescript", "--out-dir"])
        .arg(output_directory)
        .arg(&wasm_artifact)
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("     page-side wasm-bindgen exited with {status}");
            false
        }
        Err(error) => {
            eprintln!("     page-side wasm-bindgen failed to start: {error}");
            false
        }
    }
}

/// Assemble the one file the worklet is allowed to load.
///
/// This is the "worklet-scope massaging" the task calls for, and in wasm-bindgen 0.2.127
/// it comes to two things:
///
/// 1. **Concatenation.** `AudioWorklet.addModule()` takes one URL, and inside
///    `AudioWorkletGlobalScope` there is no `fetch`, no `importScripts` and no dependable
///    dynamic `import`. So the glue, the ring protocol and the processor have to arrive
///    as a single script. `--target no-modules` is what makes this possible at all: it
///    emits a self-contained IIFE with no `export`, no `import.meta` and no top-level
///    `await`, all of which `--target web` has and none of which survive concatenation
///    into a worklet.
///
/// 2. **Disarming the script-path sniff.** The glue opens by reading
///    `document.currentScript` to guess where its `.wasm` sits, so that a bare
///    `wasm_bindgen()` call can fetch it. The guard already short-circuits in a worklet,
///    where `document` is undefined — but the branch it protects is the only place the
///    glue reaches for `location` and `fetch`, neither of which exists there. Forcing it
///    off makes that unreachable by construction rather than by luck, and the assertion
///    below turns a future wasm-bindgen that changes the shape of this code into a build
///    failure instead of a silent runtime one.
///
/// 3. **A `TextDecoder`.** The glue constructs one at the top of its IIFE, unconditionally,
///    and the worklet realm has none — so without `worklet-prelude.js` in front of it the
///    bundle throws before `registerProcessor` runs, and the page only finds out later,
///    when `new AudioWorkletNode(...)` reports an undefined processor name. The prelude
///    supplies a decoder and nothing else; the check below fails the build rather than the
///    browser if a future glue starts wanting to encode as well.
fn build_worklet_bundle(root: &Path, output_directory: &Path) -> bool {
    let glue_path = output_directory.join(BINDGEN_GLUE);
    let Ok(glue) = std::fs::read_to_string(&glue_path) else {
        eprintln!("xtask wasm: `{}` was not produced", glue_path.display());
        return false;
    };

    if glue.contains("TextEncoder") {
        eprintln!("xtask wasm: the wasm-bindgen glue now uses `TextEncoder`, which");
        eprintln!("            `AudioWorkletGlobalScope` does not have. Extend");
        eprintln!("            `{WORKLET_BUNDLE_PRELUDE}` with an encoder before shipping this.");
        return false;
    }

    const SCRIPT_SNIFF: &str = "if (typeof document !== 'undefined' && document.currentScript !== null) {";
    if !glue.contains(SCRIPT_SNIFF) {
        eprintln!("xtask wasm: the wasm-bindgen glue no longer contains the `document.currentScript`");
        eprintln!("            sniff this build knows how to disarm. Re-read the generated");
        eprintln!("            `{BINDGEN_GLUE}` and update `build_worklet_bundle` before shipping it");
        eprintln!("            into a worklet.");
        return false;
    }
    let glue = glue.replace(
        SCRIPT_SNIFF,
        "if (false) { // starplayer: disarmed by `xtask wasm` — worklet scope has no document, location or fetch",
    );

    let mut bundle = String::new();
    bundle.push_str("// GENERATED by `cargo xtask wasm` — do not edit.\n");
    bundle.push_str("//\n");
    bundle.push_str("// wasm-bindgen `--target no-modules` glue, then the ring protocol, then the\n");
    bundle.push_str("// AudioWorklet processor, concatenated because a worklet can load exactly one file\n");
    bundle.push_str("// and cannot fetch or import anything once it is running.\n\n");
    let prelude_path = root.join(WEB_SOURCE_DIRECTORY).join(WORKLET_BUNDLE_PRELUDE);
    let Ok(prelude) = std::fs::read_to_string(&prelude_path) else {
        eprintln!("xtask wasm: cannot read `{}`", prelude_path.display());
        return false;
    };
    bundle.push_str(&prelude);
    bundle.push_str("\n\n");
    bundle.push_str(&glue);

    for part in WORKLET_BUNDLE_PARTS {
        let part_path = root.join(WEB_SOURCE_DIRECTORY).join(part);
        let Ok(contents) = std::fs::read_to_string(&part_path) else {
            eprintln!("xtask wasm: cannot read `{}`", part_path.display());
            return false;
        };
        bundle.push_str("\n\n// ── ");
        bundle.push_str(part);
        bundle.push_str(" ──────────────────────────────────────────────────────────\n\n");
        bundle.push_str(&contents);
    }

    let bundle_path = output_directory.join(WORKLET_BUNDLE);
    if let Err(error) = std::fs::write(&bundle_path, bundle) {
        eprintln!("xtask wasm: cannot write `{}`: {error}", bundle_path.display());
        return false;
    }
    // The standalone glue is now dead weight, and leaving it would invite somebody to
    // load it instead of the bundle. The page never instantiates wasm itself — it
    // compiles the module and hands it to the worklet.
    if let Err(error) = std::fs::remove_file(&glue_path) {
        eprintln!("xtask wasm: cannot remove `{}`: {error}", glue_path.display());
        return false;
    }
    println!("     bundled {WORKLET_BUNDLE_PRELUDE} + {BINDGEN_GLUE} + {} → {WORKLET_BUNDLE}", WORKLET_BUNDLE_PARTS.join(" + "));
    true
}

/// Copy the hand-written page into the output directory. Flat by design: this is four
/// files, and a recursive copy would only hide that.
fn copy_web_sources(root: &Path, output_directory: &Path) -> bool {
    let source_directory = root.join(WEB_SOURCE_DIRECTORY);
    let entries = match std::fs::read_dir(&source_directory) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("xtask wasm: cannot read `{}`: {error}", source_directory.display());
            return false;
        }
    };

    let mut all_succeeded = true;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name() else { continue };
        if WORKLET_ONLY_SOURCES.iter().any(|only| std::ffi::OsStr::new(only) == name) {
            continue;
        }
        if let Err(error) = std::fs::copy(&path, output_directory.join(name)) {
            eprintln!("xtask wasm: cannot copy `{}`: {error}", path.display());
            all_succeeded = false;
        }
    }
    all_succeeded
}

/// Ship the five licensed test modules as one-click smoke fixtures. The source remains
/// the format crate's corpus; packaging copies it rather than creating a second checked-in
/// set that could drift.
fn copy_fixture_modules(root: &Path, output_directory: &Path) -> bool {
    let source = root.join(FIXTURE_SOURCE_DIRECTORY);
    let destination = output_directory.join(PACKAGED_MODULE_DIRECTORY);
    if let Err(error) = std::fs::create_dir_all(&destination) {
        eprintln!("xtask wasm: cannot create `{}`: {error}", destination.display());
        return false;
    }
    let names = ["ARMANI.S3M", "MOVEMENT.S3M", "NICETUNE.S3M", "PETRI.S3M", "REFLEX.S3M"];
    for name in names {
        if let Err(error) = std::fs::copy(source.join(name), destination.join(name)) {
            eprintln!("xtask wasm: cannot package fixture `{name}`: {error}");
            return false;
        }
    }
    println!("     packaged {} S3M fixtures", names.len());
    true
}

fn report_output(output_directory: &Path) -> bool {
    let entries = match std::fs::read_dir(output_directory) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("xtask wasm: cannot list `{}`: {error}", output_directory.display());
            return false;
        }
    };
    let mut listing: Vec<(String, u64)> = entries
        .flatten()
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            Some((entry.file_name().to_string_lossy().into_owned(), metadata.len()))
        })
        .collect();
    listing.sort();

    println!();
    for (name, size) in &listing {
        println!("     {size:>9}  {name}");
    }
    true
}

// ── dev server ──────────────────────────────────────────────────────────────────────

/// Serve the packaged build with the COOP/COEP headers `SharedArrayBuffer` requires.
fn run_serve(arguments: &[String]) -> bool {
    let mut port = DEFAULT_SERVE_PORT;
    let mut host = DEFAULT_SERVE_HOST.to_string();
    let mut tls_subject_alt_names: Option<String> = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--tls" => {
                tls_subject_alt_names.get_or_insert_with(String::new);
                index += 1;
            }
            "--tls-san" => {
                let Some(value) = arguments.get(index + 1) else {
                    eprintln!("xtask serve: `--tls-san` needs a comma-separated list");
                    return false;
                };
                tls_subject_alt_names = Some(value.clone());
                index += 2;
            }
            "--host" => {
                let Some(value) = arguments.get(index + 1) else {
                    eprintln!("xtask serve: `--host` needs a value");
                    return false;
                };
                host = value.clone();
                index += 2;
            }
            "--port" => {
                let Some(value) = arguments.get(index + 1) else {
                    eprintln!("xtask serve: `--port` needs a value");
                    return false;
                };
                let Ok(parsed) = value.parse::<u16>() else {
                    eprintln!("xtask serve: `{value}` is not a port number");
                    return false;
                };
                port = parsed;
                index += 2;
            }
            other => {
                eprintln!("xtask serve: unexpected argument `{other}`");
                return false;
            }
        }
    }
    serve(&workspace_root(), port, &host, tls_subject_alt_names.as_deref())
}

/// `tls_subject_alt_names`: `None` for plain http; `Some(list)` for https with a self-signed
/// certificate whose subjectAltName carries the comma-separated `list` (may be empty).
fn serve(root: &Path, port: u16, host: &str, tls_subject_alt_names: Option<&str>) -> bool {
    let output_directory = root.join(WEB_OUTPUT_DIRECTORY);
    if !output_directory.join("index.html").exists() {
        eprintln!("xtask serve: `{}` is not packaged yet — run `cargo xtask wasm` first", output_directory.display());
        return false;
    }

    let script = root.join(DEV_SERVER_SCRIPT);
    let mut node = Command::new("node");
    node.arg(&script).args(["--port", &port.to_string(), "--host", host, "--root"]).arg(&output_directory);
    if let Some(subject_alt_names) = tls_subject_alt_names {
        node.arg("--tls");
        if !subject_alt_names.is_empty() {
            node.args(["--tls-san", subject_alt_names]);
        }
    }
    println!("     node {} --port {port} --host {host}{}", script.display(), if tls_subject_alt_names.is_some() { " --tls" } else { "" });
    match node.status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("     node exited with {status}");
            false
        }
        Err(error) => {
            eprintln!("     node failed to start: {error} — Node 24 or later is required");
            false
        }
    }
}

// ── paths and versions ──────────────────────────────────────────────────────────────

/// The workspace root, derived from this crate's own manifest directory rather than from
/// the current directory, so `cargo xtask` works from anywhere inside the tree.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

/// The exact `wasm-bindgen` version pinned in `[workspace.dependencies]`.
fn pinned_bindgen_version(root: &Path) -> Option<String> {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    for line in manifest.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some(rest) = line.strip_prefix("wasm-bindgen") else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('=') else { continue };
        let version = rest.trim().trim_matches('"').trim_start_matches('=').trim();
        if !version.is_empty() {
            return Some(version.to_string());
        }
    }
    None
}

/// The generated glue and the CLI that produces it must be the same version — a mismatch
/// is a confusing runtime failure rather than a build error, so it is checked here.
fn check_bindgen_version(root: &Path) -> bool {
    let Some(pinned) = pinned_bindgen_version(root) else {
        eprintln!("xtask wasm: no `wasm-bindgen` pin found in [workspace.dependencies]");
        return false;
    };

    let output = match Command::new("wasm-bindgen").arg("--version").output() {
        Ok(output) => output,
        Err(error) => {
            eprintln!("xtask wasm: `wasm-bindgen` is not on PATH: {error}");
            eprintln!("            install it with `cargo install wasm-bindgen-cli --version {pinned}`");
            return false;
        }
    };
    let reported = String::from_utf8_lossy(&output.stdout);
    let installed = reported.split_whitespace().nth(1).unwrap_or("").trim();
    if installed != pinned {
        eprintln!("xtask wasm: wasm-bindgen CLI is {installed}, but the crate is pinned to {pinned}");
        eprintln!("            the glue and the CLI must match; run");
        eprintln!("            `cargo install wasm-bindgen-cli --version {pinned}`");
        return false;
    }
    println!("     wasm-bindgen CLI {installed} matches the pinned crate version");
    true
}
