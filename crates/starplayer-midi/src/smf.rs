//! The Standard MIDI File parser and its tempo map (feature `smf`).
//!
//! [`parse_smf`] reads a `.mid` file's header and track chunks into an [`Smf`]: every
//! track's channel voice events merged into one list, sorted by absolute tick with a
//! stable per-track order, plus the file's tempo map. [`Smf::to_frames`] is the only place
//! ticks become output frames, through the same no-floats, remainder-carried Q32.32
//! arithmetic [`starplayer_core::clock::FrameClock`] uses for a tracker tick — see its
//! doc comment for why that shape is what makes the conversion drift-free.
//!
//! Reference: the 1996 MMA/AMEI Standard MIDI Files 1.0 specification (RP-001) —
//! header chunk, track chunks, variable-length quantities, running status (cancelled by
//! any meta or sysex event, per the specification's own wording), and the `set_tempo` /
//! `end_of_track` / `time_signature` meta events.

use alloc::vec::Vec;

use starplayer_core::{Error, Event, Frame, Q32_32, Target, TimedEvent};
use starplayer_engine::midi_channel;

use crate::codec::{channel_message_data_len, decode_message};

/// The four bytes a Standard MIDI File header chunk begins with.
const HEADER_MAGIC: [u8; 4] = *b"MThd";
/// The four bytes a track chunk begins with.
const TRACK_MAGIC: [u8; 4] = *b"MTrk";

/// Meta event type: Set Tempo (`FF 51 03 tttttt`, microseconds per quarter note).
const META_SET_TEMPO: u8 = 0x51;
/// Meta event type: End of Track (`FF 2F 00`). Mandatory at the end of every track.
const META_END_OF_TRACK: u8 = 0x2F;

/// Default tempo (RP-001): 500,000 microseconds per quarter note, 120 BPM. In effect
/// from tick zero of a PPQN-divided file until the first `set_tempo` meta event, if any.
const DEFAULT_MICROS_PER_QUARTER_NOTE: u32 = 500_000;

/// Whether `bytes` begins with a Standard MIDI File header. The SMF counterpart to a
/// format crate's `probe`.
pub fn probe(bytes: &[u8]) -> bool { bytes.get(..4) == Some(&HEADER_MAGIC) }

/// How a file's delta-times convert to real time.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Division {
    /// Ticks per quarter note: delta-times are tempo-relative, and [`Smf::to_frames`]
    /// consults the tempo map. This is what almost every SMF in the wild uses.
    TicksPerQuarterNote(u16),
    /// SMPTE time code: delta-times are a fixed real-time unit and the tempo map is
    /// never consulted. `frames_per_second` is the raw signed header byte — one of −24,
    /// −25 (29.97 fps drop-frame is stored as plain −29, per RP-001), −29 or −30 — and
    /// `ticks_per_frame` is the resolution within one SMPTE frame.
    Smpte {
        /// The signed SMPTE frame-rate byte, negative by construction.
        frames_per_second: i8,
        /// Ticks per SMPTE frame.
        ticks_per_frame: u8,
    },
}

/// One `set_tempo` meta event, at the tick it takes effect.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TempoChange {
    /// Absolute tick this tempo takes effect at.
    pub tick: u64,
    /// Microseconds per quarter note.
    pub micros_per_quarter_note: u32,
    track: u16,
    sequence: u32,
}

/// One channel voice event at an absolute tick, before tempo-mapping into a frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct TickEvent {
    tick: u64,
    track: u16,
    sequence: u32,
    channel: u8,
    event: Event,
}

/// A parsed Standard MIDI File: every track's channel voice events merged into one list,
/// sorted by absolute tick with a stable per-track order, and the tempo map that
/// [`Smf::to_frames`] converts them through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Smf {
    format: u16,
    division: Division,
    track_count: u16,
    events: Vec<TickEvent>,
    tempo_map: Vec<TempoChange>,
    /// The latest `end_of_track` tick across every track — the file's own length, which
    /// can run past the last channel voice event when a track ends in silence.
    length_ticks: u64,
}

