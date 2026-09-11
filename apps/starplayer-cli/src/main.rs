//! `starplayer` — the command-line player and offline renderer.
//!
//! # Usage
//!
//! ```text
//! starplayer info <file> [--entry N]
//!     Print a module's header, its resolved playback quirks, and its scanned length —
//!     the same length and loop/end verdict the web player's progress slider shows. For
//!     a `.mid`, prints its format, track count, division, tempo changes and length
//!     instead — a Standard MIDI File is not a module (task E5).
//!
//! starplayer render <file> -o <out.wav> [options]
//!     Render a module to a WAV file with the offline renderer's deterministic
//!     `render_song`. With no other flag, the song plays once and fades ten seconds into
//!     a second pass, capped at an hour (`--repeat`, `--fade` and `--at-end` change
//!     that). `--golden` renders exactly the canonical segment this build's
//!     `goldens/` fingerprint — fixed path, linear interpolation, mono 16-bit,
//!     44.1 kHz, ten seconds, no fade — and conflicts with every other rendering
//!     knob, because that fixed recipe is not always reachable through them (a song
//!     shorter than ten seconds cannot be, without pushing `--repeat` past what its own
//!     length would otherwise call for; see `src/render.rs`'s module doc):
//!         starplayer render <file> -o out.wav --golden
//!     Every render prints the length written and the SHA-256 of its PCM payload, in the
//!     same little-endian encoding a committed `.sha256` golden file holds.
//!
//!     `<file>` may instead be a `.mid`, with `--instruments <module>` naming the module
//!     whose instruments play it (program numbers on its sixteen MIDI channels index
//!     that module's instruments) — every other rendering knob but `--golden` still
//!     applies:
//!         starplayer render song.mid --instruments module.it -o out.wav
//!
//!     --insert <target>:<effect>[:<param>=<value>,...] installs an insert effect before
//!     rendering (M7-H7), repeatably; --list-effects prints every effect this build can
//!     install and exits. See `src/insert_arg.rs`'s module doc for the grammar. Conflicts
//!     with --golden, which is DSP-bypassed by policy:
//!         starplayer render <file> -o out.wav --insert 1:reverb:room=60,mix=50
//!
//!     --enhance <spec> rebuilds every sample through a load-time enhancer before
//!     rendering (M10-K5b): entries joined with `+`, each an id from
//!     `starplayer_enhance::CATALOGUE` — `sinc2x`, `sinc4x`, `loop` or `loop=<frames>`. A
//!     `sincNx` stage is capped at twice `--rate`. --list-enhancers prints every enhancer
//!     this build's catalogue offers and exits. See `src/enhance_arg.rs`'s module doc for
//!     the grammar. An enhanced render is a different configuration from the goldens, so
//!     this conflicts with --golden:
//!         starplayer render <file> -o out.wav --enhance sinc4x+loop
//!
//! starplayer trace <file> [--ticks N]
//!     Print a per-tick diagnostic trace of the sequencer's state to stdout.
//!
//! starplayer play <file> [--entry N] [--device NAME] [--rate HZ] [--buffer FRAMES]
//!                         [--repeat] [--list-devices] [--midi PORT] [--list-midi-ports]
//!                         [--insert TARGET:EFFECT[:PARAM=VALUE,...]] [--list-effects]
//!                         [--enhance SPEC] [--list-enhancers]
//!     Play a module on an audio output device through starplayer-host-cpal. Prints
//!     the title and the negotiated stream spec once, then order/pattern/row/speed/
//!     BPM/voices/peak on one updating line once a second. Stops at the song's natural
//!     end or a detected loop's fade unless --repeat keeps it looping; Ctrl-C stops the
//!     transport click-free and exits 0. --list-devices prints every output device on
//!     every backend this build can reach, instead of playing anything.
//!
//!     --midi PORT plays the module's *instruments* from a MIDI input port instead of
//!     playing the module: the keyboard sounds the module's samples and its pattern data
//!     stays silent, because hearing both at once is task E7's jam mode. PORT is an
//!     index, an exact port name, or any case-insensitive part of one, and
//!     --list-midi-ports prints what this machine has.
//!
//!     --instruments MODULE names the module whose instruments play a `.mid` (task E5).
//!
//!     --insert and --list-effects are the same flags `render` takes (M7-H7), installed
//!     through `Player` before playback starts. --enhance and --list-enhancers are
//!     `render`'s same flags too (M10-K5b), rebuilding the module through
//!     `Module::enhanced` before `Player::load_module`; they apply only to a tracker
//!     module played directly, not to the instruments behind a Standard MIDI File.
//!
//! starplayer cast --list [--timeout SECS]
//! starplayer cast --device NAME <file> [--entry N] [--format flac|wav] [--rate HZ]
//!                 [--insert TARGET:EFFECT[:PARAM=VALUE,...]] [--enhance SPEC]
//!                 [--repeat N] [--fade S] [--live] [--volume 0..1] [--timeout SECS]
//!     Play a module on a Google Home or Nest speaker, or a speaker group (A4-N2). The
//!     speaker runs Google's own Default Media Receiver, which plays a media URL and
//!     nothing else, so this command renders the module here, serves the audio over HTTP
//!     from whichever local interface reaches the speaker, and hands the receiver that
//!     URL. No Google developer account and no registration are involved.
//!
//!     Two paths. By default the whole song is rendered first and served with HTTP Range
//!     support, which is what gives the speaker a duration, a seek bar and a clean end.
//!     --live streams it as it renders instead — no duration, no seeking, and the
//!     encoder's own lead on top of the speaker's buffer — which is for an endless
//!     session rather than for playing a song.
//!
//!     Transport commands are single letters on stdin, each followed by Enter: `p`
//!     play/pause, `s` stop, `q` quit, `+`/`-` volume by 0.05, `<`/`>` seek ∓10 s, and a
//!     bare Enter prints the status line straight away. Single keypresses would need raw
//!     terminal mode and a terminal crate this workspace does not have; that choice
//!     belongs to the TUI (A1), so this reads lines.
//!
//!     Expect roughly 2–5 s between a command and the speaker acting on it: the CASTv2
//!     round trip is about a millisecond on a LAN and the rest is the receiver's own
//!     playout buffer, which a sender cannot see or shorten. That is why there is no jam
//!     mode over Cast.
//!
//!     --list finding nothing is a normal result, not necessarily a fault: mDNS is
//!     multicast and multicast does not cross a NAT, so nothing will ever answer from
//!     inside WSL2's default networking or most containers — and a speaker could not
//!     reach a media server bound in there either.
//! ```
//!
//! A ZIP archive is accepted anywhere a module file is. With more than one recognised
//! module inside, pass `--entry N` naming the entry — `starplayer info` on the archive
//! lists them.
//!
//! Apps depend only on the facade, plus the `std` helper crates this binary needs:
//! `starplayer-offline` for rendering and tracing, `starplayer-archive` for ZIP entries,
//! `starplayer-host` / `starplayer-host-cpal` for `play`, and `starplayer-cast` for
//! `cast`.

