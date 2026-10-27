//! [`SmfSequencer`] — an [`EventFeed`] over one Standard MIDI File's sorted event list
//! (feature `smf`).

use alloc::vec::Vec;

use starplayer_core::{Frame, TimedEvent};
use starplayer_engine::EventFeed;

use crate::smf::Smf;

/// A cursor over one [`Smf`]'s events, converted once to absolute output frames at
/// construction time and played back in order through [`MidiSource`](starplayer_engine::MidiSource).
///
/// [`EventFeed::refresh`] is left at its default no-op: every event is already known at
/// construction, unlike [`ExternalEventQueue`](starplayer_engine::ExternalEventQueue)'s
/// live ring, which has to peek an SPSC channel it cannot see all of at once.
pub struct SmfSequencer {
    events: Vec<TimedEvent>,
    length_frames: u64,
    cursor: usize,
    #[cfg(feature = "interactive-audition")]
    held_at: Option<Frame>,
    #[cfg(feature = "interactive-audition")]
    translation: u64,
    #[cfg(feature = "interactive-audition")]
    song_origin: i64,
    #[cfg(feature = "interactive-audition")]
    sample_rate_hz: u32,
}

impl SmfSequencer {
    /// Convert `smf` to frames at `sample_rate_hz` and start a cursor at the beginning of
    /// the file.
    pub fn new(smf: &Smf, sample_rate_hz: u32) -> SmfSequencer {
        SmfSequencer::new_at(smf, sample_rate_hz, Frame::ZERO)
    }

    /// Convert `smf` to frames and place its frame zero at `start_frame` on an already
    /// running engine's monotonic source clock.
    pub fn new_at(smf: &Smf, sample_rate_hz: u32, start_frame: Frame) -> SmfSequencer {
        let mut events = smf.to_frames(sample_rate_hz);
        for event in &mut events {
            event.frame = start_frame.saturating_add(event.frame.get());
        }
        SmfSequencer { events, length_frames: smf.length_frames(sample_rate_hz), cursor: 0,
            #[cfg(feature = "interactive-audition")] held_at: None,
            #[cfg(feature = "interactive-audition")] translation: 0,
            #[cfg(feature = "interactive-audition")] song_origin: start_frame.0.min(i64::MAX as u64) as i64,
            #[cfg(feature = "interactive-audition")] sample_rate_hz,
        }
    }

    /// Prepare song lanes0–15 without enabling interactive transport.
    #[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
    pub fn prepared_song_at(smf: &Smf, sample_rate_hz: u32, start_frame: Frame) -> Self {
        let mut source = Self::new_at(smf, sample_rate_hz, start_frame);
        for event in &mut source.events {
            if let starplayer_core::Target::Channel(channel) = &mut event.target { channel.0 = channel.0.saturating_sub(starplayer_engine::MIDI_CHANNEL_BASE); }
        }
        source
    }

    /// Prepare the interactive lane mapping and optional sound-free rebuild cursor.
    /// All event-vector work is on the control thread, never in dispatch.
    #[cfg(feature = "interactive-audition")]
    pub fn interactive_at(smf: &Smf, sample_rate_hz: u32, start_frame: Frame, cursor: Option<starplayer_core::SmfCursor>) -> Self {
        let mut source = Self::new_at(smf, sample_rate_hz, start_frame);
        for event in &mut source.events {
            if let starplayer_core::Target::Channel(channel) = &mut event.target {
                channel.0 = channel.0.saturating_sub(starplayer_engine::MIDI_CHANNEL_BASE);
            }
        }
        if let Some(cursor) = cursor {
            let cursor = cursor.converted(sample_rate_hz);
            source.cursor = (cursor.event_index as usize).min(source.events.len());
            source.song_origin = (start_frame.0.min(i64::MAX as u64) as i64).saturating_sub(cursor.song_frame.min(i64::MAX as u64) as i64);
            if let Some(next) = source.events.get(source.cursor).map(|event| event.frame) {
                let deadline = start_frame.saturating_add(cursor.remaining_frames.unwrap_or(0));
                for event in source.events.iter_mut().skip(source.cursor) { event.frame = deadline.saturating_add(next.frames_until(event.frame)); }
            }
        }
        source
    }

    /// The file's own length in output frames: the latest `end_of_track` tick across
    /// every track, converted through the tempo map — not merely the last event's frame,
    /// so trailing silence after the last note is not lost. What the CLI's `info`
    /// prints and what a host uses to know when playback is over.
    pub const fn length_frames(&self) -> u64 { self.length_frames }

