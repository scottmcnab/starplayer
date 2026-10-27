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
        Ok(words)
    }

    /// Install prepared PCM into a new build of the same session, then replay everything
    /// before the caller queues its applied barrier. Parsing/preparation is never RT.
    pub fn load_replacement(&mut self, module: RtArc<Module>, identity: PlaybackIdentity, words: &[u64]) -> Result<(), HostError> {
        let invalid = || HostError::InvalidInteractiveCommand;
        if !self.interactive_enabled || self.commands.capacity().saturating_sub(self.commands.len()) < 4 { return Err(HostError::ControlQueueFull); }
        if self.playback_identity.is_none_or(|old| old.build.0 >= identity.build.0) { return Err(invalid()); }
        if self.module.is_some() {
            let applied = self.applied_playback();
            if !self.commands.is_empty() || words.len() < 3 || self.playback_identity != Some(PlaybackIdentity { session: starplayer::core::SessionId(words[1]), build: starplayer::core::BuildGeneration(words[2]) }) || applied.identity != self.playback_identity || !applied.accepted { return Err(invalid()); }
        }
        if words.len() < 47 { return Err(invalid()); }
        let overlap_capable = self.engine_settings.playback_configuration().is_some_and(|configuration| configuration.capabilities.contains(starplayer::core::PlaybackCapabilities::NOTE_OVERLAP));
        let selection_count = if overlap_capable { module.instruments().len() } else { 0 };
        if words[0] != VERSION || words[1] != identity.session.0 || words[2] >= identity.build.0
            || words[3] == 0 || words[3] > u32::MAX as u64 || words[4] > 2 || words[5] > 1 || words[6] > 1 || words[8] > 1
            || words[46] != selection_count as u64 || words.len() != 47 + selection_count || words[47..].iter().any(|word| *word > 1)
            || (words[6] != 0 && !overlap_capable) { return Err(invalid()); }
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
        let selections: Box<[bool]> = words[47..].iter().map(|word| *word != 0).collect();
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
        self.install_identified(module, SeekKind::None, None, false, Some(identity))?;
        self.queue(HostCommand::RestoreSelections { identity, enabled: words[6] != 0, selections: selections.clone() })?;
        self.instrument_shadow = selections.into_vec();
        self.overlap_shadow = words[6] != 0;
        self.interactive(InteractiveAction::SetLiveDestination(destination))?;
        let state = match words[4] { 1 => InteractiveTransport::Playing, 2 => InteractiveTransport::Paused, _ => InteractiveTransport::Stopped };
        self.queue(HostCommand::RestoreTransport { identity, transport, state, natural_end: words[5] != 0 })?;
        self.transport_shadow = state;
        Ok(())
    }
}
