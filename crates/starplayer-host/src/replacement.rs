//! Control-thread codec for replacing a web renderer across independent WASM memories.
//! Versioned integer words preserve u64 clocks and rounded transport ramp state exactly.
use super::*;
use starplayer::core::{InstrumentId, LiveDestination, RowArrival, RowClock, SequencerCursor};

const VERSION: u64 = 1;

impl Player {
    /// Capture a coherent, sound-free handoff. No held keys or processor objects cross.
    pub fn replacement_state(&mut self) -> Result<Vec<u64>, HostError> {
        if !self.interactive_enabled || self.smf.is_some() || !self.commands.is_empty() { return Err(HostError::InvalidInteractiveCommand); }
        let saved = *self.telemetry.read();
        let identity = self.playback_identity.ok_or(HostError::InvalidInteractiveCommand)?;
        if saved.applied.identity != Some(identity) || !saved.applied.accepted { return Err(HostError::InvalidInteractiveCommand); }
        let mut cursor = saved.engine.transport.sequencer_cursor();
        if saved.state == InteractiveTransport::Playing && let Some(cursor) = cursor.as_mut() {
            let elapsed = cursor.captured_frame.frames_until(saved.source_frame).min(cursor.remaining_frames);
            cursor.remaining_frames = cursor.remaining_frames.saturating_sub(elapsed);
            cursor.song_frame = cursor.song_frame.saturating_add(elapsed);
        }
        let destination = match self.live_destination { LiveDestination::Off => 0, LiveDestination::Jam => 1, LiveDestination::Selected(instrument) => instrument.0 as u64 + 2 };
        let mut words = vec![VERSION, identity.session.0, identity.build.0, self.spec.sample_rate_hz as u64, match saved.state { InteractiveTransport::Stopped => 0, InteractiveTransport::Playing => 1, InteractiveTransport::Paused => 2 }, saved.natural_end as u64, self.overlap_shadow as u64, destination, cursor.is_some() as u64];
        words.extend(saved.transport.replacement_words());
        let cursor = cursor.unwrap_or_default();
        words.extend([
            cursor.order as u64, cursor.row as u64, cursor.row_clock.speed as u64, cursor.row_clock.pattern_delay as u64, cursor.row_clock.tick_in_row as u64, cursor.row_clock.repeat_index as u64,
            cursor.sounding_order as u64, cursor.sounding_pattern as u64, cursor.sounding_row as u64, cursor.sounding_tick as u64, cursor.tempo_bpm as u64, cursor.requested_speed as u64,
            match cursor.arrival { RowArrival::Start => 0, RowArrival::Sequential => 1, RowArrival::NextOrder => 2, RowArrival::Jump => 3, RowArrival::PatternLoop => 4, RowArrival::Wrapped => 5 },
            cursor.detector_state.0 as u64, cursor.detector_state.1 as u64, cursor.detector_state.2 as u64,
            cursor.pending_jump.is_some() as u64, cursor.pending_jump.and_then(|jump| jump.0).map_or(u64::MAX, u64::from), cursor.pending_jump.and_then(|jump| jump.1).map_or(u64::MAX, u64::from), cursor.pending_jump.is_some_and(|jump| jump.2) as u64,
            cursor.remaining_frames, cursor.fractional_remainder, cursor.song_frame, cursor.captured_frame.0, cursor.sample_rate_hz as u64, cursor.sequence_ended as u64, cursor.end_reached as u64,
            self.instrument_shadow.len() as u64,
        ]);
        words.extend(self.instrument_shadow.iter().map(|selected| *selected as u64));
        words.push(self.unique_notes_shadow as u64);
        Ok(words)
    }

    /// Install prepared PCM into a new build of the same session, then replay everything
    /// before the caller queues its applied barrier. Parsing/preparation is never RT.
    pub fn load_replacement(&mut self, module: RtArc<Module>, identity: PlaybackIdentity, words: &[u64]) -> Result<(), HostError> {
        self.load_replacement_inner(module, identity, words, false)
    }

