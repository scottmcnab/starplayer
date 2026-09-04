//! `starplayer trace` — a stable per-tick diagnostic trace of the sequencer's state,
//! printed to stdout as `starplayer_offline::trace_module` renders it.

use std::path::PathBuf;

#[cfg(feature = "trace")]
use starplayer_offline::TraceOptions;

#[cfg(feature = "trace")]
use crate::archive;

#[derive(clap::Args, Debug)]
pub struct TraceArgs {
    /// Module file, or a ZIP archive containing one.
    pub file: PathBuf,
    /// Stop after this many ticks. Runs to the module's end marker by default, capped at
    /// `starplayer_offline::MAX_CAPTURE_TICKS` — which is where an XM or IT whose order
    /// list wraps, like any module that jumps backwards, ends up.
    #[arg(long)]
    pub ticks: Option<usize>,
    /// Which recognised entry of a ZIP archive to trace.
    #[arg(long)]
    pub entry: Option<usize>,
}

#[cfg(feature = "trace")]
pub fn run(args: TraceArgs) -> Result<(), String> {
    let bytes = archive::load_module_bytes(&args.file, args.entry)?;
    let trace = starplayer_offline::trace_module(&bytes, TraceOptions { ticks: args.ticks, ..TraceOptions::default() }).map_err(|error| error.to_string())?;
    print!("{}", trace.to_text());
    Ok(())
}

/// The command exists in every build so `--help` is honest, but the recorder itself is a
/// diagnostic build: `trace` is off by default so that a workspace build never carries
/// the allocating per-tick recorder inside `render()` (see `Cargo.toml`).
#[cfg(not(feature = "trace"))]
pub fn run(args: TraceArgs) -> Result<(), String> {
    let _ = args;
    Err(String::from("this binary was built without the `trace` feature; rebuild with `cargo build -p starplayer-cli --features trace`"))
}
