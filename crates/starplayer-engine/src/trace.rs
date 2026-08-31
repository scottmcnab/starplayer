//! Stable per-tick state traces (architecture section 2.2, M2-C1).
//!
//! This module only exists with `feature = "trace"`. Shipping builds therefore contain
//! neither the recorder nor the branch, field, or allocation that feeds it. Trace builds
//! are diagnostic builds: the recorder intentionally owns an unbounded `Vec` so a complete
//! offline run can be serialized and diffed. They are not the configuration used for the
//! real-time allocation check.
//!
//! # Text format, version 1
//!
//! The first line is `starplayer-trace v=1`. Each tick then has one header followed by
//! exactly one indented line for every logical module channel, in ascending channel order:
//!
//! ```text
//! starplayer-trace v=1
//! t=00042 frm=000000037044 ord=03 pat=07 row=12 tk=00 spd=06 bpm=125 gv=64
//!  ch=00 act=1 note=C-5 ins=01 smp=01 vol=64 per=001712 pan=048 pos=0000001234.91a2b3c4 cut=255 res=000 fl=VP
//! ```
//!
//! `note` uses tracker notation with C-0 as semitone zero; `---` means no note. Instrument
//! and sample numbers are one-based, with zero meaning none. Volume and global volume are
//! 0..64, pan/filter fields are 0..255, period is the format processor's native integer,
//! and position is the exact Q32.32 sample cursor (decimal whole part, hexadecimal
//! fraction). Dirty flags are `V` volume, `S` sample, `P` pitch, `N` pan, `T` tempo and
//! `X` stop, always in that order; `-` means none.
//!
//! libxmp's `test-dev/gen_mixer_data` writes
//! `time row frame channel period note instrument volume pan position cutoff resonance`.
//! C2's adapter can map all state fields directly; its millisecond `time` is intentionally
//! not duplicated because `frm` is the exact clock, and this format adds order, pattern,
//! speed, BPM, global volume, activity, sample number and dirty flags.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write};

use starplayer_core::{ChannelId, DirtyBits, Frame, VoiceId, VoiceParam};
use starplayer_mixer::VoicePool;

use crate::channel::ChannelTable;
use crate::sequencer::{SongPosition, TraceChannelState};

/// The current on-disk/text contract version.
pub const TRACE_FORMAT_VERSION: u16 = 1;

/// A complete trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trace {
    /// Text contract version this value represents.
    pub version: u16,
    /// Ticks in dispatch order.
    pub ticks: Vec<TraceTick>,
}

impl Default for Trace {
    fn default() -> Trace { Trace { version: TRACE_FORMAT_VERSION, ticks: Vec::new() } }
}

impl Trace {
    /// Serialize the stable line-oriented representation.
    pub fn write_text(&self, destination: &mut impl Write) -> fmt::Result {
        writeln!(destination, "starplayer-trace v={}", self.version)?;
        for tick in &self.ticks {
            tick.write_text(destination)?;
        }
        Ok(())
    }

    /// Serialize into an owned string.
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        let _ = self.write_text(&mut text);
        text
    }

    /// Keep only the first `count` ticks. Useful when a render quantum crossed a requested
    /// command-line tick limit.
    pub fn truncate(&mut self, count: usize) { self.ticks.truncate(count); }
}

/// Sequencer and channel state at one tracker tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceTick {
    /// Monotonic tick number within this capture.
    pub tick: u64,
    /// Absolute output frame on which the tick landed.
    pub frame: Frame,
    /// Sounding position for this tick (the `_MActual*` snapshot).
    pub position: SongPosition,
    /// Tick within the row, including pattern-delay repeats.
    pub tick_in_row: u16,
    /// Speed left in effect by this tick.
    pub speed: u8,
    /// BPM left in effect by this tick.
    pub bpm: u16,
    /// Module global volume on its native 0..64 trace scale.
    pub global_volume: u8,
    /// Logical module channels, in channel-number order.
    pub channels: Vec<TraceChannel>,
}

impl TraceTick {
    fn write_text(&self, destination: &mut impl Write) -> fmt::Result {
        writeln!(
            destination,
            "t={:05} frm={:012} ord={:02} pat={:02} row={:02} tk={:02} spd={:02} bpm={:03} gv={:02}",
            self.tick,
            self.frame.0,
            self.position.order,
            self.position.pattern,
            self.position.row,
            self.tick_in_row,
            self.speed,
            self.bpm,
            self.global_volume,
        )?;
        for channel in &self.channels {
            channel.write_text(destination)?;
        }
        Ok(())
    }
}

