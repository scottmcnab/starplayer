//! Prepared live instruments. The rack owns key instances; PlaybackState alone owns
//! domain, lifecycle, provenance and age. No state here is shared with the song RNG.
use super::*;
use crate::{ClearScope, VoiceDomain, VoiceLifecycle};
#[cfg(feature = "interactive-audition")]
use crate::LiveOrigin;

/// One mutable, pool-indexed format runtime for every program in a rack.
/// Implemented by XM and IT, using their native articulation computations.
pub trait RackRuntime: Send {
    fn note_on(&mut self, module: &Module, instrument: InstrumentId, channel: ChannelId, params: NoteParams, context: &mut EngineContext<'_>) -> Option<VoiceId>;
    fn release(&mut self, module: &Module, voice: VoiceId, context: &mut EngineContext<'_>);
    fn step(&mut self, module: &Module, voice: VoiceId, context: &mut EngineContext<'_>);
    fn bend(&mut self, module: &Module, voice: VoiceId, note: Note, cents: i16, context: &mut EngineContext<'_>);
}

/// Preserve a custom live foreground without migrating it into the song-tail budget.
pub fn preserve_rack_foreground(module: &Module, channel: ChannelId, context: &mut EngineContext<'_>) -> bool {
    #[cfg(feature = "note-overlap")]
    if channel.0 < 64 { return context.channels.prepare_note_overlap(channel, module, context.voices); }
    #[cfg(all(feature = "note-overlap", feature = "interactive-audition"))]
    if let Some(voice) = context.channels.foreground(channel) && context.channels.custom_overlap_candidate(voice, module, context.voices) {
        return context.channels.detach_custom_foreground(channel, context.voices).ok().flatten().is_some();
    }
    let _ = (module, channel, context);
    false
}

/// Admission after the format has applied native/custom outgoing-note actions.
/// Every L voice (including IT NNA) uses the same oldest-active pressure policy.
#[allow(clippy::too_many_arguments)]
pub fn admit_rack_note(module: &Module, channel: ChannelId, instrument: InstrumentId, note: Note, base_volume: u16, mut tag: VoiceTag, region: SampleRegion, params: VoiceParams, context: &mut EngineContext<'_>) -> Option<VoiceId> {
    if let Some(previous) = context.channels.foreground(channel) { context.channels.retire_voice(previous, context.voices); }
    context.channels.playback()?;
    #[cfg(feature = "interactive-audition")]
    let playback = context.channels.playback()?;
    #[cfg(feature = "interactive-audition")]
    let domain = if channel.0 >= 64 { VoiceDomain::Live } else { VoiceDomain::Native };
    #[cfg(not(feature = "interactive-audition"))]
    let domain = VoiceDomain::Native;
    #[cfg(feature = "interactive-audition")]
    let victim = if domain == VoiceDomain::Live && playback.active_count(domain, context.voices) >= playback.configuration().capacity.live { playback.oldest(domain, false, context.voices) } else { None };
    #[cfg(not(feature = "interactive-audition"))]
    let victim = None;
    if !context.channels.prepare_admission(domain, victim, context.voices) { return None; }
    if victim.is_some() { context.voices.record_steal(); }
    tag.instrument = instrument.0.checked_add(1)?;
    let voice = context.channels.trigger(channel, context.voices, tag, region, params, 0)?;
    #[cfg(feature = "note-overlap")]
    {
        use starplayer_core::{OriginalNote, TriggerIdentity, TriggerKind};
        let identity = TriggerIdentity { instrument, note: OriginalNote::Note(note), channel, onset_volume: base_volume };
        if let Some(playback) = context.channels.playback_mut() { let _ = playback.finalize_onset(voice, TriggerKind::NewNote, identity, context.voices); }
        context.channels.replace_note_overlap_duplicates(voice, module, context.voices);
    }
    let _ = (module, note, base_volume);
    Some(voice)
}

#[derive(Copy, Clone)]
struct Instance {
    voice: VoiceId,
    note: Note,
    channel: ChannelId,
    held: bool,
    pedal: bool,
    onset: Frame,
    gain: U0F16,
}

/// Bounded diagnostics remain queryable after queue recovery or pool pressure.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveDiagnostics { pub refused_notes: u64, pub queue_overflows: u64 }

/// A prepared interactive rack, constructed off the audio thread.
pub(super) struct PreparedRack {
    pub(super) module: Arc<Module>,
    pub(super) initial_interval: u64,
    runtime: Option<Box<dyn RackRuntime>>,
    instances: Box<[Option<Instance>]>,
    #[cfg(feature = "interactive-audition")]
    preview: RackChannel,
    #[cfg(feature = "interactive-audition")]
    destination: starplayer_core::LiveDestination,
    sample_rate_hz: u32,
    mapped: bool,
    scope: RackScope,
    pub(super) diagnostics: LiveDiagnostics,
}

impl PreparedRack {
    pub(super) fn try_new(module: &Arc<Module>, sample_rate_hz: u32, capacity: usize, runtime: Option<Box<dyn RackRuntime>>, mapped: bool, scope: RackScope) -> Result<Self, alloc::collections::TryReserveError> {
        let mut instances = Vec::new(); instances.try_reserve_exact(capacity)?; instances.resize(capacity, None);
        let header = module.header();
        let tempo = starplayer_core::quirks::QuirkSelection::FromDialect.resolve(header.dialect).tempo_model;
        let initial_interval = starplayer_core::FrameClock::new(tempo, sample_rate_hz, Frame::ZERO).interval_bits(header.initial_tempo, header.initial_speed);
        Ok(Self { initial_interval, module: Arc::clone(module), runtime, instances: instances.into_boxed_slice(), #[cfg(feature = "interactive-audition")] preview: RackChannel::default(), #[cfg(feature = "interactive-audition")] destination: if scope == RackScope::Song { starplayer_core::LiveDestination::Jam } else { starplayer_core::LiveDestination::Off }, sample_rate_hz, mapped, scope, diagnostics: LiveDiagnostics::default() })
    }

    pub(super) fn capability(&self, instrument: InstrumentId) -> InstrumentCapability { instrument_capability(&self.module, instrument, self.mapped) }

    fn release(&mut self, voice: VoiceId, context: &mut EngineContext<'_>) {
        if !context.channels.playback().and_then(|state| state.get(voice, context.voices)).is_some_and(|owned| owned.lifecycle != VoiceLifecycle::Retiring) { return; }
        match &mut self.runtime {
            Some(runtime) => runtime.release(&self.module, voice, context),
            None => { context.channels.retire_voice(voice, context.voices); }
        }
    }

    fn release_lane(&mut self, channel: ChannelId, note: Option<Note>, pedal: bool, context: &mut EngineContext<'_>) {
        for index in 0..self.instances.len() {
            let Some(mut instance) = self.instances.get(index).copied().flatten() else { continue };
            if instance.channel != channel || !instance.held || note.is_some_and(|note| note != instance.note) { continue; }
            if pedal { instance.pedal = true; } else { instance.held = false; instance.pedal = false; self.release(instance.voice, context); }
            if let Some(slot) = self.instances.get_mut(index) { *slot = Some(instance); }
        }
    }

    pub(super) fn clear(&mut self, scope: ClearScope, context: &mut EngineContext<'_>) {
        for slot in self.instances.iter_mut() {
            if let Some(instance) = slot && context.channels.playback().and_then(|state| state.get(instance.voice, context.voices)).is_none_or(|owned| owned.in_scope(scope) || owned.lifecycle == VoiceLifecycle::Retiring) { *slot = None; }
        }
        #[cfg(feature = "interactive-audition")]
        if matches!(scope, ClearScope::All | ClearScope::Live | ClearScope::Lane(ChannelId(80)) | ClearScope::LiveOrigin(LiveOrigin::ButtonPreview)) { self.preview = RackChannel::default(); }
    }

    pub(super) fn control(&mut self, context: &mut EngineContext<'_>) {
        for index in 0..self.instances.len() {
            let Some(instance) = self.instances.get(index).copied().flatten() else { continue };
            let active = context.channels.playback().and_then(|state| state.get(instance.voice, context.voices)).is_some_and(|owned| owned.lifecycle != VoiceLifecycle::Retiring);
            if !active { if let Some(slot) = self.instances.get_mut(index) { *slot = None; } continue; }
            if instance.onset == context.frame { continue; }
            if let Some(runtime) = &mut self.runtime {
                runtime.step(&self.module, instance.voice, context);
                if let Some(voice) = context.voices.get_mut(instance.voice) {
                    voice.params.set_volume(scale_unit(voice.params.volume, instance.gain));
                }
            }
        }
    }

    pub(super) fn apply(&mut self, channels: &mut [RackChannel; MIDI_CHANNEL_COUNT], target: Target, event: Event, context: &mut EngineContext<'_>) -> EventOutcome {
        let Target::Channel(channel) = target else {
            if matches!(event, Event::AllNotesOff | Event::AllSoundOff | Event::Cut | Event::KeyOff | Event::FadeOut) {
                for lane in self.scope.base()..self.scope.base() + self.scope.lanes() { self.apply(channels, Target::Channel(ChannelId(lane)), event, context); }
                return EventOutcome::Applied;
            }
            return EventOutcome::Unsupported;
        };
        let Some(index) = channel.0.checked_sub(self.scope.base()).filter(|index| *index < self.scope.lanes()) else { return EventOutcome::Unsupported };
        #[cfg(feature = "interactive-audition")]
        let state = if index == 16 { &mut self.preview } else { let Some(state) = channels.get_mut(index as usize) else { return EventOutcome::Unsupported }; state };
        #[cfg(not(feature = "interactive-audition"))]
        let Some(state) = channels.get_mut(index as usize) else { return EventOutcome::Unsupported };
        match event {
            Event::NoteOn { note, velocity } if velocity != U0F16::ZERO => {
                if context.voices.capacity() > self.instances.len() { self.diagnostics.refused_notes = self.diagnostics.refused_notes.saturating_add(1); return EventOutcome::Ignored; }
                #[cfg(not(feature = "interactive-audition"))]
                let program = state.program;
                #[cfg(feature = "interactive-audition")]
                let program = if index == 16 { state.program } else { match self.destination { starplayer_core::LiveDestination::Off => return EventOutcome::Ignored, starplayer_core::LiveDestination::Jam => state.program, starplayer_core::LiveDestination::Selected(program) => program } };
                let (gain, pan, bend) = (state.volume, state.pan, state.bend);
                let params = NoteParams { note, velocity, raw_velocity: velocity, pan_override: pan };
                let voice = if let Some(runtime) = &mut self.runtime { runtime.note_on(&self.module, program, channel, params, context) } else {
                    let Some(plan) = plan_simple(&self.module, program, self.sample_rate_hz, params) else { return EventOutcome::Ignored };
                    preserve_rack_foreground(&self.module, channel, context);
                    admit_rack_note(&self.module, channel, program, note, plan.params.volume.to_bits(), plan.tag, plan.region, plan.params, context)
                };
                let Some(voice) = voice else { self.diagnostics.refused_notes = self.diagnostics.refused_notes.saturating_add(1); return EventOutcome::Ignored };
                if let Some(runtime) = &mut self.runtime && bend != I1F15::ZERO { runtime.bend(&self.module, voice, note, ((bend.to_bits() as i32 * DEFAULT_BEND_RANGE_CENTS as i32) / i16::MAX as i32) as i16, context); }
                if self.runtime.is_none() && bend != I1F15::ZERO && let Some(sample) = context.voices.get(voice).and_then(|voice| voice.tag.sample.checked_sub(1)).and_then(|sample| self.module.sample(SampleId(sample))) {
                    let cents = ((bend.to_bits() as i32 * DEFAULT_BEND_RANGE_CENTS as i32) / i16::MAX as i32) as i16;
                    context.write_voice_param(voice, VoiceParam::Step(step_for(sample.reference_rate_hz(), units_for_note(note, cents), self.sample_rate_hz)));
                }
                let base_output = context.voices.get(voice).map(|voice| voice.params.volume).unwrap_or(U0F16::ZERO);
                if let Some(state) = context.voices.get_mut(voice) { state.params.set_volume(scale_unit(base_output, gain)); }
                #[cfg(not(feature = "interactive-audition"))]
                let held = true;
                #[cfg(feature = "interactive-audition")]
                let held = index != 16 || context.voices.get(voice).and_then(|voice| voice.tag.sample.checked_sub(1)).and_then(|sample| self.module.sample(SampleId(sample))).is_some_and(|sample| sample.loop_mode().is_looping() || sample.sustain_loop().is_some());
                if let Some(slot) = self.instances.get_mut(voice.index() as usize) { *slot = Some(Instance { voice, note, channel, held, pedal: false, onset: context.frame, gain }); }
                #[cfg(feature = "interactive-audition")]
                let origin = if index == 16 { LiveOrigin::ButtonPreview } else if matches!(self.destination, starplayer_core::LiveDestination::Selected(_)) { LiveOrigin::SelectedInput } else { LiveOrigin::Jam };
                #[cfg(feature = "interactive-audition")]
                if self.scope == RackScope::Live && let Some(playback) = context.channels.playback_mut() { let _ = playback.set_live_origin(playback.configuration().identity, voice, origin, context.voices); }
                EventOutcome::Applied
            }
            Event::NoteOn { note, .. } | Event::NoteOff { note, .. } => { let sustain = state.sustain; self.release_lane(channel, Some(note), sustain, context); EventOutcome::Applied }
            Event::AllNotesOff | Event::KeyOff | Event::FadeOut => { state.sustain = false; self.release_lane(channel, None, false, context); EventOutcome::Applied }
            Event::AllSoundOff | Event::Cut | Event::Controller { number: controller::ALL_SOUND_OFF, .. } => { state.sustain = false; context.channels.clear_scope(ClearScope::Lane(channel), context.voices); self.clear(ClearScope::Lane(channel), context); EventOutcome::Applied }
            Event::Controller { number: controller::ALL_NOTES_OFF, .. } => { state.sustain = false; self.release_lane(channel, None, false, context); EventOutcome::Applied }
            Event::Program(program) => { if state.program != program { state.program = program; state.sustain = false; context.channels.clear_scope(ClearScope::Lane(channel), context.voices); self.clear(ClearScope::Lane(channel), context); } EventOutcome::Applied }
            // Preserve the existing live convention: CC7/CC10 latch for the next
            // note. Existing instances retain their onset gain through articulation.
            Event::Controller { number: controller::VOLUME, value } => { state.volume = value; EventOutcome::Applied }
            Event::Controller { number: controller::PAN, value } => { state.pan = Some(pan_from_unit(value)); EventOutcome::Applied }
            Event::Controller { number: controller::SUSTAIN, value } => {
                state.sustain = value.to_bits() >= u16::MAX / 2;
                if !state.sustain {
                    for index in 0..self.instances.len() {
                        let Some(mut instance) = self.instances.get(index).copied().flatten() else { continue };
                        if instance.channel == channel && instance.held && instance.pedal { instance.held = false; instance.pedal = false; self.release(instance.voice, context); if let Some(slot) = self.instances.get_mut(index) { *slot = Some(instance); } }
                    }
                }
                EventOutcome::Applied
            }
            Event::PitchBend(bend) => { state.bend = bend; if let Some(runtime) = &mut self.runtime { for instance in self.instances.iter().flatten().filter(|instance| instance.channel == channel && instance.held) {
                        if !context.channels.playback().and_then(|state| state.get(instance.voice, context.voices)).is_some_and(|owned| owned.lifecycle != VoiceLifecycle::Retiring) { continue; } runtime.bend(&self.module, instance.voice, instance.note, ((bend.to_bits() as i32 * DEFAULT_BEND_RANGE_CENTS as i32) / i16::MAX as i32) as i16, context); } } else {
                    for instance in self.instances.iter().flatten().filter(|instance| instance.channel == channel && instance.held) {
                        if !context.channels.playback().and_then(|state| state.get(instance.voice, context.voices)).is_some_and(|owned| owned.lifecycle != VoiceLifecycle::Retiring) { continue; }
                        if let Some(sample) = context.voices.get(instance.voice).and_then(|voice| voice.tag.sample.checked_sub(1)).and_then(|sample| self.module.sample(SampleId(sample))) {
                            let cents = ((bend.to_bits() as i32 * DEFAULT_BEND_RANGE_CENTS as i32) / i16::MAX as i32) as i16;
                            context.write_voice_param(instance.voice, VoiceParam::Step(step_for(sample.reference_rate_hz(), units_for_note(instance.note, cents), self.sample_rate_hz)));
                        }
                    }
                } EventOutcome::Applied }
            _ => EventOutcome::Ignored,
        }
    }

    #[cfg(feature = "interactive-audition")]
    pub(super) fn action(&mut self, channels: &mut [RackChannel; MIDI_CHANNEL_COUNT], action: starplayer_core::InteractiveAction, context: &mut EngineContext<'_>) {
        use starplayer_core::InteractiveAction;
        if self.scope == RackScope::Song { return; }
        match action {
            InteractiveAction::SetLiveDestination(destination) => {
                if self.destination != destination {
                    if let starplayer_core::LiveDestination::Selected(instrument) = destination && !self.capability(instrument).playable { return; }
                    context.channels.clear_scope(ClearScope::Live, context.voices); self.clear(ClearScope::Live, context);
                    for state in channels.iter_mut() { state.sustain = false; state.held.clear(); }
                    self.destination = destination;
                }
            }
            InteractiveAction::PreviewInstrument(instrument) => {
                if !self.capability(instrument).preview_playable { return; }
                if self.preview.program != instrument { context.channels.clear_scope(ClearScope::Lane(ChannelId(80)), context.voices); self.clear(ClearScope::Lane(ChannelId(80)), context); }
                self.preview.program = instrument;
                self.apply(channels, Target::Channel(ChannelId(80)), Event::NoteOn { note: Note::new(REFERENCE_NOTE), velocity: U0F16::MAX }, context);
            }
            InteractiveAction::ReleasePreview => { self.release_lane(ChannelId(80), None, false, context); }
            _ => {}
        }
    }
}

/// Metadata from the installed prepared module. Sample loop/one-shot eligibility
/// covers every mapped PCM key, independently of live adapter pitch viability.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct InstrumentCapability {
    pub playable: bool, pub one_shot: bool, pub looping: bool,
    pub preview_playable: bool, pub preview_looping: bool,
}

/// Pure metadata projection. The facade supplies native instrument/sample mode; mapped
/// instruments never fall back from an unmapped key to definition.sample.
pub fn instrument_capability(module: &Module, instrument: InstrumentId, mapped: bool) -> InstrumentCapability {
    instrument_capability_filtered(module, instrument, mapped, |_, _| true)
}

/// Add a format-native, allocation-free viability preflight without preparing a rack.
pub fn instrument_capability_filtered(module: &Module, instrument: InstrumentId, mapped: bool, playable: impl Fn(u8, SampleId) -> bool) -> InstrumentCapability {
    let Some(definition) = module.instrument(instrument) else { return InstrumentCapability::default() };
    let mut result = InstrumentCapability::default();
    for note in 0..128 {
        let sample = if mapped { definition.note_sample_map.get(note).and_then(|number| number.checked_sub(1)).map(SampleId) } else { definition.sample };
        if let Some(sample_id) = sample && let Some(sample) = module.sample(sample_id).filter(|sample| sample.length_frames() != 0) {
            let looping = sample.loop_mode().is_looping() || sample.sustain_loop().is_some();
            result.looping |= looping; result.one_shot |= !looping;
            if playable(note as u8, sample_id) {
                result.playable = true;
                if note == REFERENCE_NOTE as usize { result.preview_playable = true; result.preview_looping = looping; }
            }
        }
    }
    result
}

/// Prepared source scope; sequenced SMF uses N/C and never shares live lanes or quota.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RackScope { Song, #[cfg(feature = "interactive-audition")] Live }
impl RackScope {
    const fn base(self) -> u16 { match self { Self::Song => 0, #[cfg(feature = "interactive-audition")] Self::Live => 64 } }
    const fn lanes(self) -> u16 { match self { Self::Song => 16, #[cfg(feature = "interactive-audition")] Self::Live => 17 } }
}

#[cfg(test)]
mod layout_tests {
    #[test]
    fn prepared_rack_feature_layout() {
        extern crate std;
        std::println!("PreparedRack={} RackChannel={}", core::mem::size_of::<super::PreparedRack>(), core::mem::size_of::<super::RackChannel>());
        #[cfg(all(target_pointer_width = "64", not(feature = "interactive-audition")))]
        assert_eq!(core::mem::size_of::<super::PreparedRack>(), 72);
        #[cfg(all(target_pointer_width = "64", feature = "interactive-audition"))]
        assert_eq!(core::mem::size_of::<super::PreparedRack>(), 136);
    }
}
