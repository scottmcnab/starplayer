//! The voice render kernel: one function, monomorphised over the mixing path and the
//! interpolator, that turns one voice's sample data into accumulated output frames.
//!
//! # Bounded runs, after `Mixer_8bitMono`
//!
//! The kernel never tests for the loop point inside the inner loop. It works out how many
//! output frames it can produce before the position reaches the next boundary, mixes
//! exactly that many with no checks at all, then handles the boundary once and goes round
//! again:
//!
//! ```text
//! while frames remain:
//!     run = min(frames remaining, frames before the boundary, frames of ramp left)
//!     mix `run` frames
//!     handle the boundary: wrap, reflect, or end the voice
//! ```
//!
//! That is the shape of the original's mixer — `SB_IRQ_Handler` slices the DMA buffer at
//! tick boundaries and `Mixer_8bitMono` mixes a bounded count
//! (`plans/reference/original-s3mlib-analysis.md` §7) — and it is worth copying for the
//! reason it was written that way: loop-wrap arithmetic done per frame is the classic
//! source of off-by-one clicks, and done once per run it is one expression that can be
//! read and tested on its own.
//!
//! The ramp is a third boundary, which is what keeps the two inner loops honest: the
//! per-frame gain loop is entered only while a ramp is live, and the run ends on the frame
//! the ramp lands.
//!
//! # What makes this block-size independent
//!
//! Everything the kernel reads is either a property of the voice (position, direction,
//! step, ramps) or of the sample. Nothing depends on `destination.len()`: the run length
//! bounds *where the checks happen*, never what any frame is worth. Splitting a run of
//! frames into two calls therefore produces exactly the byte sequence one call would have,
//! which is the invariant the block-size determinism test exists to protect.
//!
//! # IT's resonant filter, and why it costs the other formats nothing
//!
//! The filter is a *voice*-level effect that runs between the resampler and the pan gains
//! (architecture §7.2), so it lives inside [`mix_run`] behind a `const FILTERED: bool`
//! rather than anywhere downstream. With `FILTERED` false the body is textually the code
//! that was there before the filter existed — which is what `cargo xtask goldens --check`
//! proves — and a MOD, S3M or MTM voice reaches it through one comparison of
//! `FilterParams` against `FilterParams::BYPASS`, taken once per voice per segment.
//!
//! # Real-time safety
//!
//! No allocation, no locks, no panic and no `dyn` call. Every slice access goes through
//! `get` or a checked split, and a sample region that does not resolve ends the voice
//! rather than faulting — `render()` outputs silence rather than panicking
//! (architecture §8).

use starplayer_dsp::Interpolate;

use crate::path::{MixPath, Stereo};
use crate::sample::{LoopMode, LoopSpan, PRE_ROLL_FRAMES, SampleData};
use crate::voice::Voice;

