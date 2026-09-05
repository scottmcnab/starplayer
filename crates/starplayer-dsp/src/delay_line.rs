//! [`DelayLine`] and [`StereoDelayLine`] — the ring buffer every delay-based effect (H3's
//! delay and chorus, H4's reverb) is built from.
//!
//! # Allocation is the control side's job, once
//!
//! [`DelayLine::new`] is the **only** allocating call in this module, and it is the only
//! one anywhere in this crate: architecture §8 forbids allocation inside `render()`, and
//! M7 decision 4 is explicit that an insert's delay lines are built off the audio thread
//! and arrive boxed over the control ring, exactly as `Arc<Module>` does. Everything past
//! construction — [`DelayLine::write`], [`DelayLine::read`], [`DelayLine::read_fractional`]
//! — touches only the buffer this call already allocated.
//!
//! # Power of two, masked, never `%`
//!
//! `capacity` is rounded up to a power of two so the wrap is `& mask` rather than `%
//! capacity`: the same reason `starplayer_mixer::kernel`'s loop points are bit positions
//! rather than frame counts. A delay line's capacity is chosen once at construction time
//! (from a maximum delay in milliseconds), so rounding it up costs a few percent of
//! memory no one will ever measure and buys a mask instead of a division in the one loop
//! every voice's worth of reverb tail runs through.

use alloc::vec;
use alloc::vec::Vec;

use crate::sample::DspSample;

/// A power-of-two ring buffer of samples, read at a delay relative to the write cursor.
///
/// `write` advances the cursor by one frame; `read`/`read_fractional` look backwards from
/// wherever the cursor currently sits, so a caller alternates one `write` with however
/// many `read`s a tap needs before advancing again — the usual "write, then read the
/// taps" order every comb, allpass or delay tap in H3/H4 will use.
#[derive(Clone, Debug)]
pub struct DelayLine<S: DspSample> {
    buffer: Vec<S>,
    /// Index the *next* `write` will land on.
    cursor: usize,
    /// `buffer.len() - 1`; `buffer.len()` is always a power of two.
    mask: usize,
}

impl<S: DspSample> DelayLine<S> {
    /// Allocate a delay line able to hold at least `capacity_frames`, rounded up to a
    /// power of two. The only allocating call in this crate — see the module
    /// documentation.
    pub fn new(capacity_frames: usize) -> DelayLine<S> {
        let capacity = capacity_frames.max(1).next_power_of_two();
        DelayLine { buffer: vec![S::ZERO; capacity], cursor: 0, mask: capacity - 1 }
    }

    /// Frames this delay line can hold — the rounded-up capacity, not the value passed to
    /// [`DelayLine::new`].
    pub fn capacity(&self) -> usize { self.buffer.len() }

    /// Write one frame at the cursor and advance it.
    pub fn write(&mut self, value: S) {
        if let Some(slot) = self.buffer.get_mut(self.cursor) {
            *slot = value;
        }
        self.cursor = (self.cursor + 1) & self.mask;
    }

    /// The frame written `delay_frames` writes ago. `delay_frames == 0` reads the frame
    /// most recently written; a `delay_frames` at or beyond [`DelayLine::capacity`] wraps
    /// rather than reading stale data outside the buffer's own history.
    pub fn read(&self, delay_frames: u32) -> S {
        let index = self.cursor.wrapping_sub(1).wrapping_sub(delay_frames as usize) & self.mask;
        self.buffer.get(index).copied().unwrap_or(S::ZERO)
    }

    /// The frame at a fractional delay, linearly interpolated between the two frames it
    /// falls between. `delay_q16` is Q16.16 frames: `0` is [`DelayLine::read`]`(0)`,
    /// `1 << 16` is one frame further back, and so on.
    pub fn read_fractional(&self, delay_q16: u32) -> S {
        let (current, next, fraction) = self.read_fractional_parts(delay_q16);
        if fraction == 0 {
            return current;
        }
        current.add(scale_by_q16_fraction(next.sub(current), fraction))
    }

    /// The two frames a fractional delay falls between, and the Q0.16 weight between them
    /// — the *gather* half of [`DelayLine::read_fractional`] (M7-H6).
    ///
    /// A caller with several taps to interpolate reads each tap's parts with this and
    /// hands them all to [`DspSample::interpolate_taps`](crate::sample::DspSample::interpolate_taps)
    /// in one call, which is the only part of a fractional read a vector unit can do: the
    /// two loads are at arbitrary distances in a ring buffer and stay scalar whatever the
    /// backend.
    pub fn read_fractional_parts(&self, delay_q16: u32) -> (S, S, i32) {
        let whole_frames = delay_q16 >> 16;
        let fraction = (delay_q16 & 0xFFFF) as i32;
        (self.read(whole_frames), self.read(whole_frames + 1), fraction)
    }

    /// Zero every frame and rewind the cursor, without reallocating.
    pub fn reset(&mut self) {
        for slot in &mut self.buffer {
            *slot = S::ZERO;
        }
        self.cursor = 0;
    }
}

