//! [`Module`] itself: two owned blobs, four index tables and a header, and nothing else.

use alloc::boxed::Box;
use starplayer_core::{Error, InstrumentId, SampleId};

use crate::header::ModuleHeader;
use crate::instrument::InstrumentDef;
use crate::pattern::{PatternId, PatternIndex};
use crate::sample::{LoopMode, SampleIndex};

/// Order-list value meaning "skip this position" — Scream Tracker 3's `+++` marker.
pub const ORDER_MARKER: u16 = 254;

/// Order-list value meaning "the song ends here" — Scream Tracker 3's `---`.
pub const ORDER_END: u16 = 255;

/// One position of the order list, as [`Module::order_entry`] reads it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum OrderEntry {
    /// Play this pattern.
    Pattern(PatternId),
    /// Skip this position and move on. S3M's marker.
    Marker,
    /// Stop, or loop back to the restart position.
    End,
}

/// A loaded module: everything the engine needs to play a song, as plain data.
///
/// # Offsets, not references
///
/// Two owned blobs — [`blob`](Module::blob) for the patterns' **native** bytes and
/// [`pcm`](Module::pcm) for every sample's decoded frames — plus index tables of `u32`
/// offsets into them. Architecture §6 pins this shape, and it buys four things at once:
///
/// * `Send + Sync` with no ceremony, so `Arc<Module>` hands straight to the audio thread;
/// * hashable, so a golden test can fingerprint a loaded module (M2);
/// * fuzzable — a loader either produces a valid index set or an `Err`, and
///   [`ModuleBuilder`](crate::ModuleBuilder) is the one place that decides which;
/// * mmap- and flash-friendly, since an embedded target can eventually borrow the sample
///   data rather than own it, and no pointer inside the module has to be rewritten.
///
/// # Nothing here panics
///
/// Every accessor takes an index and returns [`Option`]. The engine reads a `Module` from
/// inside `render()`, where a panic in an AudioWorklet kills audio for the page
/// permanently (architecture §8), so there is no indexing operator anywhere in this crate
/// and none in its API.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Module {
    blob: Box<[u8]>,
    pcm: Box<[i16]>,
    samples: Box<[SampleIndex]>,
    patterns: Box<[PatternIndex]>,
    orders: Box<[u16]>,
    instruments: Box<[InstrumentDef]>,
    header: ModuleHeader,
}

/// Design goal: `Arc<Module>` crosses to the audio thread, so this has to hold at compile
/// time rather than in a test that someone might delete.
const fn assert_send_sync<T: Send + Sync>() {}
const _: () = assert_send_sync::<Module>();

impl Module {
    /// Assemble a module from parts. Crate-private and **unvalidated**: only
    /// [`ModuleBuilder::build`](crate::ModuleBuilder::build) calls it, and it validates
    /// immediately afterwards.
    pub(crate) fn from_parts(
        blob: Box<[u8]>,
        pcm: Box<[i16]>,
        samples: Box<[SampleIndex]>,
        patterns: Box<[PatternIndex]>,
        orders: Box<[u16]>,
        instruments: Box<[InstrumentDef]>,
        header: ModuleHeader,
    ) -> Module {
        Module { blob, pcm, samples, patterns, orders, instruments, header }
    }

    /// The song header.
    pub const fn header(&self) -> &ModuleHeader { &self.header }

    /// Every pattern's native bytes, concatenated. Only a format crate may interpret
    /// these; see [`PatternIndex`].
    pub const fn blob(&self) -> &[u8] { &self.blob }

    /// Every sample's frames, concatenated, each followed by its
    /// [`GUARD_FRAMES`](starplayer_core::GUARD_FRAMES).
    ///
    /// This is the slice the mixer resolves a `SampleRegion` against.
    pub const fn pcm(&self) -> &[i16] { &self.pcm }

    /// The sample table.
    pub const fn samples(&self) -> &[SampleIndex] { &self.samples }

    /// One sample's index entry, or `None` if `id` names no sample.
    pub fn sample(&self, id: SampleId) -> Option<&SampleIndex> { self.samples.get(id.0 as usize) }

