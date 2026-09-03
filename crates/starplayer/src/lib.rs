//! The public face of StarPlayer: re-exports of the engine surface plus format
//! autodetection over whichever format crates are enabled.
//!
//! This is *the* crate an embedder depends on. Everything below it is an implementation
//! detail whose layout may change; this crate's surface may not.
//!
//! The crate is `no_std` by default and stays that way: the `std` feature is opt-in and
//! is never reachable from any default feature set (AGENTS.md design goal 4). Standard
//! library access is pulled in explicitly below rather than by dropping `#![no_std]`, so
//! the bare-metal build cannot regress silently.
//!
//! Allowed dependency edges: `starplayer-core`, `starplayer-rt`, `starplayer-dsp`,
//! `starplayer-mixer`, `starplayer-model`, `starplayer-engine`, and — behind their own
//! features — `starplayer-{mod,s3m,mtm,xm,it}`, `starplayer-midi`,
//! `starplayer-telemetry`.
//!
//! # Quirk profiles and tracker dialects
//!
//! Two different questions, answered by two different types, and it is worth keeping them
//! apart.
//!
//! A **quirk profile** is what *you* choose. [`QuirkSet`](core::quirks::QuirkSet) is a
//! small `Copy` struct of named fields — a tempo model, a Paula clock, a handful of
//! booleans — and it has two named profiles. [`QuirkSet::canonical`](core::quirks::QuirkSet::canonical)
//! is the default: ProTracker 2.3D and Scream Tracker 3.21 as those programs actually
//! behaved. [`QuirkSet::starplayer_classic`](core::quirks::QuirkSet::starplayer_classic)
//! is the 1990s DOS StarPlayer's own behaviour where it differs and a modern default
//! should not have it — today that is its double-truncated tick length, which drifts
//! about 1.3 seconds over a four-minute song. Use `canonical()` to play modules
//! accurately; use `starplayer_classic()` to hear what the original sounded like. The
//! `quirks-starplayer` cargo feature makes the classic profile the per-dialect default for
//! a whole build.
//!
//! A **dialect** is what the *file* says. [`FormatDialect`](core::quirks::FormatDialect)
//! is what a loader concluded from the header alone — `CD61` is an Atari Octalyser MOD,
//! an S3M whose `Cwt/v` is below `0x1303` came from Scream Tracker 3.01 — and it is stored
//! in [`ModuleHeader::dialect`](model::ModuleHeader::dialect). Those trackers wrote the
//! same file format but replayed it differently, mostly in how `E6x` / `SBx` pattern loops
//! interact with pattern breaks and position jumps, so a module has to be played by its
//! own tracker's rules to sound right. You do not choose a dialect; the file does.
//!
//! The two meet in [`QuirkSelection`](core::quirks::QuirkSelection), which is the
//! precedence written into the type:
//!
//! ```no_run
//! use starplayer::core::quirks::{QuirkSelection, QuirkSet};
//! # let module: starplayer::rt::Arc<starplayer::model::Module> = unimplemented!();
//! // The usual case: let the file's dialect decide.
//! let sequencer = starplayer::mod_file::sequencer_with_quirks(module.clone(), 44_100, QuirkSelection::FromDialect);
//! // Or override it, for a compatibility menu or a regression test.
//! let classic = starplayer::mod_file::sequencer_with_quirks(module, 44_100, QuirkSelection::Override(QuirkSet::starplayer_classic()));
//! ```
//!
//! The chosen set also carries the [`TempoModelId`](core::TempoModelId), so one argument
//! selects both the effect behaviour and the tick length. It is resolved **once**, when
//! the sequencer is built, and is fixed for the lifetime of that loaded module: there is
//! no setter, because a channel whose effect memories were written under one rule and read
//! under another is not a tracker any tracker ever was. Load the module again to change
//! it.
//!
//! ## One field the file header cannot settle, and [`scan_song`]
//!
//! `mod_timing` is the exception. A MOD's tag says which tracker wrote it but not which
//! *interrupt* that tracker's replayer ran off, and the same `M.K.` tag covers both. On
//! the CIA timer an `Fxx` of 32 or more is a tempo in beats per minute; on the vertical
//! blank there is no timer and the same byte is a long row. Read a VBlank module as CIA
//! and a two-second fermata becomes twenty minutes at 32 BPM.
//!
//! Only the `M&K!` and `N.T.` tags settle it from the header. Everything else is decided
//! from the pattern cells and, where those are ambiguous, by scanning the song **both
//! ways** and keeping the shorter — which needs a sequencer and so cannot live in a
//! loader. [`scan_song`] is where it lives, and it is how a host gets a module's quirks:
//!
//! ```no_run
//! # let bytes: &[u8] = &[];
//! use starplayer::core::quirks::QuirkSelection;
//! use starplayer::engine::ScanLimits;
//! use starplayer::{NativeSequencer, rt::Arc};
//! let module = Arc::new(starplayer::load(bytes).expect("a supported format"));
//! let scanned = starplayer::scan_song(&module, 44_100, ScanLimits::for_rate(44_100)).expect("a supported format");
//! let mut sequencer = NativeSequencer::new(Arc::clone(&module), 44_100, QuirkSelection::Override(scanned.quirks))
//!     .expect("this build has that format's processor");
//! sequencer.set_timeline(scanned.timeline);
//! ```
//!
//! [`load`] → `Arc::new` → [`scan_song`] → [`NativeSequencer::new`] → `set_timeline` is
//! the whole host recipe, and [`NativeSequencer`] is the only place in the repository that
//! turns a runtime [`ModuleFormat`](model::ModuleFormat) into a typed sequencer: the web
//! player, the offline renderer and the trace path all go through it, so a format is wired
//! up once rather than once per host.
//!
//! A host must build its playback sequencer from `scanned.quirks` rather than from
//! `QuirkSelection::FromDialect`. The timeline was measured under those quirks: install it
//! in a sequencer that reads `Fxx` the other way and the progress slider, the loop point
//! and the audio all disagree about where the song is. Every host in this repository —
//! the offline renderer, the web player — does it that way, and a host that wants a
//! different timing still overrides it the same way it always could.
//!
//! Every field names the `plans/product/03-accuracy-policy.md` entry it implements, and a
//! field with no policy entry is not allowed. That document is the reference for what
//! "accurate" means here and for every place StarPlayer knowingly differs from ProTracker,
//! Scream Tracker 3 or the oracles it is measured against.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

