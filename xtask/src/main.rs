//! Build orchestration for the StarPlayer workspace.
//!
//! `cargo xtask ci` runs the host-portable part of `.github/workflows/ci.yml`, and CI
//! invokes those subcommands one job at a time (`cargo xtask ci --job <job>`). The
//! workflow additionally repeats the golden job on a native ARM64 runner and executes
//! the WASIp1 golden helper under Wasmtime; those target-specific executions cannot be
//! reproduced by a host xtask process alone.
//!
//! No dependencies: `std` only, argv parsed by hand.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Bare-metal target used for both the `no_std` check and the no-std purity guard.
/// `std` is unavailable here, so any crate that reaches for it fails to compile.
const BARE_METAL_TARGET: &str = "riscv32imc-unknown-none-elf";

const WASM_TARGET: &str = "wasm32-unknown-unknown";

const LIBXMP_CORPUS_REVISION: &str = "6ec0ba21b1b28f91e22b68a51d59207c6bbf6139";
const LIBXMP_CORPUS_SHA256: &str = "5cb12ffba9371779a7e7c46c4494480f2c52a498db855e99c8a479c5fbba9f20";
const LIBXMP_CORPUS_URL: &str = "https://codeload.github.com/libxmp/libxmp/tar.gz/6ec0ba21b1b28f91e22b68a51d59207c6bbf6139";
const CONFORMANCE_MANIFEST: &str = "conformance/cases.tsv";
const CONFORMANCE_EXCLUSIONS: &str = "conformance/exclusions.tsv";

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

const JOBS: &[&str] = &["host-tests", "conformance", "goldens", "fma-check", "trace-zero-cost", "wasm-build", "no-std-check", "clippy", "no-std-purity"];

/// The LLVM policy `.cargo/config.toml` sets for the float path. Cargo **replaces**
/// `[build] rustflags` when `RUSTFLAGS` is set rather than merging, so a job or shell that
/// exports the variable drops this flag without any warning; `assert_rustflags_keep_the_fp_contract_policy`
/// is what makes that loud.
const FP_CONTRACT_FLAG: &str = "-fp-contract=off";

/// One crate in the FMA contraction audit.
///
/// Trailing `rustc` arguments and `--emit` reach the **final** crate of a `cargo rustc`
/// invocation only, so one pass audits exactly one crate. C6 had a single
/// `starplayer-offline` pass and therefore never built the non-generic float master bus
/// with `+fma` at all.
struct FmaPass {
    package: &'static str,
    /// Whether an optimized build of this crate on its own must contain an `f32` multiply
    /// for its scan to mean anything.
    requires_float_multiply: bool,
}

const FMA_PASSES: &[FmaPass] = &[
    // Monomorphises the whole float voice path: `FloatPath::mix::<Linear>`,
    // `Linear::sample_f32`, the master bus and the host output conversions.
    FmaPass { package: "starplayer-offline", requires_float_multiply: true },
    // The non-generic float master bus itself — `process_float`, `soft_knee_f32` and
    // `bound_f32` — which no C6 pass ever compiled with `+fma`.
    FmaPass { package: "starplayer-mixer", requires_float_multiply: true },
    // Every float expression in `starplayer-dsp` sits in a generic or trait-impl method,
    // so the crate's own rlib codegens none of them and this scan finds nothing today;
    // those instantiations are audited by the two passes above. The pass runs anyway, so
    // the first non-generic float helper added here is covered from that commit onwards.
    FmaPass { package: "starplayer-dsp", requires_float_multiply: false },
];

