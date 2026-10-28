//! Loaded XM live adapter; effects remain in the tracker processor.
use super::*;
use starplayer_core::VoiceId;
use starplayer_engine::instrument::{NoteParams, live::{RackRuntime, admit_rack_note, preserve_rack_foreground}};
use starplayer_engine::EngineContext;

/// The same pure tuning preflight used by live onsets and metadata queries.
pub fn live_note_table_index(sample: &starplayer_model::SampleIndex, note: Note, extra_cents: i16) -> Option<usize> {
    let index = (note.semitone as i32 - 12 + sample.relative_note() as i32) * 16 + (sample.finetune() >> 3) as i32 + 16 + cents_to_table(note.cents as i32 + extra_cents as i32);
    (0..PERIOD_TABLE_LEN as i32).contains(&index).then_some(index as usize)
}

pub struct XmLiveRuntime { articulation: XmArticulationRuntime, sample_rate_hz: u32 }
impl XmLiveRuntime {
    pub fn try_new(capacity: usize, sample_rate_hz: u32) -> Result<Self, alloc::collections::TryReserveError> { Ok(Self { articulation: XmArticulationRuntime::try_new(capacity)?, sample_rate_hz }) }
}
impl RackRuntime for XmLiveRuntime {
    fn note_on(&mut self, module: &Module, instrument: InstrumentId, channel: ChannelId, params: NoteParams, context: &mut EngineContext<'_>) -> Option<VoiceId> {
        if context.voices.capacity() > self.articulation.capacity() { return None; }
        let definition = module.instrument(instrument)?;
        let sample_id = if definition.note_sample_map.is_empty() { definition.sample } else { definition.note_sample_map.get(params.note.semitone as usize).and_then(|sample| sample.checked_sub(1)).map(SampleId) }?;
        let sample = module.sample(sample_id).filter(|sample| sample.length_frames() != 0)?;
        // Preserve the live reference: MIDI60 is XM C-4, while the map uses the
        // original input note. Native tuning uses FT2's own period tables.
        let table_index = live_note_table_index(sample, params.note, 0)?;
        let period = if module.header().flags.linear_slides { linear_period(table_index) } else { amiga_period(table_index) };
        let base = ((volume_to_byte(sample.default_volume()) as u32 * params.raw_velocity.to_bits() as u32 + 32_767) / 65_535) as u8;
        preserve_rack_foreground(module, channel, context);
        let state = XmVoiceState { instrument: Some(instrument), sample: Some(sample_id), out_period: period, out_volume: base, out_pan: pan_to_byte(params.pan_override.or(sample.default_pan())), ..XmVoiceState::default() };
        let tag = VoiceTag { channel: channel.0 as u8, instrument: instrument.0.checked_add(1)?, sample: sample_id.0.checked_add(1)?, note: params.note.to_midi() };
        let voice = admit_rack_note(module, channel, instrument, params.note, base as u16, tag, sample_region(sample), VoiceParams::SILENT, context)?;
        if !self.articulation.start(module, voice, channel, state, context.voices) { context.voices.release(voice); context.channels.release_finished(context.voices); return None; }
        self.step(module, voice, context);
        Some(voice)
    }
    fn release(&mut self, module: &Module, voice: VoiceId, context: &mut EngineContext<'_>) {
        if !context.channels.playback().and_then(|state| state.get(voice, context.voices)).is_some_and(|owned| owned.lifecycle != starplayer_engine::VoiceLifecycle::Retiring) { return; }
        self.articulation.release(module, voice); }
    fn step(&mut self, module: &Module, voice: VoiceId, context: &mut EngineContext<'_>) {
        if !context.channels.playback().and_then(|state| state.get(voice, context.voices)).is_some_and(|owned| owned.lifecycle != starplayer_engine::VoiceLifecycle::Retiring) { return; }
        self.articulation.control_step(module, voice, 64, self.sample_rate_hz, module.header().flags.linear_slides, context.voices);
        if self.articulation.voice(voice).is_some_and(|state| state.key_off && state.final_volume == 0) { context.channels.retire_voice(voice, context.voices); self.articulation.retire(voice); }
    }
    fn bend(&mut self, module: &Module, voice: VoiceId, note: Note, cents: i16, context: &mut EngineContext<'_>) {
        if !context.channels.playback().and_then(|state| state.get(voice, context.voices)).is_some_and(|owned| owned.lifecycle != starplayer_engine::VoiceLifecycle::Retiring) { return; }
        let Some(slot) = self.articulation.slots.get_mut(voice.index() as usize).filter(|slot| slot.voice == Some(voice) && !slot.retiring) else { return };
        if context.voices.get(voice).is_none() { return; }
        let Some(sample) = slot.state.sample.and_then(|sample| module.sample(sample)) else { return };
        let Some(index) = live_note_table_index(sample, note, cents) else { return };
        let modulation = slot.state.final_period.wrapping_sub(slot.state.out_period);
        slot.state.out_period = if module.header().flags.linear_slides { linear_period(index) } else { amiga_period(index) };
        slot.state.final_period = slot.state.out_period.wrapping_add(modulation);
        context.write_voice_param(voice, starplayer_core::VoiceParam::Step(step_for_period(slot.state.final_period, self.sample_rate_hz, module.header().flags.linear_slides)));
    }
}

fn cents_to_table(cents: i32) -> i32 { let units = cents * 16; (units + if units < 0 { -50 } else { 50 }) / 100 }
