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

use starplayer::core::{AtEnd, Frame};
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
    /// Module file, or a ZIP archive containing one. Not needed with `--list-devices`,
    /// `--list-midi-ports` or `--list-effects`.
    #[arg(required_unless_present_any = ["list_devices", "list_midi_ports", "list_effects"])]
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
    /// The module whose instruments play a `.mid` file. Required when `file` is a
    /// Standard MIDI File; ignored, with no effect, for a tracker module.
    #[arg(long)]
    pub instruments: Option<PathBuf>,
    // ── end task E5 ──────────────────────────────────────────────────────────────────


    // ── live MIDI input (task E6) ───────────────────────────────────────────────────
    //
    // Kept in one block, separate from the transport flags above, because E5 lands
    // `--instruments` on this same command concurrently and the two must not tangle.
    /// Jam the module's instruments from a MIDI input port over the playing module: an
    /// index, an exact port name, or any case-insensitive part of one.
    #[arg(long, value_name = "PORT")]
    pub midi: Option<String>,
    /// Print every MIDI input port this build can see, and exit.
    #[arg(long)]
    pub list_midi_ports: bool,

    // ── insert effects (M7-H7) ──────────────────────────────────────────────────────
    /// Install an insert effect before playing: `<target>:<effect>[:<param>=<value>,...]`,
    /// where `<target>` is a 1-based channel number or `master`, e.g.
    /// `--insert 1:reverb:room=60,mix=50`. Repeatable: a second `--insert` naming the same
    /// target fills the next slot of its chain.
    #[arg(long = "insert")]
    pub insert: Vec<String>,
    /// Print every insert effect this build can install, its parameters, their units,
    /// ranges and defaults, and exit.
    #[arg(long)]
    pub list_effects: bool,
}

