//! Implementation detail behind `cargo xtask conformance`.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use starplayer::engine::Trace;
use starplayer::model::{Module, ModuleFormat};
use starplayer_offline::{TraceOptions, trace_module};
use starplayer_testkit::conformance::{
    ConformanceCase, ConformanceExclusion, ConformanceFormat, ExclusionKind, SampleGeometry, audit_pinned_corpus,
    check_tick_budget, diff_libxmp_dump, oracle_tick_budget, parse_case_manifest, parse_exclusions, parse_libxmp_dump,
};

/// The facade's native processor for one conformance format. A capability registry, not
/// format lowering: a format with no entry here is a visible gate, never an exclusion or a
/// fabricated pass, and an entry exists only when `starplayer::supports` says this build
/// can actually play that format.
///
/// Load and capture are the facade's own [`starplayer::load`] and
/// [`starplayer_offline::trace_module`], so the harness measures the same dispatch every
/// host uses. What the registry still carries is the [`ModuleFormat`] the case *claims*:
/// autodetection must agree with the manifest, or the case is comparing a module against
/// another format's oracle.
#[derive(Copy, Clone)]
struct FormatIntegration {
    expected: ModuleFormat,
}

#[derive(Copy, Clone, Debug, Default)]
struct Counts {
    total: usize,
    passed: usize,
    failed: usize,
    /// Excluded because the reference resolves into the accuracy policy.
    accepted_deviations: usize,
    /// Excluded against a task file or `conformance/known-failures.md`; an M2 exit blocker.
    known_failures: usize,
    /// No native trace capture is registered for the format.
    gated: usize,
}

impl Counts {
    fn add(&mut self, other: Counts) {
        self.total += other.total;
        self.passed += other.passed;
        self.failed += other.failed;
        self.accepted_deviations += other.accepted_deviations;
        self.known_failures += other.known_failures;
        self.gated += other.gated;
    }

    fn pass_rate_percent(self) -> f64 {
        if self.total == 0 { 0.0 } else { self.passed as f64 * 100.0 / self.total as f64 }
    }
}

/// What comparing one case produced.
enum CaseOutcome {
    /// Every enforced field agreed.
    Match,
    /// A real state difference, reported as C1's first-divergence report.
    Divergence(String),
    /// The harness could not perform a valid comparison. Never a case result: it fails
    /// the run so it can never be mistaken for an engine deviation.
    HarnessError(String),
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("conformance: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool, String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let corpus = argument_path(&arguments, "--corpus")?;
    let manifest_path = argument_path(&arguments, "--manifest")?;
    let exclusions_path = argument_path(&arguments, "--exclusions")?;
    let strict = arguments.iter().any(|argument| argument == "--strict");
    reject_unknown_arguments(&arguments)?;

    let manifest_text = read(&manifest_path)?;
    let cases = parse_case_manifest(&manifest_text)?;
    let exclusions_text = read(&exclusions_path)?;
    let exclusions = parse_exclusions(&exclusions_text, &cases)?;
    validate_corpus(&corpus, &cases)?;
    validate_mtm_reference_module(&corpus)?;
    let inventory = audit_pinned_corpus(&corpus, &cases)?;
    println!(
        "corpus inventory: {} MOD/S3M/MTM/IT binaries; {} frame-state cases over {} unique modules; {} seed/documentation binaries without a state oracle",
        inventory.module_binaries, inventory.state_oracle_cases, inventory.state_oracle_modules, inventory.without_state_oracle,
    );
    println!(
        "OpenMPT inventory: {} MOD/S3M/IT modules; {} with frame-state oracles; {} documented-only",
        inventory.openmpt_modules, inventory.openmpt_oracle_modules, inventory.openmpt_documented_only,
    );

    let integrations: BTreeMap<ConformanceFormat, FormatIntegration> = ConformanceFormat::ALL.into_iter()
        .map(|format| (format, module_format(format)))
        .filter(|&(_, expected)| starplayer::supports(expected))
        .map(|(format, expected)| (format, FormatIntegration { expected }))
        .collect();

    let mut totals = Counts::default();
    let mut by_format = BTreeMap::new();
    for format in ConformanceFormat::ALL {
        let mut counts = Counts::default();
        for case in cases.iter().filter(|case| case.format == format) {
            counts.add(run_case(&corpus, case, exclusions.get(&case.id), integrations.get(&format).copied()));
        }
        totals.add(counts);
        by_format.insert(format, counts);
    }

    println!();
    println!("format  total  pass  fail  accepted  known-fail  gated  pass-rate");
    for format in ConformanceFormat::ALL {
        let counts = by_format[&format];
        println!(
            "{format:<6}  {:>5}  {:>4}  {:>4}  {:>8}  {:>10}  {:>5}  {:>8.1}%",
            counts.total, counts.passed, counts.failed, counts.accepted_deviations, counts.known_failures, counts.gated,
            counts.pass_rate_percent(),
        );
    }
    println!(
        "TOTAL   {:>5}  {:>4}  {:>4}  {:>8}  {:>10}  {:>5}  {:>8.1}%",
        totals.total, totals.passed, totals.failed, totals.accepted_deviations, totals.known_failures, totals.gated,
        totals.pass_rate_percent(),
    );
    println!(
        "gated is {} because every M2 format registers a native trace capture; a format without one would be gated, never excluded",
        totals.gated,
    );

    let blocking: Vec<&ConformanceExclusion> = cases.iter()
        .filter_map(|case| exclusions.get(&case.id))
        .filter(|exclusion| exclusion.kind() == ExclusionKind::KnownFailure)
        .collect();
    if strict && !blocking.is_empty() {
        println!();
        println!("strict: {} known-failure exclusion(s) still block the M2 exit:", blocking.len());
        for exclusion in &blocking {
            println!("  {} -> {}", exclusion.case_id, exclusion.reference);
        }
        return Ok(false);
    }
    if !strict && !blocking.is_empty() {
        println!(
            "informational: {} known-failure exclusion(s) remain; `--strict` fails on them while `C2-S3M-009` does",
            blocking.len(),
        );
    }

    Ok(totals.failed == 0)
}

fn argument_path(arguments: &[String], name: &str) -> Result<PathBuf, String> {
    let Some(index) = arguments.iter().position(|argument| argument == name) else {
        return Err(format!("missing required `{name} PATH`"));
    };
    let value = arguments.get(index + 1).ok_or_else(|| format!("`{name}` needs a path"))?;
    if value.starts_with("--") {
        return Err(format!("`{name}` needs a path"));
    }
    Ok(PathBuf::from(value))
}

fn reject_unknown_arguments(arguments: &[String]) -> Result<(), String> {
    let mut index = 0usize;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--corpus" | "--manifest" | "--exclusions" => index += 2,
            "--strict" => index += 1,
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }
    Ok(())
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("could not read `{}`: {error}", path.display()))
}

