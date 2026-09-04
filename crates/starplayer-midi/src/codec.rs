//! The MIDI 1.0 byte codec: a running-status-aware, byte-at-a-time decoder from wire
//! bytes to `(channel, Event)`, and an encoder back to wire bytes for the events that
//! round-trip.
//!
//! Architecture §2.3: "MIDI byte parsing lives in `starplayer-midi` as a codec that
//! converts to and from `Event`. MIDI's representation never becomes the internal one."
//! Nothing here keeps a MIDI-shaped value alive past this module.
//!
//! # What is not represented
//!
//! System exclusive is skipped, not captured: no [`Event`] variant carries raw sysex
//! bytes. System common (`0xF1..=0xF6`) and system real-time (`0xF8..=0xFF`) messages
//! carry no channel and map to nothing here; a decoder byte that completes one of them
//! simply produces no event. Real-time bytes are recognised and discarded wherever they
//! appear — including in the middle of a channel voice message still being assembled —
//! because the MIDI 1.0 specification allows exactly that interleaving without disturbing
//! the message in progress or its running status.

use starplayer_core::fixed::{bipolar_from_midi_bend, unit_from_midi7};
use starplayer_core::{Event, I1F15, InstrumentId, Note, U0F16};

/// Controller number 120: "All Sound Off". Decodes straight to [`Event::AllSoundOff`]
/// rather than [`Event::Controller`] (task E5 deliverable 1).
const CC_ALL_SOUND_OFF: u8 = 120;
/// Controller number 123: "All Notes Off". Decodes straight to [`Event::AllNotesOff`].
const CC_ALL_NOTES_OFF: u8 = 123;

/// How many data bytes follow a channel voice status byte.
///
/// Program Change and Channel Pressure carry one; every other channel voice message
/// carries two (MIDI 1.0 Detailed Specification, table of channel voice messages).
pub(crate) const fn channel_message_data_len(status: u8) -> u8 {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        _ => 2,
    }
}

/// How many data bytes follow a system common status byte. `0xF0` (sysex) and `0xF7`
/// (end of exclusive) are handled separately by their own framing and never reach this
/// function.
const fn system_common_data_len(status: u8) -> u8 {
    match status {
        0xF1 => 1, // MIDI Time Code quarter frame
        0xF2 => 2, // Song Position Pointer
        0xF3 => 1, // Song Select
        _ => 0,    // 0xF4, 0xF5 undefined; 0xF6 Tune Request carries no data
    }
}

/// Bytes still being collected for one channel voice or system common message.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Pending {
    status: u8,
    data: [u8; 2],
    data_len: u8,
    data_needed: u8,
}

/// A byte-at-a-time MIDI 1.0 decoder.
///
/// Feed it one byte at a time with [`MidiDecoder::decode`]. Running status is kept across
/// calls, so a stream that never repeats a status byte decodes exactly like one that
/// repeats it on every message — running status set by a channel voice message is used by
/// a following data byte with no status byte of its own, and is cleared by any system
/// common message and by the arrival of a new status byte of a different kind.
#[derive(Copy, Clone, Debug, Default)]
pub struct MidiDecoder {
    running_status: Option<u8>,
    pending: Option<Pending>,
    /// Set while skipping a system exclusive message's body, cleared at `0xF7` or by the
    /// arrival of any other status byte (the specification allows sysex to be aborted
    /// that way, without a formal terminator).
    in_sysex: bool,
}

impl MidiDecoder {
    /// A decoder with no running status and nothing pending.
    pub const fn new() -> MidiDecoder { MidiDecoder { running_status: None, pending: None, in_sysex: false } }