    /// Browser installation holds sequencing until its separately replayed settings
    /// have reached audio. The saved cursor and transport ramp are preserved verbatim.
    pub fn load_replacement_deferred(&mut self, module: RtArc<Module>, identity: PlaybackIdentity, words: &[u64]) -> Result<(), HostError> {
        self.load_replacement_inner(module, identity, words, true)
    }

    fn load_replacement_inner(&mut self, module: RtArc<Module>, identity: PlaybackIdentity, words: &[u64], defer_transport: bool) -> Result<(), HostError> {
        let invalid = || HostError::InvalidInteractiveCommand;
        if !self.interactive_enabled || self.commands.capacity().saturating_sub(self.commands.len()) < 4 { return Err(HostError::ControlQueueFull); }
        if self.playback_identity.is_none_or(|old| old.build.0 >= identity.build.0) { return Err(invalid()); }
        if self.module.is_some() {
            let applied = self.applied_playback();
            if !self.commands.is_empty() || words.len() < 3 || self.playback_identity != Some(PlaybackIdentity { session: starplayer::core::SessionId(words[1]), build: starplayer::core::BuildGeneration(words[2]) }) || applied.identity != self.playback_identity || !applied.accepted { return Err(invalid()); }
        }
        if words.len() < 48 { return Err(invalid()); }
        let overlap_capable = self.engine_settings.playback_configuration().is_some_and(|configuration| configuration.capabilities.contains(starplayer::core::PlaybackCapabilities::NOTE_OVERLAP));
        let selection_count = if overlap_capable { module.instruments().len() } else { 0 };
        if words[0] != VERSION || words[1] != identity.session.0 || words[2] >= identity.build.0
            || words[3] == 0 || words[3] > u32::MAX as u64 || words[4] > 2 || words[5] > 1 || words[6] > 1 || words[8] > 1
            || words[46] != selection_count as u64 || words.len() != 48 + selection_count || words[47..].iter().any(|word| *word > 1)
            || ((words[6] != 0 || *words.last().unwrap_or(&0) != 0) && !overlap_capable) { return Err(invalid()); }
        let transport = Transport::from_replacement_words(&words[9..19]).ok_or_else(invalid)?.converted(words[3] as u32, self.spec.sample_rate_hz);
        let cursor = &words[19..46];
        // Narrow fields and enum discriminants are checked before conversion.
        for index in [0, 1, 4, 6, 7, 8, 9, 10] { if cursor[index] > u16::MAX as u64 { return Err(invalid()); } }
        for index in [2, 3, 5, 11] { if cursor[index] > u8::MAX as u64 { return Err(invalid()); } }
        for index in [13, 15, 16, 19, 25, 26] { if cursor[index] > 1 { return Err(invalid()); } }
        if cursor[14] > u32::MAX as u64 || cursor[21] > u32::MAX as u64 || cursor[24] > u32::MAX as u64 { return Err(invalid()); }
        let arrival = match cursor[12] { 0 => RowArrival::Start, 1 => RowArrival::Sequential, 2 => RowArrival::NextOrder, 3 => RowArrival::Jump, 4 => RowArrival::PatternLoop, 5 => RowArrival::Wrapped, _ => return Err(invalid()) };
        let optional = |word| if word == u64::MAX { Ok(None) } else { u16::try_from(word).map(Some).map_err(|_| invalid()) };
        let jump_order = optional(cursor[17])?;
        let jump_row = optional(cursor[18])?;
        let destination = match words[7] { 0 => LiveDestination::Off, 1 => LiveDestination::Jam, value if value - 2 < module.instruments().len() as u64 => LiveDestination::Selected(InstrumentId((value - 2) as u16)), _ => return Err(invalid()) };
        if let LiveDestination::Selected(instrument) = destination {
            #[cfg(feature = "interactive-audition")]
            if !starplayer::live_instrument_capability(&module, instrument).playable { return Err(invalid()); }
            #[cfg(not(feature = "interactive-audition"))]
            { let _ = instrument; return Err(invalid()); }
        }
        let selections: Box<[bool]> = words[47..47 + selection_count].iter().map(|word| *word != 0).collect();
        if words[8] != 0 {
            let total_ticks = cursor[2] * (1 + cursor[3]);
            let ended_boundary = cursor[25] != 0 && cursor[4] == total_ticks;
            if cursor[2] == 0 || cursor[4] > total_ticks || (cursor[4] == total_ticks && !ended_boundary) || cursor[5] != cursor[4].saturating_sub(u64::from(ended_boundary)) / cursor[2].max(1) || cursor[10] == 0 || cursor[24] == 0 { return Err(invalid()); }
            let order = module.orders().get(cursor[0] as usize).ok_or_else(invalid)?;
            let pattern = module.pattern(starplayer::model::PatternId(*order)).ok_or_else(invalid)?;
            if cursor[1] >= pattern.rows() as u64 { return Err(invalid()); }
            self.rebuild_cursor = Some(SequencerCursor {
                order: cursor[0] as u16, row: cursor[1] as u16, row_clock: RowClock { speed: cursor[2] as u8, pattern_delay: cursor[3] as u8, tick_in_row: cursor[4] as u16, repeat_index: cursor[5] as u8 },
                sounding_order: cursor[6] as u16, sounding_pattern: cursor[7] as u16, sounding_row: cursor[8] as u16, sounding_tick: cursor[9] as u16, tempo_bpm: cursor[10] as u16, requested_speed: cursor[11] as u8, arrival,
                detector_state: (cursor[13] != 0, cursor[14] as u32, cursor[15] != 0), pending_jump: (cursor[16] != 0).then_some((jump_order, jump_row, cursor[19] != 0)),
                remaining_frames: cursor[20], fractional_remainder: cursor[21], song_frame: cursor[22], captured_frame: Frame(cursor[23]), sample_rate_hz: cursor[24] as u32, sequence_ended: cursor[25] != 0, end_reached: cursor[26] != 0,
            }.converted(self.spec.sample_rate_hz));
        }
        self.hold_replacement_transport = defer_transport;
        let installation = self.install_identified(module, SeekKind::None, None, false, Some(identity));
        self.hold_replacement_transport = false;
        installation?;
        self.queue(HostCommand::RestoreSelections { identity, enabled: words[6] != 0, unique: words[47 + selection_count] != 0, selections: selections.clone() })?;
        self.instrument_shadow = selections.into_vec();
        self.overlap_shadow = words[6] != 0;
        self.unique_notes_shadow = words[47 + selection_count] != 0;
        self.interactive(InteractiveAction::SetLiveDestination(destination))?;
        let state = match words[4] { 1 => InteractiveTransport::Playing, 2 => InteractiveTransport::Paused, _ => InteractiveTransport::Stopped };
        self.deferred_replacement_transport = None;
        self.replacement_release_queued = false;
        if defer_transport {
            self.deferred_replacement_transport = Some((identity, transport, state, words[5] != 0));
            self.transport_shadow = InteractiveTransport::Stopped;
        } else {
            self.queue(HostCommand::RestoreTransport { identity, transport, state, natural_end: words[5] != 0, apply_user_intent: false, release_epoch: None })?;
            self.transport_shadow = state;
        }
        Ok(())
    }
    /// Revoke admitted browser transport before its next audio quantum. Existing
    /// commands remain in FIFO order but their old release epoch becomes a no-op.
    pub fn cancel_load_transport(&mut self, identity: PlaybackIdentity) -> Result<(), HostError> {
        if self.playback_identity != Some(identity) { return Err(HostError::InvalidInteractiveCommand); }
        self.taps.transport_release_epoch.fetch_add(1, Ordering::AcqRel);
        self.replacement_release_queued = false;
        Ok(())
    }

