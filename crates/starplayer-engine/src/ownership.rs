//! One preallocated per-build ownership record, shared by song and live runtimes.
//! No voice allocation, stealing, detachment or clearing policy executes here.

use alloc::{boxed::Box, vec::Vec};
use starplayer_core::{ChannelId, PlaybackIdentity, VoiceId};
#[cfg(feature = "note-overlap")]
use starplayer_core::{InstrumentId, PlaybackCapabilities, TriggerIdentity, TriggerKind};
use starplayer_mixer::VoicePool;
use crate::playback::{PlaybackConfigError, PlaybackConfiguration};

/// Resource provenance is independent of today's instrument checkbox/eligibility.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VoiceDomain { Native, Custom, Live }

/// Live cleanup origin, not a separate quota: all three share L.
#[cfg(feature = "interactive-audition")]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LiveOrigin { Jam, SelectedInput, ButtonPreview }

/// Foreground/background/native release are orthogonal to resource provenance.
/// Only Retiring excludes a still-sounding voice from duplicate scans and admission.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VoiceLifecycle { Foreground, Background, Retiring }

/// Scoped clear predicates. NO-03/06 apply release ramps and invalidate held bindings.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClearScope {
    All, Song, Live, Lane(ChannelId), CustomDetached,
    #[cfg(feature = "interactive-audition")]
    LiveOrigin(LiveOrigin),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct VoiceOwnership {
    pub voice: VoiceId,
    pub domain: VoiceDomain,
    pub lifecycle: VoiceLifecycle,
    pub channel: ChannelId,
    /// Monotonic admission age, with stable slot index as the tie-break.
    pub age: u64,
    #[cfg(feature = "interactive-audition")]
    live_origin: Option<LiveOrigin>,
    /// A custom live tail stays Live; native-created backgrounds never acquire this flag
    /// because a checkbox changes. Gate the custom-only field out of audition-only builds.
    #[cfg(feature = "note-overlap")]
    pub custom_detached: bool,
    #[cfg(feature = "note-overlap")]
    identity: Option<TriggerIdentity>,
}

impl VoiceOwnership {
    pub fn in_scope(self, scope: ClearScope) -> bool {
        match scope {
            ClearScope::All => true,
            ClearScope::Song => self.domain != VoiceDomain::Live,
            ClearScope::Live => self.domain == VoiceDomain::Live,
            ClearScope::Lane(channel) => self.channel == channel,
            #[cfg(feature = "interactive-audition")]
            ClearScope::LiveOrigin(origin) => self.domain == VoiceDomain::Live && self.live_origin == Some(origin),
            ClearScope::CustomDetached => {
                #[cfg(feature = "note-overlap")]
                { self.custom_detached }
                #[cfg(not(feature = "note-overlap"))]
                { false }
            }
        }
    }

    #[cfg(feature = "interactive-audition")]
    pub const fn live_origin(self) -> Option<LiveOrigin> { self.live_origin }

    #[cfg(feature = "note-overlap")]
    pub const fn identity(self) -> Option<TriggerIdentity> { self.identity }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlaybackStateError { StaleIdentity, InvalidInstrument, InvalidVoice, InvalidOwnership, Unsupported, AlreadyFinalized }

/// Bounded session settings and generational ownership. Allocate before playback.
#[derive(Debug)]
pub struct PlaybackState {
    configuration: PlaybackConfiguration,
    slots: Box<[Option<VoiceOwnership>]>,
    next_age: u64,
    #[cfg(feature = "note-overlap")]
    let_notes_ring: Box<[bool]>,
    #[cfg(feature = "note-overlap")]
    note_overlap_enabled: bool,
}

impl PlaybackState {
    pub fn try_new(configuration: PlaybackConfiguration) -> Result<Self, PlaybackConfigError> {
        configuration.validate()?;
        let mut slots = Vec::new();
        slots.try_reserve_exact(configuration.capacity.physical()).map_err(|_| PlaybackConfigError::AllocationFailed)?;
        slots.resize(configuration.capacity.physical(), None);
        #[cfg(feature = "note-overlap")]
        let selections = {
            let mut selections = Vec::new();
            let count = if configuration.capabilities.contains(PlaybackCapabilities::NOTE_OVERLAP) { configuration.instrument_count } else { 0 };
            selections.try_reserve_exact(count).map_err(|_| PlaybackConfigError::AllocationFailed)?;
            selections.resize(count, false);
            selections.into_boxed_slice()
        };
        Ok(Self { configuration, slots: slots.into_boxed_slice(), next_age: 0,
            #[cfg(feature = "note-overlap")] let_notes_ring: selections,
            #[cfg(feature = "note-overlap")] note_overlap_enabled: false,
        })
    }