/// Whether a voice survived a render segment.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum VoiceStatus {
    /// Still sounding; keep it in the pool.
    Sounding,
    /// Reached the end of a non-looping sample, or finished ramping out after a stop. The
    /// pool releases it and its handle becomes stale.
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
    sample_rate_hz: u32,
) -> VoiceStatus {
    let Some(mut sample) = SampleData::resolve(pcm, voice.region()) else {
        return VoiceStatus::Finished;
    };

    // Consume the dirty bits and point the gain ramps wherever they now have to go. A
    // voice asked to stop heads for silence rather than being cut here; it is released
    // when the ramp lands, at the bottom of the run loop.
    if voice.retarget_gains() == VoiceStatus::Finished {
        return VoiceStatus::Finished;
    }

    // The one filter decision, taken once per voice per segment rather than per frame:
    // the coefficients are recomputed only if the parameters or the sample rate moved,
    // and `filtered` then selects between two separately compiled inner loops. For MOD,
    // S3M and MTM the parameters are `FilterParams::BYPASS` from the moment the voice is
    // allocated, so this costs one four-byte comparison and nothing else.
    let filter_params = voice.params.filter;
    let filtered = {
        let (_, filter) = voice.run_state_mut();
        filter.refresh::<Path>(filter_params, sample_rate_hz);
        filter.is_active()
    };

    let step = voice.params.step.to_bits();
    // How many frames before the interpolation point this kernel reads. It is zero for
    // `Nearest` and `Linear`, which is why everything below is the code those two have
    // always executed; for the wide kernels it defers a forward loop's wrap by that many
    // frames, so the frames *behind* the interpolation point are always real ones the
    // voice has just played. See `starplayer_core::PRE_ROLL_FRAMES`.
    let leading = Interp::LEADING_FRAMES as u32;
    let mut reverse = voice.is_reversed();
    let mut remaining: &mut [Path::Accumulator] = destination;

    // The entry position is normalised once — a trigger may have been given an offset past
    // the loop, and a previous segment may have left the voice at the very end of a
    // one-shot.
    let mut position = voice.position();
    let mut ended = match cross_boundary(voice, pcm, &mut sample, position as i128, &mut reverse, leading) {
        Some(normalised) => {
            position = normalised;
            false
        }
        // Left where it was: a voice that has already run off the end must not be handed
        // back a position it could play again.
        None => true,
    };
    let mut finished_ramp = false;

    while !ended && !finished_ramp && !remaining.is_empty() {
        // ── how long the next run is ────────────────────────────────────────────────
        let limit = run_limit(sample, reverse, leading);
        let ramp_frames = voice.gain_ramp_frames_remaining();
        let run = frames_before_limit(position, step, limit, reverse)
            .min(if ramp_frames == 0 { u64::MAX } else { ramp_frames as u64 })
            .clamp(1, remaining.len() as u64) as usize;

        let Some((window, rest)) = remaining.split_at_mut_checked(run) else { break };
        remaining = rest;

        // ── the run itself ──────────────────────────────────────────────────────────
        let frames = sample.stored();
        let (gains, filter) = voice.run_state_mut();
        let path_filter = Path::path_filter(filter);
        let coefficients = path_filter.coefficients;
        let state = &mut path_filter.state;
        match (filtered, ramp_frames > 0, reverse) {
            (false, false, false) => mix_run::<Path, Interp, false, false, false>(window, frames, position, step, gains, state, &coefficients),
            (false, false, true) => mix_run::<Path, Interp, false, false, true>(window, frames, position, step, gains, state, &coefficients),
            (false, true, false) => mix_run::<Path, Interp, false, true, false>(window, frames, position, step, gains, state, &coefficients),
            (false, true, true) => mix_run::<Path, Interp, false, true, true>(window, frames, position, step, gains, state, &coefficients),
            (true, false, false) => mix_run::<Path, Interp, true, false, false>(window, frames, position, step, gains, state, &coefficients),
            (true, false, true) => mix_run::<Path, Interp, true, false, true>(window, frames, position, step, gains, state, &coefficients),
            (true, true, false) => mix_run::<Path, Interp, true, true, false>(window, frames, position, step, gains, state, &coefficients),
            (true, true, true) => mix_run::<Path, Interp, true, true, true>(window, frames, position, step, gains, state, &coefficients),
        }

        // ── the boundary, handled once per run ──────────────────────────────────────
        //
        // The end position is recomputed widened rather than taken from the run, because a
        // reverse run's last decrement can take it below zero and a `u64` has nowhere to
        // put that. It is normalised here rather than at the top of the next iteration so
        // that the position written back to the voice is always a real one.
        let travelled = run as i128 * step as i128;
        let advanced = if reverse { position as i128 - travelled } else { position as i128 + travelled };
        match cross_boundary(voice, pcm, &mut sample, advanced, &mut reverse, leading) {
            Some(normalised) => position = normalised,
            None => {
                position = clamp_position(advanced);
                ended = true;
            }
        }

        finished_ramp = voice.finished_ramping_out();
    }

    voice.set_position(position);
    voice.set_reversed(reverse);
    if ended || finished_ramp { VoiceStatus::Finished } else { VoiceStatus::Sounding }
}

/// Handle whatever the voice has run into: a queued ProTracker sample swap first, then
/// the ordinary loop wrap or one-shot end.
///
/// `None` means the voice is over. On a swap `sample` is repointed at the new region, so
/// the run loop reads the replacement's frames from the next run onwards.
fn cross_boundary<'pcm>(
    voice: &mut Voice,
    pcm: &'pcm [i16],
    sample: &mut SampleData<'pcm>,
    position: i128,
    reverse: &mut bool,
    leading: u32,
) -> Option<u64> {
    if let Some(rebased) = take_queued_region(voice, pcm, sample, position) {
        *reverse = false;
        return rebased;
    }
    normalise_position(position, reverse, *sample, leading)
}

/// Apply a queued region if `position` has reached the boundary that releases it.
///
/// The outer `Option` says whether a swap happened at all; the inner one says where the
/// replacement starts, with `None` for ProTracker's null sample, whose zero-length region
/// ends the voice at exactly that boundary.
///
/// Only a forward loop's wrap and a one-shot's end release a swap. A ping-pong loop
/// cannot: no ProTracker-family format has one, and Paula has no equivalent of the turn.
fn take_queued_region<'pcm>(voice: &mut Voice, pcm: &'pcm [i16], sample: &mut SampleData<'pcm>, position: i128) -> Option<Option<u64>> {
    let reached = match sample.loop_span() {
        None => position >= frames_to_bits(sample.length_frames()) as i128,
        Some(span) if span.mode() == LoopMode::Forward => position >= frames_to_bits(span.end()) as i128,
        Some(_) => false,
    };
    if !reached || voice.pending_region().is_none() {
        return None;
    }
    let region = voice.take_pending_region()?;
    let Some(replacement) = SampleData::resolve(pcm, region) else { return Some(None) };
    // ProTracker's null sample, and a one-shot replacing a one-shot, both leave Paula
    // with nothing to reload: the channel falls silent at the boundary instead of
    // swapping (OpenMPT `PTStoppedSwap.mod`, `PTSwapNoLoop.mod`; libxmp `mixer.c:726`).
    if region.length_frames() == 0 || (sample.loop_span().is_none() && region.loop_span().is_none()) {
        return Some(None);
    }
    voice.adopt_region(region);
    *sample = replacement;
    // Paula reloads its pointer from the loop registers, so a looping replacement starts
    // at its loop point and a one-shot starts at its first frame.
    Some(Some(frames_to_bits(region.loop_span().map(LoopSpan::start).unwrap_or(0))))
}

