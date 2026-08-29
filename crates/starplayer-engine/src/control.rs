//! [`ControlClock`] — the one rate at which envelopes advance (architecture §5.4).

use starplayer_core::Frame;

/// Microseconds between synthesised control ticks when nothing else is driving them.
///
/// ~1 kHz: fine enough that a MIDI-driven envelope has no audible stepping, coarse enough
/// that the per-tick work is nothing next to the mixing.
pub const DEFAULT_CONTROL_INTERVAL_MICROS: u32 = 1_000;

/// What is driving the control clock.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum ControlDriver {
    /// The engine is generating ticks itself, at a fixed frame interval.
    #[default]
    Synthesised,
    /// A tracker sequencer's tick *is* the control tick, so the engine generates none.
    Tracker,
}

/// The clock envelopes, auto-vibrato, fadeout and NNA advance on.
///
/// XM and IT envelopes advance **exactly once per tracker tick** — not per sample, not per
/// buffer. Rather than special-casing that, there is one rule: *envelopes advance on
/// control ticks*, and what a control tick is depends on what is playing:
///
/// * with a tracker sequencer driving, **its** tick is the control tick — it calls
///   [`ControlClock::tick_from_tracker`] from inside its dispatch, which also switches the
///   driver to [`ControlDriver::Tracker`] so the engine stops generating its own;
/// * with no tracker (pure MIDI, live keyboard, a synth instrument), the engine
///   synthesises one every [`ControlClock::interval_frames`] frames, on exact frame
///   boundaries like any other event.
///
/// M1 lands the clock and its tick count. Nothing advances *on* it until envelopes arrive
/// in M5; the point of building it now is that the rule is uniform from the start rather
/// than retrofitted around whatever M5 happens to need.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ControlClock {
    driver: ControlDriver,
    interval_frames: u32,
    next_frame: Frame,
    ticks: u64,
}

impl ControlClock {
    /// A synthesised clock at [`DEFAULT_CONTROL_INTERVAL_MICROS`], whose first tick is due
    /// at `first_tick_frame`.
    pub fn new(sample_rate_hz: u32, first_tick_frame: Frame) -> ControlClock {
        ControlClock::with_interval_micros(sample_rate_hz, first_tick_frame, DEFAULT_CONTROL_INTERVAL_MICROS)
    }

    /// A synthesised clock at a chosen interval.
    ///
    /// The frame interval is computed with integer arithmetic only — `rate * micros /
    /// 1_000_000`, in `u64` so it cannot overflow — because a float here would make the
    /// control tick positions differ between x86, ARM and WASM, and design goal 5 bans
    /// that from the RT path. It is clamped to at least one frame so the clock always
    /// advances.
    pub fn with_interval_micros(sample_rate_hz: u32, first_tick_frame: Frame, interval_micros: u32) -> ControlClock {
        ControlClock {
            driver: ControlDriver::Synthesised,
            interval_frames: ControlClock::interval_frames_for(sample_rate_hz, interval_micros),
            next_frame: first_tick_frame,
            ticks: 0,
        }
    }

    /// `sample_rate_hz * interval_micros / 1_000_000`, at least 1.
    fn interval_frames_for(sample_rate_hz: u32, interval_micros: u32) -> u32 {
        let frames = (sample_rate_hz as u64).saturating_mul(interval_micros as u64) / 1_000_000;
        frames.clamp(1, u32::MAX as u64) as u32
    }

    /// What is driving the clock.
    pub const fn driver(&self) -> ControlDriver { self.driver }

    /// Frames between synthesised ticks.
    pub const fn interval_frames(&self) -> u32 { self.interval_frames }

    /// How many control ticks have happened since the clock was created.
    pub const fn ticks(&self) -> u64 { self.ticks }

    /// The frame at which the engine owes a synthesised tick, or `None` when a tracker is
    /// driving and the engine owes none.
    pub const fn next_synthesised_frame(&self) -> Option<Frame> {
        match self.driver {
            ControlDriver::Synthesised => Some(self.next_frame),
            ControlDriver::Tracker => None,
        }
    }