    pub const fn configuration(&self) -> PlaybackConfiguration { self.configuration }
    pub fn capacity(&self) -> usize { self.slots.len() }
    pub fn validate_identity(&self, identity: PlaybackIdentity) -> Result<(), PlaybackStateError> {
        if identity == self.configuration.identity { Ok(()) } else { Err(PlaybackStateError::StaleIdentity) }
    }

    /// Register every admitted slot, including native backgrounds. Reusing a slot replaces
    /// its old generational record; calling twice for a live generation is rejected.
    pub fn register(&mut self, voice: VoiceId, domain: VoiceDomain, channel: ChannelId, voices: &VoicePool) -> Result<(), PlaybackStateError> {
        if (domain == VoiceDomain::Custom && self.configuration.capacity.custom == 0) || (domain == VoiceDomain::Live && self.configuration.capacity.live == 0) {
            return Err(PlaybackStateError::InvalidOwnership);
        }
        #[cfg(feature = "interactive-audition")]
        if self.configuration.capabilities.contains(starplayer_core::PlaybackCapabilities::INTERACTIVE_AUDITION)
            && ((domain == VoiceDomain::Live) != (64..=80).contains(&channel.0) || channel.0 > 80)
        { return Err(PlaybackStateError::InvalidOwnership); }
        let state = voices.get(voice).ok_or(PlaybackStateError::InvalidVoice)?;
        if state.tag.channel as u16 != channel.0 { return Err(PlaybackStateError::InvalidOwnership); }
        let slot = self.slots.get_mut(voice.index() as usize).ok_or(PlaybackStateError::InvalidVoice)?;
        if slot.is_some_and(|owned| owned.voice == voice) { return Err(PlaybackStateError::InvalidOwnership); }
        self.next_age = self.next_age.saturating_add(1);
        *slot = Some(VoiceOwnership { voice, domain, lifecycle: VoiceLifecycle::Foreground, channel, age: self.next_age,
            // Existing rack admissions are Jam; lane 80 is always button Preview.
            // NO-06 sets SelectedInput at the same event frame before mixing.
            #[cfg(feature = "interactive-audition")] live_origin: if domain == VoiceDomain::Live { Some(if channel.0 == 80 { LiveOrigin::ButtonPreview } else { LiveOrigin::Jam }) } else { None },
            #[cfg(feature = "note-overlap")] custom_detached: false,
            #[cfg(feature = "note-overlap")] identity: None,
        });
        Ok(())
    }

    /// Stale handles and reclaimed slots never resolve, even before the next sweep.
    pub fn get(&self, voice: VoiceId, voices: &VoicePool) -> Option<&VoiceOwnership> {
        voices.get(voice)?;
        self.slots.get(voice.index() as usize)?.as_ref().filter(|owned| owned.voice == voice)
    }

    pub fn iter<'state>(&'state self, voices: &'state VoicePool) -> impl Iterator<Item = &'state VoiceOwnership> {
        self.slots.iter().flatten().filter(|owned| voices.get(owned.voice).is_some())
    }

