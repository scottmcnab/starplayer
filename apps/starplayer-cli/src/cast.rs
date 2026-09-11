//! `starplayer cast` — play a module on a Google Home or Nest speaker.
//!
//! The speaker runs Google's own **Default Media Receiver**, which plays a media URL and
//! nothing else, so this command does the rendering here and serves the result over HTTP
//! on the LAN. Two paths:
//!
//! - **Pre-render (the default).** Render the whole song to 16-bit stereo, encode it as
//!   FLAC (or WAV), serve it with `Range` support, and LOAD it as a `BUFFERED` stream.
//!   The receiver gets a duration, a seek bar and a clean end, which is what makes the
//!   Google Home app and the speaker's own controls behave.
//! - **Live (`--live`).** Put a [`Player`] on a `CastStreamBackend` and chunk the encoder's
//!   output into the HTTP response as it is produced, LOADed as a `LIVE` stream with no
//!   duration. This exists for an endless session, not for playing a song.
//!
//! # Transport keys arrive on stdin, one letter per line
//!
//! `p` play/pause, `s` stop, `q` quit, `+`/`-` volume, `<`/`>` seek, and a bare newline
//! prints the status line at once. **This is a deliberate deviation from the A4 master
//! plan's "transport keys while it runs"**: single-keypress input needs raw terminal mode,
//! which needs a terminal crate this workspace does not have. Choosing one here would
//! pre-empt the TUI's own choice (A1), where raw mode actually belongs, so this command
//! reads lines from stdin instead and says so in `--help`.
//!
//! # Threads
//!
//! The loop below is the **control thread**, and it is the only thread that calls into
//! [`Player`] — the command ring is single-producer. The stdin reader is a thread of its
//! own feeding a channel; the media server, the CASTv2 heartbeat and (under `--live`) the
//! audio driver are threads owned by `starplayer-cast`.

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::time::{Duration, Instant};

use starplayer::core::AtEnd;
use starplayer::engine::MixerMode;
use starplayer_cast::{CastDevice, CastEncoder, CastError, CastSession, CastStreamBackend, FlacEncoder, MediaRequest, MediaServer, StreamKind, WavEncoder};
use starplayer_host::{AudioSpec, Player, format_seconds};
use starplayer_offline::RenderLength;

use crate::archive;
use crate::render::{InterpArg, MixPathArg, PcmTarget};

/// How long `--list` and the pre-cast browse listen for mDNS answers, in seconds.
const DEFAULT_DISCOVERY_SECONDS: f64 = 4.0;

/// How long the control loop sleeps between polls.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How often the status line is refreshed and the session's inbox is pumped.
const REPORT_INTERVAL: Duration = Duration::from_secs(1);

/// How much `+`/`-` move the speaker's volume.
const VOLUME_STEP: f32 = 0.05;

/// How far `<`/`>` seek, in seconds.
const SEEK_STEP: f32 = 10.0;

/// How many encoded chunks may wait for the HTTP thread before the driver has to block.
///
/// Bounded on purpose: a speaker that stops draining should make the driver wait, not make
/// this process grow. Sixty-four FLAC frames is about six seconds of audio at 44.1 kHz.
const LIVE_CHUNK_CAPACITY: usize = 64;

/// The block size the `--live` driver renders at: one whole FLAC frame per pass.
const LIVE_BLOCK_FRAMES: u32 = 4_096;

/// The fade a looping song is played out under, in seconds — the same figure `play` uses.
const LOOP_FADE_SECONDS: f64 = 5.0;

/// The prose `starplayer cast --help` opens with.
///
/// Everything here is something an operator needs *before* reading the flag list: what the
/// command actually does with the audio, why `--list` finding nothing is normal, how the
/// transport keys work, and the latency figure that rules out playing live over Cast.
pub const CAST_LONG_ABOUT: &str = "\
Play a module on a Google Home, Nest speaker or speaker group.

The speaker runs Google's own Default Media Receiver, which plays a media URL and nothing
else. So this command renders the module here, serves the audio over HTTP from whichever
local interface reaches the speaker, and tells the receiver to play that URL. No Google
developer account and no registration are involved.