/// The frame a voice sitting at `position` would read, folded through `sample`'s loop
/// exactly as the render kernel folds it. `None` once a one-shot has run past its end.
///
/// # Why this is here rather than in the caller
///
/// The scope taps (M3-D6, architecture §9(b)) read one PCM frame per tap bucket at a
/// position the voice has not reached yet, which may be past a loop end. There is exactly
/// one right answer to "where does that land", and it is [`normalise_position`] — the
/// same fold the run loop uses, including a ping-pong loop's turn. Exposing it is how the
/// tap avoids writing a second loop rule that would drift out of step with this one.
///
/// `reverse` is the voice's current direction; a ping-pong loop's fold may turn it round,
/// and the tap deliberately does not care where it ends up, because it does not advance
/// the voice. Nothing here mutates anything: this is a pure read.
pub fn folded_frame(sample: SampleData<'_>, position: i128, reverse: bool) -> Option<i16> {
    let mut reverse = reverse;
    // No leading frames: the tap reads *one* frame rather than resampling, so it has no
    // taps to keep behind it and wants the wrap where the sample says it is. A wide
    // kernel's deferred wrap moves the fold by at most `LEADING_FRAMES` frames, and the
    // guard frames it reads there are a copy of the loop, so the tap sees the same value
    // either way.
    let normalised = normalise_position(position, &mut reverse, sample, 0)?;
    sample.frames().get((normalised >> 32) as usize).copied()
}

/// Bring a position back inside the sample, and say which way a ping-pong loop is now
/// travelling. `None` means a one-shot has run out and the voice is over.
fn normalise_position(position: i128, reverse: &mut bool, sample: SampleData<'_>, leading: u32) -> Option<u64> {
    match sample.loop_span() {
        None => {
            if position >= frames_to_bits(sample.length_frames()) as i128 {
                None
            } else {
                Some(clamp_position(position))
            }
        }
        Some(span) if span.mode() == LoopMode::Forward => {
            if position >= forward_wrap_bits(span, leading) as i128 {
                Some(wrap_forward(position, span))
            } else {
                Some(clamp_position(position))
            }
        }
        Some(span) => {
            let turn = ping_pong_turn_bits(span) as i128;
            let start = frames_to_bits(span.start()) as i128;
            // Below `start` while going *forwards* is the run-in before the loop, which a
            // ping-pong sample plays exactly like any other; below it while going
            // backwards is the bottom turn. Folding the first would swallow the run-in.
            if position > turn || (*reverse && position < start) {
                let (folded, folded_reverse) = fold_ping_pong(position, *reverse, span);
                *reverse = folded_reverse;
                Some(folded)
            } else {
                Some(clamp_position(position))
            }
        }
    }
}

/// The position a run must stop at: the first position out of range going forwards, the
/// last one still in range going backwards.
fn run_limit(sample: SampleData<'_>, reverse: bool, leading: u32) -> u64 {
    match sample.loop_span() {
        None => frames_to_bits(sample.length_frames()),
        Some(span) if span.mode() == LoopMode::Forward => forward_wrap_bits(span, leading),
        Some(span) => {
            if reverse {
                frames_to_bits(span.start())
            } else {
                // The turning frame itself is played, so the run ends one Q32.32 tick past
                // it.
                ping_pong_turn_bits(span).saturating_add(1)
            }
        }
    }
}

/// One bounded run of frames, with no boundary test and no `dyn` call in it.
///
/// `FILTERED`, `RAMPING` and `REVERSE` are `const` parameters rather than locals so that
/// the eight shapes of this loop are eight separate bodies: the steady forward unfiltered
/// case — the one that runs 99.9% of the time, and every frame of every MOD, S3M and MTM
/// — has no direction test, no gain update and no filter in it at all. That is what keeps
/// `cargo xtask goldens --check` byte-identical across this change: with `FILTERED` false
/// the body is the code that was here before, one expression at a time.
fn mix_run<Path: MixPath, Interp: Interpolate, const FILTERED: bool, const RAMPING: bool, const REVERSE: bool>(
    destination: &mut [Path::Accumulator],
    frames: &[i16],
    start_position: u64,
    step: u64,
    gains: &mut Stereo<starplayer_dsp::GainRamp>,
    filter_state: &mut [Path::Mono; 2],
    coefficients: &starplayer_dsp::FilterCoefficients<Path::Mono>,
) {
    let mut position = start_position;
    let steady = Stereo::new(Path::gain(gains.left.current()), Path::gain(gains.right.current()));

    for accumulator in destination.iter_mut() {
        let frame_gains = if RAMPING {
            Stereo::new(Path::gain(gains.left.advance()), Path::gain(gains.right.advance()))
        } else {
            steady
        };
        // `frames` is the stored run, so frame 0 sits at index `PRE_ROLL_FRAMES` and a
        // kernel reading `index - LEADING_FRAMES` stays inside the slice. The addition is
        // loop-invariant and folds into the addressing mode.
        let value = Path::interpolate::<Interp>(frames, ((position >> 32) as usize).wrapping_add(PRE_ROLL_FRAMES), position as u32);
        // Between the resampler and the pan gains, which is where IT puts it: OpenMPT's
        // `SampleLoop` runs `interpolate(); filter(); mix();` in that order.
        let value = if FILTERED { Path::filter(value, filter_state, coefficients) } else { value };
        Path::accumulate(accumulator, value, frame_gains);
        position = if REVERSE { position.wrapping_sub(step) } else { position.wrapping_add(step) };
    }
}