#![forbid(unsafe_code)]

mod archive;
mod cast;
mod enhance_arg;
mod info;
mod insert_arg;
mod play;
mod render;
mod trace;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// StarPlayer's command-line player and offline renderer.
///
/// Plays, renders and inspects MOD/S3M/MTM/XM modules — see the module-level docs
/// (`starplayer --help` after any subcommand shows that subcommand's own flags).
#[derive(Parser, Debug)]
#[command(name = "starplayer", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Print every insert effect this build can install (M7-H7), its parameters, their
    /// units, ranges and defaults, and exit. Needs no subcommand; `render --list-effects`
    /// and `play --list-effects` do the same for a caller already typing one of those.
    #[arg(long)]
    list_effects: bool,
    /// Print every load-time sample enhancer `--enhance` can name (M10-K5b) and exit.
    /// Needs no subcommand; `render --list-enhancers` and `play --list-enhancers` do the
    /// same for a caller already typing one of those.
    #[arg(long)]
    list_enhancers: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print a module's header, resolved quirks and scanned length.
    Info(info::InfoArgs),
    /// Render a module to a WAV file.
    Render(render::RenderArgs),
    /// Print a per-tick diagnostic trace to stdout.
    Trace(trace::TraceArgs),
    /// Play a module through an audio output device.
    Play(play::PlayArgs),
    /// Play a module on a Google Home or Nest speaker.
    #[command(long_about = cast::CAST_LONG_ABOUT)]
    Cast(cast::CastArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // A top-level `starplayer --list-effects` needs no subcommand at all (M7-H7); `render
    // --list-effects` and `play --list-effects` are each subcommand's own copy of the same
    // flag, for a caller already typing one of those.
    if cli.list_effects {
        print!("{}", insert_arg::list_effects());
        return ExitCode::SUCCESS;
    }
    if cli.list_enhancers {
        print!("{}", enhance_arg::list_enhancers());
        return ExitCode::SUCCESS;
    }
    let Some(command) = cli.command else {
        eprintln!("starplayer: no subcommand given; see --help");
        return ExitCode::FAILURE;
    };

    let result = match command {
        Command::Info(args) => info::run(args),
        Command::Render(args) => render::run(args),
        Command::Trace(args) => trace::run(args),
        Command::Play(args) => play::run(args),
        Command::Cast(args) => cast::run(args),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("starplayer: {message}");
            ExitCode::FAILURE
        }
    }
}
