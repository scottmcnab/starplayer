//! Loaded IT live adapter. Native note actions and articulation share tracker arithmetic;
//! resource pressure remains the engine's deterministic L policy.
use super::*;
use starplayer_engine::instrument::{NoteParams, live::{RackRuntime, admit_rack_note, preserve_rack_foreground}};
use starplayer_engine::{EngineContext, VoiceLifecycle};

pub struct ItLiveRuntime { articulation: ItArticulationRuntime, sample_rate_hz: u32 }
impl ItLiveRuntime {
    pub fn try_new(capacity: usize, sample_rate_hz: u32) -> Result<Self, alloc::collections::TryReserveError> { Ok(Self { articulation: ItArticulationRuntime::try_new(capacity)?, sample_rate_hz }) }
    fn write(&mut self, module: &Module, voice: VoiceId, trigger: bool, context: &mut EngineContext<'_>) {
        if !context.channels.playback().and_then(|state| state.get(voice, context.voices)).is_some_and(|owned| owned.lifecycle != starplayer_engine::VoiceLifecycle::Retiring) { return; }
        let instrument_mode = ItFormatExtra::from_header(module.header()).is_instrument_mode();
        let Some(output) = self.articulation.control_step(module, voice, instrument_mode, 128, self.sample_rate_hz, trigger) else { return };
        if let Some(mixer) = context.voices.get_mut(voice) {
            let mut params = output.params;
            if let Some(state) = self.articulation.voice(voice) && state.live_bend_cents != 0 { params.step = Step::from_ratio(scale_frequency(state.output_frequency_hz, cents_to_units(state.live_bend_cents as i32)) as u64, self.sample_rate_hz.max(1) as u64); }
            params.step = starplayer_engine::scaled_step(params.step, mixer.region());
            params.dirty = DirtyBits::PITCH | DirtyBits::VOLUME | DirtyBits::PAN;
            mixer.params = params;
        }
        if output.silent { context.channels.retire_voice(voice, context.voices); self.articulation.retire(voice); }
    }
}
impl RackRuntime for ItLiveRuntime {
    fn note_playable(&self, _module: &Module, _sample: starplayer_core::SampleId, note: Note) -> bool { note.semitone < 128 }

