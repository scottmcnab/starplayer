//! [`RowClock`] — a row's tick budget, and where in it we are (architecture §4).

/// The tick budget of one pattern row.
///
/// # Why this is a type and not two `u8`s in the sequencer
///
/// The original does the naive thing (`__UpdateTracker`, `STARPLAY/S3MLIB.ASM` ~2423):
/// `if _MRowDelay != 0 { dec; goto @@nonewpattern }` — on a pattern delay the row's
/// effects re-run and its notes are not re-fetched, and the tick counter simply restarts.
/// MOD `EEx`, S3M `SEx`, XM `EEx` and IT `SEx` all differ subtly around that, and IT's
/// `SEx` × `SDx` interaction is a known minefield.
///
/// So the budget is modelled explicitly, and [`RowClock::tick_in_row`] is **absolute
/// across pattern-delay repeats**: with speed 6 and `pattern_delay` 2 it runs 0..17, not
/// 0..5 three times. That is what `Qxy` retrigger, `Ixy` tremor, `SDx` note delay and
/// `SCx` note cut all key off — `SD8` on a delayed row fires once, on absolute tick 8,
/// not once per repeat.
///
/// # There is deliberately no shared tick-zero helper
///
/// This type exposes [`RowClock::is_first_tick_of_row`] and
/// [`RowClock::is_first_tick_of_repeat`] and stops there. It does **not** offer a shared
/// `if tick == 0 { row_handler() } else { tick_handler() }` dispatcher, however obvious
/// that looks after writing the second format.
///
/// The reason is that the formats do not agree on what belongs on either side of that
/// branch, and the disagreements are not visible at the call site:
///
/// * S3M runs the tick handler for `Ixy` tremor and `Jxy` arpeggio on tick 0 **as well
///   as** the row handler, so for those two effects the branch is not exclusive at all.
/// * `SDx` note delay moves a note's effect off tick 0 entirely, without moving the row
///   parse.
/// * On a pattern-delay repeat MOD re-runs some tick-0 effects and S3M does not.
/// * XM and IT differ again on which of these the delayed repeat re-runs.
///
/// A shared dispatcher would have to grow a flag for each of those, and every one of the
/// flags would be read by all five formats. That shared branch is where every
/// cross-format bug would live. Each format's effect processor picks the predicate it
/// cares about, in its own crate, where its own tests can see it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowClock {
    /// Ticks per row repeat — the tracker's `speed`, `Axx` in S3M.
    ///
    /// A speed of 0 is representable because a corrupt module can contain one and nothing
    /// here may panic; a zero-speed row has no ticks at all and
    /// [`RowClock::advance`] ends it immediately. What a format *does* with `A00` is that
    /// format's decision (ST3 ignores it — accuracy policy D6).
    pub speed: u8,
    /// Extra whole repeats of the row, from `SEx` / `EEx`. Zero means the row plays once.
    pub pattern_delay: u8,
    /// How many ticks of this row have already been played, **absolute across repeats**.
    pub tick_in_row: u16,
    /// Which repeat the current tick belongs to: 0 for the first pass, 1 for the first
    /// `SEx` repeat, and so on.
    pub repeat_index: u8,
}

/// What [`RowClock::advance`] decided.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum RowAdvance {
    /// The row has ticks left; the sequencer stays where it is.
    SameRow,
    /// The row's budget is spent. The sequencer moves on, and whoever owns it calls
    /// [`RowClock::start_row`] with the speed the next row begins with.
    NextRow,
}

impl RowClock {
    /// A clock at the first tick of a row that lasts `speed` ticks.
    pub const fn new(speed: u8) -> RowClock {
        RowClock { speed, pattern_delay: 0, tick_in_row: 0, repeat_index: 0 }
    }

    /// The very first tick of the row — where notes are fetched and tick-0 effects run.
    ///
    /// True **once** per row, not once per pattern-delay repeat.
    pub const fn is_first_tick_of_row(&self) -> bool { self.tick_in_row == 0 }

    /// The first tick of any repeat of the row, including the first.
    ///
    /// With speed 6 and `pattern_delay` 2 this is true at absolute ticks 0, 6 and 12.
    pub const fn is_first_tick_of_repeat(&self) -> bool {
        if self.speed == 0 {
            // A zero-speed row has no repeats to be at the start of. `%` would divide by
            // zero, and nothing in this crate may panic.
            return self.tick_in_row == 0;
        }
        self.tick_in_row.is_multiple_of(self.speed as u16)
    }

    /// The row's whole tick budget: `speed * (1 + pattern_delay)`.
    pub const fn total_ticks(&self) -> u16 {
        (self.speed as u16).saturating_mul(1u16.saturating_add(self.pattern_delay as u16))
    }

    /// Ticks still to play in this row.
    pub const fn ticks_remaining(&self) -> u16 { self.total_ticks().saturating_sub(self.tick_in_row) }

    /// Consume the tick that has just been processed and report whether the row is over.
    ///
    /// Call this **after** the tick's effects have run and after any `Axx` or `SEx` on
    /// that tick has been applied, so that a mid-row change to the budget is already
    /// visible here.
    pub fn advance(&mut self) -> RowAdvance {
        self.tick_in_row = self.tick_in_row.saturating_add(1);
        if self.tick_in_row >= self.total_ticks() {
            return RowAdvance::NextRow;
        }
        self.repeat_index = if self.speed == 0 { 0 } else { (self.tick_in_row / self.speed as u16) as u8 };
        RowAdvance::SameRow
    }

    /// Begin a new row at `speed` ticks, clearing the pattern delay and the tick index.
    ///
    /// The pattern delay is cleared because `SEx` applies to the row it appears on and
    /// nothing after it — the original reloads `_MRowDelay` from the row it just parsed.
    pub fn start_row(&mut self, speed: u8) {
        self.speed = speed;
        self.pattern_delay = 0;
        self.tick_in_row = 0;
        self.repeat_index = 0;
    }