    /// One sample's frames **including its guard frames**, or `None` if `id` names no
    /// sample or the module's index set does not fit its blob.
    ///
    /// The guard frames are included because that is what an interpolator needs to read;
    /// [`SampleIndex::length_frames`] says how much of the slice is addressable.
    pub fn sample_pcm(&self, id: SampleId) -> Option<&[i16]> {
        let sample = self.sample(id)?;
        let start = sample.pcm_offset() as usize;
        let end = start.checked_add(sample.stored_frames())?;
        self.pcm.get(start..end)
    }

    /// The pattern table.
    pub const fn patterns(&self) -> &[PatternIndex] { &self.patterns }

    /// One pattern's index entry, or `None` if `id` names no pattern.
    pub fn pattern(&self, id: PatternId) -> Option<&PatternIndex> { self.patterns.get(id.0 as usize) }

    /// One pattern's native bytes, or `None` if `id` names no pattern.
    pub fn pattern_bytes(&self, id: PatternId) -> Option<&[u8]> {
        let pattern = self.pattern(id)?;
        let start = pattern.blob_offset() as usize;
        let end = start.checked_add(pattern.length_bytes() as usize)?;
        self.blob.get(start..end)
    }

    /// The order list, as the file spells it — [`ORDER_MARKER`] and [`ORDER_END`]
    /// included. [`Module::order_entry`] is the interpreted view.
    pub const fn orders(&self) -> &[u16] { &self.orders }

    /// One order-list position, interpreted, or `None` past the end of the list.
    pub fn order_entry(&self, position: usize) -> Option<OrderEntry> {
        let order = *self.orders.get(position)?;
        if (order as usize) < self.patterns.len() {
            return Some(OrderEntry::Pattern(PatternId(order)));
        }
        match order {
            ORDER_MARKER => Some(OrderEntry::Marker),
            ORDER_END => Some(OrderEntry::End),
            // Unreachable for a built module: the builder rejects any other out-of-range
            // order. Treated as the end of the song rather than as a panic, because this
            // is read from the audio thread.
            _ => Some(OrderEntry::End),
        }
    }

    /// The instrument table.
    pub const fn instruments(&self) -> &[InstrumentDef] { &self.instruments }

    /// One instrument, or `None` if `id` names no instrument.
    pub fn instrument(&self, id: InstrumentId) -> Option<&InstrumentDef> { self.instruments.get(id.0 as usize) }

    /// Check every invariant the model promises. Called by
    /// [`ModuleBuilder::build`](crate::ModuleBuilder::build), which is the only way to
    /// obtain a `Module`, so a `Module` that exists has already passed this.
    ///
    /// The list is the fuzz-resistance contract:
    ///
    /// 1. the header names at least one channel, and its pan table is either empty or
    ///    exactly `channel_count` long;
    /// 2. every sample's frames *and* its guard frames fit inside `pcm`;
    /// 3. a looping sample has `loop_start < loop_end <= length_frames`, and a forward
    ///    loop's `length_frames` equals its `loop_end` — the guard-frame layout;
    /// 4. every pattern's `blob_offset + length_bytes` fits inside `blob`, and it has at
    ///    least one row and one channel;
    /// 5. every instrument's sample reference names a sample that exists;
    /// 6. every order names a pattern that exists, or is [`ORDER_MARKER`] /
    ///    [`ORDER_END`].
    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.header.channel_count == 0 {
            return Err(Error::Invalid("a module must have at least one channel"));
        }
        let pan_length = self.header.default_pan.len();
        if pan_length != 0 && pan_length != self.header.channel_count as usize {
            return Err(Error::Invalid("default_pan must be empty or one entry per channel"));
        }

        for sample in self.samples.iter() {
            validate_sample(sample, self.pcm.len())?;
        }
        for pattern in self.patterns.iter() {
            validate_pattern(pattern, self.blob.len())?;
        }
        for instrument in self.instruments.iter() {
            if let Some(SampleId(sample)) = instrument.sample
                && (sample as usize) >= self.samples.len()
            {
                return Err(Error::OutOfRange);
            }
        }
        for order in self.orders.iter() {
            let names_a_pattern = (*order as usize) < self.patterns.len();
            if !names_a_pattern && *order != ORDER_MARKER && *order != ORDER_END {
                return Err(Error::OutOfRange);
            }
        }
        Ok(())
    }
}

