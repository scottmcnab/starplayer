//! libxmp/OpenMPT corpus parsing and projection onto the C1 trace contract.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use starplayer::core::{ExactFixedPoint, Frame, FrameClock, InstrumentId, SampleId};
use starplayer::engine::{TRACE_FORMAT_VERSION, Trace, TraceChannel, TraceTick};
use starplayer::model::Module;

use crate::{TraceDiff, TraceField, TraceTolerances, diff_traces};

/// The rate every C1 trace and every libxmp `test-dev` dump is generated at.
pub const CONFORMANCE_SAMPLE_RATE_HZ: u64 = 44_100;

/// libxmp's comparator permits one millisecond of time error; 45 output frames is that
/// millisecond rounded up at 44.1 kHz.
const FRAME_TOLERANCE: u64 = 45;

/// libxmp's comparator permits one integer source frame of position error.
const POSITION_TOLERANCE: u64 = 1u64 << 32;

/// `STARPLAY/S3MLIB.ASM`'s Scream Tracker 3 frequency numerator, and ProTracker's exact
/// PAL Paula clock. Only used to predict how far a voice advances across one tick when
/// deciding whether libxmp legitimately dropped it (accuracy policy D18).
const ST3_FREQUENCY_NUMERATOR: u64 = 14_317_056;
const PAULA_PAL_CLOCK_HZ: u64 = 3_546_895;

/// The formats covered by milestone M2.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConformanceFormat {
    Mod,
    S3m,
    Mtm,
}

impl ConformanceFormat {
    pub const ALL: [ConformanceFormat; 3] = [ConformanceFormat::Mod, ConformanceFormat::S3m, ConformanceFormat::Mtm];

    fn parse(value: &str) -> Option<ConformanceFormat> {
        match value {
            "mod" => Some(ConformanceFormat::Mod),
            "s3m" => Some(ConformanceFormat::S3m),
            "mtm" => Some(ConformanceFormat::Mtm),
            _ => None,
        }
    }
}

impl fmt::Display for ConformanceFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            ConformanceFormat::Mod => "MOD",
            ConformanceFormat::S3m => "S3M",
            ConformanceFormat::Mtm => "MTM",
        })
    }
}

/// Provenance of one case.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CorpusSource {
    Libxmp,
    OpenMpt,
}

impl CorpusSource {
    fn parse(value: &str) -> Option<CorpusSource> {
        match value {
            "libxmp" => Some(CorpusSource::Libxmp),
            "openmpt" => Some(CorpusSource::OpenMpt),
            _ => None,
        }
    }
}

/// One pinned module/oracle pair from `conformance/cases.tsv`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConformanceCase {
    pub id: String,
    pub source: CorpusSource,
    pub format: ConformanceFormat,
    pub module: PathBuf,
    pub oracle: PathBuf,
    pub behaviour: String,
}

/// The tracking reference that marks an exclusion as a deliberate, documented deviation
/// rather than an outstanding failure.
pub const ACCURACY_POLICY_REFERENCE: &str = "plans/product/03-accuracy-policy.md";

/// What an exclusion row means for the M2 exit criteria.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExclusionKind {
    /// The reference resolves into `plans/product/03-accuracy-policy.md`: a deliberate
    /// difference from a secondary oracle that StarPlayer does not intend to adopt.
    AcceptedDeviation,
    /// Anything else — a task file, or `conformance/known-failures.md`. These block the
    /// M2 exit, and `--strict` refuses a run that still contains one.
    KnownFailure,
}

impl fmt::Display for ExclusionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            ExclusionKind::AcceptedDeviation => "accepted deviation",
            ExclusionKind::KnownFailure => "known failure",
        })
    }
}

/// A known failure or per-field waiver which is still executed on every run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConformanceExclusion {
    pub case_id: String,
    pub reason: String,
    pub reference: String,
    /// Fields the differ ignores for this case alone. Empty for an ordinary exclusion,
    /// which suppresses the whole case; non-empty for a waiver, which suppresses exactly
    /// these fields and enforces every other one.
    pub waived_fields: Vec<TraceField>,
}

impl ConformanceExclusion {
    /// Whether the row waives named fields rather than the whole case.
    pub fn is_waiver(&self) -> bool { !self.waived_fields.is_empty() }

    /// Classify the row by the document its reference resolves into.
    pub fn kind(&self) -> ExclusionKind {
        let path = self.reference.split('#').next().unwrap_or("");
        if path == ACCURACY_POLICY_REFERENCE { ExclusionKind::AcceptedDeviation } else { ExclusionKind::KnownFailure }
    }

    /// The waived fields, spelled the way the differ names them.
    pub fn waiver_summary(&self) -> String {
        self.waived_fields.iter().map(|field| field.name()).collect::<Vec<_>>().join(",")
    }
}

/// Structural fields describe the shape of a comparison rather than one observable value,
/// so waiving them would silently drop whole ticks or channels from the enforcement.
const UNWAIVABLE_FIELDS: [TraceField; 3] = [TraceField::Version, TraceField::TickCount, TraceField::ChannelCount];

/// Scope accounting for the pinned snapshot.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CorpusInventory {
    /// Every MOD/S3M/MTM binary present, including malformed loader/fuzzer seeds.
    pub module_binaries: usize,
    /// Calls to libxmp's frame-state comparator (manifest rows).
    pub state_oracle_cases: usize,
    /// Unique binaries covered by those calls.
    pub state_oracle_modules: usize,
    /// Binaries which are useful seeds but do not ship a frame-state oracle.
    pub without_state_oracle: usize,
    /// All MOD/S3M resources under libxmp's OpenMPT snapshot.
    pub openmpt_modules: usize,
    /// OpenMPT modules with at least one generated frame-state oracle.
    pub openmpt_oracle_modules: usize,
    /// OpenMPT modules whose expectations remain wiki/audio documentation only.
    pub openmpt_documented_only: usize,
}

/// Prove that the checked-in manifest is the complete machine-readable state corpus at
/// the pin, then account for every additional format binary without mislabeling it a
/// pass. The authoritative set is the first two string arguments of every
/// `compare_mixer_data*` call in upstream's `test_*.c` files.
pub fn audit_pinned_corpus(corpus: &Path, cases: &[ConformanceCase]) -> Result<CorpusInventory, String> {
    let manifest_pairs: BTreeSet<(PathBuf, PathBuf)> = cases.iter().map(|case| (case.module.clone(), case.oracle.clone())).collect();
    let upstream_pairs = discover_state_oracle_pairs(corpus)?;
    if manifest_pairs != upstream_pairs {
        let missing: Vec<String> = upstream_pairs.difference(&manifest_pairs)
            .map(|(module, oracle)| format!("{} -> {}", module.display(), oracle.display()))
            .collect();
        let extra: Vec<String> = manifest_pairs.difference(&upstream_pairs)
            .map(|(module, oracle)| format!("{} -> {}", module.display(), oracle.display()))
            .collect();
        return Err(format!(
            "case manifest does not match pinned libxmp compare_mixer_data calls ({} missing, {} extra){}{}",
            missing.len(), extra.len(),
            missing.first().map(|value| format!("; first missing `{value}`")).unwrap_or_default(),
            extra.first().map(|value| format!("; first extra `{value}`")).unwrap_or_default(),
        ));
    }

    let mut binaries = BTreeSet::new();
    collect_format_binaries(corpus, corpus, &mut binaries)?;
    let oracle_modules: BTreeSet<PathBuf> = upstream_pairs.iter().map(|(module, _)| module.clone()).collect();
    let openmpt_modules: BTreeSet<&PathBuf> = binaries.iter().filter(|path| path.starts_with("openmpt/mod") || path.starts_with("openmpt/s3m")).collect();
    let openmpt_oracle_modules = oracle_modules.iter().filter(|path| path.starts_with("openmpt/mod") || path.starts_with("openmpt/s3m")).count();
    Ok(CorpusInventory {
        module_binaries: binaries.len(),
        state_oracle_cases: upstream_pairs.len(),
        state_oracle_modules: oracle_modules.len(),
        without_state_oracle: binaries.len().saturating_sub(oracle_modules.len()),
        openmpt_modules: openmpt_modules.len(),
        openmpt_oracle_modules,
        openmpt_documented_only: openmpt_modules.len().saturating_sub(openmpt_oracle_modules),
    })
}