/// Core event, timing and fixed-point types used by hosts.
pub use starplayer_core as core;
/// Interpolators used to select an engine instantiation.
pub use starplayer_dsp as dsp;
/// The render engine and tracker sequencing surface.
pub use starplayer_engine as engine;
/// Mixer paths and host output formats.
pub use starplayer_mixer as mixer;
/// Loaded-module and display-only pattern types.
pub use starplayer_model as model;
/// Real-time ownership primitives, including the portable [`Arc`](starplayer_rt::Arc).
pub use starplayer_rt as rt;

mod sequencer;

pub use sequencer::NativeSequencer;
#[cfg(feature = "mod")]
pub use sequencer::ModSequencer;
#[cfg(feature = "mtm")]
pub use sequencer::MtmSequencer;
#[cfg(feature = "s3m")]
pub use sequencer::S3mSequencer;
#[cfg(feature = "xm")]
pub use sequencer::XmSequencer;

/// The widest voice pool the engine will build, and what a **persistent** host takes.
///
/// A host that builds one engine before it has seen a module — the browser worklet, a
/// plugin — cannot ask a processor how wide its pool should be, so it takes the maximum
/// once and plays every module through it. A host that builds an engine per module asks
/// [`recommended_voice_capacity`] instead.
pub use starplayer_engine::MAX_VOICE_CAPACITY;
/// Coherent UI snapshots.
#[cfg(feature = "telemetry")]
pub use starplayer_telemetry as telemetry;

/// Scream Tracker 3 loading, native pattern access and sequencer construction.
#[cfg(feature = "s3m")]
pub use starplayer_s3m as s3m;

/// ProTracker MOD loading, native pattern access and sequencer construction.
///
/// `mod` is a Rust keyword, so the facade spells the namespace `mod_file`; the generic
/// [`load`] and [`probe`] entry points normally mean callers do not need this name.
#[cfg(feature = "mod")]
pub use starplayer_mod as mod_file;

/// MultiTracker loading, native pattern access and sequencer construction.
#[cfg(feature = "mtm")]
pub use starplayer_mtm as mtm;

