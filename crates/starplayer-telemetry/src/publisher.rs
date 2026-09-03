//! [`TelemetryPublisher`] and [`TelemetryReader`] — the audio thread's side and the UI's.

use starplayer_core::{ChannelId, I1F15, Note, U0F16, VoiceId};
use starplayer_rt::{DEFAULT_SNAPSHOT_DEPTH, SnapshotPublisher, SnapshotReader, snapshot_channel};

use crate::snapshot::{EffectDisplay, MAX_CHANNELS, Snapshot, SongEnd, TransportState, WarningFlags};
use crate::vu::VuMeter;

/// One channel's live state for one tick, as the engine reads it off the voice pool.
///
/// Every field but `voice` and `muted` is an `Option` meaning **"unchanged"**, because
/// the display fields are sticky: the original's `_CurrentNote`, `_CurrentVol` and
/// `_PanPosition` keep their last values after a voice ends, and a channel row that blanked
/// itself the instant a one-shot sample finished would flicker.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ChannelUpdate {
    /// The voice this channel is sounding, or `None` for a silent channel.
    ///
    /// This is both the original's `_ActiveFlag` and the retrigger signal: a *different*
    /// handle from last tick's is a new note, which is what strikes the VU meter. It is
    /// more reliable than watching [`DirtyBits::SAMPLE`](starplayer_core::DirtyBits) for
    /// that, because a processor is free to build a fresh
    /// [`VoiceParams`](starplayer_core::VoiceParams) with no dirty bits set at all and
    /// hand it straight to `ChannelTable::trigger`.
    pub voice: Option<VoiceId>,
    /// The note the voice is playing, or `None` to keep the last one.
    pub note: Option<Note>,
    /// The instrument it came from, or `None` to keep the last one.
    pub instrument: Option<u8>,
    /// Channel volume, or `None` to keep the last one.
    pub volume: Option<U0F16>,
    /// Pan, or `None` to keep the last one.
    pub pan: Option<I1F15>,
    /// A volume write landed on this tick — [`DirtyBits::VOLUME`](starplayer_core::DirtyBits)
    /// on the foreground voice. The other signal that strikes the VU meter.
    pub volume_written: bool,
    /// Whether the host has muted the lane.
    pub muted: bool,
}

impl ChannelUpdate {
    /// A channel with nothing sounding. Every display field keeps its last value.
    pub const fn silent(muted: bool) -> ChannelUpdate {
        ChannelUpdate { voice: None, note: None, instrument: None, volume: None, pan: None, volume_written: false, muted }
    }
}

/// Create a telemetry channel and split it into its two halves.
///
/// The publisher goes to the audio thread, the reader to whoever draws. `depth` is how
/// many snapshots may be in flight; [`DEFAULT_SNAPSHOT_DEPTH`] is the sensible answer and
/// what [`telemetry_channel`] uses.
///
/// **Every allocation happens here.** Both halves are allocation-free afterwards, which
/// is what makes [`TelemetryPublisher::publish`] safe to call from `render()`.
pub fn telemetry_channel_with_depth(depth: usize) -> (TelemetryPublisher, TelemetryReader) {
    let (publisher, reader) = snapshot_channel(depth, Snapshot::IDLE);
    let publisher = TelemetryPublisher {
        working: Snapshot::IDLE,
        meters: [VuMeter::SILENT; MAX_CHANNELS],
        sounding: [None; MAX_CHANNELS],
        publisher,
    };
    (publisher, TelemetryReader { reader })
}

/// Create a telemetry channel [`DEFAULT_SNAPSHOT_DEPTH`] deep.
pub fn telemetry_channel() -> (TelemetryPublisher, TelemetryReader) {
    telemetry_channel_with_depth(DEFAULT_SNAPSHOT_DEPTH)
}

/// The audio thread's side: accumulate a tick's worth of state, then publish it whole.
///
/// # Real-time safety
///
/// Nothing here allocates, locks, or panics. The working snapshot is an inline `Copy`
/// value owned by this struct; every per-channel write goes through `slice::get_mut` and
/// silently ignores an out-of-range channel rather than indexing; and
/// [`TelemetryPublisher::publish`] is one `memcpy` into a ring allocated at construction.
/// The `assert_no_alloc` hook architecture §8 plans for M2-C7 will cover this path along
/// with the rest of `render()`; until then the property is held by inspection.
///
/// # Cadence: once per tick, not once per quantum
///
/// The engine publishes from the sequencer's dispatch, immediately after the tick's
/// outcome has been committed. That is the `_MActual*` cadence — the original latched its
/// display snapshot in `__UpdateTracker`, at tick rate — and it is the only cadence at
/// which the numbers are all from the same moment. Publishing per render quantum instead
/// (~345 Hz at 44.1 kHz) would republish an unchanged snapshot six times a tick and pay
/// the copy for it; publishing per host block would make the telemetry rate depend on the
/// host's buffer size, which is exactly the coupling design goal 3 exists to prevent.
pub struct TelemetryPublisher {
    working: Snapshot,
    meters: [VuMeter; MAX_CHANNELS],
    /// Last tick's foreground voice per channel, so a retrigger is detectable.
    sounding: [Option<VoiceId>; MAX_CHANNELS],
    publisher: SnapshotPublisher<Snapshot>,
}