fn discover_state_oracle_pairs(corpus: &Path) -> Result<BTreeSet<(PathBuf, PathBuf)>, String> {
    let entries = std::fs::read_dir(corpus).map_err(|error| format!("cannot inspect corpus root `{}`: {error}", corpus.display()))?;
    let mut pairs = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot inspect corpus entry: {error}"))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        if !name.starts_with("test_") || path.extension().and_then(|extension| extension.to_str()) != Some("c") {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|error| format!("cannot read upstream test `{}`: {error}", path.display()))?;
        for (module, oracle) in mixer_calls(&text)? {
            if matches_target_extension(&module) && !pairs.insert((PathBuf::from(&module), PathBuf::from(&oracle))) {
                return Err(format!("pinned upstream repeats state-oracle pair `{module}` -> `{oracle}`"));
            }
        }
    }
    Ok(pairs)
}

fn mixer_calls(text: &str) -> Result<Vec<(String, String)>, String> {
    const FUNCTIONS: [&str; 4] = [
        "compare_mixer_data",
        "compare_mixer_data_loops",
        "compare_mixer_data_no_rv",
        "compare_mixer_data_player_mode",
    ];
    let mut calls = Vec::new();
    let mut offset = 0usize;
    while let Some(relative) = text[offset..].find("compare_mixer_data") {
        let start = offset + relative;
        let token_boundary = start == 0 || !text.as_bytes()[start - 1].is_ascii_alphanumeric() && text.as_bytes()[start - 1] != b'_';
        let function = token_boundary.then(|| FUNCTIONS.iter().find(|function| {
            let Some(rest) = text[start..].strip_prefix(**function) else { return false };
            rest.starts_with('(') || rest.starts_with(char::is_whitespace)
        })).flatten();
        if let Some(function) = function {
            let after_name = start + function.len();
            let whitespace = text[after_name..].find(|character: char| !character.is_whitespace()).unwrap_or(text.len() - after_name);
            let open = after_name + whitespace;
            if text.as_bytes().get(open) != Some(&b'(') {
                offset = after_name;
                continue;
            }
            let (module, after_module) = next_c_string(text, open + 1).ok_or_else(|| format!("could not parse first string argument of `{function}`"))?;
            let (oracle, after_oracle) = next_c_string(text, after_module).ok_or_else(|| format!("could not parse second string argument of `{function}`"))?;
            calls.push((module, oracle));
            offset = after_oracle;
        } else {
            offset = start + "compare_mixer_data".len();
        }
    }
    Ok(calls)
}

fn next_c_string(text: &str, start: usize) -> Option<(String, usize)> {
    let quote = start + text.get(start..)?.find('"')?;
    let end = quote + 1 + text.get(quote + 1..)?.find('"')?;
    Some((text.get(quote + 1..end)?.to_string(), end + 1))
}

fn matches_target_extension(path: &str) -> bool {
    Path::new(path).extension().and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "mod" | "s3m" | "mtm"))
}

fn collect_format_binaries(root: &Path, directory: &Path, binaries: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(directory).map_err(|error| format!("cannot inventory `{}`: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot inventory corpus entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_format_binaries(root, &path, binaries)?;
        } else if path.is_file() && matches_target_extension(path.to_string_lossy().as_ref()) {
            let relative = path.strip_prefix(root).map_err(|_| format!("corpus path `{}` escaped its root", path.display()))?;
            binaries.insert(relative.to_path_buf());
        }
    }
    Ok(())
}

/// Parse and validate the checked-in case manifest.
pub fn parse_case_manifest(text: &str) -> Result<Vec<ConformanceCase>, String> {
    let mut cases = Vec::new();
    let mut identifiers = BTreeSet::new();
    let mut pairs = BTreeSet::new();
    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = raw_line.split('\t').collect();
        if fields.len() != 6 {
            return Err(format!("case manifest line {line_number}: expected 6 tab-separated fields, found {}", fields.len()));
        }
        let [id, source, format, module, oracle, behaviour] = fields.as_slice() else { unreachable!() };
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-') {
            return Err(format!("case manifest line {line_number}: invalid id `{id}`"));
        }
        if !identifiers.insert((*id).to_string()) {
            return Err(format!("case manifest line {line_number}: duplicate id `{id}`"));
        }
        let source = CorpusSource::parse(source).ok_or_else(|| format!("case manifest line {line_number}: unknown source `{source}`"))?;
        let format = ConformanceFormat::parse(format).ok_or_else(|| format!("case manifest line {line_number}: unknown format `{format}`"))?;
        let module = safe_relative_path(module, line_number, "module")?;
        let oracle = safe_relative_path(oracle, line_number, "oracle")?;
        let extension = module.extension().and_then(|extension| extension.to_str()).unwrap_or("").to_ascii_lowercase();
        if ConformanceFormat::parse(&extension) != Some(format) {
            return Err(format!("case manifest line {line_number}: format does not match module `{}`", module.display()));
        }
        let expected_source = if module.starts_with("openmpt") { CorpusSource::OpenMpt } else { CorpusSource::Libxmp };
        if source != expected_source {
            return Err(format!("case manifest line {line_number}: source does not match module `{}`", module.display()));
        }
        if !pairs.insert((module.clone(), oracle.clone())) {
            return Err(format!("case manifest line {line_number}: duplicate module/oracle pair `{} -> {}`", module.display(), oracle.display()));
        }
        if behaviour.trim().is_empty() {
            return Err(format!("case manifest line {line_number}: behaviour must not be empty"));
        }
        cases.push(ConformanceCase { id: (*id).to_string(), source, format, module, oracle, behaviour: behaviour.trim().to_string() });
    }
    if cases.is_empty() {
        return Err("case manifest contains no cases".to_string());
    }
    Ok(cases)
}

fn safe_relative_path(value: &str, line_number: usize, field: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if value.is_empty() || path.is_absolute() || path.components().any(|component| !matches!(component, Component::Normal(_))) {
        return Err(format!("case manifest line {line_number}: {field} path `{value}` is not a safe relative path"));
    }
    Ok(path.to_path_buf())
}

/// Parse exclusions and require every row to have a reason and tracking reference.
///
/// A row is `case-id`, `reason`, `reference` and an optional fourth `waive=field[,field]`
/// column. The fourth column turns the row from "suppress this case" into "suppress
/// exactly these fields for this case", so the effect a fixture exists to test is still
/// compared. An empty fourth column is an ordinary exclusion.
pub fn parse_exclusions(text: &str, cases: &[ConformanceCase]) -> Result<BTreeMap<String, ConformanceExclusion>, String> {
    let known: BTreeSet<&str> = cases.iter().map(|case| case.id.as_str()).collect();
    let mut exclusions = BTreeMap::new();
    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = raw_line.split('\t').collect();
        if !(3..=4).contains(&fields.len()) {
            return Err(format!("exclusions line {line_number}: expected case id, reason, reference, and an optional `waive=` column"));
        }
        let case_id = fields[0].trim();
        let reason = fields[1].trim();
        let reference = fields[2].trim();
        if !known.contains(case_id) {
            return Err(format!("exclusions line {line_number}: unknown case `{case_id}`"));
        }
        if reason.is_empty() {
            return Err(format!("exclusions line {line_number}: `{case_id}` has no reason"));
        }
        if reference.is_empty() {
            return Err(format!("exclusions line {line_number}: `{case_id}` has no accuracy-policy or tracking reference"));
        }
        let waived_fields = parse_waiver(fields.get(3).map(|value| value.trim()).unwrap_or(""), case_id, line_number)?;
        let exclusion = ConformanceExclusion {
            case_id: case_id.to_string(),
            reason: reason.to_string(),
            reference: reference.to_string(),
            waived_fields,
        };
        if exclusions.insert(case_id.to_string(), exclusion).is_some() {
            return Err(format!("exclusions line {line_number}: duplicate case `{case_id}`"));
        }
    }
    Ok(exclusions)
}

