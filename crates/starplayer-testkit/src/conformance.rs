//! libxmp/OpenMPT corpus parsing and projection onto the C1 trace contract.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use starplayer::core::{ExactFixedPoint, Frame, FrameClock};
use starplayer::engine::{TRACE_FORMAT_VERSION, Trace, TraceChannel, TraceTick};

use crate::{TraceDiff, TraceTolerances, diff_traces};

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

/// A known failure which is still executed on every run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConformanceExclusion {
    pub case_id: String,
    pub reason: String,
    pub reference: String,
}

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
        if fields.len() != 3 {
            return Err(format!("exclusions line {line_number}: expected case id, reason, and reference"));
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
        let exclusion = ConformanceExclusion { case_id: case_id.to_string(), reason: reason.to_string(), reference: reference.to_string() };
        if exclusions.insert(case_id.to_string(), exclusion).is_some() {
            return Err(format!("exclusions line {line_number}: duplicate case `{case_id}`"));
        }
    }
    Ok(exclusions)
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

/// Project libxmp's partial state records and StarPlayer's full C1 trace onto the same
/// shape, then invoke the C1 differ.
pub fn diff_libxmp_dump(format: ConformanceFormat, upstream: &[LibxmpTick], actual: &Trace) -> Result<TraceDiff, String> {
    let (expected, actual) = project_libxmp_dump(format, upstream, actual)?;
    // libxmp's own mixer-data comparator permits one integer source frame. MOD first
    // projects C1's fraction away below, then applies that same whole-frame bound;
    // S3M/MTM preserve C2's equivalent tolerance.
    let position_tolerance = 1u64 << 32;
    let tolerances = TraceTolerances { frame: 45, period: 1, position: position_tolerance, ..TraceTolerances::default() };
    Ok(diff_traces(&expected, &actual, &tolerances))
}

