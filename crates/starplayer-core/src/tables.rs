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
//!
//! [`LINEAR_FREQUENCY_TABLE`] and the IT linear-slide helpers below it are not from the
//! original assembly — MOD, S3M and MTM derive pitch from Amiga periods and never need a
//! `2^x` — but they belong here rather than in `starplayer-xm` or `starplayer-it` because
//! both formats need exactly the same table (task E2) and a shared no-`std` crate is the
//! one place a merge conflict between the two loaders is avoided outright.

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

// ---------------------------------------------------------------------------------------
// Linear frequency (XM) and linear slides (IT) — task E2.
// ---------------------------------------------------------------------------------------

/// Fractional bits of the fixed-point working precision [`exp2_fraction_q60`] computes in.
///
/// Q0.60 leaves comfortable headroom in `i128` for the widened multiply every term of the
/// series needs — the same idea as `mul_q30` / `sin_q30` in `starplayer-mixer`'s
/// `gain.rs`, just wider, because `2^x` needs more terms to converge than the `sin`
/// series does over its narrower quarter-turn range (research point 1).
const EXP2_WORKING_FRACTION_BITS: u32 = 60;

/// `ln(2)` in Q0.60, rounded to nearest: `0.69314718055994530941723212145818… × 2^60`.
///
/// Used only inside [`exp2_fraction_q60`]'s const evaluation, and only there — this is
/// the one transcendental *value* anywhere in this module, folded to a constant at
/// StarPlayer's own build time rather than computed by a libm call at anyone else's.
const LN2_Q60: i128 = 799_144_290_325_165_979;

/// Terms of the `exp` Taylor series [`exp2_fraction_q60`] sums.
///
/// Twelve is the minimum that reproduces every one of [`LINEAR_FREQUENCY_TABLE`]'s 768
/// entries, and every entry of the four IT slide tables below, exactly — checked by
/// brute-force search over the term count before this constant was chosen (research point
/// 1). Fourteen leaves margin without costing anything measurable, since the whole
/// computation happens once, in the const evaluator, never at run time.
const EXP2_SERIES_TERMS: i128 = 14;

/// `2^(numerator/denominator)` in Q0.60, by a fixed-point Taylor series for `exp`:
/// `2^x = exp(x·ln2)` and `exp(z) = Σ zⁿ⁄n!`.
///
/// `numerator` may be negative — the IT slide-down helpers need `2^(−x)`, and the series
/// converges the same way for a negative `z` as a positive one. `denominator` must be
/// positive. Every operation is integer arithmetic in `i128`, so this is exact,
/// reproducible fixed-point computation, not a call to a libm `exp2` — architecture §7.3
/// bans the latter from ever running on a caller's machine, and this function is `const`,
/// so it only ever runs on StarPlayer's own build host, folded into the binary as a
/// literal table by the time anything links against it (research point 1).
const fn exp2_fraction_q60(numerator: i32, denominator: i32) -> i128 {
    let one = 1i128 << EXP2_WORKING_FRACTION_BITS;
    let z = (LN2_Q60 * numerator as i128) / denominator as i128;
    let mut term = one;
    let mut sum = one;
    let mut order = 1i128;
    while order < EXP2_SERIES_TERMS {
        term = ((term * z) >> EXP2_WORKING_FRACTION_BITS) / order;
        sum += term;
        order += 1;
    }
    sum
}

/// `round(2^fraction_bits × 2^(numerator/denominator))`, computed via
/// [`exp2_fraction_q60`] and rounded to nearest. Every value this module's tables hold is
/// positive, so "nearest" needs no tie-breaking rule beyond ordinary rounding.
const fn round_exp2_to_q(numerator: i32, denominator: i32, fraction_bits: u32) -> u32 {
    let value = exp2_fraction_q60(numerator, denominator);
    let shift = EXP2_WORKING_FRACTION_BITS - fraction_bits;
    let half = 1i128 << (shift - 1);
    ((value + half) >> shift) as u32
}

/// Entries in [`LINEAR_FREQUENCY_TABLE`]: one per 1/768th of an octave, matching FT2's
/// `logTab[4 * 12 * 16]` layout (four octaves' worth of 1/16-semitone finetune steps
/// across twelve semitones).
pub const LINEAR_FREQUENCY_TABLE_LEN: usize = 768;