    /// Take the synthesised tick that was due, and schedule the next one.
    ///
    /// Always moves [`ControlClock::next_synthesised_frame`] forward by at least one frame,
    /// so a caller looping on "is a tick due?" always terminates.
    pub fn tick_synthesised(&mut self) {
        self.ticks = self.ticks.saturating_add(1);
        self.next_frame = self.next_frame.saturating_add(self.interval_frames as u64);
    }

    /// A tracker sequencer's tick. Counts as the control tick, and stops the engine
    /// synthesising its own.
    pub fn tick_from_tracker(&mut self, frame: Frame) {
        self.driver = ControlDriver::Tracker;
        self.ticks = self.ticks.saturating_add(1);
        self.next_frame = frame;
    }

    /// Go back to generating ticks, starting at `first_tick_frame`.
    ///
    /// The engine calls this whenever the source set changes, because the sequencer that
    /// was driving the clock may have just been removed.
    pub fn resume_synthesis(&mut self, first_tick_frame: Frame) {
        self.driver = ControlDriver::Synthesised;
        self.next_frame = first_tick_frame;
    }

    /// Change the output rate, recomputing the synthesised interval.
    pub fn set_sample_rate_hz(&mut self, sample_rate_hz: u32, interval_micros: u32) {
        self.interval_frames = ControlClock::interval_frames_for(sample_rate_hz, interval_micros);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_synthesised_clock_ticks_at_the_requested_rate() {
        let mut clock = ControlClock::new(48_000, Frame::ZERO);
        assert_eq!(clock.interval_frames(), 48, "1 ms at 48 kHz");
        assert_eq!(clock.driver(), ControlDriver::Synthesised);
        assert_eq!(clock.next_synthesised_frame(), Some(Frame::ZERO));

        clock.tick_synthesised();
        assert_eq!(clock.ticks(), 1);
        assert_eq!(clock.next_synthesised_frame(), Some(Frame(48)));
        clock.tick_synthesised();
        assert_eq!(clock.next_synthesised_frame(), Some(Frame(96)));
    }

    #[test]
    fn the_interval_is_integer_arithmetic_and_never_zero() {
        assert_eq!(ControlClock::new(44_100, Frame::ZERO).interval_frames(), 44, "44100 / 1000 truncates to 44");
        assert_eq!(ControlClock::with_interval_micros(8_000, Frame::ZERO, 1).interval_frames(), 1, "a sub-frame interval clamps to one frame");
        assert_eq!(ControlClock::with_interval_micros(u32::MAX, Frame::ZERO, u32::MAX).interval_frames(), u32::MAX);
    }

    #[test]
    fn a_tracker_tick_takes_the_clock_over() {
        let mut clock = ControlClock::new(44_100, Frame::ZERO);
        clock.tick_from_tracker(Frame(882));
        assert_eq!(clock.driver(), ControlDriver::Tracker);
        assert_eq!(clock.ticks(), 1);
        assert_eq!(clock.next_synthesised_frame(), None, "the engine must not add ticks of its own on top");

        clock.tick_from_tracker(Frame(1_764));
        assert_eq!(clock.ticks(), 2);
    }

    #[test]
    fn synthesis_resumes_when_the_tracker_goes_away() {
        let mut clock = ControlClock::new(44_100, Frame::ZERO);
        clock.tick_from_tracker(Frame(882));
        clock.resume_synthesis(Frame(1_000));
        assert_eq!(clock.driver(), ControlDriver::Synthesised);
        assert_eq!(clock.next_synthesised_frame(), Some(Frame(1_000)));
        assert_eq!(clock.ticks(), 1, "resuming does not rewind the tick count");
    }

    #[test]
    fn changing_the_sample_rate_changes_the_interval() {
        let mut clock = ControlClock::new(44_100, Frame::ZERO);
        clock.set_sample_rate_hz(96_000, DEFAULT_CONTROL_INTERVAL_MICROS);
        assert_eq!(clock.interval_frames(), 96);
    }
}