/// FastTracker 2 loading, native pattern access and sequencer construction.
#[cfg(feature = "xm")]
pub use starplayer_xm as xm;

/// One module's scanned song shape and the [`QuirkSet`](core::quirks::QuirkSet) it was
/// scanned under.
///
/// Both fields go together on purpose: the quirks are what the timeline was measured
/// with, so a host that installs the timeline in a sequencer built from any other set has
/// two players with different opinions about how long the song is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedSong {
    /// Every row the song plays, when it plays it, and how the song ends.
    pub timeline: starplayer_engine::SongTimeline,
    /// The resolved quirks the scan ran under — build the playback sequencer with
    /// `QuirkSelection::Override` of exactly this.
    pub quirks: starplayer_core::quirks::QuirkSet,
}

/// One pass of a song that takes at least this long is the length at which libxmp stops
/// believing a MOD's CIA reading and tries the vertical blank as well.
///
/// libxmp `src/scan.c:50`, `VBLANK_TIME_THRESHOLD 480000.0` milliseconds.
pub const VBLANK_COMPARISON_THRESHOLD_SECONDS: u64 = 480;

/// Scan a loaded module through a throwaway sequencer and report both the song's shape and
/// the quirks it must be played with.
///
/// This is the entry point a host uses before it builds a playback sequencer. For S3M and
/// MTM it is one scan under the loader-detected dialect. For MOD it additionally resolves
/// `mod_timing`, which the file header cannot settle on its own: the tag, the pattern
/// evidence the loader gathered and — where those are ambiguous and the CIA reading runs
/// past [`VBLANK_COMPARISON_THRESHOLD_SECONDS`] — a second scan under
/// [`ModTiming::VBlank`](core::quirks::ModTiming::VBlank), keeping whichever pass is
/// shorter. That mirrors libxmp `src/scan.c:671-708`, and the second scan only ever runs
/// on a module that already takes eight minutes to scan through once.
///
/// Off the audio thread: it allocates the scan's throwaway sequencer, voice pool and
/// timeline.
pub fn scan_song(
    module: &starplayer_rt::Arc<starplayer_model::Module>,
    sample_rate_hz: u32,
    limits: starplayer_engine::ScanLimits,
) -> Result<ScannedSong, starplayer_core::Error> {
    let _ = (module, sample_rate_hz, limits);
    match module.header().format {
        #[cfg(feature = "mod")]
        starplayer_model::ModuleFormat::Mod => scan_mod(module, sample_rate_hz, limits),
        _ => {
            let quirks = module.header().dialect.quirks();
            scan_with(module, sample_rate_hz, limits, quirks)
        }
    }
}

/// One scan of `module` under an explicit resolved [`QuirkSet`](core::quirks::QuirkSet),
/// through a throwaway [`NativeSequencer`].
///
/// Every scan in this crate goes through here, so the sequencer a scan measures and the
/// sequencer a host plays are built by the same code from the same quirks.
///
/// With every format feature off there is nothing to scan: `NativeSequencer` is
/// uninhabited, the `?` above the match never returns, and the whole body is dead. The
/// `allow` says so, and exists only in that build.
#[cfg_attr(not(any(feature = "s3m", feature = "mod", feature = "mtm", feature = "xm")), allow(unreachable_code, unused_mut, unused_variables))]
fn scan_with(
    module: &starplayer_rt::Arc<starplayer_model::Module>,
    sample_rate_hz: u32,
    limits: starplayer_engine::ScanLimits,
    quirks: starplayer_core::quirks::QuirkSet,
) -> Result<ScannedSong, starplayer_core::Error> {
    let selection = starplayer_core::quirks::QuirkSelection::Override(quirks);
    let mut sequencer = NativeSequencer::new(starplayer_rt::Arc::clone(module), sample_rate_hz, selection)?;
    // `match sequencer` on the value rather than on a reference: with every format feature
    // off the enum is uninhabited, and an empty match is only accepted on a place
    // expression of an uninhabited type.
    let timeline = match sequencer {
        #[cfg(feature = "s3m")]
        NativeSequencer::S3m(ref mut sequencer) => starplayer_engine::scan_timeline(sequencer, limits),
        #[cfg(feature = "mod")]
        NativeSequencer::Mod(ref mut sequencer) => starplayer_engine::scan_timeline(sequencer, limits),
        #[cfg(feature = "mtm")]
        NativeSequencer::Mtm(ref mut sequencer) => starplayer_engine::scan_timeline(sequencer, limits),
        #[cfg(feature = "xm")]
        NativeSequencer::Xm(ref mut sequencer) => starplayer_engine::scan_timeline(sequencer, limits),
    };
    Ok(ScannedSong { timeline, quirks })
}