    fn note_on(&mut self, module: &Module, instrument: InstrumentId, channel: ChannelId, params: NoteParams, context: &mut EngineContext<'_>) -> Option<VoiceId> {
        if context.voices.capacity() > self.articulation.capacity() { return None; }
        let definition = module.instrument(instrument)?;
        let instrument_mode = ItFormatExtra::from_header(module.header()).is_instrument_mode();
        let original = params.note.semitone;
        let sample_id = if instrument_mode { definition.note_sample_map.get(original as usize).and_then(|sample| sample.checked_sub(1)).map(SampleId) } else { definition.sample }?;
        let sample = module.sample(sample_id).filter(|sample| sample.length_frames() != 0)?;
        let mapped = if instrument_mode { definition.note_transpose_map.get(original as usize).copied().unwrap_or(original).min(MAX_NOTE) } else { original };
        let previous = context.channels.foreground(channel).and_then(|voice| self.articulation.voice(voice)).cloned();
        // Candidate-owned DCT/DCA, excluding currently eligible custom candidates.
        for index in 0..self.articulation.capacity() {
            let Some(candidate) = self.articulation.voices.get(index).filter(|state| state.root_channel as u16 == channel.0 && !state.retiring).cloned() else { continue };
            let Some(voice) = candidate.voice else { continue };
            if !context.channels.playback().and_then(|state| state.get(voice, context.voices)).is_some_and(|owned| owned.lifecycle != VoiceLifecycle::Retiring) { continue; }
            #[cfg(feature = "note-overlap")]
            if context.channels.custom_overlap_candidate(voice, module, context.voices) { continue; }
            if !instrument_mode { continue; }
            let Some(candidate_definition) = candidate.instrument.checked_sub(1).and_then(|index| module.instrument(InstrumentId(index))) else { continue };
            let matches = candidate.instrument == instrument.0.saturating_add(1) && match candidate_definition.duplicate_check {
                DuplicateCheck::Off => false, DuplicateCheck::Note => candidate.trigger_note == original,
                DuplicateCheck::Sample => candidate.sample == sample_id.0.saturating_add(1), DuplicateCheck::Instrument => true,
            };
            if matches { match candidate_definition.duplicate_action {
                DuplicateAction::Cut => { context.channels.retire_voice(voice, context.voices); self.articulation.retire(voice); }
                DuplicateAction::NoteOff => self.articulation.release(module, instrument_mode, context.voices, voice),
                DuplicateAction::NoteFade => { if let Some(state) = self.articulation.voice_mut(voice) { state.note_fade = true; } }
            } }
        }
        if !preserve_rack_foreground(module, channel, context) && let Some(voice) = context.channels.foreground(channel) {
            let action = self.articulation.voice(voice).map(|state| state.new_note_action).unwrap_or(NewNoteAction::Cut);
            if action != NewNoteAction::Cut {
                context.channels.detach_foreground(channel);
                if let Some(state) = self.articulation.voice_mut(voice) { state.foreground = false; if action == NewNoteAction::NoteFade { state.note_fade = true; } }
                if action == NewNoteAction::NoteOff { self.articulation.release(module, instrument_mode, context.voices, voice); }
            }
        }
        let sample_volume = ((sample.default_volume().to_bits() as u32 * 256 + 32_767) / 65_535).min(256);
        let base = ((sample_volume * params.raw_velocity.to_bits() as u32 + 32_767) / 65_535) as u16;
        let sample_global = ItFormatData::from_header(module.header()).and_then(|data| data.sample_global_volume(sample_id.0)).unwrap_or(64).min(64);
        let instrument_global = if instrument_mode { ((definition.global_volume.to_bits() as u32 * 64 + 32_767) / 65_535) as u8 } else { 64 };
        let pan = params.pan_override.or(sample.default_pan()).or(definition.default_pan).map(bipolar_to_pan).unwrap_or(128);
        let mut state = ItVoiceState {
            root_channel: channel.0 as u8, foreground: true, instrument: instrument.0.checked_add(1)?, sample: sample_id.0.checked_add(1)?,
            note: mapped, trigger_note: original, frequency_hz: scale_frequency(sample.reference_rate_hz(), (mapped as i32 - REFERENCE_NOTE as i32) * UNITS_PER_SEMITONE + cents_to_units(params.note.cents as i32)), reference_rate_hz: sample.reference_rate_hz(),
            note_volume: base as i32, instrument_volume: (sample_global as u16 * instrument_global as u16 / 64) as u8, channel_volume: 64,
            pan: (pan as i32 + (original as i32 - definition.pitch_pan_centre as i32) * definition.pitch_pan_separation as i32 / 2).clamp(0, 256) as i16,
            pitch_envelope_is_filter: instrument_mode && definition.pitch_envelope_is_filter, playing: true,
            cutoff: definition.initial_filter_cutoff.unwrap_or(127), resonance: definition.initial_filter_resonance.unwrap_or(0),
            new_note_action: if instrument_mode { definition.new_note_action } else { NewNoteAction::Cut }, ..ItVoiceState::default()
        };
        if let Some(previous) = previous {
            if definition.volume_envelope.as_ref().is_some_and(|envelope| envelope.carry) { state.volume_envelope = previous.volume_envelope; }
            if definition.panning_envelope.as_ref().is_some_and(|envelope| envelope.carry) { state.panning_envelope = previous.panning_envelope; }
            if definition.pitch_envelope.as_ref().is_some_and(|envelope| envelope.carry) { state.pitch_envelope = previous.pitch_envelope; }
        }
        // Native SMF backgrounds retain IT's quietest-background pressure policy.
        // Live input never enters this branch: every one of its tails belongs to L.
        if channel.0 < 64 && context.channels.foreground(channel).is_none() && context.channels.playback().is_some_and(|playback| playback.active_count(starplayer_engine::VoiceDomain::Native, context.voices) >= playback.configuration().capacity.native.min(VIRTUAL_CHANNELS)) {
            let victim = context.voices.iter().filter_map(|(voice, mixer)| {
                let owned = context.channels.playback()?.get(voice, context.voices)?;
                let state = self.articulation.voice(voice)?;
                if owned.domain != starplayer_engine::VoiceDomain::Native || owned.lifecycle != VoiceLifecycle::Background || state.retiring { return None; }
                let score = if state.fadeout == 0 { -1 } else { let score = ((state.real_volume as i64) << 9) | state.note_volume.clamp(0, MAX_NOTE_VOLUME) as i64; if mixer.region().loop_span().is_some() { score / 2 } else { score } };
                Some((voice, score))
            }).min_by_key(|(voice, score)| (*score, voice.index())).map(|(voice, _)| voice)?;
            context.voices.record_steal(); context.voices.release(victim); self.articulation.discard(victim);
        }
        let tag = VoiceTag { channel: channel.0 as u8, instrument: instrument.0.checked_add(1)?, sample: sample_id.0.checked_add(1)?, note: mapped };
        let voice = admit_rack_note(module, channel, instrument, params.note, base, tag, sample_region(sample, sample.sustain_loop().is_some()), VoiceParams::SILENT, context)?;
        if !self.articulation.start(voice, state, context.voices) { context.voices.release(voice); context.channels.release_finished(context.voices); return None; }
        self.articulation.roll_swing(module, instrument_mode, voice, instrument.0.saturating_add(1));
        self.articulation.apply_envelope_flags(module, instrument_mode, voice, instrument.0.saturating_add(1));
        if let Some(mixer) = context.voices.get_mut(voice) { mixer.filter_mut().set_extended_range(ItFormatExtra::from_header(module.header()).has_extended_filter_range()); }
        self.write(module, voice, true, context);
        Some(voice)
    }
    fn release(&mut self, module: &Module, voice: VoiceId, context: &mut EngineContext<'_>) {
        if !context.channels.playback().and_then(|state| state.get(voice, context.voices)).is_some_and(|owned| owned.lifecycle != starplayer_engine::VoiceLifecycle::Retiring) { return; }
        self.articulation.release(module, ItFormatExtra::from_header(module.header()).is_instrument_mode(), context.voices, voice); }
    fn step(&mut self, module: &Module, voice: VoiceId, context: &mut EngineContext<'_>) { self.write(module, voice, false, context); }
    fn bend(&mut self, _module: &Module, voice: VoiceId, _note: Note, cents: i16, context: &mut EngineContext<'_>) {
        if !context.channels.playback().and_then(|state| state.get(voice, context.voices)).is_some_and(|owned| owned.lifecycle != starplayer_engine::VoiceLifecycle::Retiring) { return; }
        let Some(state) = self.articulation.voice_mut(voice) else { return };
        state.live_bend_cents = cents;
        let frequency = scale_frequency(state.output_frequency_hz, cents_to_units(cents as i32));
        context.write_voice_param(voice, starplayer_core::VoiceParam::Step(Step::from_ratio(frequency as u64, self.sample_rate_hz.max(1) as u64)));
    }
}
fn cents_to_units(cents: i32) -> i32 { let units = cents * UNITS_PER_SEMITONE; (units + if units < 0 { -50 } else { 50 }) / 100 }
