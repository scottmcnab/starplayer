//! `starplayer info` — a module's header, its resolved playback quirks, and its scanned
//! length: the same length and loop/end verdict the web player's progress slider shows,
//! because both go through [`starplayer::scan_song`].

use std::path::PathBuf;

use starplayer::engine::{EndReason, ScanLimits};
use starplayer::model::{ModuleFlags, ModuleFormat};
use starplayer::rt::Arc;

use crate::archive;

/// The rate `info`'s scan measures a song's length and loop point at. Not a knob: a
/// tracker tick's length does not depend on the output rate, so any rate reports the same
/// song shape, and this is the rate the goldens and the web player both already use for
/// exactly this question.
const INFO_SCAN_SAMPLE_RATE_HZ: u32 = starplayer_offline::GOLDEN_SAMPLE_RATE_HZ;

#[derive(clap::Args, Debug)]
pub struct InfoArgs {
    /// Module file, or a ZIP archive containing one.
    pub file: PathBuf,
    /// Which recognised entry of a ZIP archive to inspect. With none given, an archive
    /// holding more than one module lists its entries instead of loading one.
    #[arg(long)]
    pub entry: Option<usize>,
}

pub fn run(args: InfoArgs) -> Result<(), String> {
    let bytes = archive::read_file(&args.file)?;

    if starplayer_archive::is_zip(&bytes) && args.entry.is_none() {
        let modules = starplayer_archive::list_modules(&bytes).map_err(|error| format!("{}: {error}", args.file.display()))?;
        if modules.len() > 1 {
            list_archive_entries(&args.file, &modules);
            return Ok(());
        }
    }

    let module_bytes = archive::resolve_entry(&args.file, &bytes, args.entry)?;
    if starplayer::probe_smf(&module_bytes) {
        return print_smf_info(&module_bytes);
    }
    print_module_info(&module_bytes)
}

/// `starplayer info` on a `.mid`: format, tracks, division, tempo changes and length
/// (task E5 deliverable 4). A Standard MIDI File is not a
/// [`Module`](starplayer::model::Module) — it carries no samples of its own — so its
/// length is the file's own [`Smf::length_frames`](starplayer::midi::Smf::length_frames)
/// rather than a scanned song timeline, and it names no playback quirks.
fn print_smf_info(bytes: &[u8]) -> Result<(), String> {
    let smf = starplayer::midi::smf::parse_smf(bytes).map_err(|error| error.to_string())?;

    println!("format:       Standard MIDI File, format {}", smf.format());
    println!("tracks:       {}", smf.track_count());
    match smf.division() {
        starplayer::midi::Division::TicksPerQuarterNote(ppqn) => println!("division:     {ppqn} ticks per quarter note"),
        starplayer::midi::Division::Smpte { frames_per_second, ticks_per_frame } => {
            println!("division:     SMPTE {} fps, {ticks_per_frame} ticks per frame", -(frames_per_second as i16));
        }
    }
    println!("events:       {}", smf.event_count());

    let tempo_changes = smf.tempo_changes();
    if tempo_changes.is_empty() {
        println!("tempo:        120 BPM (default, no set_tempo meta events)");
    } else {
        println!("tempo changes: {}", tempo_changes.len());
        for change in tempo_changes {
            let bpm = 60_000_000.0 / change.micros_per_quarter_note as f64;
            println!("  tick {:<8} {bpm:.2} BPM ({} us/quarter)", change.tick, change.micros_per_quarter_note);
        }
    }

    let length_frames = smf.length_frames(INFO_SCAN_SAMPLE_RATE_HZ);
    let length_seconds = length_frames as f64 / INFO_SCAN_SAMPLE_RATE_HZ as f64;
    println!("length:       {} ({length_frames} frames at {INFO_SCAN_SAMPLE_RATE_HZ} Hz)", format_duration(length_seconds));
    println!("instruments:  none of its own; render or play it with --instruments <module>");

    Ok(())
}

fn list_archive_entries(path: &std::path::Path, modules: &[starplayer_archive::ArchiveEntry]) {
    println!("{}: {} recognised module(s)", path.display(), modules.len());
    for (position, entry) in modules.iter().enumerate() {
        println!("  [{position}] {} ({:?}, {} bytes)", entry.name, entry.format, entry.size);
    }
    println!("pick one with --entry N");
}

fn print_module_info(bytes: &[u8]) -> Result<(), String> {
    let module = Arc::new(starplayer::load(bytes).map_err(|error| error.to_string())?);
    let header = module.header();
    let title = if header.title.is_empty() { "(untitled)" } else { header.title.as_ref() };
    let format = header.format;

    println!("title:        {title}");
    println!("format:       {:?}", header.format);
    println!("dialect:      {:?}", header.dialect);
    println!("channels:     {}", header.channel_count);
    println!("samples:      {}", module.samples().len());
    println!("instruments:  {}", module.instruments().len());
    println!("patterns:     {}", module.patterns().len());
    println!("orders:       {}", module.orders().len());
    println!("speed/tempo:  {} ticks/row, {} BPM (initial)", header.initial_speed, header.initial_tempo);
    println!("volume:       global {}, master {}", header.global_volume, header.master_volume);
    println!("flags:        {}", format_flags(&header.flags));

    let scanned = starplayer::scan_song(&module, INFO_SCAN_SAMPLE_RATE_HZ, ScanLimits::for_rate(INFO_SCAN_SAMPLE_RATE_HZ)).map_err(|error| error.to_string())?;
    println!("tempo model:  {:?}", scanned.quirks.tempo_model);
    if format == ModuleFormat::Mod {
        println!("mod timing:   {:?}", scanned.quirks.mod_timing);
    }

    let timeline = &scanned.timeline;
    println!("length:       {} ({} frames at {} Hz)", format_duration(timeline.duration_seconds()), timeline.end_frame(), INFO_SCAN_SAMPLE_RATE_HZ);
    println!("at end:       {}", format_end(timeline.end()));

    Ok(())
}

fn format_flags(flags: &ModuleFlags) -> String {
    let mut names = Vec::new();
    if flags.amiga_limits { names.push("amiga-limits"); }
    if flags.linear_slides { names.push("linear-slides"); }
    if flags.fast_volume_slides { names.push("fast-volume-slides"); }
    if flags.stereo { names.push("stereo"); }
    if names.is_empty() { "none".to_string() } else { names.join(", ") }
}

/// `m:ss.mmm`, as the task's deliverable spells it — deliberately more precise than the
/// web player's `m:ss` progress readout, which this command does not otherwise imitate.
fn format_duration(total_seconds: f64) -> String {
    let total_seconds = total_seconds.max(0.0);
    let whole_seconds = total_seconds.floor() as u64;
    let minutes = whole_seconds / 60;
    let seconds = whole_seconds % 60;
    let milliseconds = ((total_seconds - whole_seconds as f64) * 1_000.0).round() as u64;
    format!("{minutes}:{seconds:02}.{milliseconds:03}")
}

fn format_end(end: EndReason) -> String {
    match end {
        EndReason::Ended => "ends (order list runs out)".to_string(),
        EndReason::Stopped => "ends (stop marker)".to_string(),
        EndReason::Looped { target } => format!("loops to order {} row {}", target.order, target.row),
        EndReason::Budget => "ran out of scan budget (length is a lower bound)".to_string(),
    }
}