    /// Native note-off/fade remains Background and retains identity. Retirement is only
    /// for a clear/eviction ramp, whose slot may receive no further note/effect updates.
    pub fn set_lifecycle(&mut self, voice: VoiceId, lifecycle: VoiceLifecycle, voices: &VoicePool) -> Result<(), PlaybackStateError> {
        self.get(voice, voices).ok_or(PlaybackStateError::InvalidVoice)?;
        let owned = self.slots.get_mut(voice.index() as usize).and_then(Option::as_mut).ok_or(PlaybackStateError::InvalidVoice)?;
        if owned.lifecycle == VoiceLifecycle::Retiring || (owned.lifecycle == VoiceLifecycle::Background && lifecycle == VoiceLifecycle::Foreground) {
            return Err(PlaybackStateError::InvalidOwnership);
        }
        owned.lifecycle = lifecycle;
        Ok(())
    }

    /// NO-06 sets the actual destination synchronously at admission before mixing.
    /// Generation validation covers rebuilds; neither song domain can acquire an origin.
    #[cfg(feature = "interactive-audition")]
    pub fn set_live_origin(&mut self, identity: PlaybackIdentity, voice: VoiceId, origin: LiveOrigin, voices: &VoicePool) -> Result<(), PlaybackStateError> {
        self.validate_identity(identity)?;
        let owned = self.get(voice, voices).ok_or(PlaybackStateError::InvalidVoice)?;
        if owned.domain != VoiceDomain::Live || owned.lifecycle == VoiceLifecycle::Retiring || (owned.channel.0 == 80) != (origin == LiveOrigin::ButtonPreview) {
            return Err(PlaybackStateError::InvalidOwnership);
        }
        if let Some(owned) = self.slots.get_mut(voice.index() as usize).and_then(Option::as_mut) { owned.live_origin = Some(origin); }
        Ok(())
    }

    /// Shared channel unbinding owns this transition. No pool lookup is needed: a stale
    /// generation can only update its own stale record, never a replacement voice.
    pub(crate) fn detach_foreground(&mut self, voice: VoiceId) {
        if let Some(owned) = self.slots.get_mut(voice.index() as usize).and_then(Option::as_mut)
            && owned.voice == voice && owned.lifecycle == VoiceLifecycle::Foreground
        { owned.lifecycle = VoiceLifecycle::Background; }
    }

    pub fn active_count(&self, domain: VoiceDomain, voices: &VoicePool) -> usize {
        self.iter(voices).filter(|owned| owned.domain == domain && owned.lifecycle != VoiceLifecycle::Retiring).count()
    }

    /// Deterministic oldest member, suitable for custom/live pressure and per-domain
    /// retiring fallback. Native IT's authored victim policy stays in its format runtime.
    pub fn oldest(&self, domain: VoiceDomain, retiring: bool, voices: &VoicePool) -> Option<VoiceId> {
        self.iter(voices).filter(|owned| owned.domain == domain && (owned.lifecycle == VoiceLifecycle::Retiring) == retiring)
            .min_by_key(|owned| (owned.age, owned.voice.index())).map(|owned| owned.voice)
    }

    /// Explicit slot reclamation, not key-up. Stale records are also invisible to get/iter.
    pub fn release_finished(&mut self, voices: &VoicePool) {
        for slot in self.slots.iter_mut() {
            if slot.is_some_and(|owned| voices.get(owned.voice).is_none()) { *slot = None; }
        }
    }

    #[cfg(feature = "note-overlap")]
    pub const fn note_overlap_enabled(&self) -> bool { self.note_overlap_enabled }

    #[cfg(feature = "note-overlap")]
    pub fn let_notes_ring(&self, instrument: InstrumentId) -> Option<bool> { self.let_notes_ring.get(instrument.0 as usize).copied() }

