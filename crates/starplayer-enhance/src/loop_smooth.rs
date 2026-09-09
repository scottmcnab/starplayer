//! [`LoopSmoother`] — takes the click out of a forward loop's wrap without moving the
//! sample's rate or its loop points.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use starplayer_model::{EnhancedPcm, LoopMode, SampleEnhancer, SamplePcm};

/// Smooth the seam of a forward loop.
///
/// # What counts as a click
///
/// The repo's own definition, from
/// `crates/starplayer-engine/tests/mixer_determinism.rs`'s
/// `a_forward_loop_does_not_click_at_the_wrap`: the sample-to-sample step **at the wrap**
/// must not exceed the largest step anywhere inside the loop. A loop that already passes
/// it is returned **bit-identical** — a sample that does not click must not be touched,
/// because touching it is a change a listener can hear and a golden can see.
///
/// # The two repairs
///
/// `n = min(crossfade_frames, len / 2)`, where `len` is the loop's length.
///
/// * **`loop_start >= n`** — an equal-power crossfade of the loop's last `n` frames
///   towards the `n` frames that sit *before* `loop_start` in the sample. Those frames are
///   what originally led into `x[loop_start]`, so after the fade the frame the wrap lands
///   on continues what the tail was heading for. The weights come from
///   `starplayer_dsp::equal_power_q15`, so the sum of squares is constant across the fade
///   and the loop does not dip in level at the seam.
/// * **`loop_start < n`** — the common MOD case, where the loop starts at or near frame 0
///   and there is nothing before it to fade towards. A **linear correction ramp** over the
///   last `n` frames instead: `x[end − 1]` is brought all the way to `x[loop_start]`, and
///   the correction is spread linearly backwards over the `n` frames so no new step is
///   introduced inside the loop.
///
/// Neither repair changes the rate, the length or the loop points, so a smoothed sample
/// carries the same `rate_scale_log2` it arrived with.
///
/// # What it declines
///
/// Ping-pong loops are continuous at both turns by construction — the traversal reverses
/// *on* a real frame rather than jumping — so there is nothing to smooth and they are
/// returned untouched. A sample with a sustain loop is declined too: two loops share one
/// body, and a repair aimed at one seam would rewrite frames the other plays.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct LoopSmoother {
    /// Frames to fade or ramp over. Clamped to half the loop's length.
    pub crossfade_frames: u32,
}

impl LoopSmoother {
    /// A smoother that fades over at most `crossfade_frames` frames.
    pub const fn new(crossfade_frames: u32) -> LoopSmoother { LoopSmoother { crossfade_frames } }
}

impl SampleEnhancer for LoopSmoother {
    fn name(&self) -> String {
        let mut name = String::new();
        let _ = write!(&mut name, "loop={}", self.crossfade_frames);
        name
    }

    fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm {
        let unchanged = || EnhancedPcm::unchanged(sample);
        if sample.loop_mode != LoopMode::Forward || sample.sustain_loop.is_some() {
            return unchanged();
        }
        let (start, end) = (sample.loop_start as usize, sample.loop_end as usize);
        if end > sample.frames.len() || start >= end {
            return unchanged();
        }
        let length = end - start;
        let fade = (self.crossfade_frames as usize).min(length / 2);
        if fade == 0 || !seam_clicks(sample.frames, start, end) {
            return unchanged();
        }

        let mut frames = Vec::from(sample.frames);
        match start >= fade {
            true => crossfade_tail(&mut frames, start, end, fade),
            false => ramp_out_the_step(&mut frames, start, end, fade),
        }
        EnhancedPcm { frames, ..unchanged() }
    }
}

