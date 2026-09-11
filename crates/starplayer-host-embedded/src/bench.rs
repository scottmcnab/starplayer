//! The on-device bench: the golden digest, and a plain render for a cycle count.
//!
//! M8's exit criterion is that the device's own SHA-256 of the ten-second fixed / mono /
//! linear render of every golden fixture equals the hash committed under `goldens/`. That
//! is a claim about *this* build on *that* silicon, so the render and the hash both have to
//! happen there — which is what this module is.
//!
//! # Why this is not [`RenderHalf`](crate::RenderHalf)
//!
//! The goldens are **mono**, they have no transport over them, and their master bus runs
//! [`Limiter::Clamp`] rather than the soft knee a player uses. So the bench builds its own
//! engine: the same `Engine::with_settings` → `take_control` → `load_module` →
//! `set_source` → `set_limiter` sequence as `starplayer_offline`'s `render_with_kernel`,
//! which is what wrote the committed hashes. The mono arm is a second **instantiation** of
//! the same generic engine — `FixedOut<i16, 1>` instead of `FixedOut<i16, 2>` — never a
//! second code path.
//!
//! Neither routine is real-time: both allocate an engine, and [`render_digest`] allocates a
//! block-sized scratch. They run once, from a firmware task, before or instead of playback.

use alloc::boxed::Box;
use alloc::vec;

use sha2::{Digest, Sha256};
use starplayer::core::quirks::QuirkSelection;
use starplayer::dsp::Interpolate;
use starplayer::engine::Engine;
use starplayer::mixer::{FixedFrame, FixedPath, Limiter, MonoI16, OutputFormat, StereoI16};
use starplayer::model::Module;
use starplayer::{NativeSequencer, rt::Arc};

use crate::player::settings_for;
use crate::source::scan_module;
use crate::Error;

/// One engine built the way `starplayer_offline::render_with_kernel` builds one, in
/// whichever channel count `Out` names.
///
/// Every difference between this and a playing [`EmbeddedPlayer`](crate::EmbeddedPlayer) is
/// deliberate and is what the golden contract specifies: no transport, no telemetry reader
/// taken, and the transparent clamp on the master bus. `Out` is the **only** thing that
/// differs between the mono digest and the stereo cycle count: one generic function, two
/// instantiations, never two code paths.
fn golden_engine<Interp, Out>(module: &Arc<Module>, sample_rate_hz: u32) -> Result<Engine<FixedPath, Interp, Out, Arc<Module>>, Error>
where
    Interp: Interpolate,
    Out: OutputFormat<Accumulator = FixedFrame, Sample = i16>,
{
    let mut engine = Engine::<FixedPath, Interp, Out, Arc<Module>>::with_settings(settings_for(module, sample_rate_hz));
    let mut control = engine.take_control().ok_or(Error::EngineHandleUnavailable)?;
    control.load_module(Arc::clone(module)).map_err(|_| Error::CommandQueueFull)?;
    // The scan's quirks, never the header's: for a MOD it is the scan that settles CIA
    // against VBlank, and a fixed-length hash has to be taken the way the module would be
    // played.
    let quirks = scan_module(module, sample_rate_hz)?.quirks;
    let sequencer = NativeSequencer::new(Arc::clone(module), sample_rate_hz, QuirkSelection::Override(quirks))?;
    engine.set_source(Box::new(sequencer));
    engine.set_limiter(Limiter::Clamp);
    Ok(engine)
}

/// SHA-256 of `frames` frames of `module` rendered **mono** `i16`, in blocks of
/// `block_frames`.
///
/// The bytes hashed are the samples as little-endian signed PCM words, which is exactly
/// `starplayer_offline::canonical_sha256_with`'s rule — hashing an explicit byte order,
/// rather than the in-memory representation of `i16`, is what lets x86-64, Xtensa and
/// RISC-V compare one digest. At `sample_rate_hz` 44 100, `frames` 441 000 and
/// `block_frames` 128 this returns the hash committed under `goldens/`.
///
/// The hash is taken incrementally, block by block, so the whole ten seconds never exists
/// in RAM at once: the only buffer is `block_frames` samples long.
pub fn render_digest<Interp: Interpolate>(module: &Arc<Module>, sample_rate_hz: u32, frames: usize, block_frames: usize) -> Result<[u8; 32], Error> {
    let mut engine = golden_engine::<Interp, MonoI16>(module, sample_rate_hz)?;
    let block_samples = block_frames.max(1);
    let mut scratch = vec![0i16; block_samples];
    let mut hasher = Sha256::new();
    let mut produced = 0usize;
    while produced < frames {
        let take = block_samples.min(frames.saturating_sub(produced));
        let Some(chunk) = scratch.get_mut(..take) else { break };
        engine.render(chunk);
        for sample in chunk.iter() {
            hasher.update(sample.to_le_bytes());
        }
        produced = produced.saturating_add(take);
    }
    // The goldens are a contract about a *clean* render: a zero-advance guard that fired or
    // a dropped retirement means the bytes hashed are not the bytes the contract is about.
    let warnings = engine.warnings();
    if warnings.any() {
        return Err(Error::EngineWarnings(warnings));
    }

    let digest = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);
    Ok(hash)
}

/// Render `frames` frames of `module` as interleaved **stereo** `i16` through
/// `scratch`, and report how many frames were produced.
///
/// This is the cycles-per-frame measurement's body: the firmware reads its cycle counter,
/// calls this, reads it again, and divides by the returned count. `scratch` is the caller's
/// so that the figure is taken at the block size the device really uses — its I2S DMA
/// buffer — and so that the measurement itself allocates nothing.
///
/// Building the engine (the voice pool, the buses, the scan) happens **inside** this call
/// and is therefore included in whatever the caller times. Ask for a long render — ten
/// seconds is 441 000 frames — so the one-off construction amortises away; a firmware that
/// wants the per-block figure on its own should bracket
/// [`RenderHalf::render`](crate::RenderHalf::render) instead.
pub fn render_frames<Interp: Interpolate>(module: &Arc<Module>, sample_rate_hz: u32, frames: usize, scratch: &mut [i16]) -> Result<usize, Error> {
    let mut engine = golden_engine::<Interp, StereoI16>(module, sample_rate_hz)?;
    let block_frames = (scratch.len() / StereoI16::CHANNELS).max(1);
    let mut produced = 0usize;
    while produced < frames {
        let take = block_frames.min(frames.saturating_sub(produced));
        let Some(chunk) = scratch.get_mut(..take.saturating_mul(StereoI16::CHANNELS)) else { break };
        engine.render(chunk);
        produced = produced.saturating_add(take);
    }
    Ok(produced)
}