fn project_libxmp_dump(format: ConformanceFormat, upstream: &[LibxmpTick], actual: &Trace) -> Result<(Trace, Trace), String> {
    let mut expected_ticks = Vec::with_capacity(upstream.len());
    let mut actual_ticks = Vec::with_capacity(upstream.len());
    let actual_end_frames = tick_end_frames(actual);
    let mut actual_index = 0usize;

    for upstream_tick in upstream {
        while let Some(actual_tick) = actual.ticks.get(actual_index) {
            if actual_tick.position.row == upstream_tick.row && actual_tick.tick_in_row == upstream_tick.frame {
                break;
            }
            if actual_tick.channels.iter().any(|channel| channel.active) {
                return Err(format!(
                    "trace alignment diverged at actual tick {}: expected row {} frame {}, found row {} frame {} with active channels",
                    actual_tick.tick, upstream_tick.row, upstream_tick.frame, actual_tick.position.row, actual_tick.tick_in_row,
                ));
            }
            actual_index += 1;
        }
        let Some(actual_tick) = actual.ticks.get(actual_index) else {
            return Err(format!("trace ended before libxmp row {} frame {} at {} ms", upstream_tick.row, upstream_tick.frame, upstream_tick.time_ms));
        };

        let mut expected_tick = header_projection(actual_tick);
        expected_tick.frame = Frame((upstream_tick.time_ms.saturating_mul(44_100).saturating_add(500)) / 1_000);
        expected_tick.position.row = upstream_tick.row;
        expected_tick.tick_in_row = upstream_tick.frame;
        let mut actual_tick_projection = header_projection(actual_tick);
        actual_tick_projection.frame = actual_end_frames[actual_index];

        let upstream_by_channel: BTreeMap<u16, &LibxmpChannel> = upstream_tick.channels.iter().map(|channel| (channel.channel, channel)).collect();
        let channel_ids: BTreeSet<u16> = upstream_by_channel.keys().copied()
            .chain(actual_tick.channels.iter().filter(|channel| channel.active).map(|channel| channel.channel))
            .collect();
        for channel_id in channel_ids {
            let mut actual_channel = actual_tick.channels.iter().find(|channel| channel.channel == channel_id).cloned()
                .unwrap_or_else(|| TraceChannel { channel: channel_id, ..TraceChannel::default() });
            // C1 names the sample reference-rate pitch C-4, one octave above the
            // ProTracker display octave used as the MOD comparison axis. libxmp's
            // mixer note is a further octave above C1, so expected and actual remove
            // two octaves and one octave respectively.
            if format == ConformanceFormat::Mod {
                actual_channel.note = actual_channel.note.map(|note| note.saturating_sub(12));
            }
            actual_channel.position = project_actual_position(format, actual_channel.position);
            let mut expected_channel = actual_channel.clone();
            if let Some(upstream_channel) = upstream_by_channel.get(&channel_id) {
                expected_channel.active = true;
                expected_channel.note = Some(project_note(format, upstream_channel.note));
                expected_channel.instrument = upstream_channel.instrument_zero_based.saturating_add(1);
                expected_channel.volume = (upstream_channel.volume_x16 + 8) / 16;
                expected_channel.period = project_period(format, upstream_channel.period_q12);
                expected_channel.pan = project_pan(format, upstream_channel.pan_signed);
                expected_channel.position = (upstream_channel.position as u64) << 32;
                if let Some(cutoff) = upstream_channel.cutoff {
                    // libxmp uses zero for its disabled-filter sentinel and treats all
                    // values at or above 254 as equivalent fully-open cutoffs. C1 uses
                    // 255 for that state. Operative values remain exact.
                    expected_channel.cutoff = if cutoff == 0 || cutoff >= 254 { 255 } else { cutoff };
                }
                if let Some(resonance) = upstream_channel.resonance {
                    expected_channel.resonance = resonance;
                }
            } else {
                // An active StarPlayer channel omitted by libxmp is an observable active
                // set mismatch, not a channel that projection may discard.
                expected_channel.active = false;
            }
            // These two C1 fields have no libxmp dump column.
            expected_channel.sample = actual_channel.sample;
            expected_channel.flags = actual_channel.flags;
            expected_tick.channels.push(expected_channel);
            actual_tick_projection.channels.push(actual_channel);
        }
        expected_ticks.push(expected_tick);
        actual_ticks.push(actual_tick_projection);
        actual_index += 1;
    }

    Ok((Trace { version: TRACE_FORMAT_VERSION, ticks: expected_ticks }, Trace { version: actual.version, ticks: actual_ticks }))
}

fn project_note(format: ConformanceFormat, note: u8) -> u8 {
    // The MOD comparison axis is ProTracker's displayed octave: libxmp's mixer is
    // two octaves above it, while C1 (projected above) is one. S3M/MTM compare on
    // C1's axis and therefore remove libxmp's one-octave bias.
    note.saturating_sub(match format {
        ConformanceFormat::Mod => 24,
        ConformanceFormat::S3m | ConformanceFormat::Mtm => 12,
    })
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
        assert!(diff_libxmp_dump(ConformanceFormat::S3m, &dump, &trace).expect("aligned").is_identical());
        trace.ticks[0].channels[0].volume = 63;
        let difference = diff_libxmp_dump(ConformanceFormat::S3m, &dump, &trace).expect("aligned");
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
        let identical = diff_libxmp_dump(ConformanceFormat::Mod, &dump, &trace).expect("aligned");
        assert!(identical.is_identical(), "{identical}");

        trace.ticks[0].channels[0].position = (8u64 << 32) | 1;
        assert!(diff_libxmp_dump(ConformanceFormat::Mod, &dump, &trace).expect("aligned").is_identical());

        trace.ticks[0].channels[0].position = 9u64 << 32;
        let difference = diff_libxmp_dump(ConformanceFormat::Mod, &dump, &trace).expect("aligned");
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
        let identical = diff_libxmp_dump(ConformanceFormat::Mtm, &dump, &trace).expect("aligned");
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
        assert!(diff_libxmp_dump(ConformanceFormat::S3m, &fully_open, &trace).expect("aligned").is_identical());
        let difference = diff_libxmp_dump(ConformanceFormat::S3m, &operative, &trace).expect("aligned");
        assert_eq!(difference.first_divergence.expect("different").field.to_string(), "cutoff");
    }
}
