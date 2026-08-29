//! The voice render kernel: one function, monomorphised over the mixing path and the
//! interpolator, that turns one voice's sample data into accumulated output frames.
//!
//! # What makes this block-size independent
//!
//! Everything the kernel reads is either a property of the voice (position, step, gains)
//! or of the sample. Nothing depends on `destination.len()`. Splitting a run of frames
//! into two calls therefore produces exactly the byte sequence one call would have, which
//! is the invariant the block-size determinism test exists to protect.
//!
//! # Real-time safety
//!
//! No allocation, no locks, no panic and no `dyn` call. Every slice access goes through
//! `get`, and a sample region that does not resolve ends the voice rather than faulting —
//! `render()` outputs silence rather than panicking (architecture §8).

use starplayer_dsp::Interpolate;

use crate::path::MixPath;
use crate::sample::SampleData;
use crate::voice::Voice;

/// Whether a voice survived a render segment.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum VoiceStatus {
    /// Still sounding; keep it in the pool.
    Sounding,
    /// Reached the end of a non-looping sample, or was asked to stop. The pool releases
    /// it and its handle becomes stale.
    Finished,
}

/// Render one voice into `destination`, adding to whatever is already there.
///
/// `pcm` is the module's whole PCM blob; the voice's [`SampleRegion`](crate::sample::SampleRegion)
/// selects its slice of it.
pub fn accumulate_voice<Path: MixPath, Interp: Interpolate>(
    voice: &mut Voice,
    pcm: &[i16],
    destination: &mut [Path::Accumulator],
) -> VoiceStatus {
    if voice.wants_stop() {
        return VoiceStatus::Finished;
    }
    let Some(sample) = SampleData::resolve(pcm, voice.region()) else {
        return VoiceStatus::Finished;
    };

    // Gains are read once per segment; see `path` for why that is still sample-exact.
    let gains = Path::gains(voice.params.volume, voice.params.pan);
    voice.params.clear_dirty();

    let frames = sample.frames();
    let length_bits = (sample.length_frames() as u64) << 32;
    let step = voice.params.step.to_bits();
    let mut position = voice.position();
    let mut status = VoiceStatus::Sounding;

    for accumulator in destination.iter_mut() {
        if position >= length_bits {
            status = VoiceStatus::Finished;
            break;
        }
        Path::mix::<Interp>(accumulator, frames, (position >> 32) as usize, position as u32, gains);
        position = position.wrapping_add(step);

        if let Some(loop_span) = sample.loop_span() {
            let end_bits = (loop_span.end() as u64) << 32;
            if position >= end_bits {
                // One modulo rather than a `while`, so a step longer than the loop — a
                // three-frame loop played an octave up, say — costs the same as any
                // other frame and cannot spin.
                let start_bits = (loop_span.start() as u64) << 32;
                let loop_length_bits = (loop_span.length() as u64) << 32;
                position = start_bits + (position - start_bits) % loop_length_bits;
            }
        }
    }

    voice.set_position(position);
    status
}
