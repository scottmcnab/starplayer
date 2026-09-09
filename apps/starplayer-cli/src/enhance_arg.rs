//! `--enhance <SPEC>` and `--list-enhancers`, shared between `starplayer render`, `play`
//! and `info` (M10-K5b), after the `--insert` model in `insert_arg.rs`.
//!
//! Grammar: entries joined with `+`, each an id from `starplayer_enhance::CATALOGUE` —
//! `denoise`, `sinc4x`, `sinc2x`, `sbr`, `loop` or `loop=<frames>` (default
//! `starplayer_enhance::DEFAULT_CROSSFADE_FRAMES`). Order is the order the stages run in,
//! matching `Chain::name`'s own `+`-joined spelling — `--enhance sinc4x+loop` and a
//! `Chain`'s printed name read the same way.

use starplayer_enhance::{CATALOGUE, Chain, LoopSmoother, SampleEnhancer, descriptor, enhancer_for_id};

/// Parse a `--enhance` spec into the chain it names.
///
/// `rate_ceiling_hz` caps a `sincNx` stage's effective playback rate, exactly like
/// `starplayer_enhance::catalogue::from_flags` does; the loop smoother ignores it, since it
/// changes no rate.
pub fn parse_enhance_arg(spec: &str, rate_ceiling_hz: Option<u32>) -> Result<Chain, String> {
    let mut chain = Chain::new();
    for entry in spec.split('+') {
        if entry.is_empty() {
            return Err(format!("{spec}: empty entry in --enhance"));
        }
        chain = chain.then(parse_one(spec, entry, rate_ceiling_hz)?);
    }
    if chain.is_empty() {
        return Err(format!("{spec}: --enhance needs at least one enhancer"));
    }
    Ok(chain)
}

fn parse_one(spec: &str, entry: &str, rate_ceiling_hz: Option<u32>) -> Result<Box<dyn SampleEnhancer>, String> {
    if let Some(frames_text) = entry.strip_prefix("loop=") {
        let frames: u32 = frames_text.parse().map_err(|_| format!("{spec}: `{entry}` is not loop=<frames>"))?;
        return Ok(Box::new(LoopSmoother::new(frames)));
    }
    if descriptor(entry).is_none() {
        let ids: Vec<&str> = CATALOGUE.iter().map(|candidate| candidate.id).collect();
        return Err(format!("{spec}: `{entry}` is not an enhancer; choose one of {}, or loop=<frames>", ids.join(", ")));
    }
    // Every id in `CATALOGUE` is constructible (`catalogue::every_id_is_unique_and_constructible`),
    // so this only fails if that invariant is ever broken.
    enhancer_for_id(entry, rate_ceiling_hz).ok_or_else(|| format!("{spec}: `{entry}` has no constructor"))
}

/// `--list-enhancers`: every enhancer this build's catalogue offers — id, label and
/// description — the `--list-effects` shape, for the enhancer catalogue rather than the
/// insert-effect one.
pub fn list_enhancers() -> String {
    use std::fmt::Write;

    let mut out = String::new();
    for entry in CATALOGUE {
        let _ = writeln!(out, "{}", entry.id);
        let _ = writeln!(out, "    {}", entry.description);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_id_parses_to_a_one_stage_chain() {
        let chain = parse_enhance_arg("sinc4x", None).expect("parses");
        assert_eq!(chain.name(), "sinc4x");
    }

    #[test]
    fn a_plus_joined_spec_parses_in_order() {
        let chain = parse_enhance_arg("sinc4x+loop", None).expect("parses");
        assert_eq!(chain.name(), "sinc4x+loop=64");
    }

    #[test]
    fn loop_takes_an_explicit_frame_count() {
        let chain = parse_enhance_arg("loop=128", None).expect("parses");
        assert_eq!(chain.name(), "loop=128");
    }

    #[test]
    fn a_rate_ceiling_reaches_the_upsampler_and_shows_in_the_name() {
        let chain = parse_enhance_arg("sinc4x", Some(48_000)).expect("parses");
        assert_eq!(chain.name(), "sinc4x-ceil48000");
    }

    #[test]
    fn an_unknown_id_names_the_valid_choices() {
        let Err(error) = parse_enhance_arg("reverb", None) else { panic!("an unknown id must be rejected") };
        assert!(error.contains("sinc4x"), "the error should list the real enhancer ids: {error}");
        assert!(error.contains("loop=<frames>"), "the error should mention the loop=<frames> form: {error}");
    }

    #[test]
    fn a_malformed_loop_count_is_reported() {
        let Err(error) = parse_enhance_arg("loop=nope", None) else { panic!("a malformed frame count must be rejected") };
        assert!(error.contains("loop=nope"), "the error should quote the offending flag text: {error}");
    }

    #[test]
    fn an_empty_spec_entry_is_reported() {
        assert!(parse_enhance_arg("", None).is_err());
        assert!(parse_enhance_arg("sinc4x+", None).is_err());
    }

    #[test]
    fn list_enhancers_names_every_catalogue_entry() {
        let listing = list_enhancers();
        for entry in CATALOGUE {
            assert!(listing.contains(entry.id), "{}: missing from --list-enhancers", entry.id);
        }
    }
}
