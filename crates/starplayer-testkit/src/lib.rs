//! Test harness shared by the workspace: golden comparison, the libxmp / libopenmpt diff
//! harness, and the trace differ.
//!
//! C1 supplies the trace parser and differ. The corpus and its libxmp adapter are C2; the
//! engine trace module documents the upstream field mapping so that adapter has one
//! canonical place to implement it.
//!
//! # libxmp `test-dev` mapping for C2
//!
//! `gen_mixer_data` writes twelve whitespace-separated values:
//! `time row frame channel period note instrument volume pan position cutoff resonance`.
//! Map `row`, `frame` (libxmp's tick-in-row), `channel`, `period`, `note`, `instrument`,
//! `volume`, `pan`, integer `position`, `cutoff` and `resonance` to their corresponding v1
//! state. `time` is rounded end-of-frame milliseconds and is compared with StarPlayer's
//! exact tick-end frame. Presence in the dump defines the active channel set; extra
//! StarPlayer voices remain observable. Order/pattern, speed/BPM/global volume, sample
//! number, fractional position and dirty flags have no libxmp column. MOD explicitly
//! projects StarPlayer's Q32.32 position down to libxmp's whole-frame `pos0` domain;
//! all formats then apply libxmp's own one-frame position bound. Timing keeps libxmp's
//! one-millisecond bound.

#![forbid(unsafe_code)]

use std::fmt;

use starplayer::core::{DirtyBits, Frame};
use starplayer::engine::{TRACE_FORMAT_VERSION, SongPosition, Trace, TraceChannel, TraceTick};

pub mod conformance;

/// A malformed or unsupported trace text document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceParseError {
    /// One-based line number, or zero for an absent header.
    pub line: usize,
    /// Human-readable reason.
    pub message: String,
}

impl TraceParseError {
    fn new(line: usize, message: impl Into<String>) -> TraceParseError {
        TraceParseError { line, message: message.into() }
    }
}

impl fmt::Display for TraceParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 { write!(formatter, "trace: {}", self.message) } else { write!(formatter, "trace line {}: {}", self.line, self.message) }
    }
}

impl std::error::Error for TraceParseError {}

/// Parse the stable versioned line format emitted by [`Trace::to_text`].
pub fn parse_trace(text: &str) -> Result<Trace, TraceParseError> {
    let mut lines = text.lines().enumerate();
    let Some((_, header)) = lines.next() else { return Err(TraceParseError::new(0, "missing version header")) };
    let version_text = header.strip_prefix("starplayer-trace v=").ok_or_else(|| TraceParseError::new(1, "expected `starplayer-trace v=N`"))?;
    let version = parse_number::<u16>(version_text, 1, "version")?;
    if version != TRACE_FORMAT_VERSION {
        return Err(TraceParseError::new(1, format!("unsupported version {version}; expected {TRACE_FORMAT_VERSION}")));
    }

    let mut ticks = Vec::new();
    let mut current: Option<TraceTick> = None;
    for (zero_based_line, line) in lines {
        let line_number = zero_based_line + 1;
        if line.starts_with("t=") {
            if let Some(tick) = current.take() {
                ticks.push(tick);
            }
            current = Some(parse_tick(line, line_number)?);
        } else if line.starts_with(" ch=") {
            let Some(tick) = current.as_mut() else { return Err(TraceParseError::new(line_number, "channel appears before its tick header")) };
            tick.channels.push(parse_channel(line, line_number)?);
        } else if !line.is_empty() {
            return Err(TraceParseError::new(line_number, "expected a tick or indented channel line"));
        }
    }
    if let Some(tick) = current {
        ticks.push(tick);
    }
    Ok(Trace { version, ticks })
}

