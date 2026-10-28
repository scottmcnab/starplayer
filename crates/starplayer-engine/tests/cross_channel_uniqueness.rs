#![cfg(feature = "note-overlap")]
use starplayer_core::{ChannelId, InstrumentId, Note, OriginalNote, PlaybackCapabilities, Step, TriggerIdentity, TriggerKind, U0F16, VoiceId, VoiceParams};
use starplayer_engine::{ChannelTable, PlaybackConfiguration, VoiceCapacity, VoiceLifecycle};
#[cfg(feature = "interactive-audition")]
use starplayer_engine::VoiceDomain;
use starplayer_mixer::{SampleRegion, VoicePool, VoiceTag};
use starplayer_model::{InstrumentDef, Module, ModuleBuilder, SampleSpec};

struct Harness { channels: ChannelTable, voices: VoicePool, module: Module }
impl Harness {
    fn new() -> Self {
        let mut builder = ModuleBuilder::new();
        let sample = builder.add_sample(&[1000; 256], SampleSpec::one_shot("one shot")).unwrap();
        for _ in 0..2 { builder.add_instrument(InstrumentDef::from_sample("instrument", sample, U0F16::MAX)).unwrap(); }
        builder.set_header(starplayer_model::ModuleHeader::new(starplayer_model::ModuleFormat::Mod, 4));
        let module = builder.build().unwrap();
        #[cfg(feature = "interactive-audition")]
        let mut channels = ChannelTable::try_with_layout(starplayer_engine::LaneLayout::interactive(4).unwrap()).unwrap();
        #[cfg(not(feature = "interactive-audition"))]
        let mut channels = ChannelTable::new(4);
        let capabilities = PlaybackCapabilities::NOTE_OVERLAP;
        #[cfg(feature = "interactive-audition")]
        let capabilities = capabilities | PlaybackCapabilities::INTERACTIVE_AUDITION;
        let configuration = PlaybackConfiguration { identity: Default::default(), capabilities, instrument_count: 2, capacity: VoiceCapacity { native: 4, custom: 4, live: if cfg!(feature = "interactive-audition") { 2 } else { 0 }, interactive_transport: cfg!(feature = "interactive-audition") } };
        channels.prepare_playback(configuration).unwrap();
        let state = channels.playback_mut().unwrap();
        state.set_note_overlap_enabled(Default::default(), true).unwrap();
        for instrument in [0, 1] { state.set_let_notes_ring(Default::default(), InstrumentId(instrument), true).unwrap(); }
        Self { channels, voices: VoicePool::new(configuration.capacity.physical()), module }
    }
    fn onset(&mut self, channel: u16, instrument: u16, note: OriginalNote, volume: u16) -> (VoiceId, TriggerIdentity) {
        let voice = self.channels.trigger(ChannelId(channel), &mut self.voices, VoiceTag { channel: channel as u8, instrument: instrument + 1, sample: 1, ..Default::default() }, SampleRegion::one_shot(self.module.samples()[0].pcm_offset(), 256), VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT }, 0).unwrap();
        let identity = TriggerIdentity { instrument: InstrumentId(instrument), channel: ChannelId(channel), note, onset_volume: volume };
        self.channels.playback_mut().unwrap().finalize_onset(voice, TriggerKind::NewNote, identity, &self.voices).unwrap();
        (voice, identity)
    }
    fn unique(&mut self, enabled: bool) { self.channels.playback_mut().unwrap().set_unique_notes_across_channels(Default::default(), enabled).unwrap(); }
    fn detached(&mut self, channel: u16) { self.channels.detach_custom_foreground(ChannelId(channel), &self.voices).unwrap(); }
    fn retiring(&self, voice: VoiceId) -> bool { self.channels.playback().unwrap().get(voice, &self.voices).unwrap().lifecycle == VoiceLifecycle::Retiring }
    fn replace(&mut self, voice: VoiceId) { self.channels.replace_note_overlap_duplicates(voice, &self.module, &mut self.voices); }
}

#[test]
fn cross_channel_matches_exact_identity_only_and_toggling_does_not_scan() {
    for (instrument, note, volume, matches) in [(0, OriginalNote::ModPeriod(427), 37, true), (1, OriginalNote::ModPeriod(427), 37, false), (0, OriginalNote::ModPeriod(428), 37, false), (0, OriginalNote::Note(Note::new(60)), 37, false), (0, OriginalNote::ModPeriod(427), 38, false)] {
        let mut harness = Harness::new();
        let (tail, original) = harness.onset(0, 0, OriginalNote::ModPeriod(427), 37); harness.detached(0);
        let (incoming, identity) = harness.onset(1, instrument, note, volume);
        harness.replace(incoming); assert!(!harness.retiring(tail), "default is off");
        harness.unique(true); assert!(!harness.retiring(tail), "settings never scan");
        let helper = harness.channels.playback().unwrap().duplicate_candidates(incoming, identity, &harness.voices).any(|voice| voice == tail);
        assert_eq!(helper, matches); harness.replace(incoming); assert_eq!(harness.retiring(tail), matches);
        assert_eq!(harness.channels.playback().unwrap().get(tail, &harness.voices).unwrap().identity(), Some(original), "original channel never changes");
    }
}

#[test]
fn only_custom_detached_backgrounds_are_cross_channel_candidates_and_same_channel_stays_native() {
    for kind in 0..3 {
        let mut harness = Harness::new(); harness.unique(true);
        let (candidate, _) = harness.onset(0, 0, OriginalNote::Note(Note::new(60)), 64);
        match kind {
            1 => { harness.channels.playback_mut().unwrap().set_lifecycle(candidate, VoiceLifecycle::Background, &harness.voices).unwrap(); }
            2 => harness.detached(0),
            _ => {}
        }
        let (incoming, identity) = harness.onset(1, 0, OriginalNote::Note(Note::new(60)), 64);
        assert_eq!(harness.channels.playback().unwrap().duplicate_candidates(incoming, identity, &harness.voices).count(), usize::from(kind == 2));
        harness.replace(incoming); assert_eq!(harness.retiring(candidate), kind == 2);
    }
    let mut harness = Harness::new();
    let (candidate, _) = harness.onset(0, 0, OriginalNote::Note(Note::new(60)), 64); harness.detached(0);
    let (incoming, _) = harness.onset(0, 0, OriginalNote::Note(Note::new(60)), 64);
    harness.replace(incoming); assert!(harness.retiring(candidate), "same-channel behavior does not require uniqueness");
}

#[cfg(feature = "interactive-audition")]
#[test]
fn live_onsets_and_live_tails_are_excluded_in_both_cross_channel_directions() {
    for (tail_channel, incoming_channel) in [(0, 64), (64, 0), (64, 65)] {
        let mut harness = Harness::new(); harness.unique(true);
        let (tail, _) = harness.onset(tail_channel, 0, OriginalNote::Note(Note::new(60)), 64); harness.detached(tail_channel);
        let (incoming, identity) = harness.onset(incoming_channel, 0, OriginalNote::Note(Note::new(60)), 64);
        assert_eq!(harness.channels.playback().unwrap().duplicate_candidates(incoming, identity, &harness.voices).count(), 0);
        harness.replace(incoming); assert!(!harness.retiring(tail));
        assert_eq!(harness.channels.playback().unwrap().get(tail, &harness.voices).unwrap().domain, if tail_channel >= 64 { VoiceDomain::Live } else { VoiceDomain::Custom });
    }
}