    /// A fresh load has no saved transport, but its pending user intent needs the
    /// same revocable release as replacement restoration. One Engine slot suffices.
    pub fn finish_load_transport(&mut self, identity: PlaybackIdentity, action: InteractiveAction) -> Result<(), HostError> {
        if self.playback_identity != Some(identity) || !matches!(action, InteractiveAction::Play | InteractiveAction::Resume | InteractiveAction::Pause | InteractiveAction::Stop) { return Err(HostError::InvalidInteractiveCommand); }
        self.validate_interactive(action)?;
        if !self.can_admit_browser_batch(1, 0, 0) { return Err(HostError::ControlQueueFull); }
        let release_epoch = self.taps.transport_release_epoch.load(Ordering::Acquire);
        self.queue(HostCommand::BrowserTransport { command: InteractiveCommand { identity, action }, release_epoch })
    }

    /// Release only the matching deferred installation, after an applied policy barrier.
    /// An override is the newest user intent observed during installation.
    pub fn finish_replacement_transport(&mut self, identity: PlaybackIdentity, requested: Option<InteractiveTransport>) -> Result<(), HostError> {
        let (saved_identity, transport, saved_state, natural_end) = self.deferred_replacement_transport.ok_or(HostError::InvalidInteractiveCommand)?;
        if identity != saved_identity || self.playback_identity != Some(identity) || self.replacement_release_queued { return Err(HostError::InvalidInteractiveCommand); }
        // Restoring plus applying a new host intent sends one Engine action. Its
        // optional rewind uses the existing seek mailbox and needs no extra slot.
        if !self.can_admit_browser_batch(1, 0, 0) { return Err(HostError::ControlQueueFull); }
        let state = requested.unwrap_or(saved_state);
        let release_epoch = self.taps.transport_release_epoch.load(Ordering::Acquire);
        self.queue(HostCommand::RestoreTransport { identity, transport, state, natural_end, apply_user_intent: requested.is_some(), release_epoch: Some(release_epoch) })?;
        // Retain the exact saved ramp for a newer policy that revokes this release.
        self.replacement_release_queued = true;
        self.transport_shadow = state;
        Ok(())
    }
}