fn parse_tick(line: &str, line_number: usize) -> Result<TraceTick, TraceParseError> {
    let fields = parse_fields(line, line_number)?;
    expect_keys(&fields, &["t", "frm", "ord", "pat", "row", "tk", "spd", "bpm", "gv"], line_number)?;
    Ok(TraceTick {
        tick: parse_field(&fields, 0, line_number)?,
        frame: Frame(parse_field(&fields, 1, line_number)?),
        position: SongPosition {
            order: parse_field(&fields, 2, line_number)?,
            pattern: parse_field(&fields, 3, line_number)?,
            row: parse_field(&fields, 4, line_number)?,
        },
        tick_in_row: parse_field(&fields, 5, line_number)?,
        speed: parse_field(&fields, 6, line_number)?,
        bpm: parse_field(&fields, 7, line_number)?,
        global_volume: parse_field(&fields, 8, line_number)?,
        channels: Vec::new(),
    })
}

fn parse_channel(line: &str, line_number: usize) -> Result<TraceChannel, TraceParseError> {
    let fields = parse_fields(line, line_number)?;
    expect_keys(
        &fields,
        &["ch", "act", "note", "ins", "smp", "vol", "per", "pan", "pos", "cut", "res", "fl"],
        line_number,
    )?;
    Ok(TraceChannel {
        channel: parse_field(&fields, 0, line_number)?,
        active: match fields[1].1 {
            "0" => false,
            "1" => true,
            value => return Err(TraceParseError::new(line_number, format!("invalid activity `{value}`"))),
        },
        note: parse_note(fields[2].1, line_number)?,
        instrument: parse_field(&fields, 3, line_number)?,
        sample: parse_field(&fields, 4, line_number)?,
        volume: parse_field(&fields, 5, line_number)?,
        period: parse_field(&fields, 6, line_number)?,
        pan: parse_field(&fields, 7, line_number)?,
        position: parse_position(fields[8].1, line_number)?,
        cutoff: parse_field(&fields, 9, line_number)?,
        resonance: parse_field(&fields, 10, line_number)?,
        flags: parse_flags(fields[11].1, line_number)?,
    })
}

fn parse_fields(line: &str, line_number: usize) -> Result<Vec<(&str, &str)>, TraceParseError> {
    line.split_whitespace().map(|field| {
        field.split_once('=').ok_or_else(|| TraceParseError::new(line_number, format!("field `{field}` has no `=`")))
    }).collect()
}

fn expect_keys(fields: &[(&str, &str)], keys: &[&str], line_number: usize) -> Result<(), TraceParseError> {
    if fields.len() != keys.len() {
        return Err(TraceParseError::new(line_number, format!("expected {} fields, found {}", keys.len(), fields.len())));
    }
    for (index, expected) in keys.iter().enumerate() {
        let actual = fields.get(index).map(|field| field.0).unwrap_or("");
        if actual != *expected {
            return Err(TraceParseError::new(line_number, format!("expected field `{expected}`, found `{actual}`")));
        }
    }
    Ok(())
}

fn parse_field<T: std::str::FromStr>(fields: &[(&str, &str)], index: usize, line_number: usize) -> Result<T, TraceParseError> {
    let (name, value) = fields.get(index).copied().ok_or_else(|| TraceParseError::new(line_number, "missing field"))?;
    parse_number(value, line_number, name)
}

fn parse_number<T: std::str::FromStr>(value: &str, line_number: usize, name: &str) -> Result<T, TraceParseError> {
    value.parse().map_err(|_| TraceParseError::new(line_number, format!("invalid `{name}` value `{value}`")))
}

fn parse_note(value: &str, line_number: usize) -> Result<Option<u8>, TraceParseError> {
    if value == "---" {
        return Ok(None);
    }
    if value.len() < 3 {
        return Err(TraceParseError::new(line_number, format!("invalid note `{value}`")));
    }
    let (name, octave) = value.split_at(2);
    let semitone = ["C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-"]
        .iter().position(|candidate| *candidate == name)
        .ok_or_else(|| TraceParseError::new(line_number, format!("invalid note `{value}`")))?;
    let octave = parse_number::<u16>(octave, line_number, "note octave")?;
    let note = octave.saturating_mul(12).saturating_add(semitone as u16);
    u8::try_from(note).map(Some).map_err(|_| TraceParseError::new(line_number, format!("note `{value}` is out of range")))
}

