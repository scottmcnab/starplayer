//! [`ModuleBuilder`] — the one way to make a [`Module`], and the one place that decides
//! whether a file is playable.
//!
//! A loader parses bytes and calls `add_sample` / `add_pattern` / `add_instrument` /
//! `set_orders` / `set_header`; the builder lays the blobs out, fills the guard frames in
//! and checks every invariant listed on [`ModuleBuilder::build`]. Fuzz resistance is
//! concentrated here on purpose: a loader that mis-parses a field produces an `Err`,
//! not a [`Module`] that makes the mixer read the wrong memory.

use alloc::vec::Vec;
use starplayer_core::{Error, GUARD_FRAMES, InstrumentId, PRE_ROLL_FRAMES, SampleId};

use crate::header::ModuleHeader;
use crate::instrument::InstrumentDef;
use crate::module::Module;
use crate::pattern::{PatternId, PatternIndex};
use crate::sample::{LoopMode, SampleIndex, SampleSpec};

/// Accumulates the parts of a [`Module`] and validates them on [`ModuleBuilder::build`].
#[derive(Clone, Debug, Default)]
pub struct ModuleBuilder {
    blob: Vec<u8>,
    pcm: Vec<i16>,
    samples: Vec<SampleIndex>,
    patterns: Vec<PatternIndex>,
    orders: Vec<u16>,
    instruments: Vec<InstrumentDef>,
    header: Option<ModuleHeader>,
}

impl ModuleBuilder {
    /// An empty builder.
    pub fn new() -> ModuleBuilder { ModuleBuilder::default() }