impl Smf {
    /// The header's format: 0 (single track), 1 (multiple tracks, one tempo map) or 2
    /// (multiple independent songs — [`parse_smf`] never produces this, see its doc).
    pub const fn format(&self) -> u16 { self.format }

    /// How this file's delta-times convert to real time.
    pub const fn division(&self) -> Division { self.division }

    /// The number of track chunks the header declared (and [`parse_smf`] found).
    pub const fn track_count(&self) -> u16 { self.track_count }

    /// Every `set_tempo` meta event in the file, across every track, sorted by tick.
    /// Empty for an [`Division::Smpte`]-divided file: SMPTE delta-times are already real
    /// time, so a `set_tempo` in one — legal, but pointless — is parsed and ignored.
    pub fn tempo_changes(&self) -> &[TempoChange] { &self.tempo_map }

    /// The number of channel voice events across every track — `NoteOn`, `NoteOff`,
    /// `Controller` and the rest of what [`crate::codec::decode_message`] produces. Meta
    /// and sysex events are not counted; they carry nothing [`Smf::to_frames`] emits.
    pub fn event_count(&self) -> usize { self.events.len() }

    /// Convert every event to an absolute output frame at `sample_rate_hz`, through the
    /// tempo map, in the file's own sorted order.
    ///
    /// No floats anywhere: the running conversion carries its sub-frame remainder in
    /// [`Q32_32`] exactly as [`starplayer_core::clock::FrameClock::advance_tick`] does,
    /// so the same file at the same rate produces the same frames on every target
    /// (research point 2).
    pub fn to_frames(&self, sample_rate_hz: u32) -> Vec<TimedEvent> {
        let mut converter = TickToFrameConverter::new(self.division, &self.tempo_map, sample_rate_hz);
        self.events
            .iter()
            .map(|tick_event| {
                let frame = converter.advance_to(tick_event.tick);
                TimedEvent { frame: Frame(frame), target: Target::Channel(midi_channel(tick_event.channel)), event: tick_event.event }
            })
            .collect()
    }

    /// The file's own length in output frames at `sample_rate_hz`: the latest
    /// `end_of_track` tick across every track, converted through the same tempo map
    /// [`Smf::to_frames`] uses. Not merely the last event's frame — a track that ends in
    /// silence after its last note still has a length.
    pub fn length_frames(&self, sample_rate_hz: u32) -> u64 {
        TickToFrameConverter::new(self.division, &self.tempo_map, sample_rate_hz).advance_to(self.length_ticks)
    }
}

/// Q32.32 frames per tick for one PPQN-divided segment at a fixed tempo:
/// `sample_rate_hz * micros_per_quarter_note / (ppqn * 1_000_000)`.
///
/// Widened through `u128` so the multiply cannot overflow before the shift, and
/// saturating at `u64::MAX` rather than panicking — the same shape
/// [`starplayer_core::tempo::exact_frames_per_tick`] uses for a tracker tick.
fn ppqn_frames_per_tick_bits(sample_rate_hz: u32, micros_per_quarter_note: u32, ppqn: u16) -> u64 {
    let numerator = ((sample_rate_hz as u128) * (micros_per_quarter_note as u128)) << 32;
    let denominator = (ppqn.max(1) as u128) * 1_000_000;
    let quotient = numerator / denominator;
    if quotient > u64::MAX as u128 { u64::MAX } else { quotient as u64 }
}

/// Q32.32 frames per tick for an SMPTE-divided file: `sample_rate_hz / (fps * ticks_per_frame)`,
/// constant and never consulting the tempo map.
fn smpte_frames_per_tick_bits(sample_rate_hz: u32, frames_per_second: i8, ticks_per_frame: u8) -> u64 {
    let fps = (frames_per_second.unsigned_abs()).max(1) as u128;
    let ticks_per_frame = (ticks_per_frame.max(1)) as u128;
    let numerator = (sample_rate_hz as u128) << 32;
    let denominator = fps * ticks_per_frame;
    let quotient = numerator / denominator;
    if quotient > u64::MAX as u128 { u64::MAX } else { quotient as u64 }
}