fn parse_position(value: &str, line_number: usize) -> Result<u64, TraceParseError> {
    let (whole, fraction) = value.split_once('.').ok_or_else(|| TraceParseError::new(line_number, format!("invalid position `{value}`")))?;
    if fraction.len() != 8 {
        return Err(TraceParseError::new(line_number, "position fraction must contain exactly eight hexadecimal digits"));
    }
    let whole = parse_number::<u32>(whole, line_number, "position")?;
    let fraction = u32::from_str_radix(fraction, 16).map_err(|_| TraceParseError::new(line_number, format!("invalid position fraction `{fraction}`")))?;
    Ok((whole as u64) << 32 | fraction as u64)
}

fn parse_flags(value: &str, line_number: usize) -> Result<DirtyBits, TraceParseError> {
    if value == "-" {
        return Ok(DirtyBits::empty());
    }
    let mut flags = DirtyBits::empty();
    for symbol in value.chars() {
        flags.insert(match symbol {
            'V' => DirtyBits::VOLUME,
            'S' => DirtyBits::SAMPLE,
            'P' => DirtyBits::PITCH,
            'N' => DirtyBits::PAN,
            'T' => DirtyBits::TEMPO,
            'X' => DirtyBits::STOP,
            _ => return Err(TraceParseError::new(line_number, format!("unknown dirty flag `{symbol}`"))),
        });
    }
    Ok(flags)
}

/// Explicit tolerances for every numeric field in the v1 contract.
///
/// Exact comparison is [`TraceTolerances::default`]. There is deliberately no global
/// epsilon: a caller adapting libxmp can tolerate (for example) one unit of period and
/// one whole source frame of position without weakening order, note, flags, or anything
/// else.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceTolerances {
    pub tick: u64,
    pub frame: u64,
    pub order: u16,
    pub pattern: u16,
    pub row: u16,
    pub tick_in_row: u16,
    pub speed: u8,
    pub bpm: u16,
    pub global_volume: u8,
    pub channel: u16,
    pub note: u8,
    pub instrument: u16,
    pub sample: u16,
    pub volume: u16,
    pub period: u32,
    pub pan: u16,
    /// Q32.32 units. One whole source frame is `1 << 32`.
    pub position: u64,
    pub cutoff: u16,
    pub resonance: u16,
}

/// A named field in the stable trace contract.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TraceField {
    Version,
    TickCount,
    Tick,
    Frame,
    Order,
    Pattern,
    Row,
    TickInRow,
    Speed,
    Bpm,
    GlobalVolume,
    ChannelCount,
    Channel,
    Active,
    Note,
    Instrument,
    Sample,
    Volume,
    Period,
    Pan,
    Position,
    Cutoff,
    Resonance,
    Flags,
}

impl fmt::Display for TraceField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            TraceField::Version => "version",
            TraceField::TickCount => "tick-count",
            TraceField::Tick => "tick",
            TraceField::Frame => "frame",
            TraceField::Order => "order",
            TraceField::Pattern => "pattern",
            TraceField::Row => "row",
            TraceField::TickInRow => "tick-in-row",
            TraceField::Speed => "speed",
            TraceField::Bpm => "bpm",
            TraceField::GlobalVolume => "global-volume",
            TraceField::ChannelCount => "channel-count",
            TraceField::Channel => "channel",
            TraceField::Active => "active",
            TraceField::Note => "note",
            TraceField::Instrument => "instrument",
            TraceField::Sample => "sample",
            TraceField::Volume => "volume",
            TraceField::Period => "period",
            TraceField::Pan => "pan",
            TraceField::Position => "position",
            TraceField::Cutoff => "cutoff",
            TraceField::Resonance => "resonance",
            TraceField::Flags => "flags",
        };
        formatter.write_str(name)
    }
}

/// The earliest mismatching field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceDivergence {
    pub tick_index: Option<usize>,
    pub tick: Option<u64>,
    pub channel: Option<u16>,
    pub field: TraceField,
    pub expected: String,
    pub actual: String,
    pub tolerance: String,
}

