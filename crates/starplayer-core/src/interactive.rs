//! Optional interactive playback vocabulary. Types alone do not enable a runtime.

use bitflags::bitflags;
use crate::{ChannelId, InstrumentId, Note};

/// Explicit user load identity; a reload of identical bytes starts a new session.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

/// Renderer replacement identity, including rebuilds within the same user session.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct BuildGeneration(pub u64);

/// Both identities travel atomically with an interactive command.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PlaybackIdentity {
    pub session: SessionId,
    pub build: BuildGeneration,
}

bitflags! {
    /// Capabilities configured by this host, intersected with compiled support.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
    pub struct PlaybackCapabilities: u8 {
        const NOTE_OVERLAP = 1;
        const INTERACTIVE_AUDITION = 2;
    }
}

/// Sequencing state; the interactive audio/live path runs in every state.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum InteractiveTransport {
    Playing,
    Paused,
    #[default]
    Stopped,
}

/// Sparse interactive control vocabulary. NO-03 supplies transport execution and NO-W1
/// assigns versioned wire opcodes; legacy `Command::Stop` retains its existing meaning.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InteractiveAction {
    Play,
    Pause,
    Resume,
    /// Clear voices/DSP and hold the cursor, accepting new live notes afterward.
    Stop,
    AllSoundOff,
    StopPreview,
    SetLiveDestination(LiveDestination),
    PreviewInstrument(InstrumentId),
    ReleasePreview,
    SetNoteOverlapEnabled(bool),
    SetLetNotesRing { instrument: InstrumentId, enabled: bool },
}

/// Mutually exclusive input routing. All destinations share the same sixteen lanes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum LiveDestination { #[default] Off, Jam, Selected(InstrumentId) }

/// No generation-prefix protocol: identity and action form one indivisible record.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InteractiveCommand {
    pub identity: PlaybackIdentity,
    pub action: InteractiveAction,
}

/// Original musical input, before transposition, portamento or later pitch effects.
/// MOD periods remain exact: different periods must not collapse to a nearest note.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OriginalNote {
    Note(Note),
    ModPeriod(u16),
}

/// Immutable identity finalized after the channel's actual onset-tick effects.
///
/// `onset_volume` is the exact native base field, with no identity-only scaling:
/// MOD/MTM/S3M `current_volume`, IT `note_volume`, XM `real_volume`, or the
/// live format's resolved velocity/default base volume before CC7. Simple PCM live
/// input uses the existing resolved Q0.16 bits. Different native domains never mix
/// within one instrument. Tremolo/tremor/envelopes/random swing and separate gains
/// are excluded. Later effects and key-off never rewrite this record.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TriggerIdentity {
    pub instrument: InstrumentId,
    pub note: OriginalNote,
    pub channel: ChannelId,
    pub onset_volume: u16,
}

/// Classification belongs to the format's musical seam, not the pool allocator.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TriggerKind {
    /// Includes a delayed note only when it actually starts.
    NewNote,
    EffectRetrigger,
    SampleReplacement,
}

/// Logical sequencer cursor for a renderer rebuild. Sound and processor working state
/// are rebuilt separately; restoring this record never dispatches a musical tick.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SequencerCursor {
    pub order: u16,
    pub row: u16,
    pub row_clock: crate::RowClock,
    pub sounding_order: u16,
    pub sounding_pattern: u16,
    pub sounding_row: u16,
    pub sounding_tick: u16,
    pub tempo_bpm: u16,
    pub requested_speed: u8,
    pub arrival: RowArrival,
    pub detector_state: (bool, u32, bool),
    pub pending_jump: Option<(Option<u16>, Option<u16>, bool)>,
    pub remaining_frames: u64,
    pub fractional_remainder: u64,
    pub song_frame: u64,
    pub captured_frame: crate::Frame,
    pub sample_rate_hz: u32,
    pub sequence_ended: bool,
    pub end_reached: bool,
}

impl SequencerCursor {
    /// Convert time, including the pending subframe phase, instead of copying old-rate
    /// frame counts into a replacement renderer. Current tempo uses its native model.
    pub fn converted(mut self, sample_rate_hz: u32) -> Self {
        let old_rate = self.sample_rate_hz.max(1) as u128;
        let pending = (((self.remaining_frames as u128) << 32) + self.fractional_remainder as u128)
            .saturating_mul(sample_rate_hz as u128) / old_rate;
        self.remaining_frames = (pending >> 32).min(u64::MAX as u128) as u64;
        self.fractional_remainder = (pending & 0xffff_ffff) as u64;
        self.song_frame = (self.song_frame as u128 * sample_rate_hz as u128 / old_rate).min(u64::MAX as u128) as u64;
        self.sample_rate_hz = sample_rate_hz;
        self
    }
}

/// How sequencing arrived at the current row, retained across renderer replacement.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum RowArrival {
    /// The song was just started, or seeked. Nothing before this row is continuous with it.
    #[default]
    Start,
    /// The next row of the same pattern.
    Sequential,
    /// The first row of the next order, because the pattern ran out.
    NextOrder,
    /// A `Bxx` / `Cxx` / `Dxx` jump through the order list.
    Jump,
    /// An `SBx` / `E6x` pattern loop, back to a row of the pattern already playing.
    PatternLoop,
    /// The order list ran out and `EndOfSongPolicy::Loop`
    /// restarted it.
    ///
    /// **This is the end of the song, not a loop point.** Running off the end of the order
    /// list is not something the music asks for — it is what is left when the music stops
    /// asking for anything — so it ends the pass whatever row the restart order names and
    /// whether or not that row has been played (task D2).
    Wrapped,
}

/// Sound-free MIDI channel state used when replacing an interactive renderer.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MidiControllerState {
    pub program: InstrumentId,
    pub volume: crate::U0F16,
    pub pan: Option<crate::I1F15>,
    pub bend: crate::I1F15,
    pub sustain: bool,
}

/// Exact next-event cursor; already consumed events are never replayed on restore.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SmfCursor {
    pub event_index: u64,
    pub song_frame: u64,
    pub remaining_frames: Option<u64>,
    pub captured_frame: crate::Frame,
    pub sample_rate_hz: u32,
}
impl SmfCursor {
    pub fn converted(mut self, sample_rate_hz: u32) -> Self {
        let old_rate = self.sample_rate_hz.max(1) as u128;
        self.song_frame = (self.song_frame as u128 * sample_rate_hz as u128 / old_rate).min(u64::MAX as u128) as u64;
        self.remaining_frames = self.remaining_frames.map(|frames| (frames as u128 * sample_rate_hz as u128 / old_rate).min(u64::MAX as u128) as u64);
        self.sample_rate_hz = sample_rate_hz;
        self
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SmfPlaybackState {
    pub cursor: SmfCursor,
    pub channels: [MidiControllerState; 16],
}