    /// Append one sample's PCM to the module's blob, guard frames and all, and return the
    /// id the module will know it by.
    ///
    /// `pcm` is the **decoded** sample: delta-decoding, 8-bit widening and any format
    /// unpacking have already happened, and every sample in the module is `i16`
    /// (see the crate documentation for why).
    ///
    /// # The layout this writes
    ///
    /// Exactly what `starplayer_mixer::sample::SampleData::resolve` expects, which is why
    /// this is the only supported way to build the blob. Every sample's stored run is
    /// [`PRE_ROLL_FRAMES`] of silence, then its frames, then its [`GUARD_FRAMES`]; the
    /// returned id's `pcm_offset` points at **frame 0**, so the pre-roll sits just before
    /// it and every offset in the index means what it always meant. The pre-roll is what
    /// lets a cubic or windowed-sinc kernel read `index - 1` or `index - 3` at the very
    /// first frame of a sample; see [`PRE_ROLL_FRAMES`] for why it is silence and where a
    /// *loop's* leading frames come from instead.
    ///
    /// * **[`LoopMode::Forward`], no sustain loop** — frames `0 .. loop_end` are stored
    ///   and the tail after the loop end is discarded, because a forward loop never plays
    ///   it. The [`GUARD_FRAMES`] that follow repeat the loop from `loop_start`, wrapping
    ///   as many times as a loop shorter than the guard needs.
    /// * **[`LoopMode::PingPong`], no sustain loop** — frames `0 .. loop_end` are stored,
    ///   same as a forward loop, and the guard frames hold the **reflected** continuation
    ///   — `pcm[loop_end - 2], pcm[loop_end - 3], …` — computed with the same arithmetic
    ///   `starplayer_mixer::sample::LoopSpan::ping_pong_frame` uses to read them back, so
    ///   the two agree frame for frame.
    /// * **[`LoopMode::None`], no sustain loop** — the whole sample is stored and the
    ///   guard frames are silence, so a kernel interpolating over the final frame decays
    ///   to zero rather than clicking.
    /// * **any sample with a sustain loop**, regardless of `loop_mode` — the whole sample
    ///   is stored, because the normal loop and the sustain loop may each lie anywhere
    ///   inside it, and the guard frames are silence. Playback swaps the voice's sample
    ///   region between the sustain-loop region and the normal region on key-off; neither
    ///   region's loop end need sit at the stored length any more, so the guard cannot be
    ///   a loop continuation. One frame of `Linear` interpolation therefore reads real PCM
    ///   past a loop end here instead of the wrapped or reflected copy — accepted for now,
    ///   left for M7's kernels to reconsider.
    ///
    /// # Errors
    ///
    /// * [`Error::Invalid`] — a looping sample with `loop_start >= loop_end`; a sustain
    ///   loop with `start >= end` or a non-looping [`LoopMode`]; a zero `reference_rate_hz`
    ///   (which would make the resample step meaningless).
    /// * [`Error::OutOfRange`] — `loop_end`, or a sustain loop's `end`, past the end of
    ///   `pcm`.
    /// * [`Error::TooLarge`] — more samples, or more frames, than `u16` ids and `u32`
    ///   offsets can address.
    pub fn add_sample(&mut self, pcm: &[i16], specification: SampleSpec) -> Result<SampleId, Error> {
        let id = u16::try_from(self.samples.len()).map_err(|_| Error::TooLarge("more than 65536 samples"))?;

        if specification.reference_rate_hz == 0 {
            return Err(Error::Invalid("a sample needs a non-zero reference rate"));
        }
        let source_frames = u32::try_from(pcm.len()).map_err(|_| Error::TooLarge("a sample longer than 4 GFrames"))?;

        let looping = specification.loop_mode.is_looping();
        if looping {
            if specification.loop_start >= specification.loop_end {
                return Err(Error::Invalid("a looping sample needs loop_start < loop_end"));
            }
            if specification.loop_end > source_frames {
                return Err(Error::OutOfRange);
            }
        }

        let has_sustain_loop = if let Some(sustain) = specification.sustain_loop {
            if !sustain.mode.is_looping() {
                return Err(Error::Invalid("a sustain loop needs a looping mode"));
            }
            if sustain.start >= sustain.end {
                return Err(Error::Invalid("a sustain loop needs start < end"));
            }
            if sustain.end > source_frames {
                return Err(Error::OutOfRange);
            }
            true
        } else {
            false
        };

        // A forward or ping-pong loop with no sustain loop keeps only `0 .. loop_end`; a
        // sustain loop needs the whole sample, because neither loop's end has to sit at
        // the stored length any more; a one-shot keeps the lot too.
        let stored_frames = if has_sustain_loop {
            source_frames
        } else {
            match specification.loop_mode {
                LoopMode::Forward | LoopMode::PingPong => specification.loop_end,
                LoopMode::None => source_frames,
            }
        };

        let pre_roll_offset = u32::try_from(self.pcm.len()).map_err(|_| Error::TooLarge("module PCM larger than 4 GFrames"))?;
        let pcm_offset = pre_roll_offset.checked_add(PRE_ROLL_FRAMES as u32).ok_or(Error::TooLarge("module PCM larger than 4 GFrames"))?;
        let stored_total = (stored_frames as usize)
            .checked_add(PRE_ROLL_FRAMES + GUARD_FRAMES)
            .ok_or(Error::TooLarge("sample length"))?;
        let blob_end = (pre_roll_offset as usize).checked_add(stored_total).ok_or(Error::TooLarge("module PCM"))?;
        u32::try_from(blob_end).map_err(|_| Error::TooLarge("module PCM larger than 4 GFrames"))?;

        let body = pcm.get(..stored_frames as usize).ok_or(Error::OutOfRange)?;
        self.pcm.extend(core::iter::repeat_n(0, PRE_ROLL_FRAMES));
        self.pcm.extend_from_slice(body);
        match (has_sustain_loop, specification.loop_mode) {
            (true, _) => self.pcm.extend(core::iter::repeat_n(0, GUARD_FRAMES)),
            (false, LoopMode::Forward) => {
                let loop_length = specification.loop_end - specification.loop_start;
                for guard_index in 0..GUARD_FRAMES {
                    let wrapped = specification.loop_start as usize + guard_index % loop_length as usize;
                    self.pcm.push(pcm.get(wrapped).copied().unwrap_or(0));
                }
            }
            (false, LoopMode::PingPong) => {
                for guard_index in 0..GUARD_FRAMES as u32 {
                    let source = ping_pong_reflect(specification.loop_start, specification.loop_end, specification.loop_end + guard_index);
                    self.pcm.push(pcm.get(source as usize).copied().unwrap_or(0));
                }
            }
            (false, LoopMode::None) => self.pcm.extend(core::iter::repeat_n(0, GUARD_FRAMES)),
        }

        self.samples.push(SampleIndex::new(pcm_offset, stored_frames, specification));
        Ok(SampleId(id))
    }