/// Complete comparison result: one actionable first failure and compact totals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceDiff {
    pub first_divergence: Option<TraceDivergence>,
    pub divergent_ticks: usize,
    /// Number of per-tick channel records that differ.
    pub divergent_channels: usize,
    pub expected_ticks: usize,
    pub actual_ticks: usize,
    /// Previous/current/next tick blocks around the first divergence.
    pub context: String,
}

impl TraceDiff {
    pub const fn is_identical(&self) -> bool { self.first_divergence.is_none() }
}

impl fmt::Display for TraceDiff {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(first) = &self.first_divergence else {
            return write!(formatter, "identical: {} ticks", self.expected_ticks);
        };
        write!(formatter, "first divergence")?;
        if let Some(tick) = first.tick { write!(formatter, " at tick {tick}")?; }
        if let Some(channel) = first.channel { write!(formatter, " channel {channel}")?; }
        writeln!(formatter, " field {}: expected {}, actual {} (tolerance {})", first.field, first.expected, first.actual, first.tolerance)?;
        if !self.context.is_empty() {
            writeln!(formatter, "context:\n{}", self.context)?;
        }
        write!(
            formatter,
            "summary: {} divergent tick(s), {} divergent channel record(s); expected {} tick(s), actual {}",
            self.divergent_ticks,
            self.divergent_channels,
            self.expected_ticks,
            self.actual_ticks,
        )
    }
}

/// Compare two typed traces under an explicit per-field tolerance policy.
pub fn diff_traces(expected: &Trace, actual: &Trace, tolerances: &TraceTolerances) -> TraceDiff {
    let mut first_divergence = None;
    let mut divergent_ticks = 0usize;
    let mut divergent_channels = 0usize;

    if expected.version != actual.version {
        first_divergence = Some(divergence(None, None, None, TraceField::Version, expected.version, actual.version, 0));
    }

    let common_ticks = expected.ticks.len().min(actual.ticks.len());
    for tick_index in 0..common_ticks {
        let expected_tick = &expected.ticks[tick_index];
        let actual_tick = &actual.ticks[tick_index];
        let header_mismatch = compare_tick_header(expected_tick, actual_tick, tolerances);
        let mut tick_diverged = header_mismatch.is_some();
        if first_divergence.is_none()
            && let Some((field, expected_value, actual_value, tolerance)) = header_mismatch
        {
            first_divergence = Some(divergence(Some(tick_index), Some(expected_tick.tick), None, field, expected_value, actual_value, tolerance));
        }

        let common_channels = expected_tick.channels.len().min(actual_tick.channels.len());
        for channel_index in 0..common_channels {
            let expected_channel = &expected_tick.channels[channel_index];
            let actual_channel = &actual_tick.channels[channel_index];
            if let Some((field, expected_value, actual_value, tolerance)) = compare_channel(expected_channel, actual_channel, tolerances) {
                tick_diverged = true;
                divergent_channels += 1;
                if first_divergence.is_none() {
                    first_divergence = Some(divergence(
                        Some(tick_index),
                        Some(expected_tick.tick),
                        Some(expected_channel.channel),
                        field,
                        expected_value,
                        actual_value,
                        tolerance,
                    ));
                }
            }
        }
        if expected_tick.channels.len() != actual_tick.channels.len() {
            tick_diverged = true;
            divergent_channels += expected_tick.channels.len().abs_diff(actual_tick.channels.len());
            if first_divergence.is_none() {
                first_divergence = Some(divergence(
                    Some(tick_index),
                    Some(expected_tick.tick),
                    None,
                    TraceField::ChannelCount,
                    expected_tick.channels.len(),
                    actual_tick.channels.len(),
                    0,
                ));
            }
        }
        if tick_diverged {
            divergent_ticks += 1;
        }
    }

    if expected.ticks.len() != actual.ticks.len() {
        divergent_ticks += expected.ticks.len().abs_diff(actual.ticks.len());
        if first_divergence.is_none() {
            first_divergence = Some(divergence(
                Some(common_ticks),
                expected.ticks.get(common_ticks).map(|tick| tick.tick).or_else(|| actual.ticks.get(common_ticks).map(|tick| tick.tick)),
                None,
                TraceField::TickCount,
                expected.ticks.len(),
                actual.ticks.len(),
                0,
            ));
        }
    }

    let context = first_divergence.as_ref().and_then(|first| first.tick_index).map(|index| trace_context(expected, actual, index)).unwrap_or_default();
    TraceDiff {
        first_divergence,
        divergent_ticks,
        divergent_channels,
        expected_ticks: expected.ticks.len(),
        actual_ticks: actual.ticks.len(),
        context,
    }
}

