//! NO-01 contracts only: policies/transport execution arrive in the dependent tasks.
use starplayer_core::{ChannelId, PlaybackCapabilities};
use starplayer_engine::{ChannelTable, EngineSettings, LaneLayout, MAX_VOICE_CAPACITY, VoiceCapacity, compiled_playback_capabilities};

#[test]
fn legacy_defaults_and_feature_bits_are_independent() {
    assert_eq!(ChannelTable::MAX_CHANNELS, 64);
    assert_eq!(starplayer_engine::MIDI_CHANNEL_BASE, 48);
    assert_eq!(MAX_VOICE_CAPACITY, 272);
    assert_eq!(ChannelTable::new(81).len(), 64);
    let settings = EngineSettings::default();
    assert_eq!(settings.channel_count, 32);
    assert_eq!(settings.voice_capacity, 64);
    #[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
    assert_eq!(settings.playback, None);
    let compiled = compiled_playback_capabilities();
    assert_eq!(compiled.contains(PlaybackCapabilities::NOTE_OVERLAP), cfg!(feature = "note-overlap"));
    assert_eq!(compiled.contains(PlaybackCapabilities::INTERACTIVE_AUDITION), cfg!(feature = "interactive-audition"));
    assert_eq!(VoiceCapacity::web().active(), 336);
    assert_eq!(VoiceCapacity::web().retirement(), 3);
    assert_eq!(VoiceCapacity::web().physical(), 339);
}

#[test]
fn layouts_reject_unsupported_widths_without_wrapping() {
    let legacy = LaneLayout::legacy(64);
    assert_eq!(legacy.midi_lane(0), Some(ChannelId(48)));
    assert_eq!(legacy.midi_lane(15), Some(ChannelId(63)));
    assert_eq!(legacy.midi_lane(16), None);
    assert_eq!(legacy.preview_lane(), None);
    #[cfg(not(feature = "interactive-audition"))]
    assert_eq!(LaneLayout::interactive(4), Err(starplayer_engine::PlaybackConfigError::FeatureUnavailable));
    #[cfg(feature = "interactive-audition")]
    {
        for width in [0, 4, 48, 64] {
            let layout = LaneLayout::interactive(width).unwrap();
            assert_eq!(layout.tracker_channels(), width);
            assert_eq!(layout.lane_count(), 81);
            assert_eq!(layout.midi_lane(0), Some(ChannelId(64)));
            assert_eq!(layout.midi_lane(15), Some(ChannelId(79)));
            assert_eq!(layout.midi_lane(16), None);
            assert_eq!(layout.midi_index(ChannelId(63)), None);
            assert_eq!(layout.midi_index(ChannelId(79)), Some(15));
            assert_eq!(layout.midi_index(ChannelId(80)), None);
            assert_eq!(layout.preview_lane(), Some(ChannelId(80)));
            let table = ChannelTable::try_with_layout(layout).unwrap();
            assert_eq!(table.len(), 81);
            assert!(table.get(ChannelId(80)).is_some());
            assert!(table.get(ChannelId(81)).is_none());
        }
        for width in [65, 81, 256, usize::MAX] {
            assert_eq!(LaneLayout::interactive(width), Err(starplayer_engine::PlaybackConfigError::UnsupportedTrackerWidth));
        }
    }
}

#[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
mod optional {
    use super::*;
    use starplayer_core::{BuildGeneration, PlaybackIdentity, SessionId, Step, VoiceParams, U0F16};
    use starplayer_engine::{PlaybackConfiguration, PlaybackState, PlaybackStateError, VoiceDomain, VoiceLifecycle};
    use starplayer_mixer::{SampleRegion, VoicePool, VoiceTag};

    fn configuration() -> PlaybackConfiguration {
        let capabilities = compiled_playback_capabilities();
        PlaybackConfiguration {
            identity: PlaybackIdentity { session: SessionId(7), build: BuildGeneration(9) },
            capabilities, instrument_count: 3,
            capacity: VoiceCapacity { native: 4, custom: if cfg!(feature = "note-overlap") { 2 } else { 0 }, live: if cfg!(feature = "interactive-audition") { 2 } else { 0 }, interactive_transport: cfg!(feature = "interactive-audition") },
        }
    }