/// `value × fraction`, `fraction` a Q0.16 weight in `0..=0x1_0000`. Written directly
/// against `DspSample`'s two narrowing primitives — `scale_q15` is the wrong scale here
/// (Q1.15, not Q0.16) — so this is its own small Q16 multiply, generic over `S`.
fn scale_by_q16_fraction<S: DspSample>(value: S, fraction_q16: i32) -> S {
    // `mul_q24` expects a Q8.24 coefficient; a Q0.16 fraction shifted up by eight bits is
    // exactly that, and the two formats share one exact bit pattern for every fraction
    // this method is ever called with (fraction_q16 is at most 0xFFFF).
    value.mul_q24(fraction_q16 << 8)
}

/// Two independent [`DelayLine`]s, one per stereo channel.
#[derive(Clone, Debug)]
pub struct StereoDelayLine<S: DspSample> {
    pub left: DelayLine<S>,
    pub right: DelayLine<S>,
}

impl<S: DspSample> StereoDelayLine<S> {
    /// Allocate a stereo delay line: two [`DelayLine`]s, each sized as
    /// [`DelayLine::new`] would size one.
    pub fn new(capacity_frames: usize) -> StereoDelayLine<S> {
        StereoDelayLine { left: DelayLine::new(capacity_frames), right: DelayLine::new(capacity_frames) }
    }

    /// Write one stereo frame and advance both cursors together.
    pub fn write(&mut self, left: S, right: S) {
        self.left.write(left);
        self.right.write(right);
    }

    /// Zero both channels and rewind both cursors.
    pub fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_rounds_up_to_a_power_of_two() {
        assert_eq!(DelayLine::<i32>::new(1).capacity(), 1);
        assert_eq!(DelayLine::<i32>::new(5).capacity(), 8);
        assert_eq!(DelayLine::<i32>::new(1_000).capacity(), 1_024);
        assert_eq!(DelayLine::<i32>::new(1_024).capacity(), 1_024, "an exact power of two is not rounded further up");
    }

    #[test]
    fn a_fresh_delay_line_reads_silence() {
        let line = DelayLine::<i32>::new(16);
        for delay in [0u32, 1, 15, 100] {
            assert_eq!(line.read(delay), 0);
        }
    }

    #[test]
    fn read_zero_is_the_most_recent_write() {
        let mut line = DelayLine::<i32>::new(4);
        line.write(111);
        line.write(222);
        assert_eq!(line.read(0), 222);
        assert_eq!(line.read(1), 111);
        assert_eq!(line.read(2), 0);
    }

    #[test]
    fn writes_wrap_around_the_ring() {
        let mut line = DelayLine::<i32>::new(4);
        for value in 1..=6 {
            line.write(value);
        }
        // Values 1 and 2 have been overwritten by 5 and 6.
        assert_eq!(line.read(0), 6);
        assert_eq!(line.read(1), 5);
        assert_eq!(line.read(2), 4);
        assert_eq!(line.read(3), 3);
    }

    #[test]
    fn read_fractional_interpolates_linearly() {
        let mut line = DelayLine::<i32>::new(4);
        line.write(0);
        line.write(1_000);
        // read(0) = 1000 (most recent), read(1) = 0.
        assert_eq!(line.read_fractional(0), 1_000);
        assert_eq!(line.read_fractional(1 << 16), 0);
        assert_eq!(line.read_fractional(1 << 15), 500, "halfway between 1000 and 0");
    }

    #[test]
    fn read_fractional_on_the_float_path_agrees_with_the_fixed_path() {
        let mut fixed = DelayLine::<i32>::new(4);
        let mut float = DelayLine::<f32>::new(4);
        for value in [1_000i16, -500, 2_000] {
            fixed.write(i32::from_i16(value));
            float.write(f32::from_i16(value));
        }
        for delay_q16 in [0u32, 1 << 15, 1 << 16, 3 << 15] {
            let fixed_value = fixed.read_fractional(delay_q16) as f32;
            let float_value = float.read_fractional(delay_q16);
            assert!((fixed_value - float_value).abs() < 1.0, "delay {delay_q16}: fixed {fixed_value} vs float {float_value}");
        }
    }

    #[test]
    fn reset_zeroes_the_buffer_and_rewinds_the_cursor() {
        let mut line = DelayLine::<i32>::new(4);
        line.write(1_234);
        line.reset();
        assert_eq!(line.read(0), 0);
        line.write(5_678);
        assert_eq!(line.read(0), 5_678, "the cursor is usable again after reset");
    }

    #[test]
    fn stereo_delay_line_keeps_channels_independent() {
        let mut stereo = StereoDelayLine::<i32>::new(4);
        stereo.write(100, -100);
        stereo.write(200, -200);
        assert_eq!(stereo.left.read(0), 200);
        assert_eq!(stereo.right.read(0), -200);
        assert_eq!(stereo.left.read(1), 100);
        assert_eq!(stereo.right.read(1), -100);
        stereo.reset();
        assert_eq!(stereo.left.read(0), 0);
        assert_eq!(stereo.right.read(0), 0);
    }

    #[test]
    fn a_delay_at_or_past_capacity_still_reads_something_in_bounds_rather_than_panicking() {
        let mut line = DelayLine::<i32>::new(4);
        for value in 1..=4 {
            line.write(value);
        }
        // Must not panic; the exact wrapped value is an implementation detail.
        let _ = line.read(4);
        let _ = line.read(1_000_000);
    }
}
