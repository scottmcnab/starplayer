//! `--insert <target>:<effect>[:<param>=<value>,...]` and `--list-effects`, shared between
//! `starplayer play` and `starplayer render` (M7-H7).
//!
//! `<target>` is a 1-based channel number or `master` — the CLI's own convention, chosen
//! because a listener typing a flag counts channels from one; `InsertTarget::Channel`
//! itself is zero-based, so parsing a target is the one place that subtraction happens.
//! Repeating `--insert` for the same target fills that chain's four slots in the order
//! given; a fifth repeat for one target is refused rather than silently overwriting the
//! first.

use std::collections::HashMap;

use starplayer::core::ChannelId;
use starplayer::dsp::{InsertDescriptor, InsertKind, ParamId, ParamUnit, build_insert};
use starplayer::engine::{InsertTarget, MAX_INSERTS_PER_CHAIN};

/// One `--insert` flag, parsed and validated against its effect's own descriptor.
#[derive(Clone, Debug)]
pub struct InsertArg {
    /// Which bus.
    pub target: InsertTarget,
    /// Which of the four ordered slots — the next free one for `target`, in the order
    /// `--insert` named it.
    pub slot: u8,
    /// Which effect.
    pub kind: InsertKind,
    /// Parameters named away from the effect's own defaults, by position in its
    /// descriptor.
    pub params: Vec<(ParamId, i32)>,
}

/// Parse every `--insert` flag the caller repeated, assigning each target's chain slots
/// in the order given.
pub fn parse_insert_args(specs: &[String]) -> Result<Vec<InsertArg>, String> {
    let mut next_slot: HashMap<InsertTarget, u8> = HashMap::new();
    let mut parsed = Vec::with_capacity(specs.len());
    for spec in specs {
        let (target, kind, params) = parse_one(spec)?;
        let slot = next_slot.entry(target).or_insert(0);
        if *slot as usize >= MAX_INSERTS_PER_CHAIN {
            return Err(format!("{spec}: this target already has {MAX_INSERTS_PER_CHAIN} inserts, its whole chain"));
        }
        parsed.push(InsertArg { target, slot: *slot, kind, params });
        *slot += 1;
    }
    Ok(parsed)
}

/// One parsed `--insert` flag before its slot is assigned: the target, the effect, and
/// every `<param>=<value>` pair named in it.
type ParsedInsert = (InsertTarget, InsertKind, Vec<(ParamId, i32)>);

fn parse_one(spec: &str) -> Result<ParsedInsert, String> {
    let mut parts = spec.splitn(3, ':');
    let target_text = parts.next().filter(|text| !text.is_empty()).ok_or_else(|| format!("{spec}: empty --insert"))?;
    let effect_text = parts
        .next()
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("{spec}: expected <target>:<effect>[:<param>=<value>,...]"))?;
    let params_text = parts.next();

    let target = parse_target(target_text)?;
    let kind = InsertKind::from_name(effect_text).ok_or_else(|| {
        let names: Vec<&str> = InsertKind::ALL.iter().map(|kind| kind.name()).collect();
        format!("{effect_text}: not an effect; choose one of {}", names.join(", "))
    })?;

    // A throwaway instance, built only to read its `'static` descriptor — `build_insert`
    // is the one place a descriptor comes from, and the sample rate does not change what
    // a descriptor says (only how a time-based effect's delay lines are sized).
    let described: Box<dyn starplayer::dsp::Insert<f32>> = build_insert(kind, 44_100);
    let descriptor = described.descriptor();

    let params = match params_text.filter(|text| !text.is_empty()) {
        Some(params_text) => parse_params(spec, descriptor, params_text)?,
        None => Vec::new(),
    };
    Ok((target, kind, params))
}

fn parse_target(text: &str) -> Result<InsertTarget, String> {
    if text.eq_ignore_ascii_case("master") {
        return Ok(InsertTarget::Master);
    }
    let channel: u16 = text.parse().map_err(|_| format!("{text}: expected a 1-based channel number or `master`"))?;
    if channel == 0 {
        return Err(format!("{text}: channels are 1-based; the first channel is `1`"));
    }
    Ok(InsertTarget::Channel(ChannelId(channel - 1)))
}

fn parse_params(spec: &str, descriptor: &'static InsertDescriptor, params_text: &str) -> Result<Vec<(ParamId, i32)>, String> {
    let mut params = Vec::new();
    for pair in params_text.split(',') {
        if pair.is_empty() {
            continue;
        }
        let (key, value_text) = pair.split_once('=').ok_or_else(|| format!("{spec}: `{pair}` is not <param>=<value>"))?;
        let index = descriptor.params.iter().position(|candidate| candidate.name == key).ok_or_else(|| {
            let names: Vec<&str> = descriptor.params.iter().map(|candidate| candidate.name).collect();
            format!("{spec}: {} has no parameter `{key}`; choose one of {}", descriptor.name, names.join(", "))
        })?;
        let parameter = &descriptor.params[index];
        let value: i32 = value_text.parse().map_err(|_| format!("{spec}: `{key}={value_text}` is not a whole number"))?;
        if parameter.clamp(value) != value {
            return Err(format!(
                "{spec}: {}.{key} takes {}..={} ({}), not {value}",
                descriptor.name,
                parameter.min,
                parameter.max,
                unit_label(parameter.unit)
            ));
        }
        params.push((ParamId(index as u8), value));
    }
    Ok(params)
}

