//! `starplayer trace` — a stable per-tick diagnostic trace of the sequencer's state,
//! printed to stdout as `starplayer_offline::trace_module` renders it.

use std::path::PathBuf;

use starplayer_offline::TraceOptions;

use crate::archive;

#[derive(clap::Args, Debug)]
pub struct TraceArgs {
    /// Module file, or a ZIP archive containing one.
    pub file: PathBuf,
    /// Stop after this many ticks. Runs to the module's end marker by default, capped at
    /// `starplayer_offline::MAX_CAPTURE_TICKS`.
    #[arg(long)]
    pub ticks: Option<usize>,
    /// Which recognised entry of a ZIP archive to trace.
    #[arg(long)]
    pub entry: Option<usize>,
}

pub fn run(args: TraceArgs) -> Result<(), String> {
    let bytes = archive::load_module_bytes(&args.file, args.entry)?;
    let trace = starplayer_offline::trace_module(&bytes, TraceOptions { ticks: args.ticks, ..TraceOptions::default() }).map_err(|error| error.to_string())?;
    print!("{}", trace.to_text());
    Ok(())
}
