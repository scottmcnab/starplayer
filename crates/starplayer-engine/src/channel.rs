//! [`Channel`] and [`ChannelTable`] — the binding between a control lane and the voice it
//! is currently sounding (architecture §5).
//!
//! A **channel** is a logical control lane: a tracker pattern column, or a MIDI channel.
//! A **voice** is one sounding sample drawn from the global pool. They are deliberately
//! not the same thing, because IT lets a channel keep sounding *several* voices at once.

use alloc::boxed::Box;
use alloc::vec;

use starplayer_core::{ChannelId, VoiceId, VoiceParams};
use starplayer_mixer::{SampleRegion, VoicePool, VoiceTag};

/// One control lane.
///
/// # Foreground only, for now
///
/// Architecture §5.1: in IT a channel has one *foreground* voice that receives channel
/// effect updates, plus zero or more *background* voices that have been detached and only
/// run their own envelopes and fadeout until they die. Background voices are **not** owned
/// by the channel — they live in the pool, tagged so Duplicate Check can find them — so
/// they need no field here, and MOD/S3M/MTM simply never create one.
///
/// The cost of anticipating IT is therefore this doc comment and nothing else. Effect
/// memories live in the format crate's own per-channel state (B4's port of `ChannelData`),
/// not here, because they are format semantics.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Channel {
    /// The voice this channel is driving, if any. `None` once the voice has ended or been
    /// stolen — the handle is generational, so a stale one resolves to nothing rather than
    /// to somebody else's voice.
    pub foreground: Option<VoiceId>,
    /// Whether the host has muted this lane. A muted channel runs exactly as an audible
    /// one — its effects, its voices, its telemetry — and the engine renders its voices
    /// into a discard buffer instead of the mix. Unmuting therefore resumes mid-note, and
    /// the other channels' output is bit-identical either way.
    pub muted: bool,
}

/// A fixed-capacity array of [`Channel`]s, indexed by [`ChannelId`].
///
/// Allocated once, like everything else the audio thread touches.
#[derive(Debug)]
pub struct ChannelTable {
    channels: Box<[Channel]>,
}

/// The widest voice pool any in-scope source arrangement asks for: IT's virtual-channel
/// limit, plus one foreground voice for each MIDI channel jammed over it.
///
/// The companion to [`ChannelTable::MAX_CHANNELS`], and the number a **persistent host**
/// uses — one that builds its engine before it has seen a module, and so cannot ask the
/// module's processor for [`TrackerProcessor::recommended_voice_capacity`]. A pool is
/// allocated once and the mixer walks active slots only, so sizing it at the maximum costs
/// memory and nothing else: no rendered sample changes.
///
/// A module-specific host — the offline renderer, a scan — sizes its pool from the
/// processor instead, and gets a pool exactly as wide as that format can fill.
pub const MAX_VOICE_CAPACITY: usize = 256 + 16;

impl ChannelTable {
    /// The widest tracker channel count in any format this engine targets. IT allows 64
    /// pattern channels; S3M allows 32. The voice-side companion is
    /// [`MAX_VOICE_CAPACITY`]: IT's 64 channels can each hold several sounding voices at
    /// once, which is why the two numbers are not the same.
    pub const MAX_CHANNELS: usize = 64;

    /// A table of `count` silent, unmuted channels, clamped to
    /// [`ChannelTable::MAX_CHANNELS`].
    pub fn new(count: usize) -> ChannelTable {
        ChannelTable { channels: vec![Channel::default(); count.min(ChannelTable::MAX_CHANNELS)].into_boxed_slice() }
    }

    /// How many lanes there are.
    pub fn len(&self) -> usize { self.channels.len() }

    /// Whether there are no lanes at all.
    pub fn is_empty(&self) -> bool { self.channels.is_empty() }

    /// One lane, or `None` if the index is past the end.
    pub fn get(&self, channel: ChannelId) -> Option<&Channel> { self.channels.get(channel.0 as usize) }

    /// One lane, mutably.
    pub fn get_mut(&mut self, channel: ChannelId) -> Option<&mut Channel> { self.channels.get_mut(channel.0 as usize) }

    /// Every lane, in order.
    pub fn iter(&self) -> impl Iterator<Item = (ChannelId, &Channel)> {
        self.channels.iter().enumerate().map(|(index, channel)| (ChannelId(index as u16), channel))
    }

    /// The voice `channel` is currently driving, if it is still live.
    pub fn foreground(&self, channel: ChannelId) -> Option<VoiceId> {
        self.get(channel).and_then(|lane| lane.foreground)
    }

