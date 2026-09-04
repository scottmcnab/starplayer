//! Live input: the clock a host stamps events against, and the [`EventSender`] that pushes
//! them at the engine's [`ExternalEventQueue`].
//!
//! # Which clock an event is stamped against
//!
//! The master plan writes the stamp as `output_frame + lead`. That is right for an engine
//! that has never been paused and wrong for every other one, and a [`Player`](crate::Player)
//! is always paused at least once: [`Player::open`](crate::Player::open) freezes the musical
//! clock before the stream starts, so the output clock is already ahead by however long the
//! caller took to press Play. The engine dispatches sources against
//! [`Engine::source_frame`](starplayer::engine::Engine::source_frame) — the **musical**
//! clock, the one that stops with the transport — so that is the clock a live event has to
//! be stamped in. Stamping in the output clock instead would delay every note by exactly
//! the time the transport had spent stopped, and stop the keyboard entirely after a long
//! pause. See this task's research resolution, point 2.
//!
//! # The lead, and why it has a floor
//!
//! An event is only sample-exact if it is still in the *future* when the audio thread
//! reaches it. The control thread's view of the musical clock is republished once per
//! callback, so during a callback it is up to one whole device block stale — and an event
//! stamped less than a block ahead therefore lands inside audio that has already been
//! rendered. The engine tolerates that (it dispatches at the current frame and counts the
//! event late), but the timing is then quantised to the block rather than to the sample.
//!
//! So the effective lead is `max(requested, largest block seen + RENDER_QUANTUM)`. The
//! block size is *observed* rather than asked for, because a device that was given no
//! preference answers with its own: PulseAudio's WSLg sink hands over 96 000 frames at a
//! time. On a worklet, where the block is always 128 frames, the floor is 256 — the two
//! quanta the master plan chose, 5.3 ms at 48 kHz.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use starplayer::core::{ChannelId, Event, Frame, TimedEvent};
use starplayer::engine::{ExternalEventProducer, RENDER_QUANTUM, midi_channel};

use crate::backend::HostError;

/// Events a live-input queue holds before it starts refusing them.
///
/// A keyboard produces a handful of events per keystroke and a MIDI port a few hundred a
/// second at its busiest; 256 is several seconds of the worst of that, and the ring is
/// allocated once, off the audio thread.
pub const EVENT_QUEUE_CAPACITY: usize = 256;

/// The stamping lead a host asks for before anything has measured a block size: two render
/// quanta, which is master-plan decision 6.
pub const DEFAULT_EVENT_LEAD_FRAMES: u32 = 2 * RENDER_QUANTUM as u32;

/// The clock, the lead policy and the counters an [`EventSender`] shares with its
/// [`Player`](crate::Player).
///
/// Shared rather than copied because the sender may live on another thread — a `midir`
/// callback, a plugin host's event thread — while the player that made it goes on being
/// steered from the control thread. Every field is a relaxed atomic: nothing here is
/// ordered against anything else, and the audio side only ever *writes* two of them.
#[derive(Debug)]
pub struct EventClock {
    /// The engine's musical clock, republished once per callback.
    source_frame: AtomicU64,
    /// The largest block the backend has asked for so far.
    observed_block_frames: AtomicU32,
    /// The lead the caller asked for, which the effective lead never goes below.
    requested_lead_frames: AtomicU32,
    /// Events accepted onto the queue.
    sent: AtomicU64,
    /// Events a full queue refused.
    rejected: AtomicU64,
    /// The rate the stream negotiated, so a lead can be reported in milliseconds.
    sample_rate_hz: u32,
}

impl EventClock {
    /// A clock for a stream running at `sample_rate_hz`.
    pub fn new(sample_rate_hz: u32) -> Arc<EventClock> {
        Arc::new(EventClock {
            source_frame: AtomicU64::new(0),
            observed_block_frames: AtomicU32::new(0),
            requested_lead_frames: AtomicU32::new(DEFAULT_EVENT_LEAD_FRAMES),
            sent: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            sample_rate_hz,
        })
    }

    /// The musical clock as of the last callback — the frame a live event is stamped from.
    pub fn source_frame(&self) -> Frame { Frame(self.source_frame.load(Ordering::Relaxed)) }

    /// The lead the caller asked for, before the block-size floor is applied.
    pub fn requested_lead_frames(&self) -> u32 { self.requested_lead_frames.load(Ordering::Relaxed) }