By default the whole song is rendered first and served with HTTP Range support, which is
what gives the speaker a duration, a seek bar and a clean end. --live streams it as it
renders instead: no duration, no seeking, and the encoder's own lead added to the
speaker's buffer. That is for an endless session, not for playing a song.

Transport commands are single letters on stdin, each followed by Enter:
  p  play/pause      s  stop          q  quit
  +  volume up       -  volume down   <  back 10 s      >  forward 10 s
  (a bare Enter prints the status line straight away)
Single keypresses would need raw terminal mode and a terminal crate this workspace does
not have yet; that choice belongs to the TUI, so this command reads lines instead.

Latency: expect roughly 2-5 s between a command and the speaker acting on it. The CASTv2
round trip itself is about a millisecond on a LAN; the rest is the receiver's own playout
buffer, which a sender cannot see or shorten. That is why there is no jam mode over Cast
and why --live is not a way to get one: playing along with what you hear is impossible
when what you hear left this machine seconds ago.

--list finding nothing is a normal result, not necessarily a fault. mDNS is multicast, and
multicast does not cross a NAT: inside WSL2's default networking, inside most containers
and across a VLAN boundary nothing will ever answer, and a speaker could not reach a media
server bound in there either.";

/// Which encoder puts the audio on the wire.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum CastFormatArg {
    /// FLAC: lossless, about half WAV's bitrate, and inside the 2 Mbit/s the audio-device
    /// guidance allows.
    Flac,
    /// WAV/LPCM: no codec at all, 1.4 Mbit/s at 44.1 kHz stereo.
    Wav,
}

#[derive(clap::Args, Debug)]
pub struct CastArgs {
    /// Module file, or a ZIP archive containing one. Not needed with `--list`,
    /// `--list-effects` or `--list-enhancers`.
    #[arg(required_unless_present_any = ["list", "list_effects", "list_enhancers"])]
    pub file: Option<PathBuf>,
    /// Which recognised entry of a ZIP archive to play.
    #[arg(long)]
    pub entry: Option<usize>,
    /// The speaker or speaker group to play on: any case-insensitive prefix of its
    /// friendly name, so `--device kit` finds "Kitchen speaker".
    #[arg(long, required_unless_present_any = ["list", "list_effects", "list_enhancers"])]
    pub device: Option<String>,
    /// List every Cast device and speaker group that answers on this network, and exit.
    #[arg(long)]
    pub list: bool,
    /// How long to listen for mDNS answers, in seconds.
    #[arg(long, default_value_t = DEFAULT_DISCOVERY_SECONDS)]
    pub timeout: f64,
    /// What to put on the wire.
    #[arg(long, value_enum, default_value = "flac")]
    pub format: CastFormatArg,
    /// The sample rate to render and stream at, in Hz.
    #[arg(long, default_value_t = 44_100)]
    pub rate: u32,
    /// Voice-accumulation path.
    #[arg(long = "path", value_enum, default_value = "float")]
    pub mix_path: MixPathArg,
    /// Resampling kernel.
    #[arg(long, value_enum, default_value = "linear")]
    pub interp: InterpArg,
    /// Extra passes through the repeating section after the first. Zero plays it once and
    /// fades. Ignored by `--live`, which never ends on its own.
    #[arg(long, default_value_t = 0)]
    pub repeat: u32,
    /// Fade length in seconds at a detected loop point. Defaults to five seconds.
    #[arg(long)]
    pub fade: Option<f64>,
    /// Stream the module as it renders instead of pre-rendering it: an endless session
    /// rather than a song. Costs seek, duration and a clean end, and adds the encoder's
    /// own lead to the speaker's buffer.
    #[arg(long)]
    pub live: bool,
    /// Set the **speaker's** volume, 0.0 to 1.0, before playing.
    #[arg(long)]
    pub volume: Option<f32>,
    /// Install an insert effect before playing: `<target>:<effect>[:<param>=<value>,...]`,
    /// where `<target>` is a 1-based channel number or `master`, e.g.
    /// `--insert 1:reverb:room=60,mix=50`. Repeatable.
    #[arg(long = "insert")]
    pub insert: Vec<String>,
    /// Print every insert effect this build can install, and exit.
    #[arg(long)]
    pub list_effects: bool,
    /// Rebuild every sample through a load-time enhancer before playing: entries joined
    /// with `+`, each an id from the catalogue — `sinc2x`, `sinc4x`, `loop` or
    /// `loop=<frames>` — e.g. `--enhance sinc4x+loop`.
    #[arg(long)]
    pub enhance: Option<String>,
    /// Print every load-time sample enhancer `--enhance` can name, and exit.
    #[arg(long)]
    pub list_enhancers: bool,
}

