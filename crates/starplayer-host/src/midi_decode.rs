//! A byte-at-a-time MIDI channel-voice decoder.
//!
//! **E5 supersedes this whole module.** Task E5 lands `starplayer_midi::MidiDecoder` with
//! the same shape — fed one byte at a time, yielding `(channel, Event)` — plus system
//! common, the SMF parser and the encoder this does not have. E6 needs a decoder before
//! that arrives, so this is the subset both native MIDI input and Web MIDI actually use,
//! written against E5's stated API so the swap at merge is:
//!
//! ```text
//! -use crate::midi_decode::MidiDecoder;
//! +use starplayer::midi::MidiDecoder;
//! ```
//!
//! and this file, its `pub use` in `lib.rs`, and this note all go.
//!
//! # What it decodes
//!
//! The eight channel voice messages, with running status; system real-time bytes
//! (`0xF8`–`0xFF`) pass through anywhere, including between the data bytes of a message,
//! without disturbing it; system exclusive is skipped to its `0xF7`; system common clears
//! running status, as the specification requires. Velocities and controller values become
//! [`U0F16`] through `unit_from_midi7` and pitch bend an [`I1F15`] through
//! `bipolar_from_midi_bend`, so MIDI's 7 and 14 bits never become the engine's internal
//! representation (architecture §2.3).
//!
//! Nothing here allocates or can panic: the decoder is a fixed struct with a two-byte data
//! buffer, and the browser host feeds it from inside `process()`.

use starplayer::core::fixed::{bipolar_from_midi_bend, unit_from_midi7};
use starplayer::core::{Event, InstrumentId, Note};

/// Controller numbers the rack gives a meaning of their own.
const CONTROLLER_ALL_SOUND_OFF: u8 = 120;
const CONTROLLER_ALL_NOTES_OFF: u8 = 123;

/// A MIDI 1.0 channel-voice decoder.
#[derive(Clone, Copy, Debug, Default)]
pub struct MidiDecoder {
    /// The running status byte, or zero when there is none.
    status: u8,
    /// Data bytes collected for the message in progress.
    data: [u8; 2],
    collected: u8,
    /// Whether a system-exclusive message is being skipped.
    in_sysex: bool,
}

impl MidiDecoder {
    /// A decoder with no running status.
    pub const fn new() -> MidiDecoder { MidiDecoder { status: 0, data: [0; 2], collected: 0, in_sysex: false } }

    /// Forget the running status and any half-collected message — what a port that has
    /// just been opened, or a stream that has been interrupted, starts from.
    pub const fn reset(&mut self) { *self = MidiDecoder::new(); }

    /// Feed one byte, and hand back `(channel, event)` when it completes a message this
    /// engine has a meaning for.
    pub fn feed(&mut self, byte: u8) -> Option<(u8, Event)> {
        // System real-time. Interleaved anywhere, including mid-message, and it must not
        // disturb the message in progress or the running status.
        if byte >= 0xF8 {
            return None;
        }
        if byte >= 0x80 {
            return self.begin(byte);
        }
        if self.in_sysex || self.status == 0 {
            return None;
        }
        self.data[usize::from(self.collected).min(1)] = byte;
        self.collected = self.collected.saturating_add(1);
        if u32::from(self.collected) < data_bytes_for(self.status) {
            return None;
        }
        self.collected = 0;
        message_to_event(self.status, self.data[0], self.data[1])
    }

    /// A status byte: start a message, a sysex, or clear running status.
    fn begin(&mut self, status: u8) -> Option<(u8, Event)> {
        self.collected = 0;
        match status {
            0xF0 => {
                self.in_sysex = true;
                self.status = 0;
            }
            0xF7 => {
                self.in_sysex = false;
                self.status = 0;
            }
            // System common. The specification says it clears running status.
            0xF1..=0xF6 => {
                self.in_sysex = false;
                self.status = 0;
            }
            _ => {
                self.in_sysex = false;
                self.status = status;
            }
        }
        None
    }
}

/// Data bytes a channel-voice status byte carries.
const fn data_bytes_for(status: u8) -> u32 {
    match status & 0xF0 {
        0xC0 | 0xD0 => 1,
        _ => 2,
    }
}

