//! Native MIDI **input**: a `midir` port, decoded on the port's own callback thread and
//! pushed at a [`Player`](starplayer_host::Player)'s live-input queue.
//!
//! ```no_run
//! use starplayer::engine::MixerMode;
//! use starplayer_host::{AudioSpec, ManualBackend, Player};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Any `AudioBackend` — `starplayer_host_cpal::CpalBackend` on a real machine.
//! let mut backend = ManualBackend::new();
//! let mut player = Player::open(&mut backend, None, AudioSpec::stereo(48_000), MixerMode::DEFAULT)?;
//! player.load(&std::fs::read("song.s3m")?)?;
//! player.midi_only()?;                                    // the module's instruments, on live input
//! player.play()?;
//! let sender = player.take_event_sender().expect("live input installed a sender");
//! // Held for as long as the notes should keep arriving; dropping it closes the port.
//! let _connection = starplayer_midi_native::open_input(Some("keystation"), sender)?;
//! # Ok(())
//! # }
//! ```
//!
//! # Why this is not part of `starplayer-host-cpal` (task E6, research point 1)
//!
//! Because it is not an audio backend. `starplayer-host-cpal` is the crate that turns an
//! output device into an [`AudioBackend`](starplayer_host::AudioBackend), and its own
//! documentation scopes it to "what is genuinely platform" *for output* — device
//! enumeration and an `i16` conversion. MIDI input shares none of that: no `AudioSpec`, no
//! stream, no negotiation, no render callback. Putting it there would mean a host on any
//! other audio backend — a JACK one, the plugin host M9 plans, a future embedded host —
//! had to depend on cpal to read a keyboard.
//!
//! What the two *do* share on Linux is `alsa-lib`, and the header story is therefore
//! identical: cpal binds the PCM interface and `midir` the **sequencer** interface
//! (`snd_seq_*`) out of the same library, so `sudo apt install libasound2-dev` — or the
//! `PKG_CONFIG_PATH` this repository's `~/.cargo/config.toml` sets to a user-built
//! `alsa-lib` prefix — satisfies both. Nothing further is needed.
//!
//! # Real-time rules
//!
//! `midir` calls back on a thread of its own — an ALSA sequencer poll loop, a CoreMIDI
//! notification thread, a WinMM callback. Everything reachable from that callback here is
//! allocation-free and non-blocking: a fixed decoder state machine, and one push onto the
//! player's SPSC ring. A full ring is counted and the event dropped, never waited on.

#![forbid(unsafe_code)]

use std::fmt;

use midir::{Ignore, MidiInput, MidiInputConnection};
use starplayer_host::{EventSender, MidiDecoder};

/// The client name this crate registers with the platform's MIDI stack; it is what shows
/// up in `aconnect -l`, Audio MIDI Setup and the like.
pub const CLIENT_NAME: &str = "StarPlayer";

/// What can go wrong between a host and a MIDI port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiError {
    /// The platform has no MIDI stack this build can reach: no ALSA sequencer device, no
    /// CoreMIDI server. Common in containers and on WSL2, and not an error worth failing a
    /// player over — `--list-midi-ports` prints it and carries on.
    Unavailable(String),
    /// The stack is there and enumerated no input port.
    NoPorts,
    /// A `--midi` selector matched nothing.
    UnknownPort(String),
    /// The port was found and would not open.
    Backend(String),
}

impl fmt::Display for MidiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MidiError::Unavailable(reason) => write!(formatter, "no MIDI support on this system: {reason}"),
            MidiError::NoPorts => formatter.write_str("no MIDI input port is available"),
            MidiError::UnknownPort(selector) => write!(formatter, "no MIDI input port matches `{selector}`"),
            MidiError::Backend(message) => write!(formatter, "the MIDI backend failed: {message}"),
        }
    }
}

impl std::error::Error for MidiError {}

/// One MIDI input port, as a chooser sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiPortInfo {
    /// Position in the platform's own list — what `--midi 2` names.
    pub index: usize,
    /// The port's name, which a `--midi` selector also matches against.
    pub name: String,
}

/// Every MIDI input port this build can see.
///
/// [`MidiError::Unavailable`] rather than an empty list when the platform has no MIDI stack
/// at all, because "there is no MIDI here" and "there is MIDI here and nothing plugged in"
/// are different answers to a caller deciding what to print.
pub fn input_ports() -> Result<Vec<MidiPortInfo>, MidiError> {
    let input = new_input()?;
    Ok(input
        .ports()
        .iter()
        .enumerate()
        .map(|(index, port)| MidiPortInfo {
            index,
            name: input.port_name(port).unwrap_or_else(|_| String::from("(unnamed)")),
        })
        .collect())
}