/// Converts a monotonically increasing sequence of absolute ticks to absolute output
/// frames, one call to [`TickToFrameConverter::advance_to`] per tick, carrying the
/// sub-frame remainder exactly across tempo changes — the SMF equivalent of
/// [`starplayer_core::clock::FrameClock`].
struct TickToFrameConverter<'a> {
    division: Division,
    tempo_map: &'a [TempoChange],
    next_tempo: usize,
    current_micros_per_quarter_note: u32,
    sample_rate_hz: u32,
    last_tick: u64,
    frame: u64,
    remainder: Q32_32,
}

impl<'a> TickToFrameConverter<'a> {
    fn new(division: Division, tempo_map: &'a [TempoChange], sample_rate_hz: u32) -> TickToFrameConverter<'a> {
        TickToFrameConverter {
            division,
            tempo_map,
            next_tempo: 0,
            current_micros_per_quarter_note: DEFAULT_MICROS_PER_QUARTER_NOTE,
            sample_rate_hz,
            last_tick: 0,
            frame: 0,
            remainder: Q32_32::ZERO,
        }
    }

    fn segment_bits(&self) -> u64 {
        match self.division {
            Division::TicksPerQuarterNote(ppqn) => ppqn_frames_per_tick_bits(self.sample_rate_hz, self.current_micros_per_quarter_note, ppqn),
            Division::Smpte { frames_per_second, ticks_per_frame } => smpte_frames_per_tick_bits(self.sample_rate_hz, frames_per_second, ticks_per_frame),
        }
    }

    /// Advance from wherever the converter last stopped to `target_tick`, applying every
    /// tempo change at or before it along the way, and return the new absolute frame.
    ///
    /// `target_tick` must not be less than the previous call's: [`Smf::to_frames`] and
    /// [`Smf::length_frames`] only ever call this with ticks already sorted ascending.
    fn advance_to(&mut self, target_tick: u64) -> u64 {
        if matches!(self.division, Division::TicksPerQuarterNote(_)) {
            while let Some(change) = self.tempo_map.get(self.next_tempo) {
                if change.tick > target_tick {
                    break;
                }
                self.advance_by(change.tick.saturating_sub(self.last_tick));
                self.current_micros_per_quarter_note = change.micros_per_quarter_note;
                self.next_tempo += 1;
            }
        }
        self.advance_by(target_tick.saturating_sub(self.last_tick));
        self.frame
    }

    fn advance_by(&mut self, delta_ticks: u64) {
        let segment = (delta_ticks as u128) * (self.segment_bits() as u128);
        let segment_bits = if segment > u64::MAX as u128 { u64::MAX } else { segment as u64 };
        self.remainder = self.remainder.saturating_add(Q32_32::from_bits(segment_bits));
        self.frame = self.frame.saturating_add(self.remainder.take_whole() as u64);
        self.last_tick = self.last_tick.saturating_add(delta_ticks);
    }
}

// ── the parser ──────────────────────────────────────────────────────────────────────

/// A cursor over a byte slice with bounds-checked reads, mirroring the style of every
/// other loader in this repository (e.g. `starplayer-mtm`'s `Source`).
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Cursor<'a> { Cursor { bytes, pos: 0 } }

    fn remaining(&self) -> usize { self.bytes.len().saturating_sub(self.pos) }

    fn peek_u8(&self) -> Result<u8, Error> {
        self.bytes.get(self.pos).copied().ok_or(Error::Truncated { offset: self.pos, needed: 1 })
    }

    fn u8(&mut self) -> Result<u8, Error> {
        let byte = self.peek_u8()?;
        self.pos += 1;
        Ok(byte)
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).ok_or(Error::TooLarge("SMF chunk"))?;
        let slice = self.bytes.get(self.pos..end).ok_or(Error::Truncated { offset: self.pos, needed: n })?;
        self.pos = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let slice = self.bytes(N)?;
        let mut array = [0u8; N];
        array.copy_from_slice(slice);
        Ok(array)
    }

    fn u16_be(&mut self) -> Result<u16, Error> { Ok(u16::from_be_bytes(self.array()?)) }

    fn u32_be(&mut self) -> Result<u32, Error> { Ok(u32::from_be_bytes(self.array()?)) }

    /// A variable-length quantity: seven bits per byte, most significant byte first, the
    /// high bit marking every byte but the last (RP-001). At most four bytes, so the
    /// largest legal value is `0x0FFF_FFFF`; a fifth continuation byte is malformed.
    fn vlq(&mut self) -> Result<u32, Error> {
        let mut value: u32 = 0;
        for _ in 0..4 {
            let byte = self.u8()?;
            value = (value << 7) | (byte & 0x7F) as u32;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::Invalid("SMF variable-length quantity longer than four bytes"))
    }
}