#[cfg(all(test, feature = "interactive-audition", feature = "note-overlap"))]
mod deferred_tests {
    use super::*;
    use crate::{ManualBackend, ManualDriver};
    use starplayer::core::{BuildGeneration, SessionId};

    fn open() -> (Player, ManualDriver) {
        let mut backend = ManualBackend::new();
        let identity = PlaybackIdentity { session: SessionId(0), build: BuildGeneration(0) };
        let settings = EngineSettings::interactive_web(identity, 31).unwrap();
        let player = Player::open_with_settings(&mut backend, None, AudioSpec::stereo(44_100), MixerMode::DEFAULT, settings).unwrap();
        (player, backend.driver().unwrap())
    }
    fn module() -> RtArc<Module> { RtArc::new(starplayer::load(include_bytes!("../../../fuzz/seeds/mod/synthetic-golden.mod")).unwrap()) }
    fn pump(player: &mut Player, driver: &ManualDriver, quanta: usize) {
        for _ in 0..quanta { driver.render(&mut [0.0; 256]); player.telemetry(); player.collect_garbage(); }
    }
    fn playing(player: &mut Player, driver: &ManualDriver) -> Vec<u64> {
        player.load_module_identified(module(), PlaybackIdentity { session: SessionId(1), build: BuildGeneration(1) }).unwrap();
        player.play().unwrap(); player.playback_barrier(1).unwrap(); pump(player, driver, 8);
        let words = player.replacement_state().unwrap();
        assert_eq!(words[4], 1);
        words
    }

