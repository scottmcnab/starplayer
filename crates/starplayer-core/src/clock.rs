//! [`FrameClock`] — where the next tracker tick lands, in absolute output frames.

use crate::fixed::Q32_32;
use crate::frame::Frame;
use crate::tempo::TempoModel;

/// The tick clock: an absolute [`Frame`], a Q32.32 remainder, and a [`TempoModel`].
///
/// # The one rule
///
/// > **Never cache the next-event frame across a dispatch.** `Txx`, `Axx`, `SEx`, `SDx`,
/// > `Bxx` and `Cxx` all change *when the next tick is* from inside the tick just
/// > processed. Compute the next boundary at the **end** of processing the current tick.
/// > — architecture §3.1 rule 1
///
/// This type is shaped so that rule cannot be broken. There is exactly one function that
/// computes a tick boundary, [`FrameClock::advance_tick`], it takes the tempo and speed
/// as arguments rather than holding them, and it is meant to be called once at the end of
/// each tick with the values that tick left in effect. [`FrameClock::pending_tick_frame`]
/// only reads back what that call produced, so it cannot go stale: a tempo change made
/// during a tick is picked up by the `advance_tick` at the end of that same tick.
///
/// There is deliberately no `set_tempo`, no cached `frames_per_tick`, and no way to ask
/// for "the next tick frame" without also committing the clock to it.
///
/// # Generic, not `dyn`
///
/// The model is a type parameter, so `frames_per_tick` inlines and the zero-sized models
/// cost nothing. A host that wants to switch models at run time instantiates
/// `FrameClock<TempoModelId>`, which dispatches through a `match` on a plain data tag —
/// still no vtable in the audio path.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FrameClock<Tempo: TempoModel> {
    tempo_model: Tempo,
    sample_rate_hz: u32,
    pending_tick_frame: Frame,
    /// The sub-frame remainder carried between ticks, always strictly less than one
    /// frame. This is what makes `ExactFixedPoint` drift-free.
    remainder: Q32_32,
}

impl<Tempo: TempoModel> FrameClock<Tempo> {
    /// A clock whose first tick is due at `first_tick_frame`.
    pub fn new(tempo_model: Tempo, sample_rate_hz: u32, first_tick_frame: Frame) -> FrameClock<Tempo> {
        FrameClock { tempo_model, sample_rate_hz, pending_tick_frame: first_tick_frame, remainder: Q32_32::ZERO }
    }

    /// The absolute frame at which the next tick is due.
    ///
    /// Before the first [`FrameClock::advance_tick`] this is the frame passed to
    /// [`FrameClock::new`]; afterwards it is whatever the most recent `advance_tick`
    /// computed, from the tempo in effect at the end of that tick.
    pub fn pending_tick_frame(&self) -> Frame { self.pending_tick_frame }

    /// The output sample rate this clock converts against.
    pub fn sample_rate_hz(&self) -> u32 { self.sample_rate_hz }

    /// The tempo model in use.
    pub fn tempo_model(&self) -> &Tempo { &self.tempo_model }

    /// Consume the tick that was due at [`FrameClock::pending_tick_frame`] and compute
    /// where the next one lands. Returns the new pending frame.
    ///
    /// **Call this once, at the end of processing a tick**, with the `tempo_bpm` and
    /// `speed` that tick left in effect — after `Txx` and `Axx` have been applied, not
    /// before. That is the whole point of the type (architecture §3.1 rule 1).
    ///
    /// The Q32.32 tick length is added to a carried remainder and only whole frames are
    /// released, so a fractional tick length accumulates exactly rather than being
    /// rounded away every tick.
    pub fn advance_tick(&mut self, tempo_bpm: u16, speed: u8) -> Frame {
        let tick_length = self.tempo_model.frames_per_tick(self.sample_rate_hz, tempo_bpm, speed);
        self.remainder = self.remainder.saturating_add(Q32_32::from_bits(tick_length));
        let whole_frames = self.remainder.take_whole();
        self.pending_tick_frame = self.pending_tick_frame.saturating_add(whole_frames as u64);
        self.pending_tick_frame
    }

    /// Restart the clock at `first_tick_frame`, dropping the carried remainder.
    ///
    /// This is a seek, not a rewind: the caller supplies a frame on the engine's
    /// monotonic timeline. The remainder is dropped because it describes a position
    /// within a tick that is no longer being played.
    pub fn reset(&mut self, first_tick_frame: Frame) {
        self.pending_tick_frame = first_tick_frame;
        self.remainder = Q32_32::ZERO;
    }