pub fn run(args: CastArgs) -> Result<(), String> {
    if args.list_effects {
        print!("{}", crate::insert_arg::list_effects());
        return Ok(());
    }
    if args.list_enhancers {
        print!("{}", crate::enhance_arg::list_enhancers());
        return Ok(());
    }

    let timeout = Duration::from_secs_f64(args.timeout.max(0.1));
    if args.list {
        return list_devices(timeout);
    }

    let file = args.file.clone().ok_or_else(|| String::from("no module given; see --help"))?;
    let device_name = args.device.clone().ok_or_else(|| String::from("no --device given; `starplayer cast --list` shows what answered"))?;

    println!("browsing for cast devices for {:.1} s ...", timeout.as_secs_f64());
    let devices = starplayer_cast::discover(timeout).map_err(|error| error.to_string())?;
    if devices.is_empty() {
        return Err(no_devices_message());
    }
    let device = starplayer_cast::find(&devices, &device_name).map_err(|error| error.to_string())?.clone();
    println!("device  {}", device.describe());

    // Installed once, from the thread that also owns the player's command ring, exactly as
    // `play` does: the handler only sets a flag.
    let interrupted = Arc::new(AtomicBool::new(false));
    let handler_flag = Arc::clone(&interrupted);
    ctrlc::set_handler(move || handler_flag.store(true, Ordering::SeqCst)).map_err(|error| format!("could not install the Ctrl-C handler: {error}"))?;

    if args.live { cast_live(&args, &file, &device, &interrupted) } else { cast_prerendered(&args, &file, &device, &interrupted) }
}

/// `--list`: every device and group that answered, one per line.
fn list_devices(timeout: Duration) -> Result<(), String> {
    let devices = starplayer_cast::discover(timeout).map_err(|error| error.to_string())?;
    if devices.is_empty() {
        // Not an error: on a NATed network this is the correct answer, and exiting
        // non-zero would make a normal outcome look like a failure.
        println!("{}", no_devices_message());
        return Ok(());
    }
    for device in &devices {
        println!("{}", device.describe());
    }
    Ok(())
}

/// The one message that explains an empty browse, because it is the outcome most people
/// will meet first.
fn no_devices_message() -> String {
    String::from(
        "no cast device answered on this network.\n\
         That is a normal result, not necessarily a fault: mDNS is multicast, and multicast does not\n\
         cross a NAT. Inside WSL2's default networking, inside most containers, and across a VLAN\n\
         boundary, nothing will ever answer — and a speaker could not reach a media server bound in\n\
         there either. Try a longer --timeout, or run this from a native Linux or Windows build on\n\
         the same subnet as the speaker (WSL2 needs mirrored networking).",
    )
}