    /// Feed one byte. Returns the channel (0..=15) and event the byte completed, if any.
    pub fn decode(&mut self, byte: u8) -> Option<(u8, Event)> {
        // System real-time: passed through wherever it appears, including mid-message,
        // and never disturbs `pending` or `running_status` (MIDI 1.0 spec, "System
        // Real-Time Messages"). None of these map to an `Event`.
        if byte >= 0xF8 {
            return None;
        }

        if byte & 0x80 != 0 {
            self.in_sysex = false;
            return match byte {
                0xF0 => {
                    self.in_sysex = true;
                    self.pending = None;
                    None
                }
                0xF7 => {
                    // End of exclusive with nothing being skipped: nothing to do.
                    self.pending = None;
                    None
                }
                0xF1..=0xF6 => {
                    // System common cancels running status (MIDI 1.0 spec, "Running
                    // Status"), and none of these six messages map to an `Event`.
                    self.running_status = None;
                    let needed = system_common_data_len(byte);
                    self.pending = if needed == 0 { None } else { Some(Pending { status: byte, data: [0; 2], data_len: 0, data_needed: needed }) };
                    None
                }
                _ => {
                    // Channel voice: sets running status and starts collecting data bytes.
                    self.running_status = Some(byte);
                    self.pending = Some(Pending { status: byte, data: [0; 2], data_len: 0, data_needed: channel_message_data_len(byte) });
                    None
                }
            };
        }

        // A data byte.
        if self.in_sysex {
            return None;
        }
        let mut pending = match self.pending {
            Some(pending) => pending,
            None => {
                // No message in progress: fall back to running status, if any. A stray
                // data byte with no running status (the very first byte of a malformed
                // stream) is discarded.
                let status = self.running_status?;
                Pending { status, data: [0; 2], data_len: 0, data_needed: channel_message_data_len(status) }
            }
        };
        if (pending.data_len as usize) < pending.data.len() {
            pending.data[pending.data_len as usize] = byte;
        }
        pending.data_len += 1;

        if pending.data_len < pending.data_needed {
            self.pending = Some(pending);
            return None;
        }
        self.pending = None;
        decode_message(pending.status, &pending.data)
    }
}

/// Turn one complete status-plus-data message into `(channel, Event)`, or `None` for a
/// system common message that carries no channel.
pub(crate) fn decode_message(status: u8, data: &[u8; 2]) -> Option<(u8, Event)> {
    let channel = status & 0x0F;
    match status & 0xF0 {
        0x80 => Some((channel, Event::NoteOff { note: Note::from_midi(data[0] & 0x7F), velocity: unit_from_midi7(data[1] & 0x7F) })),
        0x90 => {
            let note = Note::from_midi(data[0] & 0x7F);
            let velocity = unit_from_midi7(data[1] & 0x7F);
            // "Note On with velocity 0" is the wire convention for Note Off (MIDI 1.0
            // spec, "Running Status"); honoured here so every consumer of `Event` sees a
            // real `NoteOff` rather than a silent `NoteOn`.
            if velocity == U0F16::ZERO {
                Some((channel, Event::NoteOff { note, velocity }))
            } else {
                Some((channel, Event::NoteOn { note, velocity }))
            }
        }
        0xA0 => Some((channel, Event::PolyAftertouch { note: Note::from_midi(data[0] & 0x7F), pressure: unit_from_midi7(data[1] & 0x7F) })),
        0xB0 => {
            let number = data[0] & 0x7F;
            match number {
                CC_ALL_SOUND_OFF => Some((channel, Event::AllSoundOff)),
                CC_ALL_NOTES_OFF => Some((channel, Event::AllNotesOff)),
                _ => Some((channel, Event::Controller { number: number as u16, value: unit_from_midi7(data[1] & 0x7F) })),
            }
        }
        0xC0 => Some((channel, Event::Program(InstrumentId((data[0] & 0x7F) as u16)))),
        0xD0 => Some((channel, Event::ChannelAftertouch(unit_from_midi7(data[0] & 0x7F)))),
        0xE0 => {
            let bend = ((data[1] as u16 & 0x7F) << 7) | (data[0] as u16 & 0x7F);
            Some((channel, Event::PitchBend(bipolar_from_midi_bend(bend))))
        }
        _ => None,
    }
}