    /// Whether `channel` still has a sounding voice.
    ///
    /// This is the original's `_ActiveFlag` (`STARPLAY/S3MLIB.INC`), and it is **not**
    /// informational: the tick-0 tone-portamento decision reads it, so a `Gxx` onto a
    /// channel whose one-shot sample has already finished has to behave as a fresh
    /// trigger. Asking the pool rather than trusting a cached flag is what makes that
    /// correct by construction.
    pub fn is_sounding(&self, channel: ChannelId, voices: &VoicePool) -> bool {
        self.foreground(channel).is_some_and(|voice| voices.get(voice).is_some())
    }

    /// Start a voice on `channel`, replacing whatever it was sounding.
    ///
    /// Returns the new voice, or `None` if the channel does not exist or the pool is
    /// full. A muted channel still starts its voice — muting happens in the mixer, so the
    /// channel's state is what it would have been. A full pool is a **normal** outcome — it is where IT's voice-stealing
    /// heuristic gets to choose a victim in M6; until then the note is simply dropped,
    /// which is what the original does.
    ///
    /// The previous foreground voice is **released immediately** rather than flagged with
    /// [`DirtyBits::STOP`](starplayer_core::DirtyBits::STOP), because the slot it occupies
    /// is needed for the replacement *now*: flagging it would return it to the pool only
    /// at the next accumulation pass, and a channel retriggering every row would need two
    /// slots to play one note. Stopping without replacing is [`ChannelTable::stop`], which
    /// does use the flag.
    ///
    /// `tag.channel` is overwritten with `channel`, so a caller cannot accidentally
    /// mis-tag a voice and break IT's Duplicate Check later.
    pub fn trigger(
        &mut self,
        channel: ChannelId,
        voices: &mut VoicePool,
        tag: VoiceTag,
        region: SampleRegion,
        params: VoiceParams,
        offset_frames: u32,
    ) -> Option<VoiceId> {
        let lane = self.channels.get_mut(channel.0 as usize)?;
        if let Some(previous) = lane.foreground.take() {
            voices.release(previous);
        }

        let tag = VoiceTag { channel: channel.0 as u8, ..tag };
        let voice = voices.allocate(tag, region, params, offset_frames)?;
        lane.foreground = Some(voice);
        Some(voice)
    }

    /// Unbind `channel`'s foreground voice **without touching it**: it keeps sounding, it
    /// keeps its tag — `tag.channel` included, so Duplicate Check can still find it — and
    /// it now belongs to nobody. Returns the voice that was detached, if there was one.
    ///
    /// IT's New Note Actions `Continue`, `NoteOff` and `NoteFade` all start here: the note
    /// already sounding becomes a *background* voice which runs on under its own envelopes
    /// and fadeout until it dies, while the channel goes on to trigger the new note.
    ///
    /// # Ordering
    ///
    /// **Detach first, then [`ChannelTable::trigger`].** `trigger` releases whatever
    /// foreground it finds before allocating the replacement, so triggering first would
    /// cut the very voice the NNA meant to keep. The detached voice is now owned by
    /// nobody, so whoever detached it is responsible for ending it — a fadeout that
    /// reaches zero, an explicit [`VoicePool::release`], or the voice running off the end
    /// of a one-shot sample.
    pub fn detach_foreground(&mut self, channel: ChannelId) -> Option<VoiceId> {
        self.channels.get_mut(channel.0 as usize)?.foreground.take()
    }

    /// Ask `channel`'s voice to stop and unbind it.
    ///
    /// The voice is flagged with `DirtyBits::STOP` — the original's `_CHN_StopVoice` — and
    /// the pool reclaims it at the next accumulation pass, which is the segment boundary
    /// the stop was scheduled for. Returns whether there was a live voice to stop.
    pub fn stop(&mut self, channel: ChannelId, voices: &mut VoicePool) -> bool {
        let Some(lane) = self.channels.get_mut(channel.0 as usize) else { return false };
        let Some(voice) = lane.foreground.take() else { return false };
        let Some(voice) = voices.get_mut(voice) else { return false };
        voice.stop();
        true
    }

    /// Stop every channel. What `AllSoundOff`, a module unload and a transport stop want.
    pub fn stop_all(&mut self, voices: &mut VoicePool) {
        for index in 0..self.channels.len() {
            self.stop(ChannelId(index as u16), voices);
        }
    }

