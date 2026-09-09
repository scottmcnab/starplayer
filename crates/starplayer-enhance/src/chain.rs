//! [`Chain`] — several enhancers applied to one sample in order.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use starplayer_model::{EnhancedPcm, SampleEnhancer, SamplePcm};

/// Apply each enhancer in turn, feeding one's output to the next.
///
/// The name is the stages' names joined with `+`, so a chain identifies itself the way a
/// golden filename needs it to: `sinc4x+loop=64` is a different configuration from
/// `loop=64+sinc4x` and hashes under a different name.
///
/// `relative_note` and `finetune` are carried through from the original sample rather than
/// from the previous stage's output: no enhancer may change a sample's tuning, so they are
/// the same at every step, and passing them on is what lets a later stage's rate ceiling
/// stay honest about an XM sample.
pub struct Chain(pub Vec<Box<dyn SampleEnhancer>>);

impl Chain {
    /// An empty chain — the identity.
    pub const fn new() -> Chain { Chain(Vec::new()) }

    /// The same chain with `enhancer` appended.
    pub fn then(mut self, enhancer: Box<dyn SampleEnhancer>) -> Chain {
        self.0.push(enhancer);
        self
    }

    /// How many stages the chain has.
    pub fn len(&self) -> usize { self.0.len() }

    /// Whether the chain is the identity.
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

impl Default for Chain {
    fn default() -> Chain { Chain::new() }
}

impl SampleEnhancer for Chain {
    fn name(&self) -> String {
        if self.0.is_empty() {
            return String::from("none");
        }
        let mut name = String::new();
        for (index, stage) in self.0.iter().enumerate() {
            if index > 0 {
                name.push('+');
            }
            name.push_str(&stage.name());
        }
        name
    }

    fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm {
        let mut current = EnhancedPcm::unchanged(sample);
        for stage in self.0.iter() {
            let next = stage.enhance(SamplePcm {
                frames: &current.frames,
                rate_hz: current.rate_hz,
                relative_note: sample.relative_note,
                finetune: sample.finetune,
                loop_mode: current.loop_mode,
                loop_start: current.loop_start,
                loop_end: current.loop_end,
                sustain_loop: current.sustain_loop,
            });
            current = next;
        }
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LoopSmoother, SincUpsampler, UpsampleFactor};
    use starplayer_model::{DEFAULT_REFERENCE_RATE_HZ, LoopMode};
    use std::vec;

    fn looping(frames: &[i16]) -> SamplePcm<'_> {
        SamplePcm {
            frames,
            rate_hz: DEFAULT_REFERENCE_RATE_HZ,
            relative_note: 0,
            finetune: 0,
            loop_mode: LoopMode::Forward,
            loop_start: 0,
            loop_end: frames.len() as u32,
            sustain_loop: None,
        }
    }

    #[test]
    fn an_empty_chain_is_the_identity() {
        let frames = vec![1i16, 2, 3, 4];
        let chain = Chain::new();
        assert_eq!(chain.name(), "none");
        assert_eq!(chain.enhance(looping(&frames)).frames, frames);
    }

    #[test]
    fn a_chain_applies_in_order_and_names_itself_in_order() {
        let frames: std::vec::Vec<i16> = (0..128).map(|index| index as i16 * 200).collect();
        let chain = Chain::new()
            .then(Box::new(SincUpsampler::new(UpsampleFactor::Four)))
            .then(Box::new(LoopSmoother::new(64)));
        assert_eq!(chain.name(), "sinc4x+loop=64");

        let enhanced = chain.enhance(looping(&frames));
        assert_eq!(enhanced.rate_hz, DEFAULT_REFERENCE_RATE_HZ * 4, "the upsampler ran");
        assert_eq!(enhanced.frames.len(), 512);
        assert_eq!((enhanced.loop_start, enhanced.loop_end), (0, 512), "and the smoother left the loop alone");
    }

    #[test]
    fn the_second_stage_sees_the_first_stages_rate() {
        // A ceiling the source clears at 4x but the upsampled sample does not.
        let frames = vec![0i16; 64];
        let chain = Chain::new()
            .then(Box::new(SincUpsampler::new(UpsampleFactor::Two)))
            .then(Box::new(SincUpsampler::new(UpsampleFactor::Four).with_rate_ceiling(20_000)));
        let enhanced = chain.enhance(SamplePcm { rate_hz: 4_000, ..looping(&frames) });
        assert_eq!(enhanced.rate_hz, 4_000 * 2 * 2, "8 kHz × 4 would be 32 kHz, so the second stage fell back to 2x");
    }
}