/// Encode `event` as a wire message on `channel` (masked to 0..=15), for the events that
/// round-trip through a MIDI 1.0 channel voice or channel mode message. Everything else —
/// [`Event::KeyOff`], [`Event::FadeOut`], [`Event::Cut`] (none of which is a MIDI wire
/// concept), the tracker-native [`Event::Trigger`] and [`Event::Param`], and
/// [`Event::Tempo`] / [`Event::GlobalVolume`] (which an SMF carries as meta events, not a
/// channel voice message) — returns `None`.
///
/// The returned array always has three bytes; [`status_byte_data_len`] says how many of
/// them are meaningful for a given status byte — one for Program Change and Channel
/// Pressure, two for everything else this function produces. The third byte of a
/// one-data-byte message is always zero.
pub fn encode(channel: u8, event: &Event) -> Option<[u8; 3]> {
    let channel = channel & 0x0F;
    match *event {
        Event::NoteOn { note, velocity } => Some([0x90 | channel, note.to_midi(), midi7_from_unit(velocity)]),
        Event::NoteOff { note, velocity } => Some([0x80 | channel, note.to_midi(), midi7_from_unit(velocity)]),
        Event::PolyAftertouch { note, pressure } => Some([0xA0 | channel, note.to_midi(), midi7_from_unit(pressure)]),
        Event::ChannelAftertouch(pressure) => Some([0xD0 | channel, midi7_from_unit(pressure), 0]),
        Event::Controller { number, value } if number <= 0x7F => Some([0xB0 | channel, number as u8, midi7_from_unit(value)]),
        Event::Program(instrument) if instrument.0 <= 0x7F => Some([0xC0 | channel, instrument.0 as u8, 0]),
        Event::PitchBend(bend) => {
            let word = midi_bend_from_bipolar(bend);
            Some([0xE0 | channel, (word & 0x7F) as u8, (word >> 7) as u8])
        }
        Event::AllSoundOff => Some([0xB0 | channel, CC_ALL_SOUND_OFF, 0]),
        Event::AllNotesOff => Some([0xB0 | channel, CC_ALL_NOTES_OFF, 0]),
        _ => None,
    }
}

/// How many of [`encode`]'s three returned bytes are meaningful, given the status byte
/// (its low nibble, the channel, is ignored here).
pub const fn status_byte_data_len(status: u8) -> usize { channel_message_data_len(status) as usize }

/// The inverse of [`starplayer_core::fixed::unit_from_midi7`], rounded to nearest.
const fn midi7_from_unit(value: U0F16) -> u8 {
    (((value.to_bits() as u32) * 127 + (u16::MAX as u32) / 2) / (u16::MAX as u32)) as u8
}

