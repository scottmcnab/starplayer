//! Concrete, bounded overlap policy shared by format processors. Musical classification
//! and onset volume stay in the format; ownership/age/provenance stay in PlaybackState.

use starplayer_core::{ChannelId, TriggerIdentity, TriggerKind, VoiceId};
use starplayer_mixer::{Voice, VoicePool};
use starplayer_model::Module;
use crate::{ChannelTable, VoiceDomain, VoiceLifecycle};

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct OverlapDiagnostics {
    pub custom_evictions: u64,
    pub refused_detachments: u64,
}

/// Inspect the actual sounding PCM, never a channel's possibly newer instrument/sample.
/// The model stores only PCM samples; unmapped/FM instruments have no matching region.
/// Check both model loops: an already-released sustain loop is still ineligible.
pub fn eligible_one_shot(voice: &Voice, module: &Module) -> bool {
    let region = voice.region();
    let Some(sample) = voice.tag.sample.checked_sub(1).and_then(|index| module.sample(starplayer_core::SampleId(index))) else { return false };
    !voice.wants_stop() && region.length_frames() != 0 && region.loop_span().is_none()
        && sample.pcm_offset() == region.pcm_offset() && sample.length_frames() != 0
        && !sample.loop_mode().is_looping() && sample.sustain_loop().is_none()
}

impl ChannelTable {
    /// Current checkbox/eligibility decides custom action; provenance decides quota and
    /// cleanup only. NO-05 can use this predicate before native IT candidate processing.
    pub fn custom_overlap_candidate(&self, voice: VoiceId, module: &Module, voices: &VoicePool) -> bool {
        let Some(playback) = self.playback() else { return false };
        let Some(owned) = playback.get(voice, voices) else { return false };
        playback.note_overlap_enabled() && owned.identity().is_some() && owned.lifecycle != VoiceLifecycle::Retiring
            && voices.get(voice).and_then(|voice| voice.tag.instrument.checked_sub(1)).is_some_and(|instrument| playback.let_notes_ring(starplayer_core::InstrumentId(instrument)) == Some(true))
            && voices.get(voice).is_some_and(|voice| eligible_one_shot(voice, module))
    }

    /// Preserve an eligible foreground at the format's actual new-note seam. This does
    /// not finalize the incoming identity or run duplicates: IT needs those after effects.
    /// A failed admission leaves the binding and pending swap intact for native fallback.
    pub fn prepare_note_overlap(&mut self, channel: ChannelId, module: &Module, voices: &mut VoicePool) -> bool {
        let Some(outgoing) = self.foreground(channel) else { return false };
        if !self.custom_overlap_candidate(outgoing, module, voices) { return false; }
        let Some(playback) = self.playback() else { return false };
        let Some(owned) = playback.get(outgoing, voices).copied() else { return false };
        // This seam migrates song foregrounds N → C. NO-06 owns held live instances
        // and L admission before using the common predicate/duplicate scan.
        if owned.domain != VoiceDomain::Native { return false; }
        let domain = VoiceDomain::Custom;
        let victim = if playback.active_count(domain, voices) >= playback.configuration().capacity.custom {
            playback.oldest(domain, false, voices)
        } else { None };
        if !self.prepare_admission(domain, victim, voices) {
            if let Some(playback) = self.playback_mut() { playback.overlap_diagnostics.refused_detachments = playback.overlap_diagnostics.refused_detachments.saturating_add(1); }
            return false;
        }
        if self.detach_custom_foreground(channel, voices).ok().flatten() != Some(outgoing) { return false; }
        if let Some(voice) = voices.get_mut(outgoing) { voice.discard_pending_region(); }
        if victim.is_some() && let Some(playback) = self.playback_mut() {
            playback.overlap_diagnostics.custom_evictions = playback.overlap_diagnostics.custom_evictions.saturating_add(1);
        }
        true
    }

    /// Run after finalization at the same event frame, skipping the incoming generation.
    /// Retire every eligible exact match, including a passively recorded native voice.
    /// Native-created backgrounds retain N and custom-created tails retain C/L throughout.
    pub fn replace_note_overlap_duplicates(&mut self, incoming: VoiceId, module: &Module, voices: &mut VoicePool) {
        let identity = self.playback().and_then(|playback| playback.get(incoming, voices)).and_then(|owned| owned.identity());
        let Some(identity) = identity else { return };
        for index in 0..voices.capacity() {
            let candidate = self.playback().and_then(|playback| playback.slot(index, voices))
                .filter(|owned| self.playback().is_some_and(|playback| playback.duplicate_matches(incoming, identity, *owned, voices)));
            if let Some(owned) = candidate && self.custom_overlap_candidate(owned.voice, module, voices) { self.retire_voice(owned.voice, voices); }
        }
    }
}

impl crate::TickContext<'_> {
    /// Legacy formats flush after all onset-tick effects, so preservation, finalization
    /// and duplicate retirement can execute synchronously here before any mixing.
    pub fn trigger_legacy_overlap(&mut self, tag: starplayer_mixer::VoiceTag, region: starplayer_mixer::SampleRegion, params: starplayer_core::VoiceParams, offset_frames: u32, onset: Option<TriggerIdentity>, module: &Module) -> Option<VoiceId> {
        let channel = ChannelId(tag.channel as u16);
        let actual_start = onset.filter(|_| region.length_frames() != 0 && (offset_frames < region.length_frames() || region.loop_span().is_some()));
        let previous_identity = self.channels.foreground(channel)
            .and_then(|voice| self.channels.playback().and_then(|playback| playback.get(voice, self.voices))).and_then(|owned| owned.identity());
        if actual_start.is_some() { self.channels.prepare_note_overlap(channel, module, self.voices); }
        let voice = self.trigger_channel(channel, tag, region, params, offset_frames)?;
        // Native retrigger changes the mixer generation, but belongs to the same musical
        // note. Preserve its immutable identity without starting a new duplicate scan.
        if let Some(identity) = actual_start {
            if self.finalize_onset(voice, TriggerKind::NewNote, identity).ok() == Some(true) {
                self.channels.replace_note_overlap_duplicates(voice, module, self.voices);
            }
        } else if onset.is_none() && let Some(identity) = previous_identity && let Some(playback) = self.channels.playback_mut() {
            let _ = playback.inherit_onset(voice, identity, self.voices);
        }
        Some(voice)
    }
}