    /// Events not yet popped.
    pub fn remaining(&self) -> usize { self.events.len().saturating_sub(self.cursor) }

    /// Move the cursor to the first event at or after `frame`, without producing it.
    pub fn seek_frame(&mut self, frame: Frame) {
        self.cursor = self.events.partition_point(|event| event.frame < frame);
    }

    /// Rewind to the start of the file.
    pub fn restart(&mut self) { self.cursor = 0; }
}

impl EventFeed for SmfSequencer {
    #[cfg(feature = "interactive-audition")]
    fn set_transport(&mut self, state: starplayer_core::InteractiveTransport, frame: Frame) {
        if state == starplayer_core::InteractiveTransport::Playing {
            if let Some(held) = self.held_at.take() {
                let elapsed = held.frames_until(frame);
                self.translation = self.translation.saturating_add(elapsed);
                self.song_origin = self.song_origin.saturating_add(elapsed.min(i64::MAX as u64) as i64);
            }
        } else if self.held_at.is_none() { self.held_at = Some(frame); }
    }

    #[cfg(feature = "interactive-audition")]
    fn smf_cursor(&self, frame: Frame) -> Option<starplayer_core::SmfCursor> {
        let now = self.held_at.unwrap_or(frame);
        Some(starplayer_core::SmfCursor {
            event_index: self.cursor as u64,
            song_frame: (now.0.min(i64::MAX as u64) as i64).saturating_sub(self.song_origin).max(0) as u64,
            remaining_frames: self.events.get(self.cursor).map(|event| now.frames_until(event.frame.saturating_add(self.translation))),
            captured_frame: frame, sample_rate_hz: self.sample_rate_hz,
        })
    }

    fn next_frame(&self) -> Option<Frame> {
        #[cfg(feature = "interactive-audition")]
        if self.held_at.is_some() { return None; }
        let frame = self.events.get(self.cursor)?.frame;
        #[cfg(feature = "interactive-audition")]
        let frame = frame.saturating_add(self.translation);
        Some(frame)
    }

    fn pop_due(&mut self, frame: Frame) -> Option<TimedEvent> {
        self.next_frame().filter(|next| *next <= frame)?;
        let event = *self.events.get(self.cursor)?;
        #[cfg(feature = "interactive-audition")]
        let event = TimedEvent { frame: event.frame.saturating_add(self.translation), ..event };
        if event.frame > frame {
            return None;
        }
        self.cursor += 1;
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use starplayer_core::{ChannelId, Event, Note, Target, U0F16};

    use super::*;
    use crate::smf::parse_smf;

    /// Two tracks, a tempo change and running status — the deliverable's own proof
    /// shape, reused here at the sequencer level.
    fn two_track_smf_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MThd");
        bytes.extend_from_slice(&6u32.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes()); // format 1
        bytes.extend_from_slice(&2u16.to_be_bytes()); // two tracks
        bytes.extend_from_slice(&96u16.to_be_bytes()); // 96 PPQN

        // Track 0: a tempo change to 120 BPM at tick 0, then a note on channel 0 at tick
        // 96 (one quarter note later), running status carrying its note off.
        let track0: &[u8] = &[
            0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, // set_tempo 500000us = 120 BPM
            0x60, 0x90, 60, 100, // note on, tick 96
            0x60, 60, 0, // note off via running status, tick 192
            0x00, 0xFF, 0x2F, 0x00, // end of track
        ];
        // Track 1: a note on channel 1 at tick 48 (mid-way through track 0's first gap).
        let track1: &[u8] = &[
            0x30, 0x91, 64, 90, // note on, tick 48
            0x30, 64, 0, // note off via running status, tick 96
            0x00, 0xFF, 0x2F, 0x00, // end of track
        ];
        for track in [track0, track1] {
            bytes.extend_from_slice(b"MTrk");
            bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
            bytes.extend_from_slice(track);
        }
        bytes
    }

