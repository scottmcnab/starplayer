//! The effect set, and the one place a host names an effect.
//!
//! M7 lands them in three tasks: H1 the [`gain`] trim that the [`Insert`] trait is proved
//! against, H3 the EQ, delay and chorus, H4 the reverb and compressor. [`InsertKind`] and
//! [`build`] grow one arm each time, and a host never names a concrete effect type.

pub mod chorus;
pub mod compressor;
pub mod delay;
pub mod eq;
pub mod gain;
pub mod reverb;
#[cfg(test)]
pub(crate) mod testing;

pub use chorus::Chorus;
pub use compressor::Compressor;
pub use delay::Delay;
pub use eq::Eq;
pub use reverb::Reverb;
pub use gain::{GAIN_MAX_CENTI_DB, GAIN_MIN_CENTI_DB, GAIN_PARAM, GainInsert, fader_gain_q15};

use alloc::boxed::Box;

use crate::insert::Insert;
use crate::sample::DspSample;

/// Every effect the engine can build.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum InsertKind {
    /// A smoothed gain trim in centi-decibels ([`GainInsert`]).
    #[default]
    Gain,
    /// A three-band shelving/peaking equaliser ([`Eq`]).
    Eq,
    /// A stereo delay with damped feedback and an optional ping-pong ([`Delay`]).
    Delay,
    /// Two or three modulated taps per channel ([`Chorus`]).
    Chorus,
    /// Freeverb: eight combs into four allpasses per channel ([`Reverb`]).
    Reverb,
    /// A feed-forward, stereo-linked peak compressor ([`Compressor`]).
    Compressor,
}

impl InsertKind {
    /// Every effect this crate builds, in the order a host should list them.
    ///
    /// H7 uses this so `--list-effects`, the wasm host's descriptor export and the web
    /// page's effect selector all enumerate the same six things without duplicating the
    /// list a fourth time.
    pub const ALL: [InsertKind; 6] =
        [InsertKind::Gain, InsertKind::Eq, InsertKind::Delay, InsertKind::Chorus, InsertKind::Reverb, InsertKind::Compressor];

    /// The short lower-case name a built effect's own [`InsertDescriptor::name`] reads —
    /// stated here as a `const fn` so a host can name a target before it has built
    /// anything.
    pub const fn name(self) -> &'static str {
        match self {
            InsertKind::Gain => "gain",
            InsertKind::Eq => "eq",
            InsertKind::Delay => "delay",
            InsertKind::Chorus => "chorus",
            InsertKind::Reverb => "reverb",
            InsertKind::Compressor => "compressor",
        }
    }

    /// The kind named `name`, or `None` for anything else. The inverse of
    /// [`InsertKind::name`]; a host parsing a `--insert channel:effect:...` flag or a wire
    /// message uses this rather than building one of everything to compare descriptors.
    pub fn from_name(name: &str) -> Option<InsertKind> { InsertKind::ALL.into_iter().find(|kind| kind.name() == name) }
}

/// Build one effect, boxed for a chain slot.
///
/// **Off the audio thread.** Building an effect allocates — its delay lines, from H3
/// onwards — which is exactly why an insert arrives at the engine over a control ring
/// rather than being constructed inside `render()`.
///
/// `sample_rate_hz` is the rate the effect will run at; a time-based effect sizes its
/// delay lines from it. The gain trim has no use for it.
pub fn build_insert<Sample: DspSample>(kind: InsertKind, sample_rate_hz: u32) -> Box<dyn Insert<Sample>> {
    match kind {
        InsertKind::Gain => Box::new(GainInsert::new()),
        InsertKind::Eq => Box::new(Eq::new(sample_rate_hz)),
        InsertKind::Delay => Box::new(Delay::new(sample_rate_hz)),
        InsertKind::Chorus => Box::new(Chorus::new(sample_rate_hz)),
        InsertKind::Reverb => Box::new(Reverb::new(sample_rate_hz)),
        InsertKind::Compressor => Box::new(Compressor::new(sample_rate_hz)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::insert::ParamId;

    #[test]
    fn build_hands_out_an_effect_at_its_defaults() {
        let insert: Box<dyn Insert<f32>> = build_insert(InsertKind::Gain, 44_100);
        assert_eq!(insert.descriptor().name, "gain");
        assert_eq!(insert.param(ParamId(0)), Some(0));
    }

    #[test]
    fn every_kind_builds_on_both_paths_at_its_own_defaults() {
        for kind in InsertKind::ALL {
            let float: Box<dyn Insert<f32>> = build_insert(kind, 44_100);
            let fixed: Box<dyn Insert<i32>> = build_insert(kind, 44_100);
            assert_eq!(float.descriptor(), fixed.descriptor(), "{kind:?} describes itself differently on the two paths");
            for (index, spec) in float.descriptor().params.iter().enumerate() {
                assert_eq!(float.param(ParamId(index as u8)), Some(spec.default), "{kind:?}.{} is not at its default", spec.name);
            }
        }
    }

    #[test]
    fn every_kind_names_itself_the_way_its_own_descriptor_does_and_round_trips_through_from_name() {
        for kind in InsertKind::ALL {
            let built: Box<dyn Insert<f32>> = build_insert(kind, 44_100);
            assert_eq!(kind.name(), built.descriptor().name, "{kind:?}.name() disagrees with its own descriptor");
            assert_eq!(InsertKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(InsertKind::from_name("not-an-effect"), None);
    }

    #[test]
    fn both_sample_types_build() {
        let float: Box<dyn Insert<f32>> = build_insert(InsertKind::Gain, 48_000);
        let fixed: Box<dyn Insert<i32>> = build_insert(InsertKind::Gain, 48_000);
        assert_eq!(float.descriptor().name, fixed.descriptor().name);
    }
}