/// An open input port. Dropping it closes the port and stops the events.
pub struct MidiConnection {
    connection: MidiInputConnection<PortState>,
    port_name: String,
}

impl MidiConnection {
    /// The name of the port that is open, for a caller that let the selector choose.
    pub fn port_name(&self) -> &str { &self.port_name }

    /// Close the port and hand the [`EventSender`] back, so it can be moved to another port
    /// or another player.
    pub fn close(self) -> EventSender {
        let (_input, state) = self.connection.close();
        state.sender
    }
}

impl fmt::Debug for MidiConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("MidiConnection").field("port_name", &self.port_name).finish()
    }
}

/// What travels with the `midir` callback: the running-status decoder and the queue's
/// sending half. Both are owned by the callback thread, which is what makes the queue's
/// single-producer contract hold.
struct PortState {
    decoder: MidiDecoder,
    sender: EventSender,
}

/// Decode one platform message into the player's queue. The whole of what runs on the
/// platform's MIDI thread, kept in one named function so that "nothing here allocates" is
/// checkable by reading eight lines.
fn on_message(message: &[u8], state: &mut PortState) {
    for byte in message {
        if let Some((channel, event)) = state.decoder.feed(*byte) {
            // A refused event is counted on the shared `EventClock` and dropped. Blocking
            // here would stall the platform's own MIDI thread.
            let _ = state.sender.send_event(channel, event);
        }
    }
}

/// Open the input port `selector` names, or the first one when it names nothing.
///
/// `selector` matches, in order: a decimal index into [`input_ports`], an exact port name,
/// then any case-insensitive part of one — the same convention `--device` already uses for
/// output devices.
pub fn open_input(selector: Option<&str>, sender: EventSender) -> Result<MidiConnection, MidiError> {
    let mut input = new_input()?;
    // Nothing is filtered out: system real-time and sysex bytes reach the decoder, which is
    // what makes the running-status handling honest — a clock byte really can land between
    // a status byte and its data.
    input.ignore(Ignore::None);

    let ports = input.ports();
    let index = match selector {
        None => 0,
        Some(selector) => choose(&input, &ports, selector)?,
    };
    let port = ports.get(index).ok_or(MidiError::NoPorts)?.clone();
    let port_name = input.port_name(&port).unwrap_or_else(|_| String::from("(unnamed)"));

    let state = PortState { decoder: MidiDecoder::new(), sender };
    let connection = input
        .connect(&port, CLIENT_NAME, |_timestamp_micros, message, state: &mut PortState| on_message(message, state), state)
        .map_err(|error| MidiError::Backend(error.to_string()))?;
    Ok(MidiConnection { connection, port_name })
}

/// Index, exact name, then case-insensitive substring.
fn choose(input: &MidiInput, ports: &[midir::MidiInputPort], selector: &str) -> Result<usize, MidiError> {
    if ports.is_empty() {
        return Err(MidiError::NoPorts);
    }
    if let Ok(index) = selector.parse::<usize>()
        && index < ports.len()
    {
        return Ok(index);
    }
    let names: Vec<String> = ports.iter().map(|port| input.port_name(port).unwrap_or_default()).collect();
    if let Some(index) = names.iter().position(|name| name == selector) {
        return Ok(index);
    }
    let wanted = selector.to_lowercase();
    names
        .iter()
        .position(|name| name.to_lowercase().contains(&wanted))
        .ok_or_else(|| MidiError::UnknownPort(String::from(selector)))
}

fn new_input() -> Result<MidiInput, MidiError> {
    MidiInput::new(CLIENT_NAME).map_err(|error| MidiError::Unavailable(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unavailable_stack_and_an_unmatched_selector_read_differently() {
        assert_eq!(MidiError::NoPorts.to_string(), "no MIDI input port is available");
        assert!(MidiError::Unavailable(String::from("no /dev/snd/seq")).to_string().starts_with("no MIDI support"));
        assert!(MidiError::UnknownPort(String::from("nord")).to_string().contains("`nord`"));
    }

    /// Enumeration must not panic, whatever the machine has — including a machine with no
    /// MIDI stack at all, which is every container and every WSL2 box without `/dev/snd`.
    #[test]
    fn enumeration_answers_on_a_machine_with_no_midi_stack() {
        match input_ports() {
            Ok(ports) => {
                for (position, port) in ports.iter().enumerate() {
                    assert_eq!(port.index, position);
                }
            }
            Err(MidiError::Unavailable(_)) => {}
            Err(other) => panic!("enumeration should not fail with {other}"),
        }
    }
}