/// Crates whose tracker processors carry a `#[cfg(feature = "trace")]` per-channel report
/// loop, and the symbol that loop is compiled into.
const TRACE_HOOK_PACKAGES: &[&str] = &["starplayer-mod", "starplayer-s3m"];
const TRACE_HOOK_SYMBOL: &str = "report_trace_channels";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let subcommand = arguments.first().map(String::as_str);

    let succeeded = match subcommand {
        Some("ci") => run_ci(&arguments[1..]),
        Some("conformance") => run_conformance(&arguments[1..]),
        Some("goldens") => run_goldens(&arguments[1..]),
        Some("trace") => run_trace(&arguments[1..]),
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
    println!("  conformance [--offline|--fetch-only] [--strict] [--archive PATH]   acquire and run the pinned tracker corpora");
    println!("  goldens [--check]  regenerate canonical SHA-256 renders, or verify them");
    println!("  trace <module> [--ticks N]   print a stable per-tick state trace");
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

/// Run the std-only offline trace driver without making xtask depend on the audio crate
/// graph. Keeping xtask dependency-free is a repository invariant; the helper binary is
/// also useful to hosts that want the same capture path directly.
fn run_trace(arguments: &[String]) -> bool {
    if arguments.is_empty() {
        eprintln!("xtask trace: usage: cargo xtask trace <module> [--ticks N]");
        return false;
    }
    let mut command = Command::new(cargo_binary());
    command
        .current_dir(workspace_root())
        // The parent `cargo xtask` process owns the workspace target-directory lock for
        // as long as xtask runs. A dedicated target dir avoids recursively waiting on our
        // own lock while preserving xtask's dependency-free manifest.
        // `starplayer-trace` carries `required-features = ["trace"]`: the feature is
        // optional precisely so an ordinary workspace build never links the recorder.
        .args(["run", "--quiet", "--target-dir", "target/xtask-trace", "-p", "starplayer-offline", "--features", "trace", "--bin", "starplayer-trace", "--"])
        .args(arguments);
    match command.status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("xtask trace: offline driver exited with {status}");
            false
        }
        Err(error) => {
            eprintln!("xtask trace: offline driver failed to start: {error}");
            false
        }
    }
}

/// Run the std-only golden driver without adding audio or digest dependencies to xtask.
/// A dedicated target directory avoids recursively waiting on the parent cargo process's
/// target-directory lock.
fn run_goldens(arguments: &[String]) -> bool {
    if !(arguments.is_empty() || matches!(arguments, [flag] if flag == "--check")) {
        eprintln!("xtask goldens: usage: cargo xtask goldens [--check]");
        return false;
    }
    let mut command = Command::new(cargo_binary());
    command
        .current_dir(workspace_root())
        .args(["run", "--quiet", "--target-dir", "target/xtask-goldens", "-p", "starplayer-offline", "--bin", "starplayer-goldens", "--"])
        .args(arguments);
    match command.status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("xtask goldens: offline driver exited with {status}");
            false
        }
        Err(error) => {
            eprintln!("xtask goldens: offline driver failed to start: {error}");
            false
        }
    }
}

