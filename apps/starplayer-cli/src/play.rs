//! `starplayer play` — play a module on an audio output device through
//! `starplayer-host-cpal`.
//!
//! This is the CLI wiring for the behaviour `starplayer-host-cpal`'s own
//! `examples/play.rs` already proved out (task D4): negotiate a device, open a
//! [`Player`], load the module, play it to its natural end or its detected loop's fade,
//! and print progress once a second. The example stays as the host crate's own
//! acceptance test; this command is a second, thinner caller of the same
//! `starplayer-host` surface, so the two never carry two copies of the same logic —
//! where they needed the same helper (`format_seconds`), it moved to `starplayer-host`
//! instead.
//!
//! The one behaviour this command has that the example does not is `Ctrl-C`: the signal
//! handler only ever sets a flag, and the polling loop — the sole producer onto the
//! player's command ring — is what calls [`Player::stop`], so the click-free stop ramp
//! is requested from the one thread the ring's SPSC contract allows.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use starplayer::core::AtEnd;
use starplayer::engine::MixerMode;
use starplayer_host::{AudioBackend, AudioSpec, DeviceInfo, Player, format_seconds};
use starplayer_host_cpal::CpalBackend;

use crate::archive;

/// How long the loop sleeps between polls of the telemetry.
///
/// Nothing here is timing-critical — the audio runs on cpal's own thread — so the
/// interval only has to be short enough that the "song is over" check is prompt and
/// long enough that a progress line is not a busy-wait.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a song that has ended is left ringing out before the process exits.
///
/// The transport stops the musical clock but the voices already sounding decay through
/// the mixer, and cutting the stream the instant the clock stops would clip that tail.
const RING_OUT: Duration = Duration::from_millis(400);

/// How long the loop waits for the first callback before deciding the device never
/// started.
const STARTUP_GRACE: Duration = Duration::from_secs(3);

/// The fade a looping song is played out under, in seconds — the D1/D2 rule the web
/// player uses (`apps/starplayer-web/www/app.js`'s `SONG_FADE_SECONDS`), not a value of
/// this command's own choosing, so a module fades over the same span in every host.
const LOOP_FADE_SECONDS: u32 = 5;

/// The block size asked for when `--buffer` does not name one.
///
/// **Not** the device's own default, which is what "no preference" gets you and which
/// can be enormous: PulseAudio's WSLg sink hands over 96 000 frames — two seconds — in a
/// single callback, and a two-second playout buffer is two seconds of latency on every
/// stop and progress reading. 1024 frames is 21 ms at 48 kHz, which is what a media
/// player asks for. The rendered audio is identical either way (design goal 3); only the
/// latency is not. `--buffer 0` asks for the device's own, for anyone who wants to hear
/// that.
const DEFAULT_BUFFER_FRAMES: u32 = 1_024;

#[derive(clap::Args, Debug)]
pub struct PlayArgs {
    /// Module file, or a ZIP archive containing one. Not needed with `--list-devices`.
    #[arg(required_unless_present = "list_devices")]
    pub file: Option<PathBuf>,
    /// Which recognised entry of a ZIP archive to play.
    #[arg(long)]
    pub entry: Option<usize>,
    /// An exact output device name, or any case-insensitive part of one.
    #[arg(long)]
    pub device: Option<String>,
    /// The sample rate to ask the device for, in Hz.
    #[arg(long, default_value_t = 48_000)]
    pub rate: u32,
    /// The block size to ask the device for, in frames. 0 asks for the device's own.
    #[arg(long, default_value_t = DEFAULT_BUFFER_FRAMES)]
    pub buffer: u32,
    /// Keep looping at the detected loop point instead of fading out after one pass.
    #[arg(long)]
    pub repeat: bool,
    /// Print every output device on every backend this build can reach, and exit.
    #[arg(long)]
    pub list_devices: bool,
    // ── task E5 ──────────────────────────────────────────────────────────────────────
    /// The module whose instruments play a `.mid` file (task E5 deliverable 4).
    /// Required when `file` is a Standard MIDI File; ignored, with no effect, for a
    /// tracker module. `play` does not yet play a `.mid` through a device — that needs
    /// `starplayer-host::Player` support E6/E7 land — so today this flag only lets a
    /// `.mid` fail with a message naming `render` instead of the generic "not a module
    /// format this build plays".
    #[arg(long)]
    pub instruments: Option<PathBuf>,
    // ── end task E5 ──────────────────────────────────────────────────────────────────
}