    /// Replace the output sample rate, keeping the pending tick frame and dropping the
    /// remainder. Only a host reconfiguration should call this, never a module.
    pub fn set_sample_rate_hz(&mut self, sample_rate_hz: u32) {
        self.sample_rate_hz = sample_rate_hz;
        self.remainder = Q32_32::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tempo::{ExactFixedPoint, ItModern, St3Truncating};

    #[test]
    fn a_new_clock_is_due_immediately() {
        let clock = FrameClock::new(ExactFixedPoint, 44100, Frame::ZERO);
        assert_eq!(clock.pending_tick_frame(), Frame::ZERO);
        assert_eq!(clock.sample_rate_hz(), 44100);
        assert_eq!(*clock.tempo_model(), ExactFixedPoint);
    }

    /// The A2 verification case: 10,000 ticks of `ExactFixedPoint` at 44100 Hz / 130 BPM
    /// must land exactly on the closed form, with no accumulated drift.
    #[test]
    fn exact_fixed_point_never_drifts() {
        let mut clock = FrameClock::new(ExactFixedPoint, 44100, Frame::ZERO);
        for _ in 0..10_000 {
            clock.advance_tick(130, 6);
        }
        // floor(10000 * 44100 * 2.5 / 130) == floor(1102500000 / 130) == 8480769
        let closed_form = 10_000u64 * 44100 * 5 / (2 * 130);
        assert_eq!(closed_form, 8_480_769);
        assert_eq!(clock.pending_tick_frame(), Frame(8_480_769), "10,000 exact ticks must land on the closed form");
    }

    #[test]
    fn exact_fixed_point_never_drifts_at_awkward_tempos() {
        for (rate, bpm) in [(44100u32, 143u16), (48000, 33), (22050, 250), (8000, 37)] {
            let mut clock = FrameClock::new(ExactFixedPoint, rate, Frame::ZERO);
            for _ in 0..10_000 {
                clock.advance_tick(bpm, 6);
            }
            let closed_form = 10_000u64 * rate as u64 * 5 / (2 * bpm as u64);
            assert_eq!(clock.pending_tick_frame(), Frame(closed_form), "rate {rate}, bpm {bpm}");
        }
    }

    #[test]
    fn st3_truncating_drifts_exactly_as_the_original_does() {
        let mut clock = FrameClock::new(St3Truncating, 44100, Frame::ZERO);
        for _ in 0..10_000 {
            clock.advance_tick(130, 6);
        }
        assert_eq!(clock.pending_tick_frame(), Frame(8_480_000), "848 frames per tick, no remainder carried");
        let exact = 10_000u64 * 44100 * 5 / (2 * 130);
        assert_eq!(exact - 8_480_000, 769, "the original loses 769 frames over 10,000 ticks");
    }

    #[test]
    fn a_tempo_change_takes_effect_on_the_tick_it_is_applied_to() {
        let mut clock = FrameClock::new(St3Truncating, 44100, Frame::ZERO);
        assert_eq!(clock.advance_tick(130, 6), Frame(848));
        // Txx during that tick raised the tempo; the next boundary uses the new value.
        assert_eq!(clock.advance_tick(65, 6), Frame(848 + 1696));
        assert_eq!(clock.advance_tick(130, 6), Frame(848 + 1696 + 848));
    }

    #[test]
    fn reset_drops_the_carried_remainder() {
        let mut clock = FrameClock::new(ExactFixedPoint, 44100, Frame::ZERO);
        clock.advance_tick(130, 6);
        clock.reset(Frame(1_000));
        assert_eq!(clock.pending_tick_frame(), Frame(1_000));
        assert_eq!(clock.advance_tick(130, 6), Frame(1_848), "the first tick after a reset is a whole 848 frames");
    }

    #[test]
    fn set_sample_rate_changes_the_tick_length() {
        let mut clock = FrameClock::new(ItModern, 44100, Frame::ZERO);
        assert_eq!(clock.advance_tick(125, 6), Frame(882), "44100 * 2.5 / 125 == 882 exactly");
        clock.set_sample_rate_hz(22050);
        assert_eq!(clock.sample_rate_hz(), 22050);
        assert_eq!(clock.advance_tick(125, 6), Frame(882 + 441));
    }

    #[test]
    fn the_clock_saturates_rather_than_wrapping() {
        let mut clock = FrameClock::new(ExactFixedPoint, 44100, Frame::MAX);
        assert_eq!(clock.advance_tick(130, 6), Frame::MAX);
    }
}