    /// Ask for a lead. The effective lead never drops below what the device's own block
    /// size requires — see this module's preamble.
    pub fn set_requested_lead_frames(&self, frames: u32) {
        self.requested_lead_frames.store(frames, Ordering::Relaxed);
    }

    /// The largest block the backend has asked for since the stream opened.
    pub fn observed_block_frames(&self) -> u32 { self.observed_block_frames.load(Ordering::Relaxed) }

    /// The lead actually applied to the next event.
    pub fn lead_frames(&self) -> u32 {
        let floor = self.observed_block_frames().saturating_add(RENDER_QUANTUM as u32);
        self.requested_lead_frames().max(floor)
    }

    /// [`EventClock::lead_frames`] in milliseconds, which is what a UI shows.
    pub fn lead_millis(&self) -> f32 {
        match self.sample_rate_hz {
            0 => 0.0,
            rate => self.lead_frames() as f32 * 1_000.0 / rate as f32,
        }
    }

    /// The rate the stream negotiated.
    pub const fn sample_rate_hz(&self) -> u32 { self.sample_rate_hz }

    /// Events accepted onto the queue.
    pub fn sent(&self) -> u64 { self.sent.load(Ordering::Relaxed) }

    /// Events a full queue refused. Never a panic and never a block — the honest answer to
    /// a host sending faster than the audio thread consumes is to say so.
    pub fn rejected(&self) -> u64 { self.rejected.load(Ordering::Relaxed) }

    /// Republish the musical clock. Called once per callback, from the audio thread.
    pub(crate) fn publish(&self, source_frame: Frame) {
        self.source_frame.store(source_frame.0, Ordering::Relaxed);
    }

    /// Record the block size the backend asked for. Called once per callback, from the
    /// audio thread; a `fetch_max` rather than a store so a single enormous block raises
    /// the floor and a later small one does not lower it back.
    pub(crate) fn observe_block(&self, frames: u32) {
        self.observed_block_frames.fetch_max(frames, Ordering::Relaxed);
    }
}

/// The sending half of a [`Player`](crate::Player)'s live-input queue.
///
/// It is `Send` and deliberately **not** `Sync`: the queue underneath is a single-producer
/// ring, so exactly one thread may hold this at a time. A native host moves it into the
/// `midir` callback; the browser host leaves it inside the player, where the worklet's
/// command drain reaches it.
///
/// Nothing here allocates, locks, or can panic, so it is safe to call from an audio
/// callback as well as from a control thread.
pub struct EventSender {
    producer: ExternalEventProducer,
    clock: Arc<EventClock>,
}

impl EventSender {
    /// Pair a producer with the clock its events are stamped against.
    ///
    /// [`Player::midi_only`](crate::Player::midi_only) is how a host normally gets one;
    /// this is public for a host driving an `ExternalEventQueue` it built itself, and for
    /// the `midir` round-trip test, which owns both ends of a virtual port.
    pub fn new(producer: ExternalEventProducer, clock: Arc<EventClock>) -> EventSender {
        EventSender { producer, clock }
    }

    /// Queue `event` on MIDI channel `channel` (0–15), stamped `source_frame + lead`.
    ///
    /// A full queue is counted and reported, never waited on.
    pub fn send_event(&mut self, channel: u8, event: Event) -> Result<(), HostError> {
        self.send_to(midi_channel(channel), event)
    }

    /// [`EventSender::send_event`] addressed at an engine lane directly, for a caller that
    /// already holds one.
    pub fn send_to(&mut self, channel: ChannelId, event: Event) -> Result<(), HostError> {
        let frame = self.clock.source_frame().saturating_add(self.clock.lead_frames() as u64);
        match self.producer.send(TimedEvent::on_channel(frame, channel, event)) {
            Ok(()) => {
                self.clock.sent.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(_rejected) => {
                self.clock.rejected.fetch_add(1, Ordering::Relaxed);
                Err(HostError::EventQueueFull)
            }
        }
    }

    /// The frame the next event would be stamped at, for a test that wants to see the
    /// stamp rather than infer it from when a note sounds.
    pub fn next_stamp(&self) -> Frame {
        self.clock.source_frame().saturating_add(self.clock.lead_frames() as u64)
    }

    /// The clock and counters this sender shares with the player that made it.
    pub fn clock(&self) -> &Arc<EventClock> { &self.clock }

    /// Events queued but not yet consumed by the audio thread.
    pub fn queued(&self) -> usize { self.producer.len() }

    /// How many events the queue holds.
    pub fn capacity(&self) -> usize { self.producer.capacity() }
}

impl core::fmt::Debug for EventSender {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("EventSender")
            .field("queued", &self.queued())
            .field("lead_frames", &self.clock.lead_frames())
            .field("rejected", &self.clock.rejected())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer::core::{Note, U0F16};
    use starplayer::engine::external_event_channel;