fn parse_waiver(column: &str, case_id: &str, line_number: usize) -> Result<Vec<TraceField>, String> {
    if column.is_empty() {
        return Ok(Vec::new());
    }
    let list = column.strip_prefix("waive=")
        .ok_or_else(|| format!("exclusions line {line_number}: `{case_id}` fourth column must be empty or `waive=field[,field]`, found `{column}`"))?;
    let mut fields = Vec::new();
    for name in list.split(',').map(str::trim) {
        if name.is_empty() {
            return Err(format!("exclusions line {line_number}: `{case_id}` names an empty waiver field"));
        }
        let field = TraceField::from_name(name)
            .ok_or_else(|| format!("exclusions line {line_number}: `{case_id}` waives unknown field `{name}`"))?;
        if UNWAIVABLE_FIELDS.contains(&field) {
            return Err(format!("exclusions line {line_number}: `{case_id}` may not waive the structural field `{name}`"));
        }
        if fields.contains(&field) {
            return Err(format!("exclusions line {line_number}: `{case_id}` waives `{name}` twice"));
        }
        fields.push(field);
    }
    Ok(fields)
}

/// One active libxmp channel record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibxmpChannel {
    pub channel: u16,
    pub period_q12: u32,
    pub note: u8,
    pub instrument_zero_based: u16,
    pub volume_x16: u16,
    pub pan_signed: i16,
    pub position: u32,
    pub cutoff: Option<u16>,
    pub resonance: Option<u16>,
}

/// Consecutive channel records sharing libxmp's time/row/frame tuple.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibxmpTick {
    pub time_ms: u64,
    pub row: u16,
    pub frame: u16,
    pub channels: Vec<LibxmpChannel>,
}

/// Parse `test-dev/gen_mixer_data` output (10, 11, or 12 integer columns).
pub fn parse_libxmp_dump(text: &str) -> Result<Vec<LibxmpTick>, String> {
    let mut ticks: Vec<LibxmpTick> = Vec::new();
    for (line_index, raw_line) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if !(10..=12).contains(&fields.len()) {
            return Err(format!("libxmp dump line {line_number}: expected 10 to 12 integer fields, found {}", fields.len()));
        }
        let number = |index: usize, name: &str| -> Result<i64, String> {
            fields[index].parse::<i64>().map_err(|_| format!("libxmp dump line {line_number}: invalid {name} `{}`", fields[index]))
        };
        let time_ms = unsigned::<u64>(number(0, "time")?, line_number, "time")?;
        let row = unsigned::<u16>(number(1, "row")?, line_number, "row")?;
        let frame = unsigned::<u16>(number(2, "frame")?, line_number, "frame")?;
        let channel = LibxmpChannel {
            channel: unsigned(number(3, "channel")?, line_number, "channel")?,
            period_q12: unsigned(number(4, "period")?, line_number, "period")?,
            note: unsigned(number(5, "note")?, line_number, "note")?,
            instrument_zero_based: unsigned(number(6, "instrument")?, line_number, "instrument")?,
            volume_x16: unsigned(number(7, "volume")?, line_number, "volume")?,
            pan_signed: signed_i16(number(8, "pan")?, line_number, "pan")?,
            position: unsigned(number(9, "position")?, line_number, "position")?,
            cutoff: if fields.len() >= 11 { Some(unsigned(number(10, "cutoff")?, line_number, "cutoff")?) } else { None },
            resonance: if fields.len() >= 12 { Some(unsigned(number(11, "resonance")?, line_number, "resonance")?) } else { None },
        };
        if !(-128..=127).contains(&channel.pan_signed) {
            return Err(format!("libxmp dump line {line_number}: pan {} is outside -128..127", channel.pan_signed));
        }

        let key_matches = ticks.last().is_some_and(|tick| tick.time_ms == time_ms && tick.row == row && tick.frame == frame);
        if !key_matches {
            ticks.push(LibxmpTick { time_ms, row, frame, channels: Vec::new() });
        }
        let tick = ticks.last_mut().expect("the tick was just inserted");
        if tick.channels.iter().any(|existing| existing.channel == channel.channel) {
            return Err(format!("libxmp dump line {line_number}: duplicate channel {} in one frame", channel.channel));
        }
        tick.channels.push(channel);
    }
    if ticks.is_empty() {
        return Err("libxmp dump contains no channel records".to_string());
    }
    Ok(ticks)
}

fn unsigned<T>(value: i64, line_number: usize, name: &str) -> Result<T, String>
where
    T: TryFrom<i64>,
{
    T::try_from(value).map_err(|_| format!("libxmp dump line {line_number}: {name} `{value}` is out of range"))
}

fn signed_i16(value: i64, line_number: usize, name: &str) -> Result<i16, String> {
    i16::try_from(value).map_err(|_| format!("libxmp dump line {line_number}: {name} `{value}` is out of range"))
}

/// The sample geometry the adapter needs to reason about loop wrap and one-shot ends.
///
/// Indexed by the C1 trace's one-based `smp` number so no field has to be added to the
/// committed trace format for a comparison-only concern (C2a research point 1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SampleGeometry {
    spans: Vec<Option<SampleSpan>>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct SampleSpan {
    length_frames: u32,
    loop_start: u32,
    loop_end: u32,
    looping: bool,
}

impl SampleGeometry {
    /// Read the loaded module's sample table into the trace's numbering.
    ///
    /// MOD and MTM trace the ProTracker instrument number and their loaders add exactly
    /// one instrument per sample, so the instrument table resolves it. The S3M processor
    /// already resolves its instrument slot down to a sample id before tracing it.
    pub fn from_module(format: ConformanceFormat, module: &Module) -> SampleGeometry {
        let count = match format {
            ConformanceFormat::Mod | ConformanceFormat::Mtm => module.instruments().len(),
            ConformanceFormat::S3m => module.samples().len(),
        };
        let spans = (0..=count).map(|number| {
            let index = u16::try_from(number.checked_sub(1)?).ok()?;
            let sample = match format {
                ConformanceFormat::Mod | ConformanceFormat::Mtm => {
                    module.instrument(InstrumentId(index)).and_then(|instrument| instrument.sample).and_then(|id| module.sample(id))?
                }
                ConformanceFormat::S3m => module.sample(SampleId(index))?,
            };
            Some(SampleSpan {
                length_frames: sample.length_frames(),
                loop_start: sample.loop_start(),
                loop_end: sample.loop_end(),
                looping: sample.loop_mode().is_looping(),
            })
        }).collect();
        SampleGeometry { spans }
    }

    fn span(&self, sample_number: u16) -> Option<SampleSpan> {
        self.spans.get(sample_number as usize).copied().flatten()
    }
}

/// Ticks to capture so a trace certainly spans the oracle's whole timeline.
///
/// libxmp writes no line for a tick with no mapped active voice, so the record count is a
/// lower bound on the tick count and never an estimate of it. The timeline is the honest
/// bound: frames needed to reach the last record, divided by the shortest tracker tick
/// any legal tempo can produce (255 BPM), plus a margin.
pub fn oracle_tick_budget(upstream: &[LibxmpTick]) -> usize {
    const FASTEST_LEGAL_BPM: u64 = 255;
    const BUDGET_MARGIN_TICKS: usize = 256;
    let shortest_tick_frames = (CONFORMANCE_SAMPLE_RATE_HZ * 5) / (2 * FASTEST_LEGAL_BPM);
    let frames = oracle_end_frame(upstream).0.saturating_add(FRAME_TOLERANCE);
    (frames / shortest_tick_frames.max(1)) as usize + BUDGET_MARGIN_TICKS
}