/// A whole frame count as a Q32.32 position.
const fn frames_to_bits(frames: u32) -> u64 { (frames as u64) << 32 }

/// Back into `u64` after the boundary arithmetic, which is done widened.
const fn clamp_position(position: i128) -> u64 {
    if position < 0 {
        0
    } else if position > u64::MAX as i128 {
        u64::MAX
    } else {
        position as u64
    }
}

/// How many frames can be produced from `position` before it leaves the run's side of
/// `limit`.
///
/// Forward, `limit` is the first position that is *out* of range and the answer is
/// `ceil((limit - position) / step)`; reverse, `limit` is the lowest position still *in*
/// range and the answer is `floor((position - limit) / step) + 1`. A zero step never
/// reaches either, which is a stalled voice rather than an error: it mixes its current
/// frame for as long as it is asked to.
const fn frames_before_limit(position: u64, step: u64, limit: u64, reverse: bool) -> u64 {
    if step == 0 {
        return u64::MAX;
    }
    if reverse {
        if position < limit {
            0
        } else {
            ((position - limit) / step).saturating_add(1)
        }
    } else if position >= limit {
        0
    } else {
        ((limit - position) as u128).div_ceil(step as u128) as u64
    }
}

/// The position at which a forward loop wraps, which is `loop_end` **plus the kernel's
/// leading frames**.
///
/// Deferring the wrap is how a symmetric kernel gets the frames it needs *behind* the
/// interpolation point without a second copy of them anywhere: the trailing guard already
/// holds the loop's continuation, so a position `t < LEADING_FRAMES` frames past
/// `loop_end` reads exactly the frames position `loop_start + t` would have, while the
/// taps behind it are the real frames just before `loop_end`. The output is therefore the
/// same sequence an unrolled loop produces, frame for frame, and the wrap lands
/// `LEADING_FRAMES` frames further round the loop than it used to — `loop_start + t`
/// rather than `loop_start`.
///
/// `Nearest` and `Linear` have no leading frames, so this is `loop_end` for them and the
/// canonical goldens do not move.
fn forward_wrap_bits(span: LoopSpan, leading: u32) -> u64 {
    frames_to_bits(span.end()).saturating_add(frames_to_bits(leading))
}

/// Bring a position that has run past `loop_end` back into a forward loop.
///
/// One modulo rather than a `while`, so a step longer than the loop — a three-frame loop
/// played an octave up, say — costs the same as any other wrap and cannot spin.
fn wrap_forward(position: i128, span: LoopSpan) -> u64 {
    let start = frames_to_bits(span.start()) as i128;
    let length = frames_to_bits(span.length()) as i128;
    clamp_position(start + (position - start).rem_euclid(length))
}

/// The last position a ping-pong loop plays going forwards: the turning frame `end - 1`,
/// which is a real frame and is played exactly once per turn.
fn ping_pong_turn_bits(span: LoopSpan) -> u64 { frames_to_bits(span.end()).saturating_sub(frames_to_bits(1)) }