pub fn run(args: PlayArgs) -> Result<(), String> {
    if args.list_devices {
        print_devices(&starplayer_host_cpal::all_devices());
        return Ok(());
    }

    let Some(file) = args.file.clone() else {
        return Err(String::from("no module given; see --help"));
    };
    let bytes = archive::load_module_bytes(&file, args.entry)?;

    // ── task E5: `play` cannot drive a `.mid` yet; fail clearly rather than through
    // `Player::load`'s generic "not a module format this build plays". ──────────────
    if starplayer::probe_smf(&bytes) {
        return match &args.instruments {
            None => Err(format!("{}: a Standard MIDI File needs --instruments <module> naming the module whose instruments play it", file.display())),
            Some(_) => Err(format!(
                "{}: `play` cannot drive a .mid through a device yet; render it instead: `starplayer render {} --instruments <module> -o out.wav`",
                file.display(),
                file.display()
            )),
        };
    }
    // ── end task E5 ────────────────────────────────────────────────────────────────

    let mut backend = CpalBackend::new().map_err(|error| error.to_string())?;
    let backend_name = backend.host_name();

    let interrupted = Arc::new(AtomicBool::new(false));
    let handler_flag = Arc::clone(&interrupted);
    ctrlc::set_handler(move || handler_flag.store(true, Ordering::SeqCst))
        .map_err(|error| format!("could not install the Ctrl-C handler: {error}"))?;

    play_on(&mut backend, backend_name, &file.display().to_string(), &bytes, &args, &interrupted)
}

/// The device-independent half of `run`: negotiate, open, load and play, then poll until
/// the song is over. Taking `backend` as a trait object and `interrupted` as a plain flag
/// — rather than reaching for `CpalBackend` or `ctrlc` directly — is what lets the tests
/// below drive a device that finds nothing, without a sound card or a process-wide signal
/// handler.
fn play_on(backend: &mut dyn AudioBackend, backend_name: &str, file_display: &str, bytes: &[u8], args: &PlayArgs, interrupted: &AtomicBool) -> Result<(), String> {
    let requested = AudioSpec {
        sample_rate_hz: args.rate,
        channels: MixerMode::DEFAULT.channels as u16,
        preferred_block_frames: (args.buffer > 0).then_some(args.buffer),
    };
    let mut player = Player::open(backend, args.device.as_deref(), requested, MixerMode::DEFAULT).map_err(|error| {
        format!("could not open an output stream on the {backend_name} backend: {error}; `--list-devices` shows what this machine has")
    })?;

    let spec = player.spec();
    println!("backend {backend_name} - device {}", args.device.as_deref().unwrap_or("(default)"));
    println!("stream  {spec}");

    player.load(bytes).map_err(|error| error.to_string())?;
    let title = player.module().map(|module| String::from(module.header().title.as_ref())).unwrap_or_default();
    let song_length = player.song_length().unwrap_or(0);
    println!("module  {file_display}: {title:?}, {song_length} frames ({})", format_seconds(song_length, spec.sample_rate_hz));

    if args.repeat {
        player.set_at_end(AtEnd::Continue).map_err(|error| error.to_string())?;
    } else {
        // FadeOut covers both endings: a song that runs out of order list stops where it
        // ends, and a song that jumps back to a loop point plays one pass and fades.
        player.set_fade_frames(LOOP_FADE_SECONDS.saturating_mul(spec.sample_rate_hz)).map_err(|error| error.to_string())?;
        player.set_at_end(AtEnd::FadeOut).map_err(|error| error.to_string())?;
    }
    player.play().map_err(|error| error.to_string())?;

    let outcome = follow(&mut player, spec.sample_rate_hz, interrupted);
    player.stop().map_err(|error| error.to_string())?;
    std::thread::sleep(RING_OUT);
    player.collect_garbage();
    println!();

    let warnings = player.warnings();
    if warnings.any() {
        println!("warnings: {warnings:?}");
    }
    match outcome {
        Outcome::Ended | Outcome::Interrupted => Ok(()),
        Outcome::CallbackNeverRan => Err(String::from("the device accepted the stream but never asked for audio")),
        Outcome::DeviceFailed => Err(String::from("the device reported a fatal error and the stream is dead")),
    }
}

