//! [`SampleEnhancer`] — a load-time transform on one sample's decoded PCM — and
//! [`Module::enhanced`](crate::Module::enhanced), the module rebuild that applies one.
//!
//! # Why a rebuild rather than a builder field
//!
//! Every loader creates its own [`ModuleBuilder`](crate::ModuleBuilder) and only MOD has
//! a load-options type, so a `&dyn SampleEnhancer` field on the builder would put a
//! lifetime on the builder and on every `&mut ModuleBuilder` helper in the S3M, XM and IT
//! loaders. Rebuilding instead keeps the loaders untouched: a `Module` is already the
//! complete, validated description of a song, and every part of it can be handed back to
//! a fresh builder in id order.
//!
//! That is only sound because the model was already shaped for it — sample and pattern ids
//! are push order, `sample_pcm(id)[..length_frames()]` is exactly the stored body, and
//! every header field goes through `set_header`. So an **identity** enhancer rebuilds a
//! module that compares equal to the one it came from, which is
//! `tests::an_identity_enhancer_rebuilds_an_equal_module` below.
//!
//! # What an enhancer may change
//!
//! The frames, the loop points, the sustain loop, and the rate — but only by a power of
//! two, at most [`MAX_RATE_SCALE_LOG2`](crate::sample::MAX_RATE_SCALE_LOG2) doublings in
//! total. The factor is recorded in
//! [`SampleSpec::rate_scale_log2`](crate::SampleSpec::rate_scale_log2) and
//! `reference_rate_hz` keeps the file's own value; playback shifts the step and the sample
//! offset by the exponent. See that field for why the rate itself must not move.
//!
//! Nothing else changes: name, volume, pan, auto-vibrato, the patterns, the instruments,
//! the orders and the header are copied through verbatim.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use starplayer_core::{Error, SampleId};

use crate::builder::ModuleBuilder;
use crate::module::Module;
use crate::pattern::PatternId;
use crate::sample::{LoopMode, SustainLoop};

/// One sample's decoded PCM, as an enhancer sees it.
///
/// `frames` is the **stored body** — the addressable frames, guard frames excluded — which
/// for a forward or ping-pong loop with no sustain loop is exactly `0 .. loop_end`. The
/// loop fields are the sample's own, in those frames' units.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SamplePcm<'pcm> {
    /// The stored body, guard frames excluded.
    pub frames: &'pcm [i16],
    /// The rate these frames represent: the sample's reference rate as the file spells it,
    /// times any [`rate_scale_log2`](crate::SampleSpec::rate_scale_log2) already applied.
    /// An enhancer that changes the rate must return this value times a power of two.
    pub rate_hz: u32,
    /// XM's signed semitone offset, raw. Needed because an XM sample's *effective* rate is
    /// `rate_hz × 2^((relative_note + finetune/128)/12)`, not `rate_hz`, and a rate
    /// ceiling that ignored it would be dishonest for exactly the format that transposes
    /// most.
    pub relative_note: i8,
    /// XM's signed finetune in 1/128 semitone, raw. See [`SamplePcm::relative_note`].
    pub finetune: i8,
    /// How the sample repeats.
    pub loop_mode: LoopMode,
    /// First frame of the loop.
    pub loop_start: u32,
    /// One past the last frame of the loop.
    pub loop_end: u32,
    /// IT's sustain loop, if the sample has one.
    pub sustain_loop: Option<SustainLoop>,
}

/// What an enhancer hands back: the same sample, possibly at a higher rate.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EnhancedPcm {
    /// The new stored body. A forward or ping-pong loop with no sustain loop must end
    /// exactly at its `loop_end`, as [`ModuleBuilder::add_sample`] requires.
    pub frames: Vec<i16>,
    /// The rate these frames represent. Must be [`SamplePcm::rate_hz`] times `2^k` for
    /// `k` in `0 ..= 3`, or the rebuild fails with [`Error::Invalid`].
    pub rate_hz: u32,
    /// How the sample repeats.
    pub loop_mode: LoopMode,
    /// First frame of the loop, in the returned frames' units.
    pub loop_start: u32,
    /// One past the last frame of the loop, in the returned frames' units.
    pub loop_end: u32,
    /// IT's sustain loop, in the returned frames' units.
    pub sustain_loop: Option<SustainLoop>,
}

impl EnhancedPcm {
    /// The input, unchanged — what an enhancer returns when it declines a sample.
    pub fn unchanged(source: SamplePcm<'_>) -> EnhancedPcm {
        EnhancedPcm {
            frames: Vec::from(source.frames),
            rate_hz: source.rate_hz,
            loop_mode: source.loop_mode,
            loop_start: source.loop_start,
            loop_end: source.loop_end,
            sustain_loop: source.sustain_loop,
        }
    }
}

