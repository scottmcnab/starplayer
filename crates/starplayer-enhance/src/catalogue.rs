//! [`CATALOGUE`] — the one place an enhancer's id, its user-facing wording and its flag
//! bit are written down.
//!
//! K5b's `--enhance` parser and W4's `enhancements_json()` both read this table, and the
//! web front end transcribes nothing: a checkbox's label, its description and the bit it
//! sets all come from here, so adding an enhancer is one entry rather than four edits in
//! three languages.

use alloc::boxed::Box;

use starplayer_model::SampleEnhancer;

use crate::chain::Chain;
use crate::denoise::DecayDenoiser;
use crate::loop_smooth::LoopSmoother;
use crate::sbr::BandwidthExtender;
use crate::upsample::{SincUpsampler, UpsampleFactor};

/// [`EnhancerDescriptor::flag_bit`] for an enhancer that has no bit in the flag word — one
/// a command line can ask for but a checkbox cannot.
pub const NO_FLAG_BIT: u8 = u8::MAX;

/// The crossfade length the `loop` flag asks for. Sixty-four frames is a few milliseconds
/// at tracker rates: long enough to hide a step, short enough not to smear a short loop.
pub const DEFAULT_CROSSFADE_FRAMES: u32 = 64;

/// One enhancer, as a host offers it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EnhancerDescriptor {
    /// The stable identifier a command line spells and a hash filename carries.
    pub id: &'static str,
    /// A short label for a checkbox.
    pub label: &'static str,
    /// One sentence of help.
    pub description: &'static str,
    /// The bit this enhancer occupies in [`from_flags`]'s word, or [`NO_FLAG_BIT`].
    pub flag_bit: u8,
}

/// Every enhancer a host may offer, in the order [`from_flags`] applies them.
///
/// **Catalogue order is run order, and it is no longer the bit order.** Bits 0 and 1 were
/// shipped first and live in browsers' `localStorage`, so they cannot move; the order the
/// stages have to run in is a signal-chain question, not a history question:
///
/// * `denoise` first, on the **source** frames — its noise floor is read from the fact
///   that every frame is a multiple of 256, which is true of an 8-bit sample and false of
///   anything an upsampler has already touched.
/// * then the upsampler, which is what creates the headroom everything after it works in.
/// * then `sbr`, which needs that headroom: with none it is the identity.
/// * `loop` last, so the crossfade is measured in the rebuilt sample's own frames and the
///   seam it repairs is the seam playback will actually reach.
pub const CATALOGUE: &[EnhancerDescriptor] = &[
    EnhancerDescriptor {
        id: "denoise",
        label: "Reduce decay noise",
        description: "Attenuates each block of a sample by how much of it is signal rather than the constant hiss an 8-bit source carries, so notes fade into silence rather than into a wash of noise. Loud passages and loop regions are left at full level.",
        flag_bit: 2,
    },
    EnhancerDescriptor {
        id: "sinc4x",
        label: "Upsample samples (4x sinc)",
        description: "Rebuilds every sample at four times its stored rate through a 64-tap windowed sinc, so the mixer interpolates between real frames instead of inventing them.",
        flag_bit: 0,
    },
    EnhancerDescriptor {
        id: "sinc2x",
        label: "Upsample samples (2x sinc)",
        description: "The same filter at half the factor, for a module whose samples are already close to the output rate or a host with a tight memory budget.",
        flag_bit: NO_FLAG_BIT,
    },
    EnhancerDescriptor {
        id: "sbr",
        label: "Extend sample bandwidth",
        description: "Transposes the octave below a sample's band edge upwards, at the level the sample's own roll-off predicts, so a bright instrument recorded at 8 kHz gets a top octave back. A sample that already fills its band, or one that is dark because it was recorded dark, is left alone.",
        flag_bit: 3,
    },
    EnhancerDescriptor {
        id: "loop",
        label: "Smooth loop seams",
        description: "Crossfades the end of each forward loop into the frames that led into its start, removing the click a loop whose ends do not meet makes on every wrap. A loop that does not click is left untouched.",
        flag_bit: 1,
    },
];

/// The descriptor with this id, if there is one.
pub fn descriptor(id: &str) -> Option<&'static EnhancerDescriptor> {
    CATALOGUE.iter().find(|descriptor| descriptor.id == id)
}

/// Build the enhancer a catalogue id names.
///
/// `ceiling_hz` caps the effective playback rate of any sample an upsampler touches; it is
/// meaningless to the loop smoother, which changes no rate.
pub fn enhancer_for_id(id: &str, ceiling_hz: Option<u32>) -> Option<Box<dyn SampleEnhancer>> {
    let upsampler = |factor| {
        let upsampler = SincUpsampler::new(factor);
        match ceiling_hz {
            Some(hz) => upsampler.with_rate_ceiling(hz),
            None => upsampler,
        }
    };
    match id {
        "sinc4x" => Some(Box::new(upsampler(UpsampleFactor::Four))),
        "sinc2x" => Some(Box::new(upsampler(UpsampleFactor::Two))),
        "loop" => Some(Box::new(LoopSmoother::new(DEFAULT_CROSSFADE_FRAMES))),
        "denoise" => Some(Box::new(DecayDenoiser::new())),
        "sbr" => Some(Box::new(BandwidthExtender::new())),
        _ => None,
    }
}

