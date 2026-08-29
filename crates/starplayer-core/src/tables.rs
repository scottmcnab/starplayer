//! The shared ST3 constants, the note [`PERIOD_TABLE`] and the four vibrato/tremolo
//! waveform tables.
//!
//! These are data, not behaviour: the period and slide arithmetic that consumes them
//! lives in the format crates. They sit in `starplayer-core` because MOD, S3M and MTM all
//! read the same tables, and because design goal 5 bans transcendental functions from the
//! RT path — a table is how a tracker gets a sine, and it is also what makes the output
//! bit-identical on x86, ARM and WASM (architecture §7.3).
//!
//! Everything here was transcribed from `STARPLAY/S3MLIB.ASM` (lines 417–470) and checked
//! against `plans/reference/original-s3mlib-analysis.md` §§4 and 6.

use crate::note::Period;

/// The default C4 sample rate in Hz, ST3's reference pitch.
///
/// `mov [ebx+_C4SPD],8363` — `STARPLAY/S3MLIB.ASM` ~3643.
pub const ST3_C4_SPEED: u32 = 8363;

/// The note→period scale factor, `8363 * 16`.
///
/// `PeriodFromNote` computes `period = (PERIOD_TABLE[n] * 8363 * 16 >> octave) / c4_speed`
/// and the literal is written `imul eax,eax,8363*16` in the original
/// (`STARPLAY/S3MLIB.ASM` ~3549, and again in `ClipPitch` ~2666); the assembler folds it
/// to 133808. Confirmed against the assembly (task A2 research point 2).
pub const ST3_PERIOD_SCALE: u32 = ST3_C4_SPEED * 16;

/// The period→frequency numerator: `hz = ST3_FREQUENCY_NUMERATOR / period`.
///
/// `PeriodToPitch` is `mov eax,0da7600h / div ebx`, with the source comment
/// `note_herz=14317056 / note_st3period` (`STARPLAY/S3MLIB.ASM` ~3561–3564). `0DA7600h`
/// is 14,317,056.
///
/// # It is *not* `8363 * 1712`
///
/// `plans/reference/original-s3mlib-analysis.md` §6 describes this constant as
/// "`0DA7600h` = 8363 × 1712". That identity does not hold: `8363 * 1712` is
/// **14,317,456**, four hundred more than the constant ST3 actually uses. The
/// discrepancy is Scream Tracker 3's, not the original's — `0DA7600h` is what both ship —
/// and it is why round-tripping C-4 is lossy: period 1712 reads back as
/// `14317056 / 1712 = 8362.77` Hz rather than 8363. Do not "fix" it; every S3M in
/// existence was tracked against this number.
///
/// Verified against the assembly for task A2 research point 2, which is what turned the
/// discrepancy up.
pub const ST3_FREQUENCY_NUMERATOR: u32 = 14_317_056;

/// Number of entries in each vibrato/tremolo waveform table. The phase counter is taken
/// modulo this value, so every table must be exactly this long — see [`VIB_PULSE_TABLE`]
/// for why that is worth a regression test.
pub const WAVEFORM_TABLE_LEN: usize = 64;

/// Peak amplitude of every waveform table. ST3's tables run to ±255 and the effect
/// handlers scale from there, so the value is part of the effect arithmetic rather than
/// an arbitrary choice.
pub const WAVEFORM_AMPLITUDE: i16 = 255;

/// Octave-4 periods for the twelve semitones, at the default C4 speed.
///
/// `Period_Table dw 1712,1616,…,907` — `STARPLAY/S3MLIB.ASM` ~418. In the original this
/// is preceded by a stray `dw 1814`, which no code reads; it is not reproduced.
pub const PERIOD_TABLE: [u16; 12] = [
    1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140, 1076, 1016, 960, 907,
];