/// A short label for a parameter's unit, for `--list-effects` and an out-of-range error.
fn unit_label(unit: ParamUnit) -> &'static str {
    match unit {
        ParamUnit::CentiDecibels => "centi-dB",
        ParamUnit::Frames => "frames",
        ParamUnit::Cents => "cents",
        ParamUnit::Percent => "%",
        ParamUnit::Milliseconds => "ms",
        ParamUnit::CentiMilliseconds => "centi-ms",
        ParamUnit::Hertz => "Hz",
        ParamUnit::CentiHertz => "centi-Hz",
        ParamUnit::Count => "count",
        ParamUnit::Ratio => "x100",
        ParamUnit::Switch => "0/1",
    }
}

/// `--list-effects`: every effect this build can install, its parameters, their units,
/// ranges and defaults — read straight from each effect's own [`InsertDescriptor`], so
/// this can never drift from what `--insert` actually accepts.
pub fn list_effects() -> String {
    use std::fmt::Write;

    let mut out = String::new();
    for kind in InsertKind::ALL {
        let described: Box<dyn starplayer::dsp::Insert<f32>> = build_insert(kind, 44_100);
        let descriptor = described.descriptor();
        let _ = writeln!(out, "{}", descriptor.name);
        for parameter in descriptor.params {
            let _ = writeln!(
                out,
                "    {:<14} {:<9} {}..={} (default {})",
                parameter.name,
                unit_label(parameter.unit),
                parameter.min,
                parameter.max,
                parameter.default
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_target_parses_one_based() {
        let (target, kind, params) = parse_one("1:reverb:room=60,mix=40").expect("parses");
        assert_eq!(target, InsertTarget::Channel(ChannelId(0)));
        assert_eq!(kind, InsertKind::Reverb);
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn master_is_case_insensitive_and_needs_no_channel_number() {
        let (target, kind, _) = parse_one("Master:compressor:threshold=-1800,ratio=400").expect("parses");
        assert_eq!(target, InsertTarget::Master);
        assert_eq!(kind, InsertKind::Compressor);
    }

    #[test]
    fn an_insert_with_no_parameters_still_parses() {
        let (target, kind, params) = parse_one("2:gain").expect("parses");
        assert_eq!(target, InsertTarget::Channel(ChannelId(1)));
        assert_eq!(kind, InsertKind::Gain);
        assert!(params.is_empty());
    }

    #[test]
    fn channel_zero_is_refused_because_targets_are_one_based() { assert!(parse_one("0:gain").is_err()); }

    #[test]
    fn an_unknown_effect_names_the_choices() {
        let error = parse_one("1:flanger").unwrap_err();
        assert!(error.contains("gain"), "the error should list the real effect names: {error}");
    }

    #[test]
    fn an_unknown_parameter_names_the_choices() {
        let error = parse_one("1:reverb:sparkle=1").unwrap_err();
        assert!(error.contains("room"), "the error should list reverb's real parameter names: {error}");
    }

    #[test]
    fn an_out_of_range_value_names_the_range() {
        let error = parse_one("1:reverb:room=999").unwrap_err();
        assert!(error.contains("0..=100"), "the error should name reverb.room's own range: {error}");
    }

    #[test]
    fn repeating_insert_for_one_target_fills_consecutive_slots() {
        let specs = vec![String::from("1:eq"), String::from("1:delay"), String::from("1:chorus"), String::from("1:reverb")];
        let parsed = parse_insert_args(&specs).expect("four inserts fit the chain");
        assert_eq!(parsed.iter().map(|arg| arg.slot).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
        let fifth = parse_insert_args(&[String::from("1:eq"), String::from("1:delay"), String::from("1:chorus"), String::from("1:reverb"), String::from("1:gain")]);
        assert!(fifth.is_err(), "a fifth insert on one target has nowhere to go");
    }

    #[test]
    fn different_targets_each_start_at_slot_zero() {
        let specs = vec![String::from("1:eq"), String::from("2:eq"), String::from("master:eq")];
        let parsed = parse_insert_args(&specs).expect("parses");
        assert!(parsed.iter().all(|arg| arg.slot == 0));
    }

    #[test]
    fn list_effects_names_every_kind() {
        let listing = list_effects();
        for kind in InsertKind::ALL {
            assert!(listing.contains(kind.name()), "{}: missing from --list-effects", kind.name());
        }
    }
}