    #[test]
    fn plays_every_event_in_frame_order_then_reports_no_more() {
        let smf = parse_smf(&two_track_smf_bytes()).expect("the hand-assembled file parses");
        let mut sequencer = SmfSequencer::new(&smf, 44_100);

        // 96 ticks at 96 PPQN and 120 BPM is exactly half a second: 22050 frames.
        let mut popped = Vec::new();
        while let Some(next) = sequencer.next_frame() {
            let event = sequencer.pop_due(next).expect("the reported next frame is always due");
            popped.push(event);
        }
        assert_eq!(popped.len(), 4, "two note-on/note-off pairs across two tracks");
        assert_eq!(popped[0].frame, Frame(11_025), "track 1's note on at tick 48 (a quarter of 22050 * 2)");
        assert_eq!(popped[0].target, Target::Channel(starplayer_engine::midi_channel(1)));
        assert_eq!(popped[1].frame, Frame(22_050));
        assert_eq!(popped[1].event, Event::NoteOn { note: Note::from_midi(60), velocity: starplayer_core::fixed::unit_from_midi7(100) });
        assert_eq!(popped[1].target, Target::Channel(ChannelId(48)));
        assert_eq!(popped[2].frame, Frame(22_050), "track 1's note off lands on the same frame as track 0's note on");
        assert_eq!(popped[3].frame, Frame(44_100));
        assert_eq!(popped[3].event, Event::NoteOff { note: Note::from_midi(60), velocity: U0F16::ZERO });

        assert_eq!(sequencer.next_frame(), None);
        assert_eq!(sequencer.pop_due(Frame(1_000_000)), None);
    }

    #[test]
    fn can_start_on_an_already_running_source_clock() {
        let smf = parse_smf(&two_track_smf_bytes()).expect("the hand-assembled file parses");
        let sequencer = SmfSequencer::new_at(&smf, 44_100, Frame(7_000));

        assert_eq!(sequencer.next_frame(), Some(Frame(18_025)), "the first event is rebased by the source-clock start");
        assert_eq!(sequencer.length_frames(), 44_100, "the reported file duration is not rebased");
    }

    #[test]
    fn seek_frame_moves_the_cursor_without_producing_skipped_events() {
        let smf = parse_smf(&two_track_smf_bytes()).expect("parses");
        let mut sequencer = SmfSequencer::new(&smf, 44_100);
        sequencer.seek_frame(Frame(22_050));
        assert_eq!(sequencer.remaining(), 3, "the three events at or after 22050 remain");
        assert_eq!(sequencer.next_frame(), Some(Frame(22_050)));
    }

    #[test]
    fn restart_rewinds_to_the_beginning() {
        let smf = parse_smf(&two_track_smf_bytes()).expect("parses");
        let mut sequencer = SmfSequencer::new(&smf, 44_100);
        sequencer.pop_due(sequencer.next_frame().expect("has a first event"));
        assert_eq!(sequencer.remaining(), 3);
        sequencer.restart();
        assert_eq!(sequencer.remaining(), 4);
        assert_eq!(sequencer.next_frame(), Some(Frame(11_025)));
    }

    #[test]
    fn length_frames_matches_the_smfs_own_end_of_track() {
        let smf = parse_smf(&two_track_smf_bytes()).expect("parses");
        let sequencer = SmfSequencer::new(&smf, 44_100);
        assert_eq!(sequencer.length_frames(), smf.length_frames(44_100));
        assert_eq!(sequencer.length_frames(), 44_100, "track 0's end of track sits at tick 192, right after its own last note off");
    }
}

#[cfg(all(test, feature = "interactive-audition"))]
mod interactive_tests {
    use super::*;
    use starplayer_core::{Event, InteractiveTransport, Target};
    #[test]
    fn smf_holds_only_its_own_deadlines_and_repeated_toggles_preserve_remainder() {
        let mut source = SmfSequencer { events: alloc::vec![TimedEvent { frame: Frame(1000), target: Target::Channel(starplayer_core::ChannelId(48)), event: Event::AllSoundOff }], length_frames: 2000, cursor: 0, held_at: None, translation: 0, song_origin: 0, sample_rate_hz: 44_100 };
        source.set_transport(InteractiveTransport::Paused, Frame(400));
        assert_eq!(source.next_frame(), None); assert!(source.pop_due(Frame(9000)).is_none());
        source.set_transport(InteractiveTransport::Stopped, Frame(500));
        source.set_transport(InteractiveTransport::Playing, Frame(2400));
        assert_eq!(source.next_frame(), Some(Frame(3000)));
        source.set_transport(InteractiveTransport::Paused, Frame(2600));
        source.set_transport(InteractiveTransport::Playing, Frame(2700));
        assert_eq!(source.next_frame(), Some(Frame(3100)));
        assert!(source.pop_due(Frame(3099)).is_none()); assert_eq!(source.pop_due(Frame(3100)).unwrap().frame, Frame(3100));
    }
}