/// The chain a flag word asks for, or `None` when it asks for nothing.
///
/// Bits are read in [`CATALOGUE`] order, which is also the order the stages run in. An
/// unknown bit is ignored rather than rejected, so a newer front end talking to an older
/// engine degrades to the enhancers that engine has.
pub fn from_flags(flags: u32, ceiling_hz: Option<u32>) -> Option<Chain> {
    let mut chain = Chain::new();
    for descriptor in CATALOGUE.iter() {
        if descriptor.flag_bit == NO_FLAG_BIT || descriptor.flag_bit >= u32::BITS as u8 {
            continue;
        }
        if flags & (1 << descriptor.flag_bit) == 0 {
            continue;
        }
        if let Some(enhancer) = enhancer_for_id(descriptor.id, ceiling_hz) {
            chain = chain.then(enhancer);
        }
    }
    if chain.is_empty() { None } else { Some(chain) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn every_id_is_unique_and_constructible() {
        for (index, descriptor) in CATALOGUE.iter().enumerate() {
            assert!(enhancer_for_id(descriptor.id, None).is_some(), "{} has no constructor", descriptor.id);
            assert!(!descriptor.label.is_empty() && !descriptor.description.is_empty(), "{} needs its wording", descriptor.id);
            for other in CATALOGUE.iter().skip(index + 1) {
                assert_ne!(descriptor.id, other.id, "duplicate id");
                if descriptor.flag_bit != NO_FLAG_BIT {
                    assert_ne!(descriptor.flag_bit, other.flag_bit, "{} and {} share a bit", descriptor.id, other.id);
                }
            }
        }
    }

    /// Bits 0 and 1 shipped first and live in browsers' `localStorage`, so they are
    /// frozen: a page that remembered "upsample and smooth loops" must still mean that
    /// after this crate grows two more enhancers.
    #[test]
    fn the_checkbox_bits_are_the_ones_that_shipped_and_the_new_ones_sit_above_them() {
        assert_eq!(descriptor("sinc4x").map(|descriptor| descriptor.flag_bit), Some(0));
        assert_eq!(descriptor("loop").map(|descriptor| descriptor.flag_bit), Some(1));
        assert_eq!(descriptor("denoise").map(|descriptor| descriptor.flag_bit), Some(2));
        assert_eq!(descriptor("sbr").map(|descriptor| descriptor.flag_bit), Some(3));
        assert_eq!(descriptor("sinc2x").map(|descriptor| descriptor.flag_bit), Some(NO_FLAG_BIT), "2x is command-line only");
    }

    /// Catalogue order is **run** order, and it is deliberately not bit order.
    #[test]
    fn the_catalogue_runs_the_signal_chain_in_the_order_the_stages_belong_in() {
        let ids: Vec<&str> = CATALOGUE.iter().map(|descriptor| descriptor.id).collect();
        assert_eq!(ids, ["denoise", "sinc4x", "sinc2x", "sbr", "loop"]);
    }

    #[test]
    fn a_flag_word_builds_the_chain_it_names_in_catalogue_order() {
        assert!(from_flags(0, None).is_none(), "no flags asks for nothing");
        assert_eq!(from_flags(1, None).map(|chain| chain.name()).as_deref(), Some("sinc4x"));
        assert_eq!(from_flags(2, None).map(|chain| chain.name()).as_deref(), Some("loop=64"));
        assert_eq!(from_flags(3, None).map(|chain| chain.name()).as_deref(), Some("sinc4x+loop=64"));
        assert_eq!(from_flags(4, None).map(|chain| chain.name()).as_deref(), Some("denoise"));
        assert_eq!(from_flags(8, None).map(|chain| chain.name()).as_deref(), Some("sbr"));
        assert_eq!(from_flags(0b1101, None).map(|chain| chain.name()).as_deref(), Some("denoise+sinc4x+sbr"), "run order, not bit order");
        assert_eq!(from_flags(0b1111, None).map(|chain| chain.name()).as_deref(), Some("denoise+sinc4x+sbr+loop=64"));
        assert!(from_flags(0xffff_fff0, None).is_none(), "every bit above the catalogue's is ignored");
        assert_eq!(from_flags(0xffff_fff2, None).map(|chain| chain.name()).as_deref(), Some("loop=64"), "unknown bits are ignored");
    }

    #[test]
    fn a_ceiling_reaches_the_upsampler_and_shows_in_the_name() {
        assert_eq!(from_flags(1, Some(48_000)).map(|chain| chain.name()).as_deref(), Some("sinc4x-ceil48000"));
        assert_eq!(from_flags(2, Some(48_000)).map(|chain| chain.name()).as_deref(), Some("loop=64"), "the smoother changes no rate");
    }
}