    fn note_on() -> Event { Event::NoteOn { note: Note::MIDDLE_C, velocity: U0F16::MAX } }

    #[test]
    fn the_lead_never_drops_below_a_whole_device_block_plus_a_quantum() {
        let clock = EventClock::new(48_000);
        assert_eq!(clock.lead_frames(), DEFAULT_EVENT_LEAD_FRAMES, "two quanta before any block has been seen");

        clock.observe_block(128);
        assert_eq!(clock.lead_frames(), 256, "a worklet's own block is already covered by the default");

        clock.observe_block(1_024);
        assert_eq!(clock.lead_frames(), 1_024 + RENDER_QUANTUM as u32, "a 1024-frame device block raises the floor");
        clock.observe_block(128);
        assert_eq!(clock.lead_frames(), 1_024 + RENDER_QUANTUM as u32, "and a later small block does not lower it");

        clock.set_requested_lead_frames(8_192);
        assert_eq!(clock.lead_frames(), 8_192, "a caller asking for more gets more");
        clock.set_requested_lead_frames(0);
        assert_eq!(clock.lead_frames(), 1_024 + RENDER_QUANTUM as u32, "and asking for none still gets the floor");
    }

    #[test]
    fn the_lead_reads_back_in_milliseconds() {
        let clock = EventClock::new(48_000);
        assert!((clock.lead_millis() - 256.0 * 1_000.0 / 48_000.0).abs() < 1e-4);
        assert_eq!(EventClock::new(0).lead_millis(), 0.0, "a rate of zero reports nothing rather than dividing by it");
    }

    #[test]
    fn an_event_is_stamped_on_the_musical_clock_plus_the_lead() {
        let clock = EventClock::new(48_000);
        let (producer, mut queue) = external_event_channel(8);
        let mut sender = EventSender::new(producer, Arc::clone(&clock));

        clock.publish(Frame(10_000));
        assert_eq!(sender.next_stamp(), Frame(10_256));
        assert!(sender.send_event(0, note_on()).is_ok());

        use starplayer::engine::EventFeed;
        queue.refresh(Frame(10_000));
        assert_eq!(queue.next_frame(), Some(Frame(10_256)));
        let due = queue.pop_due(Frame(10_256)).expect("the event is due at its stamp");
        assert_eq!(due.target, starplayer::core::Target::Channel(midi_channel(0)));
        assert_eq!(clock.sent(), 1);
        assert_eq!(clock.rejected(), 0);
    }

    #[test]
    fn a_full_queue_is_counted_and_reported_rather_than_waited_on() {
        let clock = EventClock::new(48_000);
        let (producer, _queue) = external_event_channel(2);
        let mut sender = EventSender::new(producer, Arc::clone(&clock));

        assert!(sender.send_event(0, note_on()).is_ok());
        assert!(sender.send_event(0, note_on()).is_ok());
        assert_eq!(sender.send_event(0, note_on()), Err(HostError::EventQueueFull));
        assert_eq!(clock.sent(), 2);
        assert_eq!(clock.rejected(), 1);
        assert_eq!(sender.capacity(), 2);
        assert_eq!(sender.queued(), 2);
    }

    #[test]
    fn a_midi_channel_past_sixteen_wraps_rather_than_reaching_a_tracker_lane() {
        let clock = EventClock::new(48_000);
        let (producer, mut queue) = external_event_channel(4);
        let mut sender = EventSender::new(producer, clock);
        assert!(sender.send_event(16, note_on()).is_ok());

        use starplayer::engine::EventFeed;
        queue.refresh(Frame(0));
        let due = queue.pop_due(Frame(u64::MAX)).expect("queued");
        assert_eq!(due.target, starplayer::core::Target::Channel(ChannelId(48)));
    }
}