/// One logical channel at a tick boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceChannel {
    /// Zero-based logical channel number.
    pub channel: u16,
    /// Whether the channel owns a live foreground voice.
    pub active: bool,
    /// Linear note with C-0 as zero. `None` is rendered as `---`.
    pub note: Option<u8>,
    /// One-based instrument number; zero means none.
    pub instrument: u16,
    /// One-based sample number; zero means none.
    pub sample: u16,
    /// Format-native channel volume, normalized to 0..64.
    pub volume: u16,
    /// Format-native period.
    pub period: u32,
    /// Pan normalized to 0..255.
    pub pan: u16,
    /// Exact Q32.32 sample position.
    pub position: u64,
    /// Filter cutoff normalized to 0..255.
    pub cutoff: u16,
    /// Filter resonance normalized to 0..255.
    pub resonance: u16,
    /// Parameter writes which landed on this tick.
    pub flags: DirtyBits,
}

impl Default for TraceChannel {
    fn default() -> TraceChannel {
        TraceChannel {
            channel: 0,
            active: false,
            note: None,
            instrument: 0,
            sample: 0,
            volume: 0,
            period: 0,
            pan: 128,
            position: 0,
            cutoff: 255,
            resonance: 0,
            flags: DirtyBits::empty(),
        }
    }
}

impl TraceChannel {
    fn write_text(&self, destination: &mut impl Write) -> fmt::Result {
        write!(
            destination,
            " ch={:02} act={} note=",
            self.channel,
            u8::from(self.active),
        )?;
        write_note(destination, self.note)?;
        write!(
            destination,
            " ins={:02} smp={:02} vol={:02} per={:06} pan={:03} pos={:010}.{:08x} cut={:03} res={:03} fl=",
            self.instrument,
            self.sample,
            self.volume,
            self.period,
            self.pan,
            self.position >> 32,
            self.position as u32,
            self.cutoff,
            self.resonance,
        )?;
        write_flags(destination, self.flags)?;
        destination.write_char('\n')
    }
}

fn write_note(destination: &mut impl Write, note: Option<u8>) -> fmt::Result {
    let Some(note) = note else { return destination.write_str("---") };
    const NAMES: [&str; 12] = ["C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-"];
    let name = NAMES.get((note % 12) as usize).copied().unwrap_or("--");
    write!(destination, "{}{}", name, note / 12)
}