/// `2^(i/768)` in Q8.24, for `i` in `0..768` — entry-for-entry equal to FT2-clone's
/// `logTab` (`src/ft2_replayer.c` line 32 declares `logTab[4*12*16]`; line 2909 fills it
/// with `logTab[i] = (uint32_t)round(16777216.0 * exp2(i * (1.0 / 768.0)))`).
///
/// XM's linear-frequency mode and IT's linear slides both need `2^x` for a fractional
/// `x`, and design goal 5 bans a transcendental function anywhere outside a test — so this
/// is a table, computed once at StarPlayer's own build time by [`round_exp2_to_q`]
/// (research point 1 chose a `const fn` over a literal: fourteen Taylor terms reproduce
/// every one of these 768 entries exactly, so there was never a rounding-boundary
/// disagreement with FT2's own runtime-computed table to paper over with a hand-pasted
/// literal). `#[cfg(test)]` below checks all 768 entries against `f64::exp2`.
pub const LINEAR_FREQUENCY_TABLE: [u32; LINEAR_FREQUENCY_TABLE_LEN] = build_linear_frequency_table();

const fn build_linear_frequency_table() -> [u32; LINEAR_FREQUENCY_TABLE_LEN] {
    let mut table = [0u32; LINEAR_FREQUENCY_TABLE_LEN];
    let mut index = 0;
    while index < LINEAR_FREQUENCY_TABLE_LEN {
        table[index] = round_exp2_to_q(index as i32, LINEAR_FREQUENCY_TABLE_LEN as i32, 24);
        index += 1;
    }
    table
}

/// Read [`LINEAR_FREQUENCY_TABLE`] the way FT2 reads `logTab`: `units` is a period-like
/// value in 1/768ths of an octave, split into an octave and a table index, and the table
/// entry is shifted down by the octave.
///
/// `logTab[period % 768] >> ((14 − period / 768) & 31)` — `src/ft2_replayer.c`
/// `period2Ft2Delta`, lines 250–257 (`quotient = invPeriod / 768`,
/// `remainder = invPeriod % 768`, `shiftValue = (14 - quotient) & 31`). `14` and the `& 31`
/// mask are FT2's own constants: the mask is what makes a shift count that would
/// otherwise go negative for a very low octave wrap instead of panicking, matching the
/// `uint8_t` truncation of an unsigned `14 - quotient` underflow in the original C. This
/// function exists so the XM crate calls it once rather than carrying the idiom itself.
pub const fn linear_frequency_q24(units: u32) -> u32 {
    let octave = units / LINEAR_FREQUENCY_TABLE_LEN as u32;
    let index = (units % LINEAR_FREQUENCY_TABLE_LEN as u32) as usize;
    let shift = 14u32.wrapping_sub(octave) & 31;
    LINEAR_FREQUENCY_TABLE[index] >> shift
}

/// Entries in each IT linear-slide table below.
const LINEAR_SLIDE_TABLE_LEN: usize = 256;

/// Entries in each IT *fine* linear-slide table below.
const FINE_LINEAR_SLIDE_TABLE_LEN: usize = 16;