/// The output frame libxmp's last record is timestamped at.
pub fn oracle_end_frame(upstream: &[LibxmpTick]) -> Frame {
    upstream.iter().map(|tick| expected_frame(tick.time_ms)).max().unwrap_or(Frame::ZERO)
}

/// Detect a capture that stopped because it ran out of tick budget rather than because
/// the module ended.
///
/// This is a harness defect, never a case result: reporting it as a divergence is exactly
/// how `libxmp-s3m-pattern-loop-mpt-breakjump` came to be filed as an engine bug.
pub fn check_tick_budget(upstream: &[LibxmpTick], actual: &Trace, budget: usize) -> Result<(), String> {
    let reached = tick_end_frames(actual).last().copied().unwrap_or(Frame::ZERO);
    let needed = oracle_end_frame(upstream);
    if actual.ticks.len() >= budget && reached.0 + FRAME_TOLERANCE < needed.0 {
        return Err(format!(
            "harness tick budget of {budget} tick(s) reached output frame {} but the oracle runs to frame {}; raise the budget in `oracle_tick_budget`",
            reached.0, needed.0,
        ));
    }
    Ok(())
}

/// Project libxmp's partial state records and StarPlayer's full C1 trace onto the same
/// shape, then invoke the C1 differ.
///
/// `waived` names fields the differ must ignore for this case alone; every other field is
/// enforced. Pairing problems are ordinary [`TraceDiff`] entries — an unpairable record
/// surfaces as a `frame` or `row` divergence naming the tick — so the harness always
/// delivers C1's promised first-divergence report.
pub fn diff_libxmp_dump(
    format: ConformanceFormat,
    upstream: &[LibxmpTick],
    actual: &Trace,
    geometry: &SampleGeometry,
    waived: &[TraceField],
) -> TraceDiff {
    let (expected, actual) = project_libxmp_dump(format, upstream, actual, geometry, waived);
    // libxmp's own mixer-data comparator permits one integer source frame and one
    // millisecond of time. MOD and MTM first project C1's fraction away below, then
    // apply that same whole-frame bound; S3M compares the unfloored Q32.32 value.
    let tolerances = TraceTolerances {
        frame: FRAME_TOLERANCE,
        period: 1,
        position: POSITION_TOLERANCE,
        ..TraceTolerances::default()
    };
    diff_traces(&expected, &actual, &tolerances)
}

fn expected_frame(time_ms: u64) -> Frame {
    Frame(time_ms.saturating_mul(CONFORMANCE_SAMPLE_RATE_HZ).saturating_add(500) / 1_000)
}

/// Pair each libxmp record with the StarPlayer tick whose **end** frame carries the same
/// timestamp.
///
/// libxmp records carry `time_ms` but no order index, so pairing on `(row, frame)` alone
/// mis-associates a jump destination's row 0 with the starting order's row 0. Time is the
/// only unambiguous key; `row` and `tick_in_row` then become ordinary compared fields.
///
/// A case that waives `frame` has declared its two timelines incomparable — D15's CIA
/// latch is exactly that — so absolute time cannot be the key either. Such a case anchors
/// on the first tick carrying the first record's `(row, frame)` and then tracks the
/// residual offset from one paired tick to the next, which keeps the shape of the trace
/// enforced while the timing itself is the waived field.
fn pair_by_time(upstream: &[LibxmpTick], actual: &Trace, end_frames: &[Frame], timeline_waived: bool) -> Vec<Option<usize>> {
    let mut search = 0usize;
    let mut offset = 0i64;
    if timeline_waived && let Some(first) = upstream.first() {
        let anchor = actual.ticks.iter()
            .position(|tick| tick.position.row == first.row && tick.tick_in_row == first.frame)
            .unwrap_or(0);
        offset = i64::try_from(end_frames.get(anchor).copied().unwrap_or(Frame::ZERO).0).unwrap_or(i64::MAX)
            - i64::try_from(expected_frame(first.time_ms).0).unwrap_or(i64::MAX);
        search = anchor;
    }
    // The first tick after the last successful pairing: where a waived timeline
    // re-anchors when the residual offset jumps.
    let mut next_candidate = search;
    upstream.iter().map(|upstream_tick| {
        let target = expected_frame(upstream_tick.time_ms).0.saturating_add_signed(offset);
        while search < end_frames.len() && end_frames[search].0 + FRAME_TOLERANCE < target {
            search += 1;
        }
        match end_frames.get(search) {
            Some(frame) if frame.0.abs_diff(target) <= FRAME_TOLERANCE => {
                let paired = search;
                search += 1;
                next_candidate = search;
                if timeline_waived {
                    offset = i64::try_from(frame.0).unwrap_or(i64::MAX)
                        - i64::try_from(expected_frame(upstream_tick.time_ms).0).unwrap_or(i64::MAX);
                }
                Some(paired)
            }
            _ if timeline_waived => {
                // A waived timeline's residual offset is only re-derived on a successful
                // pairing, so a tempo command that moves it by more than the tolerance in
                // one step — `DelayBreak`-style `Fxx` rows alternating 255 and 63 BPM —
                // strands the records until the offset is re-learned. Re-anchor on the
                // record's own `(row, frame)`, searching forward from the last pairing so
                // a repeated row inside a loop cannot pull the cursor backwards.
                let re_anchor = actual.ticks.iter().enumerate().skip(next_candidate)
                    .find(|(_, tick)| tick.position.row == upstream_tick.row && tick.tick_in_row == upstream_tick.frame)
                    .map(|(index, _)| index);
                match re_anchor {
                    Some(index) => {
                        search = index + 1;
                        next_candidate = search;
                        offset = i64::try_from(end_frames[index].0).unwrap_or(i64::MAX)
                            - i64::try_from(expected_frame(upstream_tick.time_ms).0).unwrap_or(i64::MAX);
                        Some(index)
                    }
                    None => {
                        search = next_candidate;
                        None
                    }
                }
            }
            _ => None,
        }
    }).collect()
}