    /// Change the row's tick budget mid-row (`Axx`).
    ///
    /// # The wrap rule, stated rather than inherited
    ///
    /// Architecture §4 requires this to be explicit rather than falling out of modulo
    /// arithmetic. The rule here is: **the budget is re-read on every
    /// [`RowClock::advance`]**, so lowering `speed` below the current `tick_in_row` ends
    /// the row at the next `advance` rather than wrapping to a negative remainder, and
    /// raising it lengthens the row in progress. [`RowClock::repeat_index`] is
    /// recomputed from the new speed as well, so a mid-row change moves the repeat
    /// boundaries for the rest of that row and no further.
    ///
    /// Whether a format applies `Axx` to the current row at all is the format's decision,
    /// made by choosing when to report the new speed; this type only says what happens
    /// once it has been applied.
    pub fn set_speed(&mut self, speed: u8) {
        self.speed = speed;
        self.repeat_index = if speed == 0 { 0 } else { (self.tick_in_row / speed as u16) as u8 };
    }

    /// Add whole repeats of this row (`SEx` / `EEx`), extending the budget in place.
    pub fn set_pattern_delay(&mut self, pattern_delay: u8) { self.pattern_delay = pattern_delay; }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The task's stated case: speed 6, `SE2`.
    #[test]
    fn speed_six_with_a_two_row_pattern_delay_lasts_eighteen_ticks() {
        let mut clock = RowClock::new(6);
        clock.set_pattern_delay(2);
        assert_eq!(clock.total_ticks(), 18);

        let mut first_tick_of_row = alloc::vec::Vec::new();
        let mut first_tick_of_repeat = alloc::vec::Vec::new();
        for tick in 0..18u16 {
            assert_eq!(clock.tick_in_row, tick);
            if clock.is_first_tick_of_row() {
                first_tick_of_row.push(tick);
            }
            if clock.is_first_tick_of_repeat() {
                first_tick_of_repeat.push(tick);
            }
            let advance = clock.advance();
            let expected = if tick == 17 { RowAdvance::NextRow } else { RowAdvance::SameRow };
            assert_eq!(advance, expected, "tick {tick}");
        }
        assert_eq!(first_tick_of_row, alloc::vec![0], "notes are fetched once, not once per repeat");
        assert_eq!(first_tick_of_repeat, alloc::vec![0, 6, 12]);
    }

    #[test]
    fn the_repeat_index_counts_repeats_not_ticks() {
        let mut clock = RowClock::new(4);
        clock.set_pattern_delay(2);
        let mut seen = alloc::vec::Vec::new();
        for _ in 0..12 {
            seen.push(clock.repeat_index);
            clock.advance();
        }
        assert_eq!(seen, alloc::vec![0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2]);
    }

    #[test]
    fn a_plain_row_ends_after_speed_ticks() {
        let mut clock = RowClock::new(3);
        assert_eq!(clock.total_ticks(), 3);
        assert_eq!(clock.ticks_remaining(), 3);
        assert_eq!(clock.advance(), RowAdvance::SameRow);
        assert_eq!(clock.advance(), RowAdvance::SameRow);
        assert_eq!(clock.ticks_remaining(), 1);
        assert_eq!(clock.advance(), RowAdvance::NextRow);
    }

    #[test]
    fn starting_a_row_clears_the_pattern_delay() {
        let mut clock = RowClock::new(6);
        clock.set_pattern_delay(3);
        clock.advance();
        clock.start_row(4);
        assert_eq!(clock, RowClock::new(4), "SEx applies to its own row and nothing after it");
        assert_eq!(clock.total_ticks(), 4);
    }

    #[test]
    fn lowering_the_speed_mid_row_ends_the_row_rather_than_wrapping() {
        let mut clock = RowClock::new(8);
        for _ in 0..5 {
            clock.advance();
        }
        assert_eq!(clock.tick_in_row, 5);
        clock.set_speed(3);
        assert_eq!(clock.total_ticks(), 3, "the budget is re-read, not remembered");
        assert_eq!(clock.advance(), RowAdvance::NextRow, "6 >= 3, so the row is over");
    }

    #[test]
    fn raising_the_speed_mid_row_lengthens_the_row_in_progress() {
        let mut clock = RowClock::new(3);
        clock.advance();
        clock.set_speed(6);
        assert_eq!(clock.advance(), RowAdvance::SameRow);
        assert_eq!(clock.tick_in_row, 2);
        for _ in 2..5 {
            assert_eq!(clock.advance(), RowAdvance::SameRow);
        }
        assert_eq!(clock.advance(), RowAdvance::NextRow);
    }

    #[test]
    fn a_zero_speed_row_neither_panics_nor_hangs() {
        let mut clock = RowClock::new(0);
        assert_eq!(clock.total_ticks(), 0);
        assert!(clock.is_first_tick_of_row());
        assert!(clock.is_first_tick_of_repeat(), "no modulo by zero");
        assert_eq!(clock.advance(), RowAdvance::NextRow, "a row with no ticks is over immediately");
    }

    /// The widest budget a corrupt module can ask for still fits, and the saturating
    /// arithmetic means nothing wraps on the way there.
    #[test]
    fn the_widest_possible_budget_does_not_wrap() {
        let mut clock = RowClock::new(u8::MAX);
        clock.set_pattern_delay(u8::MAX);
        assert_eq!(clock.total_ticks(), 65_280, "255 ticks x 256 repeats, computed in u16 without wrapping");
        clock.tick_in_row = 65_279;
        assert_eq!(clock.advance(), RowAdvance::NextRow);
    }
}
