//! The `midir` round trip, over a virtual port this test creates itself.
//!
//! # Why it may skip, and why that is not a hole
//!
//! A virtual port needs a MIDI stack the process can register a client with: an ALSA
//! sequencer (`/dev/snd/seq`) on Linux, CoreMIDI on macOS. Windows has no virtual-port
//! concept in WinMM at all, and a container or a WSL2 box without `/dev/snd` has no
//! sequencer either — which is exactly the machine this task was written on. The test
//! therefore **skips with a printed reason** rather than failing, and everything it would
//! have proved about the decode is proved without any hardware by
//! `starplayer-host`'s `midi_decode` tests (byte stream → `Event`, running status, real-time
//! bytes mid-message) and by its `tests/live_input.rs` (`Event` → a sounding note). What is
//! left for this test is the one link those cannot reach: that `midir` really does deliver
//! a message to the callback this crate installs.

#![cfg(unix)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use midir::os::unix::VirtualInput;
use midir::{Ignore, MidiInput, MidiOutput};
use starplayer::core::{Event, Note};
use starplayer::core::fixed::unit_from_midi7;
use starplayer::engine::external_event_channel;
use starplayer_host::{EventClock, EventSender, MidiDecoder};

const VIRTUAL_PORT: &str = "starplayer-e6-test-in";

/// How long the test waits for the platform to deliver a message it has already sent.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);

/// The decode-and-push callback `starplayer_midi_native::open_input` installs, driven here
/// over a port this test owns both ends of. It cannot call `open_input` itself, because
/// that opens a port the platform enumerated rather than a virtual one — creating a
/// virtual port is a different `midir` entry point.
fn on_message(message: &[u8], state: &mut (MidiDecoder, EventSender, Arc<AtomicUsize>)) {
    for byte in message {
        if let Some((channel, event)) = state.0.decode(*byte) {
            let _ = state.1.send_event(channel, event);
            state.2.fetch_add(1, Ordering::SeqCst);
        }
    }
}

#[test]
fn a_virtual_port_round_trips_a_note_into_the_players_queue() {
    let Ok(input) = MidiInput::new("starplayer-e6-test") else {
        println!("SKIPPED: this machine has no MIDI stack (no ALSA sequencer or CoreMIDI client), so no virtual port can be created");
        return;
    };
    let Ok(output) = MidiOutput::new("starplayer-e6-test-out") else {
        println!("SKIPPED: this machine enumerates MIDI input but not output, so nothing can drive the virtual port");
        return;
    };

    let clock = EventClock::new(48_000);
    let (producer, mut queue) = external_event_channel(64);
    let sender = EventSender::new(producer, Arc::clone(&clock));
    let delivered = Arc::new(AtomicUsize::new(0));

    let mut input = input;
    input.ignore(Ignore::None);
    let state = (MidiDecoder::new(), sender, Arc::clone(&delivered));
    let Ok(_connection) = input.create_virtual(VIRTUAL_PORT, |_timestamp, message, state| on_message(message, state), state)
    else {
        println!("SKIPPED: the MIDI stack refused a virtual input port");
        return;
    };

    // Find the port that was just created and drive it.
    let ports = output.ports();
    let Some(port) = ports.iter().find(|port| output.port_name(port).unwrap_or_default().contains(VIRTUAL_PORT)) else {
        println!("SKIPPED: the virtual port was created but the output side cannot see it");
        return;
    };
    let mut connection = output.connect(port, "starplayer-e6-test").expect("the virtual port opens");

    connection.send(&[0x90, 60, 100]).expect("note on");
    // Running status: the second note carries no status byte of its own.
    connection.send(&[62, 100]).expect("a running-status note on");
    connection.send(&[0x80, 60, 64]).expect("note off");

    let deadline = Instant::now() + DELIVERY_TIMEOUT;
    while delivered.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert_eq!(delivered.load(Ordering::SeqCst), 3, "three messages crossed the virtual port");

    use starplayer::engine::EventFeed;
    let mut received = Vec::new();
    for _ in 0..3 {
        queue.refresh(starplayer::core::Frame(0));
        received.push(queue.pop_due(starplayer::core::Frame(u64::MAX)).expect("an event reached the queue").event);
    }
    assert_eq!(received[0], Event::NoteOn { note: Note::new(60), velocity: unit_from_midi7(100) });
    assert_eq!(received[1], Event::NoteOn { note: Note::new(62), velocity: unit_from_midi7(100) }, "running status survived the port");
    assert_eq!(received[2], Event::NoteOff { note: Note::new(60), velocity: unit_from_midi7(64) });
    assert_eq!(clock.rejected(), 0);
}