impl TelemetryPublisher {
    /// Latch the `_MActual*` position: the order, pattern, row and tick currently
    /// **sounding**.
    pub fn set_position(&mut self, order: u16, pattern: u16, row: u16, tick: u16) {
        self.working.transport.order = order;
        self.working.transport.pattern = pattern;
        self.working.transport.row = row;
        self.working.transport.tick = tick;
    }

    /// Speed and tempo in effect (`_MCurrentSpd` / `_MCurrentBPM`).
    pub fn set_timing(&mut self, speed: u8, tempo_bpm: u16) {
        self.working.transport.speed = speed;
        self.working.transport.tempo_bpm = tempo_bpm;
    }

    /// The song clock: elapsed frames into this pass, the length of a pass, how the song
    /// ends, and whether the loop point has been passed.
    ///
    /// All four come from the sequencer's scanned [`SongTimeline`](https://docs.rs/starplayer-engine);
    /// with no timeline installed they are zero, zero, [`SongEnd::Unknown`] and false.
    pub fn set_song_clock(&mut self, song_frame: u64, song_length_frames: u64, song_end: SongEnd, end_reached: bool) {
        self.working.transport.song_frame = song_frame;
        self.working.transport.song_length_frames = song_length_frames;
        self.working.transport.song_end = song_end;
        self.working.transport.end_reached = end_reached;
    }

    /// The module's global volume (`Vxx`).
    pub fn set_global_volume(&mut self, global_volume: U0F16) {
        self.working.transport.global_volume = global_volume;
    }

    /// Mirror the engine's sticky warnings.
    pub fn set_warnings(&mut self, warnings: WarningFlags) { self.working.warnings = warnings; }

    /// How many voices are sounding in the global pool.
    pub fn set_voices_active(&mut self, voices_active: u16) { self.working.voices_active = voices_active; }

    /// How many channels the module uses, clamped to [`MAX_CHANNELS`].
    pub fn set_channel_count(&mut self, channel_count: u8) {
        self.working.channel_count = channel_count.min(MAX_CHANNELS as u8);
    }

    /// Everything the transport currently reads.
    pub const fn transport(&self) -> TransportState { self.working.transport }

    /// The snapshot as it stands, before the next publish.
    pub const fn working(&self) -> &Snapshot { &self.working }

    /// Blank every channel's effect column. The sequencer calls this at the top of each
    /// row, so a row whose channel has no effect shows none rather than the previous
    /// row's.
    pub fn clear_effects(&mut self) {
        for channel in self.working.channels.iter_mut() {
            channel.effect = EffectDisplay::NONE;
        }
    }

    /// Record the row's effect column for `channel` — `_CMDVal` / `_CMDData` plus the
    /// English name the format resolved.
    pub fn report_effect(&mut self, channel: ChannelId, effect: EffectDisplay) {
        if let Some(state) = self.working.channels.get_mut(channel.0 as usize) {
            state.effect = effect;
        }
    }

    /// Record the row's note and instrument columns for `channel`, overriding what the
    /// voice pool says.
    ///
    /// The engine already derives both from the sounding voice's
    /// [`VoiceTag`](https://docs.rs/starplayer-mixer), so a format only needs this where
    /// the two differ — a note that was parsed but delayed by `SDx`, or a portamento
    /// target the voice has not reached.
    pub fn report_note(&mut self, channel: ChannelId, note: Option<Note>, instrument: Option<u8>) {
        if let Some(state) = self.working.channels.get_mut(channel.0 as usize) {
            if let Some(note) = note {
                state.note = Some(note);
            }
            if let Some(instrument) = instrument {
                state.instrument = instrument;
            }
        }
    }

