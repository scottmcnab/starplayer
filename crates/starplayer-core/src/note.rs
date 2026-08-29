//! Pitch representation: [`Note`] and the tracker-native [`Period`].

/// A pitch, as a semitone plus a signed cents offset.
///
/// The semitone number is the MIDI note number: 60 is middle C, 69 is A440. Cents are
/// hundredths of a semitone and carry everything MIDI cannot express — MOD/S3M finetune,
/// a mid-slide portamento position, XM's 8-cent-per-unit finetune, an arbitrary
/// microtonal scale (architecture §2.3). `cents` is not normalised: a caller may hold
/// ±1200 if that is what its arithmetic produced, and only [`Note::to_midi`] folds it
/// back into a semitone.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Note {
    /// Semitone index on the MIDI scale: 60 = middle C, 69 = A440.
    pub semitone: u8,
    /// Signed offset from `semitone`, in hundredths of a semitone.
    pub cents: i16,
}

/// The highest MIDI note number.
pub const MIDI_NOTE_MAX: u8 = 127;

impl Note {
    /// Middle C, MIDI note 60.
    pub const MIDDLE_C: Note = Note { semitone: 60, cents: 0 };

    /// Concert A, MIDI note 69.
    pub const A440: Note = Note { semitone: 69, cents: 0 };

    /// A note at an exact semitone.
    pub const fn new(semitone: u8) -> Note { Note { semitone, cents: 0 } }

    /// A note at a semitone plus a cents offset.
    pub const fn with_cents(semitone: u8, cents: i16) -> Note { Note { semitone, cents } }

    /// Build from a MIDI note number. The semitone scale *is* the MIDI scale, so this is
    /// an exact widening with no cents offset.
    pub const fn from_midi(midi_note: u8) -> Note { Note { semitone: midi_note, cents: 0 } }

    /// Collapse to the nearest MIDI note number, folding whole semitones out of `cents`
    /// and rounding the remainder to nearest (ties away from zero).
    ///
    /// Saturates at 0 and [`MIDI_NOTE_MAX`] rather than wrapping. Round-trips exactly
    /// with [`Note::from_midi`] over the whole MIDI range.
    pub const fn to_midi(self) -> u8 {
        let rounded_semitones = if self.cents >= 0 {
            (self.cents as i32 + 50) / 100
        } else {
            (self.cents as i32 - 50) / 100
        };
        let midi_note = self.semitone as i32 + rounded_semitones;
        if midi_note < 0 {
            0
        } else if midi_note > MIDI_NOTE_MAX as i32 {
            MIDI_NOTE_MAX
        } else {
            midi_note as u8
        }
    }

    /// Shift by whole semitones, saturating at the ends of the MIDI range. This is what
    /// arpeggio and transpose want.
    pub const fn transposed(self, semitones: i16) -> Note {
        let shifted = self.semitone as i32 + semitones as i32;
        let semitone = if shifted < 0 {
            0
        } else if shifted > MIDI_NOTE_MAX as i32 {
            MIDI_NOTE_MAX
        } else {
            shifted as u8
        };
        Note { semitone, cents: self.cents }
    }

    /// Shift by cents, saturating the offset rather than wrapping it.
    pub const fn detuned(self, cents: i16) -> Note {
        Note { semitone: self.semitone, cents: self.cents.saturating_add(cents) }
    }
}

/// A tracker period, in S3M units.
///
/// **S3M periods are Amiga periods × 4** (`plans/reference/original-s3mlib-analysis.md`
/// §6): with the default C4 speed of 8363 the note table's octave-4 entry 1712 is
/// Amiga 428 × 4. That factor is why every S3M pitch slide multiplies its parameter by
/// four and why the Amiga clamp is `[113 * 4, 856 * 4]`.
///
/// Period *arithmetic* — note→period, period→frequency, glissando snapping, the slide
/// handlers — lives in the format crates. This type only carries the value, so the
/// formats cannot accidentally mix a raw Amiga period into an S3M one.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Period(pub u32);