    #[test]
    fn same_player_deferred_replacement_holds_saved_song_cursor_across_replay_quanta() {
        for requested in [None, Some(InteractiveTransport::Paused), Some(InteractiveTransport::Stopped)] {
            let (mut player, driver) = open();
            let mut words = playing(&mut player, &driver);
            words[9..19].copy_from_slice(&[1000, 32767, 496, 64, 123456, 1, 789, 0, 0, 0]);
            let identity = PlaybackIdentity { session: SessionId(1), build: BuildGeneration(2) };
            player.load_replacement_deferred(module(), identity, &words).unwrap();
            player.playback_barrier(2).unwrap(); pump(&mut player, &driver, 12);
            let held = player.replacement_state().unwrap();
            assert_eq!(held[4], 2, "the real audio host, not only its shadow, holds sequencing");
            assert_eq!(&held[39..42], &words[39..42], "remaining tick time, fractional clock and exact song position survive multiple quanta");
            assert_eq!(player.deferred_replacement_transport.unwrap().1.replacement_words().as_slice(), &words[9..19], "the saved gain/fade ramp is untouched while held");
            let wrong = PlaybackIdentity { session: SessionId(1), build: BuildGeneration(3) };
            assert!(player.finish_replacement_transport(wrong, None).is_err());
            player.finish_replacement_transport(identity, requested).unwrap();
            player.playback_barrier(3).unwrap(); pump(&mut player, &driver, 1);
            let released = player.replacement_state().unwrap();
            assert_eq!(released[4], match requested { Some(InteractiveTransport::Paused) => 2, Some(InteractiveTransport::Stopped) => 0, _ => 1 });
            if requested == Some(InteractiveTransport::Paused) {
                assert_eq!(&released[9..18], &words[9..18], "Pause preserves the nontrivial saved ramp and fade");
                assert_eq!(released[18], 1);
            }
            if requested.is_some() { assert_eq!(&released[39..42], &words[39..42]); }
            else { assert_eq!(released[41], words[41] + 128, "saved Playing advances only after the release quantum"); }
            assert!(player.finish_replacement_transport(identity, None).is_err(), "release is identity-scoped and one-shot");
        }
    }

    #[test]
    fn native_replacement_still_restores_playing_immediately_and_deferred_release_reserves_room() {
        let (mut player, driver) = open();
        let words = playing(&mut player, &driver);
        let identity = PlaybackIdentity { session: SessionId(1), build: BuildGeneration(2) };
        player.load_replacement(module(), identity, &words).unwrap(); player.playback_barrier(2).unwrap(); pump(&mut player, &driver, 2);
        assert_eq!(player.replacement_state().unwrap()[4], 1);
        let words = player.replacement_state().unwrap();
        let identity = PlaybackIdentity { session: SessionId(1), build: BuildGeneration(3) };
        player.load_replacement_deferred(module(), identity, &words).unwrap(); player.playback_barrier(3).unwrap(); pump(&mut player, &driver, 2);
        for _ in 0..30 { player.set_master_volume(U0F16::from_bits(1234)).unwrap(); }
        assert!(matches!(player.finish_replacement_transport(identity, None), Err(HostError::ControlQueueFull)));
        assert!(player.deferred_replacement_transport.is_some());
        pump(&mut player, &driver, 4);
        player.finish_replacement_transport(identity, None).unwrap(); player.playback_barrier(4).unwrap(); pump(&mut player, &driver, 2);
        assert_eq!(player.replacement_state().unwrap()[4], 1);
    }

