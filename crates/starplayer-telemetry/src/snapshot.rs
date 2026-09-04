//! [`Snapshot`] and the plain-data types it is made of: what a UI renders one frame from.

use starplayer_core::{I1F15, Note, U0F16};

/// Channels a [`Snapshot`] carries, always.
///
/// **Fixed at 64, not a const generic** (M1-B6, research point 2). 64 is IT's pattern
/// channel count and therefore the widest of any format in scope — it is the same bound
/// as [`ChannelTable::MAX_CHANNELS`](https://docs.rs/starplayer-engine), which the engine
/// clamps to. A const generic would be marginally tidier in memory and would then appear
/// in the signature of **every** function in every UI that touches a snapshot, for a
/// saving of at most a couple of kilobytes on a type that exists to be memcpy'd once per
/// tick. [`Snapshot::channel_count`] says how many of the 64 the module actually uses, so
/// a UI never has to guess.
pub const MAX_CHANNELS: usize = 64;

/// Where the song is — the fields the original's status line reads.
///
/// `order`, `pattern`, `row` and `tick` are the **`_MActual*` snapshot**: they are
/// latched at the top of the row that is *currently sounding*, not the row being parsed
/// (`plans/reference/original-star-ui.md` §2.1). `speed` and `tempo_bpm` are the live
/// `_MCurrentSpd` / `_MCurrentBPM`, which is what the original displays for those two.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TransportState {
    /// Index into the order list — the original's `_MActualPos`, and what its
    /// `pattern:` field actually shows.
    pub order: u16,
    /// The pattern that order entry names (`_MActualPatt`).
    pub pattern: u16,
    /// Row within that pattern (`_MActualRow`).
    pub row: u16,
    /// Tick within the row, counting **up** from 0 and absolute across pattern-delay
    /// repeats, matching [`RowClock::tick_in_row`](starplayer_core::RowClock). The
    /// original's `_MActualTick` counts down; a modern UI wants the up-count.
    pub tick: u16,
    /// Ticks per row in effect (`_MCurrentSpd`, `Axx`).
    pub speed: u8,
    /// Tempo in effect (`_MCurrentBPM`, `Txx`).
    pub tempo_bpm: u16,
    /// The module's global volume (`Vxx`), **not** the host's master volume.
    pub global_volume: U0F16,
    /// Frames into the current pass through the song — the elapsed position a media
    /// player's progress slider is drawn from. Zero when no song timeline is installed.
    pub song_frame: u64,
    /// Frames in one pass, from the scanned song timeline. Zero when there is none, which
    /// is how a UI tells "no total to show" from "at the start".
    pub song_length_frames: u64,
    /// What the scan found at the end of the song.
    pub song_end: SongEnd,
    /// Whether the song has been heard through once — its detected loop point, or the end
    /// of its order list. A one-tick pulse when the host asked to keep playing, sticky
    /// otherwise.
    pub end_reached: bool,
}

/// What a scanned song does when it gets to the end of itself.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum SongEnd {
    /// Nothing has been scanned, so the length and the end are both unknown.
    #[default]
    Unknown,
    /// The song jumps back into music it has already played — a `Bxx`/`Cxx`/`Dxx` — and
    /// repeats from there for ever.
    Loops,
    /// The song ends: the order list runs out, or a stop marker fires.
    Stops,
}

impl TransportState {
    /// Nothing playing: no position, no speed, no tempo, and the global volume a module
    /// starts at.
    pub const IDLE: TransportState = TransportState {
        order: 0,
        pattern: 0,
        row: 0,
        tick: 0,
        speed: 0,
        tempo_bpm: 0,
        global_volume: U0F16::MAX,
        song_frame: 0,
        song_length_frames: 0,
        song_end: SongEnd::Unknown,
        end_reached: false,
    };
}

impl Default for TransportState {
    fn default() -> TransportState { TransportState::IDLE }
}

/// One channel's effect column, as the original's status line spelled it.
///
/// The original carried `_CMDVal` and `_CMDData` in `ChannelData` marked "for host
/// program" — they exist purely to feed the UI — and rendered them through a table of
/// English names rather than as raw hex
/// (`plans/reference/original-star-ui.md` §2.3).
///
/// # Why the name arrives from outside
///
/// The name table is per format, because the same letter means different things in
/// different formats, and it lives in
/// [`starplayer_model::EffectNames`](https://docs.rs/starplayer-model) next to the
/// display-only `PatternCell` that shares it. This crate's allowed dependency edges are
/// `starplayer-core` and `starplayer-rt` only, so it cannot see that table and does not
/// duplicate it: a format's effect processor resolves the name and reports the whole
/// [`EffectDisplay`], which is the same crate that knows what the bytes meant in the
/// first place.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct EffectDisplay {
    /// The format's own command code — for S3M, 1..=26 for `A`..`Z`. Zero is an empty
    /// effect column.
    pub code: u8,
    /// The command's parameter byte.
    pub param: u8,
    /// The effect spelled out in English, or `""` when the format has no name for it.
    pub name: &'static str,
}