/// A load-time transform on one sample's decoded PCM.
///
/// Implementations run **outside** `render()`, once per sample, so they may allocate and
/// they may use whatever arithmetic they like — but they must be deterministic, because a
/// rebuilt module is hashed as a regression contract and because the same module has to
/// come out the same on x86, ARM and WASM.
pub trait SampleEnhancer {
    /// A name that goes into the enhanced configuration's golden filename
    /// (`stem__i16_mono_44100_linear_enh-<name>.sha256`), so it must encode **every**
    /// parameter that changes the output — `sinc4x`, `loop=64` — and nothing that does
    /// not.
    fn name(&self) -> String;

    /// Transform one sample.
    fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm;
}

impl<T: SampleEnhancer + ?Sized> SampleEnhancer for Box<T> {
    fn name(&self) -> String { (**self).name() }
    fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm { (**self).enhance(sample) }
}

impl Module {
    /// Rebuild this module with `enhancer` applied to every sample.
    ///
    /// Patterns, instruments, orders and the header are copied through in id order, so the
    /// only thing that can differ is the sample table and the PCM blob. An identity
    /// enhancer therefore produces a module that compares **equal** to this one.
    ///
    /// A sample that already carries a non-zero
    /// [`rate_scale_log2`](crate::SampleSpec::rate_scale_log2) is enhanced from its stored
    /// frames — the enhancer sees the rate those frames really are — and the two scales
    /// add.
    ///
    /// # Errors
    ///
    /// * [`Error::Invalid`] — the enhancer returned a rate that is not the source rate
    ///   times a power of two, or a rebuilt sample that
    ///   [`ModuleBuilder::add_sample`] rejects (a loop that no longer fits its frames, a
    ///   total rate scale above `2^3`).
    /// * [`Error::OutOfRange`] / [`Error::TooLarge`] — as [`ModuleBuilder::add_sample`]
    ///   and [`ModuleBuilder::build`] report them; a 4x rebuild of a very large module can
    ///   legitimately outgrow the `u32` PCM offsets.
    pub fn enhanced(&self, enhancer: &dyn SampleEnhancer) -> Result<Module, Error> {
        let mut builder = ModuleBuilder::new();
        builder.set_header(self.header().clone());
        builder.set_orders(self.orders());

        for instrument in self.instruments().iter() {
            builder.add_instrument(instrument.clone())?;
        }
        for (index, pattern) in self.patterns().iter().enumerate() {
            let id = PatternId(u16::try_from(index).map_err(|_| Error::TooLarge("more than 65536 patterns"))?);
            let bytes = self.pattern_bytes(id).ok_or(Error::OutOfRange)?;
            builder.add_pattern(bytes, pattern.rows(), pattern.channels())?;
        }
        for (index, sample) in self.samples().iter().enumerate() {
            let id = SampleId(u16::try_from(index).map_err(|_| Error::TooLarge("more than 65536 samples"))?);
            let readable = self.sample_pcm(id).ok_or(Error::OutOfRange)?;
            let frames = readable.get(..sample.length_frames() as usize).ok_or(Error::OutOfRange)?;
            let source_rate_hz = (sample.reference_rate_hz() as u64) << sample.rate_scale_log2();
            let source_rate_hz = u32::try_from(source_rate_hz).unwrap_or(u32::MAX);
            let enhanced = enhancer.enhance(SamplePcm {
                frames,
                rate_hz: source_rate_hz,
                relative_note: sample.relative_note(),
                finetune: sample.finetune(),
                loop_mode: sample.loop_mode(),
                loop_start: sample.loop_start(),
                loop_end: sample.loop_end(),
                sustain_loop: sample.sustain_loop(),
            });
            let added_scale = rate_scale_log2_between(source_rate_hz, enhanced.rate_hz)?;
            let specification = crate::sample::SampleSpec {
                loop_mode: enhanced.loop_mode,
                loop_start: enhanced.loop_start,
                loop_end: enhanced.loop_end,
                sustain_loop: enhanced.sustain_loop,
                rate_scale_log2: sample.rate_scale_log2().saturating_add(added_scale),
                ..sample.to_spec()
            };
            builder.add_sample(&enhanced.frames, specification)?;
        }
        builder.build()
    }
}