    /// Fold one tick of live channel state in, and run that channel's VU meter.
    ///
    /// The VU rule, from the original: **strike** — hold at the channel volume — on a new
    /// note or a volume write, and otherwise **decay** by
    /// [`VuMeter::DECAY_PER_TICK`], clamped at zero.
    pub fn update_channel(&mut self, channel: ChannelId, update: ChannelUpdate) {
        let index = channel.0 as usize;
        let (Some(state), Some(meter), Some(sounding)) =
            (self.working.channels.get_mut(index), self.meters.get_mut(index), self.sounding.get_mut(index))
        else {
            return;
        };

        let retriggered = update.voice.is_some() && update.voice != *sounding;
        *sounding = update.voice;

        state.active = update.voice.is_some();
        state.muted = update.muted;
        if let Some(note) = update.note {
            state.note = Some(note);
        }
        if let Some(instrument) = update.instrument {
            state.instrument = instrument;
        }
        if let Some(volume) = update.volume {
            state.volume = volume;
        }
        if let Some(pan) = update.pan {
            state.pan = pan;
        }

        if retriggered || update.volume_written {
            meter.strike(state.volume);
        } else {
            meter.decay();
        }
        state.vu_level = meter.level();
    }

    /// Publish the accumulated snapshot.
    ///
    /// Stamps the next [`Snapshot::sequence`] and the running dropped-publish count, then
    /// hands the whole value to the ring. Returns whether the reader had room for it;
    /// a `false` is a stalled reader, not an error, and the next tick publishes again.
    ///
    /// [`Snapshot::sequence`] advances whether or not the ring accepted the snapshot, so
    /// a reader that sees a gap larger than one between consecutive reads knows exactly
    /// how many frames it missed.
    pub fn publish(&mut self) -> bool {
        self.working.sequence = self.working.sequence.saturating_add(1);
        self.working.publishes_dropped = self.publisher.publishes_dropped();
        self.publisher.publish(self.working)
    }

    /// How many publishes have been dropped because the reader was behind.
    pub const fn publishes_dropped(&self) -> u32 { self.publisher.publishes_dropped() }
}

impl core::fmt::Debug for TelemetryPublisher {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("TelemetryPublisher")
            .field("sequence", &self.working.sequence)
            .field("transport", &self.working.transport)
            .field("publishes_dropped", &self.publishes_dropped())
            .finish()
    }
}

/// The UI's side: poll for the newest coherent snapshot.
///
/// Cheap to poll at frame rate and cheap to poll faster: with nothing new published
/// [`TelemetryReader::read`] hands back the previous snapshot unchanged.
#[derive(Debug)]
pub struct TelemetryReader {
    reader: SnapshotReader<Snapshot>,
}

impl TelemetryReader {
    /// Drain to the newest published snapshot and return it.
    pub fn read(&mut self) -> &Snapshot { self.reader.read() }

    /// The last snapshot [`TelemetryReader::read`] returned, without draining.
    pub const fn latest(&self) -> &Snapshot { self.reader.latest() }