/// The octave-4 period of a semitone, as a [`Period`].
///
/// `semitone` is taken modulo 12, matching the original's `and bl,0Fh` masking only in
/// the sense of never indexing out of bounds — the original's failure to do so is
/// accuracy-policy deviation D2, which the format crates handle by carrying the octave
/// properly.
pub const fn period_for_semitone(semitone: usize) -> Period { Period(PERIOD_TABLE[semitone % 12] as u32) }

/// Waveform 0: a 64-point sine, amplitude ±255.
///
/// `Vib_Sine_Table` — `STARPLAY/S3MLIB.ASM` ~440. Transcribed verbatim; the original
/// writes index 32 as `-0`, which assembles to 0.
pub const VIB_SINE_TABLE: [i16; WAVEFORM_TABLE_LEN] = [
       0,   25,   50,   74,   98,  120,  142,  162,
     180,  197,  212,  225,  236,  244,  250,  254,
     255,  254,  250,  244,  236,  225,  212,  197,
     180,  162,  142,  120,   98,   74,   50,   25,
       0,  -25,  -50,  -74,  -98, -120, -142, -162,
    -180, -197, -212, -225, -236, -244, -250, -254,
    -255, -254, -250, -244, -236, -225, -212, -197,
    -180, -162, -142, -120,  -98,  -74,  -50,  -25,
];

/// Waveform 1: a rising ramp from −255 to +255.
///
/// **Accuracy-policy deviation D5** (`plans/product/03-accuracy-policy.md` §3). The
/// original `Vib_Ramp_Table` (`STARPLAY/S3MLIB.ASM` ~449) steps by 8 but skips the value
/// 8 entirely between indices 32 and 33 (`…, -8, -0, 16, 24, …`), so its rising edge has
/// a double-width step in the middle of the waveform. That is a transcription defect, not
/// a format behaviour, so this table is a *regular* ramp instead.
///
/// The ±255 endpoints are kept, since that is ST3's amplitude. Sixty-four points spanning
/// 510 units cannot all be 8 apart, so the ramp is the exact line
/// `value = round(phase * 510 / 63) - 255`: every step is 8 or 9, the table is exactly
/// antisymmetric (`value[63 - i] == -value[i]`), and it starts and ends on ±255.
pub const VIB_RAMP_TABLE: [i16; WAVEFORM_TABLE_LEN] = {
    let mut table = [0i16; WAVEFORM_TABLE_LEN];
    let mut phase = 0;
    while phase < WAVEFORM_TABLE_LEN {
        // Rounded to nearest; the offsets never land on an exact half, so the result is
        // antisymmetric about the centre of the table.
        table[phase] = ((phase as i32 * 510 + 31) / 63 - 255) as i16;
        phase += 1;
    }
    table
};

/// Waveform 2: a square wave — 32 entries of 0 followed by 32 entries of 255.
///
/// **Accuracy-policy deviation D1** (`plans/product/03-accuracy-policy.md` §3). The
/// original `Vib_Pulse_Table` (`STARPLAY/S3MLIB.ASM` ~456) is **62 entries** — 31 zeros
/// then 31 × 255 — while the phase counter runs 0..63. Phases 62 and 63 therefore read
/// past the end of the table into the first two entries of `Vib_Rand_Table`, returning
/// 105 and 17 instead of 255. That is an out-of-bounds read, so this table is a full,
/// correct 64-entry square wave.
///
/// The zero half comes first, matching the original's ordering.
pub const VIB_PULSE_TABLE: [i16; WAVEFORM_TABLE_LEN] = {
    let mut table = [0i16; WAVEFORM_TABLE_LEN];
    let mut phase = WAVEFORM_TABLE_LEN / 2;
    while phase < WAVEFORM_TABLE_LEN {
        table[phase] = WAVEFORM_AMPLITUDE;
        phase += 1;
    }
    table
};