/// Parse and compare two text traces.
pub fn diff_trace_text(expected: &str, actual: &str, tolerances: &TraceTolerances) -> Result<TraceDiff, TraceParseError> {
    Ok(diff_traces(&parse_trace(expected)?, &parse_trace(actual)?, tolerances))
}

type Mismatch = (TraceField, String, String, String);

fn compare_tick_header(expected: &TraceTick, actual: &TraceTick, tolerance: &TraceTolerances) -> Option<Mismatch> {
    macro_rules! numeric {
        ($field:ident, $expected:expr, $actual:expr, $tolerance:expr) => {
            if $expected.abs_diff($actual) > $tolerance {
                return Some((TraceField::$field, $expected.to_string(), $actual.to_string(), $tolerance.to_string()));
            }
        };
    }
    numeric!(Tick, expected.tick, actual.tick, tolerance.tick);
    numeric!(Frame, expected.frame.0, actual.frame.0, tolerance.frame);
    numeric!(Order, expected.position.order, actual.position.order, tolerance.order);
    numeric!(Pattern, expected.position.pattern, actual.position.pattern, tolerance.pattern);
    numeric!(Row, expected.position.row, actual.position.row, tolerance.row);
    numeric!(TickInRow, expected.tick_in_row, actual.tick_in_row, tolerance.tick_in_row);
    numeric!(Speed, expected.speed, actual.speed, tolerance.speed);
    numeric!(Bpm, expected.bpm, actual.bpm, tolerance.bpm);
    numeric!(GlobalVolume, expected.global_volume, actual.global_volume, tolerance.global_volume);
    None
}

fn compare_channel(expected: &TraceChannel, actual: &TraceChannel, tolerance: &TraceTolerances) -> Option<Mismatch> {
    macro_rules! numeric {
        ($field:ident, $expected:expr, $actual:expr, $tolerance:expr) => {
            if $expected.abs_diff($actual) > $tolerance {
                return Some((TraceField::$field, $expected.to_string(), $actual.to_string(), $tolerance.to_string()));
            }
        };
    }
    numeric!(Channel, expected.channel, actual.channel, tolerance.channel);
    if expected.active != actual.active { return Some((TraceField::Active, expected.active.to_string(), actual.active.to_string(), "exact".into())); }
    match (expected.note, actual.note) {
        (Some(expected), Some(actual)) => numeric!(Note, expected, actual, tolerance.note),
        (expected, actual) if expected != actual => return Some((TraceField::Note, format!("{expected:?}"), format!("{actual:?}"), tolerance.note.to_string())),
        _ => {}
    }
    numeric!(Instrument, expected.instrument, actual.instrument, tolerance.instrument);
    numeric!(Sample, expected.sample, actual.sample, tolerance.sample);
    numeric!(Volume, expected.volume, actual.volume, tolerance.volume);
    numeric!(Period, expected.period, actual.period, tolerance.period);
    numeric!(Pan, expected.pan, actual.pan, tolerance.pan);
    numeric!(Position, expected.position, actual.position, tolerance.position);
    numeric!(Cutoff, expected.cutoff, actual.cutoff, tolerance.cutoff);
    numeric!(Resonance, expected.resonance, actual.resonance, tolerance.resonance);
    if expected.flags != actual.flags {
        return Some((TraceField::Flags, format!("{:02x}", expected.flags.bits()), format!("{:02x}", actual.flags.bits()), "exact".into()));
    }
    None
}