impl Period {
    /// The smallest usable period. The original forces a computed period of zero up to
    /// one (`PeriodFromNote`, `STARPLAY/S3MLIB.ASM` ~3541), since it is about to be used
    /// as a divisor.
    pub const MIN: Period = Period(1);

    /// The Amiga low clamp, `113 * 4`, applied when the module's Amiga-limits flag is
    /// set (`ClipPitch`, `STARPLAY/S3MLIB.ASM` ~2634).
    pub const AMIGA_LIMIT_LOW: Period = Period(113 * AMIGA_PERIOD_SCALE);

    /// The Amiga high clamp, `856 * 4`.
    pub const AMIGA_LIMIT_HIGH: Period = Period(856 * AMIGA_PERIOD_SCALE);

    /// The raw S3M period.
    pub const fn get(self) -> u32 { self.0 }

    /// Widen an Amiga (ProTracker) period into S3M units.
    pub const fn from_amiga(amiga_period: u32) -> Period { Period(amiga_period.saturating_mul(AMIGA_PERIOD_SCALE)) }

    /// Narrow back to an Amiga period, truncating.
    pub const fn to_amiga(self) -> u32 { self.0 / AMIGA_PERIOD_SCALE }

    /// Slide the period up in pitch (period *down*), saturating at [`Period::MIN`].
    pub const fn saturating_sub(self, delta: u32) -> Period {
        let lowered = self.0.saturating_sub(delta);
        if lowered < Period::MIN.0 { Period::MIN } else { Period(lowered) }
    }

    /// Slide the period down in pitch (period *up*), saturating.
    pub const fn saturating_add(self, delta: u32) -> Period { Period(self.0.saturating_add(delta)) }
}

/// The factor between an Amiga period and an S3M period.
pub const AMIGA_PERIOD_SCALE: u32 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midi_round_trips_over_the_full_range() {
        for midi_note in 0..=MIDI_NOTE_MAX {
            assert_eq!(Note::from_midi(midi_note).to_midi(), midi_note, "MIDI note {midi_note} must round trip");
        }
    }

    #[test]
    fn cents_fold_into_the_nearest_semitone() {
        assert_eq!(Note::with_cents(60, 0).to_midi(), 60);
        assert_eq!(Note::with_cents(60, 49).to_midi(), 60);
        assert_eq!(Note::with_cents(60, 50).to_midi(), 61, "ties round away from zero");
        assert_eq!(Note::with_cents(60, -50).to_midi(), 59);
        assert_eq!(Note::with_cents(60, -49).to_midi(), 60);
        assert_eq!(Note::with_cents(60, 1200).to_midi(), 72);
        assert_eq!(Note::with_cents(60, -1200).to_midi(), 48);
    }

    #[test]
    fn midi_conversion_saturates_instead_of_wrapping() {
        assert_eq!(Note::with_cents(0, -5000).to_midi(), 0);
        assert_eq!(Note::with_cents(127, 5000).to_midi(), MIDI_NOTE_MAX);
        assert_eq!(Note::new(0).transposed(-12), Note::new(0));
        assert_eq!(Note::new(127).transposed(12), Note::new(MIDI_NOTE_MAX));
        assert_eq!(Note::with_cents(60, i16::MAX).detuned(100).cents, i16::MAX);
    }

    #[test]
    fn s3m_periods_are_amiga_periods_times_four() {
        assert_eq!(Period::from_amiga(428), Period(1712));
        assert_eq!(Period(1712).to_amiga(), 428);
        assert_eq!(Period::AMIGA_LIMIT_LOW, Period(452));
        assert_eq!(Period::AMIGA_LIMIT_HIGH, Period(3424));
    }

    #[test]
    fn period_slides_saturate_at_one() {
        assert_eq!(Period(10).saturating_sub(100), Period::MIN, "a period must never reach zero: it is a divisor");
        assert_eq!(Period(u32::MAX).saturating_add(100), Period(u32::MAX));
        assert_eq!(Period(1712).saturating_sub(112), Period(1600));
    }
}
