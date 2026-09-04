//! The engine's side of telemetry v1 (M1-B6): turning live voice-pool and channel-table
//! state into a [`Snapshot`](starplayer_telemetry::Snapshot) the UI can render.
//!
//! Present only under `feature = "telemetry"`. Everything here is on the tick path, so it
//! allocates nothing, locks nothing and cannot panic — every channel lookup goes through
//! [`ChannelTable::iter`] and [`VoicePool::get`], both of which answer `None` rather than
//! indexing.
//!
//! # Where the dependency edge runs
//!
//! `starplayer-engine` → `starplayer-telemetry`, optional, behind this feature
//! (architecture §11). The engine is the only thing that can fill a snapshot in
//! *coherently* — the transport position, the channel table and the voice pool all have to
//! be read at the same instant — so it is the engine that owns the publisher and the
//! format crates that decorate it through [`TickContext::report_effect`](crate::TickContext::report_effect).
//! The alternative shape, a `TelemetrySink` trait declared in the engine and implemented
//! in `starplayer-telemetry`, was rejected: it would put a `dyn` call on the tick path and
//! leave the snapshot types split across two crates for every UI to reassemble.
//!
//! # What a UI reads
//!
//! [`Engine::telemetry_reader`](crate::Engine::telemetry_reader), once, before the engine
//! goes to the audio thread. Then
//! [`TelemetryReader::read`](starplayer_telemetry::TelemetryReader::read) at frame rate.

use starplayer_core::{DirtyBits, Note};
use starplayer_mixer::VoicePool;
use starplayer_telemetry::{ChannelUpdate, TelemetryPublisher, WarningFlags};

use crate::channel::ChannelTable;
use crate::engine::EngineWarnings;

/// Mirror the engine's sticky warnings into the snapshot's.
///
/// Two types rather than one because the edge runs engine → telemetry: a UI that reads
/// snapshots must not have to depend on the engine to name their fields.
impl From<EngineWarnings> for WarningFlags {
    fn from(warnings: EngineWarnings) -> WarningFlags {
        WarningFlags {
            zero_advance_forced: warnings.zero_advance_forced,
            event_limit_reached: warnings.event_limit_reached,
            retired_module_dropped: warnings.retired_module_dropped,
            unsupported_command: warnings.unsupported_command,
            late_events: warnings.late_events,
        }
    }
}

/// Fold one tick of live channel and voice state into `publisher`.
///
/// Called from [`PatternSequencer`](crate::PatternSequencer)'s dispatch, after the tick's
/// outcome has been committed and before anything is mixed — which is exactly when the
/// dirty bits set by the tick are still visible, since the mixer clears them on the next
/// accumulation pass (`starplayer_mixer::kernel`).
///
/// This does **not** publish. The sequencer latches the `_MActual*` transport first and
/// then calls [`TelemetryPublisher::publish`] once, so everything in the snapshot comes
/// from this one tick.
pub fn capture_channels(publisher: &mut TelemetryPublisher, channels: &ChannelTable, voices: &VoicePool) {
    publisher.set_channel_count(channels.len() as u8);
    publisher.set_voices_active(voices.voices_active().min(u16::MAX as usize) as u16);

    for (id, lane) in channels.iter() {
        let sounding = lane.foreground.and_then(|voice| voices.get(voice).map(|state| (voice, state)));
        let update = match sounding {
            Some((voice, state)) => ChannelUpdate {
                voice: Some(voice),
                // The tag is the voice's own record of what it was started with, so the
                // display follows a stolen or retriggered voice without the sequencer
                // having to remember anything (architecture §5.1).
                note: Some(Note::new(state.tag.note)),
                instrument: Some(state.tag.instrument),
                volume: Some(state.params.volume),
                pan: Some(state.params.pan),
                volume_written: state.params.dirty.contains(DirtyBits::VOLUME),
                muted: lane.muted,
            },
            None => ChannelUpdate::silent(lane.muted),
        };
        publisher.update_channel(id, update);
    }
}

/// Fold the sixteen MIDI lanes into `publisher`, for a [`MidiSource`](crate::instrument::MidiSource).
///
/// The counterpart of [`capture_channels`] for the musical half: the same
/// [`ChannelUpdate`] per lane, but only for channels
/// [`MIDI_CHANNEL_BASE`](crate::instrument::MIDI_CHANNEL_BASE) upwards, and the channel
/// count is **raised** to cover them rather than set — a tracker sharing the mux publishes
/// its song's own channel count from the same working snapshot, and neither source may
/// hide the other's lanes.
///
/// Like [`capture_channels`] this does not publish; the source does, once, after it.
#[cfg(feature = "telemetry")]
pub fn capture_midi_channels(publisher: &mut TelemetryPublisher, channels: &ChannelTable, voices: &VoicePool) {
    let base = crate::instrument::MIDI_CHANNEL_BASE as usize;
    let top = base + crate::instrument::MIDI_CHANNEL_COUNT;
    let lanes = channels.len().min(top);
    publisher.set_channel_count(publisher.working().channel_count.max(lanes.min(u8::MAX as usize) as u8));
    publisher.set_voices_active(voices.voices_active().min(u16::MAX as usize) as u16);

    for index in base..lanes {
        let id = starplayer_core::ChannelId(index as u16);
        let lane = match channels.get(id) {
            Some(lane) => lane,
            None => continue,
        };
        let sounding = lane.foreground.and_then(|voice| voices.get(voice).map(|state| (voice, state)));
        let update = match sounding {
            Some((voice, state)) => ChannelUpdate {
                voice: Some(voice),
                note: Some(Note::new(state.tag.note)),
                instrument: Some(state.tag.instrument),
                volume: Some(state.params.volume),
                pan: Some(state.params.pan),
                volume_written: state.params.dirty.contains(DirtyBits::VOLUME),
                muted: lane.muted,
            },
            None => ChannelUpdate::silent(lane.muted),
        };
        publisher.update_channel(id, update);
    }
}