/// Parse a Standard MIDI File.
///
/// Formats 0 and 1 are supported; format 2 (independent, unsynchronised songs sharing one
/// file) is [`Error::Unsupported`], because it has no single tempo map or event timeline
/// for [`Smf::to_frames`] to produce — a host that wants it has to demultiplex the file
/// into one [`Smf`] per song itself, which this function does not attempt to guess at.
///
/// Malformed input is rejected rather than guessed at: a truncated variable-length
/// quantity, a chunk whose declared length runs past the end of the file, or a track with
/// no `end_of_track` meta event all return `Err`. A chunk type other than `MTrk` after
/// the header is forward-compatible padding (RP-001) and is skipped by its declared
/// length rather than rejected.
pub fn parse_smf(bytes: &[u8]) -> Result<Smf, Error> {
    let mut cursor = Cursor::new(bytes);
    let magic: [u8; 4] = cursor.array()?;
    if magic != HEADER_MAGIC {
        return Err(Error::BadMagic);
    }
    let header_length = cursor.u32_be()? as usize;
    if header_length < 6 {
        return Err(Error::Invalid("SMF header chunk shorter than six bytes"));
    }
    let format = cursor.u16_be()?;
    let track_count = cursor.u16_be()?;
    let raw_division = cursor.u16_be()?;
    // A forward-compatible file may carry header bytes beyond the six standard ones;
    // nothing here needs them.
    let _ = cursor.bytes(header_length - 6)?;

    if format >= 2 {
        return Err(Error::Unsupported("SMF format 2 (independent songs)"));
    }
    let division = if raw_division & 0x8000 == 0 {
        Division::TicksPerQuarterNote(raw_division.max(1))
    } else {
        Division::Smpte { frames_per_second: (raw_division >> 8) as i8, ticks_per_frame: (raw_division & 0xFF) as u8 }
    };

    let mut events: Vec<TickEvent> = Vec::new();
    let mut tempo_map: Vec<TempoChange> = Vec::new();
    let mut end_of_track_ticks: Vec<u64> = Vec::new();

    while end_of_track_ticks.len() < track_count as usize {
        if cursor.remaining() == 0 {
            return Err(Error::Truncated { offset: cursor.pos, needed: track_count as usize - end_of_track_ticks.len() });
        }
        let chunk_id: [u8; 4] = cursor.array()?;
        let chunk_length = cursor.u32_be()? as usize;
        let chunk_data = cursor.bytes(chunk_length)?;
        if chunk_id != TRACK_MAGIC {
            // Forward-compatible padding: a chunk type this parser does not know is
            // skipped by its declared length, per RP-001.
            continue;
        }
        let track_index = end_of_track_ticks.len() as u16;
        let end_of_track_tick = parse_track(track_index, chunk_data, &mut events, &mut tempo_map)?;
        end_of_track_ticks.push(end_of_track_tick);
    }

    events.sort_by_key(|event| (event.tick, event.track, event.sequence));
    tempo_map.sort_by_key(|change| (change.tick, change.track, change.sequence));
    let length_ticks = end_of_track_ticks.iter().copied().max().unwrap_or(0);

    Ok(Smf { format, division, track_count, events, tempo_map, length_ticks })
}