fn validate_corpus(corpus: &Path, cases: &[ConformanceCase]) -> Result<(), String> {
    if !corpus.is_dir() {
        return Err(format!("corpus directory `{}` does not exist", corpus.display()));
    }
    let mut missing = Vec::new();
    for case in cases {
        for relative in [&case.module, &case.oracle] {
            if !corpus.join(relative).is_file() {
                missing.push(relative.display().to_string());
            }
        }
    }
    missing.sort();
    missing.dedup();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("pinned corpus is incomplete; missing {} file(s), first is `{}`", missing.len(), missing[0]))
    }
}

fn validate_mtm_reference_module(corpus: &Path) -> Result<(), String> {
    let relative = Path::new("data/m/fall1.mtm");
    let path = corpus.join(relative);
    let bytes = std::fs::read(&path).map_err(|error| format!("could not read pinned MTM loader reference `{}`: {error}", path.display()))?;
    let module = starplayer::mtm::load(&bytes).map_err(|error| format!("could not load pinned MTM loader reference `{}`: {error}", path.display()))?;
    let observed = (
        module.header().title.as_ref(),
        module.header().channel_count,
        starplayer::mtm::track_count(&module),
        module.patterns().len(),
    );
    let expected = ("- One Must Fall! 1 -", 5, Some(51), 12);
    if observed != expected {
        return Err(format!(
            "pinned MTM loader reference metadata differs from libxmp format_mtm.data: got title {:?}, {} channels, {:?} stored tracks, {} patterns",
            observed.0, observed.1, observed.2, observed.3,
        ));
    }
    println!("MTM loader reference: fall1.mtm title/channels/stored-tracks/patterns match libxmp");
    Ok(())
}