fn validate_sample(sample: &SampleIndex, pcm_length: usize) -> Result<(), Error> {
    let start = sample.pcm_offset() as usize;
    let end = start.checked_add(sample.stored_frames()).ok_or(Error::OutOfRange)?;
    if end > pcm_length {
        return Err(Error::OutOfRange);
    }

    if sample.loop_mode().is_looping() {
        if sample.loop_start() >= sample.loop_end() {
            return Err(Error::Invalid("a looping sample needs loop_start < loop_end"));
        }
        if sample.loop_end() > sample.length_frames() {
            return Err(Error::Invalid("loop_end is past the end of the sample"));
        }
    }
    if sample.loop_mode() == LoopMode::Forward && sample.loop_end() != sample.length_frames() {
        return Err(Error::Invalid("a forward loop stores exactly loop_end frames, so the guard frames can hold the loop"));
    }
    if sample.length_frames() == 0 && sample.loop_mode().is_looping() {
        return Err(Error::Invalid("an empty sample cannot loop"));
    }
    Ok(())
}

fn validate_pattern(pattern: &PatternIndex, blob_length: usize) -> Result<(), Error> {
    if pattern.rows() == 0 || pattern.channels() == 0 {
        return Err(Error::Invalid("a pattern needs at least one row and one channel"));
    }
    let start = pattern.blob_offset() as usize;
    let end = start.checked_add(pattern.length_bytes() as usize).ok_or(Error::OutOfRange)?;
    if end > blob_length {
        return Err(Error::OutOfRange);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::ModuleBuilder;
    use crate::header::{ModuleFormat, ModuleHeader};
    use crate::sample::SampleSpec;
    use core::hash::{Hash, Hasher};

    /// FNV-1a, because `std::hash::DefaultHasher` does not exist in a `no_std` crate and
    /// this only has to be deterministic within a single test run.
    #[derive(Default)]
    struct TestHasher {
        state: u64,
    }

    impl Hasher for TestHasher {
        fn finish(&self) -> u64 { self.state }

        fn write(&mut self, bytes: &[u8]) {
            let mut state = match self.state {
                0 => 0xcbf2_9ce4_8422_2325,
                seeded => seeded,
            };
            for byte in bytes {
                state ^= *byte as u64;
                state = state.wrapping_mul(0x0000_0100_0000_01b3);
            }
            self.state = state;
        }
    }

    fn hash_of(module: &Module) -> u64 {
        let mut hasher = TestHasher::default();
        module.hash(&mut hasher);
        hasher.finish()
    }

    fn module_with_title(title: &str) -> Module {
        let mut builder = ModuleBuilder::new();
        builder.add_sample(&[1, 2, 3, 4], SampleSpec::one_shot("hit")).expect("a valid sample");
        builder.add_pattern(&[9, 9], 64, 4).expect("a valid pattern");
        builder.set_orders(&[0, ORDER_END]);
        let mut header = ModuleHeader::new(ModuleFormat::S3m, 4);
        header.title = alloc::string::String::from(title).into_boxed_str();
        builder.set_header(header);
        builder.build().expect("a valid module")
    }

    #[test]
    fn a_module_is_send_and_sync_so_an_arc_of_one_reaches_the_audio_thread() {
        // The real check is the `const _: () = assert_send_sync::<Module>()` above; this
        // is the same statement in a place a reader will look for it.
        assert_send_sync::<Module>();
    }

    #[test]
    fn hashing_the_same_module_twice_gives_the_same_hash() {
        let module = module_with_title("goldens need this");
        assert_eq!(hash_of(&module), hash_of(&module));
        assert_eq!(hash_of(&module), hash_of(&module.clone()));
    }

    #[test]
    fn modules_that_differ_hash_differently() {
        assert_ne!(hash_of(&module_with_title("one")), hash_of(&module_with_title("another")));
    }

    #[test]
    fn a_module_compares_equal_to_its_clone() {
        let module = module_with_title("equal");
        assert_eq!(module, module.clone());
        assert_ne!(module, module_with_title("different"));
    }

    #[test]
    fn an_empty_pan_table_centres_every_channel_in_range() {
        let header = ModuleHeader::new(ModuleFormat::S3m, 2);
        assert_eq!(header.channel_pan(0), Some(starplayer_core::I1F15::ZERO));
        assert_eq!(header.channel_pan(1), Some(starplayer_core::I1F15::ZERO));
        assert_eq!(header.channel_pan(2), None);
    }
}
