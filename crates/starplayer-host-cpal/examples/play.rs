//! Play a module on the default sound device.
//!
//! ```sh
//! cargo run -p starplayer-host-cpal --example play -- --list-devices
//! cargo run -p starplayer-host-cpal --example play -- song.s3m
//! cargo run -p starplayer-host-cpal --example play -- song.s3m --device rdp --rate 44100 --buffer 512
//! ```
//!
//! This is the acceptance test for the native host until the CLI's `play` command is
//! wired to the same [`Player`]. It is deliberately thin: everything interesting — the
//! decode, the scan, the transport, the seek mailbox, the retirement of the module the
//! audio thread has finished with — belongs to `starplayer-host`, and a host that had to
//! reimplement any of it here would be evidence that the split had failed.
//!
//! On Linux the build needs the ALSA headers (`sudo apt install libasound2-dev`), or a
//! `PKG_CONFIG_PATH` pointing at an `alsa-lib` prefix you built yourself; see this crate's
//! module documentation.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use starplayer::core::AtEnd;
use starplayer::engine::MixerMode;
use starplayer_host::{AudioSpec, DeviceInfo, HostError, Player, format_seconds};
use starplayer_host_cpal::{CpalBackend, all_devices};

/// How long the loop sleeps between polls of the telemetry.
///
/// Nothing here is timing-critical — the audio runs on cpal's own thread — so the interval
/// only has to be short enough that the "song is over" check is prompt and long enough
/// that a progress line is not a busy-wait.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a song that has ended is left ringing out before the process exits.
///
/// The transport stops the musical clock but the voices already sounding decay through the
/// mixer, and cutting the stream the instant the clock stops would clip that tail.
const RING_OUT: Duration = Duration::from_millis(400);

/// How long the loop waits for the first callback before deciding the device never started.
const STARTUP_GRACE: Duration = Duration::from_secs(3);

/// The fade a looping song is played out under, in seconds.
///
/// A song that loops has no end to play to, so the example plays one pass and fades; a
/// song that stops needs no fade and gets none.
const LOOP_FADE_SECONDS: u32 = 6;

/// The block size asked for when `--buffer` does not name one.
///
/// **Not** the device's own default, which is what "no preference" gets you and which can
/// be enormous: PulseAudio's WSLg sink hands over 96 000 frames — two seconds — in a single
/// callback, and a two-second playout buffer is two seconds of latency on every stop, seek
/// and progress reading. 1024 frames is 21 ms at 48 kHz, which is what a media player asks
/// for. The rendered audio is identical either way (design goal 3); only the latency is
/// not. `--buffer 0` asks for the device's own, for anyone who wants to hear that.
const DEFAULT_BUFFER_FRAMES: u32 = 1_024;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("play: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The parsed command line.
struct Options {
    module_path: Option<String>,
    device: Option<String>,
    sample_rate_hz: u32,
    buffer_frames: Option<u32>,
    list_devices: bool,
    // ── live MIDI input (task E6) ───────────────────────────────────────────────────
    midi: Option<String>,
    list_midi_ports: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            module_path: None,
            device: None,
            sample_rate_hz: 48_000,
            buffer_frames: Some(DEFAULT_BUFFER_FRAMES),
            list_devices: false,
            midi: None,
            list_midi_ports: false,
        }
    }
}