/// `round(65536 × 2^(n/192))` in Q16.16, `n` in `0..256` — OpenMPT's `LinearSlideUpTable`
/// (`soundlib/Tables.cpp` lines 540–579), transcribed verbatim rather than derived from
/// [`LINEAR_FREQUENCY_TABLE`].
///
/// Research point 2: `table[4n] >> 8` — reading the shared Q8.24 table at four times the
/// index and truncating down to Q16.16 — disagrees with this table at 125 of its 256
/// entries; even a *rounding* shift (`(table[4n] + 128) >> 8`) still disagrees at one
/// (`n = 107`: 96437 against OpenMPT's 96436). An independent Taylor-series evaluation at
/// each `n` (rather than a shift of the shared table) reproduces this table exactly, so
/// the mismatch is double rounding — Q8.24's own rounding, then the shift's — not a
/// disagreement about the underlying value. Either way this table is transcribed as
/// OpenMPT's own literal data, both because that is what the research point calls for
/// when the derivation disagrees and because [`FINE_LINEAR_SLIDE_DOWN_TABLE`] must be
/// transcribed regardless (see its own doc comment) — keeping all four IT tables sourced
/// the same way, rather than three computed and one pasted, is what makes them easy to
/// audit against `Tables.cpp` side by side.
const LINEAR_SLIDE_UP_TABLE: [u32; LINEAR_SLIDE_TABLE_LEN] = [
     65536,  65773,  66011,  66250,  66489,  66730,  66971,  67213,
     67456,  67700,  67945,  68191,  68438,  68685,  68933,  69183,
     69433,  69684,  69936,  70189,  70443,  70698,  70953,  71210,
     71468,  71726,  71985,  72246,  72507,  72769,  73032,  73297,
     73562,  73828,  74095,  74363,  74632,  74902,  75172,  75444,
     75717,  75991,  76266,  76542,  76819,  77096,  77375,  77655,
     77936,  78218,  78501,  78785,  79069,  79355,  79642,  79930,
     80220,  80510,  80801,  81093,  81386,  81681,  81976,  82273,
     82570,  82869,  83169,  83469,  83771,  84074,  84378,  84683,
     84990,  85297,  85606,  85915,  86226,  86538,  86851,  87165,
     87480,  87796,  88114,  88433,  88752,  89073,  89396,  89719,
     90043,  90369,  90696,  91024,  91353,  91684,  92015,  92348,
     92682,  93017,  93354,  93691,  94030,  94370,  94711,  95054,
     95398,  95743,  96089,  96436,  96785,  97135,  97487,  97839,
     98193,  98548,  98905,  99262,  99621,  99982, 100343, 100706,
    101070, 101436, 101803, 102171, 102540, 102911, 103283, 103657,
    104032, 104408, 104786, 105165, 105545, 105927, 106310, 106694,
    107080, 107468, 107856, 108246, 108638, 109031, 109425, 109821,
    110218, 110617, 111017, 111418, 111821, 112226, 112631, 113039,
    113448, 113858, 114270, 114683, 115098, 115514, 115932, 116351,
    116772, 117194, 117618, 118043, 118470, 118899, 119329, 119760,
    120194, 120628, 121065, 121502, 121942, 122383, 122825, 123270,
    123715, 124163, 124612, 125063, 125515, 125969, 126425, 126882,
    127341, 127801, 128263, 128727, 129193, 129660, 130129, 130600,
    131072, 131546, 132022, 132499, 132978, 133459, 133942, 134427,
    134913, 135401, 135890, 136382, 136875, 137370, 137867, 138366,
    138866, 139368, 139872, 140378, 140886, 141395, 141907, 142420,
    142935, 143452, 143971, 144491, 145014, 145539, 146065, 146593,
    147123, 147655, 148189, 148725, 149263, 149803, 150345, 150889,
    151434, 151982, 152532, 153083, 153637, 154193, 154750, 155310,
    155872, 156435, 157001, 157569, 158139, 158711, 159285, 159861,
    160439, 161019, 161602, 162186, 162773, 163361, 163952, 164545,
];

/// `round(65536 × 2^(−n/192))` in Q16.16, `n` in `0..256` — OpenMPT's
/// `LinearSlideDownTable` (`soundlib/Tables.cpp` lines 583–612), transcribed verbatim
/// alongside [`LINEAR_SLIDE_UP_TABLE`]. Unlike [`FINE_LINEAR_SLIDE_DOWN_TABLE`], every
/// entry here agrees with the pure formula — checked in `#[cfg(test)]` — but it is kept a
/// literal for the same reason its sibling tables are: one sourcing convention for all
/// four, trivially auditable against `Tables.cpp`.
const LINEAR_SLIDE_DOWN_TABLE: [u32; LINEAR_SLIDE_TABLE_LEN] = [
     65536,  65300,  65065,  64830,  64596,  64364,  64132,  63901,
     63670,  63441,  63212,  62984,  62757,  62531,  62306,  62081,
     61858,  61635,  61413,  61191,  60971,  60751,  60532,  60314,
     60097,  59880,  59664,  59449,  59235,  59022,  58809,  58597,
     58386,  58176,  57966,  57757,  57549,  57341,  57135,  56929,
     56724,  56519,  56316,  56113,  55911,  55709,  55508,  55308,
     55109,  54910,  54713,  54515,  54319,  54123,  53928,  53734,
     53540,  53347,  53155,  52963,  52773,  52582,  52393,  52204,
     52016,  51829,  51642,  51456,  51270,  51085,  50901,  50718,
     50535,  50353,  50172,  49991,  49811,  49631,  49452,  49274,
     49097,  48920,  48743,  48568,  48393,  48218,  48044,  47871,
     47699,  47527,  47356,  47185,  47015,  46846,  46677,  46509,
     46341,  46174,  46008,  45842,  45677,  45512,  45348,  45185,
     45022,  44859,  44698,  44537,  44376,  44216,  44057,  43898,
     43740,  43582,  43425,  43269,  43113,  42958,  42803,  42649,
     42495,  42342,  42189,  42037,  41886,  41735,  41584,  41434,
     41285,  41136,  40988,  40840,  40693,  40547,  40400,  40255,
     40110,  39965,  39821,  39678,  39535,  39392,  39250,  39109,
     38968,  38828,  38688,  38548,  38409,  38271,  38133,  37996,
     37859,  37722,  37586,  37451,  37316,  37181,  37047,  36914,
     36781,  36648,  36516,  36385,  36254,  36123,  35993,  35863,
     35734,  35605,  35477,  35349,  35221,  35095,  34968,  34842,
     34716,  34591,  34467,  34343,  34219,  34095,  33973,  33850,
     33728,  33607,  33486,  33365,  33245,  33125,  33005,  32887,
     32768,  32650,  32532,  32415,  32298,  32182,  32066,  31950,
     31835,  31720,  31606,  31492,  31379,  31266,  31153,  31041,
     30929,  30817,  30706,  30596,  30485,  30376,  30266,  30157,
     30048,  29940,  29832,  29725,  29618,  29511,  29405,  29299,
     29193,  29088,  28983,  28879,  28774,  28671,  28567,  28464,
     28362,  28260,  28158,  28056,  27955,  27855,  27754,  27654,
     27554,  27455,  27356,  27258,  27159,  27062,  26964,  26867,
     26770,  26674,  26577,  26482,  26386,  26291,  26196,  26102,
];