/// The default path: render the whole song, encode it, serve it, LOAD it.
fn cast_prerendered(args: &CastArgs, file: &std::path::Path, device: &CastDevice, interrupted: &AtomicBool) -> Result<(), String> {
    let bytes = archive::load_module_bytes(file, args.entry)?;
    if starplayer::probe_smf(&bytes) {
        return Err(String::from("`cast` plays tracker modules; a Standard MIDI File needs `render --instruments` first"));
    }
    let format = starplayer::probe(&bytes)
        .and_then(crate::render::golden_format)
        .ok_or_else(|| format!("{}: not a module format this build plays", file.display()))?;

    let module = starplayer::load(&bytes).map_err(|error| error.to_string())?;
    let title = String::from(module.header().title.as_ref());
    drop(module);

    let inserts = crate::render::parse_inserts(&args.insert)?;
    let enhance_chain = args.enhance.as_deref().map(|spec| crate::enhance_arg::parse_enhance_arg(spec, Some(args.rate.saturating_mul(2)))).transpose()?;
    let enhancer: Option<&dyn starplayer::model::SampleEnhancer> = enhance_chain.as_ref().map(|chain| chain as &dyn starplayer::model::SampleEnhancer);

    let mut length = RenderLength::default_for(args.rate);
    length.repeat_count = args.repeat;
    length.fade_frames = crate::render::seconds_to_frames(args.fade.unwrap_or(LOOP_FADE_SECONDS), args.rate);
    length.at_end = AtEnd::FadeOut;

    println!("render  {} at {} Hz, 16-bit stereo ...", file.display(), args.rate);
    let samples = crate::render::render_pcm_i16_stereo(PcmTarget {
        mix_path: args.mix_path,
        interp: args.interp,
        format,
        bytes: &bytes,
        rate: args.rate,
        host_block_frames: starplayer_offline::GOLDEN_HOST_BLOCK_FRAMES,
        length,
        inserts: &inserts,
        enhancer,
    })?;
    let frames = samples.len() / 2;
    let duration_seconds = frames as f32 / args.rate.max(1) as f32;

    let mut encoder = new_encoder(args.format, args.rate, 2)?;
    let encoded = encoder.encode_all(&samples, args.rate, 2).map_err(|error| error.to_string())?;
    println!(
        "encode  {} · {frames} frames ({}) · {:.1} KiB",
        encoder.content_type(),
        format_seconds(frames as u64, args.rate),
        encoded.len() as f64 / 1024.0
    );

    let mut server = MediaServer::bind_for(device.address).map_err(|error| error.to_string())?;
    let path = media_path(&title, file, encoder.extension());
    server.serve_bytes(&path, encoder.content_type(), encoded);
    let url = server.url(&path);
    println!("serving {url}");
    warn_about_an_unroutable_url(&url);

    let media = MediaRequest {
        content_id: url,
        content_type: String::from(encoder.content_type()),
        stream_kind: StreamKind::Buffered,
        title: display_title(&title, file),
        duration_seconds: Some(duration_seconds),
    };
    run_session(args, device, &media, media_session(interrupted, None), server)
}

/// `--live`: a `Player` on a `CastStreamBackend`, chunked into the HTTP response.
fn cast_live(args: &CastArgs, file: &std::path::Path, device: &CastDevice, interrupted: &AtomicBool) -> Result<(), String> {
    let bytes = archive::load_module_bytes(file, args.entry)?;
    if starplayer::probe_smf(&bytes) {
        return Err(String::from("`cast --live` plays tracker modules; a Standard MIDI File needs `render --instruments` first"));
    }

    let (chunk_sender, chunk_receiver) = sync_channel::<Vec<u8>>(LIVE_CHUNK_CAPACITY);
    let encoder = new_encoder(args.format, args.rate, 2)?;
    let content_type = String::from(encoder.content_type());
    let extension = encoder.extension();

    let spec = AudioSpec { sample_rate_hz: args.rate, channels: MixerMode::DEFAULT.channels as u16, preferred_block_frames: Some(LIVE_BLOCK_FRAMES) };
    let mut backend = CastStreamBackend::new(spec, encoder, chunk_sender);
    let mut player = Player::open(&mut backend, None, spec, MixerMode::DEFAULT).map_err(|error| format!("could not open the cast stream: {error}"))?;

    let title = match args.enhance.as_deref() {
        Some(spec) => {
            let chain = crate::enhance_arg::parse_enhance_arg(spec, Some(args.rate.saturating_mul(2)))?;
            let module = starplayer::load(&bytes).map_err(|error| error.to_string())?;
            let module = module.enhanced(&chain).map_err(|error| error.to_string())?;
            let title = String::from(module.header().title.as_ref());
            player.load_module(starplayer::rt::Arc::new(module)).map_err(|error| error.to_string())?;
            title
        }
        None => {
            player.load(&bytes).map_err(|error| error.to_string())?;
            player.module().map(|module| String::from(module.header().title.as_ref())).unwrap_or_default()
        }
    };

    for insert in crate::insert_arg::parse_insert_args(&args.insert)? {
        player.install_insert(insert.target, insert.slot, insert.kind).map_err(|error| error.to_string())?;
        for (param, value) in insert.params {
            player.set_insert_param(insert.target, insert.slot, param, value).map_err(|error| error.to_string())?;
        }
    }
    // A live stream has no end, so the song loops rather than fading out: `--repeat` and
    // `--fade` have nothing to describe here.
    player.set_at_end(AtEnd::Continue).map_err(|error| error.to_string())?;

    let mut server = MediaServer::bind_for(device.address).map_err(|error| error.to_string())?;
    let path = media_path(&title, file, extension);
    server.serve_stream(&path, &content_type, chunk_receiver);
    let url = server.url(&path);
    println!("serving {url} (live, chunked)");
    warn_about_an_unroutable_url(&url);

    player.play().map_err(|error| error.to_string())?;

    let media = MediaRequest {
        content_id: url,
        content_type,
        stream_kind: StreamKind::Live,
        title: display_title(&title, file),
        duration_seconds: None,
    };
    run_session(args, device, &media, media_session(interrupted, Some(player)), server)
}