impl EffectDisplay {
    /// An empty effect column.
    pub const NONE: EffectDisplay = EffectDisplay { code: 0, param: 0, name: "" };

    /// A command and its parameter, with no name resolved yet.
    pub const fn raw(code: u8, param: u8) -> EffectDisplay { EffectDisplay { code, param, name: "" } }

    /// The same command, with its English name attached.
    pub const fn with_name(self, name: &'static str) -> EffectDisplay {
        EffectDisplay { code: self.code, param: self.param, name }
    }

    /// Whether the column is empty.
    pub const fn is_empty(self) -> bool { self.code == 0 }
}

/// One channel, as a UI draws it: the original's channel row.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ChannelState {
    /// The note the channel is sounding, or the last one it sounded (`_CurrentNote`).
    /// **Sticky**: a channel that has gone silent keeps showing the note it played, which
    /// is what the original does and what a tracker display wants.
    pub note: Option<Note>,
    /// The instrument that note came from (`_SampleNum`), one-based as the file numbers
    /// it. Sticky, like `note`.
    pub instrument: u8,
    /// Channel volume (`_CurrentVol`). Sticky.
    pub volume: U0F16,
    /// Pan: −1.0 hard left, 0 centre, +1.0 hard right (`_PanPosition`). Sticky.
    pub pan: I1F15,
    /// The row's effect column (`_CMDVal` / `_CMDData`), cleared at the top of each row.
    pub effect: EffectDisplay,
    /// Peak-hold VU level — see [`VuMeter`](crate::VuMeter).
    ///
    /// This is scalar state riding in the coherent snapshot because a UI wants it in M1;
    /// architecture §9 moves it to the lossy audio taps in M3 when the scope rings
    /// arrive.
    pub vu_level: U0F16,
    /// Whether a voice is still sounding on this channel — the original's `_ActiveFlag`.
    pub active: bool,
    /// Whether the host has muted the lane.
    pub muted: bool,
}

impl ChannelState {
    /// A channel that has never played anything.
    pub const SILENT: ChannelState = ChannelState {
        note: None,
        instrument: 0,
        volume: U0F16::ZERO,
        pan: I1F15::ZERO,
        effect: EffectDisplay::NONE,
        vu_level: U0F16::ZERO,
        active: false,
        muted: false,
    };
}

/// What the render loop wants the host to know, mirrored out of the engine's
/// `EngineWarnings`.
///
/// Sticky on the engine side and therefore sticky here: a flag stays raised in every
/// snapshot until the host clears it on the engine. The two types are deliberately
/// separate because this crate cannot depend on `starplayer-engine` — the edge runs the
/// other way.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WarningFlags {
    /// A source kept reporting the same frame and the engine forced the clock forward
    /// (architecture §3.1 rule 2).
    pub zero_advance_forced: bool,
    /// A source produced more events in one render quantum than the engine will dispatch.
    pub event_limit_reached: bool,
    /// A retired module handle was dropped on the audio thread because the garbage
    /// channel was full. A host bug, not a module bug.
    pub retired_module_dropped: bool,
    /// A command arrived that this milestone does not act on yet.
    pub unsupported_command: bool,
    /// An external event was stamped for a frame that had already passed and dispatched at
    /// the current one. The host's event lead is too short.
    pub late_events: bool,
}

impl WarningFlags {
    /// Nothing flagged.
    pub const NONE: WarningFlags = WarningFlags {
        zero_advance_forced: false,
        event_limit_reached: false,
        retired_module_dropped: false,
        unsupported_command: false,
        late_events: false,
    };

    /// Whether anything has been flagged.
    pub const fn any(self) -> bool {
        self.zero_advance_forced
            || self.event_limit_reached
            || self.retired_module_dropped
            || self.unsupported_command
            || self.late_events
    }
}