/// Acquire the immutable corpus snapshot and run the std-only testkit driver.
///
/// `--offline` refuses acquisition and is the CI gate. `--fetch-only` prepares the cache
/// without compiling or running the harness. `--archive PATH` lets maintainers validate
/// a previously downloaded archive while still enforcing the pinned checksum. `--strict`
/// fails the run while any known-failure exclusion remains; the `conformance` CI job
/// deliberately runs the informational form until C5, C7 and C9 land — making it strict
/// is the one-line change in `job_conformance`.
fn run_conformance(arguments: &[String]) -> bool {
    let mut offline = false;
    let mut fetch_only = false;
    let mut strict = false;
    let mut supplied_archive: Option<PathBuf> = None;
    let mut index = 0usize;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--offline" => { offline = true; index += 1; }
            "--fetch-only" => { fetch_only = true; index += 1; }
            "--strict" => { strict = true; index += 1; }
            "--archive" => {
                let Some(path) = arguments.get(index + 1) else {
                    eprintln!("xtask conformance: `--archive` needs a path");
                    return false;
                };
                supplied_archive = Some(PathBuf::from(path));
                index += 2;
            }
            other => {
                eprintln!("xtask conformance: unexpected argument `{other}`");
                return false;
            }
        }
    }
    if offline && supplied_archive.is_some() {
        eprintln!("xtask conformance: `--offline` and `--archive` are mutually exclusive");
        return false;
    }

    let root = workspace_root();
    let corpus = conformance_corpus_directory(&root);
    if supplied_archive.is_some() || !corpus_is_ready(&corpus) {
        if offline {
            eprintln!("xtask conformance: pinned corpus is not cached at `{}`", corpus.display());
            eprintln!("                   run `cargo xtask conformance --fetch-only` during acquisition");
            return false;
        }
        if !acquire_conformance_corpus(&root, &corpus, supplied_archive.as_deref()) {
            return false;
        }
    }

    println!("xtask conformance: libxmp corpus {LIBXMP_CORPUS_REVISION}");
    if fetch_only {
        println!("xtask conformance: cache ready at {}", corpus.display());
        return true;
    }

    let mut command = Command::new(cargo_binary());
    command
        .current_dir(&root)
        .args([
            "run", "--quiet", "--target-dir", "target/xtask-conformance",
            "-p", "starplayer-testkit", "--features", "trace", "--bin", "starplayer-conformance", "--",
            "--corpus",
        ])
        .arg(corpus.join("test-dev"))
        .args(["--manifest", CONFORMANCE_MANIFEST, "--exclusions", CONFORMANCE_EXCLUSIONS]);
    if strict {
        command.arg("--strict");
    }
    match command.status() {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("xtask conformance: harness exited with {status}");
            false
        }
        Err(error) => {
            eprintln!("xtask conformance: harness failed to start: {error}");
            false
        }
    }
}

fn conformance_corpus_directory(root: &Path) -> PathBuf {
    root.join("target/conformance/corpora").join(format!("libxmp-{LIBXMP_CORPUS_REVISION}"))
}

fn corpus_is_ready(corpus: &Path) -> bool {
    let marker = std::fs::read_to_string(corpus.join(".starplayer-revision"));
    matches!(marker, Ok(value) if value.trim() == LIBXMP_CORPUS_REVISION) && corpus.join("test-dev").is_dir()
}

fn acquire_conformance_corpus(root: &Path, corpus: &Path, supplied_archive: Option<&Path>) -> bool {
    let downloads = root.join("target/conformance/downloads");
    if let Err(error) = std::fs::create_dir_all(&downloads) {
        eprintln!("xtask conformance: cannot create `{}`: {error}", downloads.display());
        return false;
    }
    let cached_archive = downloads.join(format!("libxmp-{LIBXMP_CORPUS_REVISION}.tar.gz"));
    let archive = supplied_archive.unwrap_or(&cached_archive);
    if supplied_archive.is_none() && !archive.is_file() && !download_corpus_archive(archive) {
        return false;
    }
    if !verify_sha256(archive, LIBXMP_CORPUS_SHA256) {
        if supplied_archive.is_none() {
            let _ = std::fs::remove_file(archive);
        }
        return false;
    }

    let Some(parent) = corpus.parent() else {
        eprintln!("xtask conformance: corpus path has no parent");
        return false;
    };
    if let Err(error) = std::fs::create_dir_all(parent) {
        eprintln!("xtask conformance: cannot create `{}`: {error}", parent.display());
        return false;
    }
    let partial = parent.join(format!(".libxmp-{LIBXMP_CORPUS_REVISION}.partial"));
    if partial.exists() && let Err(error) = std::fs::remove_dir_all(&partial) {
        eprintln!("xtask conformance: cannot clear partial extraction `{}`: {error}", partial.display());
        return false;
    }
    if let Err(error) = std::fs::create_dir_all(&partial) {
        eprintln!("xtask conformance: cannot create `{}`: {error}", partial.display());
        return false;
    }

    println!("     tar -xzf {}", archive.display());
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(archive)
        .args(["-C"])
        .arg(&partial)
        .arg("--strip-components=1")
        .status();
    if !matches!(status, Ok(status) if status.success()) {
        eprintln!("xtask conformance: failed to extract `{}`", archive.display());
        let _ = std::fs::remove_dir_all(&partial);
        return false;
    }
    if let Err(error) = std::fs::write(partial.join(".starplayer-revision"), format!("{LIBXMP_CORPUS_REVISION}\n")) {
        eprintln!("xtask conformance: cannot write revision marker: {error}");
        let _ = std::fs::remove_dir_all(&partial);
        return false;
    }
    if corpus.exists() && let Err(error) = std::fs::remove_dir_all(corpus) {
        eprintln!("xtask conformance: cannot replace stale corpus `{}`: {error}", corpus.display());
        let _ = std::fs::remove_dir_all(&partial);
        return false;
    }
    if let Err(error) = std::fs::rename(&partial, corpus) {
        eprintln!("xtask conformance: cannot install corpus at `{}`: {error}", corpus.display());
        let _ = std::fs::remove_dir_all(&partial);
        return false;
    }
    true
}