const USAGE: &str = "\
usage: play [--list-devices] [--device NAME] [--rate HZ] [--buffer FRAMES]
            [--midi PORT] [--list-midi-ports] <module>

  --list-devices     print every output device on every backend and exit
  --device NAME      an exact device name, or any case-insensitive part of one
  --rate HZ          the sample rate to ask the device for (default 48000)
  --buffer FRAMES    the block size to ask the device for (default 1024; 0 for the device's own)
  --midi PORT        play the module's instruments from a MIDI input port instead of
                     playing the module: an index, a name, or part of one
  --list-midi-ports  print every MIDI input port this build can see and exit
";

fn parse(arguments: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        let mut value = |name: &str| arguments.next().ok_or_else(|| std::format!("{name} needs a value"));
        match argument.as_str() {
            "--list-devices" => options.list_devices = true,
            "--list-midi-ports" => options.list_midi_ports = true,
            "--midi" => options.midi = Some(value("--midi")?),
            "--device" => options.device = Some(value("--device")?),
            "--rate" => {
                let raw = value("--rate")?;
                options.sample_rate_hz = raw.parse().map_err(|_| std::format!("--rate wants a number, not {raw:?}"))?;
            }
            "--buffer" => {
                let raw = value("--buffer")?;
                let frames: u32 = raw.parse().map_err(|_| std::format!("--buffer wants a number, not {raw:?}"))?;
                options.buffer_frames = (frames > 0).then_some(frames);
            }
            "--help" | "-h" => return Err(String::from(USAGE)),
            other if other.starts_with("--") => return Err(std::format!("unknown option {other}\n\n{USAGE}")),
            other => options.module_path = Some(String::from(other)),
        }
    }
    Ok(options)
}