/// One internally consistent frame of engine state.
///
/// Every field comes from the same tracker tick. That is the whole point of the type: a
/// row number from one tick paired with a note from the next renders wrong, so the
/// snapshot is built in the publisher and moved through
/// [`SnapshotPublisher`](starplayer_rt::SnapshotPublisher) in one piece.
///
/// `Copy`, `Default`, and entirely free of heap: 2600 bytes on a 64-bit host, where the
/// `&'static str` in each [`EffectDisplay`] is 16 of the 40 bytes a [`ChannelState`] costs;
/// noticeably less on `wasm32`, where a fat pointer is half the width.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Snapshot {
    /// Increments once per published snapshot, starting at 1.
    ///
    /// A reader compares it with the previous one to tell "nothing new since I last
    /// looked" from "the writer moved on"; a **gap** larger than one means publishes were
    /// dropped between the two reads.
    pub sequence: u64,
    /// How many publishes had been dropped when this snapshot was written, cumulative
    /// (see [`SnapshotPublisher::publishes_dropped`](starplayer_rt::SnapshotPublisher::publishes_dropped)).
    pub publishes_dropped: u32,
    /// How many of [`Snapshot::channels`] the module actually uses. The rest are
    /// [`ChannelState::SILENT`].
    pub channel_count: u8,
    /// Voices sounding in the global pool.
    pub voices_active: u16,
    /// Where the song is.
    pub transport: TransportState,
    /// What the render loop wants the host to know.
    pub warnings: WarningFlags,
    /// Every channel, always [`MAX_CHANNELS`] of them.
    pub channels: [ChannelState; MAX_CHANNELS],
}

impl Snapshot {
    /// Nothing has played yet.
    pub const IDLE: Snapshot = Snapshot {
        sequence: 0,
        publishes_dropped: 0,
        channel_count: 0,
        voices_active: 0,
        transport: TransportState::IDLE,
        warnings: WarningFlags::NONE,
        channels: [ChannelState::SILENT; MAX_CHANNELS],
    };

    /// One channel, or `None` past [`MAX_CHANNELS`].
    pub fn channel(&self, index: usize) -> Option<&ChannelState> { self.channels.get(index) }

    /// Just the channels the module uses, in order. What a UI iterates.
    pub fn active_channels(&self) -> &[ChannelState] {
        let used = (self.channel_count as usize).min(MAX_CHANNELS);
        self.channels.get(..used).unwrap_or(&[])
    }

    /// Whether anything has ever been published into this snapshot.
    pub const fn is_idle(&self) -> bool { self.sequence == 0 }
}

impl Default for Snapshot {
    fn default() -> Snapshot { Snapshot::IDLE }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idle_snapshot_is_silent_everywhere() {
        let snapshot = Snapshot::default();
        assert!(snapshot.is_idle());
        assert_eq!(snapshot.channels.len(), MAX_CHANNELS);
        assert_eq!(snapshot.active_channels(), &[] as &[ChannelState], "no module, no channels to draw");
        assert_eq!(snapshot.transport.global_volume, U0F16::MAX, "a module starts at full global volume");
        assert_eq!(snapshot.transport.song_end, SongEnd::Unknown, "nothing has been scanned");
        assert_eq!((snapshot.transport.song_frame, snapshot.transport.song_length_frames), (0, 0));
        assert!(!snapshot.transport.end_reached);
        assert!(!snapshot.warnings.any());
        assert!(snapshot.channels.iter().all(|channel| *channel == ChannelState::SILENT));
    }

    #[test]
    fn active_channels_is_bounded_by_the_array_however_absurd_the_count() {
        let mut snapshot = Snapshot { channel_count: 200, ..Snapshot::default() };
        assert_eq!(snapshot.active_channels().len(), MAX_CHANNELS, "a corrupt count cannot index past the array");
        snapshot.channel_count = 4;
        assert_eq!(snapshot.active_channels().len(), 4);
        assert!(snapshot.channel(MAX_CHANNELS).is_none());
    }

    #[test]
    fn an_effect_display_carries_the_raw_command_and_its_english_name() {
        assert_eq!(EffectDisplay::raw(1, 0x06), EffectDisplay { code: 1, param: 0x06, name: "" });
        assert_eq!(EffectDisplay::raw(1, 0x06).with_name("change speed").name, "change speed");
        assert!(EffectDisplay::NONE.is_empty());
        assert!(!EffectDisplay::raw(1, 0).is_empty());
    }

    #[test]
    fn warning_flags_mirror_the_engines_four() {
        assert!(!WarningFlags::NONE.any());
        assert!(WarningFlags { zero_advance_forced: true, ..WarningFlags::NONE }.any());
        assert!(WarningFlags { unsupported_command: true, ..WarningFlags::NONE }.any());
    }
}