fn divergence(
    tick_index: Option<usize>,
    tick: Option<u64>,
    channel: Option<u16>,
    field: TraceField,
    expected: impl ToString,
    actual: impl ToString,
    tolerance: impl ToString,
) -> TraceDivergence {
    TraceDivergence {
        tick_index,
        tick,
        channel,
        field,
        expected: expected.to_string(),
        actual: actual.to_string(),
        tolerance: tolerance.to_string(),
    }
}

fn trace_context(expected: &Trace, actual: &Trace, tick_index: usize) -> String {
    let start = tick_index.saturating_sub(1);
    let expected_end = (tick_index + 2).min(expected.ticks.len());
    let actual_end = (tick_index + 2).min(actual.ticks.len());
    let expected_ticks = expected.ticks.get(start..expected_end).unwrap_or(&[]).to_vec();
    let actual_ticks = actual.ticks.get(start..actual_end).unwrap_or(&[]).to_vec();
    format!(
        " expected:\n{} actual:\n{}",
        indent(&Trace { version: expected.version, ticks: expected_ticks }.to_text()),
        indent(&Trace { version: actual.version, ticks: actual_ticks }.to_text()),
    )
}

fn indent(text: &str) -> String {
    text.lines().map(|line| format!("  {line}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_trace() -> Trace {
        Trace {
            version: TRACE_FORMAT_VERSION,
            ticks: (0..3).map(|tick| TraceTick {
                tick,
                frame: Frame(tick * 882),
                position: SongPosition { order: 0, pattern: 0, row: (tick / 2) as u16 },
                tick_in_row: (tick % 2) as u16,
                speed: 2,
                bpm: 125,
                global_volume: 64,
                channels: vec![TraceChannel {
                    channel: 0,
                    active: true,
                    note: Some(60),
                    instrument: 1,
                    sample: 1,
                    volume: 32,
                    period: 1712,
                    pan: 128,
                    position: tick << 32,
                    cutoff: 255,
                    resonance: 0,
                    flags: DirtyBits::PITCH,
                }],
            }).collect(),
        }
    }

    #[test]
    fn stable_text_round_trips_without_loss() {
        let trace = example_trace();
        let parsed = parse_trace(&trace.to_text()).expect("version one parses");
        assert_eq!(parsed, trace);
        assert_eq!(parsed.to_text(), trace.to_text(), "the checked-in representation is canonical");
    }

    #[test]
    fn identical_traces_report_identical() {
        let trace = example_trace();
        let difference = diff_traces(&trace, &trace, &TraceTolerances::default());
        assert!(difference.is_identical());
        assert_eq!(difference.to_string(), "identical: 3 ticks");
    }

    #[test]
    fn first_divergence_names_the_exact_tick_channel_field_and_context() {
        let expected = example_trace();
        let mut actual = expected.clone();
        actual.ticks[1].channels[0].volume = 33;
        actual.ticks[2].channels[0].period = 1700;

        let difference = diff_traces(&expected, &actual, &TraceTolerances::default());
        let first = difference.first_divergence.as_ref().expect("different");
        assert_eq!((first.tick, first.channel, first.field), (Some(1), Some(0), TraceField::Volume));
        assert_eq!((difference.divergent_ticks, difference.divergent_channels), (2, 2));
        let report = difference.to_string();
        assert!(report.contains("first divergence at tick 1 channel 0 field volume: expected 32, actual 33"));
        assert!(report.contains("t=00000"), "the previous tick is context");
        assert!(report.contains("t=00002"), "the following tick is context");
        assert!(report.contains("summary: 2 divergent tick(s), 2 divergent channel record(s)"));
    }

    #[test]
    fn tolerance_is_explicit_and_applies_only_to_its_field() {
        let expected = example_trace();
        let mut actual = expected.clone();
        actual.ticks[1].channels[0].period += 2;
        let tolerances = TraceTolerances { period: 2, ..TraceTolerances::default() };
        assert!(diff_traces(&expected, &actual, &tolerances).is_identical());

        actual.ticks[1].channels[0].volume += 1;
        let difference = diff_traces(&expected, &actual, &tolerances);
        assert_eq!(difference.first_divergence.map(|first| first.field), Some(TraceField::Volume));
    }
}