/// Whether this build has a native processor for `format` — whether
/// [`NativeSequencer::new`] can succeed for a module of that format.
///
/// The facade's format features are chosen by the embedder, so "is this format playable
/// here?" is a build-time question with a runtime answer. A test harness uses it to *gate*
/// a format rather than silently skip it.
///
/// Not `matches!`, whatever clippy says of the default build: each arm answers with its
/// own feature, and they only all read `true` when every format is compiled in.
#[allow(clippy::match_like_matches_macro)]
pub const fn supports(format: starplayer_model::ModuleFormat) -> bool {
    match format {
        starplayer_model::ModuleFormat::S3m => cfg!(feature = "s3m"),
        starplayer_model::ModuleFormat::Mod => cfg!(feature = "mod"),
        starplayer_model::ModuleFormat::Mtm => cfg!(feature = "mtm"),
        starplayer_model::ModuleFormat::Xm => cfg!(feature = "xm"),
        _ => false,
    }
}

/// How many voices a host should give the engine it builds for **this module**.
///
/// The format's own answer — the same number
/// [`NativeSequencer::recommended_voice_capacity`] reports — reached from the module
/// header alone, so a host can size an engine before it has a sequencer to ask. A format
/// this build cannot play answers with the channel count, which is what every engine sized
/// itself to before this existed.
///
/// It can be zero, for a module whose header claims no channels at all; a caller that
/// hands the number straight to [`EngineSettings`](engine::EngineSettings) should
/// `.max(1)`, exactly as [`scan_timeline`](engine::scan_timeline) does.
///
/// A **persistent** host — one engine, many modules — wants [`MAX_VOICE_CAPACITY`]
/// instead, because it has no module to ask about when it builds its engine.
pub fn recommended_voice_capacity(module: &starplayer_model::Module) -> usize {
    let channel_count = module.header().channel_count as usize;
    match module.header().format {
        #[cfg(feature = "s3m")]
        starplayer_model::ModuleFormat::S3m => starplayer_s3m::recommended_voice_capacity(channel_count),
        #[cfg(feature = "mod")]
        starplayer_model::ModuleFormat::Mod => starplayer_mod::recommended_voice_capacity(channel_count),
        #[cfg(feature = "mtm")]
        starplayer_model::ModuleFormat::Mtm => starplayer_mtm::recommended_voice_capacity(channel_count),
        #[cfg(feature = "xm")]
        starplayer_model::ModuleFormat::Xm => starplayer_xm::recommended_voice_capacity(channel_count),
        _ => channel_count,
    }
}

/// The MOD arm of [`scan_song`]: libxmp's loader verdict, then its length comparison.
#[cfg(feature = "mod")]
fn scan_mod(
    module: &starplayer_rt::Arc<starplayer_model::Module>,
    sample_rate_hz: u32,
    limits: starplayer_engine::ScanLimits,
) -> Result<ScannedSong, starplayer_core::Error> {
    use starplayer_core::quirks::{ModTiming, QuirkSet};
    use starplayer_engine::EndReason;
    use starplayer_mod::TimingVerdict;

    let dialect = module.header().dialect.quirks();
    let cia = QuirkSet { mod_timing: ModTiming::Cia, ..dialect };
    let vblank = QuirkSet { mod_timing: ModTiming::VBlank, ..dialect };
    let verdict = starplayer_mod::timing_verdict_for(module).unwrap_or(TimingVerdict::Cia);
    match verdict {
        TimingVerdict::Cia => scan_mod_with(module, sample_rate_hz, limits, cia),
        TimingVerdict::VBlank => scan_mod_with(module, sample_rate_hz, limits, vblank),
        TimingVerdict::CompareLengths => {
            let first = scan_mod_with(module, sample_rate_hz, limits, cia)?;
            // A scan that ran out its budget is past the threshold by definition: it never
            // reached the end of one pass.
            let over_budget = matches!(first.timeline.end(), EndReason::Budget);
            let threshold_frames = VBLANK_COMPARISON_THRESHOLD_SECONDS * sample_rate_hz as u64;
            if !over_budget && first.timeline.end_frame() < threshold_frames { return Ok(first); }
            // The rescan gets its own fresh `ScanLimits`, so a module that is over budget
            // both ways is not compared on two truncated lengths — it keeps CIA.
            let second = scan_mod_with(module, sample_rate_hz, limits, vblank)?;
            let both_over_budget = over_budget && matches!(second.timeline.end(), EndReason::Budget);
            let shorter = !both_over_budget && second.timeline.end_frame() < first.timeline.end_frame();
            Ok(if shorter { second } else { first })
        }
    }
}

