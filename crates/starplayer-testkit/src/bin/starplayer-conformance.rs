//! Implementation detail behind `cargo xtask conformance`.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use starplayer::engine::Trace;
use starplayer_offline::{TraceOptions, trace_mod, trace_s3m};
use starplayer_testkit::conformance::{
    ConformanceCase, ConformanceExclusion, ConformanceFormat, audit_pinned_corpus, diff_libxmp_dump,
    parse_case_manifest, parse_exclusions, parse_libxmp_dump,
};

type CaptureTrace = fn(&[u8], usize) -> Result<Trace, String>;

#[derive(Copy, Clone, Debug, Default)]
struct Counts {
    total: usize,
    passed: usize,
    failed: usize,
    excluded: usize,
    gated: usize,
}

impl Counts {
    fn add(&mut self, other: Counts) {
        self.total += other.total;
        self.passed += other.passed;
        self.failed += other.failed;
        self.excluded += other.excluded;
        self.gated += other.gated;
    }
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
    reject_unknown_arguments(&arguments)?;

    let manifest_text = read(&manifest_path)?;
    let cases = parse_case_manifest(&manifest_text)?;
    let exclusions_text = read(&exclusions_path)?;
    let exclusions = parse_exclusions(&exclusions_text, &cases)?;
    validate_corpus(&corpus, &cases)?;
    let inventory = audit_pinned_corpus(&corpus, &cases)?;
    println!(
        "corpus inventory: {} MOD/S3M/MTM binaries; {} frame-state cases over {} unique modules; {} seed/documentation binaries without a state oracle",
        inventory.module_binaries, inventory.state_oracle_cases, inventory.state_oracle_modules, inventory.without_state_oracle,
    );
    println!(
        "OpenMPT inventory: {} MOD/S3M modules; {} with frame-state oracles; {} documented-only",
        inventory.openmpt_modules, inventory.openmpt_oracle_modules, inventory.openmpt_documented_only,
    );

    // Capability registry, not format lowering. Each format integration registers its
    // native capture function here; until then a missing capability is a visible gate,
    // never an exclusion or fabricated pass. This is deliberately a function table
    // rather than a trait committed before the second implementation exists.
    let capture_functions: BTreeMap<ConformanceFormat, CaptureTrace> = [
        (ConformanceFormat::Mod, capture_mod as CaptureTrace),
        (ConformanceFormat::S3m, capture_s3m as CaptureTrace),
    ].into_iter().collect();
    let pending_integrations: BTreeMap<ConformanceFormat, &str> = [
        (ConformanceFormat::Mtm, "C4"),
    ].into_iter().collect();

    let mut totals = Counts::default();
    let mut by_format = BTreeMap::new();
    for format in ConformanceFormat::ALL {
        let mut counts = Counts::default();
        for case in cases.iter().filter(|case| case.format == format) {
            let outcome = run_case(
                &corpus,
                case,
                exclusions.get(&case.id),
                capture_functions.get(&format).copied(),
                pending_integrations.get(&format).copied(),
            );
            counts.add(outcome);
        }
        totals.add(counts);
        by_format.insert(format, counts);
    }

    println!();
    println!("format  total  pass  fail  excluded  gated");
    for format in ConformanceFormat::ALL {
        let counts = by_format[&format];
        println!("{format:<6}  {:>5}  {:>4}  {:>4}  {:>8}  {:>5}", counts.total, counts.passed, counts.failed, counts.excluded, counts.gated);
    }
    println!("TOTAL   {:>5}  {:>4}  {:>4}  {:>8}  {:>5}", totals.total, totals.passed, totals.failed, totals.excluded, totals.gated);

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

fn run_case(
    corpus: &Path,
    case: &ConformanceCase,
    exclusion: Option<&ConformanceExclusion>,
    capture: Option<CaptureTrace>,
    pending_integration: Option<&str>,
) -> Counts {
    let Some(capture) = capture else {
        if let Some(exclusion) = exclusion {
            println!("{} CONFIG ERROR: a gated case may not be excluded ({})", case.id, exclusion.reason);
            return Counts { total: 1, failed: 1, ..Counts::default() };
        }
        let task = pending_integration.unwrap_or("unassigned integration");
        println!("{} GATED ({task} must register native {} trace capture) — {}", case.id, case.format, case.behaviour);
        return Counts { total: 1, gated: 1, ..Counts::default() };
    };

    let result = compare_case(corpus, case, capture);
    match (result, exclusion) {
        (Ok(()), None) => {
            println!("{} PASS — {}", case.id, case.behaviour);
            Counts { total: 1, passed: 1, ..Counts::default() }
        }
        (Ok(()), Some(exclusion)) => {
            println!("{} STALE EXCLUSION: now passes; remove `{}` ({})", case.id, exclusion.reason, exclusion.reference);
            Counts { total: 1, failed: 1, ..Counts::default() }
        }
        (Err(message), Some(exclusion)) => {
            println!("{} EXCLUDED — {} [{}; {}]", case.id, message, exclusion.reason, exclusion.reference);
            Counts { total: 1, excluded: 1, ..Counts::default() }
        }
        (Err(message), None) => {
            println!("{} FAIL — {}\n{}", case.id, case.behaviour, indent(&message));
            Counts { total: 1, failed: 1, ..Counts::default() }
        }
    }
}

fn compare_case(corpus: &Path, case: &ConformanceCase, capture: CaptureTrace) -> Result<(), String> {
    let module_path = corpus.join(&case.module);
    let oracle_path = corpus.join(&case.oracle);
    let module = std::fs::read(&module_path).map_err(|error| format!("could not read `{}`: {error}", module_path.display()))?;
    let oracle = read(&oracle_path)?;
    let oracle = parse_libxmp_dump(&oracle)?;
    // libxmp emits no line for a tick with no mapped active voice. A bounded allowance
    // lets projection step across such ticks without tracing a deliberately looping test
    // module to the offline driver's million-tick safety limit.
    let trace = capture(&module, oracle.len().saturating_add(256))?;
    let difference = diff_libxmp_dump(case.format, &oracle, &trace)?;
    if difference.is_identical() { Ok(()) } else { Err(difference.to_string()) }
}

fn capture_s3m(module: &[u8], ticks: usize) -> Result<Trace, String> {
    trace_s3m(module, TraceOptions { ticks: Some(ticks), ..TraceOptions::default() }).map_err(|error| error.to_string())
}

fn capture_mod(module: &[u8], ticks: usize) -> Result<Trace, String> {
    trace_mod(module, TraceOptions { ticks: Some(ticks), ..TraceOptions::default() }).map_err(|error| error.to_string())
}

fn indent(message: &str) -> String {
    message.lines().map(|line| format!("  {line}\n")).collect()
}