    /// Append one pattern's **native** bytes to the module's blob and return the id the
    /// module will know it by.
    ///
    /// The model does not parse `bytes` and never will; `rows` and `channels` are what the
    /// loader read from the file's own header, and `length_bytes` is recorded so the
    /// region can be bounds-checked without understanding it.
    ///
    /// # Errors
    ///
    /// * [`Error::Invalid`] — zero rows or zero channels.
    /// * [`Error::TooLarge`] — more patterns, or a bigger blob, than the ids and offsets
    ///   can address.
    pub fn add_pattern(&mut self, bytes: &[u8], rows: u16, channels: u8) -> Result<PatternId, Error> {
        let id = u16::try_from(self.patterns.len()).map_err(|_| Error::TooLarge("more than 65536 patterns"))?;

        if rows == 0 || channels == 0 {
            return Err(Error::Invalid("a pattern needs at least one row and one channel"));
        }
        let blob_offset = u32::try_from(self.blob.len()).map_err(|_| Error::TooLarge("module blob larger than 4 GB"))?;
        let length_bytes = u32::try_from(bytes.len()).map_err(|_| Error::TooLarge("a pattern larger than 4 GB"))?;
        let blob_end = (blob_offset as usize).checked_add(bytes.len()).ok_or(Error::TooLarge("module blob"))?;
        u32::try_from(blob_end).map_err(|_| Error::TooLarge("module blob larger than 4 GB"))?;

        self.blob.extend_from_slice(bytes);
        self.patterns.push(PatternIndex::new(blob_offset, length_bytes, rows, channels));
        Ok(PatternId(id))
    }

    /// Add an instrument and return the id the module will know it by.
    ///
    /// The sample it names does not have to exist yet; [`ModuleBuilder::build`] checks
    /// that it does by then.
    pub fn add_instrument(&mut self, instrument: InstrumentDef) -> Result<InstrumentId, Error> {
        let id = u16::try_from(self.instruments.len()).map_err(|_| Error::TooLarge("more than 65536 instruments"))?;
        self.instruments.push(instrument);
        Ok(InstrumentId(id))
    }

    /// Set the order list, replacing anything set before.
    ///
    /// Values are the file's own: a pattern number, or
    /// [`ORDER_MARKER`](crate::ORDER_MARKER) / [`ORDER_END`](crate::ORDER_END). Any other
    /// value at or past the pattern count is rejected by [`ModuleBuilder::build`].
    pub fn set_orders(&mut self, orders: &[u16]) { self.orders.clear(); self.orders.extend_from_slice(orders); }

    /// Set the song header, replacing anything set before.
    pub fn set_header(&mut self, header: ModuleHeader) { self.header = Some(header); }

    /// Validate everything and produce the [`Module`].
    ///
    /// # Errors
    ///
    /// * [`Error::Invalid`] — no header was set; the header names zero channels; its pan
    ///   table, or its `default_channel_volume` table, is neither empty nor one entry per
    ///   channel; a looping sample has `loop_start >= loop_end` or a `loop_end` past its
    ///   stored length; a forward or ping-pong loop with no sustain loop stores anything
    ///   other than `max(loop_end, sustain_end)` frames; a pattern has no rows or no
    ///   channels.
    /// * [`Error::OutOfRange`] — a sample's frames or guard frames fall outside `pcm`; a
    ///   pattern's bytes fall outside `blob`; an instrument names a sample that does not
    ///   exist, or has a `note_sample_map` entry that does; an order names a pattern that
    ///   does not exist and is neither [`ORDER_MARKER`](crate::ORDER_MARKER) nor
    ///   [`ORDER_END`](crate::ORDER_END).
    pub fn build(self) -> Result<Module, Error> {
        let header = self.header.ok_or(Error::Invalid("no module header was set"))?;
        let module = Module::from_parts(
            self.blob.into_boxed_slice(),
            self.pcm.into_boxed_slice(),
            self.samples.into_boxed_slice(),
            self.patterns.into_boxed_slice(),
            self.orders.into_boxed_slice(),
            self.instruments.into_boxed_slice(),
            header,
        );
        module.validate()?;
        Ok(module)
    }