fn project_libxmp_dump(
    format: ConformanceFormat,
    upstream: &[LibxmpTick],
    actual: &Trace,
    geometry: &SampleGeometry,
    waived: &[TraceField],
) -> (Trace, Trace) {
    let mut expected_ticks = Vec::with_capacity(upstream.len());
    let mut actual_ticks = Vec::with_capacity(upstream.len());
    let actual_end_frames = tick_end_frames(actual);
    let pairing = pair_by_time(upstream, actual, &actual_end_frames, waived.contains(&TraceField::Frame));
    let mut fallback_index = 0usize;

    for (upstream_tick, paired) in upstream.iter().zip(pairing) {
        // An unpairable record still has to produce a first-divergence report, so it is
        // compared against the nearest surviving tick and diverges on `frame`.
        let actual_index = paired.unwrap_or_else(|| fallback_index.min(actual.ticks.len().saturating_sub(1)));
        let Some(actual_tick) = actual.ticks.get(actual_index) else { continue };
        if paired.is_some() {
            fallback_index = actual_index + 1;
        }
        let tick_frames = actual_end_frames[actual_index].0.saturating_sub(actual_tick.frame.0);

        let mut expected_tick = header_projection(actual_tick);
        expected_tick.frame = expected_frame(upstream_tick.time_ms);
        expected_tick.position.row = upstream_tick.row;
        expected_tick.tick_in_row = upstream_tick.frame;
        let mut actual_tick_projection = header_projection(actual_tick);
        actual_tick_projection.frame = actual_end_frames[actual_index];

        let upstream_by_channel: BTreeMap<u16, &LibxmpChannel> = upstream_tick.channels.iter().map(|channel| (channel.channel, channel)).collect();
        let channel_ids: BTreeSet<u16> = upstream_by_channel.keys().copied()
            .chain(actual_tick.channels.iter().filter(|channel| channel.active).map(|channel| channel.channel))
            .collect();
        for channel_id in channel_ids {
            let raw_actual = actual_tick.channels.iter().find(|channel| channel.channel == channel_id).cloned()
                .unwrap_or_else(|| TraceChannel { channel: channel_id, ..TraceChannel::default() });
            let mut actual_channel = raw_actual.clone();
            // C1 names the sample reference-rate pitch C-4, one octave above the
            // ProTracker display octave used as the MOD comparison axis. libxmp's
            // mixer note is a further octave above C1, so expected and actual remove
            // two octaves and one octave respectively.
            if format == ConformanceFormat::Mod {
                actual_channel.note = actual_channel.note.and_then(|note| shift_note(note, 12));
            }
            actual_channel.position = project_actual_position(format, actual_channel.position);
            let mut expected_channel = actual_channel.clone();
            if let Some(upstream_channel) = upstream_by_channel.get(&channel_id) {
                expected_channel.active = true;
                expected_channel.note = project_note(format, upstream_channel.note);
                expected_channel.instrument = upstream_channel.instrument_zero_based.saturating_add(1);
                expected_channel.volume = (upstream_channel.volume_x16 + 8) / 16;
                expected_channel.period = project_period(format, upstream_channel.period_q12);
                expected_channel.pan = project_pan(format, upstream_channel.pan_signed);
                expected_channel.position = (upstream_channel.position as u64) << 32;
                if loop_equivalent(geometry.span(actual_channel.sample), expected_channel.position, actual_channel.position) {
                    // libxmp's own comparator accepts start/end equivalence at a loop
                    // boundary (`test-dev/compare_mixer_data.c:78-82`); a wrapped voice
                    // that is one frame apart circularly is at the same place, however
                    // far apart the two linear values look.
                    expected_channel.position = actual_channel.position;
                }
                if let Some(cutoff) = upstream_channel.cutoff {
                    // libxmp uses zero for its disabled-filter sentinel and treats all
                    // values at or above 254 as equivalent fully-open cutoffs. C1 uses
                    // 255 for that state. Operative values remain exact.
                    expected_channel.cutoff = if cutoff == 0 || cutoff >= 254 { 255 } else { cutoff };
                }
                if let Some(resonance) = upstream_channel.resonance {
                    expected_channel.resonance = resonance;
                }
            } else if !one_shot_ends_within_interval(format, geometry, &raw_actual, tick_frames) {
                // An active StarPlayer channel omitted by libxmp is an observable active
                // set mismatch, not a channel that projection may discard — unless it is
                // the D18 boundary: libxmp drops a `NOTE_SAMPLE_END` voice after mixing
                // the interval, where C1 snapshots the channel before it, so a one-shot
                // that runs out inside this interval is legitimately live on our side.
                // See `plans/product/03-accuracy-policy.md` entry D18.
                expected_channel.active = false;
            }
            // These two C1 fields have no libxmp dump column.
            expected_channel.sample = actual_channel.sample;
            expected_channel.flags = actual_channel.flags;
            expected_tick.channels.push(expected_channel);
            actual_tick_projection.channels.push(actual_channel);
        }
        for field in waived {
            waive_field(*field, &mut expected_tick, &actual_tick_projection);
        }
        expected_ticks.push(expected_tick);
        actual_ticks.push(actual_tick_projection);
    }

    (Trace { version: TRACE_FORMAT_VERSION, ticks: expected_ticks }, Trace { version: actual.version, ticks: actual_ticks })
}

/// Copy one field from the projected actual tick over the expected tick, so the differ
/// cannot see a difference in it. Every other field stays enforced.
fn waive_field(field: TraceField, expected: &mut TraceTick, actual: &TraceTick) {
    match field {
        TraceField::Tick => expected.tick = actual.tick,
        TraceField::Frame => expected.frame = actual.frame,
        TraceField::Order => expected.position.order = actual.position.order,
        TraceField::Pattern => expected.position.pattern = actual.position.pattern,
        TraceField::Row => expected.position.row = actual.position.row,
        TraceField::TickInRow => expected.tick_in_row = actual.tick_in_row,
        TraceField::Speed => expected.speed = actual.speed,
        TraceField::Bpm => expected.bpm = actual.bpm,
        TraceField::GlobalVolume => expected.global_volume = actual.global_volume,
        // `parse_waiver` rejects the structural fields, so nothing else is reachable
        // here; channel fields fall through to the per-record loop below.
        TraceField::Version | TraceField::TickCount | TraceField::ChannelCount => {}
        channel_field => {
            for (expected_channel, actual_channel) in expected.channels.iter_mut().zip(&actual.channels) {
                waive_channel_field(channel_field, expected_channel, actual_channel);
            }
        }
    }
}

fn waive_channel_field(field: TraceField, expected: &mut TraceChannel, actual: &TraceChannel) {
    match field {
        TraceField::Channel => expected.channel = actual.channel,
        TraceField::Active => expected.active = actual.active,
        TraceField::Note => expected.note = actual.note,
        TraceField::Instrument => expected.instrument = actual.instrument,
        TraceField::Sample => expected.sample = actual.sample,
        TraceField::Volume => expected.volume = actual.volume,
        TraceField::Period => expected.period = actual.period,
        TraceField::Pan => expected.pan = actual.pan,
        TraceField::Position => expected.position = actual.position,
        TraceField::Cutoff => expected.cutoff = actual.cutoff,
        TraceField::Resonance => expected.resonance = actual.resonance,
        TraceField::Flags => expected.flags = actual.flags,
        _ => {}
    }
}

/// Whether a one-shot voice runs out inside the interval this tick begins — the D18
/// boundary that makes libxmp omit a record StarPlayer still reports as active.
fn one_shot_ends_within_interval(format: ConformanceFormat, geometry: &SampleGeometry, actual: &TraceChannel, tick_frames: u64) -> bool {
    if !actual.active {
        return false;
    }
    let Some(span) = geometry.span(actual.sample) else { return false };
    if span.looping {
        return false;
    }
    let step = step_per_output_frame(format, actual.period);
    let advance = step.saturating_mul(tick_frames as u128);
    (actual.position as u128).saturating_add(advance) >= (span.length_frames as u128) << 32
}

/// Source frames advanced per output frame, in Q32.32. Only used to predict a one-shot's
/// end; the mixer owns the authoritative step.
fn step_per_output_frame(format: ConformanceFormat, period: u32) -> u128 {
    if period == 0 {
        return 0;
    }
    match format {
        // MOD and MTM run Paula's exact PAL clock divided by the Amiga period.
        ConformanceFormat::Mod | ConformanceFormat::Mtm => {
            ((PAULA_PAL_CLOCK_HZ as u128) << 32) / (period as u128 * CONFORMANCE_SAMPLE_RATE_HZ as u128)
        }
        // ST3 truncates its playback frequency to whole hertz before dividing.
        ConformanceFormat::S3m => {
            (((ST3_FREQUENCY_NUMERATOR / period as u64) as u128) << 32) / CONFORMANCE_SAMPLE_RATE_HZ as u128
        }
    }
}

/// Circular position equivalence inside a forward loop.
///
/// The loop span is read from the loaded `Module` through [`SampleGeometry`] rather than
/// added to the C1 trace format, which would have invalidated every committed expectation
/// for a comparison-only concern.
fn loop_equivalent(span: Option<SampleSpan>, expected: u64, actual: u64) -> bool {
    let Some(span) = span else { return false };
    if !span.looping || span.loop_end <= span.loop_start {
        return false;
    }
    let start = (span.loop_start as u64) << 32;
    let end = (span.loop_end as u64) << 32;
    if expected < start || actual < start || expected > end || actual > end {
        return false;
    }
    let length = end - start;
    let forward = (expected - start) % length;
    let backward = (actual - start) % length;
    let distance = forward.abs_diff(backward);
    distance.min(length - distance) <= POSITION_TOLERANCE
}