fn download_corpus_archive(destination: &Path) -> bool {
    let partial = destination.with_extension("tar.gz.partial");
    let _ = std::fs::remove_file(&partial);
    println!("     curl {LIBXMP_CORPUS_URL}");
    let status = Command::new("curl")
        .args(["--fail", "--location", "--retry", "3", "--output"])
        .arg(&partial)
        .arg(LIBXMP_CORPUS_URL)
        .status();
    if !matches!(status, Ok(status) if status.success()) {
        eprintln!("xtask conformance: failed to download the pinned libxmp archive");
        let _ = std::fs::remove_file(&partial);
        return false;
    }
    if let Err(error) = std::fs::rename(&partial, destination) {
        eprintln!("xtask conformance: cannot cache `{}`: {error}", destination.display());
        let _ = std::fs::remove_file(&partial);
        return false;
    }
    true
}

fn verify_sha256(path: &Path, expected: &str) -> bool {
    println!("     sha256sum {}", path.display());
    let output = Command::new("sha256sum").arg(path).output();
    let Ok(output) = output else {
        eprintln!("xtask conformance: `sha256sum` is required to verify the corpus archive");
        return false;
    };
    if !output.status.success() {
        eprintln!("xtask conformance: sha256sum failed for `{}`", path.display());
        return false;
    }
    let actual = String::from_utf8_lossy(&output.stdout).split_whitespace().next().unwrap_or("").to_string();
    if actual != expected {
        eprintln!("xtask conformance: checksum mismatch for `{}`", path.display());
        eprintln!("                   expected {expected}");
        eprintln!("                   actual   {actual}");
        return false;
    }
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

    if !assert_rustflags_keep_the_fp_contract_policy() {
        return false;
    }

    let mut failed_jobs: Vec<&str> = Vec::new();
    for job in &jobs {
        println!("\n=== xtask ci: {job} ===");
        let succeeded = match *job {
            "host-tests" => job_host_tests(),
            "conformance" => job_conformance(),
            "goldens" => run_goldens(&["--check".to_string()]),
            "fma-check" => job_fma_check(),
            "trace-zero-cost" => job_trace_zero_cost(),
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
    assert_trace_stays_off_the_default_workspace_build()
        && cargo(&["test", "--workspace"])
        && cargo(&["test", "-p", "starplayer-engine", "--features", "telemetry"])
        // The recorder, the trace capture path and the trace differ only exist with
        // `trace` on, so without these three passes nothing would ever test them. They are
        // the mirror image of the telemetry pass above.
        && cargo(&["test", "-p", "starplayer-engine", "--features", "trace"])
        && cargo(&["test", "-p", "starplayer-offline", "--features", "trace"])
        && cargo(&["test", "-p", "starplayer-testkit", "--features", "trace"])
}

/// Resolver 2 unifies features across a workspace build, so one crate declaring
/// `starplayer = { features = ["trace"] }` as an ordinary dependency puts the per-tick
/// recorder inside `render()` for every other crate's tests — and for the golden hashes.
/// `trace` is an optional feature of `starplayer-offline` and `starplayer-testkit` for
/// exactly that reason; this asks cargo what it actually resolved rather than trusting the
/// manifests.
fn assert_trace_stays_off_the_default_workspace_build() -> bool {
    let arguments = ["tree", "--edges", "features", "--workspace"];
    println!("     cargo {}", arguments.join(" "));

    let output = match Command::new(cargo_binary()).current_dir(workspace_root()).args(arguments).output() {
        Ok(output) => output,
        Err(error) => {
            eprintln!("     cargo tree failed to start: {error}");
            return false;
        }
    };
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        eprintln!("     cargo tree failed for the default workspace resolution");
        return false;
    }

    let tree = String::from_utf8_lossy(&output.stdout);
    let offenders: Vec<&str> = tree.lines()
        .map(trim_tree_glyphs)
        .filter(|line| line.ends_with("feature \"trace\""))
        .collect();
    if offenders.is_empty() {
        println!("     `trace` is off for the default workspace build");
        return true;
    }
    eprintln!("     `trace` leaked into the default workspace build:");
    for offender in offenders {
        eprintln!("       {offender}");
    }
    eprintln!("     make it an optional feature and put it behind `required-features` on the binary that needs it");
    false
}

/// Cargo replaces `[build] rustflags` when `RUSTFLAGS` is set — it does not merge — so an
/// exported `RUSTFLAGS` silently drops the float-contraction policy for every job in this
/// process. Setting it to anything that does not carry the flag is a hard failure rather
/// than a warning, because the audit downstream would then be testing a configuration
/// nobody ships.
fn assert_rustflags_keep_the_fp_contract_policy() -> bool {
    match std::env::var("RUSTFLAGS") {
        Err(_) => true,
        Ok(value) if value.contains(FP_CONTRACT_FLAG) => {
            println!("xtask ci: RUSTFLAGS carries `{FP_CONTRACT_FLAG}` explicitly");
            true
        }
        Ok(value) => {
            eprintln!("xtask ci: RUSTFLAGS is set to `{value}`");
            eprintln!("          cargo *replaces* `[build] rustflags` from .cargo/config.toml when RUSTFLAGS is set,");
            eprintln!("          so `-C llvm-args={FP_CONTRACT_FLAG}` is silently dropped from every build in this shell.");
            eprintln!("          Unset RUSTFLAGS, or add `-C llvm-args={FP_CONTRACT_FLAG}` to it.");
            false
        }
    }
}

/// Compile every crate that carries float code for an x86-64 CPU where FMA is explicitly
/// available, then inspect both optimized LLVM IR and machine code.
///
/// # What this proves, and what it does not
///
/// The **positive passes** prove the shipped configuration emits ordinary, separately
/// rounded multiplies: no fused mnemonic, no `llvm.fma`/`llvm.fmuladd` intrinsic and no
/// contract-marked float operation, on a target where the instruction exists. That is the
/// property architecture §7.3 needs.
///
/// The **negative control** re-runs one pass with `-fp-contract=fast`, which fuses
/// regardless of what the IR asks for, and requires the scan to find a fused mnemonic. Its
/// success condition is that the audit *fails*: without it, "we found no fusion" would be
/// indistinguishable from "the scan looks in the wrong place".
///
/// What no available control can show is that `-fp-contract=off` is itself load-bearing.
/// Measured on this toolchain, stripping it changes nothing: rustc never emits `contract`
/// fast-math flags, and LLVM's default fusion policy already declines to fuse operations
/// that do not carry them. The flag is defence in depth against a future default, not the
/// thing being proven — C6a corrected C6's task file where it claimed otherwise.
fn job_fma_check() -> bool {
    let mut all_succeeded = true;
    for pass in FMA_PASSES {
        all_succeeded &= run_fma_pass(pass);
    }
    all_succeeded & run_fma_negative_control()
}

fn run_fma_pass(pass: &FmaPass) -> bool {
    let Some(target_directory) = fresh_audit_directory("fma-check", pass.package) else { return false };
    let findings = fma_findings(pass.package, &target_directory, None);
    let succeeded = match findings {
        Err(message) => {
            eprintln!("     {message}");
            false
        }
        Ok(findings) => {
            let mut succeeded = true;
            for violation in &findings.violations {
                eprintln!("     {}: {violation}", pass.package);
                succeeded = false;
            }
            if pass.requires_float_multiply && !(findings.saw_assembly_float_multiply && findings.saw_ir_float_multiply) {
                eprintln!("     {}: audit inconclusive — the optimized build contains no f32 multiply", pass.package);
                succeeded = false;
            }
            if succeeded && findings.saw_assembly_float_multiply {
                println!("     {}: separate multiply, no contraction marker, intrinsic or fused mnemonic", pass.package);
            } else if succeeded {
                println!("     {}: no float codegen of its own; scanned anyway so a future non-generic helper is covered", pass.package);
            }
            succeeded
        }
    };
    succeeded & remove_audit_directory(&target_directory)
}

/// Rebuild one pass with the fusion policy turned all the way on. A fused mnemonic **must**
/// appear; if it does not, every "no fusion found" above proves nothing and the job fails.
fn run_fma_negative_control() -> bool {
    const PACKAGE: &str = "starplayer-mixer";
    let Some(target_directory) = fresh_audit_directory("fma-control", PACKAGE) else { return false };
    println!("     negative control: RUSTFLAGS=\"-C llvm-args=-fp-contract=fast\" (a fused mnemonic is required)");
    let findings = fma_findings(PACKAGE, &target_directory, Some("-C llvm-args=-fp-contract=fast"));
    let succeeded = match findings {
        Err(message) => {
            eprintln!("     {message}");
            false
        }
        Ok(findings) if findings.violations.is_empty() => {
            eprintln!("     negative control found no fused operation with contraction forced on.");
            eprintln!("     The audit above therefore proves nothing: fix the scan, or drop the claim.");
            false
        }
        Ok(findings) => {
            println!("     negative control passed: {} contraction finding(s) with the policy forced on", findings.violations.len());
            true
        }
    };
    succeeded & remove_audit_directory(&target_directory)
}

#[derive(Default)]
struct FmaFindings {
    /// Every contraction the scan saw, already formatted for a report.
    violations: Vec<String>,
    saw_assembly_float_multiply: bool,
    saw_ir_float_multiply: bool,
}

fn fma_findings(package: &str, target_directory: &Path, rustflags: Option<&str>) -> Result<FmaFindings, String> {
    println!("     cargo rustc -p {package} --release --lib -- -C target-feature=+fma --emit=asm,llvm-ir");
    let mut command = Command::new(cargo_binary());
    command
        .current_dir(workspace_root())
        .env("CARGO_TARGET_DIR", target_directory)
        .args(["rustc", "-p", package, "--release", "--lib", "--", "-C", "target-feature=+fma", "--emit=asm,llvm-ir"]);
    if let Some(rustflags) = rustflags {
        command.env("RUSTFLAGS", rustflags);
    }
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(status) => return Err(format!("FMA audit build for `{package}` exited with {status}")),
        Err(error) => return Err(format!("FMA audit build for `{package}` failed to start: {error}")),
    }

    let mut findings = FmaFindings::default();

    let mut assembly_paths = Vec::new();
    collect_files_with_extension(target_directory, "s", &mut assembly_paths);
    for path in &assembly_paths {
        let assembly = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read FMA audit output `{}`: {error}", path.display()))?
            .to_ascii_lowercase();
        findings.saw_assembly_float_multiply |= assembly.contains("mulss") || assembly.contains("mulps");
        for mnemonic in ["vfmadd", "vfmsub", "vfnmadd", "vfnmsub"] {
            if assembly.contains(mnemonic) {
                findings.violations.push(format!("fused mnemonic `{mnemonic}` in `{}`", path.display()));
            }
        }
    }

    let mut llvm_ir_paths = Vec::new();
    collect_files_with_extension(target_directory, "ll", &mut llvm_ir_paths);
    for path in &llvm_ir_paths {
        let llvm_ir = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read FMA audit IR `{}`: {error}", path.display()))?
            .to_ascii_lowercase();
        findings.saw_ir_float_multiply |= llvm_ir.contains("fmul ");
        if llvm_ir.contains("llvm.fma.") || llvm_ir.contains("llvm.fmuladd.") {
            findings.violations.push(format!("fma/fmuladd intrinsic in `{}`", path.display()));
        }
        if llvm_ir.lines().any(|line| line.contains("contract") && (line.contains("fmul") || line.contains("fadd") || line.contains("fsub"))) {
            findings.violations.push(format!("contract-enabled float operation in `{}`", path.display()));
        }
    }
    Ok(findings)
}