    /// Returns a scoped clear request on bypass; NO-03/04/06 must execute it with ramps.
    /// Settings alone never cut a voice, and turning on cannot recreate lost tails.
    #[cfg(feature = "note-overlap")]
    pub fn set_note_overlap_enabled(&mut self, identity: PlaybackIdentity, enabled: bool) -> Result<Option<ClearScope>, PlaybackStateError> {
        self.validate_identity(identity)?;
        if !self.configuration.capabilities.contains(PlaybackCapabilities::NOTE_OVERLAP) { return Err(PlaybackStateError::Unsupported); }
        let clear = self.note_overlap_enabled && !enabled;
        self.note_overlap_enabled = enabled;
        Ok(if clear { Some(ClearScope::CustomDetached) } else { None })
    }

    #[cfg(feature = "note-overlap")]
    pub fn set_let_notes_ring(&mut self, identity: PlaybackIdentity, instrument: InstrumentId, enabled: bool) -> Result<(), PlaybackStateError> {
        self.validate_identity(identity)?;
        let selection = self.let_notes_ring.get_mut(instrument.0 as usize).ok_or(PlaybackStateError::InvalidInstrument)?;
        *selection = enabled;
        Ok(())
    }

    /// Finalize once after all onset-tick effects, even while bypassed/unchecked/ineligible.
    /// Effect retriggers/sample swaps do not start new identities. Never infer base volume
    /// from VoiceParams: those already contain modulation and separate gains.
    #[cfg(feature = "note-overlap")]
    pub fn finalize_onset(&mut self, voice: VoiceId, kind: TriggerKind, identity: TriggerIdentity, voices: &VoicePool) -> Result<bool, PlaybackStateError> {
        if !self.configuration.capabilities.contains(PlaybackCapabilities::NOTE_OVERLAP) { return Ok(false); }
        if kind != TriggerKind::NewNote { return Ok(false); }
        if identity.instrument.0 as usize >= self.configuration.instrument_count { return Err(PlaybackStateError::InvalidInstrument); }
        let owned = self.get(voice, voices).ok_or(PlaybackStateError::InvalidVoice)?;
        if owned.lifecycle == VoiceLifecycle::Retiring || owned.channel != identity.channel { return Err(PlaybackStateError::InvalidOwnership); }
        if owned.identity.is_some() { return Err(PlaybackStateError::AlreadyFinalized); }
        if let Some(owned) = self.slots.get_mut(voice.index() as usize).and_then(Option::as_mut) { owned.identity = Some(identity); }
        Ok(true)
    }

    #[cfg(feature = "note-overlap")]
    pub fn duplicate_candidates<'state>(&'state self, incoming: VoiceId, identity: TriggerIdentity, voices: &'state VoicePool) -> impl Iterator<Item = VoiceId> + 'state {
        self.iter(voices).filter(move |owned| owned.voice != incoming && owned.lifecycle != VoiceLifecycle::Retiring && owned.identity == Some(identity)).map(|owned| owned.voice)
    }

    /// Only an actual custom detachment sets provenance. Live stays in L; song tails move
    /// from N to C. Checkbox toggles never invoke this method.
    #[cfg(feature = "note-overlap")]
    pub fn mark_custom_detached(&mut self, voice: VoiceId, voices: &VoicePool) -> Result<(), PlaybackStateError> {
        if !self.configuration.capabilities.contains(PlaybackCapabilities::NOTE_OVERLAP) { return Err(PlaybackStateError::Unsupported); }
        let owned = self.get(voice, voices).ok_or(PlaybackStateError::InvalidVoice)?;
        if owned.lifecycle != VoiceLifecycle::Foreground || (owned.domain == VoiceDomain::Native && self.configuration.capacity.custom == 0) {
            return Err(PlaybackStateError::InvalidOwnership);
        }
        if let Some(owned) = self.slots.get_mut(voice.index() as usize).and_then(Option::as_mut) {
            if owned.domain == VoiceDomain::Native { owned.domain = VoiceDomain::Custom; }
            owned.custom_detached = true;
            owned.lifecycle = VoiceLifecycle::Background;
        }
        Ok(())
    }
}