fn run() -> Result<ExitCode, String> {
    let options = parse(std::env::args().skip(1))?;

    if options.list_devices {
        print_devices(&all_devices());
        return Ok(ExitCode::SUCCESS);
    }
    if options.list_midi_ports {
        print_midi_ports();
        return Ok(ExitCode::SUCCESS);
    }

    let Some(module_path) = options.module_path.clone() else { return Err(std::format!("no module given\n\n{USAGE}")) };
    let bytes = std::fs::read(&module_path).map_err(|error| std::format!("{module_path}: {error}"))?;

    let mut backend = CpalBackend::new().map_err(describe)?;
    let requested = AudioSpec {
        sample_rate_hz: options.sample_rate_hz,
        channels: MixerMode::DEFAULT.channels as u16,
        preferred_block_frames: options.buffer_frames,
    };
    let mut player = match Player::open(&mut backend, options.device.as_deref(), requested, MixerMode::DEFAULT) {
        Ok(player) => player,
        Err(error) => {
            eprintln!("play: could not open an output stream on the {} backend: {}", backend.host_name(), describe(error));
            eprintln!("play: `--list-devices` shows what this machine has.");
            return Ok(ExitCode::FAILURE);
        }
    };

    let spec = player.spec();
    println!("backend {} - device {}", backend.host_name(), options.device.as_deref().unwrap_or("(default)"));
    println!("stream  {spec}");

    player.load(&bytes).map_err(describe)?;
    let module_title = player.module().map(|module| String::from(module.header().title.as_ref())).unwrap_or_default();
    let song_length = player.song_length().unwrap_or(0);
    println!("module  {module_path}: {module_title:?}, {} frames ({})", song_length, format_seconds(song_length, spec.sample_rate_hz));

    player.set_fade_frames(LOOP_FADE_SECONDS.saturating_mul(spec.sample_rate_hz)).map_err(describe)?;
    // FadeOut covers both endings: a song that runs out of order list stops where it ends,
    // and a song that jumps back to a loop point plays one pass and fades.
    player.set_at_end(AtEnd::FadeOut).map_err(describe)?;

    // ── live MIDI input (task E6) ───────────────────────────────────────────────────
    //
    // The module's *instruments* on a live-input source, with its pattern data silent.
    // Hearing both at once is task E7's jam mode. Bound to a name because dropping the
    // connection closes the port.
    let _midi_connection = match options.midi.as_deref() {
        None => None,
        Some(selector) => {
            player.midi_only().map_err(describe)?;
            let sender = player.take_event_sender().ok_or("live input installed no sender")?;
            let connection = starplayer_midi_native::open_input(Some(selector), sender)
                .map_err(|error| std::format!("{error}; `--list-midi-ports` shows what this machine has"))?;
            println!("midi    {} - the module's instruments only; its pattern data is not playing", connection.port_name());
            println!("lead    {} frames ({:.1} ms) ahead of the audio clock", player.event_lead(), player.event_lead_millis());
            Some(connection)
        }
    };
    // ── end of the live MIDI input block ────────────────────────────────────────────

    player.play().map_err(describe)?;

    let outcome = follow(&mut player, spec.sample_rate_hz);
    player.stop().map_err(describe)?;
    std::thread::sleep(RING_OUT);
    player.collect_garbage();

    let warnings = player.warnings();
    if warnings.any() {
        println!("warnings: {warnings:?}");
    }
    match outcome {
        Outcome::Ended => {
            println!("done.");
            Ok(ExitCode::SUCCESS)
        }
        Outcome::CallbackNeverRan => {
            eprintln!("play: the device accepted the stream but never asked for audio.");
            Ok(ExitCode::FAILURE)
        }
        Outcome::DeviceFailed => {
            eprintln!("play: the device reported a fatal error and the stream is dead.");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// Why the follow loop stopped.
enum Outcome {
    /// The song reached its end, or the loop point and the end of its fade.
    Ended,
    /// The stream opened but no callback ever arrived.
    CallbackNeverRan,
    /// The backend's error callback latched something fatal.
    DeviceFailed,
}

/// Poll telemetry until the song is over, printing a line a second.
fn follow(player: &mut Player, sample_rate_hz: u32) -> Outcome {
    let started = Instant::now();
    let mut next_report = Instant::now();
    // The transport is queued, not immediate: `is_playing` is false for the first block or
    // two after `play()`, so "it stopped" only counts once it has been seen to start.
    let mut has_started = false;
    loop {
        if player.health().is_failed() {
            return Outcome::DeviceFailed;
        }
        if player.blocks_rendered() == 0 {
            if started.elapsed() > STARTUP_GRACE {
                return Outcome::CallbackNeverRan;
            }
        } else if player.is_playing() {
            has_started = true;
        } else if has_started {
            return Outcome::Ended;
        }

        if Instant::now() >= next_report {
            report(player, sample_rate_hz);
            next_report = Instant::now() + Duration::from_secs(1);
        }
        player.collect_garbage();
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// `--list-midi-ports`. A machine with no MIDI stack prints why rather than failing.
fn print_midi_ports() {
    match starplayer_midi_native::input_ports() {
        Err(error) => println!("{error}"),
        Ok(ports) if ports.is_empty() => println!("no MIDI input port is available"),
        Ok(ports) => {
            for port in ports {
                println!("{:>2}  {}", port.index, port.name);
            }
        }
    }
}

fn report(player: &mut Player, sample_rate_hz: u32) {
    let peak = player.peak();
    let snapshot = *player.telemetry();
    let transport = snapshot.transport;
    println!(
        "{:>8} / {:<8}  order {:>3} pattern {:>3} row {:>3}  speed {:>2} bpm {:>3}  voices {:>3}  peak {:>5.3}",
        format_seconds(transport.song_frame, sample_rate_hz),
        format_seconds(transport.song_length_frames, sample_rate_hz),
        transport.order,
        transport.pattern,
        transport.row,
        transport.speed,
        transport.tempo_bpm,
        snapshot.voices_active,
        peak,
    );
}

fn print_devices(devices: &[DeviceInfo]) {
    if devices.is_empty() {
        println!("no output devices on any backend cpal can reach");
        return;
    }
    for device in devices {
        let marker = if device.is_default { "*" } else { " " };
        let rates: Vec<String> = device.supported_rates.iter().map(|rate| rate.to_string()).collect();
        let rates = if rates.is_empty() { String::from("(unadvertised)") } else { rates.join(", ") };
        println!("{marker} [{}] {}\n    rates: {rates}", device.backend, device.name);
    }
}

fn describe(error: HostError) -> String { std::format!("{error}") }
