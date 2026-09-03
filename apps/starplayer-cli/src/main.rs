//! `starplayer` — the command-line player and offline renderer.
//!
//! # Usage
//!
//! ```text
//! starplayer info <file> [--entry N]
//!     Print a module's header, its resolved playback quirks, and its scanned length —
//!     the same length and loop/end verdict the web player's progress slider shows.
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
//! starplayer trace <file> [--ticks N]
//!     Print a per-tick diagnostic trace of the sequencer's state to stdout.
//!
//! starplayer play <file> [--entry N] [--device NAME] [--rate HZ] [--buffer FRAMES]
//!                         [--repeat] [--list-devices]
//!     Play a module on an audio output device through starplayer-host-cpal. Prints
//!     the title and the negotiated stream spec once, then order/pattern/row/speed/
//!     BPM/voices/peak on one updating line once a second. Stops at the song's natural
//!     end or a detected loop's fade unless --repeat keeps it looping; Ctrl-C stops the
//!     transport click-free and exits 0. --list-devices prints every output device on
//!     every backend this build can reach, instead of playing anything.
//! ```
//!
//! A ZIP archive is accepted anywhere a module file is. With more than one recognised
//! module inside, pass `--entry N` naming the entry — `starplayer info` on the archive
//! lists them.
//!
//! Apps depend only on the facade, plus the `std` helper crates this binary needs:
//! `starplayer-offline` for rendering and tracing, `starplayer-archive` for ZIP entries,
//! and `starplayer-host` / `starplayer-host-cpal` for `play`.

#![forbid(unsafe_code)]

mod archive;
mod info;
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
    command: Command,
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Info(args) => info::run(args),
        Command::Render(args) => render::run(args),
        Command::Trace(args) => trace::run(args),
        Command::Play(args) => play::run(args),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("starplayer: {message}");
            ExitCode::FAILURE
        }
    }
}