fn write_flags(destination: &mut impl Write, flags: DirtyBits) -> fmt::Result {
    if flags.is_empty() {
        return destination.write_char('-');
    }
    for (flag, symbol) in [
        (DirtyBits::VOLUME, 'V'),
        (DirtyBits::SAMPLE, 'S'),
        (DirtyBits::PITCH, 'P'),
        (DirtyBits::PAN, 'N'),
        (DirtyBits::TEMPO, 'T'),
        (DirtyBits::STOP, 'X'),
    ] {
        if flags.contains(flag) {
            destination.write_char(symbol)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct WorkingChannel {
    state: TraceChannel,
    reported: bool,
    writes: DirtyBits,
}

impl WorkingChannel {
    fn new(channel: u16) -> WorkingChannel {
        WorkingChannel { state: TraceChannel { channel, ..TraceChannel::default() }, reported: false, writes: DirtyBits::empty() }
    }
}

#[derive(Debug)]
struct WorkingTick {
    tick: u64,
    frame: Frame,
    position: SongPosition,
    tick_in_row: u16,
    global_volume: u8,
    channels: Vec<WorkingChannel>,
}

/// Engine-owned trace hook. One working tick is assembled during processor dispatch and
/// committed only after the sequencer has applied its outcome.
#[derive(Debug)]
pub(crate) struct TraceRecorder {
    trace: Trace,
    working: Option<WorkingTick>,
    global_volume: u8,
}

impl Default for TraceRecorder {
    fn default() -> TraceRecorder {
        TraceRecorder { trace: Trace::default(), working: None, global_volume: 64 }
    }
}

impl TraceRecorder {
    pub(crate) fn begin_tick(&mut self, frame: Frame, position: SongPosition, tick_in_row: u16, channel_count: u8) {
        let tick = self.trace.ticks.len() as u64;
        let channels = (0..channel_count).map(|channel| WorkingChannel::new(channel as u16)).collect();
        self.working = Some(WorkingTick { tick, frame, position, tick_in_row, global_volume: self.global_volume, channels });
    }

    pub(crate) fn report_global_volume(&mut self, volume: u8) {
        self.global_volume = volume.min(64);
        if let Some(working) = self.working.as_mut() {
            working.global_volume = self.global_volume;
        }
    }

    pub(crate) fn report_channel(&mut self, channel: ChannelId, state: TraceChannelState) {
        let Some(working) = self.working.as_mut() else { return };
        let Some(entry) = working.channels.get_mut(channel.0 as usize) else { return };
        entry.state.note = state.note;
        entry.state.instrument = state.instrument;
        entry.state.sample = state.sample;
        entry.state.volume = state.volume.min(64);
        entry.state.period = state.period;
        entry.state.pan = state.pan.min(255);
        entry.reported = true;
    }

    pub(crate) fn record_param_write(&mut self, voices: &VoicePool, voice: VoiceId, param: VoiceParam) {
        let flag = match param {
            VoiceParam::Step(_) | VoiceParam::Filter(_) => DirtyBits::PITCH,
            VoiceParam::Volume(_) => DirtyBits::VOLUME,
            VoiceParam::Pan(_) => DirtyBits::PAN,
        };
        self.record_voice_flags(voices, voice, flag);
    }

    pub(crate) fn record_voice_flags(&mut self, voices: &VoicePool, voice: VoiceId, flags: DirtyBits) {
        let Some(channel) = voices.get(voice).map(|state| state.tag.channel) else { return };
        self.record_channel_flags(ChannelId(channel as u16), flags);
    }

    pub(crate) fn record_channel_flags(&mut self, channel: ChannelId, flags: DirtyBits) {
        let Some(working) = self.working.as_mut() else { return };
        if let Some(entry) = working.channels.get_mut(channel.0 as usize) {
            entry.writes.insert(flags);
        }
    }

    pub(crate) fn finish_tick(&mut self, speed: u8, bpm: u16, channels: &ChannelTable, voices: &VoicePool) {
        let Some(mut working) = self.working.take() else { return };
        for entry in &mut working.channels {
            let channel_id = ChannelId(entry.state.channel);
            let voice = channels.foreground(channel_id).and_then(|voice| voices.get(voice));
            if let Some(voice) = voice {
                entry.state.active = true;
                if !entry.reported {
                    // Voice tags use the trace contract's one-based instrument and sample
                    // numbers. Format processors with richer native state report it before
                    // this fallback runs.
                    entry.state.note = Some(voice.tag.note);
                    entry.state.instrument = voice.tag.instrument as u16;
                    entry.state.sample = voice.tag.sample as u16;
                    entry.state.volume = unit_to_scale(voice.params.volume.to_bits(), 64);
                    entry.state.pan = bipolar_pan_to_u8(voice.params.pan.to_bits());
                }
                entry.state.position = voice.position();
                entry.state.cutoff = unit_to_scale(voice.params.filter.cutoff.to_bits(), 255);
                entry.state.resonance = unit_to_scale(voice.params.filter.resonance.to_bits(), 255);
                entry.state.flags = voice.params.dirty | entry.writes;
            } else {
                entry.state.flags = entry.writes;
            }
        }
        let channels = working.channels.into_iter().map(|entry| entry.state).collect();
        self.trace.ticks.push(TraceTick {
            tick: working.tick,
            frame: working.frame,
            position: working.position,
            tick_in_row: working.tick_in_row,
            speed,
            bpm,
            global_volume: working.global_volume,
            channels,
        });
    }

    pub(crate) fn trace(&self) -> &Trace { &self.trace }
    pub(crate) fn take(&mut self) -> Trace { core::mem::take(&mut self.trace) }
    pub(crate) fn clear(&mut self) {
        self.trace.ticks.clear();
        self.working = None;
    }
}

fn unit_to_scale(bits: u16, scale: u32) -> u16 {
    ((bits as u32 * scale + u16::MAX as u32 / 2) / u16::MAX as u32) as u16
}

fn bipolar_pan_to_u8(bits: i16) -> u16 {
    (((bits as i32 + 32_767) * 255 + 32_767) / 65_534).clamp(0, 255) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_one_text_is_stable_and_line_oriented() {
        let trace = Trace {
            version: TRACE_FORMAT_VERSION,
            ticks: alloc::vec![TraceTick {
                tick: 42,
                frame: Frame(37_044),
                position: SongPosition { order: 3, pattern: 7, row: 12 },
                tick_in_row: 0,
                speed: 6,
                bpm: 125,
                global_volume: 64,
                channels: alloc::vec![TraceChannel {
                    channel: 0,
                    active: true,
                    note: Some(60),
                    instrument: 1,
                    sample: 1,
                    volume: 64,
                    period: 1712,
                    pan: 48,
                    position: (1234u64 << 32) | 0x91a2_b3c4,
                    cutoff: 255,
                    resonance: 0,
                    flags: DirtyBits::VOLUME | DirtyBits::PITCH,
                }],
            }],
        };
        assert_eq!(
            trace.to_text(),
            concat!(
                "starplayer-trace v=1\n",
                "t=00042 frm=000000037044 ord=03 pat=07 row=12 tk=00 spd=06 bpm=125 gv=64\n",
                " ch=00 act=1 note=C-5 ins=01 smp=01 vol=64 per=001712 pan=048 pos=0000001234.91a2b3c4 cut=255 res=000 fl=VP\n",
            )
        );
    }
}