fn project_note(format: ConformanceFormat, note: u8) -> Option<u8> {
    // The MOD comparison axis is ProTracker's displayed octave: libxmp's mixer is
    // two octaves above it, while C1 (projected above) is one. S3M/MTM compare on
    // C1's axis and therefore remove libxmp's one-octave bias. A note below the
    // projection offset has no image on the comparison axis; carry that explicitly
    // rather than clamping it to zero, where it would match every other such note.
    shift_note(note, match format {
        ConformanceFormat::Mod => 24,
        ConformanceFormat::S3m | ConformanceFormat::Mtm => 12,
    })
}

fn shift_note(note: u8, semitones_down: i16) -> Option<u8> {
    u8::try_from(note as i16 - semitones_down).ok()
}

fn project_period(format: ConformanceFormat, period_q12: u32) -> u32 {
    // MOD and MTM trace the Amiga-period command domain itself. S3M retains C2's
    // native quarter-period scale, hence its additional factor of four.
    let divisor = match format {
        ConformanceFormat::Mod | ConformanceFormat::Mtm => 4096,
        ConformanceFormat::S3m => 1024,
    };
    period_q12.saturating_add(divisor / 2) / divisor
}

fn project_actual_position(format: ConformanceFormat, position: u64) -> u64 {
    match format {
        // libxmp's `pos0` deliberately discards the mixer's fraction. Compare formats
        // using the MOD-period mixer domain in that same observable integer domain.
        ConformanceFormat::Mod | ConformanceFormat::Mtm => position & !(u32::MAX as u64),
        ConformanceFormat::S3m => position,
    }
}

fn project_pan(format: ConformanceFormat, pan_signed: i16) -> u16 {
    let pan_u8 = (pan_signed as i32 + 128).clamp(0, 255) as u16;
    match format {
        // MOD 8xx is a full-byte pan. Keeping every bit here prevents the adapter
        // from hiding low-nibble differences supplied by the oracle.
        ConformanceFormat::Mod => pan_u8,
        // libxmp shifts these formats' native four-bit pan into the high nibble.
        ConformanceFormat::S3m | ConformanceFormat::Mtm => (pan_u8 >> 4) * 17,
    }
}

fn tick_end_frames(trace: &Trace) -> Vec<Frame> {
    let mut clock = FrameClock::new(ExactFixedPoint, 44_100, Frame::ZERO);
    trace.ticks.iter().map(|tick| clock.advance_tick(tick.bpm, tick.speed)).collect()
}