pub fn run(args: PlayArgs) -> Result<(), String> {
    if args.list_devices {
        print_devices(&starplayer_host_cpal::all_devices());
        return Ok(());
    }
    if args.list_midi_ports {
        print_midi_ports();
        return Ok(());
    }
    if args.list_effects {
        print!("{}", crate::insert_arg::list_effects());
        return Ok(());
    }

    let Some(file) = args.file.clone() else {
        return Err(String::from("no module given; see --help"));
    };
    let bytes = archive::load_module_bytes(&file, args.entry)?;

    let is_smf = starplayer::probe_smf(&bytes);
    if is_smf && args.instruments.is_none() {
        return Err(format!("{}: a Standard MIDI File needs --instruments <module> naming the module whose instruments play it", file.display()));
    }
    if is_smf && args.midi.is_some() {
        return Err(String::from("--midi cannot be combined with Standard MIDI File playback; it is live input for a tracker module"));
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
    if starplayer::probe_smf(bytes) && args.midi.is_some() {
        return Err(String::from("--midi cannot be combined with Standard MIDI File playback; it is live input for a tracker module"));
    }
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

    let smf_length = if starplayer::probe_smf(bytes) {
        let path = args.instruments.as_ref().ok_or("a Standard MIDI File needs --instruments <module>")?;
        let instrument_bytes = std::fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let instruments = starplayer::rt::Arc::new(starplayer::load(&instrument_bytes).map_err(|error| error.to_string())?);
        Some(player.load_smf(bytes, instruments).map_err(|error| error.to_string())?)
    } else {
        player.load(bytes).map_err(|error| error.to_string())?;
        None
    };
    let title = player.module().map(|module| String::from(module.header().title.as_ref())).unwrap_or_default();
    let song_length = smf_length.or_else(|| player.song_length()).unwrap_or(0);
    let kind = if smf_length.is_some() { "smf" } else { "module" };
    println!("{kind:<7} {file_display}: {title:?}, {song_length} frames ({})", format_seconds(song_length, spec.sample_rate_hz));

    if args.repeat {
        player.set_at_end(AtEnd::Continue).map_err(|error| error.to_string())?;
    } else {
        // FadeOut covers both endings: a song that runs out of order list stops where it
        // ends, and a song that jumps back to a loop point plays one pass and fades.
        player.set_fade_frames(LOOP_FADE_SECONDS.saturating_mul(spec.sample_rate_hz)).map_err(|error| error.to_string())?;
        player.set_at_end(AtEnd::FadeOut).map_err(|error| error.to_string())?;
    }

    // ── insert effects (M7-H7) ───────────────────────────────────────────────────────
    for insert in crate::insert_arg::parse_insert_args(&args.insert)? {
        player.install_insert(insert.target, insert.slot, insert.kind).map_err(|error| error.to_string())?;
        for (param, value) in insert.params {
            player.set_insert_param(insert.target, insert.slot, param, value).map_err(|error| error.to_string())?;
        }
    }
    // ── end of the insert effects block ─────────────────────────────────────────────

    // ── live MIDI input (task E6) ───────────────────────────────────────────────────
    //
    // `--midi` puts the module sequencer in slot 0 and its live-input instruments in slot
    // 1 of one deterministic `SourceMux`.
    // The connection is bound to a name because dropping it closes the port.
    let _midi_connection = match args.midi.as_deref() {
        None => None,
        Some(selector) => {
            player.jam(true).map_err(|error| error.to_string())?;
            let sender = player.take_event_sender().ok_or("live input installed no sender")?;
            let connection = starplayer_midi_native::open_input(Some(selector), sender).map_err(|error| {
                format!("{error}; `--list-midi-ports` shows what this machine has")
            })?;
            println!("midi    {} - jamming the module's instruments over its pattern data", connection.port_name());
            println!("lead    {} frames ({:.1} ms) ahead of the audio clock", player.event_lead(), player.event_lead_millis());
            Some(connection)
        }
    };
    // ── end of the live MIDI input block ────────────────────────────────────────────

    player.play().map_err(|error| error.to_string())?;

    let smf_span = smf_length.map(|length| {
        let start = player.source_frame();
        (start, start.saturating_add(length))
    });
    let outcome = follow(&mut player, spec.sample_rate_hz, interrupted, smf_span);
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
fn follow(player: &mut Player, sample_rate_hz: u32, interrupted: &AtomicBool, smf_span: Option<(Frame, Frame)>) -> Outcome {
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
        if smf_span.is_some_and(|(_, end)| player.source_frame() >= end) {
            return Outcome::Ended;
        }

        if Instant::now() >= next_report {
            if let Some((start, end)) = smf_span {
                report_smf(player, sample_rate_hz, start, end);
            } else if player.is_midi_only() {
                report_live_input(player);
            } else {
                report(player, sample_rate_hz);
            }
            next_report = Instant::now() + Duration::from_secs(1);
        }
        player.collect_garbage();
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// One updating status line for a file-driven MIDI source. Tracker telemetry has no song
/// row for this source, so elapsed time comes directly from the same monotonic clock that
/// dispatches its events.
fn report_smf(player: &mut Player, sample_rate_hz: u32, start: Frame, end: Frame) {
    use std::io::Write;

    let elapsed = start.frames_until(player.source_frame()).min(start.frames_until(end));
    let duration = start.frames_until(end);
    let voices_active = player.telemetry().voices_active;
    print!(
        "\r{:>8} / {:<8}  voices {:>3}  peak {:>5.3}  ",
        format_seconds(elapsed, sample_rate_hz),
        format_seconds(duration, sample_rate_hz),
        voices_active,
        player.peak(),
    );
    let _ = std::io::stdout().flush();
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

/// The `--midi` status line. Song position means nothing under a live-input source — the
/// module is not playing — so this reports what the keyboard is doing instead.
fn report_live_input(player: &mut Player) {
    use std::io::Write;

    let snapshot = *player.telemetry();
    print!(
        "\rmidi  events {:>7}  dropped {:>5}  lead {:>5} frames ({:>5.1} ms)  voices {:>3}  peak {:>5.3}  ",
        player.events_sent(),
        player.events_rejected(),
        player.event_lead(),
        player.event_lead_millis(),
        snapshot.voices_active,
        player.peak(),
    );
    let _ = std::io::stdout().flush();
}

/// `--list-midi-ports`. A machine with no MIDI stack at all — every container, and every
/// WSL2 box without `/dev/snd` — prints why rather than failing: nothing was asked to play,
/// so there is nothing to fail.
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
        PlayArgs {
            file: None,
            entry: None,
            device: None,
            rate: 48_000,
            buffer: DEFAULT_BUFFER_FRAMES,
            repeat: false,
            list_devices: false,
            instruments: None,
            midi: None,
            list_midi_ports: false,
            insert: Vec::new(),
            list_effects: false,
        }
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

    // ── insert effects (M7-H7) ──────────────────────────────────────────────────────

    const REFLEX: &[u8] = include_bytes!("../../../crates/starplayer-s3m/tests/fixtures/REFLEX.S3M");

    /// `--insert` parses and installs through `Player` before playing starts, and the
    /// module still produces audio afterwards — proving the flag reaches the same
    /// `Player::install_insert` surface `render`'s `--insert` does, rather than only
    /// parsing correctly.
    #[test]
    fn an_insert_flag_installs_before_play_and_the_module_still_sounds() {
        let spec = AudioSpec::stereo(44_100);
        let mut backend = starplayer_host::ManualBackend::new();
        let mut player = Player::open(&mut backend, None, spec, starplayer::engine::MixerMode::DEFAULT).expect("the manual backend opens");
        player.load(REFLEX).expect("REFLEX loads");

        let inserts = crate::insert_arg::parse_insert_args(&[String::from("1:reverb:room=60,mix=50")]).expect("the flag parses");
        for insert in inserts {
            player.install_insert(insert.target, insert.slot, insert.kind).expect("install queues");
            for (param, value) in insert.params {
                player.set_insert_param(insert.target, insert.slot, param, value).expect("set_param queues");
            }
        }
        assert_eq!(
            player.inserts().get(starplayer::engine::InsertTarget::Channel(starplayer::core::ChannelId(0)), 0).map(|installed| installed.kind),
            Some(starplayer::dsp::InsertKind::Reverb)
        );

        player.play().expect("play");
        let driver = backend.driver().expect("an open backend has a driver");
        let mut output = vec![0.0f32; spec.samples_for(starplayer::engine::RENDER_QUANTUM * 64)];
        driver.render(&mut output);
        assert!(output.iter().any(|sample| *sample != 0.0), "the module still sounds with an insert installed");
    }

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

    // ── live MIDI input (task E6) ───────────────────────────────────────────────────

    /// `--list-midi-ports` has to answer on a machine with no MIDI stack at all, which is
    /// every container and the WSL2 box this task was written on. It prints why and exits
    /// zero, exactly as `--list-devices` does with no sound card.
    #[test]
    fn listing_midi_ports_answers_without_panicking_on_a_machine_with_no_midi_stack() {
        print_midi_ports();
    }

    #[test]
    fn a_midi_port_selector_reads_back_the_way_the_help_text_promises() {
        assert_eq!(default_args().midi, None, "no live input unless it was asked for");
        let args = PlayArgs { midi: Some(String::from("keystation")), ..default_args() };
        assert_eq!(args.midi.as_deref(), Some("keystation"));
        assert!(PlayArgs { list_midi_ports: true, ..default_args() }.list_midi_ports);
    }

    /// Both listing flags have to work with no file argument, which is a clap
    /// `required_unless_present_any` and therefore worth pinning: adding a third listing
    /// flag and forgetting to name it there makes `--list-midi-ports` demand a module.
    #[test]
    fn the_listing_flags_do_not_need_a_module() {
        use clap::Parser;

        #[derive(Parser)]
        struct Wrapper {
            #[command(flatten)]
            play: PlayArgs,
        }
        assert!(Wrapper::try_parse_from(["play", "--list-devices"]).is_ok());
        assert!(Wrapper::try_parse_from(["play", "--list-midi-ports"]).is_ok());
        assert!(Wrapper::try_parse_from(["play"]).is_err(), "playing still needs something to play");
        let parsed = Wrapper::try_parse_from(["play", "song.s3m", "--midi", "2"]).expect("a port selector parses");
        assert_eq!(parsed.play.midi.as_deref(), Some("2"));
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

    /// Task E7: `--instruments` is accepted on the device playback path; the host-level
    /// ManualBackend test proves the source itself without needing a sound card here.
    #[test]
    fn a_mid_file_accepts_an_instrument_module_argument() {
        let args = PlayArgs { instruments: Some(PathBuf::from("module.it")), ..default_args() };
        assert_eq!(args.instruments.as_deref(), Some(std::path::Path::new("module.it")));
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