/// `round(65536 × 2^(n/768))` in Q16.16, `n` in `0..16` — OpenMPT's
/// `FineLinearSlideUpTable` (`soundlib/Tables.cpp` lines 516–521), transcribed verbatim.
/// Every entry agrees with the pure formula (checked in `#[cfg(test)]`); it is a literal
/// for the same one-sourcing-convention reason as its three siblings.
const FINE_LINEAR_SLIDE_UP_TABLE: [u32; FINE_LINEAR_SLIDE_TABLE_LEN] =
    [65536, 65595, 65654, 65714, 65773, 65832, 65892, 65951, 66011, 66071, 66130, 66190, 66250, 66309, 66369, 66429];

/// `round(65536 × 2^(−n/768))` in Q16.16, `n` in `0..16` — OpenMPT's
/// `FineLinearSlideDownTable` (`soundlib/Tables.cpp` lines 524–534), transcribed verbatim
/// **including three values that are wrong under the formula in its own doc comment**.
/// OpenMPT's comment records them as coming "straight from Impulse Tracker's source":
///
/// * entry 0 is `65535`, not `65536` (unity; OpenMPT's comment guesses this is
///   deliberate, so the value still fits a 16-bit integer — entry 0 is never read, since
///   a slide of zero steps changes nothing anyway);
/// * entry 11 is `64888`, not `64889` — OpenMPT's comment calls it a rounding error;
/// * entry 15 is `64645`, not `64655` — OpenMPT's comment calls it a typo.
///
/// These are not OpenMPT's mistake to fix: they are what real Impulse Tracker actually
/// ships and actually plays, so accuracy policy §0's rule — "the format specifications
/// and OpenMPT's documented compatibility behaviour are the reference" for XM/IT — makes
/// reproducing them, typos included, the canonical behaviour rather than a deviation.
/// This is also decisive for research point 2: no formula, and no derivation from
/// [`LINEAR_FREQUENCY_TABLE`], can produce these three values, so a literal table is not
/// optional for this one entry in the family — and once one of the four must be a
/// literal, all four are, for one consistent sourcing story.
const FINE_LINEAR_SLIDE_DOWN_TABLE: [u32; FINE_LINEAR_SLIDE_TABLE_LEN] =
    [65535, 65477, 65418, 65359, 65300, 65241, 65182, 65123, 65065, 65006, 64947, 64888, 64830, 64772, 64713, 64645];

/// `2^(steps/192)` in Q16.16, `steps` in `0..=255` — a coarse IT linear-slide step.
pub const fn linear_slide_up_q16(steps: u8) -> u32 { LINEAR_SLIDE_UP_TABLE[steps as usize] }

/// `2^(−steps/192)` in Q16.16, `steps` in `0..=255` — a coarse IT linear-slide step.
pub const fn linear_slide_down_q16(steps: u8) -> u32 { LINEAR_SLIDE_DOWN_TABLE[steps as usize] }