/// One complete channel-voice message as `(channel, event)`.
///
/// Public because the browser sends whole messages rather than a byte stream: a Web MIDI
/// `MIDIMessageEvent` is already framed, so the page packs status, data1 and data2 into one
/// wire record and the worklet converts it here without a state machine.
pub fn message_to_event(status: u8, data1: u8, data2: u8) -> Option<(u8, Event)> {
    let channel = status & 0x0F;
    let note = Note::new(data1 & 0x7F);
    let event = match status & 0xF0 {
        0x80 => Event::NoteOff { note, velocity: unit_from_midi7(data2 & 0x7F) },
        // A note-on with zero velocity is a note-off. Every sequencer written since
        // running status became common relies on it.
        0x90 if data2 & 0x7F == 0 => Event::NoteOff { note, velocity: Default::default() },
        0x90 => Event::NoteOn { note, velocity: unit_from_midi7(data2 & 0x7F) },
        0xA0 => Event::PolyAftertouch { note, pressure: unit_from_midi7(data2 & 0x7F) },
        0xB0 => match data1 & 0x7F {
            CONTROLLER_ALL_SOUND_OFF => Event::AllSoundOff,
            CONTROLLER_ALL_NOTES_OFF => Event::AllNotesOff,
            number => Event::Controller { number: number as u16, value: unit_from_midi7(data2 & 0x7F) },
        },
        0xC0 => Event::Program(InstrumentId((data1 & 0x7F) as u16)),
        0xD0 => Event::ChannelAftertouch(unit_from_midi7(data1 & 0x7F)),
        0xE0 => Event::PitchBend(bipolar_from_midi_bend(u16::from(data1 & 0x7F) | (u16::from(data2 & 0x7F) << 7))),
        _ => return None,
    };
    Some((channel, event))
}

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer::core::{I1F15, U0F16};

    fn decode(bytes: &[u8]) -> Vec<(u8, Event)> {
        let mut decoder = MidiDecoder::new();
        bytes.iter().filter_map(|byte| decoder.feed(*byte)).collect()
    }

    #[test]
    fn the_eight_channel_voice_messages_decode_to_their_events() {
        assert_eq!(decode(&[0x90, 60, 100]), vec![(0, Event::NoteOn { note: Note::new(60), velocity: unit_from_midi7(100) })]);
        assert_eq!(decode(&[0x93, 60, 0]), vec![(3, Event::NoteOff { note: Note::new(60), velocity: U0F16::ZERO })], "velocity zero is a note-off");
        assert_eq!(decode(&[0x85, 62, 64]), vec![(5, Event::NoteOff { note: Note::new(62), velocity: unit_from_midi7(64) })]);
        assert_eq!(decode(&[0xA0, 62, 20]), vec![(0, Event::PolyAftertouch { note: Note::new(62), pressure: unit_from_midi7(20) })]);
        assert_eq!(decode(&[0xB1, 7, 100]), vec![(1, Event::Controller { number: 7, value: unit_from_midi7(100) })]);
        assert_eq!(decode(&[0xB1, 120, 0]), vec![(1, Event::AllSoundOff)]);
        assert_eq!(decode(&[0xB1, 123, 0]), vec![(1, Event::AllNotesOff)]);
        assert_eq!(decode(&[0xC2, 9]), vec![(2, Event::Program(InstrumentId(9)))], "program change carries one data byte");
        assert_eq!(decode(&[0xD2, 77]), vec![(2, Event::ChannelAftertouch(unit_from_midi7(77)))]);
        assert_eq!(decode(&[0xE0, 0x00, 0x40]), vec![(0, Event::PitchBend(I1F15::ZERO))], "8192 is centre");
    }

    #[test]
    fn running_status_repeats_the_last_channel_voice_message() {
        let decoded = decode(&[0x90, 60, 100, 62, 100, 64, 0]);
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].1, Event::NoteOn { note: Note::new(60), velocity: unit_from_midi7(100) });
        assert_eq!(decoded[2].1, Event::NoteOff { note: Note::new(64), velocity: U0F16::ZERO });
    }

    #[test]
    fn a_real_time_byte_inside_a_message_does_not_disturb_it() {
        // Clock (0xF8), start (0xFA) and active sensing (0xFE) may appear between any two
        // bytes of any message, including between a status byte and its data.
        let decoded = decode(&[0x90, 0xF8, 60, 0xFE, 100, 0xFA, 62, 100]);
        assert_eq!(decoded.len(), 2, "both notes survived the interleaving");
        assert_eq!(decoded[0].1, Event::NoteOn { note: Note::new(60), velocity: unit_from_midi7(100) });
        assert_eq!(decoded[1].1, Event::NoteOn { note: Note::new(62), velocity: unit_from_midi7(100) });
    }

    #[test]
    fn system_exclusive_is_skipped_and_clears_running_status() {
        let decoded = decode(&[0x90, 60, 100, 0xF0, 0x7E, 0x00, 0x06, 0x01, 0xF7, 62, 100, 0x90, 64, 100]);
        assert_eq!(decoded.len(), 2, "the sysex body decoded to nothing and the bytes after it had no status");
        assert_eq!(decoded[1].1, Event::NoteOn { note: Note::new(64), velocity: unit_from_midi7(100) });
    }

    #[test]
    fn system_common_clears_running_status() {
        // Song position pointer, then two bytes that would have been a note under the
        // running status the specification says it just cleared.
        let decoded = decode(&[0x90, 60, 100, 0xF2, 0x00, 0x10, 62, 100]);
        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn a_data_byte_before_any_status_is_ignored_rather_than_guessed_at() {
        assert!(decode(&[60, 100, 62, 100]).is_empty());
    }

    #[test]
    fn the_whole_message_form_agrees_with_the_byte_stream_form() {
        for status in [0x90u8, 0x85, 0xA2, 0xB3, 0xE7] {
            let streamed = decode(&[status, 40, 90]);
            assert_eq!(streamed, message_to_event(status, 40, 90).into_iter().collect::<Vec<_>>(), "status {status:#04x}");
        }
        assert_eq!(message_to_event(0xF0, 0, 0), None, "a system message is not a channel voice message");
    }

    #[test]
    fn pitch_bend_reads_its_fourteen_bits_low_byte_first() {
        let (_, event) = message_to_event(0xE0, 0x7F, 0x7F).expect("a bend");
        assert_eq!(event, Event::PitchBend(bipolar_from_midi_bend(16_383)));
        let (_, event) = message_to_event(0xE0, 0x00, 0x00).expect("a bend");
        assert_eq!(event, Event::PitchBend(bipolar_from_midi_bend(0)));
    }

    #[test]
    fn a_reset_forgets_the_running_status() {
        let mut decoder = MidiDecoder::new();
        assert_eq!(decoder.feed(0x90), None);
        assert_eq!(decoder.feed(60), None);
        decoder.reset();
        assert_eq!(decoder.feed(100), None, "the half-collected note went with the reset");
    }
}