/// Parse one track's events, appending channel voice events to `events` and `set_tempo`
/// metas to `tempo_map`. Returns the track's `end_of_track` tick, or `Err` if the track
/// never reaches one.
fn parse_track(track_index: u16, data: &[u8], events: &mut Vec<TickEvent>, tempo_map: &mut Vec<TempoChange>) -> Result<u64, Error> {
    let mut cursor = Cursor::new(data);
    let mut tick: u64 = 0;
    // RP-001: "The first event in each MTrk chunk must specify status" — so a track
    // starts with no running status to fall back on.
    let mut running_status: Option<u8> = None;
    let mut sequence: u32 = 0;

    loop {
        if cursor.remaining() == 0 {
            // The chunk ended cleanly, on an event boundary, without ever seeing
            // `end_of_track` — distinct from a VLQ or event that starts and then runs
            // out of bytes mid-read, which the reads below report as `Truncated`.
            return Err(Error::Invalid("SMF track missing end of track"));
        }
        let delta = cursor.vlq()?;
        tick = tick.saturating_add(delta as u64);
        let byte = cursor.peek_u8()?;

        if byte == 0xFF {
            cursor.u8()?;
            let meta_type = cursor.u8()?;
            let length = cursor.vlq()? as usize;
            let meta_data = cursor.bytes(length)?;
            // RP-001: "Sysex events and meta events cancel any running status which was
            // in effect."
            running_status = None;
            if meta_type == META_END_OF_TRACK {
                return Ok(tick);
            }
            if meta_type == META_SET_TEMPO {
                if length != 3 {
                    return Err(Error::Invalid("SMF set_tempo meta event must be three bytes"));
                }
                let micros = ((meta_data[0] as u32) << 16) | ((meta_data[1] as u32) << 8) | meta_data[2] as u32;
                tempo_map.push(TempoChange { tick, micros_per_quarter_note: micros.max(1), track: track_index, sequence });
                sequence = sequence.saturating_add(1);
            }
            // Every other meta type (track name, time signature, key signature, ...) is
            // consumed above and otherwise ignored: nothing `Smf::to_frames` emits comes
            // from it.
            continue;
        }

        if byte == 0xF0 || byte == 0xF7 {
            // RP-001 sysex framing: the status byte, then a variable-length quantity
            // naming exactly how many bytes follow — distinct from the delimiter-framed
            // sysex a live MIDI stream uses (`crate::codec::MidiDecoder`).
            cursor.u8()?;
            let length = cursor.vlq()? as usize;
            let _ = cursor.bytes(length)?;
            running_status = None;
            continue;
        }

        if byte & 0x80 != 0 {
            if !(0x80..=0xEF).contains(&byte) {
                return Err(Error::Invalid("SMF track contains a status byte no channel voice message or event framing uses"));
            }
            cursor.u8()?;
            running_status = Some(byte);
        }
        let status = match running_status {
            Some(status) => status,
            None => return Err(Error::Invalid("SMF running status used with none in effect")),
        };
        let data_len = channel_message_data_len(status) as usize;
        let data_bytes = cursor.bytes(data_len)?;
        let mut message_data = [0u8; 2];
        message_data[..data_len].copy_from_slice(data_bytes);
        if let Some((channel, event)) = decode_message(status, &message_data) {
            events.push(TickEvent { tick, track: track_index, sequence, channel, event });
            sequence = sequence.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use starplayer_core::{ChannelId, I1F15, Note, U0F16};

    use super::*;

    /// Build a minimal `MThd` + one `MTrk` file: `format` 0 or 1, one track whose body is
    /// `track_body` with a mandatory end-of-track meta appended.
    fn smf_bytes(format: u16, track_count: u16, division: u16, track_bodies: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&HEADER_MAGIC);
        bytes.extend_from_slice(&6u32.to_be_bytes());
        bytes.extend_from_slice(&format.to_be_bytes());
        bytes.extend_from_slice(&track_count.to_be_bytes());
        bytes.extend_from_slice(&division.to_be_bytes());
        for body in track_bodies {
            bytes.extend_from_slice(&TRACK_MAGIC);
            let mut track = Vec::new();
            track.extend_from_slice(body);
            track.extend_from_slice(&[0x00, 0xFF, META_END_OF_TRACK, 0x00]);
            bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
            bytes.extend_from_slice(&track);
        }
        bytes
    }

    #[test]
    fn probe_recognises_the_header_magic_and_nothing_else() {
        assert!(probe(&smf_bytes(0, 1, 96, &[&[]])));
        assert!(!probe(b"RIFF____WAVEfmt "));
        assert!(!probe(&[]));
    }

    #[test]
    fn a_non_mthd_file_reports_bad_magic() {
        assert_eq!(parse_smf(b"not a midi file"), Err(Error::BadMagic));
    }

    #[test]
    fn format_two_is_unsupported() {
        let bytes = smf_bytes(2, 1, 96, &[&[]]);
        assert!(matches!(parse_smf(&bytes), Err(Error::Unsupported(_))));
    }

    #[test]
    fn a_track_without_end_of_track_is_rejected() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&HEADER_MAGIC);
        bytes.extend_from_slice(&6u32.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&96u16.to_be_bytes());
        bytes.extend_from_slice(&TRACK_MAGIC);
        let track: &[u8] = &[0x00, 0x90, 60, 64]; // a note on, no end-of-track
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(track);
        assert!(matches!(parse_smf(&bytes), Err(Error::Invalid(_))));
    }

    #[test]
    fn a_truncated_variable_length_quantity_is_rejected() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&HEADER_MAGIC);
        bytes.extend_from_slice(&6u32.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&96u16.to_be_bytes());
        bytes.extend_from_slice(&TRACK_MAGIC);
        let track: &[u8] = &[0x81]; // continuation bit set, no following byte
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(track);
        assert!(matches!(parse_smf(&bytes), Err(Error::Truncated { .. })));
    }

    #[test]
    fn a_chunk_length_past_the_end_of_the_file_is_rejected() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&HEADER_MAGIC);
        bytes.extend_from_slice(&6u32.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&96u16.to_be_bytes());
        bytes.extend_from_slice(&TRACK_MAGIC);
        bytes.extend_from_slice(&1000u32.to_be_bytes()); // far past what follows
        bytes.extend_from_slice(&[0x00]);
        assert!(matches!(parse_smf(&bytes), Err(Error::Truncated { .. })));
    }

    #[test]
    fn running_status_decodes_a_note_on_then_a_note_off_with_no_repeated_status() {
        let track: &[u8] = &[0x00, 0x90, 60, 100, 0x60, 61, 0];
        let bytes = smf_bytes(0, 1, 96, &[track]);
        let smf = parse_smf(&bytes).expect("a minimal format-0 file parses");
        assert_eq!(smf.event_count(), 2);
        let frames = smf.to_frames(44_100);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].event, Event::NoteOn { note: Note::from_midi(60), velocity: starplayer_core::fixed::unit_from_midi7(100) });
        assert_eq!(frames[1].event, Event::NoteOff { note: Note::from_midi(61), velocity: U0F16::ZERO });
    }

    #[test]
    fn sysex_and_unknown_meta_events_are_skipped_without_disturbing_the_events_around_them() {
        let track: &[u8] = &[
            0x00, 0x90, 60, 100, // note on
            0x00, 0xF0, 0x03, 0x7E, 0x00, 0xF7, // sysex, three bytes, self-terminated
            0x00, 0xFF, 0x03, 0x04, b'n', b'a', b'm', b'e', // track name meta, ignored
            0x00, 0x80, 60, 0, // note off (explicit status: sysex cancelled running status)
        ];
        let bytes = smf_bytes(1, 1, 96, &[track]);
        let smf = parse_smf(&bytes).expect("parses");
        assert_eq!(smf.event_count(), 2);
    }

    #[test]
    fn a_tempo_change_is_recorded_and_frames_reflect_it() {
        // Two ticks per event at 96 PPQN: 500ms of silence at the default tempo (120 BPM)
        // is 48000 ticks; instead use a tiny division and a well-known tempo change.
        let track: &[u8] = &[
            0x00, 0xFF, META_SET_TEMPO, 0x03, 0x07, 0xA1, 0x20, // 500000 -> 500000 (120 BPM), explicit at tick 0
            0x60, 0x90, 60, 100, // note on 96 ticks later
        ];
        let bytes = smf_bytes(0, 1, 96, &[track]);
        let smf = parse_smf(&bytes).expect("parses");
        assert_eq!(smf.tempo_changes().len(), 1);
        assert_eq!(smf.tempo_changes()[0].micros_per_quarter_note, 500_000);
        let frames = smf.to_frames(44_100);
        // 96 ticks at 96 PPQN is exactly one quarter note; at 120 BPM a quarter note is
        // 0.5s, so at 44100 Hz that is exactly 22050 frames.
        assert_eq!(frames[0].frame, Frame(22_050));
    }

    #[test]
    fn length_frames_extends_past_the_last_event_to_the_end_of_track_tick() {
        // A note on at tick 0, then 480 ticks of trailing silence before end-of-track.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&HEADER_MAGIC);
        bytes.extend_from_slice(&6u32.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&96u16.to_be_bytes());
        bytes.extend_from_slice(&TRACK_MAGIC);
        let mut track = vec![0x00, 0x90, 60, 100];
        track.extend_from_slice(&[0x83, 0x60, 0xFF, META_END_OF_TRACK, 0x00]); // delta 480 (0x83 0x60 VLQ) then end of track
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&track);

        let smf = parse_smf(&bytes).expect("parses");
        let frames = smf.to_frames(44_100);
        assert_eq!(frames.len(), 1);
        assert!(smf.length_frames(44_100) > frames[0].frame.get(), "the file's length must include the trailing silence");
    }

    #[test]
    fn smpte_division_ignores_the_tempo_map_and_uses_a_fixed_rate() {
        // 0xE7 = -25 (25 fps), 0x01 = one tick per frame: division 0xE701. Chosen so
        // 44100 Hz / 25 fps is an exact integer (1764) and the Q32.32 per-tick value
        // carries no truncated fraction, which keeps this test's expected frame count
        // unambiguous regardless of how many ticks are summed per call.
        let track: &[u8] = &[0x01, 0x90, 60, 100]; // one tick later = exactly one frame
        let bytes = smf_bytes(0, 1, 0xE701, &[track]);
        let smf = parse_smf(&bytes).expect("parses");
        assert!(matches!(smf.division(), Division::Smpte { frames_per_second: -25, ticks_per_frame: 1 }));
        let frames = smf.to_frames(44_100);
        // One 25fps frame at 44100 Hz is exactly 1764 output frames.
        assert_eq!(frames[0].frame, Frame(1_764));
    }

    #[test]
    fn events_from_two_tracks_merge_by_tick_with_a_stable_per_track_tie_break() {
        let first_track: &[u8] = &[0x00, 0x90, 60, 100];
        let second_track: &[u8] = &[0x00, 0x91, 61, 100]; // same tick (0), different track
        let bytes = smf_bytes(1, 2, 96, &[first_track, second_track]);
        let smf = parse_smf(&bytes).expect("parses");
        let frames = smf.to_frames(44_100);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].frame, frames[1].frame, "both events are at tick zero");
        assert_eq!(frames[0].event, Event::NoteOn { note: Note::from_midi(60), velocity: starplayer_core::fixed::unit_from_midi7(100) }, "track 0 sorts before track 1");
        assert_eq!(frames[1].event, Event::NoteOn { note: Note::from_midi(61), velocity: starplayer_core::fixed::unit_from_midi7(100) });
    }

    #[test]
    fn a_channel_carries_through_to_the_engines_midi_channel_mapping() {
        let track: &[u8] = &[0x00, 0x93, 60, 100]; // channel 3
        let bytes = smf_bytes(0, 1, 96, &[track]);
        let smf = parse_smf(&bytes).expect("parses");
        let frames = smf.to_frames(44_100);
        assert_eq!(frames[0].target, Target::Channel(midi_channel(3)));
        assert_eq!(frames[0].target, Target::Channel(ChannelId(51)));
    }

    #[test]
    fn pitch_bend_and_controller_events_decode_through_the_shared_codec() {
        let track: &[u8] = &[0x00, 0xE0, 0x00, 0x40, 0x00, 0xB0, 7, 100];
        let bytes = smf_bytes(0, 1, 96, &[track]);
        let smf = parse_smf(&bytes).expect("parses");
        let frames = smf.to_frames(44_100);
        assert_eq!(frames[0].event, Event::PitchBend(I1F15::ZERO));
        assert_eq!(frames[1].event, Event::Controller { number: 7, value: starplayer_core::fixed::unit_from_midi7(100) });
    }
}
