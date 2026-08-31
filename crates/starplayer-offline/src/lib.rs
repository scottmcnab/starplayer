//! Deterministic offline rendering: the WAV writer, fixed-block-size render drivers and
//! the per-tick trace dump.
//!
//! This is where the buffer-size-independence invariant is exercised — rendering the same
//! module at host block sizes 1, 3, 64, 128, 4096 and 8191 must produce byte-identical
//! output.
//!
//! Allowed dependency edges: `starplayer`.

#![forbid(unsafe_code)]

use std::fmt;

use starplayer::core::{Error, ExactFixedPoint, Frame};
use starplayer::dsp::Linear;
use starplayer::engine::{EndOfSongPolicy, Engine, EngineSettings, EventSource, PatternSequencer, SequencerSettings, Trace};
use starplayer::mixer::{FixedPath, StereoI16};
use starplayer::model::Module;
use starplayer::rt::Arc;

/// Output rate used by diagnostic traces and the future canonical golden renderer.
pub const TRACE_SAMPLE_RATE_HZ: u32 = 44_100;

/// Guard against a malformed module whose control flow never reaches its end marker.
pub const MAX_CAPTURE_TICKS: usize = 1_000_000;

/// Knobs for deterministic trace capture.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TraceOptions {
    /// Stop after this many ticks. `None` follows the module until its end marker.
    pub ticks: Option<usize>,
    /// Frames requested per host call. This cannot affect the resulting trace.
    pub host_block_frames: usize,
}

impl Default for TraceOptions {
    fn default() -> TraceOptions { TraceOptions { ticks: None, host_block_frames: 128 } }
}

/// Failure to load or finish a diagnostic trace.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TraceError {
    /// The S3M loader rejected the input.
    Load(Error),
    /// Playback did not terminate within [`MAX_CAPTURE_TICKS`].
    TickLimit,
}

impl fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TraceError::Load(error) => write!(formatter, "could not load S3M module: {error}"),
            TraceError::TickLimit => write!(formatter, "trace exceeded the safety limit of {MAX_CAPTURE_TICKS} ticks"),
        }
    }
}

impl std::error::Error for TraceError {}

impl From<Error> for TraceError {
    fn from(error: Error) -> TraceError { TraceError::Load(error) }
}

type TraceEngine = Engine<FixedPath, Linear, StereoI16, Arc<Module>>;

/// Load an S3M and capture its stable per-tick trace.
///
/// M1 has one production loader/effect processor, S3M. MOD and MTM route through the same
/// entry point when their native processors land in C3/C4; no format is lowered to S3M.
pub fn trace_s3m(bytes: &[u8], options: TraceOptions) -> Result<Trace, TraceError> {
    let module = Arc::new(starplayer::s3m::load(bytes)?);
    let channel_count = module.header().channel_count as usize;
    let engine_settings = EngineSettings {
        sample_rate_hz: TRACE_SAMPLE_RATE_HZ,
        channel_count,
        voice_capacity: channel_count.max(1),
        ..EngineSettings::default()
    };
    let mut engine: TraceEngine = Engine::with_settings(engine_settings);
    let mut control = engine.take_control().expect("a fresh offline engine owns its control handle");
    control.load_module(Arc::clone(&module)).map_err(|_| TraceError::Load(Error::Invalid("module command queue is full")))?;

    let sequencer_settings = SequencerSettings {
        sample_rate_hz: TRACE_SAMPLE_RATE_HZ,
        first_tick_frame: Frame::ZERO,
        initial_speed: module.header().initial_speed,
        initial_tempo_bpm: module.header().initial_tempo,
        restart_order: 0,
        end_of_song: EndOfSongPolicy::Stop,
    };
    let sequencer = PatternSequencer::new(
        ExactFixedPoint,
        starplayer::s3m::S3mPatternData(Arc::clone(&module)),
        starplayer::s3m::S3mProcessor::new(module, TRACE_SAMPLE_RATE_HZ),
        sequencer_settings,
    );
    engine.set_source(Box::new(sequencer));

    let requested_ticks = options.ticks.unwrap_or(MAX_CAPTURE_TICKS);
    if requested_ticks == 0 {
        return Ok(engine.take_trace());
    }
    let block_frames = options.host_block_frames.max(1);
    let mut output = vec![0i16; block_frames.saturating_mul(2).max(2)];
    while engine.trace().ticks.len() < requested_ticks && engine.sources().next_event_frame().is_some() {
        engine.render(&mut output);
    }

    let source_still_running = engine.sources().next_event_frame().is_some();
    let mut trace = engine.take_trace();
    if let Some(ticks) = options.ticks {
        trace.truncate(ticks);
    } else if source_still_running && trace.ticks.len() >= MAX_CAPTURE_TICKS {
        return Err(TraceError::TickLimit);
    }
    Ok(trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFLEX: &[u8] = include_bytes!("../../starplayer-s3m/tests/fixtures/REFLEX.S3M");

    fn trace_with_block_size(host_block_frames: usize) -> Trace {
        trace_s3m(REFLEX, TraceOptions { ticks: Some(24), host_block_frames }).expect("REFLEX traces")
    }

    #[test]
    fn a_trace_is_repeatable_and_host_block_size_independent() {
        let first = trace_with_block_size(128).to_text();
        assert_eq!(trace_with_block_size(128).to_text(), first, "two runs are byte-identical");
        for host_block_frames in [1, 3, 64, 128, 4096, 8191] {
            assert_eq!(trace_with_block_size(host_block_frames).to_text(), first, "host block size {host_block_frames} changed the trace");
        }
    }
}
