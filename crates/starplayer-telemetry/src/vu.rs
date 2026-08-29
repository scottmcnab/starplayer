//! [`VuMeter`] — the original's `_VUBarLevel`, in `U0F16`.

use starplayer_core::U0F16;

/// One channel's peak-hold VU level.
///
/// # The original's behaviour, scaled
///
/// `ChannelData._VUBarLevel` is a 0..64 byte. It is **set** — not maximised — to the
/// channel volume on a new note or a volume-column write, and `__UpdateTracker` decays it
/// **by 2 per tick**, clamping at 0
/// (`plans/reference/original-star-ui.md` §2.2, `original-s3mlib-analysis.md` §2). The
/// display renders it as a 16-cell green → yellow → red bar.
///
/// Keeping the ratio in `U0F16` makes the decay `2/64` of full scale per tick, which is
/// `2 * 65536 / 64 = 2048` in `U0F16`'s raw bits — [`VuMeter::DECAY_PER_TICK`]. From full
/// scale that reaches zero in exactly 32 ticks, the same as the original's `64 / 2`, and
/// at roughly 50 Hz that is a shade over half a second of fall time.
///
/// Setting rather than maximising is deliberate and is what the original does: a note
/// retriggered at a lower volume drops the bar immediately instead of leaving a stale
/// peak hanging above it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VuMeter {
    level: U0F16,
}

impl VuMeter {
    /// How much a tick takes off the level: `2/64` of full scale, the original's ratio.
    pub const DECAY_PER_TICK: U0F16 = U0F16::from_bits(2 * (u16::MAX / 64 + 1));

    /// Ticks a meter at full scale takes to reach zero.
    pub const TICKS_TO_SILENCE: u32 = 32;

    /// A meter reading zero.
    pub const SILENT: VuMeter = VuMeter { level: U0F16::ZERO };

    /// The current level.
    pub const fn level(self) -> U0F16 { self.level }

    /// Whether the meter has decayed away.
    pub fn is_silent(self) -> bool { self.level == U0F16::ZERO }

    /// A new note or a volume-column write: hold at `volume`.
    pub fn strike(&mut self, volume: U0F16) { self.level = volume; }

    /// One tick's decay, clamped at zero.
    pub fn decay(&mut self) { self.level = self.level.saturating_sub(VuMeter::DECAY_PER_TICK); }

    /// Back to silence, without waiting out the decay. What a transport stop wants.
    pub fn reset(&mut self) { self.level = U0F16::ZERO; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_decay_is_two_sixty_fourths_of_full_scale_per_tick() {
        assert_eq!(VuMeter::DECAY_PER_TICK.to_bits(), 2048, "2 * 65536 / 64, the original's 2-per-tick on a 0..64 scale");
    }

    #[test]
    fn full_scale_decays_to_zero_in_exactly_thirty_two_ticks_and_clamps() {
        let mut meter = VuMeter::SILENT;
        meter.strike(U0F16::MAX);
        assert_eq!(meter.level(), U0F16::MAX);

        for tick in 1..VuMeter::TICKS_TO_SILENCE {
            meter.decay();
            assert!(!meter.is_silent(), "still audible after {tick} tick(s)");
        }
        meter.decay();
        assert!(meter.is_silent(), "silent after exactly {} ticks", VuMeter::TICKS_TO_SILENCE);

        meter.decay();
        meter.decay();
        assert_eq!(meter.level(), U0F16::ZERO, "the decay clamps at zero rather than wrapping");
    }

    #[test]
    fn a_strike_sets_the_level_rather_than_maximising_it() {
        let mut meter = VuMeter::SILENT;
        meter.strike(U0F16::MAX);
        meter.strike(U0F16::from_bits(1_000));
        assert_eq!(meter.level().to_bits(), 1_000, "a quieter retrigger drops the bar, as in the original");

        meter.reset();
        assert!(meter.is_silent());
    }

    #[test]
    fn a_partial_level_decays_by_the_same_step() {
        let mut meter = VuMeter::SILENT;
        meter.strike(U0F16::from_bits(5_000));
        meter.decay();
        assert_eq!(meter.level().to_bits(), 5_000 - 2_048);
    }
}