/// Waveform 3: a fixed pseudo-random table, amplitude ±255.
///
/// `Vib_Rand_Table` — `STARPLAY/S3MLIB.ASM` ~463. The original declares 128 entries, but
/// the second 64 are a verbatim copy of the first and the phase counter only ever reaches
/// 63, so only the first 64 are reproduced here. It is a *table*, not a generator, which
/// is what makes waveform 3 deterministic and therefore golden-testable.
pub const VIB_RANDOM_TABLE: [i16; WAVEFORM_TABLE_LEN] = [
     105,   17,   40, -108, -102,  140, -249,  133,
     161,  107, -233,  -45,  185,  148,  -65,  236,
     190, -228,  230,  -70,   12,  136, -229,   47,
     -17, -104,   62,   75, -121, -113,  168,  166,
      45,  248,  210, -140,   99,  245, -132,   17,
    -202,  255,   90, -248,   38, -205, -204,  153,
    -111, -233, -105,  -61, -102,  229,  245,  -51,
    -114, -174, -173,   75,  -47,  -45,  108,  -89,
];

/// The four waveform tables in the original's order, indexable by the low two bits of an
/// `S3x` / `S4x` waveform selector (`Vibrato_Tables`, `STARPLAY/S3MLIB.ASM` ~435).
pub const WAVEFORM_TABLES: [&[i16; WAVEFORM_TABLE_LEN]; 4] = [
    &VIB_SINE_TABLE,
    &VIB_RAMP_TABLE,
    &VIB_PULSE_TABLE,
    &VIB_RANDOM_TABLE,
];