/// `2^(steps/768)` in Q16.16, `steps` in `0..=15` — an IT *fine* linear-slide step.
/// `steps` is masked to the table's sixteen entries the way [`waveform_sample`] masks its
/// phase, since IT's fine-slide parameter is a single nibble and can never carry more.
pub const fn fine_linear_slide_up_q16(steps: u8) -> u32 {
    FINE_LINEAR_SLIDE_UP_TABLE[(steps as usize) % FINE_LINEAR_SLIDE_TABLE_LEN]
}

/// `2^(−steps/768)` in Q16.16, `steps` in `0..=15` — an IT *fine* linear-slide step.
pub const fn fine_linear_slide_down_q16(steps: u8) -> u32 {
    FINE_LINEAR_SLIDE_DOWN_TABLE[(steps as usize) % FINE_LINEAR_SLIDE_TABLE_LEN]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Design goal 5 bans a transcendental function anywhere in `render()`'s path, but
    // this crate is unconditionally `#![no_std]` — even under `cargo test` — so `f64`'s
    // libm-backed methods (`exp2` among them) are not in scope without pulling `std` in
    // explicitly. This is that pull, and it reaches no further than this test module: the
    // production build of this crate, and everything that depends on it, never sees
    // `std`. Research point 1's verification test, below, is what this exists for.
    extern crate std;

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

    /// Research point 1: every one of [`LINEAR_FREQUENCY_TABLE`]'s 768 entries against
    /// `f64::exp2` — the same formula FT2-clone's `calcMiscReplayerVars` computes `logTab`
    /// with at its own startup (`round(16777216.0 * exp2(i * (1.0 / 768.0)))`), so this is
    /// not a check against a second implementation of the same idea, it is a check against
    /// the reference's own arithmetic.
    #[test]
    fn linear_frequency_table_matches_ft2s_logtab_formula() {
        assert_eq!(LINEAR_FREQUENCY_TABLE.len(), LINEAR_FREQUENCY_TABLE_LEN);
        for (index, &entry) in LINEAR_FREQUENCY_TABLE.iter().enumerate() {
            let reference = (16_777_216.0 * f64::exp2(index as f64 / 768.0)).round() as u32;
            assert_eq!(entry, reference, "entry {index}");
        }
        assert_eq!(LINEAR_FREQUENCY_TABLE[0], 1 << 24, "2^0 is exactly unity in Q8.24");
        assert_eq!(LINEAR_FREQUENCY_TABLE[767], 33_524_162, "2^(767/768), one table step short of doubling");
    }

    #[test]
    fn linear_frequency_q24_reproduces_ft2s_shift_idiom() {
        // FT2's `14` is the octave that reads the table unshifted: `shiftValue = (14 -
        // quotient) & 31` is zero exactly when `quotient` (our `octave`) is 14.
        assert_eq!(linear_frequency_q24(14 * LINEAR_FREQUENCY_TABLE_LEN as u32), LINEAR_FREQUENCY_TABLE[0]);
        assert_eq!(linear_frequency_q24(14 * LINEAR_FREQUENCY_TABLE_LEN as u32 + 383), LINEAR_FREQUENCY_TABLE[383]);
        // One octave lower halves the result — a pitch an octave down is half the
        // frequency — because the shift grows by exactly one.
        assert_eq!(linear_frequency_q24(13 * LINEAR_FREQUENCY_TABLE_LEN as u32), LINEAR_FREQUENCY_TABLE[0] >> 1);
        assert_eq!(linear_frequency_q24(12 * LINEAR_FREQUENCY_TABLE_LEN as u32), LINEAR_FREQUENCY_TABLE[0] >> 2);
        // A very high `units` pushes `octave` past 14 far enough that `14 - octave`
        // underflows and `& 31` wraps the shift back into `0..32` rather than panicking:
        // this must not panic, whatever it returns.
        let _ = linear_frequency_q24(u32::MAX);
    }

    /// Research point 2: independent verification of every IT slide table against the
    /// pure formula, `f64::exp2`-checked the same way as [`LINEAR_FREQUENCY_TABLE`]. The
    /// three [`FINE_LINEAR_SLIDE_DOWN_TABLE`] entries that disagree are the whole reason
    /// that table is a literal rather than a computed one; this test pins them by name so
    /// a future edit cannot silently "fix" them back to the formula.
    #[test]
    fn linear_slide_tables_match_the_formula_except_where_it_documents_a_typo() {
        for (steps, &entry) in LINEAR_SLIDE_UP_TABLE.iter().enumerate() {
            let reference = (65_536.0 * f64::exp2(steps as f64 / 192.0)).round() as u32;
            assert_eq!(entry, reference, "linear slide up, steps {steps}");
        }
        for (steps, &entry) in LINEAR_SLIDE_DOWN_TABLE.iter().enumerate() {
            let reference = (65_536.0 * f64::exp2(-(steps as f64) / 192.0)).round() as u32;
            assert_eq!(entry, reference, "linear slide down, steps {steps}");
        }
        for (steps, &entry) in FINE_LINEAR_SLIDE_UP_TABLE.iter().enumerate() {
            let reference = (65_536.0 * f64::exp2(steps as f64 / 768.0)).round() as u32;
            assert_eq!(entry, reference, "fine linear slide up, steps {steps}");
        }
        for (steps, &entry) in FINE_LINEAR_SLIDE_DOWN_TABLE.iter().enumerate() {
            let reference = (65_536.0 * f64::exp2(-(steps as f64) / 768.0)).round() as u32;
            if steps == 0 || steps == 11 || steps == 15 {
                assert_ne!(entry, reference, "steps {steps} is one of Impulse Tracker's own three typo'd entries — it must NOT match the formula");
            } else {
                assert_eq!(entry, reference, "fine linear slide down, steps {steps}");
            }
        }
        assert_eq!(FINE_LINEAR_SLIDE_DOWN_TABLE[0], 65_535, "IT's own value, one short of unity — see the table's doc comment");
        assert_eq!(FINE_LINEAR_SLIDE_DOWN_TABLE[11], 64_888, "IT's own rounding error");
        assert_eq!(FINE_LINEAR_SLIDE_DOWN_TABLE[15], 64_645, "IT's own typo");
    }

    #[test]
    fn linear_slide_accessors_read_the_tables_they_are_named_for() {
        assert_eq!(linear_slide_up_q16(0), 65_536);
        assert_eq!(linear_slide_up_q16(255), LINEAR_SLIDE_UP_TABLE[255]);
        assert_eq!(linear_slide_down_q16(0), 65_536);
        assert_eq!(linear_slide_down_q16(255), LINEAR_SLIDE_DOWN_TABLE[255]);
        assert_eq!(fine_linear_slide_up_q16(0), 65_536);
        assert_eq!(fine_linear_slide_up_q16(15), FINE_LINEAR_SLIDE_UP_TABLE[15]);
        assert_eq!(fine_linear_slide_down_q16(0), 65_535, "IT's own typo, not unity");
        assert_eq!(fine_linear_slide_down_q16(15), FINE_LINEAR_SLIDE_DOWN_TABLE[15]);
    }

    #[test]
    fn fine_linear_slide_accessors_cannot_index_out_of_bounds() {
        assert_eq!(fine_linear_slide_up_q16(16), fine_linear_slide_up_q16(0), "steps is masked to the table's sixteen entries");
        assert_eq!(fine_linear_slide_down_q16(255), fine_linear_slide_down_q16(255 % 16));
    }

    /// Every table this module builds at compile time is monotonic: pitch rises with the
    /// index for the "up" tables and falls for the "down" ones, since each is a `2^x` for
    /// a monotonically changing `x`. This would catch a sign error or a swapped
    /// numerator/denominator in [`exp2_fraction_q60`]'s callers that the exact-value tests
    /// above might not, if the mistake happened to still land on an integer by chance.
    #[test]
    fn every_new_table_is_monotonic() {
        for window in LINEAR_FREQUENCY_TABLE.windows(2) {
            assert!(window[1] > window[0], "linear frequency table must rise: {window:?}");
        }
        for window in LINEAR_SLIDE_UP_TABLE.windows(2) {
            assert!(window[1] > window[0], "linear slide up must rise: {window:?}");
        }
        for window in LINEAR_SLIDE_DOWN_TABLE.windows(2) {
            assert!(window[1] < window[0], "linear slide down must fall: {window:?}");
        }
        for window in FINE_LINEAR_SLIDE_UP_TABLE.windows(2) {
            assert!(window[1] > window[0], "fine linear slide up must rise: {window:?}");
        }
        // Even with IT's own three typo'd entries (see FINE_LINEAR_SLIDE_DOWN_TABLE's doc
        // comment), the table is still strictly falling throughout: entry 0 is one short
        // of unity rather than exactly unity, but it is still the largest entry, and the
        // other two typos are each still below their predecessor.
        for window in FINE_LINEAR_SLIDE_DOWN_TABLE.windows(2) {
            assert!(window[1] < window[0], "fine linear slide down must fall: {window:?}");
        }
    }
}