/// What the control loop owns for the lifetime of one cast session.
struct Session<'a> {
    interrupted: &'a AtomicBool,
    /// Present only on the `--live` path. The control loop is the sole caller.
    player: Option<Player>,
}

fn media_session(interrupted: &AtomicBool, player: Option<Player>) -> Session<'_> { Session { interrupted, player } }

/// Connect, launch, LOAD, then follow until the song ends or the operator says stop.
fn run_session(args: &CastArgs, device: &CastDevice, media: &MediaRequest, mut session_state: Session<'_>, server: MediaServer) -> Result<(), String> {
    let mut session = CastSession::connect(device.address, device.port).map_err(|error| error.to_string())?;
    session.launch_default_media_receiver().map_err(|error| error.to_string())?;

    // One player at a time: if the receiver was already showing something, take it down
    // before loading ours rather than queueing behind it.
    if !session.launched_here()
        && let Ok(status) = session.status()
        && status.player_state != "IDLE"
    {
        println!("receiver was already playing ({}); stopping it first", status.player_state);
        let _ = session.stop_media();
    }

    if let Some(level) = args.volume {
        session.set_volume(level).map_err(|error| error.to_string())?;
    }

    session.load(media).map_err(|error| error.to_string())?;
    println!("loaded  {:?} as {}", media.title, if media.stream_kind == StreamKind::Live { "a live stream" } else { "a buffered stream" });
    println!("keys    p play/pause · s stop · q quit · +/- volume · </> seek ±{SEEK_STEP:.0}s · enter status (one letter, then Enter)");

    let outcome = follow(&mut session, &mut session_state, media, args.volume);

    // The exit path, and all three steps happen whatever ended the session — a natural
    // end, `q`, or a Ctrl-C mid-song.
    println!();
    let _ = session.stop_media();
    if session.launched_here() {
        let _ = session.stop_app();
    } else {
        println!("leaving the receiver application running: this session attached to it rather than starting it");
    }
    if let Some(player) = session_state.player.as_mut() {
        let _ = player.stop();
        player.collect_garbage();
    }
    // Dropping the player closes the cast stream, which flushes the encoder and closes the
    // chunk channel; the server thread then finishes its response and can be joined.
    drop(session_state.player.take());
    server.shutdown();
    outcome
}

/// Why the control loop stopped.
enum Outcome {
    /// The receiver reached the end of the media, or the operator asked to stop.
    Ended,
    /// Something in the session failed.
    Failed(String),
}

