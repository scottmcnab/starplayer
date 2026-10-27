//! Prepared interactive layout/capacity contracts. Legacy constructors remain unchanged.

use starplayer_core::{ChannelId, PlaybackCapabilities, PlaybackIdentity};

pub const INTERACTIVE_LIVE_BASE: u16 = 64;
pub const INTERACTIVE_LIVE_LANES: usize = 17;
pub const INTERACTIVE_PREVIEW_LANE: ChannelId = ChannelId(80);
pub const MAX_INTERACTIVE_CHANNELS: usize = 81;
pub const WEB_CUSTOM_TAILS: usize = 64;
pub const WEB_LIVE_VOICES: usize = 16;
pub const MAX_INTERACTIVE_VOICE_CAPACITY: usize = 256 + 64 + 16 + 3;

/// Layout/configuration rejection, before a renderer accepts input.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlaybackConfigError {
    UnsupportedTrackerWidth,
    UnsupportedLane,
    FeatureUnavailable,
    InvalidCapacity,
    InvalidInstrumentCount,
    AllocationFailed,
}

/// Explicit address mapping. Hardware MIDI channel numbers remain 0–15.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LaneLayout {
    tracker_channels: usize,
    lane_count: usize,
    live_base: u16,
    interactive: bool,
}

impl LaneLayout {
    /// Historical 64-lane table, including the historical 48-based MIDI overlap.
    pub const fn legacy(count: usize) -> Self {
        let count = if count > 64 { 64 } else { count };
        Self { tracker_channels: count, lane_count: count, live_base: 48, interactive: false }
    }

    /// 0–64 tracker lanes and fixed live lanes 64–80; no clamping or wrapping.
    pub fn interactive(tracker_channels: usize) -> Result<Self, PlaybackConfigError> {
        if !cfg!(feature = "interactive-audition") { return Err(PlaybackConfigError::FeatureUnavailable); }
        if tracker_channels > 64 { return Err(PlaybackConfigError::UnsupportedTrackerWidth); }
        Ok(Self { tracker_channels, lane_count: MAX_INTERACTIVE_CHANNELS, live_base: INTERACTIVE_LIVE_BASE, interactive: true })
    }

    pub const fn tracker_channels(self) -> usize { self.tracker_channels }
    pub const fn lane_count(self) -> usize { self.lane_count }
    pub const fn is_interactive(self) -> bool { self.interactive }
    pub const fn live_base(self) -> usize { self.live_base as usize }
    pub const fn preview_lane(self) -> Option<ChannelId> { if self.interactive { Some(INTERACTIVE_PREVIEW_LANE) } else { None } }

    /// Strict mapping for new hosts. The legacy public midi_channel helper still wraps.
    pub const fn midi_lane(self, midi_channel: u8) -> Option<ChannelId> {
        if midi_channel >= 16 { None } else { Some(ChannelId(self.live_base + midi_channel as u16)) }
    }

    pub const fn midi_index(self, lane: ChannelId) -> Option<usize> {
        if lane.0 < self.live_base || lane.0 >= self.live_base + 16 { None } else { Some((lane.0 - self.live_base) as usize) }
    }
}

/// Features compiled into the engine. Hosts must opt in separately for each build.
pub fn compiled_playback_capabilities() -> PlaybackCapabilities {
    let mut result = PlaybackCapabilities::empty();
    if cfg!(feature = "note-overlap") { result.insert(PlaybackCapabilities::NOTE_OVERLAP); }
    if cfg!(feature = "interactive-audition") { result.insert(PlaybackCapabilities::INTERACTIVE_AUDITION); }
    result
}

/// Active admission budgets; physical capacity adds one reserve per enabled domain.
/// Native IT keeps its authored 256-voice limit, including its backgrounds. Live native
/// IT backgrounds use `live`, never `native`; custom-detached live voices also use live.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct VoiceCapacity {
    pub native: usize,
    pub custom: usize,
    pub live: usize,
    pub interactive_transport: bool,
}

impl VoiceCapacity {
    pub const fn active(self) -> usize { self.native.saturating_add(self.custom).saturating_add(self.live) }
    pub const fn retirement(self) -> usize { self.interactive_transport as usize + (self.custom != 0) as usize + (self.live != 0) as usize }
    pub const fn physical(self) -> usize { self.active().saturating_add(self.retirement()) }
    pub const fn web() -> Self { Self { native: 256, custom: WEB_CUSTOM_TAILS, live: WEB_LIVE_VOICES, interactive_transport: true } }

    pub fn validate(self, capabilities: PlaybackCapabilities) -> Result<(), PlaybackConfigError> {
        if self.native == 0 || self.physical() > u16::MAX as usize { return Err(PlaybackConfigError::InvalidCapacity); }
        if self.custom != 0 && !capabilities.contains(PlaybackCapabilities::NOTE_OVERLAP) { return Err(PlaybackConfigError::InvalidCapacity); }
        if (self.live != 0 || self.interactive_transport) && !capabilities.contains(PlaybackCapabilities::INTERACTIVE_AUDITION) { return Err(PlaybackConfigError::InvalidCapacity); }
        Ok(())
    }
}

/// One prepared engine build. Settings/selections belong to the session, not Module.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlaybackConfiguration {
    pub identity: PlaybackIdentity,
    pub capabilities: PlaybackCapabilities,
    pub instrument_count: usize,
    pub capacity: VoiceCapacity,
}

impl PlaybackConfiguration {
    pub fn validate(self) -> Result<(), PlaybackConfigError> {
        if !compiled_playback_capabilities().contains(self.capabilities) { return Err(PlaybackConfigError::FeatureUnavailable); }
        if self.instrument_count > u16::MAX as usize + 1 { return Err(PlaybackConfigError::InvalidInstrumentCount); }
        self.capacity.validate(self.capabilities)
    }
}