/// Sample one of the four waveforms.
///
/// `selector` is masked to two bits and `phase` to six, so this can never index out of
/// bounds — which is precisely the defect D1 records.
pub const fn waveform_sample(selector: u8, phase: u8) -> i16 {
    WAVEFORM_TABLES[(selector & 0b11) as usize][(phase as usize) % WAVEFORM_TABLE_LEN]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn st3_constants_match_the_assembly() {
        assert_eq!(ST3_C4_SPEED, 8363);
        assert_eq!(ST3_PERIOD_SCALE, 133808, "8363 * 16, the `imul eax,eax,8363*16` in PeriodFromNote");
        assert_eq!(ST3_FREQUENCY_NUMERATOR, 0x00DA_7600, "0DA7600h, the PeriodToPitch numerator");
        assert_ne!(ST3_FREQUENCY_NUMERATOR, ST3_C4_SPEED * PERIOD_TABLE[0] as u32, "the reference doc's `8363 * 1712` identity is wrong");
        assert_eq!(ST3_C4_SPEED * PERIOD_TABLE[0] as u32 - ST3_FREQUENCY_NUMERATOR, 400, "ST3's constant is 400 low");
    }

    #[test]
    fn period_table_is_the_st3_table() {
        assert_eq!(PERIOD_TABLE.len(), 12);
        assert_eq!(PERIOD_TABLE[0], 1712);
        assert_eq!(PERIOD_TABLE[11], 907);
        assert_eq!(period_for_semitone(0), Period(1712));
        assert_eq!(period_for_semitone(12), Period(1712), "the semitone index wraps within the octave");
        for window in PERIOD_TABLE.windows(2) {
            assert!(window[1] < window[0], "periods must fall as pitch rises: {window:?}");
        }
    }

    #[test]
    fn octave_four_period_identity_holds() {
        // PeriodFromNote at octave 4 with the default C4 speed returns the table entry
        // unchanged: (1712 * 8363 * 16 >> 0) / 8363 == 1712.
        let period = ((PERIOD_TABLE[0] as u64 * ST3_PERIOD_SCALE as u64) >> 4) / ST3_C4_SPEED as u64;
        assert_eq!(period, 1712);
        // …and the same period converts back to *almost* the C4 speed: ST3's frequency
        // numerator is 400 low, so C-4 comes back as 8362 Hz rather than 8363.
        assert_eq!(ST3_FREQUENCY_NUMERATOR as u64 / period, 8362);
        // Octave 4 is Amiga 428 * 4.
        assert_eq!(Period(period as u32).to_amiga(), 428);
    }

    /// The regression test for accuracy-policy deviation D1: the original pulse table was
    /// 62 entries long and read out of bounds at phases 62 and 63.
    #[test]
    fn every_waveform_table_has_sixty_four_entries() {
        assert_eq!(VIB_SINE_TABLE.len(), WAVEFORM_TABLE_LEN);
        assert_eq!(VIB_RAMP_TABLE.len(), WAVEFORM_TABLE_LEN);
        assert_eq!(VIB_PULSE_TABLE.len(), WAVEFORM_TABLE_LEN);
        assert_eq!(VIB_RANDOM_TABLE.len(), WAVEFORM_TABLE_LEN);
        for (index, table) in WAVEFORM_TABLES.iter().enumerate() {
            assert_eq!(table.len(), WAVEFORM_TABLE_LEN, "waveform table {index} is the wrong length");
        }
    }

    #[test]
    fn waveform_tables_stay_within_the_amplitude() {
        for (index, table) in WAVEFORM_TABLES.iter().enumerate() {
            for (phase, &value) in table.iter().enumerate() {
                assert!(value.abs() <= WAVEFORM_AMPLITUDE, "table {index} phase {phase} is {value}, outside ±255");
            }
        }
    }

    #[test]
    fn sine_table_is_the_original_sine() {
        assert_eq!(VIB_SINE_TABLE[0], 0);
        assert_eq!(VIB_SINE_TABLE[16], 255);
        assert_eq!(VIB_SINE_TABLE[32], 0);
        assert_eq!(VIB_SINE_TABLE[48], -255);
        for phase in 1..32 {
            assert_eq!(VIB_SINE_TABLE[phase + 32], -VIB_SINE_TABLE[phase], "the sine's second half is the negated first");
        }
    }

    /// The regression test for accuracy-policy deviation D5.
    #[test]
    fn ramp_table_is_regular() {
        assert_eq!(VIB_RAMP_TABLE[0], -255);
        assert_eq!(VIB_RAMP_TABLE[63], 255);
        for phase in 0..WAVEFORM_TABLE_LEN {
            assert_eq!(VIB_RAMP_TABLE[63 - phase], -VIB_RAMP_TABLE[phase], "the ramp must be antisymmetric at phase {phase}");
        }
        for phase in 0..63 {
            let step = VIB_RAMP_TABLE[phase + 1] - VIB_RAMP_TABLE[phase];
            assert!(step == 8 || step == 9, "ramp step at phase {phase} is {step}, not the regular 8 or 9");
        }
        assert!(!VIB_RAMP_TABLE.contains(&8), "the original's skipped value is simply absent from a regular ramp");
    }

    #[test]
    fn pulse_table_is_a_full_square_wave() {
        for (phase, &value) in VIB_PULSE_TABLE.iter().enumerate().take(32) {
            assert_eq!(value, 0, "phase {phase} is in the zero half of the square wave");
        }
        for (phase, &value) in VIB_PULSE_TABLE.iter().enumerate().skip(32) {
            assert_eq!(value, 255, "phase {phase} is in the full-amplitude half of the square wave");
        }
        assert_eq!(VIB_PULSE_TABLE[62], 255, "D1: the original read 105 here, out of bounds");
        assert_eq!(VIB_PULSE_TABLE[63], 255, "D1: the original read 17 here, out of bounds");
    }

    #[test]
    fn waveform_sample_cannot_index_out_of_bounds() {
        assert_eq!(waveform_sample(0, 16), 255);
        assert_eq!(waveform_sample(2, 63), 255);
        assert_eq!(waveform_sample(3, 0), 105);
        assert_eq!(waveform_sample(7, 0), waveform_sample(3, 0), "the selector is masked to two bits");
        assert_eq!(waveform_sample(0, 255), VIB_SINE_TABLE[255 % WAVEFORM_TABLE_LEN]);
    }
}