    #[test]
    fn deferred_user_play_rewinds_naturally_ended_cursor_but_saved_pause_and_stop_keep_their_semantics() {
        for requested in [None, Some(InteractiveTransport::Playing), Some(InteractiveTransport::Paused), Some(InteractiveTransport::Stopped)] {
            let (mut player, driver) = open();
            let mut bytes = include_bytes!("../../../fuzz/seeds/mod/synthetic-golden.mod").to_vec();
            bytes[1100..1104].copy_from_slice(&starplayer::mod_file::ModCell { period: 0, instrument: 0, effect: 0xF, param: 0 }.to_bytes());
            let module = RtArc::new(starplayer::load(&bytes).unwrap());
            player.load_module_identified(RtArc::clone(&module), PlaybackIdentity { session: SessionId(1), build: BuildGeneration(1) }).unwrap();
            player.set_at_end(AtEnd::Stop).unwrap(); player.play().unwrap(); player.playback_barrier(1).unwrap();
            for _ in 0..1000 { pump(&mut player, &driver, 1); if player.natural_end() { break; } }
            assert!(player.natural_end()); pump(&mut player, &driver, 2);
            let saved = player.replacement_state().unwrap();
            assert_eq!(saved[17], 1, "natural end has armed the next user Play rewind");
            assert_eq!(saved[44], 1, "the saved source is genuinely sequence-ended");
            let identity = PlaybackIdentity { session: SessionId(1), build: BuildGeneration(2) };
            player.load_replacement_deferred(module, identity, &saved).unwrap(); player.playback_barrier(2).unwrap(); pump(&mut player, &driver, 12);
            assert_eq!(&player.replacement_state().unwrap()[39..42], &saved[39..42]);
            player.finish_replacement_transport(identity, requested).unwrap(); player.playback_barrier(3).unwrap(); pump(&mut player, &driver, 1);
            let restored = player.replacement_state().unwrap();
            match requested {
                Some(InteractiveTransport::Playing) => {
                    assert_eq!(player.song_frame(), 0, "pending Play/Resume consumes the saved armed rewind through the normal seek mailbox");
                    assert!(!player.natural_end()); assert_eq!(restored[17], 0); assert_eq!(restored[44], 0);
                }
                Some(InteractiveTransport::Stopped) => {
                    assert!(!player.natural_end()); assert_eq!(restored[17], 0);
                    assert_eq!(&restored[39..42], &saved[39..42], "explicit Stop retains the ended position and cancels its rewind");
                    player.play().unwrap(); pump(&mut player, &driver, 1);
                    assert!(player.song_frame() >= saved[41], "a later Play must not resurrect the cancelled rewind");
                }
                _ => {
                    assert!(player.natural_end()); assert_eq!(restored[17], 1);
                    assert_eq!(&restored[39..42], &saved[39..42]);
                    if requested.is_none() { assert_eq!(&restored[9..19], &saved[9..19], "saved-only native semantics preserve the complete transport"); }
                    player.resume().unwrap(); pump(&mut player, &driver, 1);
                    assert_eq!(player.song_frame(), 0, "Pause and saved-only restoration preserve the next Resume rewind");
                    assert!(!player.natural_end());
                }
            }
        }
    }

    #[test]
    fn revoked_browser_releases_cannot_advance_the_saved_or_fresh_source_before_new_policy() {
        for replacement in [false, true] {
            let (mut player, driver) = open();
            let mut saved = playing(&mut player, &driver);
            saved[9..19].copy_from_slice(&[1000, 32767, 496, 64, 123456, 1, 789, 0, 0, 0]);
            let identity = PlaybackIdentity { session: SessionId(if replacement { 1 } else { 2 }), build: BuildGeneration(2) };
            if replacement { player.load_replacement_deferred(module(), identity, &saved).unwrap(); }
            else { player.load_module_identified(module(), identity).unwrap(); }
            player.playback_barrier(2).unwrap(); pump(&mut player, &driver, 3);
            let held = player.replacement_state().unwrap();
            if replacement { player.finish_replacement_transport(identity, None).unwrap(); }
            else { player.finish_load_transport(identity, InteractiveAction::Resume).unwrap(); }
            player.playback_barrier(3).unwrap();
            let wrong = PlaybackIdentity { session: identity.session, build: BuildGeneration(1) };
            assert!(player.cancel_load_transport(wrong).is_err());
            player.cancel_load_transport(identity).unwrap();
            pump(&mut player, &driver, 12);
            let cancelled = player.replacement_state().unwrap();
            assert_eq!(&cancelled[39..42], &held[39..42], "revoked release cannot move sequencing in any of the following replay quanta");
            assert_ne!(cancelled[4], 1);
            player.interactive(InteractiveAction::SetNoteOverlapEnabled(true)).unwrap(); player.playback_barrier(4).unwrap(); pump(&mut player, &driver, 2);
            if replacement {
                assert_eq!(&player.deferred_replacement_transport.unwrap().1.replacement_words(), saved[9..19].as_ref());
                player.finish_replacement_transport(identity, Some(InteractiveTransport::Playing)).unwrap();
            } else { player.finish_load_transport(identity, InteractiveAction::Play).unwrap(); }
            player.playback_barrier(5).unwrap(); pump(&mut player, &driver, 1);
            let released = player.replacement_state().unwrap(); assert_eq!(released[4], 1); assert_eq!(released[6], 1);
        }
    }

}