/// Whether the wrap step is bigger than the largest step inside the loop — the criterion
/// `mixer_determinism.rs` states, applied to the source frames rather than to a render.
fn seam_clicks(frames: &[i16], start: usize, end: usize) -> bool {
    let Some(first) = frames.get(start).copied() else { return false };
    let Some(last) = frames.get(end - 1).copied() else { return false };
    let wrap_step = (first as i32 - last as i32).abs();
    let mut largest_inside = 0i32;
    for index in start..end - 1 {
        let (Some(current), Some(next)) = (frames.get(index).copied(), frames.get(index + 1).copied()) else { continue };
        largest_inside = largest_inside.max((next as i32 - current as i32).abs());
    }
    wrap_step > largest_inside
}

/// Equal-power crossfade of the loop's last `fade` frames towards the `fade` frames before
/// `loop_start`.
fn crossfade_tail(frames: &mut [i16], start: usize, end: usize, fade: usize) {
    let incoming: Vec<i16> = (0..fade).map(|step| frames.get(start - fade + step).copied().unwrap_or(0)).collect();
    for step in 0..fade {
        let mix_percent = match fade {
            1 => 100,
            _ => (step as i32 * 100) / (fade as i32 - 1),
        };
        let (dry, wet) = starplayer_dsp::equal_power_q15(mix_percent);
        let Some(target) = frames.get_mut(end - fade + step) else { continue };
        let tail = *target as i32;
        let lead_in = incoming.get(step).copied().unwrap_or(0) as i32;
        let mixed = (tail * dry + lead_in * wet + (1 << 14)) >> 15;
        *target = mixed.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}

/// Spread the wrap's DC step linearly back over the loop's last `fade` frames, so that
/// `x[end − 1]` meets `x[loop_start]` and no new step appears inside the loop.
fn ramp_out_the_step(frames: &mut [i16], start: usize, end: usize, fade: usize) {
    let first = frames.get(start).copied().unwrap_or(0) as i32;
    let last = frames.get(end - 1).copied().unwrap_or(0) as i32;
    let step = first - last;
    for index in 0..fade {
        // `(index + 1) / fade` of the correction, rounded to nearest: the last frame gets
        // all of it, so the seam closes exactly.
        let numerator = step * (index as i32 + 1);
        let correction = match numerator >= 0 {
            true => (numerator + fade as i32 / 2) / fade as i32,
            false => -((-numerator + fade as i32 / 2) / fade as i32),
        };
        let Some(target) = frames.get_mut(end - fade + index) else { continue };
        *target = (*target as i32 + correction).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer_model::{DEFAULT_REFERENCE_RATE_HZ, SustainLoop};

    fn looping(frames: &[i16], loop_start: u32, loop_end: u32) -> SamplePcm<'_> {
        SamplePcm {
            frames,
            rate_hz: DEFAULT_REFERENCE_RATE_HZ,
            relative_note: 0,
            finetune: 0,
            loop_mode: LoopMode::Forward,
            loop_start,
            loop_end,
            sustain_loop: None,
        }
    }

    /// A ramp with a big jump back to its start: the textbook loop click.
    fn stepped_loop() -> std::vec::Vec<i16> { (0..256).map(|index| index as i16 * 40).collect() }

    fn wrap_step(frames: &[i16], start: usize, end: usize) -> i32 {
        (frames[start] as i32 - frames[end - 1] as i32).abs()
    }

    fn largest_internal_step(frames: &[i16], start: usize, end: usize) -> i32 {
        (start..end - 1).map(|index| (frames[index + 1] as i32 - frames[index] as i32).abs()).max().unwrap_or(0)
    }

    #[test]
    fn a_ten_thousand_unit_dc_step_passes_the_click_criterion_after_smoothing() {
        // A ramp that starts at frame 0, so there is nothing before the loop to fade
        // towards: the correction-ramp branch. Steps of 100 inside, 10000 at the wrap.
        let frames: std::vec::Vec<i16> = (0..101).map(|index| index as i16 * 100).collect();
        assert_eq!(wrap_step(&frames, 0, 101), 10_000);
        assert_eq!(largest_internal_step(&frames, 0, 101), 100, "the fixture has to click to begin with");

        let smoothed = LoopSmoother::new(64).enhance(looping(&frames, 0, 101));
        assert_eq!(smoothed.frames.len(), frames.len(), "smoothing changes no length");
        assert_eq!(smoothed.rate_hz, DEFAULT_REFERENCE_RATE_HZ, "and no rate");
        assert_eq!((smoothed.loop_start, smoothed.loop_end), (0, 101), "and no loop point");
        assert_eq!(smoothed.frames.get(..51), frames.get(..51), "only the last half of the loop moves");
        assert!(
            wrap_step(&smoothed.frames, 0, 101) <= largest_internal_step(&smoothed.frames, 0, 101),
            "the wrap step is {} against a largest internal step of {}",
            wrap_step(&smoothed.frames, 0, 101),
            largest_internal_step(&smoothed.frames, 0, 101)
        );
    }

    #[test]
    fn a_loop_with_room_before_it_takes_the_crossfade_branch_and_closes_its_seam() {
        let frames = stepped_loop();
        assert!(wrap_step(&frames, 64, 256) > largest_internal_step(&frames, 64, 256), "the fixture has to click");

        let smoothed = LoopSmoother::new(32).enhance(looping(&frames, 64, 256));
        assert_ne!(smoothed.frames, frames, "the crossfade branch has to change something");
        assert_eq!(smoothed.frames.get(..224), frames.get(..224), "only the last 32 frames move");
        assert!(
            wrap_step(&smoothed.frames, 64, 256) <= largest_internal_step(&smoothed.frames, 64, 256),
            "the crossfade left a click: wrap {} against {}",
            wrap_step(&smoothed.frames, 64, 256),
            largest_internal_step(&smoothed.frames, 64, 256)
        );
    }

    #[test]
    fn an_already_continuous_loop_is_bit_identical() {
        // A whole number of cycles of a triangle: the wrap step is one ordinary step.
        let frames: std::vec::Vec<i16> = (0..256).map(|index| ((index % 32) as i16 - 16) * 500).collect();
        let smoothed = LoopSmoother::new(64).enhance(looping(&frames, 0, 256));
        assert_eq!(smoothed.frames, frames, "a loop that does not click must not be touched at all");
    }

    #[test]
    fn a_ping_pong_loop_and_a_sustain_loop_are_both_declined() {
        let frames = stepped_loop();
        let bouncing = SamplePcm { loop_mode: LoopMode::PingPong, ..looping(&frames, 0, 256) };
        assert_eq!(LoopSmoother::new(64).enhance(bouncing).frames, frames, "a ping-pong turn is continuous already");

        let sustained = SamplePcm { sustain_loop: Some(SustainLoop { mode: LoopMode::Forward, start: 4, end: 20 }), ..looping(&frames, 0, 256) };
        assert_eq!(LoopSmoother::new(64).enhance(sustained).frames, frames, "two loops share one body");
    }

    #[test]
    fn a_zero_crossfade_is_a_no_op() {
        let frames = stepped_loop();
        assert_eq!(LoopSmoother::new(0).enhance(looping(&frames, 0, 256)).frames, frames);
    }

    #[test]
    fn the_fade_is_clamped_to_half_the_loop() {
        // An eight-frame loop with a 64-frame request: the fade must not reach outside it.
        let frames: std::vec::Vec<i16> = std::vec![0, 1_000, 2_000, 3_000, 4_000, 5_000, 6_000, 20_000];
        let smoothed = LoopSmoother::new(64).enhance(looping(&frames, 0, 8));
        assert_eq!(smoothed.frames.get(..4), frames.get(..4), "the first half of the loop is untouched");
        assert_eq!(smoothed.frames.len(), 8);
    }

    #[test]
    fn the_name_carries_the_crossfade_length() {
        assert_eq!(LoopSmoother::new(64).name(), "loop=64");
        assert_eq!(LoopSmoother::new(8).name(), "loop=8");
    }
}