fn run_case(corpus: &Path, case: &ConformanceCase, exclusion: Option<&ConformanceExclusion>, integration: Option<FormatIntegration>) -> Counts {
    let Some(integration) = integration else {
        if let Some(exclusion) = exclusion {
            println!("{} CONFIG ERROR: a gated case may not be excluded ({})", case.id, exclusion.reason);
            return Counts { total: 1, failed: 1, ..Counts::default() };
        }
        println!("{} GATED (no native {} trace capture is registered) — {}", case.id, case.format, case.behaviour);
        return Counts { total: 1, gated: 1, ..Counts::default() };
    };

    let waived = exclusion.map(|exclusion| exclusion.waived_fields.as_slice()).unwrap_or(&[]);
    match (compare_case(corpus, case, integration, waived), exclusion) {
        (CaseOutcome::HarnessError(message), _) => {
            println!("{} HARNESS ERROR — {}\n{}", case.id, case.behaviour, indent(&message));
            Counts { total: 1, failed: 1, ..Counts::default() }
        }
        (CaseOutcome::Match, None) => {
            println!("{} PASS — {}", case.id, case.behaviour);
            Counts { total: 1, passed: 1, ..Counts::default() }
        }
        (CaseOutcome::Match, Some(exclusion)) if exclusion.is_waiver() => {
            // The waiver is the recorded deviation; the rest of the trace was enforced
            // and agreed, so this is a pass, not a stale exclusion.
            println!("{} PASS waiving {} — {} [{}; {}]", case.id, exclusion.waiver_summary(), case.behaviour, exclusion.reason, exclusion.reference);
            Counts { total: 1, passed: 1, ..Counts::default() }
        }
        (CaseOutcome::Match, Some(exclusion)) => {
            println!("{} STALE EXCLUSION: now passes; remove `{}` ({})", case.id, exclusion.reason, exclusion.reference);
            Counts { total: 1, failed: 1, ..Counts::default() }
        }
        (CaseOutcome::Divergence(message), Some(exclusion)) => {
            let kind = exclusion.kind();
            let waiver = if exclusion.is_waiver() { format!(" (waiving {})", exclusion.waiver_summary()) } else { String::new() };
            println!("{} EXCLUDED [{kind}]{waiver} — {} [{}; {}]", case.id, message, exclusion.reason, exclusion.reference);
            match kind {
                ExclusionKind::AcceptedDeviation => Counts { total: 1, accepted_deviations: 1, ..Counts::default() },
                ExclusionKind::KnownFailure => Counts { total: 1, known_failures: 1, ..Counts::default() },
            }
        }
        (CaseOutcome::Divergence(message), None) => {
            println!("{} FAIL — {}\n{}", case.id, case.behaviour, indent(&message));
            Counts { total: 1, failed: 1, ..Counts::default() }
        }
    }
}

fn compare_case(corpus: &Path, case: &ConformanceCase, integration: FormatIntegration, waived: &[starplayer_testkit::TraceField]) -> CaseOutcome {
    let module_path = corpus.join(&case.module);
    let oracle_path = corpus.join(&case.oracle);
    let bytes = match std::fs::read(&module_path) {
        Ok(bytes) => bytes,
        Err(error) => return CaseOutcome::HarnessError(format!("could not read `{}`: {error}", module_path.display())),
    };
    let oracle_text = match read(&oracle_path) {
        Ok(text) => text,
        Err(message) => return CaseOutcome::HarnessError(message),
    };
    let oracle = match parse_libxmp_dump(&oracle_text) {
        Ok(oracle) => oracle,
        Err(message) => return CaseOutcome::HarnessError(message),
    };

    let module = match load_module(integration.expected, &bytes) {
        Ok(module) => module,
        Err(message) => return CaseOutcome::Divergence(message),
    };
    let geometry = SampleGeometry::from_module(case.format, &module);
    let budget = oracle_tick_budget(&oracle);
    let trace = match capture_trace(&bytes, budget) {
        Ok(trace) => trace,
        Err(message) => return CaseOutcome::Divergence(message),
    };
    if let Err(message) = check_tick_budget(case.format, &oracle, &trace, budget) {
        return CaseOutcome::HarnessError(message);
    }

    let difference = diff_libxmp_dump(case.format, &oracle, &trace, &geometry, waived);
    if difference.is_identical() { CaseOutcome::Match } else { CaseOutcome::Divergence(difference.to_string()) }
}

/// Which loaded-module format a conformance format's cases must decode to.
const fn module_format(format: ConformanceFormat) -> ModuleFormat {
    match format {
        ConformanceFormat::Mod => ModuleFormat::Mod,
        ConformanceFormat::S3m => ModuleFormat::S3m,
        ConformanceFormat::Mtm => ModuleFormat::Mtm,
        ConformanceFormat::It => ModuleFormat::It,
    }
}

/// Load through the facade's autodetection, and insist the result is the format the
/// manifest named.
///
/// A loader rejection has always been the case's own result — the C5 tracker-dialect rows
/// report bad magic here — and so is a module that probes as something else: comparing it
/// against another format's oracle would be meaningless.
fn load_module(expected: ModuleFormat, bytes: &[u8]) -> Result<Module, String> {
    let module = starplayer::load(bytes).map_err(|error| error.to_string())?;
    let actual = module.header().format;
    if actual == expected { Ok(module) } else { Err(format!("autodetected {actual:?}, but the case is a {expected:?} case")) }
}

fn capture_trace(bytes: &[u8], ticks: usize) -> Result<Trace, String> {
    trace_module(bytes, TraceOptions { ticks: Some(ticks), ..TraceOptions::default() }).map_err(|error| error.to_string())
}

fn indent(message: &str) -> String {
    message.lines().map(|line| format!("  {line}\n")).collect()
}
