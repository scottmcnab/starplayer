//! Stable per-tick state traces (architecture section 2.2, M2-C1).
//!
//! This module only exists with `feature = "trace"`. Shipping builds therefore contain
//! neither the recorder nor the branch, field, or allocation that feeds it. Trace builds
//! are diagnostic builds: the recorder intentionally owns an unbounded `Vec` so a complete
//! offline run can be serialized and diffed. They are not the configuration used for the
//! real-time allocation check.
//!
//! # Text format, version 2
//!
//! The first line is `starplayer-trace v=2`. Each tick then has one header, followed by
//! exactly one indented ` ch=` line for every logical module channel in ascending channel
//! order, followed by one indented ` vc=` line for every **active voice no channel owns**,
//! in pool slot order:
//!
//! ```text
//! starplayer-trace v=2
//! t=00042 frm=000000037044 ord=03 pat=07 row=12 tk=00 spd=06 bpm=125 gv=64
//!  ch=00 act=1 note=C-5 ins=01 smp=0001 vol=64 per=001712 pan=048 pos=0000001234.91a2b3c4 cut=255 res=000 fl=VP
//!  vc=017 root=03 note=C-5 ins=01 smp=0001 vol=32 per=001712 pan=128 pos=0000001234.91a2b3c4 cut=255 res=000 fl=-
//! ```
//!
//! `note` uses tracker notation with C-0 as semitone zero; `---` means no note. Instrument
//! and sample numbers are one-based, with zero meaning none. Volume and global volume are
//! 0..64, pan/filter fields are 0..255, period is the format processor's native integer,
//! and position is the exact Q32.32 sample cursor (decimal whole part, hexadecimal
//! fraction). Dirty flags are `V` volume, `S` sample, `P` pitch, `N` pan, `T` tempo and
//! `X` stop, always in that order; `-` means none.
//!
//! ## What version 2 added, and why
//!
//! IT's New Note Actions **detach** a channel's sounding voice into the background, where
//! it keeps running its own envelopes and fadeout with no channel owning it. Such a voice
//! has no ` ch=` row to appear on, and before v2 it was simply invisible to the trace — so
//! the diff harness could not see an NNA at all. A ` vc=` line therefore names the voice's
//! **pool slot** rather than a channel, and carries `root`, the channel that triggered it
//! (the voice's `tag.channel`, still set after detachment for Duplicate Check). Its
//! remaining fields and encodings are the ` ch=` line's, minus `act`: a voice that is
//! listed is active by definition.
//!
//! A tick with no background voices emits no ` vc=` lines at all, so a MOD, S3M or MTM
//! trace — none of which ever detaches a voice — differs from its v1 text only in the
//! version header and in `smp` now being four digits, widened with
//! [`VoiceTag::sample`](starplayer_mixer::VoiceTag) itself.
//!
//! A parameter write is attributed to the ` ch=` row when its voice **is** that channel's
//! foreground, and to the voice's own ` vc=` row otherwise.
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
pub const TRACE_FORMAT_VERSION: u16 = 2;

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
    /// Active voices that are no channel's foreground, in pool slot order. Empty for every
    /// format that never detaches a voice, which is every format before IT.
    pub voices: Vec<TraceVoice>,
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
        for voice in &self.voices {
            voice.write_text(destination)?;
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
            " ins={:02} smp={:04} vol={:02} per={:06} pan={:03} pos={:010}.{:08x} cut={:03} res={:03} fl=",
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

/// One active voice that **no channel owns** at a tick boundary — trace format v2.
///
/// The trace's view of an IT background voice: detached by a New Note Action, still
/// sounding under its own envelopes, and reachable through no channel row. Every field
/// but `voice` and `root` carries the same quantity, on the same scale, as the
/// [`TraceChannel`] field of the same name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceVoice {
    /// The voice's pool slot index — the `vc=` key, and what makes the row stable across
    /// ticks while the voice lives.
    pub voice: u16,
    /// The channel that triggered the voice: its `tag.channel`, kept after detachment so
    /// IT's Duplicate Check can still find it. No channel owns the voice any more.
    pub root: u16,
    /// Linear note with C-0 as zero. `None` is rendered as `---`.
    pub note: Option<u8>,
    /// One-based instrument number; zero means none.
    pub instrument: u16,
    /// One-based sample number; zero means none.
    pub sample: u16,
    /// Format-native voice volume, normalized to 0..64.
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
    /// Parameter writes which landed on this voice this tick.
    pub flags: DirtyBits,
}