/// Bring a position that has left a ping-pong loop back into it, and say which way it is
/// now travelling.
///
/// The fold is a triangle wave between the two turning frames `start` and `end - 1`,
/// evaluated in one modulo so that an arbitrarily long step lands correctly rather than
/// being reflected repeatedly. `travelled` is the distance around that triangle: its first
/// half runs forwards from `start`, its second half runs back from `end - 1`.
///
/// **Both turning points are real frames**, which is the whole reason the traversal is
/// defined this way (see [`LoopSpan::ping_pong_frame`]): the position never leaves
/// `start ..= end-1`, so the mixer reads the same addressable frames a forward loop does
/// and needs no leading guard. A one-frame loop has no triangle at all and holds its
/// frame.
fn fold_ping_pong(position: i128, reverse: bool, span: LoopSpan) -> (u64, bool) {
    let start = frames_to_bits(span.start()) as i128;
    let half_period = ping_pong_turn_bits(span) as i128 - start;
    if half_period <= 0 {
        return (clamp_position(start), false);
    }
    let period = 2 * half_period;

    let travelled = if reverse { period - (position - start) } else { position - start };
    let phase = travelled.rem_euclid(period);
    if phase <= half_period {
        (clamp_position(start + phase), false)
    } else {
        (clamp_position(start + period - phase), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use starplayer_core::{Step, U0F16, VoiceParams};
    use starplayer_dsp::{Linear, Nearest};

    use crate::gain::RAMP_FRAMES;
    use crate::path::{FixedFrame, FixedPath};
    use crate::sample::{SampleRegion, append_guarded_sample};
    use crate::voice::VoiceTag;

    /// A voice at full volume, hard left, so the left channel carries the source unscaled
    /// by the pan law and the arithmetic under test is visible in the output.
    fn voice_for(region: SampleRegion, step: Step) -> Voice {
        let params = VoiceParams { step, volume: U0F16::MAX, pan: starplayer_core::I1F15::MIN, ..VoiceParams::SILENT };
        let mut voice = Voice::new(VoiceTag::default(), region, params, 0);
        voice.settle_gains();
        voice
    }

    fn blob_with(pcm: &[i16], loop_span: Option<LoopSpan>) -> (Vec<i16>, SampleRegion) {
        let mut blob = Vec::new();
        let region = append_guarded_sample(&mut blob, pcm, loop_span);
        (blob, region)
    }

    #[test]
    fn a_run_bounded_forward_counts_the_frames_before_the_limit() {
        let one = 1u64 << 32;
        assert_eq!(frames_before_limit(0, one, 4 * one, false), 4, "0,1,2,3 then the limit");
        assert_eq!(frames_before_limit(one / 2, one, 4 * one, false), 4, "0.5,1.5,2.5,3.5");
        assert_eq!(frames_before_limit(3 * one, one, 4 * one, false), 1);
        assert_eq!(frames_before_limit(4 * one, one, 4 * one, false), 0, "already past it");
        assert_eq!(frames_before_limit(0, 8 * one, 4 * one, false), 1, "a step longer than the sample");
        assert_eq!(frames_before_limit(0, 0, 4 * one, false), u64::MAX, "a stalled voice never arrives");
    }

    #[test]
    fn a_run_bounded_backward_counts_the_frames_before_the_limit() {
        let one = 1u64 << 32;
        assert_eq!(frames_before_limit(4 * one, one, 4 * one, true), 1, "the limit frame itself still plays");
        assert_eq!(frames_before_limit(4 * one, one, 0, true), 5, "4,3,2,1,0");
        assert_eq!(frames_before_limit(one / 2, one, 0, true), 1);
        assert_eq!(frames_before_limit(0, 8 * one, 4 * one, true), 0);
    }

    #[test]
    fn the_ping_pong_fold_reflects_about_both_turning_frames() {
        let one = 1u64 << 32;
        let span = LoopSpan::ping_pong(4, 8).expect("a valid span");
        // The turning frames are 4 and 7; a quarter of a frame past 7 reflects to a
        // quarter of a frame below it.
        let (position, reverse) = fold_ping_pong((7 * one + one / 4) as i128, false, span);
        assert_eq!((position, reverse), (7 * one - one / 4, true));
        // And a quarter of a frame below 4 reflects back up.
        let (position, reverse) = fold_ping_pong((4 * one - one / 4) as i128, true, span);
        assert_eq!((position, reverse), (4 * one + one / 4, false));
        // A step of many loop lengths lands in one operation: the period is 2 x 3 frames,
        // so 25 frames past the top is 25 mod 6 = 1 frame into the triangle from the top.
        let (position, reverse) = fold_ping_pong((7 * one + 25 * one) as i128, false, span);
        assert_eq!((position, reverse), (6 * one, true));
        // A one-frame loop holds its frame whatever it is asked.
        let pinned = LoopSpan::ping_pong(4, 5).expect("a valid span");
        assert_eq!(fold_ping_pong((99 * one) as i128, false, pinned), (4 * one, false));
    }

    #[test]
    fn a_forward_wrap_of_many_loop_lengths_lands_in_one_operation() {
        let one = 1u64 << 32;
        let span = LoopSpan::new(4, 8).expect("a valid span");
        assert_eq!(wrap_forward((8 * one) as i128, span), 4 * one);
        assert_eq!(wrap_forward((8 * one + 25 * one) as i128, span), 5 * one, "25 mod 4 = 1 past the loop start");
    }

    /// The guard frames exist so that the inner loop needs no test at the loop point. The
    /// proof is that reading straight through the wrap produces the *continuation of the
    /// loop*: if the guard were missing or short, the interpolator's out-of-range fallback
    /// would return silence and this would be a dip instead.
    #[test]
    fn linear_interpolation_reads_through_the_loop_point_without_a_branch() {
        let pcm: Vec<i16> = (0..8).map(|index| 1_000 + 1_000 * index as i16).collect();
        let (blob, region) = blob_with(&pcm, LoopSpan::new(0, 8));
        // Half a frame per output frame, so every second output frame sits exactly halfway
        // between two source frames — including halfway across the wrap.
        let mut voice = voice_for(region, Step::from_bits(1u64 << 31));
        let mut output = [FixedFrame::default(); 18];
        assert_eq!(accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Sounding);

        // The frame straddling the wrap is halfway between f[7] = 8000 and f[0] = 1000.
        let midpoint = output.get(15).copied().expect("18 frames were rendered");
        let expected = ((8_000 + 1_000) / 2) * 32_766 / 32_768;
        assert!((midpoint.left - expected).abs() <= 2, "the wrap interpolated to {} rather than about {expected}", midpoint.left);
    }

    /// Task B5's verification: no discontinuity at the wrap.
    #[test]
    fn a_forward_loop_has_no_step_at_the_wrap() {
        // A triangle that joins to itself, so the loop is continuous and any step in the
        // output is the mixer's doing rather than the sample's.
        let pcm: Vec<i16> = (0..64).map(|index: i32| ((if index < 32 { index } else { 64 - index }) * 900) as i16).collect();
        let (blob, region) = blob_with(&pcm, LoopSpan::new(0, 64));
        let mut voice = voice_for(region, Step::from_ratio(7, 3));
        let mut output = [FixedFrame::default(); 512];
        accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, 44_100);

        let deltas: Vec<i32> = output.windows(2)
            .filter_map(|pair| match pair {
                [first, second] => Some((second.left - first.left).abs()),
                _ => None,
            })
            .collect();
        // The waveform's own slope is 900 per source frame at 7/3 frames per output frame,
        // so a continuous render steps by about 2100 and never by more than that.
        let largest = deltas.iter().copied().max().unwrap_or(0);
        assert!(largest <= 2_200, "a discontinuity of {largest} appeared somewhere in the loop");
    }

    #[test]
    fn a_ping_pong_loop_turns_round_at_both_ends() {
        let pcm: Vec<i16> = (0..8).map(|index| 1_000 * (index as i16 + 1)).collect();
        let (blob, region) = blob_with(&pcm, LoopSpan::ping_pong(0, 8));
        let mut voice = voice_for(region, Step::ONE);
        let mut output = [FixedFrame::default(); 24];
        accumulate_voice::<FixedPath, Nearest>(&mut voice, &blob, &mut output, 44_100);

        let played: Vec<i32> = output.iter().map(|frame| (frame.left as i64 * 32_768 / 32_766) as i32).collect();
        let expected: Vec<i32> = [
            1, 2, 3, 4, 5, 6, 7, 8, // forwards, turning *on* frame 8
            7, 6, 5, 4, 3, 2, 1,    // and back, turning on frame 1
            2, 3, 4, 5, 6, 7, 8,
            7, 6,
        ].iter().map(|value| value * 1_000).collect();
        for (index, (actual, expected)) in played.iter().zip(expected.iter()).enumerate() {
            assert!((actual - expected).abs() <= 2, "frame {index}: {actual} rather than {expected}");
        }
    }

    /// A ping-pong sample plays its run-in — everything before `loop_start` — forwards,
    /// exactly like any other sample, and only then starts turning round.
    #[test]
    fn a_ping_pong_sample_plays_its_run_in_before_it_starts_turning() {
        let pcm: Vec<i16> = (0..8).map(|index| 1_000 * (index as i16 + 1)).collect();
        let (blob, region) = blob_with(&pcm, LoopSpan::ping_pong(4, 8));
        let mut voice = voice_for(region, Step::ONE);
        let mut output = [FixedFrame::default(); 12];
        accumulate_voice::<FixedPath, Nearest>(&mut voice, &blob, &mut output, 44_100);

        let played: Vec<i32> = output.iter().map(|frame| (frame.left as i64 * 32_768 / 32_766) as i32).collect();
        // Frames 0..7 straight through, then the loop turns on frame 8 and comes back.
        let expected: Vec<i32> = [1, 2, 3, 4, 5, 6, 7, 8, 7, 6, 5, 6].iter().map(|value| value * 1_000).collect();
        for (index, (actual, expected)) in played.iter().zip(expected.iter()).enumerate() {
            assert!((actual - expected).abs() <= 2, "frame {index}: {actual} rather than {expected}");
        }
    }

    #[test]
    fn a_ping_pong_loop_survives_a_step_longer_than_the_loop() {
        let pcm: Vec<i16> = (0..4).map(|index| 1_000 * (index as i16 + 1)).collect();
        let (blob, region) = blob_with(&pcm, LoopSpan::ping_pong(0, 4));
        let mut voice = voice_for(region, Step::from_ratio(13, 1));
        let mut output = [FixedFrame::default(); 32];
        assert_eq!(accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Sounding);
        assert!(output.iter().all(|frame| frame.left.abs() <= 4_100), "every frame stayed inside the sample");
    }

    #[test]
    fn splitting_a_run_anywhere_produces_the_same_frames() {
        let pcm: Vec<i16> = (0..12).map(|index| 500 + 37 * index as i16).collect();
        for span in [LoopSpan::new(3, 12), LoopSpan::ping_pong(3, 12), None] {
            let (blob, region) = blob_with(&pcm, span);
            let render = |chunk: usize| {
                let mut voice = voice_for(region, Step::from_ratio(7, 3));
                let mut output = [FixedFrame::default(); 40];
                let mut written = 0;
                while written < output.len() {
                    let end = (written + chunk).min(output.len());
                    if let Some(window) = output.get_mut(written..end) {
                        accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, window, 44_100);
                    }
                    written = end;
                }
                output
            };
            let whole = render(40);
            for chunk in [1usize, 3, 7, 11, 25] {
                assert_eq!(render(chunk), whole, "{span:?} at chunk length {chunk}");
            }
        }
    }

    #[test]
    fn a_ramp_produces_the_same_frames_however_it_is_split() {
        let pcm: Vec<i16> = (0..32).map(|index| 3_000 + 300 * index as i16).collect();
        let (blob, region) = blob_with(&pcm, LoopSpan::new(0, 32));
        let render = |chunk: usize| {
            let mut voice = voice_for(region, Step::ONE);
            voice.params.set_volume(U0F16::from_bits(8_192));
            let mut output = [FixedFrame::default(); 4 * RAMP_FRAMES as usize];
            let mut written = 0;
            while written < output.len() {
                let end = (written + chunk).min(output.len());
                if let Some(window) = output.get_mut(written..end) {
                    accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, window, 44_100);
                }
                written = end;
            }
            output
        };
        let whole = render(4 * RAMP_FRAMES as usize);
        for chunk in [1usize, 3, 7, 64, 100] {
            assert_eq!(render(chunk), whole, "chunk length {chunk} changed a ramped render");
        }
    }

    #[test]
    fn a_one_shot_ends_when_it_runs_out() {
        let (blob, region) = blob_with(&[100, 200, 300, 400], None);
        let mut voice = voice_for(region, Step::ONE);
        let mut output = [FixedFrame::default(); 3];
        assert_eq!(accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Sounding);
        assert_eq!(accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Finished);
    }

    #[test]
    fn a_degenerate_voice_produces_silence_rather_than_a_fault() {
        // A zero-length sample: nothing to play, so the voice ends on its first frame.
        let (blob, region) = blob_with(&[], None);
        let mut voice = voice_for(region, Step::ONE);
        let mut output = [FixedFrame::default(); 8];
        assert_eq!(accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Finished);
        assert_eq!(output, [FixedFrame::default(); 8]);

        // A zero step: a stalled voice holds its frame rather than spinning or ending.
        let (blob, region) = blob_with(&[1_234, 5_678], None);
        let mut voice = voice_for(region, Step::ZERO);
        let mut output = [FixedFrame::default(); 8];
        assert_eq!(accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Sounding);
        assert!(output.iter().all(|frame| frame.left == output.first().map(|first| first.left).unwrap_or(0)));

        // A step longer than the sample: one frame, then the end.
        let mut voice = voice_for(region, Step::from_ratio(99, 1));
        let mut output = [FixedFrame::default(); 8];
        assert_eq!(accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Finished);

        // An unresolvable region — a corrupt module — ends the voice rather than faulting.
        let mut voice = voice_for(SampleRegion::one_shot(9_999, 64), Step::ONE);
        assert_eq!(accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Finished);
    }

    #[test]
    fn the_maximum_step_neither_overflows_nor_spins() {
        let pcm: Vec<i16> = (0..16).map(|index| 100 * index as i16).collect();
        for span in [LoopSpan::new(2, 16), LoopSpan::ping_pong(2, 16)] {
            let (blob, region) = blob_with(&pcm, span);
            let mut voice = voice_for(region, Step::MAX);
            let mut output = [FixedFrame::default(); 64];
            assert_eq!(accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Sounding);
            assert!(voice.position() < frames_to_bits(16), "the position stayed inside the sample");
        }
    }

    // ── C3b / accuracy policy D12: the voice-boundary sample swap ──────────────────

    #[test]
    fn a_queued_region_replaces_a_looping_sample_at_its_wrap() {
        let old: Vec<i16> = (0..8).map(|index| 1_000 + index as i16).collect();
        let new: Vec<i16> = (0..8).map(|index| 5_000 + index as i16).collect();
        let mut blob = Vec::new();
        let first = append_guarded_sample(&mut blob, &old, LoopSpan::new(0, 8));
        let second = append_guarded_sample(&mut blob, &new, LoopSpan::new(4, 8));

        let mut voice = voice_for(first, Step::ONE);
        voice.queue_region(second);
        let mut output = [FixedFrame::default(); 12];
        assert_eq!(accumulate_voice::<FixedPath, Nearest>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Sounding);
        assert_eq!(voice.region(), second, "the wrap adopted the queued region");
        assert_eq!(voice.pending_region(), None, "and cleared the slot");

        let played: Vec<i32> = output.iter().map(|frame| (frame.left as i64 * 32_768 / 32_766) as i32).collect();
        // Eight frames of the old sample, then the replacement from its loop start.
        let expected = [1_000, 1_001, 1_002, 1_003, 1_004, 1_005, 1_006, 1_007, 5_004, 5_005, 5_006, 5_007];
        for (index, (actual, expected)) in played.iter().zip(expected.iter()).enumerate() {
            assert!((actual - expected).abs() <= 2, "frame {index}: {actual} rather than {expected}");
        }
    }

    #[test]
    fn a_queued_looping_region_takes_over_when_a_one_shot_runs_out() {
        let new: Vec<i16> = (0..8).map(|index| 5_000 + index as i16).collect();
        let mut blob = Vec::new();
        let first = append_guarded_sample(&mut blob, &[1_000, 1_001, 1_002, 1_003], None);
        let second = append_guarded_sample(&mut blob, &new, LoopSpan::new(2, 8));

        let mut voice = voice_for(first, Step::ONE);
        voice.queue_region(second);
        let mut output = [FixedFrame::default(); 8];
        assert_eq!(accumulate_voice::<FixedPath, Nearest>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Sounding, "the one-shot's end is a swap, not an end");
        assert_eq!(voice.region(), second);
        let played: Vec<i32> = output.iter().map(|frame| (frame.left as i64 * 32_768 / 32_766) as i32).collect();
        let expected = [1_000, 1_001, 1_002, 1_003, 5_002, 5_003, 5_004, 5_005];
        for (index, (actual, expected)) in played.iter().zip(expected.iter()).enumerate() {
            assert!((actual - expected).abs() <= 2, "frame {index}: {actual} rather than {expected}");
        }
    }

    #[test]
    fn a_one_shot_queued_behind_a_one_shot_stops_the_voice_and_so_does_the_null_sample() {
        let mut blob = Vec::new();
        let one_shot = append_guarded_sample(&mut blob, &[1_000, 1_001, 1_002, 1_003], None);
        let other_one_shot = append_guarded_sample(&mut blob, &[2_000, 2_001], None);
        let looping = append_guarded_sample(&mut blob, &[3_000, 3_001, 3_002, 3_003], LoopSpan::new(0, 4));

        let mut voice = voice_for(one_shot, Step::ONE);
        voice.queue_region(other_one_shot);
        let mut output = [FixedFrame::default(); 8];
        assert_eq!(accumulate_voice::<FixedPath, Nearest>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Finished, "Paula has no loop registers to reload");

        // The same queue behind a looping voice is ProTracker's null sample only when the
        // replacement is empty.
        let mut voice = voice_for(looping, Step::ONE);
        voice.queue_region(SampleRegion::default());
        assert_eq!(accumulate_voice::<FixedPath, Nearest>(&mut voice, &blob, &mut output, 44_100), VoiceStatus::Finished);
    }

    #[test]
    fn a_retrigger_adopts_a_queued_region_at_once_and_a_set_region_discards_it() {
        let mut blob = Vec::new();
        let first = append_guarded_sample(&mut blob, &[1_000, 1_001, 1_002, 1_003], LoopSpan::new(0, 4));
        let second = append_guarded_sample(&mut blob, &[2_000, 2_001], LoopSpan::new(0, 2));

        let mut voice = voice_for(first, Step::ONE);
        voice.queue_region(second);
        voice.retrigger(0);
        assert_eq!(voice.region(), second, "a position change makes the queued sample take effect now");
        assert_eq!(voice.pending_region(), None);

        let mut voice = voice_for(first, Step::ONE);
        voice.queue_region(second);
        voice.set_region(first);
        assert_eq!(voice.pending_region(), None, "an explicit region write supersedes the queue");
    }

    /// The invariant most at risk from the swap: the boundary is a property of the
    /// voice's position, never of how the run was split.
    #[test]
    fn a_swap_lands_in_the_same_place_however_the_render_is_split() {
        let old: Vec<i16> = (0..12).map(|index| 500 + 37 * index as i16).collect();
        let new: Vec<i16> = (0..12).map(|index| 900 - 21 * index as i16).collect();
        let mut blob = Vec::new();
        let first = append_guarded_sample(&mut blob, &old, LoopSpan::new(3, 12));
        let second = append_guarded_sample(&mut blob, &new, LoopSpan::new(5, 12));

        let render = |chunk: usize| {
            let mut voice = voice_for(first, Step::from_ratio(7, 3));
            voice.queue_region(second);
            let mut output = [FixedFrame::default(); 40];
            let mut written = 0;
            while written < output.len() {
                let end = (written + chunk).min(output.len());
                if let Some(window) = output.get_mut(written..end) {
                    accumulate_voice::<FixedPath, Linear>(&mut voice, &blob, window, 44_100);
                }
                written = end;
            }
            (output, voice.region(), voice.position())
        };
        let whole = render(40);
        for chunk in [1usize, 3, 7, 11, 25] {
            assert_eq!(render(chunk), whole, "chunk length {chunk} moved the swap");
        }
        assert_eq!(whole.1, second, "the swap did happen inside the render");
    }
}
