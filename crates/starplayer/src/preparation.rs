//! Fallible playback preparation with owned or caller-provided timeline storage.

use starplayer_core::Error;
use starplayer_core::quirks::{QuirkSelection, QuirkSet};
use starplayer_engine::{RowMark, ScanLimits, SongTimeline};
use starplayer_model::Module;
use starplayer_rt::Arc;
use crate::{NativeSequencer, ScannedSong};

type TimelineTables = (&'static mut [RowMark], &'static mut [Option<u32>]);

/// Fallible counterpart of [`crate::scan_song`].
pub fn try_scan_song(module: &Arc<Module>, sample_rate_hz: u32, limits: ScanLimits) -> Result<ScannedSong, Error> {
    try_scan_song_with_voice_capacity(module, sample_rate_hz, limits, crate::recommended_voice_capacity(module))
}

/// Scan with the host's actual fixed voice capacity, retaining canonical timing selection.
pub fn try_scan_song_with_voice_capacity(module: &Arc<Module>, sample_rate_hz: u32, limits: ScanLimits, voice_capacity: usize) -> Result<ScannedSong, Error> {
    let quirks = select_quirks(module, sample_rate_hz, limits, voice_capacity)?;
    scan_with_storage(module, sample_rate_hz, limits, voice_capacity, quirks, None)
}

/// Scan into caller-owned tables. Borrowed timeline clones share immutable storage.
pub fn try_scan_song_in(module: &Arc<Module>, sample_rate_hz: u32, limits: ScanLimits, marks: &'static mut [RowMark], order_marks: &'static mut [Option<u32>]) -> Result<ScannedSong, Error> {
    try_scan_song_in_with_voice_capacity(module, sample_rate_hz, limits, crate::recommended_voice_capacity(module), marks, order_marks)
}

/// Scan into caller-owned tables using the host's actual fixed voice capacity.
pub fn try_scan_song_in_with_voice_capacity(module: &Arc<Module>, sample_rate_hz: u32, limits: ScanLimits, voice_capacity: usize, marks: &'static mut [RowMark], order_marks: &'static mut [Option<u32>]) -> Result<ScannedSong, Error> {
    let quirks = select_quirks(module, sample_rate_hz, limits, voice_capacity)?;
    scan_with_storage(module, sample_rate_hz, limits, voice_capacity, quirks, Some((marks, order_marks)))
}

fn select_quirks(module: &Arc<Module>, sample_rate_hz: u32, limits: ScanLimits, voice_capacity: usize) -> Result<QuirkSet, Error> {
    let dialect = module.header().dialect.quirks();
    let _ = (sample_rate_hz, limits, voice_capacity);
    #[cfg(feature = "mod")]
    if module.header().format == starplayer_model::ModuleFormat::Mod {
        use starplayer_core::quirks::ModTiming;
        use starplayer_engine::EndReason;
        use starplayer_mod::TimingVerdict;
        let cia = QuirkSet { mod_timing: ModTiming::Cia, ..dialect };
        let vblank = QuirkSet { mod_timing: ModTiming::VBlank, ..dialect };
        return match starplayer_mod::timing_verdict_for(module).unwrap_or(TimingVerdict::Cia) {
            TimingVerdict::Cia => Ok(cia),
            TimingVerdict::VBlank => Ok(vblank),
            TimingVerdict::CompareLengths => {
                let first = scan_mod_end(module, sample_rate_hz, limits, voice_capacity, cia)?;
                let over_budget = matches!(first.0, EndReason::Budget);
                if !over_budget && first.1 < crate::VBLANK_COMPARISON_THRESHOLD_SECONDS * sample_rate_hz as u64 { return Ok(cia); }
                let second = scan_mod_end(module, sample_rate_hz, limits, voice_capacity, vblank)?;
                let both_over_budget = over_budget && matches!(second.0, EndReason::Budget);
                Ok(if !both_over_budget && second.1 < first.1 { vblank } else { cia })
            }
        };
    }
    Ok(dialect)
}

#[cfg(feature = "mod")]
fn scan_mod_end(module: &Arc<Module>, sample_rate_hz: u32, limits: ScanLimits, voice_capacity: usize, quirks: QuirkSet) -> Result<(starplayer_engine::EndReason, u64), Error> {
    let native = NativeSequencer::try_new_with_voice_capacity(Arc::clone(module), sample_rate_hz, QuirkSelection::Override(quirks), voice_capacity)?;
    match native {
        NativeSequencer::Mod(mut sequencer) => starplayer_engine::try_scan_timeline_end(&mut sequencer, limits).map_err(|_| Error::Resource("not enough memory for the timing scan")),
        #[allow(unreachable_patterns)]
        _ => Err(Error::Invalid("MOD timing scan requires a MOD module")),
    }
}

#[cfg(any(feature = "s3m", feature = "mod", feature = "mtm", feature = "xm", feature = "it"))]
fn timeline_error(error: starplayer_engine::TimelineBufferError) -> Error {
    use starplayer_engine::TimelineBufferError;
    match error {
        TimelineBufferError::Allocation(_) => Error::Resource("not enough memory for the playback timeline"),
        TimelineBufferError::TooSmall { .. } => Error::Resource("caller storage is too small for the playback timeline"),
    }
}

#[cfg_attr(not(any(feature = "s3m", feature = "mod", feature = "mtm", feature = "xm", feature = "it")), allow(unreachable_code, unused_mut, unused_variables))]
fn scan_with_storage(module: &Arc<Module>, sample_rate_hz: u32, limits: ScanLimits, voice_capacity: usize, quirks: QuirkSet, tables: Option<TimelineTables>) -> Result<ScannedSong, Error> {
    let mut native = NativeSequencer::try_new_with_voice_capacity(Arc::clone(module), sample_rate_hz, QuirkSelection::Override(quirks), voice_capacity)?;
    #[cfg(any(feature = "s3m", feature = "mod", feature = "mtm", feature = "xm", feature = "it"))]
    macro_rules! scan {
        ($sequencer:expr) => {
            match tables {
                Some((marks, order_marks)) => starplayer_engine::try_scan_timeline_in($sequencer, limits, marks, order_marks).map_err(timeline_error),
                None => starplayer_engine::try_scan_timeline($sequencer, limits).map_err(|_| Error::Resource("not enough memory for the playback timeline")),
            }
        };
    }
    let timeline: Result<SongTimeline, Error> = match native {
        #[cfg(feature = "s3m")]
        NativeSequencer::S3m(ref mut sequencer) => scan!(sequencer),
        #[cfg(feature = "mod")]
        NativeSequencer::Mod(ref mut sequencer) => scan!(sequencer),
        #[cfg(feature = "mtm")]
        NativeSequencer::Mtm(ref mut sequencer) => scan!(sequencer),
        #[cfg(feature = "xm")]
        NativeSequencer::Xm(ref mut sequencer) => scan!(sequencer),
        #[cfg(feature = "it")]
        NativeSequencer::It(ref mut sequencer) => scan!(sequencer),
    };
    Ok(ScannedSong { timeline: timeline?, quirks })
}