fn header_projection(actual: &TraceTick) -> TraceTick {
    TraceTick {
        tick: actual.tick,
        frame: actual.frame,
        position: actual.position,
        tick_in_row: actual.tick_in_row,
        speed: actual.speed,
        bpm: actual.bpm,
        global_volume: actual.global_volume,
        channels: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use starplayer::core::{DirtyBits, Frame};
    use starplayer::engine::SongPosition;

    use super::*;

    fn cases() -> Vec<ConformanceCase> {
        parse_case_manifest("case-one\tlibxmp\ts3m\tdata/a.s3m\tdata/a.data\tparameter memory\n").expect("valid manifest")
    }

    #[test]
    fn exclusion_needs_a_reason_and_reference() {
        let cases = cases();
        assert!(parse_exclusions("case-one\t\tplans/engine/M2.md\n", &cases).unwrap_err().contains("has no reason"));
        assert!(parse_exclusions("case-one\tknown failure\t\n", &cases).unwrap_err().contains("has no accuracy-policy"));
        assert!(parse_exclusions("unknown\treason\tissue-1\n", &cases).unwrap_err().contains("unknown case"));
        assert_eq!(parse_exclusions("case-one\treason\tissue-1\n", &cases).expect("valid").len(), 1);
    }

    fn geometry(spans: &[Option<SampleSpan>]) -> SampleGeometry {
        SampleGeometry { spans: core::iter::once(None).chain(spans.iter().copied()).collect() }
    }

    fn one_tick_trace(channel: TraceChannel) -> Trace {
        Trace {
            version: TRACE_FORMAT_VERSION,
            ticks: vec![TraceTick {
                tick: 0,
                frame: Frame::ZERO,
                position: SongPosition { order: 0, pattern: 0, row: 0 },
                tick_in_row: 0,
                speed: 6,
                bpm: 125,
                global_volume: 64,
                channels: vec![channel],
            }],
        }
    }

    #[test]
    fn a_waiver_names_real_fields_and_only_real_fields() {
        let cases = cases();
        let waived = parse_exclusions("case-one\treason\tissue-1\twaive=frame,position\n", &cases).expect("valid waiver");
        assert_eq!(waived["case-one"].waived_fields, vec![TraceField::Frame, TraceField::Position]);
        assert!(waived["case-one"].is_waiver());
        assert_eq!(waived["case-one"].waiver_summary(), "frame,position");

        assert!(parse_exclusions("case-one\treason\tissue-1\twaive=frames\n", &cases).unwrap_err().contains("waives unknown field `frames`"));
        assert!(parse_exclusions("case-one\treason\tissue-1\twaive=tick-count\n", &cases).unwrap_err().contains("structural field"));
        assert!(parse_exclusions("case-one\treason\tissue-1\tframe\n", &cases).unwrap_err().contains("must be empty or `waive="));
        assert!(parse_exclusions("case-one\treason\tissue-1\twaive=frame,frame\n", &cases).unwrap_err().contains("twice"));
        assert!(!parse_exclusions("case-one\treason\tissue-1\t\n", &cases).expect("empty waiver column")["case-one"].is_waiver());
    }

    #[test]
    fn an_exclusion_is_classified_by_the_document_it_points_at() {
        let cases = cases();
        let rows = concat!(
            "case-one\tdeliberate\tplans/product/03-accuracy-policy.md#3-documented-deviations\n",
            "case-two\toutstanding\tplans/engine/M2-task-C9-s3m-conformance-repairs.md\n",
        );
        let cases = [cases, parse_case_manifest("case-two\tlibxmp\ts3m\tdata/b.s3m\tdata/b.data\tsecond\n").expect("valid")].concat();
        let exclusions = parse_exclusions(rows, &cases).expect("valid rows");
        assert_eq!(exclusions["case-one"].kind(), ExclusionKind::AcceptedDeviation);
        assert_eq!(exclusions["case-two"].kind(), ExclusionKind::KnownFailure);
    }

    #[test]
    fn alignment_is_by_time_so_a_jump_destination_reports_its_row_as_a_field() {
        // Two libxmp records one tick apart. The second names row 0 of a jump
        // destination; StarPlayer is on row 1. Pairing on `(row, frame)` would walk past
        // the tick and produce a string error, which is what C2a removed.
        let dump = parse_libxmp_dump(concat!(
            "20 0 0 0 1753088 60 0 1024 0 0\n",
            "40 0 0 0 1753088 60 0 1024 0 0\n",
        )).expect("valid dump");
        let channel = TraceChannel { channel: 0, active: true, note: Some(48), instrument: 1, sample: 1, volume: 64, period: 1712, pan: 136, position: 0, cutoff: 255, resonance: 0, flags: DirtyBits::empty() };
        let mut trace = one_tick_trace(channel.clone());
        trace.ticks.push(TraceTick { tick: 1, frame: Frame(882), position: SongPosition { order: 1, pattern: 1, row: 1 }, tick_in_row: 0, channels: vec![channel], ..trace.ticks[0].clone() });

        let difference = diff_libxmp_dump(ConformanceFormat::S3m, &dump, &trace, &SampleGeometry::default(), &[]);
        let first = difference.first_divergence.expect("the row disagrees");
        assert_eq!((first.tick, first.field), (Some(1), TraceField::Row), "a mis-paired row is a field, not an error");
        assert_eq!((first.expected.as_str(), first.actual.as_str()), ("0", "1"));
    }

    #[test]
    fn a_waived_timeline_re_anchors_on_row_and_frame_when_the_offset_jumps() {
        // Three records a tick apart in libxmp's timeline. StarPlayer's second tick runs
        // at 63 BPM, so its end lands 1218 frames later than the rolling offset predicts —
        // a CIA-latched tempo change under D15 — and the third follows it. Without
        // re-anchoring, records two and three pair with nothing and are reported against
        // the wrong ticks. (Tick end frames come from the trace's `bpm`/`speed`, not its
        // `frame` column.)
        let dump = parse_libxmp_dump(concat!(
            "20 0 0 0 1753088 60 0 1024 0 0\n",
            "40 1 0 0 1753088 60 0 1024 0 0\n",
            "60 2 0 0 1753088 60 0 1024 0 0\n",
        )).expect("valid dump");
        let channel = TraceChannel { channel: 0, active: true, note: Some(48), instrument: 1, sample: 1, volume: 64, period: 1712, pan: 136, position: 0, cutoff: 255, resonance: 0, flags: DirtyBits::empty() };
        let mut trace = one_tick_trace(channel.clone());
        trace.ticks.push(TraceTick { tick: 1, frame: Frame(882), bpm: 63, position: SongPosition { order: 0, pattern: 0, row: 1 }, tick_in_row: 0, channels: vec![channel.clone()], ..trace.ticks[0].clone() });
        trace.ticks.push(TraceTick { tick: 2, frame: Frame(882 + 2100), position: SongPosition { order: 0, pattern: 0, row: 2 }, tick_in_row: 0, channels: vec![channel], ..trace.ticks[0].clone() });

        let waived = diff_libxmp_dump(ConformanceFormat::S3m, &dump, &trace, &SampleGeometry::default(), &[TraceField::Frame]);
        assert!(waived.is_identical(), "the pairer re-anchored on (row, frame) after the jump: {waived}");
        let enforced = diff_libxmp_dump(ConformanceFormat::S3m, &dump, &trace, &SampleGeometry::default(), &[]);
        assert_eq!(enforced.first_divergence.expect("the timing differs").field, TraceField::Frame, "without the waiver the jump is a real frame divergence");
    }

    #[test]
    fn position_comparison_wraps_around_a_forward_loop() {
        // The exact position is 64.003 in a 64-frame loop: StarPlayer reports 0 and
        // libxmp 63, one frame apart circularly and 63 apart linearly.
        let dump = parse_libxmp_dump("20 0 0 0 1753088 60 0 1024 0 63\n").expect("valid dump");
        let channel = TraceChannel { channel: 0, active: true, note: Some(48), instrument: 1, sample: 1, volume: 64, period: 1712, pan: 136, position: 0, cutoff: 255, resonance: 0, flags: DirtyBits::empty() };
        let trace = one_tick_trace(channel);
        let looping = geometry(&[Some(SampleSpan { length_frames: 64, loop_start: 0, loop_end: 64, looping: true })]);
        let one_shot = geometry(&[Some(SampleSpan { length_frames: 64, loop_start: 0, loop_end: 0, looping: false })]);

        assert!(diff_libxmp_dump(ConformanceFormat::S3m, &dump, &trace, &looping, &[]).is_identical(), "wrapped positions are one frame apart");
        let difference = diff_libxmp_dump(ConformanceFormat::S3m, &dump, &trace, &one_shot, &[]);
        assert_eq!(difference.first_divergence.expect("different").field, TraceField::Position, "a one-shot has no loop to wrap through");
    }

    #[test]
    fn a_one_shot_ending_inside_the_interval_is_the_d18_projection() {
        // libxmp drops a `NOTE_SAMPLE_END` voice after mixing the interval; C1 snapshots
        // the channel before it. A one-shot with 10 frames left at 44.1 kHz cannot
        // survive an 882-frame tick, so its absence upstream is not an active-set gap.
        let dump = parse_libxmp_dump("20 0 0 1 1753088 60 0 1024 0 0\n").expect("valid dump");
        let ending = TraceChannel { channel: 0, active: true, note: Some(48), instrument: 1, sample: 1, volume: 64, period: 856, pan: 136, position: 990u64 << 32, cutoff: 255, resonance: 0, flags: DirtyBits::empty() };
        let sounding = TraceChannel { channel: 1, period: 428, position: 0, ..ending.clone() };
        let mut trace = one_tick_trace(ending);
        trace.ticks[0].channels.push(sounding);
        let short = geometry(&[Some(SampleSpan { length_frames: 1_000, loop_start: 0, loop_end: 0, looping: false })]);
        let long = geometry(&[Some(SampleSpan { length_frames: 1_000_000, loop_start: 0, loop_end: 0, looping: false })]);

        let projected = diff_libxmp_dump(ConformanceFormat::Mtm, &dump, &trace, &short, &[]);
        assert!(projected.is_identical(), "the one-shot ends inside this interval: {projected}");
        let difference = diff_libxmp_dump(ConformanceFormat::Mtm, &dump, &trace, &long, &[]);
        assert_eq!(difference.first_divergence.expect("different").field, TraceField::Active, "a voice with data left is still an active-set mismatch");
    }

    #[test]
    fn a_waiver_hides_exactly_one_field_and_enforces_the_rest() {
        let dump = parse_libxmp_dump("20 0 0 0 1753088 60 0 1024 0 0\n").expect("valid dump");
        let channel = TraceChannel { channel: 0, active: true, note: Some(48), instrument: 1, sample: 1, volume: 60, period: 1712, pan: 136, position: 0, cutoff: 255, resonance: 0, flags: DirtyBits::empty() };
        let trace = one_tick_trace(channel);

        let difference = diff_libxmp_dump(ConformanceFormat::S3m, &dump, &trace, &SampleGeometry::default(), &[TraceField::Position]);
        assert_eq!(difference.first_divergence.expect("different").field, TraceField::Volume, "waiving position leaves volume enforced");
        assert!(diff_libxmp_dump(ConformanceFormat::S3m, &dump, &trace, &SampleGeometry::default(), &[TraceField::Volume]).is_identical());
    }

    #[test]
    fn note_projection_carries_an_out_of_range_note_instead_of_clamping_it() {
        assert_eq!(project_note(ConformanceFormat::Mod, 36), Some(12));
        assert_eq!(project_note(ConformanceFormat::Mod, 24), Some(0));
        assert_eq!(project_note(ConformanceFormat::Mod, 23), None, "a note below the MOD offset has no image on the comparison axis");
        assert_eq!(project_note(ConformanceFormat::S3m, 11), None);
        assert_eq!(project_note(ConformanceFormat::S3m, 12), Some(0));
    }

    #[test]
    fn the_tick_budget_comes_from_the_oracle_timeline_not_its_record_count() {
        // One record at 10 seconds: the oracle is one line long, but the trace has to
        // reach output frame 441_000 to be comparable at all.
        let dump = parse_libxmp_dump("10000 0 0 0 1753088 60 0 1024 0 0\n").expect("valid dump");
        let budget = oracle_tick_budget(&dump);
        assert!(budget > 1_000, "a one-line oracle at ten seconds still needs {budget} ticks");
        assert_eq!(oracle_end_frame(&dump), Frame(441_000));

        let truncated = one_tick_trace(TraceChannel::default());
        assert!(check_tick_budget(&dump, &truncated, 1).unwrap_err().contains("tick budget"), "exhaustion is a harness error");
        assert!(check_tick_budget(&dump, &truncated, budget).is_ok(), "a short trace under budget ended for its own reasons");
    }

    #[test]
    fn manifest_rejects_duplicate_pairs_and_false_provenance() {
        let duplicate = concat!(
            "one\tlibxmp\ts3m\tdata/a.s3m\tdata/a.data\tfirst\n",
            "two\tlibxmp\ts3m\tdata/a.s3m\tdata/a.data\tsecond\n",
        );
        assert!(parse_case_manifest(duplicate).unwrap_err().contains("duplicate module/oracle pair"));
        assert!(parse_case_manifest("one\topenmpt\ts3m\tdata/a.s3m\tdata/a.data\tcase\n").unwrap_err().contains("source does not match"));
        assert!(parse_case_manifest("one\tlibxmp\tmod\tdata/a.s3m\tdata/a.data\tcase\n").unwrap_err().contains("format does not match"));
    }

    #[test]
    fn upstream_call_scan_requires_a_real_function_token() {
        let source = concat!(
            "/* don't use compare_mixer_data, this prose is not a call */\n",
            "compare_mixer_data(\"data/a.s3m\", \"data/a.data\");\n",
            "compare_mixer_data_loops (\"data/b.mtm\", \"data/b.data\", 2);\n",
            "not_compare_mixer_data(\"data/fake.mod\", \"data/fake.data\");\n",
        );
        assert_eq!(mixer_calls(source).expect("valid calls"), vec![
            ("data/a.s3m".to_string(), "data/a.data".to_string()),
            ("data/b.mtm".to_string(), "data/b.data".to_string()),
        ]);
    }

    #[test]
    fn libxmp_dump_groups_channels_and_accepts_optional_filter_fields() {
        let dump = parse_libxmp_dump(concat!(
            "20 0 0 0 1753088 60 0 1024 -16 0\n",
            "20 0 0 1 1753088 60 1 512 16 4 0\n",
            "40 0 1 0 1752064 60 0 1008 -16 167 0 0\n",
        )).expect("valid dump");
        assert_eq!(dump.len(), 2);
        assert_eq!(dump[0].channels.len(), 2);
        assert_eq!(dump[0].channels[0].cutoff, None);
        assert_eq!(dump[0].channels[1].cutoff, Some(0));
        assert_eq!(dump[1].channels[0].resonance, Some(0));
    }

    #[test]
    fn adapter_uses_the_c1_differ_and_names_the_first_field() {
        let dump = parse_libxmp_dump("20 0 0 0 1753088 60 0 1024 0 0 0 0\n").expect("valid dump");
        let mut trace = Trace {
            version: TRACE_FORMAT_VERSION,
            ticks: vec![TraceTick {
                tick: 0,
                frame: Frame::ZERO,
                position: SongPosition { order: 0, pattern: 0, row: 0 },
                tick_in_row: 0,
                speed: 6,
                bpm: 125,
                global_volume: 64,
                channels: vec![TraceChannel {
                    channel: 0,
                    active: true,
                    note: Some(48),
                    instrument: 1,
                    sample: 1,
                    volume: 64,
                    period: 1712,
                    pan: 136,
                    position: 0,
                    cutoff: 255,
                    resonance: 0,
                    flags: DirtyBits::PITCH,
                }],
            }],
        };
        assert!(diff_libxmp_dump(ConformanceFormat::S3m, &dump, &trace, &SampleGeometry::default(), &[]).is_identical());
        trace.ticks[0].channels[0].volume = 63;
        let difference = diff_libxmp_dump(ConformanceFormat::S3m, &dump, &trace, &SampleGeometry::default(), &[]);
        assert_eq!(difference.first_divergence.expect("different").field.to_string(), "volume");
    }

    #[test]
    fn pan_projection_preserves_mod_8xx_and_decodes_nibble_formats() {
        assert_eq!(project_pan(ConformanceFormat::Mod, -15), 113);
        assert_eq!(project_pan(ConformanceFormat::S3m, -15), 119);
        assert_eq!(project_pan(ConformanceFormat::Mtm, -15), 119);
    }

    #[test]
    fn mod_projection_uses_native_axes_and_libxmp_integer_position_bound() {
        let dump = parse_libxmp_dump("20 0 0 0 1753088 60 0 1024 8 7 0 0\n").expect("valid dump");
        let mut trace = Trace {
            version: TRACE_FORMAT_VERSION,
            ticks: vec![TraceTick {
                tick: 0,
                frame: Frame::ZERO,
                position: SongPosition { order: 0, pattern: 0, row: 0 },
                tick_in_row: 0,
                speed: 6,
                bpm: 125,
                global_volume: 64,
                channels: vec![TraceChannel {
                    channel: 0,
                    active: true,
                    note: Some(48),
                    instrument: 1,
                    sample: 1,
                    volume: 64,
                    period: 428,
                    pan: 136,
                    position: (7u64 << 32) | 0x89ab_cdef,
                    cutoff: 255,
                    resonance: 0,
                    flags: DirtyBits::empty(),
                }],
            }],
        };
        let identical = diff_libxmp_dump(ConformanceFormat::Mod, &dump, &trace, &SampleGeometry::default(), &[]);
        assert!(identical.is_identical(), "{identical}");

        trace.ticks[0].channels[0].position = (8u64 << 32) | 1;
        assert!(diff_libxmp_dump(ConformanceFormat::Mod, &dump, &trace, &SampleGeometry::default(), &[]).is_identical());

        trace.ticks[0].channels[0].position = 9u64 << 32;
        let difference = diff_libxmp_dump(ConformanceFormat::Mod, &dump, &trace, &SampleGeometry::default(), &[]);
        assert_eq!(difference.first_divergence.expect("different").field.to_string(), "position");
    }

    #[test]
    fn mtm_projection_uses_its_linear_note_and_amiga_period_axes() {
        let dump = parse_libxmp_dump("20 0 0 0 3506176 48 0 1024 0 0 0 0\n").expect("valid dump");
        let trace = Trace {
            version: TRACE_FORMAT_VERSION,
            ticks: vec![TraceTick {
                tick: 0,
                frame: Frame::ZERO,
                position: SongPosition { order: 0, pattern: 0, row: 0 },
                tick_in_row: 0,
                speed: 6,
                bpm: 125,
                global_volume: 64,
                channels: vec![TraceChannel {
                    channel: 0,
                    active: true,
                    note: Some(36),
                    instrument: 1,
                    sample: 1,
                    volume: 64,
                    period: 856,
                    pan: 136,
                    position: 0,
                    cutoff: 255,
                    resonance: 0,
                    flags: DirtyBits::empty(),
                }],
            }],
        };
        let identical = diff_libxmp_dump(ConformanceFormat::Mtm, &dump, &trace, &SampleGeometry::default(), &[]);
        assert!(identical.is_identical(), "{identical}");
    }

    #[test]
    fn only_disabled_and_fully_open_cutoffs_are_normalized() {
        let fully_open = parse_libxmp_dump("20 0 0 0 1753088 60 0 1024 0 0 254 0\n").expect("valid dump");
        let operative = parse_libxmp_dump("20 0 0 0 1753088 60 0 1024 0 0 253 0\n").expect("valid dump");
        let trace = Trace {
            version: TRACE_FORMAT_VERSION,
            ticks: vec![TraceTick {
                tick: 0,
                frame: Frame::ZERO,
                position: SongPosition { order: 0, pattern: 0, row: 0 },
                tick_in_row: 0,
                speed: 6,
                bpm: 125,
                global_volume: 64,
                channels: vec![TraceChannel {
                    channel: 0,
                    active: true,
                    note: Some(48),
                    instrument: 1,
                    sample: 1,
                    volume: 64,
                    period: 1712,
                    pan: 136,
                    position: 0,
                    cutoff: 255,
                    resonance: 0,
                    flags: DirtyBits::empty(),
                }],
            }],
        };
        assert!(diff_libxmp_dump(ConformanceFormat::S3m, &fully_open, &trace, &SampleGeometry::default(), &[]).is_identical());
        let difference = diff_libxmp_dump(ConformanceFormat::S3m, &operative, &trace, &SampleGeometry::default(), &[]);
        assert_eq!(difference.first_divergence.expect("different").field.to_string(), "cutoff");
    }
}
