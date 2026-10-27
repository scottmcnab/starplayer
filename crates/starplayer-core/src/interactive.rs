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
    SetNoteOverlapEnabled(bool),
    SetLetNotesRing { instrument: InstrumentId, enabled: bool },
}

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
/// MOD/MTM/S3M `current_volume`, IT `note_volume`, XM's base field (NO-05), or the
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