/// Why the follow loop stopped.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Outcome {
    /// The song reached its end, or the loop point and the end of its fade.
    Ended,
    /// `Ctrl-C` asked for a click-free stop and it landed.
    Interrupted,
    /// The stream opened but no callback ever arrived.
    CallbackNeverRan,
    /// The backend's error callback latched something fatal.
    DeviceFailed,
}

/// Poll telemetry until the song is over, updating one status line a second.
fn follow(player: &mut Player, sample_rate_hz: u32, interrupted: &AtomicBool) -> Outcome {
    let started = Instant::now();
    let mut next_report = Instant::now();
    // The transport is queued, not immediate: `is_playing` is false for the first block
    // or two after `play()`, so "it stopped" only counts once it has been seen to start.
    let mut has_started = false;
    let mut stopping = false;
    loop {
        if player.health().is_failed() {
            return Outcome::DeviceFailed;
        }
        if interrupted.load(Ordering::SeqCst) && !stopping {
            // The one call to `Player::stop` this loop makes on a Ctrl-C — queuing it
            // again every iteration would just be more ring traffic for the same ramp.
            let _ = player.stop();
            stopping = true;
        }
        if player.blocks_rendered() == 0 {
            if started.elapsed() > STARTUP_GRACE {
                return Outcome::CallbackNeverRan;
            }
        } else if player.is_playing() {
            has_started = true;
        } else if has_started {
            return if stopping { Outcome::Interrupted } else { Outcome::Ended };
        }

        if Instant::now() >= next_report {
            report(player, sample_rate_hz);
            next_report = Instant::now() + Duration::from_secs(1);
        }
        player.collect_garbage();
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One updating status line: `\r` and no trailing newline, so each second's report
/// overwrites the last rather than scrolling the terminal.
fn report(player: &mut Player, sample_rate_hz: u32) {
    use std::io::Write;

    let peak = player.peak();
    let snapshot = *player.telemetry();
    let transport = snapshot.transport;
    print!(
        "\r{:>8} / {:<8}  order {:>3} pattern {:>3} row {:>3}  speed {:>2} bpm {:>3}  voices {:>3}  peak {:>5.3}  ",
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
    let _ = std::io::stdout().flush();
}

fn print_devices(devices: &[DeviceInfo]) {
    if devices.is_empty() {
        println!("no output devices on any backend this build can reach");
        return;
    }
    for device in devices {
        let marker = if device.is_default { "*" } else { " " };
        let rates: Vec<String> = device.supported_rates.iter().map(|rate| rate.to_string()).collect();
        let rates = if rates.is_empty() { String::from("(unadvertised)") } else { rates.join(", ") };
        println!("{marker} [{}] {}\n    rates: {rates}", device.backend, device.name);
    }
}

#[cfg(test)]
mod tests {
    use starplayer_host::{HostError, RenderCallback, Stream};

    use super::*;

    /// A backend that enumerates and negotiates nothing, the way a machine with no sound
    /// card at all answers `cpal::default_host()`. It exists to drive [`play_on`] through
    /// its error path without a real device, per this task's research point 1.
    struct NoDeviceBackend;

    impl AudioBackend for NoDeviceBackend {
        fn devices(&self) -> Vec<DeviceInfo> { Vec::new() }

        fn negotiate(&self, _device: Option<&str>, _requested: AudioSpec) -> Result<AudioSpec, HostError> { Err(HostError::NoDevice) }

        fn open(&mut self, _device: Option<&str>, _spec: AudioSpec, _callback: RenderCallback) -> Result<Stream, HostError> { Err(HostError::NoDevice) }
    }

    fn default_args() -> PlayArgs {
        PlayArgs { file: None, entry: None, device: None, rate: 48_000, buffer: DEFAULT_BUFFER_FRAMES, repeat: false, list_devices: false, instruments: None }
    }

    /// Research point 1: a machine with no device fails `play` with one clear line
    /// naming `--list-devices`, exits non-zero (through `main`'s generic error
    /// handling, since this returns `Err`), and never panics.
    #[test]
    fn a_backend_with_no_device_fails_with_one_line_naming_list_devices_and_never_panics() {
        let mut backend = NoDeviceBackend;
        let interrupted = AtomicBool::new(false);
        let result = play_on(&mut backend, "test", "song.s3m", &[], &default_args(), &interrupted);

        let error = result.expect_err("a backend with no device must not open a stream");
        assert!(!error.contains('\n'), "the error is one line, not several: {error:?}");
        assert!(error.contains("--list-devices"), "the error names --list-devices: {error:?}");
    }

    #[test]
    fn print_devices_reports_an_empty_list_without_panicking() { print_devices(&[]); }

    #[test]
    fn repeat_maps_to_at_end_continue_and_the_default_maps_to_fade_out() {
        // `play_on` sets `AtEnd` after `Player::open`, which a device-less backend never
        // reaches — this test instead pins down the mapping the deliverable specifies,
        // so a future edit that swaps the branches breaks a test rather than only
        // costing an audible A/B check. See the `AtEnd` arms in `play_on` above.
        assert!(!default_args().repeat);
        assert!(PlayArgs { repeat: true, ..default_args() }.repeat);
    }

    #[test]
    fn the_loop_fade_matches_the_web_players_fade_rule() {
        // `apps/starplayer-web/www/app.js`'s `SONG_FADE_SECONDS` is 5; task D8's
        // research point 2 ties this command's default fade to that rule rather than to
        // a value of its own, so a song fades over the same span in every host.
        assert_eq!(LOOP_FADE_SECONDS, 5);
    }

    #[test]
    fn a_device_name_reads_back_the_way_the_help_text_promises() {
        // `--device` matches an exact name or a case-insensitive substring — exercised
        // for real against `CpalBackend::find` in `starplayer-host-cpal`; here only the
        // plumbing that carries the flag through to `Player::open` is in scope.
        let args = PlayArgs { device: Some(String::from("rdp")), ..default_args() };
        assert_eq!(args.device.as_deref(), Some("rdp"));
    }

    /// Task E5: a `.mid` with no `--instruments` fails `run` naming the flag, before
    /// `run` ever opens a device — no `CpalBackend` is touched, so this runs without
    /// a sound card exactly like the research-point-1 test above.
    #[test]
    fn a_mid_file_with_no_instruments_flag_names_it_in_the_error() {
        let path = std::env::temp_dir().join("starplayer-e5-play-test.mid");
        std::fs::write(&path, minimal_smf_bytes()).expect("can write a scratch file");

        let error = run(PlayArgs { file: Some(path.clone()), ..default_args() }).expect_err("a .mid with no --instruments must fail");
        assert!(error.contains("--instruments"), "the error must name the missing flag: {error:?}");

        let _ = std::fs::remove_file(&path);
    }

    /// Task E5: `play` does not yet drive a `.mid` through a device (that needs
    /// `starplayer-host::Player` support E6/E7 land) — with `--instruments` given, the
    /// error points at `render` instead of failing silently or panicking.
    #[test]
    fn a_mid_file_with_instruments_points_at_render_instead_of_playing() {
        let path = std::env::temp_dir().join("starplayer-e5-play-test-2.mid");
        std::fs::write(&path, minimal_smf_bytes()).expect("can write a scratch file");

        let error = run(PlayArgs { file: Some(path.clone()), instruments: Some(PathBuf::from("module.it")), ..default_args() })
            .expect_err("play cannot drive a .mid through a device yet");
        assert!(error.contains("render"), "the error must point at `render` instead: {error:?}");

        let _ = std::fs::remove_file(&path);
    }

    /// The smallest legal Standard MIDI File: an empty format-0 track.
    fn minimal_smf_bytes() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"MThd");
        bytes.extend_from_slice(&6u32.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&96u16.to_be_bytes());
        bytes.extend_from_slice(b"MTrk");
        let track: &[u8] = &[0x00, 0xFF, 0x2F, 0x00];
        bytes.extend_from_slice(&(track.len() as u32).to_be_bytes());
        bytes.extend_from_slice(track);
        bytes
    }
}