    /// Whether a newer snapshot is waiting.
    pub fn has_pending(&self) -> bool { self.reader.has_pending() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `VoiceId`'s fields are private and only a `VoicePool` mints one, which this crate
    /// cannot see. `VoiceId: Default` gives the one handle these tests need: the retrigger
    /// signal is "different from last tick's", and going from `None` to `Some` is exactly
    /// that.
    fn a_voice() -> VoiceId { VoiceId::default() }

    fn sounding(volume: U0F16) -> ChannelUpdate {
        ChannelUpdate {
            voice: Some(a_voice()),
            note: Some(Note::MIDDLE_C),
            instrument: Some(1),
            volume: Some(volume),
            pan: Some(I1F15::ZERO),
            volume_written: false,
            muted: false,
        }
    }

    #[test]
    fn a_publish_stamps_a_sequence_and_reaches_the_reader_whole() {
        let (mut publisher, mut reader) = telemetry_channel();
        assert!(reader.latest().is_idle());

        publisher.set_position(2, 7, 13, 4);
        publisher.set_timing(6, 125);
        publisher.set_song_clock(88_200, 1_764_000, SongEnd::Loops, false);
        publisher.set_channel_count(4);
        publisher.set_voices_active(3);
        assert!(publisher.publish());

        let snapshot = reader.read();
        assert_eq!(snapshot.sequence, 1);
        assert_eq!(
            snapshot.transport,
            TransportState {
                order: 2,
                pattern: 7,
                row: 13,
                tick: 4,
                speed: 6,
                tempo_bpm: 125,
                global_volume: U0F16::MAX,
                song_frame: 88_200,
                song_length_frames: 1_764_000,
                song_end: SongEnd::Loops,
                end_reached: false,
            }
        );
        assert_eq!(snapshot.channel_count, 4);
        assert_eq!(snapshot.voices_active, 3);
        assert_eq!(snapshot.active_channels().len(), 4);
    }

    #[test]
    fn a_new_note_strikes_the_vu_meter_and_silence_decays_it() {
        let (mut publisher, _reader) = telemetry_channel();
        publisher.set_channel_count(1);

        publisher.update_channel(ChannelId(0), sounding(U0F16::MAX));
        assert_eq!(publisher.working().channels[0].vu_level, U0F16::MAX, "a fresh voice handle is a new note");
        assert!(publisher.working().channels[0].active);

        // The same handle on the next tick is the same note still sounding: no strike.
        publisher.update_channel(ChannelId(0), sounding(U0F16::MAX));
        assert_eq!(publisher.working().channels[0].vu_level.to_bits(), 65_535 - 2_048);

        publisher.update_channel(ChannelId(0), ChannelUpdate::silent(false));
        assert_eq!(publisher.working().channels[0].vu_level.to_bits(), 65_535 - 4_096);
        assert!(!publisher.working().channels[0].active, "the _ActiveFlag follows the voice");
    }

    #[test]
    fn a_volume_write_strikes_the_meter_without_a_retrigger() {
        let (mut publisher, _reader) = telemetry_channel();
        publisher.update_channel(ChannelId(0), sounding(U0F16::MAX));
        publisher.update_channel(ChannelId(0), sounding(U0F16::MAX));
        assert!(publisher.working().channels[0].vu_level < U0F16::MAX);

        let write = ChannelUpdate { volume: Some(U0F16::from_bits(30_000)), volume_written: true, ..sounding(U0F16::MAX) };
        publisher.update_channel(ChannelId(0), write);
        assert_eq!(publisher.working().channels[0].vu_level.to_bits(), 30_000, "a volume-column write holds the bar at the new volume");
    }

    #[test]
    fn the_display_fields_are_sticky_once_a_channel_goes_silent() {
        let (mut publisher, _reader) = telemetry_channel();
        let pan = I1F15::from_bits(8_000);
        publisher.update_channel(ChannelId(0), ChannelUpdate { pan: Some(pan), ..sounding(U0F16::MAX) });
        publisher.update_channel(ChannelId(0), ChannelUpdate::silent(true));

        let channel = publisher.working().channels[0];
        assert_eq!(channel.note, Some(Note::MIDDLE_C), "the channel row keeps showing the note it played");
        assert_eq!(channel.instrument, 1);
        assert_eq!(channel.volume, U0F16::MAX);
        assert_eq!(channel.pan, pan);
        assert!(channel.muted, "and the mute state is not sticky — it is whatever the host last said");
        assert!(!channel.active);
    }

    #[test]
    fn effects_are_reported_per_row_and_cleared_at_the_top_of_the_next() {
        let (mut publisher, _reader) = telemetry_channel();
        publisher.report_effect(ChannelId(1), EffectDisplay::raw(1, 0x06).with_name("change speed"));
        assert_eq!(publisher.working().channels[1].effect.name, "change speed");

        publisher.clear_effects();
        assert_eq!(publisher.working().channels[1].effect, EffectDisplay::NONE);
        assert_eq!(publisher.working().channels[0].effect, EffectDisplay::NONE);
    }

    #[test]
    fn an_out_of_range_channel_is_ignored_rather_than_panicking() {
        let (mut publisher, _reader) = telemetry_channel();
        publisher.update_channel(ChannelId(MAX_CHANNELS as u16), sounding(U0F16::MAX));
        publisher.report_effect(ChannelId(9_999), EffectDisplay::raw(1, 0));
        publisher.report_note(ChannelId(9_999), Some(Note::A440), Some(2));
        publisher.set_channel_count(200);
        assert_eq!(publisher.working().channel_count, MAX_CHANNELS as u8);
    }

    #[test]
    fn the_sequence_advances_even_when_a_publish_is_dropped() {
        let (mut publisher, mut reader) = telemetry_channel_with_depth(2);
        for _ in 0..5 {
            publisher.publish();
        }
        assert_eq!(publisher.publishes_dropped(), 3);
        let snapshot = reader.read();
        assert_eq!(snapshot.sequence, 2, "the ring kept the two oldest; a producer cannot pop");
        assert_eq!(snapshot.publishes_dropped, 0, "nothing had been dropped when that one was written");

        publisher.publish();
        assert_eq!(reader.read().publishes_dropped, 3, "the next snapshot through carries the count");
    }
}