/// The `#[cfg(feature = "trace")]` per-channel report loop must leave nothing behind in a
/// release build with the feature off (C1 deliverable 2).
///
/// The scan is only meaningful with its own control, for the same reason the FMA audit
/// needs one: an absent symbol could equally mean the grep is looking at the wrong file.
/// Each package is therefore built twice — once as shipped, once with `--features trace` —
/// and the symbol must be absent from the first and present in the second.
fn job_trace_zero_cost() -> bool {
    let mut all_succeeded = true;
    for package in TRACE_HOOK_PACKAGES {
        all_succeeded &= run_trace_zero_cost_pass(package, false);
        all_succeeded &= run_trace_zero_cost_pass(package, true);
    }
    all_succeeded
}

fn run_trace_zero_cost_pass(package: &str, trace: bool) -> bool {
    let label = if trace { "with-trace" } else { "shipped" };
    let Some(target_directory) = fresh_audit_directory(&format!("trace-{label}"), package) else { return false };
    let feature_arguments: &[&str] = if trace { &["--features", "trace"] } else { &[] };
    let printed_features = if trace { " --features trace" } else { "" };
    println!("     cargo rustc -p {package} --release --lib{printed_features} -- --emit=asm,llvm-ir");
    let status = Command::new(cargo_binary())
        .current_dir(workspace_root())
        .env("CARGO_TARGET_DIR", &target_directory)
        .args(["rustc", "-p", package, "--release", "--lib"])
        .args(feature_arguments)
        .args(["--", "--emit=asm,llvm-ir"])
        .status();
    let built = match status {
        Ok(status) if status.success() => true,
        Ok(status) => {
            eprintln!("     trace audit build for `{package}` ({label}) exited with {status}");
            false
        }
        Err(error) => {
            eprintln!("     trace audit build for `{package}` ({label}) failed to start: {error}");
            false
        }
    };

    let succeeded = built && match count_symbol_occurrences(&target_directory, TRACE_HOOK_SYMBOL) {
        Err(message) => {
            eprintln!("     {message}");
            false
        }
        Ok(occurrences) if trace && occurrences == 0 => {
            eprintln!("     `{package}` with `trace` on contains no `{TRACE_HOOK_SYMBOL}`, so the shipped-build scan proves nothing");
            false
        }
        Ok(occurrences) if !trace && occurrences != 0 => {
            eprintln!("     `{package}` still contains {occurrences} reference(s) to `{TRACE_HOOK_SYMBOL}` with `trace` off");
            false
        }
        Ok(occurrences) => {
            println!("     {package} ({label}): {occurrences} reference(s) to `{TRACE_HOOK_SYMBOL}`");
            true
        }
    };
    succeeded & remove_audit_directory(&target_directory)
}