/// `log2(enhanced / source)`, or [`Error::Invalid`] if that ratio is not an exact power of
/// two in `1 ..= 8`.
fn rate_scale_log2_between(source_rate_hz: u32, enhanced_rate_hz: u32) -> Result<u8, Error> {
    const NOT_A_POWER_OF_TWO: Error = Error::Invalid("an enhancer must return the source rate times a power of two");
    if source_rate_hz == 0 || enhanced_rate_hz < source_rate_hz {
        return Err(NOT_A_POWER_OF_TWO);
    }
    if enhanced_rate_hz % source_rate_hz != 0 {
        return Err(NOT_A_POWER_OF_TWO);
    }
    let ratio = enhanced_rate_hz / source_rate_hz;
    if !ratio.is_power_of_two() {
        return Err(NOT_A_POWER_OF_TWO);
    }
    let exponent = ratio.trailing_zeros();
    if exponent > crate::sample::MAX_RATE_SCALE_LOG2 as u32 {
        return Err(NOT_A_POWER_OF_TWO);
    }
    Ok(exponent as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{ModuleFormat, ModuleHeader};
    use crate::instrument::InstrumentDef;
    use crate::module::ORDER_END;
    use crate::sample::SampleSpec;
    use alloc::vec;
    use starplayer_core::U0F16;

    /// The enhancer every rebuild is measured against: it changes nothing at all, so
    /// `module.enhanced(&Identity)` must compare **equal** to `module`.
    struct Identity;

    impl SampleEnhancer for Identity {
        fn name(&self) -> String { String::from("identity") }
        fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm { EnhancedPcm::unchanged(sample) }
    }

    /// Repeats every frame `factor` times and multiplies the rate by the same factor —
    /// the cheapest thing that exercises the rate-scale bookkeeping without a resampler.
    struct HoldUpsampler {
        factor: u32,
    }

    impl SampleEnhancer for HoldUpsampler {
        fn name(&self) -> String {
            let mut name = String::from("hold");
            let _ = core::fmt::Write::write_fmt(&mut name, format_args!("{}", self.factor));
            name
        }

        fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm {
            let factor = self.factor.max(1);
            let mut frames = Vec::with_capacity(sample.frames.len() * factor as usize);
            for frame in sample.frames {
                for _ in 0..factor {
                    frames.push(*frame);
                }
            }
            // A ping-pong loop turns *on* `end - 1`, so its scaled end is
            // `factor * (end - 1) + 1` rather than `factor * end`; every other shape
            // scales straight through. The body has to end at the loop end for a loop
            // with no sustain loop, so it is truncated to match.
            let (loop_start, loop_end) = scaled_loop(sample.loop_mode, sample.loop_start, sample.loop_end, factor);
            if sample.sustain_loop.is_none() && sample.loop_mode.is_looping() {
                frames.truncate(loop_end as usize);
            }
            EnhancedPcm {
                frames,
                rate_hz: sample.rate_hz * factor,
                loop_mode: sample.loop_mode,
                loop_start,
                loop_end,
                sustain_loop: sample.sustain_loop.map(|sustain| {
                    let (start, end) = scaled_loop(sustain.mode, sustain.start, sustain.end, factor);
                    SustainLoop { mode: sustain.mode, start, end }
                }),
            }
        }
    }

    fn scaled_loop(mode: LoopMode, start: u32, end: u32, factor: u32) -> (u32, u32) {
        match mode {
            LoopMode::PingPong => (start * factor, (end.saturating_sub(1)) * factor + 1),
            _ => (start * factor, end * factor),
        }
    }

    /// One of every sample shape the builder knows: a forward loop, a ping-pong loop, a
    /// one-shot, an empty sample and a sample with a sustain loop.
    fn every_sample_shape() -> Module {
        let mut builder = ModuleBuilder::new();
        let forward = builder
            .add_sample(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9], SampleSpec::one_shot("forward").with_forward_loop(2, 8))
            .expect("a forward loop");
        builder
            .add_sample(&[10, 20, 30, 40, 50, 60], SampleSpec { loop_mode: LoopMode::PingPong, loop_start: 1, loop_end: 5, ..SampleSpec::one_shot("bounce") })
            .expect("a ping-pong loop");
        builder.add_sample(&[100, -100, 200, -200], SampleSpec::one_shot("hit").with_reference_rate(16_000)).expect("a one-shot");
        builder.add_sample(&[], SampleSpec::one_shot("silence")).expect("an empty sample");
        builder
            .add_sample(
                &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
                SampleSpec {
                    loop_mode: LoopMode::Forward,
                    loop_start: 8,
                    loop_end: 12,
                    sustain_loop: Some(SustainLoop { mode: LoopMode::PingPong, start: 2, end: 6 }),
                    relative_note: -12,
                    finetune: 64,
                    ..SampleSpec::one_shot("sustained")
                },
            )
            .expect("a sustain loop");

        builder.add_instrument(InstrumentDef::from_sample("forward", forward, U0F16::MAX)).expect("an instrument");
        builder.add_pattern(&[1, 2, 3, 4], 64, 4).expect("a pattern");
        builder.add_pattern(&[9], 16, 4).expect("another pattern");
        builder.set_orders(&[0, 1, ORDER_END]);
        builder.set_header(ModuleHeader::new(ModuleFormat::It, 4));
        builder.build().expect("a valid module")
    }

    #[test]
    fn an_identity_enhancer_rebuilds_an_equal_module() {
        let module = every_sample_shape();
        let rebuilt = module.enhanced(&Identity).expect("an identity rebuild");
        assert_eq!(rebuilt, module, "an identity rebuild must be indistinguishable from the module it came from");
    }

    #[test]
    fn a_doubling_enhancer_records_its_factor_and_leaves_the_reference_rate_alone() {
        let module = every_sample_shape();
        let rebuilt = module.enhanced(&HoldUpsampler { factor: 4 }).expect("a 4x rebuild");
        for (index, (before, after)) in module.samples().iter().zip(rebuilt.samples().iter()).enumerate() {
            assert_eq!(after.rate_scale_log2(), 2, "sample {index} should carry 4x");
            assert_eq!(after.reference_rate_hz(), before.reference_rate_hz(), "sample {index}'s reference rate must not move");
            assert_eq!(after.length_frames(), expected_length(before), "sample {index}'s stored length");
        }
    }

    fn expected_length(sample: &crate::sample::SampleIndex) -> u32 {
        match (sample.sustain_loop().is_some(), sample.loop_mode()) {
            (false, LoopMode::PingPong) => (sample.loop_end() - 1) * 4 + 1,
            _ => sample.length_frames() * 4,
        }
    }

    #[test]
    fn the_scales_of_two_enhancements_add() {
        let module = every_sample_shape();
        let twice = module.enhanced(&HoldUpsampler { factor: 2 }).expect("a 2x rebuild");
        let again = twice.enhanced(&HoldUpsampler { factor: 2 }).expect("a second 2x rebuild");
        for sample in again.samples().iter() {
            assert_eq!(sample.rate_scale_log2(), 2, "two doublings are one quadrupling");
        }
        // The second pass has to be told the frames are already at twice the file's rate.
        let mut seen = vec![];
        struct RecordRate<'a>(core::cell::RefCell<&'a mut Vec<u32>>);
        impl SampleEnhancer for RecordRate<'_> {
            fn name(&self) -> String { String::from("record") }
            fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm {
                self.0.borrow_mut().push(sample.rate_hz);
                EnhancedPcm::unchanged(sample)
            }
        }
        let _ = twice.enhanced(&RecordRate(core::cell::RefCell::new(&mut seen))).expect("a rebuild");
        for (index, rate) in seen.iter().enumerate() {
            let source = module.sample(SampleId(index as u16)).expect("the sample").reference_rate_hz();
            assert_eq!(*rate, source * 2, "sample {index} is already at twice its file rate");
        }
    }

    #[test]
    fn a_rate_that_is_not_a_power_of_two_is_rejected() {
        struct Detune;
        impl SampleEnhancer for Detune {
            fn name(&self) -> String { String::from("detune") }
            fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm {
                EnhancedPcm { rate_hz: sample.rate_hz * 3, ..EnhancedPcm::unchanged(sample) }
            }
        }
        assert!(every_sample_shape().enhanced(&Detune).is_err(), "3x is not a power of two");
    }

    #[test]
    fn an_exact_power_of_two_ratio_is_the_only_accepted_one() {
        assert_eq!(rate_scale_log2_between(8_363, 8_363), Ok(0));
        assert_eq!(rate_scale_log2_between(8_363, 16_726), Ok(1));
        assert_eq!(rate_scale_log2_between(8_363, 33_452), Ok(2));
        assert_eq!(rate_scale_log2_between(8_363, 66_904), Ok(3));
        assert!(rate_scale_log2_between(8_363, 133_808).is_err(), "16x is past the model's limit");
        assert!(rate_scale_log2_between(8_363, 25_089).is_err(), "3x is not a power of two");
        assert!(rate_scale_log2_between(8_363, 8_000).is_err(), "an enhancer may not lower the rate");
        assert!(rate_scale_log2_between(0, 8_363).is_err(), "a zero source rate has no ratio");
    }
}