/// Poll the receiver once a second, print a status line, and act on stdin.
fn follow(session: &mut CastSession, state: &mut Session<'_>, media: &MediaRequest, initial_volume: Option<f32>) -> Result<(), String> {
    let keys = spawn_key_reader();
    let mut next_report = Instant::now();
    // Track from what `--volume` set, or the receiver's usual default when nothing was asked.
    let mut volume = initial_volume.unwrap_or(0.5).clamp(0.0, 1.0);
    let mut paused = false;
    let mut position = 0.0f32;

    let outcome = loop {
        if state.interrupted.load(Ordering::SeqCst) {
            break Outcome::Ended;
        }
        match keys.try_recv() {
            Ok(key) => match key {
                'q' | 's' => break Outcome::Ended,
                'p' => {
                    let result = if paused { session.play() } else { session.pause() };
                    if let Err(error) = result {
                        break Outcome::Failed(error.to_string());
                    }
                    paused = !paused;
                }
                '+' => {
                    volume = (volume + VOLUME_STEP).min(1.0);
                    if let Err(error) = session.set_volume(volume) {
                        break Outcome::Failed(error.to_string());
                    }
                }
                '-' => {
                    volume = (volume - VOLUME_STEP).max(0.0);
                    if let Err(error) = session.set_volume(volume) {
                        break Outcome::Failed(error.to_string());
                    }
                }
                '>' | '<' => {
                    if media.stream_kind == StreamKind::Live {
                        // A live stream has no addressable past; saying so beats a
                        // protocol error the operator has to interpret.
                        println!("\rseeking is not possible in a live stream");
                    } else {
                        let target = if key == '>' { position + SEEK_STEP } else { position - SEEK_STEP };
                        if let Err(error) = session.seek(target.max(0.0)) {
                            break Outcome::Failed(error.to_string());
                        }
                    }
                }
                // A bare newline: report now rather than waiting for the next second.
                '\n' => next_report = Instant::now(),
                _ => {}
            },
            Err(TryRecvError::Empty) => {}
            // The reader thread ended, which means stdin closed. Keep playing: a cast
            // session piped from a script has no keyboard and should still run.
            Err(TryRecvError::Disconnected) => {}
        }

        if Instant::now() >= next_report {
            // Answer anything the receiver sent unprompted — its PING, above all.
            if let Err(error) = session.pump() {
                break Outcome::Failed(error.to_string());
            }
            match session.status() {
                Ok(status) => {
                    position = status.current_time_seconds;
                    report(&status, volume, media);
                    if status.player_state == "IDLE" {
                        // `FINISHED` is the media running out, which is the normal end of a
                        // buffered cast. Anything else idle is somebody else's stop, and
                        // is equally a reason to let go.
                        break Outcome::Ended;
                    }
                }
                Err(error) => break Outcome::Failed(error.to_string()),
            }
            next_report = Instant::now() + REPORT_INTERVAL;
        }

        if let Some(player) = state.player.as_mut() {
            // The control thread is the sole producer on the command ring, so this is the
            // only place retired modules are dropped.
            player.collect_garbage();
            if player.health().is_failed() {
                break Outcome::Failed(String::from("the cast stream reported a fatal error"));
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    };

    match outcome {
        Outcome::Ended => Ok(()),
        Outcome::Failed(message) => Err(message),
    }
}

/// One updating status line: `\r` and no trailing newline, the shape `play` uses.
fn report(status: &starplayer_cast::MediaStatus, volume: f32, media: &MediaRequest) {
    use std::io::Write;

    let position = format_clock(status.current_time_seconds);
    let duration = match media.duration_seconds {
        Some(seconds) => format_clock(seconds),
        None => String::from("   live"),
    };
    let idle = status.idle_reason.as_deref().unwrap_or("");
    print!("\r{position} / {duration}  {:<10} {idle:<12} volume {:>3.0}%  ", status.player_state, volume * 100.0);
    let _ = std::io::stdout().flush();
}

/// `m:ss` from seconds, so the status line reads like a transport rather than a float.
fn format_clock(seconds: f32) -> String {
    let seconds = seconds.max(0.0);
    format!("{:>4}:{:02}", (seconds as u64) / 60, (seconds as u64) % 60)
}

/// A thread that turns stdin lines into single characters on a channel.
///
/// A thread rather than a poll, because there is no portable way to ask whether stdin has
/// a line waiting, and the control loop must never block on the keyboard: a cast session
/// with nobody at the terminal still has to keep pumping the receiver's heartbeat.
fn spawn_key_reader() -> Receiver<char> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { return };
            let key = line.trim().chars().next().unwrap_or('\n');
            if sender.send(key).is_err() {
                return;
            }
        }
    });
    receiver
}