#[cfg(feature = "mod")]
fn scan_mod_with(
    module: &starplayer_rt::Arc<starplayer_model::Module>,
    sample_rate_hz: u32,
    limits: starplayer_engine::ScanLimits,
    quirks: starplayer_core::quirks::QuirkSet,
) -> Result<ScannedSong, starplayer_core::Error> {
    scan_with(module, sample_rate_hz, limits, quirks)
}

/// Identify a module using the probes for the format capabilities compiled into this
/// facade. No extension or S3M lowering participates in this decision.
pub fn probe(bytes: &[u8]) -> Option<starplayer_model::ModuleFormat> {
    let _ = bytes;
    #[cfg(feature = "s3m")]
    if starplayer_s3m::probe(bytes) { return Some(starplayer_model::ModuleFormat::S3m); }
    #[cfg(feature = "mtm")]
    if starplayer_mtm::probe(bytes) { return Some(starplayer_model::ModuleFormat::Mtm); }
    #[cfg(feature = "xm")]
    if starplayer_xm::probe(bytes) { return Some(starplayer_model::ModuleFormat::Xm); }
    #[cfg(feature = "mod")]
    if starplayer_mod::probe(bytes) { return Some(starplayer_model::ModuleFormat::Mod); }
    None
}

/// Autodetect and load a module through its native format crate.
pub fn load(bytes: &[u8]) -> Result<starplayer_model::Module, starplayer_core::Error> {
    match probe(bytes) {
        #[cfg(feature = "s3m")]
        Some(starplayer_model::ModuleFormat::S3m) => starplayer_s3m::load(bytes),
        #[cfg(feature = "mod")]
        Some(starplayer_model::ModuleFormat::Mod) => starplayer_mod::load(bytes),
        #[cfg(feature = "mtm")]
        Some(starplayer_model::ModuleFormat::Mtm) => starplayer_mtm::load(bytes),
        #[cfg(feature = "xm")]
        Some(starplayer_model::ModuleFormat::Xm) => starplayer_xm::load(bytes),
        _ => Err(starplayer_core::Error::BadMagic),
    }
}

#[cfg(all(test, feature = "mod", feature = "mtm"))]
mod tests {
    use alloc::vec;
    use super::*;

    fn mtm_with_a_mod_tag_collision() -> alloc::vec::Vec<u8> {
        const TRACK_COUNT: usize = 5;
        const TRACK_OFFSET: usize = 66 + 128;
        const PATTERN_OFFSET: usize = TRACK_OFFSET + TRACK_COUNT * 192;
        let mut bytes = vec![0; PATTERN_OFFSET + 64];
        bytes[..4].copy_from_slice(b"MTM\x10");
        bytes[24..26].copy_from_slice(&(TRACK_COUNT as u16).to_le_bytes());
        bytes[32] = 64;
        bytes[33] = 1;
        bytes[34] = 8;
        // Offset 1080 lies in the fifth stored MTM track and is unconstrained pattern
        // data. It is also the weak signature offset used by the MOD probe.
        bytes[1080..1084].copy_from_slice(b"M.K.");
        bytes
    }

    #[test]
    fn strong_mtm_magic_wins_over_a_legal_offset_1080_mod_tag_collision() {
        let bytes = mtm_with_a_mod_tag_collision();
        assert!(starplayer_mod::probe(&bytes));
        assert!(starplayer_mtm::probe(&bytes));
        assert_eq!(probe(&bytes), Some(starplayer_model::ModuleFormat::Mtm));
        assert_eq!(load(&bytes).map(|module| module.header().format), Ok(starplayer_model::ModuleFormat::Mtm));
    }
}