/// The inverse of [`starplayer_core::fixed::bipolar_from_midi_bend`], rounded to nearest
/// and clamped to the 14-bit wire range.
const fn midi_bend_from_bipolar(value: I1F15) -> u16 {
    let bits = value.to_bits() as i32;
    let scaled = bits * 8192;
    let half = (i16::MAX as i32) / 2;
    let rounded = if scaled >= 0 { (scaled + half) / (i16::MAX as i32) } else { (scaled - half) / (i16::MAX as i32) };
    let bend = rounded + 8192;
    if bend < 0 { 0 } else if bend > 16383 { 16383 } else { bend as u16 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(decoder: &mut MidiDecoder, bytes: &[u8]) -> alloc::vec::Vec<(u8, Event)> {
        let mut events = alloc::vec::Vec::new();
        for &byte in bytes {
            if let Some(decoded) = decoder.decode(byte) {
                events.push(decoded);
            }
        }
        events
    }

    /// The specification's own worked example: a Note On, channel 0, note 60 (middle C),
    /// velocity 64.
    #[test]
    fn decodes_a_single_note_on() {
        let mut decoder = MidiDecoder::new();
        let events = decode_all(&mut decoder, &[0x90, 60, 64]);
        assert_eq!(events, [(0, Event::NoteOn { note: Note::from_midi(60), velocity: unit_from_midi7(64) })]);
    }

    #[test]
    fn a_note_on_with_zero_velocity_decodes_as_a_note_off() {
        let mut decoder = MidiDecoder::new();
        let events = decode_all(&mut decoder, &[0x91, 60, 0]);
        assert_eq!(events, [(1, Event::NoteOff { note: Note::from_midi(60), velocity: U0F16::ZERO })]);
    }

    /// Running status: only the first message carries a status byte; the next two reuse
    /// it (MIDI 1.0 spec, "Running Status").
    #[test]
    fn running_status_repeats_the_last_channel_voice_status() {
        let mut decoder = MidiDecoder::new();
        let events = decode_all(&mut decoder, &[0x90, 60, 64, 61, 70, 62, 0]);
        assert_eq!(
            events,
            [
                (0, Event::NoteOn { note: Note::from_midi(60), velocity: unit_from_midi7(64) }),
                (0, Event::NoteOn { note: Note::from_midi(61), velocity: unit_from_midi7(70) }),
                (0, Event::NoteOff { note: Note::from_midi(62), velocity: U0F16::ZERO }),
            ]
        );
    }

    /// A real-time byte lands inside a message still being assembled and must neither
    /// produce an event of its own nor disturb the message around it.
    #[test]
    fn real_time_bytes_interleave_a_message_without_disturbing_it() {
        let mut decoder = MidiDecoder::new();
        let events = decode_all(&mut decoder, &[0x90, 0xF8, 60, 0xFA, 64]);
        assert_eq!(events, [(0, Event::NoteOn { note: Note::from_midi(60), velocity: unit_from_midi7(64) })]);
    }

    #[test]
    fn system_exclusive_is_skipped_entirely() {
        let mut decoder = MidiDecoder::new();
        let events = decode_all(&mut decoder, &[0xF0, 0x7E, 0x00, 0x09, 0x01, 0xF7, 0x90, 60, 64]);
        assert_eq!(events, [(0, Event::NoteOn { note: Note::from_midi(60), velocity: unit_from_midi7(64) })]);
    }

    /// A status byte other than 0xF7 aborts sysex without a formal terminator, and
    /// running status resumes normally afterwards.
    #[test]
    fn a_new_status_byte_aborts_an_unterminated_sysex() {
        let mut decoder = MidiDecoder::new();
        let events = decode_all(&mut decoder, &[0xF0, 0x7E, 0x00, 0x90, 60, 64]);
        assert_eq!(events, [(0, Event::NoteOn { note: Note::from_midi(60), velocity: unit_from_midi7(64) })]);
    }

    #[test]
    fn program_change_and_channel_aftertouch_carry_one_data_byte() {
        let mut decoder = MidiDecoder::new();
        let events = decode_all(&mut decoder, &[0xC3, 5, 0xD3, 100]);
        assert_eq!(
            events,
            [(3, Event::Program(InstrumentId(5))), (3, Event::ChannelAftertouch(unit_from_midi7(100)))]
        );
    }

    #[test]
    fn cc120_and_cc123_decode_as_all_sound_off_and_all_notes_off() {
        let mut decoder = MidiDecoder::new();
        let events = decode_all(&mut decoder, &[0xB2, 120, 0, 0xB2, 123, 0]);
        assert_eq!(events, [(2, Event::AllSoundOff), (2, Event::AllNotesOff)]);
    }

    #[test]
    fn an_ordinary_controller_decodes_as_a_controller_event() {
        let mut decoder = MidiDecoder::new();
        let events = decode_all(&mut decoder, &[0xB0, 7, 100]);
        assert_eq!(events, [(0, Event::Controller { number: 7, value: unit_from_midi7(100) })]);
    }

    #[test]
    fn pitch_bend_centre_decodes_as_zero() {
        let mut decoder = MidiDecoder::new();
        // 14-bit centre 8192 = 0x2000: LSB 0x00, MSB 0x40.
        let events = decode_all(&mut decoder, &[0xE0, 0x00, 0x40]);
        assert_eq!(events, [(0, Event::PitchBend(I1F15::ZERO))]);
    }

    #[test]
    fn pitch_bend_extremes_decode_as_the_documented_asymmetric_range() {
        let mut decoder = MidiDecoder::new();
        let bottom = decode_all(&mut decoder, &[0xE0, 0x00, 0x00]);
        assert_eq!(bottom, [(0, Event::PitchBend(-I1F15::MAX))]);
        let mut decoder = MidiDecoder::new();
        let top = decode_all(&mut decoder, &[0xE0, 0x7F, 0x7F]);
        assert_eq!(top, [(0, Event::PitchBend(starplayer_core::fixed::bipolar_from_ratio(8191, 8192)))]);
    }

    #[test]
    fn encode_round_trips_note_on_through_the_decoder() {
        let bytes = encode(5, &Event::NoteOn { note: Note::from_midi(72), velocity: U0F16::MAX }).expect("note on encodes");
        let mut decoder = MidiDecoder::new();
        let events = decode_all(&mut decoder, &bytes);
        assert_eq!(events, [(5, Event::NoteOn { note: Note::from_midi(72), velocity: unit_from_midi7(127) })]);
    }

    #[test]
    fn every_seven_bit_velocity_round_trips_through_the_unit_conversion() {
        for value in 0u8..=127 {
            let unit = unit_from_midi7(value);
            assert_eq!(midi7_from_unit(unit), value, "velocity {value} must round trip through U0F16");
        }
    }

    #[test]
    fn the_pitch_bend_anchors_round_trip_exactly() {
        assert_eq!(midi_bend_from_bipolar(I1F15::ZERO), 8192);
        assert_eq!(midi_bend_from_bipolar(-I1F15::MAX), 0);
        assert_eq!(midi_bend_from_bipolar(starplayer_core::fixed::bipolar_from_ratio(8191, 8192)), 16383);
    }

    #[test]
    fn encode_reports_none_for_events_with_no_midi_wire_form() {
        assert_eq!(encode(0, &Event::KeyOff), None);
        assert_eq!(encode(0, &Event::FadeOut), None);
        assert_eq!(encode(0, &Event::Cut), None);
        assert_eq!(encode(0, &Event::Tempo { bpm: 120, speed: 6 }), None);
        assert_eq!(encode(0, &Event::GlobalVolume(U0F16::MAX)), None);
    }

    #[test]
    fn encode_reports_the_all_sound_and_notes_off_controllers() {
        assert_eq!(encode(4, &Event::AllSoundOff), Some([0xB4, CC_ALL_SOUND_OFF, 0]));
        assert_eq!(encode(4, &Event::AllNotesOff), Some([0xB4, CC_ALL_NOTES_OFF, 0]));
    }

    #[test]
    fn status_byte_data_len_matches_what_encode_actually_fills_in() {
        assert_eq!(status_byte_data_len(0xC0), 1);
        assert_eq!(status_byte_data_len(0xD0), 1);
        assert_eq!(status_byte_data_len(0x90), 2);
        assert_eq!(status_byte_data_len(0xB0), 2);
        assert_eq!(status_byte_data_len(0xE0), 2);
    }

    #[test]
    fn channel_is_masked_to_the_low_nibble() {
        let event = Event::NoteOn { note: Note::from_midi(60), velocity: U0F16::MAX };
        assert_eq!(encode(0x1F, &event), encode(0x0F, &event));
    }
}