fn count_symbol_occurrences(target_directory: &Path, symbol: &str) -> Result<usize, String> {
    let mut paths = Vec::new();
    collect_files_with_extension(target_directory, "s", &mut paths);
    collect_files_with_extension(target_directory, "ll", &mut paths);
    let mut occurrences = 0usize;
    for path in &paths {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read audit output `{}`: {error}", path.display()))?;
        occurrences += text.matches(symbol).count();
    }
    Ok(occurrences)
}

/// A PID-scoped, empty target directory for one audit pass. A stale directory would let a
/// scan inspect artifacts from an earlier process whose PID was reused, so failing to
/// clear it fails the pass.
fn fresh_audit_directory(kind: &str, package: &str) -> Option<PathBuf> {
    let directory = std::env::temp_dir().join(format!("starplayer-{kind}-{package}-{}", std::process::id()));
    remove_audit_directory(&directory).then_some(directory)
}

fn remove_audit_directory(target_directory: &Path) -> bool {
    match std::fs::remove_dir_all(target_directory) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => {
            eprintln!("     cannot remove audit directory `{}`: {error}", target_directory.display());
            false
        }
    }
}

fn collect_files_with_extension(directory: &Path, extension: &str, destination: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_with_extension(&path, extension, destination);
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            destination.push(path);
        }
    }
}