/// The encoder `--format` named.
fn new_encoder(format: CastFormatArg, sample_rate_hz: u32, channels: u16) -> Result<Box<dyn CastEncoder + Send>, String> {
    match format {
        CastFormatArg::Flac => FlacEncoder::new(sample_rate_hz, channels).map(|encoder| Box::new(encoder) as Box<dyn CastEncoder + Send>).map_err(|error: CastError| error.to_string()),
        CastFormatArg::Wav => Ok(Box::new(WavEncoder::new(sample_rate_hz, channels))),
    }
}

/// The URL path the receiver fetches: the module's own title, slugified, then the file
/// stem, then `module`. The extension matters — some receivers sniff it.
fn media_path(title: &str, file: &std::path::Path, extension: &str) -> String {
    let stem = file.file_stem().and_then(|stem| stem.to_str()).and_then(starplayer_cast::slugify);
    let name = starplayer_cast::slugify(title).or(stem).unwrap_or_else(|| String::from("module"));
    format!("/{name}.{extension}")
}

/// What the Google Home app and the speaker's own controls show.
fn display_title(title: &str, file: &std::path::Path) -> String {
    if title.trim().is_empty() {
        file.file_name().and_then(|name| name.to_str()).unwrap_or("module").to_string()
    } else {
        title.trim().to_string()
    }
}

/// Say so, loudly, if the media URL names an address no speaker could fetch from.
fn warn_about_an_unroutable_url(url: &str) {
    if url.contains("127.0.0.1") || url.contains("0.0.0.0") || url.contains("[::1]") {
        println!(
            "warning: that URL is not reachable from the speaker. This machine has no route to it —\n\
             on WSL2's default NAT there is none — so the receiver will fetch nothing. A native\n\
             Linux or Windows build on the speaker's own subnet, or WSL2 mirrored networking, is\n\
             what this needs."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_media_path_prefers_the_modules_title_then_the_file_stem_then_a_constant() {
        assert_eq!(media_path("Beyond Music", Path::new("/tmp/bm.s3m"), "flac"), "/Beyond-Music.flac");
        assert_eq!(media_path("", Path::new("/tmp/petri.s3m"), "flac"), "/petri.flac");
        assert_eq!(media_path("♪♫", Path::new("/tmp/♪.s3m"), "wav"), "/module.wav");
    }

    #[test]
    fn a_media_path_is_only_ever_url_safe_ascii() {
        let path = media_path("a/b?c#d e", Path::new("/tmp/x.mod"), "flac");
        assert!(path.chars().all(|character| character.is_ascii_alphanumeric() || "/._-".contains(character)), "{path}");
    }

    #[test]
    fn a_module_with_no_title_is_displayed_by_its_file_name() {
        assert_eq!(display_title("  ", Path::new("/tmp/petri.s3m")), "petri.s3m");
        assert_eq!(display_title("Beyond Music ", Path::new("/tmp/bm.s3m")), "Beyond Music");
    }

    #[test]
    fn the_clock_reads_like_a_transport() {
        assert_eq!(format_clock(0.0), "   0:00");
        assert_eq!(format_clock(61.4), "   1:01");
        assert_eq!(format_clock(-5.0), "   0:00");
        assert_eq!(format_clock(3_600.0), "  60:00");
    }

    #[test]
    fn an_empty_browse_explains_itself_rather_than_just_failing() {
        let message = no_devices_message();
        assert!(message.contains("--timeout"), "{message}");
        assert!(message.contains("WSL2"), "{message}");
    }
}