    /// Replace the sample table wholesale, bypassing [`ModuleBuilder::add_sample`].
    ///
    /// Test-only: it exists so the rejection tests can hand [`ModuleBuilder::build`] the
    /// corrupt index sets that the public API cannot produce — an offset past the end of
    /// the PCM blob, a pattern that overruns the module blob.
    #[cfg(test)]
    pub(crate) fn replace_samples_for_test(&mut self, samples: Vec<SampleIndex>) { self.samples = samples; }

    /// Replace the pattern table wholesale. Test-only; see
    /// [`ModuleBuilder::replace_samples_for_test`].
    #[cfg(test)]
    pub(crate) fn replace_patterns_for_test(&mut self, patterns: Vec<PatternIndex>) { self.patterns = patterns; }
}

/// The source frame a ping-pong loop over `start .. end` reads at `position` — which may
/// be at or past `end`, as the guard frames in [`ModuleBuilder::add_sample`] ask for —
/// reflecting exactly the way `starplayer_mixer::sample::LoopSpan::ping_pong_frame` does.
/// Reproduced here, rather than called there, because `starplayer-model` does not depend
/// on `starplayer-mixer`; `tests::a_ping_pong_guard_matches_the_mixers_own_arithmetic`
/// below is the proof the two agree.
///
/// Public because a sample enhancer that resamples a ping-pong loop has to extend the
/// source past `end` with exactly this reflection, so that the upsampled sample's own
/// guard frames — which [`ModuleBuilder::add_sample`] fills with the same function — and
/// the frames the resampler read are one definition rather than two.
pub const fn ping_pong_reflect(start: u32, end: u32, position: u32) -> u32 {
    let length = end - start;
    if length <= 1 {
        return start;
    }
    let period = 2 * (length - 1);
    let phase = (position - start) % period;
    if phase < length { start + phase } else { start + (period - phase) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::{ModuleFormat, ModuleHeader};
    use crate::module::{ORDER_END, ORDER_MARKER, OrderEntry};
    use crate::pattern::PatternId;
    use crate::sample::SustainLoop;
    use alloc::vec;
    use starplayer_core::U0F16;

    /// The four-channel S3M-shaped module the round-trip and rejection tests start from:
    /// one looping sample, one one-shot, one instrument, two patterns, an order list that
    /// uses both marker values.
    fn populated_builder() -> ModuleBuilder {
        let mut builder = ModuleBuilder::new();
        let looping = builder
            .add_sample(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9], SampleSpec::one_shot("loop").with_forward_loop(4, 8))
            .expect("a valid looping sample");
        let one_shot = builder.add_sample(&[100, 200], SampleSpec::one_shot("hit")).expect("a valid one-shot sample");

        builder.add_instrument(InstrumentDef::from_sample("loop", looping, U0F16::MAX)).expect("an instrument");
        builder.add_instrument(InstrumentDef::from_sample("hit", one_shot, U0F16::MAX)).expect("an instrument");

        builder.add_pattern(&[1, 2, 3, 4], 64, 4).expect("a valid pattern");
        builder.add_pattern(&[5, 6], 32, 4).expect("a valid pattern");
        builder.set_orders(&[0, ORDER_MARKER, 1, ORDER_END]);
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 4));
        builder
    }

    #[test]
    fn a_hand_built_module_round_trips_with_every_invariant_satisfied() {
        let module = populated_builder().build().expect("every invariant holds");

        assert_eq!(module.header().channel_count, 4);
        assert_eq!(module.header().format, ModuleFormat::S3m);
        assert_eq!(module.samples().len(), 2);
        assert_eq!(module.instruments().len(), 2);
        assert_eq!(module.patterns().len(), 2);
        assert_eq!(module.orders(), &[0, ORDER_MARKER, 1, ORDER_END]);

        let looping = module.sample(SampleId(0)).expect("sample 0 exists");
        assert_eq!(looping.name(), "loop");
        assert_eq!(looping.loop_mode(), LoopMode::Forward);
        assert_eq!(looping.length_frames(), 8, "a forward loop stores exactly loop_end frames");
        assert_eq!(looping.reference_rate_hz(), crate::DEFAULT_REFERENCE_RATE_HZ);
        assert_eq!(module.sample_pcm(SampleId(0)).map(<[i16]>::len), Some(8 + GUARD_FRAMES));

        let one_shot = module.sample(SampleId(1)).expect("sample 1 exists");
        assert_eq!(one_shot.pcm_offset(), (PRE_ROLL_FRAMES + 8 + GUARD_FRAMES + PRE_ROLL_FRAMES) as u32, "samples pack back to back, pre-roll and guard frames included");
        assert_eq!(module.sample_pcm(SampleId(1)).map(<[i16]>::len), Some(2 + GUARD_FRAMES));

        assert_eq!(module.pattern_bytes(PatternId(0)), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(module.pattern_bytes(PatternId(1)), Some(&[5u8, 6][..]));
        assert_eq!(module.pattern(PatternId(0)).map(PatternIndex::rows), Some(64));
        assert_eq!(module.instrument(InstrumentId(1)).and_then(|instrument| instrument.sample), Some(SampleId(1)));

        assert_eq!(module.order_entry(0), Some(OrderEntry::Pattern(PatternId(0))));
        assert_eq!(module.order_entry(1), Some(OrderEntry::Marker));
        assert_eq!(module.order_entry(3), Some(OrderEntry::End));
        assert_eq!(module.order_entry(4), None, "accessors answer None rather than panicking");
    }

    #[test]
    fn the_guard_frames_of_a_forward_loop_repeat_the_loop_from_its_start() {
        // The same fixture as `starplayer_mixer::sample`'s own layout test, so the two
        // crates' agreement is visible rather than merely asserted.
        let mut builder = ModuleBuilder::new();
        builder
            .add_sample(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9], SampleSpec::one_shot("loop").with_forward_loop(4, 8))
            .expect("a valid looping sample");
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        let module = builder.build().expect("a valid module");

        assert_eq!(module.pcm(), &[/* pre-roll: */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, /* guard: */ 4, 5, 6, 7, 4, 5, 6, 7]);
    }

    #[test]
    fn a_loop_shorter_than_the_guard_wraps_repeatedly() {
        let mut builder = ModuleBuilder::new();
        builder.add_sample(&[7, 8, 9], SampleSpec::one_shot("short").with_forward_loop(1, 3)).expect("a valid sample");
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        let module = builder.build().expect("a valid module");

        assert_eq!(module.pcm(), &[/* pre-roll: */ 0, 0, 0, 0, 0, 0, 0, 0, 7, 8, 9, /* guard: */ 8, 9, 8, 9, 8, 9, 8, 9]);
    }

    #[test]
    fn the_guard_frames_of_a_non_looping_sample_are_silence() {
        let mut builder = ModuleBuilder::new();
        builder.add_sample(&[10, 20, 30], SampleSpec::one_shot("hit")).expect("a valid sample");
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        let module = builder.build().expect("a valid module");

        assert_eq!(module.pcm(), &[/* pre-roll: */ 0, 0, 0, 0, 0, 0, 0, 0, 10, 20, 30, /* guard: */ 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_ping_pong_sample_with_no_sustain_loop_stores_loop_end_frames_and_a_reflected_guard() {
        let mut builder = ModuleBuilder::new();
        let specification = SampleSpec { loop_mode: LoopMode::PingPong, loop_start: 1, loop_end: 3, ..SampleSpec::one_shot("bounce") };
        let id = builder.add_sample(&[1, 2, 3, 4], specification).expect("a valid ping-pong sample");
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        let module = builder.build().expect("a valid module");

        assert_eq!(module.sample(id).map(SampleIndex::length_frames), Some(3), "same rule as a forward loop: the tail after loop_end is never audible");
        // start=1, end=3: the reflection turns on frames 1 and 2, so the guard mirrors
        // pcm[1], pcm[2] forever — the same pattern
        // `a_ping_pong_guard_matches_the_mixers_own_arithmetic` derives independently.
        assert_eq!(module.pcm(), &[/* pre-roll: */ 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, /* guard: */ 2, 3, 2, 3, 2, 3, 2, 3]);
    }

    #[test]
    fn a_ping_pong_guard_matches_the_mixers_own_arithmetic() {
        // The same fixture built two ways: once through `ModuleBuilder::add_sample`, and
        // once through the mixer's own `append_guarded_sample`, which is the contract the
        // builder has to honour. Task E1 research point 4.
        let pcm: [i16; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

        let mut builder = ModuleBuilder::new();
        let specification = SampleSpec { loop_mode: LoopMode::PingPong, loop_start: 4, loop_end: 8, ..SampleSpec::one_shot("bounce") };
        builder.add_sample(&pcm, specification).expect("a valid ping-pong sample");
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        let module = builder.build().expect("a valid module");

        let mut mixer_blob = alloc::vec::Vec::new();
        let mixer_span = starplayer_mixer::LoopSpan::ping_pong(4, 8).expect("a valid span");
        starplayer_mixer::sample::append_guarded_sample(&mut mixer_blob, &pcm, Some(mixer_span));

        assert_eq!(module.pcm(), mixer_blob.as_slice(), "the model's guard and the mixer's own guard must agree frame for frame");
    }

    #[test]
    fn a_sustain_loop_past_the_normal_loop_stores_the_whole_sample_and_validates() {
        let mut builder = ModuleBuilder::new();
        let specification = SampleSpec {
            loop_mode: LoopMode::Forward,
            loop_start: 0,
            loop_end: 4,
            sustain_loop: Some(SustainLoop { mode: LoopMode::Forward, start: 4, end: 8 }),
            ..SampleSpec::one_shot("sustained")
        };
        let id = builder.add_sample(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9], specification).expect("a valid sustain-looping sample");
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        let module = builder.build().expect("a sustain loop past the normal loop still validates");

        let sample = module.sample(id).expect("the sample exists");
        assert_eq!(sample.length_frames(), 10, "the sustain loop's end sits past loop_end, so the whole sample is stored");
        assert_eq!(sample.sustain_loop(), Some(SustainLoop { mode: LoopMode::Forward, start: 4, end: 8 }));
        assert_eq!(module.pcm(), &[/* pre-roll: */ 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, /* guard: */ 0, 0, 0, 0, 0, 0, 0, 0], "a sustain loop's guard is silence");
    }

    #[test]
    fn add_sample_rejects_a_sustain_loop_with_start_at_or_after_end() {
        let mut builder = ModuleBuilder::new();
        let specification = SampleSpec { sustain_loop: Some(SustainLoop { mode: LoopMode::Forward, start: 4, end: 4 }), ..SampleSpec::one_shot("bad") };
        assert_eq!(builder.add_sample(&[0, 1, 2, 3, 4], specification), Err(Error::Invalid("a sustain loop needs start < end")));
    }

    #[test]
    fn add_sample_rejects_a_sustain_loop_with_a_non_looping_mode() {
        let mut builder = ModuleBuilder::new();
        let specification = SampleSpec { sustain_loop: Some(SustainLoop { mode: LoopMode::None, start: 0, end: 4 }), ..SampleSpec::one_shot("bad") };
        assert_eq!(builder.add_sample(&[0, 1, 2, 3, 4], specification), Err(Error::Invalid("a sustain loop needs a looping mode")));
    }

    #[test]
    fn add_sample_rejects_a_sustain_loop_past_the_end_of_the_sample() {
        let mut builder = ModuleBuilder::new();
        let specification = SampleSpec { sustain_loop: Some(SustainLoop { mode: LoopMode::Forward, start: 0, end: 99 }), ..SampleSpec::one_shot("bad") };
        assert_eq!(builder.add_sample(&[0, 1, 2, 3, 4], specification), Err(Error::OutOfRange));
    }

    #[test]
    fn build_rejects_a_dangling_note_sample_map_entry() {
        let mut builder = populated_builder();
        let mut instrument = InstrumentDef::from_sample("ghost", SampleId(0), U0F16::MAX);
        instrument.note_sample_map[0] = 99;
        builder.add_instrument(instrument).expect("added");
        assert_eq!(builder.build().map(|_| ()), Err(Error::OutOfRange), "sample 98 (one-based 99) does not exist");
    }

    #[test]
    fn build_accepts_a_zero_note_sample_map_entry_as_no_sample() {
        let mut builder = populated_builder();
        let instrument = InstrumentDef::from_sample("no-op", SampleId(0), U0F16::MAX);
        assert_eq!(instrument.note_sample_map[0], 0, "the default map is all zero");
        builder.add_instrument(instrument).expect("added");
        assert!(builder.build().is_ok(), "an all-zero note_sample_map names no sample anywhere");
    }

    #[test]
    fn build_rejects_a_default_channel_volume_table_of_the_wrong_length() {
        let mut builder = populated_builder();
        let mut header = ModuleHeader::new(ModuleFormat::S3m, 4);
        header.default_channel_volume = vec![U0F16::MAX; 2].into_boxed_slice();
        builder.set_header(header);
        assert_eq!(builder.build().map(|_| ()), Err(Error::Invalid("default_channel_volume must be empty or one entry per channel")));
    }

    #[test]
    fn build_accepts_an_empty_or_fully_populated_default_channel_volume_table() {
        let mut builder = populated_builder();
        let mut header = ModuleHeader::new(ModuleFormat::S3m, 4);
        header.default_channel_volume = vec![U0F16::MAX; 4].into_boxed_slice();
        builder.set_header(header);
        let module = builder.build().expect("a full-length table is valid");
        assert_eq!(module.header().channel_volume(0), Some(U0F16::MAX));
        assert_eq!(module.header().channel_volume(4), None, "past the end of the channels");
    }

    #[test]
    fn add_sample_rejects_a_loop_end_past_the_end_of_the_sample() {
        let mut builder = ModuleBuilder::new();
        let specification = SampleSpec::one_shot("bad").with_forward_loop(1, 99);
        assert_eq!(builder.add_sample(&[1, 2, 3], specification), Err(Error::OutOfRange));
        assert_eq!(builder.samples.len(), 0, "a rejected sample leaves nothing behind");
        assert_eq!(builder.pcm.len(), 0, "a rejected sample leaves nothing behind");
    }

    #[test]
    fn add_sample_rejects_a_loop_start_at_or_after_the_loop_end() {
        let mut builder = ModuleBuilder::new();
        let inverted = SampleSpec::one_shot("bad").with_forward_loop(3, 1);
        let empty = SampleSpec::one_shot("bad").with_forward_loop(2, 2);

        assert_eq!(builder.add_sample(&[1, 2, 3], inverted), Err(Error::Invalid("a looping sample needs loop_start < loop_end")));
        assert_eq!(builder.add_sample(&[1, 2, 3], empty), Err(Error::Invalid("a looping sample needs loop_start < loop_end")));
    }

    #[test]
    fn add_sample_rejects_a_zero_reference_rate() {
        let mut builder = ModuleBuilder::new();
        let specification = SampleSpec::one_shot("bad").with_reference_rate(0);
        assert_eq!(builder.add_sample(&[1, 2, 3], specification), Err(Error::Invalid("a sample needs a non-zero reference rate")));
    }

    #[test]
    fn build_rejects_a_sample_whose_pcm_offset_is_out_of_range() {
        let mut builder = populated_builder();
        builder.replace_samples_for_test(vec![SampleIndex::new(9_999, 4, SampleSpec::one_shot("stray"))]);
        assert_eq!(builder.build().map(|_| ()), Err(Error::OutOfRange));
    }

    #[test]
    fn build_rejects_a_sample_whose_guard_frames_run_past_the_end_of_the_pcm() {
        let mut builder = ModuleBuilder::new();
        builder.add_sample(&[1, 2, 3], SampleSpec::one_shot("hit")).expect("a valid sample");
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        // The body fits; the guard frames are what runs off the end.
        builder.replace_samples_for_test(vec![SampleIndex::new(0, 3 + GUARD_FRAMES as u32, SampleSpec::one_shot("hit"))]);
        assert_eq!(builder.build().map(|_| ()), Err(Error::OutOfRange));
    }

    #[test]
    fn build_rejects_a_pattern_that_overruns_the_blob() {
        let mut builder = populated_builder();
        builder.replace_patterns_for_test(vec![PatternIndex::new(0, 9_999, 64, 4)]);
        assert_eq!(builder.build().map(|_| ()), Err(Error::OutOfRange));
    }

    #[test]
    fn add_pattern_rejects_an_empty_pattern() {
        let mut builder = ModuleBuilder::new();
        assert_eq!(builder.add_pattern(&[1, 2], 0, 4), Err(Error::Invalid("a pattern needs at least one row and one channel")));
        assert_eq!(builder.add_pattern(&[1, 2], 64, 0), Err(Error::Invalid("a pattern needs at least one row and one channel")));
    }

    #[test]
    fn build_rejects_an_order_naming_a_pattern_that_does_not_exist() {
        let mut builder = populated_builder();
        builder.set_orders(&[0, 2]);
        assert_eq!(builder.build().map(|_| ()), Err(Error::OutOfRange), "the module has patterns 0 and 1 only");
    }

    #[test]
    fn build_accepts_the_two_marker_values_in_the_order_list() {
        let mut builder = populated_builder();
        builder.set_orders(&[ORDER_MARKER, ORDER_END]);
        assert!(builder.build().is_ok(), "254 and 255 are S3M's skip and end markers, not pattern numbers");
    }

    #[test]
    fn a_pattern_number_wins_over_a_marker_value_once_the_module_has_that_many_patterns() {
        let mut builder = ModuleBuilder::new();
        for _ in 0..=ORDER_MARKER {
            builder.add_pattern(&[0], 1, 1).expect("a valid pattern");
        }
        builder.set_orders(&[ORDER_MARKER]);
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
        let module = builder.build().expect("a valid module");

        assert_eq!(module.order_entry(0), Some(OrderEntry::Pattern(PatternId(ORDER_MARKER))));
    }

    #[test]
    fn build_rejects_an_instrument_naming_a_sample_that_does_not_exist() {
        let mut builder = populated_builder();
        builder.add_instrument(InstrumentDef::from_sample("ghost", SampleId(7), U0F16::MAX)).expect("added");
        assert_eq!(builder.build().map(|_| ()), Err(Error::OutOfRange));
    }

    #[test]
    fn build_rejects_a_module_with_no_header() {
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&[0], 1, 1).expect("a valid pattern");
        assert_eq!(builder.build().map(|_| ()), Err(Error::Invalid("no module header was set")));
    }

    #[test]
    fn build_rejects_a_header_with_no_channels_or_a_mismatched_pan_table() {
        let mut builder = populated_builder();
        builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 0));
        assert_eq!(builder.build().map(|_| ()), Err(Error::Invalid("a module must have at least one channel")));

        let mut builder = populated_builder();
        let mut header = ModuleHeader::new(ModuleFormat::S3m, 4);
        header.default_pan = ModuleHeader::centred_pan(2);
        builder.set_header(header);
        assert_eq!(builder.build().map(|_| ()), Err(Error::Invalid("default_pan must be empty or one entry per channel")));
    }

    #[test]
    fn an_empty_module_with_only_a_header_is_valid() {
        let mut builder = ModuleBuilder::new();
        builder.set_header(ModuleHeader::new(ModuleFormat::Mod, 4));
        let module = builder.build().expect("a header alone is enough");

        assert_eq!(module.samples().len(), 0);
        assert_eq!(module.order_entry(0), None);
        assert_eq!(module.sample_pcm(SampleId(0)), None);
        assert_eq!(module.pattern_bytes(PatternId(0)), None);
    }
}