/// Corpus acquisition is deliberately separate from the CI test command. The workflow
/// restores or prepares the checksum-pinned cache first; this job proves that the tests
/// themselves have no live third-party dependency.
///
/// This is the informational form: it reports the known-failure exclusions without
/// failing on them. Once C5, C7 and C9 land, add `"--strict".to_string()` to the argument
/// list below and CI enforces the M2 exit criterion.
fn job_conformance() -> bool {
    run_conformance(&["--offline".to_string()])
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
/// it comes to four things:
///
/// 1. **Concatenation.** `AudioWorklet.addModule()` takes one URL, and inside
///    `AudioWorkletGlobalScope` there is no `fetch`, no `importScripts` and no dependable
///    dynamic `import`. So the glue, the ring protocol and the processor have to arrive
///    as a single script. `--target no-modules` is what makes this possible at all: it
///    emits a self-contained IIFE with no `export`, no `import.meta` and no top-level
///    `await`, all of which `--target web` has and none of which survive concatenation
///    into a worklet.
///
/// 2. **One binding closure per processor.** `--target no-modules` normally creates one
///    cached wasm instance for the whole `AudioWorkletGlobalScope`. Output-channel
///    rebuilds briefly keep two nodes alive in that same scope, and each needs its own
///    Rust `HOST`, memory and stable render views. Wrap the generated IIFE in a factory so
///    every processor instantiates independent bindings while still sharing the compiled
///    `WebAssembly.Module`.
///
/// 3. **Disarming the script-path sniff.** The glue opens by reading
///    `document.currentScript` to guess where its `.wasm` sits, so that a bare
///    `wasm_bindgen()` call can fetch it. The guard already short-circuits in a worklet,
///    where `document` is undefined — but the branch it protects is the only place the
///    glue reaches for `location` and `fetch`, neither of which exists there. Forcing it
///    off makes that unreachable by construction rather than by luck, and the assertion
///    below turns a future wasm-bindgen that changes the shape of this code into a build
///    failure instead of a silent runtime one.
///
/// 4. **A `TextDecoder`.** The glue constructs one at the top of its IIFE, unconditionally,
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
    const BINDING_START: &str = "let wasm_bindgen = (function(exports) {";
    const BINDING_END: &str = "})({ __proto__: null });";
    if !glue.contains(BINDING_START) || !glue.contains(BINDING_END) {
        eprintln!("xtask wasm: the wasm-bindgen glue no longer has the no-modules IIFE");
        eprintln!("            this build wraps per AudioWorklet processor. Re-read the");
        eprintln!("            generated `{BINDGEN_GLUE}` and update `build_worklet_bundle`.");
        return false;
    }
    let glue = glue.replacen(BINDING_START, "function createStarPlayerWasmBindings() {\nreturn (function(exports) {", 1)
        .replacen(BINDING_END, "})({ __proto__: null });\n}", 1);

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