    /// Forget every binding without touching the pool — for after
    /// [`VoicePool::release_all`], when the handles are all stale by construction. Mute
    /// flags are the host's and survive.
    pub fn forget_all(&mut self) {
        for lane in self.channels.iter_mut() {
            lane.foreground = None;
        }
    }

    /// Forget any voice handle that has gone stale, so [`ChannelTable::is_sounding`] and
    /// the telemetry view agree with the pool.
    pub fn release_finished(&mut self, voices: &VoicePool) {
        for lane in self.channels.iter_mut() {
            if lane.foreground.is_some_and(|voice| voices.get(voice).is_none()) {
                lane.foreground = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use starplayer_core::{Step, U0F16};
    use starplayer_mixer::{LoopSpan, append_guarded_sample};

    fn sounding() -> VoiceParams { VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT } }

    fn looping_blob() -> (Vec<i16>, SampleRegion) {
        let mut blob = Vec::new();
        let pcm: Vec<i16> = (0..32).map(|index| 100 + index as i16).collect();
        let region = append_guarded_sample(&mut blob, &pcm, LoopSpan::new(0, 32));
        (blob, region)
    }

    #[test]
    fn a_trigger_binds_a_voice_and_tags_it_with_its_channel() {
        let (_blob, region) = looping_blob();
        let mut channels = ChannelTable::new(4);
        let mut voices = VoicePool::new(8);

        let tag = VoiceTag { channel: 99, instrument: 3, sample: 3, note: 60 };
        let voice = channels.trigger(ChannelId(2), &mut voices, tag, region, sounding(), 0).expect("a fresh pool has room");

        assert_eq!(channels.foreground(ChannelId(2)), Some(voice));
        assert_eq!(voices.get(voice).map(|voice| voice.tag.channel), Some(2), "the tag is corrected to the real channel");
        assert_eq!(voices.get(voice).map(|voice| voice.tag.instrument), Some(3), "and the rest of the tag is kept");
        assert!(channels.is_sounding(ChannelId(2), &voices));
        assert!(!channels.is_sounding(ChannelId(0), &voices));
    }

    #[test]
    fn retriggering_a_channel_reuses_one_slot_rather_than_two() {
        let (_blob, region) = looping_blob();
        let mut channels = ChannelTable::new(1);
        let mut voices = VoicePool::new(1);

        let first = channels.trigger(ChannelId(0), &mut voices, VoiceTag::default(), region, sounding(), 0).expect("slot 0");
        let second = channels.trigger(ChannelId(0), &mut voices, VoiceTag::default(), region, sounding(), 0).expect("the same slot, freed");
        assert_eq!(voices.voices_active(), 1, "a one-voice pool is enough to retrigger a channel every row");
        assert!(voices.get(first).is_none(), "the replaced handle went stale");
        assert_eq!(channels.foreground(ChannelId(0)), Some(second));
    }

    #[test]
    fn a_muted_channel_still_starts_voices_because_muting_is_the_mixers_job() {
        let (_blob, region) = looping_blob();
        let mut channels = ChannelTable::new(2);
        let mut voices = VoicePool::new(4);
        channels.get_mut(ChannelId(1)).expect("channel 1").muted = true;

        assert!(channels.trigger(ChannelId(1), &mut voices, VoiceTag::default(), region, sounding(), 0).is_some());
        assert_eq!(voices.voices_active(), 1, "the muted channel's voice exists so unmuting can resume it mid-note");
        assert!(channels.is_sounding(ChannelId(1), &voices));
    }

    #[test]
    fn detaching_leaves_the_voice_sounding_and_unowned_but_still_tagged_with_its_channel() {
        let (blob, region) = looping_blob();
        let mut channels = ChannelTable::new(2);
        let mut voices = VoicePool::new(4);
        let tag = VoiceTag { channel: 0, instrument: 3, sample: 300, note: 60 };
        let voice = channels.trigger(ChannelId(1), &mut voices, tag, region, sounding(), 0).expect("slot 0");

        assert_eq!(channels.detach_foreground(ChannelId(1)), Some(voice));
        assert_eq!(channels.foreground(ChannelId(1)), None, "the channel owns nothing now");
        assert!(!channels.is_sounding(ChannelId(1), &voices), "and reports itself silent, which is what a fresh trigger needs");

        let detached = voices.get(voice).expect("the voice is untouched");
        assert!(!detached.wants_stop(), "detaching is not stopping: the NNA meant to keep it sounding");
        assert_eq!(detached.tag.channel, 1, "`tag.channel` survives, so Duplicate Check can still find it");
        assert_eq!(detached.tag.sample, 300, "a sample number past a byte survives the widened tag");
        assert_eq!(voices.voices_active(), 1);

        let mut destination = [starplayer_mixer::FixedFrame::default(); 8];
        voices.accumulate::<starplayer_mixer::FixedPath, starplayer_dsp::Linear>(&blob, &mut destination, 44_100);
        assert_eq!(voices.voices_active(), 1, "an accumulation pass does not reclaim it — nothing asked it to stop");

        assert_eq!(channels.detach_foreground(ChannelId(1)), None, "detaching an unowned channel is a no-op");
        assert_eq!(channels.detach_foreground(ChannelId(9)), None, "and so is detaching a channel that does not exist");
    }

    #[test]
    fn a_trigger_after_a_detach_keeps_both_voices_but_a_trigger_before_it_does_not() {
        let (_blob, region) = looping_blob();
        let mut channels = ChannelTable::new(1);
        let mut voices = VoicePool::new(4);

        let background = channels.trigger(ChannelId(0), &mut voices, VoiceTag::default(), region, sounding(), 0).expect("slot 0");
        channels.detach_foreground(ChannelId(0));
        let foreground = channels.trigger(ChannelId(0), &mut voices, VoiceTag::default(), region, sounding(), 0).expect("slot 1");
        assert_eq!(voices.voices_active(), 2, "detach then trigger is the New Note Action order: both voices sound");
        assert!(voices.get(background).is_some());
        assert_eq!(channels.foreground(ChannelId(0)), Some(foreground));

        // The other order is what the ordering rule warns about: `trigger` releases the
        // foreground it finds, so the voice the NNA meant to keep is already gone.
        let replaced = channels.trigger(ChannelId(0), &mut voices, VoiceTag::default(), region, sounding(), 0).expect("slot 1 again");
        assert!(voices.get(foreground).is_none(), "trigger released it before allocating the replacement");
        assert_eq!(channels.detach_foreground(ChannelId(0)), Some(replaced), "leaving nothing but the new voice to detach");
    }

    #[test]
    fn stopping_flags_the_voice_and_unbinds_the_channel() {
        let (blob, region) = looping_blob();
        let mut channels = ChannelTable::new(2);
        let mut voices = VoicePool::new(4);
        let voice = channels.trigger(ChannelId(0), &mut voices, VoiceTag::default(), region, sounding(), 0).expect("slot 0");

        assert!(channels.stop(ChannelId(0), &mut voices));
        assert_eq!(channels.foreground(ChannelId(0)), None);
        assert!(voices.get(voice).is_some_and(|voice| voice.wants_stop()), "the voice is flagged, not yet reclaimed");

        let mut destination = [starplayer_mixer::FixedFrame::default(); 8];
        voices.accumulate::<starplayer_mixer::FixedPath, starplayer_dsp::Linear>(&blob, &mut destination, 44_100);
        assert_eq!(voices.voices_active(), 0, "the pool reclaims it at the next accumulation pass");
        assert!(!channels.stop(ChannelId(0), &mut voices), "stopping a silent channel is a no-op");
    }

    #[test]
    fn a_channel_forgets_a_voice_that_ended_on_its_own() {
        let mut blob = Vec::new();
        let pcm: Vec<i16> = (0..4).map(|index| 100 + index as i16).collect();
        let region = append_guarded_sample(&mut blob, &pcm, None);

        let mut channels = ChannelTable::new(1);
        let mut voices = VoicePool::new(2);
        channels.trigger(ChannelId(0), &mut voices, VoiceTag::default(), region, sounding(), 0).expect("slot 0");

        let mut destination = [starplayer_mixer::FixedFrame::default(); 8];
        voices.accumulate::<starplayer_mixer::FixedPath, starplayer_dsp::Linear>(&blob, &mut destination, 44_100);
        assert!(!channels.is_sounding(ChannelId(0), &voices), "a finished one-shot is not sounding");

        channels.release_finished(&voices);
        assert_eq!(channels.foreground(ChannelId(0)), None);
    }

    #[test]
    fn the_table_is_bounded_and_out_of_range_lanes_are_not_an_error() {
        let mut channels = ChannelTable::new(ChannelTable::MAX_CHANNELS * 4);
        assert_eq!(channels.len(), ChannelTable::MAX_CHANNELS);
        assert!(channels.get(ChannelId(999)).is_none());
        assert!(channels.get_mut(ChannelId(999)).is_none());
        assert_eq!(channels.iter().count(), ChannelTable::MAX_CHANNELS);
        assert!(!channels.is_empty());
    }
}