impl Default for TraceVoice {
    fn default() -> TraceVoice {
        TraceVoice {
            voice: 0,
            root: 0,
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

impl TraceVoice {
    fn write_text(&self, destination: &mut impl Write) -> fmt::Result {
        write!(destination, " vc={:03} root={:02} note=", self.voice, self.root)?;
        write_note(destination, self.note)?;
        write!(
            destination,
            " ins={:02} smp={:04} vol={:02} per={:06} pan={:03} pos={:010}.{:08x} cut={:03} res={:03} fl=",
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
    /// Native state a format reported for a voice no channel owns, keyed by the exact
    /// [`VoiceId`] it named. Kept as a flat list rather than a slot-indexed array because
    /// the recorder does not know the pool's capacity, and because in every format before
    /// IT the list stays empty.
    voice_reports: Vec<(VoiceId, TraceChannelState)>,
    /// Parameter writes which landed on a voice that was nobody's foreground at the moment
    /// of the write.
    voice_writes: Vec<(VoiceId, DirtyBits)>,
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
        self.working = Some(WorkingTick {
            tick,
            frame,
            position,
            tick_in_row,
            global_volume: self.global_volume,
            channels,
            voice_reports: Vec::new(),
            voice_writes: Vec::new(),
        });
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

    /// Native state for a voice no channel owns. The last report of a tick wins, the way
    /// a repeated [`TraceRecorder::report_channel`] does.
    pub(crate) fn report_voice(&mut self, voice: VoiceId, state: TraceChannelState) {
        let Some(working) = self.working.as_mut() else { return };
        match working.voice_reports.iter_mut().find(|(reported, _)| *reported == voice) {
            Some((_, existing)) => *existing = state,
            None => working.voice_reports.push((voice, state)),
        }
    }

    pub(crate) fn record_param_write(&mut self, voices: &VoicePool, channels: &ChannelTable, voice: VoiceId, param: VoiceParam) {
        let Some(flag) = dirty_bit_for(param) else { return };
        self.record_voice_flags(voices, channels, voice, flag);
    }

    /// Attribute a write to the row that is going to show it.
    ///
    /// A voice which **is** some channel's foreground reports on that channel's ` ch=`
    /// row, exactly as it did in v1. A voice which is nobody's — one an IT New Note Action
    /// detached — reports on its own ` vc=` row, because no ` ch=` row describes it any
    /// more. Asking the channel table rather than trusting the voice's `tag.channel` is
    /// what keeps the two decisions in step: `finish_tick` emits a ` vc=` row for exactly
    /// the voices this predicate sends there, so a write can never land on a row that is
    /// not written out.
    pub(crate) fn record_voice_flags(&mut self, voices: &VoicePool, channels: &ChannelTable, voice: VoiceId, flags: DirtyBits) {
        if voices.get(voice).is_none() {
            return;
        }
        match owning_channel(channels, voice) {
            Some(channel) => self.record_channel_flags(channel, flags),
            None => self.record_voice_writes(voice, flags),
        }
    }

    fn record_voice_writes(&mut self, voice: VoiceId, flags: DirtyBits) {
        let Some(working) = self.working.as_mut() else { return };
        match working.voice_writes.iter_mut().find(|(written, _)| *written == voice) {
            Some((_, existing)) => existing.insert(flags),
            None => working.voice_writes.push((voice, flags)),
        }
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
                    entry.state.sample = voice.tag.sample;
                    entry.state.volume = unit_to_scale(voice.params.volume.to_bits(), 64);
                    entry.state.pan = bipolar_pan_to_u8(voice.params.pan.to_bits());
                }
                entry.state.position = voice.position();
                entry.state.cutoff = unit_to_scale(voice.params.filter.cutoff.to_bits(), 255);
                entry.state.resonance = unit_to_scale(voice.params.filter.resonance.to_bits(), 255);
            }
            // Only this tick's writes, live voice or not. `voice.params.dirty` is
            // mixer-lifetime state that survives until the mixer consumes it, so unioning
            // it in here reported a change on every tick after the one that made it.
            entry.state.flags = entry.writes;
        }

        // The v2 half: every active voice that no channel owns, in pool slot order.
        // `VoicePool::iter` already walks active slots in that order, so the rows come out
        // deterministic without the recorder sorting anything.
        let mut background = Vec::new();
        for (id, voice) in voices.iter() {
            if owning_channel(channels, id).is_some() {
                continue;
            }
            let mut row = TraceVoice {
                voice: id.index(),
                root: voice.tag.channel as u16,
                position: voice.position(),
                cutoff: unit_to_scale(voice.params.filter.cutoff.to_bits(), 255),
                resonance: unit_to_scale(voice.params.filter.resonance.to_bits(), 255),
                flags: working.voice_writes.iter().find(|(written, _)| *written == id).map(|(_, flags)| *flags).unwrap_or_default(),
                ..TraceVoice::default()
            };
            match working.voice_reports.iter().find(|(reported, _)| *reported == id) {
                Some((_, state)) => {
                    row.note = state.note;
                    row.instrument = state.instrument;
                    row.sample = state.sample;
                    row.volume = state.volume.min(64);
                    row.period = state.period;
                    row.pan = state.pan.min(255);
                }
                None => {
                    // The same tag-and-params fallback an unreported channel gets.
                    row.note = Some(voice.tag.note);
                    row.instrument = voice.tag.instrument as u16;
                    row.sample = voice.tag.sample;
                    row.volume = unit_to_scale(voice.params.volume.to_bits(), 64);
                    row.pan = bipolar_pan_to_u8(voice.params.pan.to_bits());
                }
            }
            background.push(row);
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
            voices: background,
        });
    }

    pub(crate) fn trace(&self) -> &Trace { &self.trace }
    pub(crate) fn take(&mut self) -> Trace { core::mem::take(&mut self.trace) }
    pub(crate) fn clear(&mut self) {
        self.trace.ticks.clear();
        self.working = None;
    }
}

/// The channel whose **foreground** `voice` is, if any.
///
/// Not `voices.get(voice).tag.channel`: a tag says which channel *triggered* the voice and
/// survives detachment on purpose, so it answers a different question. The whole table is
/// scanned rather than only the tagged lane so that a voice allocated straight on the pool
/// — a scripted test source, a future MIDI sample player sharing the pool — is classified
/// by what the channel table actually says rather than by a tag nobody set.
fn owning_channel(channels: &ChannelTable, voice: VoiceId) -> Option<ChannelId> {
    channels.iter().find(|(_, channel)| channel.foreground == Some(voice)).map(|(id, _)| id)
}

/// The dirty bit a parameter write reports, if the v1 trace contract has one for it.
///
/// [`VoiceParam::Filter`] has none: the original's `_CHN_*` flag set never had a filter
/// bit and the version 1 text format has no letter for one, so a filter write reports no
/// flag rather than borrowing `PITCH` and claiming a pitch change that did not happen.
pub(crate) const fn dirty_bit_for(param: VoiceParam) -> Option<DirtyBits> {
    match param {
        VoiceParam::Step(_) => Some(DirtyBits::PITCH),
        VoiceParam::Volume(_) => Some(DirtyBits::VOLUME),
        VoiceParam::Pan(_) => Some(DirtyBits::PAN),
        VoiceParam::Filter(_) => None,
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

    use starplayer_core::{FilterParams, VoiceParams};
    use starplayer_mixer::{SampleRegion, VoiceTag};


    /// One tick with a single live voice on channel zero.
    fn recorded_tick(writes: &[VoiceParam], preset_dirty: DirtyBits) -> TraceChannel {
        let mut voices = VoicePool::new(1);
        let mut channels = ChannelTable::new(1);
        let params = VoiceParams { dirty: preset_dirty, ..VoiceParams::SILENT };
        let voice = channels
            .trigger(ChannelId(0), &mut voices, VoiceTag::default(), SampleRegion::one_shot(0, 8), params, 0)
            .expect("a fresh pool has a slot");

        let mut recorder = TraceRecorder::default();
        recorder.begin_tick(Frame::ZERO, SongPosition::default(), 0, 1);
        for write in writes {
            if let Some(state) = voices.get_mut(voice) { write.apply(&mut state.params); }
            recorder.record_param_write(&voices, &channels, voice, *write);
        }
        recorder.finish_tick(6, 125, &channels, &voices);
        recorder.take().ticks.remove(0).channels.remove(0)
    }

    #[test]
    fn a_filter_write_reports_no_flag_rather_than_a_false_pitch_flag() {
        assert_eq!(dirty_bit_for(VoiceParam::Step(starplayer_core::Step::ZERO)), Some(DirtyBits::PITCH));
        assert_eq!(dirty_bit_for(VoiceParam::Volume(starplayer_core::U0F16::MAX)), Some(DirtyBits::VOLUME));
        assert_eq!(dirty_bit_for(VoiceParam::Pan(starplayer_core::I1F15::ZERO)), Some(DirtyBits::PAN));
        assert_eq!(dirty_bit_for(VoiceParam::Filter(FilterParams::BYPASS)), None, "the v1 contract has no filter flag to report");

        let channel = recorded_tick(&[VoiceParam::Filter(FilterParams::BYPASS)], DirtyBits::empty());
        assert_eq!(channel.flags, DirtyBits::empty(), "a filter write is not a pitch write");
    }

    #[test]
    fn per_tick_flags_are_this_tick_s_writes_and_not_mixer_lifetime_state() {
        let quiet = recorded_tick(&[], DirtyBits::VOLUME | DirtyBits::PITCH | DirtyBits::SAMPLE);
        assert_eq!(quiet.flags, DirtyBits::empty(), "a tick with no writes reports no flags");
        assert!(quiet.active, "the voice is still live; only its flags are empty");

        let written = recorded_tick(&[VoiceParam::Volume(starplayer_core::U0F16::MAX)], DirtyBits::PITCH);
        assert_eq!(written.flags, DirtyBits::VOLUME, "only the write that landed this tick is reported");
    }

    #[test]
    fn version_two_text_is_stable_and_line_oriented() {
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
                voices: alloc::vec![TraceVoice {
                    voice: 17,
                    root: 3,
                    note: Some(60),
                    instrument: 1,
                    sample: 1,
                    volume: 32,
                    period: 1712,
                    pan: 128,
                    position: (1234u64 << 32) | 0x91a2_b3c4,
                    cutoff: 255,
                    resonance: 0,
                    flags: DirtyBits::empty(),
                }],
            }],
        };
        assert_eq!(
            trace.to_text(),
            concat!(
                "starplayer-trace v=2\n",
                "t=00042 frm=000000037044 ord=03 pat=07 row=12 tk=00 spd=06 bpm=125 gv=64\n",
                " ch=00 act=1 note=C-5 ins=01 smp=0001 vol=64 per=001712 pan=048 pos=0000001234.91a2b3c4 cut=255 res=000 fl=VP\n",
                " vc=017 root=03 note=C-5 ins=01 smp=0001 vol=32 per=001712 pan=128 pos=0000001234.91a2b3c4 cut=255 res=000 fl=-\n",
            )
        );
    }

    #[test]
    fn a_tick_with_no_background_voice_emits_no_voice_line() {
        let mut voices = VoicePool::new(2);
        let mut channels = ChannelTable::new(1);
        channels
            .trigger(ChannelId(0), &mut voices, VoiceTag::default(), SampleRegion::one_shot(0, 8), VoiceParams::SILENT, 0)
            .expect("a fresh pool has a slot");

        let mut recorder = TraceRecorder::default();
        recorder.begin_tick(Frame::ZERO, SongPosition::default(), 0, 1);
        recorder.finish_tick(6, 125, &channels, &voices);
        let tick = recorder.take().ticks.remove(0);
        assert!(tick.voices.is_empty(), "a foreground voice is described by its channel row, not a voice row");
        assert!(tick.channels.first().expect("one channel row").active);
    }

    #[test]
    fn a_detached_voice_gets_its_own_row_and_keeps_the_channel_that_triggered_it() {
        let mut voices = VoicePool::new(2);
        let mut channels = ChannelTable::new(2);
        let tag = VoiceTag { channel: 0, instrument: 2, sample: 300, note: 48 };
        let voice = channels
            .trigger(ChannelId(1), &mut voices, tag, SampleRegion::one_shot(0, 8), VoiceParams::SILENT, 0)
            .expect("a fresh pool has a slot");
        assert_eq!(channels.detach_foreground(ChannelId(1)), Some(voice));

        let mut recorder = TraceRecorder::default();
        recorder.begin_tick(Frame::ZERO, SongPosition::default(), 0, 2);
        // A write to a voice nobody owns lands on the voice's own row, not on channel 1's.
        recorder.record_param_write(&voices, &channels, voice, VoiceParam::Volume(starplayer_core::U0F16::MAX));
        recorder.finish_tick(6, 125, &channels, &voices);

        let tick = recorder.take().ticks.remove(0);
        assert_eq!(tick.voices.len(), 1, "the detached voice is listed once");
        let row = tick.voices.first().expect("the detached voice's row");
        assert_eq!(row.voice, voice.index());
        assert_eq!(row.root, 1, "`root` is the channel that triggered it, which `trigger` wrote into the tag");
        assert_eq!(row.sample, 300, "a sample number past 255 survives the widened tag");
        assert_eq!(row.instrument, 2);
        assert_eq!(row.flags, DirtyBits::VOLUME);
        let channel = tick.channels.get(1).expect("channel 1's row");
        assert!(!channel.active, "the channel it left owns nothing");
        assert_eq!(channel.flags, DirtyBits::empty(), "and the write did not land on its row");
    }

    #[test]
    fn a_reported_background_voice_overrides_the_tag_fallback() {
        let mut voices = VoicePool::new(1);
        let mut channels = ChannelTable::new(1);
        let voice = channels
            .trigger(ChannelId(0), &mut voices, VoiceTag::default(), SampleRegion::one_shot(0, 8), VoiceParams::SILENT, 0)
            .expect("a fresh pool has a slot");
        channels.detach_foreground(ChannelId(0));

        let mut recorder = TraceRecorder::default();
        recorder.begin_tick(Frame::ZERO, SongPosition::default(), 0, 1);
        recorder.report_voice(voice, TraceChannelState { note: Some(72), instrument: 5, sample: 9, volume: 33, period: 856, pan: 200 });
        recorder.finish_tick(6, 125, &channels, &voices);

        let row = recorder.take().ticks.remove(0).voices.remove(0);
        assert_eq!((row.note, row.instrument, row.sample), (Some(72), 5, 9));
        assert_eq!((row.volume, row.period, row.pan), (33, 856, 200));
    }
}