    fn allocate(pool: &mut VoicePool, channel: u8) -> starplayer_core::VoiceId {
        pool.allocate(VoiceTag { channel, ..VoiceTag::default() }, SampleRegion::one_shot(0, 64), VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT }, 0).unwrap()
    }

    #[test]
    fn bounded_ownership_is_generation_checked_and_retirement_is_domain_scoped() {
        let configuration = configuration();
        let mut state = PlaybackState::try_new(configuration).unwrap();
        let mut pool = VoicePool::new(configuration.capacity.physical());
        let first = allocate(&mut pool, 0);
        let second = allocate(&mut pool, 1);
        state.register(first, VoiceDomain::Native, ChannelId(0), &pool).unwrap();
        state.register(second, VoiceDomain::Native, ChannelId(1), &pool).unwrap();
        assert_eq!(state.oldest(VoiceDomain::Native, false, &pool), Some(first));
        let age = state.get(first, &pool).unwrap().age;
        state.set_lifecycle(first, VoiceLifecycle::Retiring, &pool).unwrap();
        assert_eq!(state.get(first, &pool).unwrap().age, age);
        assert_eq!(state.active_count(VoiceDomain::Native, &pool), 1);
        assert_eq!(state.oldest(VoiceDomain::Native, true, &pool), Some(first));
        assert_eq!(state.oldest(VoiceDomain::Live, true, &pool), None);
        assert_eq!(state.set_lifecycle(first, VoiceLifecycle::Background, &pool), Err(PlaybackStateError::InvalidOwnership));
        pool.release(first);
        let reused = allocate(&mut pool, 0);
        assert_eq!(reused.index(), first.index());
        assert_ne!(reused, first);
        assert!(state.get(first, &pool).is_none());
        assert!(state.get(reused, &pool).is_none());
        state.register(reused, VoiceDomain::Native, ChannelId(0), &pool).unwrap();
        assert!(state.get(reused, &pool).unwrap().age > age);
        assert_eq!(state.register(reused, VoiceDomain::Native, ChannelId(0), &pool), Err(PlaybackStateError::InvalidOwnership));
        assert_eq!(state.register(second, VoiceDomain::Native, ChannelId(9), &pool), Err(PlaybackStateError::InvalidOwnership));
    }

    #[test]
    fn runtime_opt_in_prepares_only_the_requested_capacity_and_rejects_invalid_builds() {
        use starplayer_dsp::Linear;
        use starplayer_engine::{Engine, PlaybackConfigError};
        use starplayer_mixer::{FixedPath, StereoI16};
        type TestEngine = Engine<FixedPath, Linear, StereoI16, ()>;
        let configuration = configuration();
        let settings = EngineSettings::default().with_playback(configuration).unwrap();
        assert_eq!(settings.voice_capacity, configuration.capacity.physical());
        let engine = TestEngine::try_with_settings(settings).unwrap();
        assert_eq!(engine.voices().capacity(), configuration.capacity.physical());
        assert!(engine.channels().playback().is_some());
        assert_eq!(engine.channels().len(), if cfg!(feature = "interactive-audition") { 81 } else { 32 });
        assert_eq!(engine.bus_count(), 32);
        assert!(matches!(TestEngine::try_with_settings(EngineSettings { voice_capacity: 1, ..settings }), Err(PlaybackConfigError::InvalidCapacity)));
        assert!(EngineSettings { channel_count: 65, ..EngineSettings::default() }.with_playback(configuration).is_err());
        let invalid = PlaybackConfiguration { capacity: VoiceCapacity { native: usize::MAX, ..configuration.capacity }, ..configuration };
        assert!(PlaybackState::try_new(invalid).is_err());
        let legacy = TestEngine::with_settings(EngineSettings::default());
        assert!(legacy.channels().playback().is_none());
    }

    #[cfg(feature = "note-overlap")]
    #[test]
    fn passive_onset_identity_survives_toggles_release_and_later_effects() {
        use starplayer_core::{InstrumentId, Note, OriginalNote, TriggerIdentity, TriggerKind};
        use starplayer_engine::ClearScope;
        let configuration = configuration();
        let mut state = PlaybackState::try_new(configuration).unwrap();
        let mut pool = VoicePool::new(configuration.capacity.physical());
        let first = allocate(&mut pool, 0);
        state.register(first, VoiceDomain::Native, ChannelId(0), &pool).unwrap();
        let identity = TriggerIdentity { instrument: InstrumentId(0), note: OriginalNote::ModPeriod(427), channel: ChannelId(0), onset_volume: 37 };
        assert!(!state.note_overlap_enabled());
        assert_eq!(state.let_notes_ring(InstrumentId(0)), Some(false));
        // This proves the seam records while unchecked AND bypassed. Formats wire their
        // post-effects call and actual replacement behavior in NO-04/05, not in NO-01.
        assert_eq!(state.finalize_onset(first, TriggerKind::NewNote, identity, &pool), Ok(true));
        state.set_let_notes_ring(configuration.identity, InstrumentId(0), true).unwrap();
        state.set_note_overlap_enabled(configuration.identity, true).unwrap();
        pool.get_mut(first).unwrap().params.set_volume(U0F16::ZERO);
        pool.get_mut(first).unwrap().params.set_step(Step::from_bits(7));
        state.set_lifecycle(first, VoiceLifecycle::Background, &pool).unwrap(); // key-off/fade/NNA release
        assert_eq!(state.get(first, &pool).unwrap().identity(), Some(identity));
        assert_eq!(state.finalize_onset(first, TriggerKind::NewNote, identity, &pool), Err(PlaybackStateError::AlreadyFinalized));
        let incoming = allocate(&mut pool, 0);
        state.register(incoming, VoiceDomain::Native, ChannelId(0), &pool).unwrap();
        assert_eq!(state.finalize_onset(incoming, TriggerKind::EffectRetrigger, identity, &pool), Ok(false));
        assert_eq!(state.finalize_onset(incoming, TriggerKind::SampleReplacement, identity, &pool), Ok(false));
        state.finalize_onset(incoming, TriggerKind::NewNote, identity, &pool).unwrap();
        assert_eq!(state.duplicate_candidates(incoming, identity, &pool).collect::<Vec<_>>(), vec![first]);
        for other in [
            TriggerIdentity { onset_volume: 38, ..identity },
            TriggerIdentity { note: OriginalNote::ModPeriod(428), ..identity },
            TriggerIdentity { note: OriginalNote::Note(Note::new(60)), ..identity },
            TriggerIdentity { channel: ChannelId(1), ..identity },
            TriggerIdentity { instrument: InstrumentId(1), ..identity },
        ] { assert_eq!(state.duplicate_candidates(incoming, other, &pool).count(), 0); }
        state.set_lifecycle(first, VoiceLifecycle::Retiring, &pool).unwrap();
        assert_eq!(state.duplicate_candidates(incoming, identity, &pool).count(), 0);
        assert_eq!(state.set_note_overlap_enabled(configuration.identity, false), Ok(Some(ClearScope::CustomDetached)));
        assert_eq!(state.let_notes_ring(InstrumentId(0)), Some(true));
        assert_eq!(state.set_note_overlap_enabled(configuration.identity, false), Ok(None));
        for stale in [
            PlaybackIdentity { session: SessionId(8), ..configuration.identity },
            PlaybackIdentity { build: BuildGeneration(10), ..configuration.identity },
        ] {
            assert_eq!(state.set_let_notes_ring(stale, InstrumentId(0), false), Err(PlaybackStateError::StaleIdentity));
            assert_eq!(state.set_note_overlap_enabled(stale, true), Err(PlaybackStateError::StaleIdentity));
        }
        assert_eq!(state.set_let_notes_ring(configuration.identity, InstrumentId(3), true), Err(PlaybackStateError::InvalidInstrument));
        assert_eq!(state.set_let_notes_ring(configuration.identity, InstrumentId(u16::MAX), true), Err(PlaybackStateError::InvalidInstrument));
        let reset = PlaybackState::try_new(PlaybackConfiguration { identity: PlaybackIdentity { session: SessionId(8), build: BuildGeneration(10) }, ..configuration }).unwrap();
        assert_eq!(reset.let_notes_ring(InstrumentId(0)), Some(false));
        assert!(!reset.note_overlap_enabled());
        pool.release(first);
        assert!(state.get(first, &pool).is_none());
    }

    #[test]
    fn shared_detach_updates_the_authoritative_lifecycle() {
        let configuration = configuration();
        let layout = if cfg!(feature = "interactive-audition") { LaneLayout::interactive(4).unwrap() } else { LaneLayout::legacy(4) };
        let mut channels = ChannelTable::try_with_layout(layout).unwrap();
        channels.prepare_playback(configuration).unwrap();
        let mut pool = VoicePool::new(configuration.capacity.physical());
        let foreground = channels.trigger(ChannelId(0), &mut pool, VoiceTag::default(), SampleRegion::one_shot(0, 64), VoiceParams::SILENT, 0).unwrap();
        assert_eq!(channels.playback().unwrap().get(foreground, &pool).unwrap().lifecycle, VoiceLifecycle::Foreground);
        channels.detach_foreground(ChannelId(0));
        let owned = channels.playback().unwrap().get(foreground, &pool).unwrap();
        assert_eq!(owned.lifecycle, VoiceLifecycle::Background);
        assert_eq!(owned.domain, VoiceDomain::Native);
        #[cfg(feature = "note-overlap")]
        {
            assert_eq!(channels.playback_mut().unwrap().mark_custom_detached(foreground, &pool), Err(PlaybackStateError::InvalidOwnership));
            let custom = channels.trigger(ChannelId(0), &mut pool, VoiceTag::default(), SampleRegion::one_shot(0, 64), VoiceParams::SILENT, 0).unwrap();
            assert_eq!(channels.detach_custom_foreground(ChannelId(0), &pool), Ok(Some(custom)));
            let owned = channels.playback().unwrap().get(custom, &pool).unwrap();
            assert_eq!(owned.lifecycle, VoiceLifecycle::Background);
            assert_eq!(owned.domain, VoiceDomain::Custom);
        }
    }

    #[cfg(feature = "note-overlap")]
    #[test]
    fn native_backgrounds_cannot_migrate_and_custom_live_tails_stay_live() {
        use starplayer_engine::ClearScope;
        let configuration = configuration();
        let mut state = PlaybackState::try_new(configuration).unwrap();
        let mut pool = VoicePool::new(configuration.capacity.physical());
        let background = allocate(&mut pool, 0);
        state.register(background, VoiceDomain::Native, ChannelId(0), &pool).unwrap();
        state.set_lifecycle(background, VoiceLifecycle::Background, &pool).unwrap();
        assert_eq!(state.mark_custom_detached(background, &pool), Err(PlaybackStateError::InvalidOwnership));
        assert!(!state.get(background, &pool).unwrap().in_scope(ClearScope::CustomDetached));
        let song = allocate(&mut pool, 1);
        state.register(song, VoiceDomain::Native, ChannelId(1), &pool).unwrap();
        state.mark_custom_detached(song, &pool).unwrap();
        assert_eq!(state.get(song, &pool).unwrap().domain, VoiceDomain::Custom);
        assert!(state.get(song, &pool).unwrap().in_scope(ClearScope::CustomDetached));
        #[cfg(feature = "interactive-audition")]
        {
            let native_live_background = allocate(&mut pool, 65);
            state.register(native_live_background, VoiceDomain::Live, ChannelId(65), &pool).unwrap();
            state.set_lifecycle(native_live_background, VoiceLifecycle::Background, &pool).unwrap();
            assert_eq!(state.mark_custom_detached(native_live_background, &pool), Err(PlaybackStateError::InvalidOwnership));
            assert!(!state.get(native_live_background, &pool).unwrap().in_scope(ClearScope::CustomDetached));
            let live = allocate(&mut pool, 64);
            state.register(live, VoiceDomain::Live, ChannelId(64), &pool).unwrap();
            state.mark_custom_detached(live, &pool).unwrap();
            assert_eq!(state.get(live, &pool).unwrap().domain, VoiceDomain::Live);
            assert!(state.get(live, &pool).unwrap().in_scope(ClearScope::CustomDetached));
            assert!(!state.get(live, &pool).unwrap().in_scope(ClearScope::Song));
            assert!(state.get(live, &pool).unwrap().in_scope(ClearScope::Lane(ChannelId(64))));
            assert_eq!(state.active_count(VoiceDomain::Live, &pool), 2);
        }
    }

    #[cfg(feature = "interactive-audition")]
    #[test]
    fn live_origins_share_one_budget_but_clear_separately_and_reject_stale_writes() {
        use starplayer_engine::{ClearScope, LiveOrigin};
        let configuration = configuration();
        let mut state = PlaybackState::try_new(configuration).unwrap();
        let mut pool = VoicePool::new(configuration.capacity.physical());
        let song = allocate(&mut pool, 0);
        let selected = allocate(&mut pool, 64);
        let button = allocate(&mut pool, 80);
        state.register(song, VoiceDomain::Native, ChannelId(0), &pool).unwrap();
        state.register(selected, VoiceDomain::Live, ChannelId(64), &pool).unwrap();
        state.register(button, VoiceDomain::Live, ChannelId(80), &pool).unwrap();
        state.set_live_origin(configuration.identity, selected, LiveOrigin::SelectedInput, &pool).unwrap();
        assert_eq!(state.active_count(VoiceDomain::Live, &pool), 2);
        assert!(state.get(selected, &pool).unwrap().in_scope(ClearScope::LiveOrigin(LiveOrigin::SelectedInput)));
        assert!(!state.get(selected, &pool).unwrap().in_scope(ClearScope::LiveOrigin(LiveOrigin::Jam)));
        assert!(state.get(button, &pool).unwrap().in_scope(ClearScope::LiveOrigin(LiveOrigin::ButtonPreview)));
        assert_eq!(state.get(song, &pool).unwrap().live_origin(), None);
        assert_eq!(state.set_live_origin(configuration.identity, song, LiveOrigin::Jam, &pool), Err(PlaybackStateError::InvalidOwnership));
        assert_eq!(state.set_live_origin(configuration.identity, selected, LiveOrigin::ButtonPreview, &pool), Err(PlaybackStateError::InvalidOwnership));
        assert_eq!(state.set_live_origin(configuration.identity, button, LiveOrigin::Jam, &pool), Err(PlaybackStateError::InvalidOwnership));
        let stale = PlaybackIdentity { build: BuildGeneration(8), ..configuration.identity };
        assert_eq!(state.set_live_origin(stale, selected, LiveOrigin::Jam, &pool), Err(PlaybackStateError::StaleIdentity));
    }

    #[cfg(feature = "note-overlap")]
    #[test]
    fn compiled_overlap_without_runtime_opt_in_allocates_no_selections_or_identity() {
        use starplayer_core::{InstrumentId, OriginalNote, TriggerIdentity, TriggerKind};
        let mut configuration = configuration();
        configuration.capabilities.remove(PlaybackCapabilities::NOTE_OVERLAP);
        configuration.capacity.custom = 0;
        let mut state = PlaybackState::try_new(configuration).unwrap();
        assert_eq!(state.let_notes_ring(InstrumentId(0)), None);
        assert_eq!(state.set_note_overlap_enabled(configuration.identity, true), Err(PlaybackStateError::Unsupported));
        let mut pool = VoicePool::new(configuration.capacity.physical());
        let voice = allocate(&mut pool, 0);
        state.register(voice, VoiceDomain::Native, ChannelId(0), &pool).unwrap();
        let identity = TriggerIdentity { instrument: InstrumentId(0), note: OriginalNote::ModPeriod(428), channel: ChannelId(0), onset_volume: 64 };
        assert_eq!(state.finalize_onset(voice, TriggerKind::NewNote, identity, &pool), Ok(false));
        assert_eq!(state.get(voice, &pool).unwrap().identity(), None);
    }

    #[cfg(feature = "interactive-audition")]
    #[test]
    fn all_interactive_live_lanes_spill_past_tracker_inserts_for_narrow_and_wide_songs() {
        use starplayer_dsp::{Linear, InsertKind, build_insert};
        use starplayer_dsp::effects::{GAIN_PARAM, GAIN_MIN_CENTI_DB};
        use starplayer_engine::{Engine, InsertTarget, RENDER_QUANTUM};
        use starplayer_mixer::{FixedPath, StereoI16, LoopSpan, append_guarded_sample};
        type TestEngine = Engine<FixedPath, Linear, StereoI16, ()>;
        for tracker_width in [4, 64] {
            for live_lane in 64..=80 {
                let mut configuration = configuration();
                configuration.capacity.live = 17;
                let settings = EngineSettings { channel_count: tracker_width, insert_command_capacity: 128, ..EngineSettings::default() }.with_playback(configuration).unwrap();
                let render = |with_inserts: bool| {
                    let mut engine = TestEngine::try_with_settings(settings).unwrap();
                    assert_eq!(engine.bus_count(), tracker_width);
                    assert_eq!(engine.channels().len(), 81);
                    let mut blob = Vec::new();
                    let region = append_guarded_sample(&mut blob, &[4096; 64], LoopSpan::new(0, 64));
                    engine.set_pcm(blob);
                    engine.voices_mut().allocate(VoiceTag { channel: live_lane, ..VoiceTag::default() }, region, VoiceParams { step: Step::ONE, volume: U0F16::MAX, ..VoiceParams::SILENT }, 0).unwrap();
                    if with_inserts {
                        let mut inserts = engine.take_insert_control().unwrap();
                        for tracker_lane in 0..tracker_width {
                            let mut gain = build_insert::<i32>(InsertKind::Gain, 44_100);
                            gain.set_param(GAIN_PARAM, GAIN_MIN_CENTI_DB);
                            gain.reset();
                            inserts.install(InsertTarget::Channel(ChannelId(tracker_lane as u16)), 0, gain).map_err(|_| "queue full").unwrap();
                        }
                    }
                    let mut output = [0i16; RENDER_QUANTUM * 2];
                    engine.render(&mut output);
                    output
                };
                let direct = render(false);
                assert!(direct.iter().any(|sample| *sample != 0));
                assert_eq!(render(true), direct, "lane {live_lane} encountered tracker inserts for width {tracker_width}");
            }
        }
    }
}
