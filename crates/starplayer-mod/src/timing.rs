//! CIA-versus-VBlank evidence: the cheap facts the loader reads out of a MOD, and the
//! verdict they imply.
//!
//! A MOD does not say which interrupt its tracker's replayer ran off. On the CIA timer an
//! `Fxx` of 32 or more is a tempo in beats per minute; on the vertical blank there is no
//! timer to set and the same byte is simply a long row. Getting it wrong is not subtle —
//! a fermata written as `F20` on a VBlank tracker plays the rest of the song at 32 BPM.
//!
//! libxmp is the oracle the pinned corpus was generated with, so its rules are the
//! specification. This module is `src/loaders/mod_load.c:816-950` split in two: the facts
//! ([`ModTimingEvidence`], gathered by the loader while it reads the patterns) and the
//! decision ([`timing_verdict`]). The third of libxmp's answers, "scan it both ways and
//! keep the shorter", needs a sequencer and therefore lives in the `starplayer` facade;
//! [`TimingVerdict::CompareLengths`] is how this module asks for it.

use starplayer_model::{Module, ModuleFormat};

/// What a MOD's tag, samples and pattern cells say about its timing.
///
/// Every field mirrors one of libxmp's loader-local variables, named after it where the
/// name is intelligible. Gathered once, in `load`, from data the loader is already
/// walking; stored in [`ModuleHeader::format_extra`](starplayer_model::ModuleHeader) and
/// read back with [`timing_evidence`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModTimingEvidence {
    /// libxmp's `needs_timing_detection`: the pattern evidence below is consulted at all.
    ///
    /// Set for the `M.K.` and `M!K!` tags only (`mod_load.c:530-531`) and cleared again
    /// when a sample header declares 32768 words or more, which no Amiga tracker could
    /// write and libxmp reads as an OpenMPT file (`mod_load.c:637-642`). Every other tag
    /// is a tracker whose timing libxmp considers already known.
    pub timing_detection: bool,
    /// The tag names a tracker with no CIA mode at all — `M&K!` or `N.T.`, libxmp's
    /// `TRACKER_NOISETRACKER` (`mod_magic[]` and `tracker_is_vblank`).
    pub vblank_only_tag: bool,
    /// Some cell carries an `Fxx` with `xx >= 0x20`, libxmp's `high_fxx`.
    pub has_high_fxx: bool,
    /// Some row carries both an `Fxx` below `0x20` and one at or above it, across its
    /// channels — libxmp's `samerow_fxx`. Strong evidence of a CIA tracker, because only
    /// a CIA tracker has two different things for those bytes to mean.
    pub mixed_row: bool,
    /// libxmp's end-silence rule: every high `Fxx` in the song is in a pattern the last
    /// two orders play, and the last such value is not `0x7D`. Reproduced from
    /// `mod_load.c:921-949`; see [`high_fxx_only_at_end`].
    pub high_fxx_only_at_end: bool,
}

/// The three answers libxmp's loader can reach, in the vocabulary of this crate.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum TimingVerdict {
    /// Play it on the CIA rule: `Fxx` below 32 is a speed, 32 and above is a BPM.
    #[default]
    Cia,
    /// Play it off the vertical blank: every non-zero `Fxx` is a speed.
    VBlank,
    /// The evidence is ambiguous. Scan the song both ways and keep the shorter — libxmp
    /// `src/scan.c:671-708`, and the facade's `scan_song` is what does it.
    CompareLengths,
}

/// The verdict `evidence` implies, mirroring libxmp `mod_load.c:897-950` branch for
/// branch.
///
/// The order matters and is libxmp's: the timing-detection gate first, because a tag that
/// already names its tracker is never second-guessed by the pattern evidence; then the
/// mixed row, which cancels everything after it; then the end-silence rule; then the bare
/// presence of a high `Fxx`, which is what asks for the length comparison.
pub const fn timing_verdict(evidence: ModTimingEvidence) -> TimingVerdict {
    if !evidence.timing_detection {
        // libxmp: `if (!needs_timing_detection) { if (tracker_is_vblank(id)) NOBPM;
        // compare_vblank = 0; }` — no comparison and no pattern evidence either way.
        if evidence.vblank_only_tag { TimingVerdict::VBlank } else { TimingVerdict::Cia }
    } else if evidence.mixed_row {
        TimingVerdict::Cia
    } else if evidence.high_fxx_only_at_end {
        TimingVerdict::VBlank
    } else if evidence.has_high_fxx {
        TimingVerdict::CompareLengths
    } else {
        TimingVerdict::Cia
    }
}

/// The evidence a MOD was loaded with, or `None` for a module some other loader built.
pub fn timing_evidence(module: &Module) -> Option<ModTimingEvidence> {
    let header = module.header();
    (header.format == ModuleFormat::Mod).then(|| decode_evidence(header.format_extra))
}

/// The verdict for a loaded MOD. `None` for a module some other loader built.
pub fn timing_verdict_for(module: &Module) -> Option<TimingVerdict> {
    timing_evidence(module).map(timing_verdict)
}

// ── the `format_extra` word ────────────────────────────────────────────────────────────
//
// One bit per fact, in the low five bits. MOD has no other claim on the word, and the
// bits are private to this crate: nothing outside it may read `format_extra` for a MOD.

const TIMING_DETECTION: u32 = 1 << 0;
const VBLANK_ONLY_TAG: u32 = 1 << 1;
const HAS_HIGH_FXX: u32 = 1 << 2;
const MIXED_ROW: u32 = 1 << 3;
const HIGH_FXX_ONLY_AT_END: u32 = 1 << 4;

/// Pack the evidence into the header word the loader stores.
pub(crate) const fn encode_evidence(evidence: ModTimingEvidence) -> u32 {
    let mut word = 0;
    if evidence.timing_detection { word |= TIMING_DETECTION; }
    if evidence.vblank_only_tag { word |= VBLANK_ONLY_TAG; }
    if evidence.has_high_fxx { word |= HAS_HIGH_FXX; }
    if evidence.mixed_row { word |= MIXED_ROW; }
    if evidence.high_fxx_only_at_end { word |= HIGH_FXX_ONLY_AT_END; }
    word
}

/// The inverse of [`encode_evidence`].
pub(crate) const fn decode_evidence(word: u32) -> ModTimingEvidence {
    ModTimingEvidence {
        timing_detection: word & TIMING_DETECTION != 0,
        vblank_only_tag: word & VBLANK_ONLY_TAG != 0,
        has_high_fxx: word & HAS_HIGH_FXX != 0,
        mixed_row: word & MIXED_ROW != 0,
        high_fxx_only_at_end: word & HIGH_FXX_ONLY_AT_END != 0,
    }
}

/// libxmp's end-silence rule, transcribed from `mod_load.c:921-949`.
///
/// `last_high_fxx[pattern]` is the **last** `Fxx >= 0x20` parameter that pattern contains
/// in cell order, or zero if it has none — libxmp's `pat_high_fxx[]`. `orders` is the
/// played part of the order list, as pattern numbers.
///
/// The rule: a song of at least eight orders whose high `Fxx` values all sit in patterns
/// only the last two orders play is a module with silence added at the end, which is a
/// VBlank idiom. The exception is a final value of `0x7D` — 125, the CIA default — which
/// means the file was written or converted to play as CIA after all.
pub(crate) fn high_fxx_only_at_end(orders: &[u16], last_high_fxx: &[u8]) -> bool {
    let high_at = |order: u16| last_high_fxx.get(order as usize).copied().unwrap_or(0);
    let length = orders.len();
    if length < 8 { return false; }
    let threshold = length - 2;
    // `for (i = 0; i < threshold; i++) if (pat_high_fxx[xxo[i]]) break; if (i != threshold) skip`
    if orders.iter().take(threshold).any(|order| high_at(*order) != 0) { return false; }
    // `for (i = len - 1; i >= threshold; i--)`: the last order with a high Fxx decides,
    // and `0x7D` there means CIA.
    for order in orders.iter().skip(threshold).rev() {
        match high_at(*order) {
            0 => continue,
            0x7D => return false,
            _ => return true,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const DETECTED: ModTimingEvidence = ModTimingEvidence { timing_detection: true, vblank_only_tag: false, has_high_fxx: false, mixed_row: false, high_fxx_only_at_end: false };

    #[test]
    fn a_vblank_only_tag_wins_outright_and_a_known_tracker_is_never_second_guessed() {
        let noisetracker = ModTimingEvidence { vblank_only_tag: true, has_high_fxx: true, ..ModTimingEvidence::default() };
        assert_eq!(timing_verdict(noisetracker), TimingVerdict::VBlank);
        // libxmp reaches the samerow demotion only under `needs_timing_detection`, which
        // a NoiseTracker tag never sets, so a mixed row does not cancel the tag.
        assert_eq!(timing_verdict(ModTimingEvidence { mixed_row: true, ..noisetracker }), TimingVerdict::VBlank);
        // An Octalyser or Digital Tracker file: high Fxx everywhere, no detection, no tag.
        assert_eq!(timing_verdict(ModTimingEvidence { has_high_fxx: true, ..ModTimingEvidence::default() }), TimingVerdict::Cia);
    }

    #[test]
    fn a_mixed_row_cancels_the_comparison_and_the_end_rule() {
        assert_eq!(timing_verdict(ModTimingEvidence { has_high_fxx: true, mixed_row: true, high_fxx_only_at_end: true, ..DETECTED }), TimingVerdict::Cia);
    }

    #[test]
    fn the_end_silence_rule_needs_no_comparison_and_a_bare_high_fxx_asks_for_one() {
        assert_eq!(timing_verdict(ModTimingEvidence { has_high_fxx: true, high_fxx_only_at_end: true, ..DETECTED }), TimingVerdict::VBlank);
        assert_eq!(timing_verdict(ModTimingEvidence { has_high_fxx: true, ..DETECTED }), TimingVerdict::CompareLengths);
        assert_eq!(timing_verdict(DETECTED), TimingVerdict::Cia, "no high Fxx at all is plain CIA");
    }

    #[test]
    fn the_evidence_word_round_trips_every_bit() {
        for word in 0..32u32 {
            assert_eq!(encode_evidence(decode_evidence(word)), word);
        }
        assert_eq!(decode_evidence(0), ModTimingEvidence::default());
    }

    #[test]
    fn the_end_silence_loop_matches_libxmps_own() {
        let mut last_high = [0u8; 8];
        // Eight orders, the high Fxx only in the pattern the last order plays.
        let orders: [u16; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
        last_high[7] = 0x30;
        assert!(high_fxx_only_at_end(&orders, &last_high));

        // 0x7D as the final high value means the file was made to play as CIA.
        last_high[7] = 0x7D;
        assert!(!high_fxx_only_at_end(&orders, &last_high), "F7D at the end is CIA, not VBlank");

        // The backwards loop stops at the *last* order with a high value, so a 0x7D there
        // shadows a legitimate value in the second-to-last order.
        last_high[6] = 0x30;
        assert!(!high_fxx_only_at_end(&orders, &last_high));
        last_high[7] = 0;
        assert!(high_fxx_only_at_end(&orders, &last_high), "with the 0x7D gone the earlier order decides");

        // A high Fxx anywhere in the first len-2 orders disqualifies the whole rule.
        last_high[0] = 0x21;
        assert!(!high_fxx_only_at_end(&orders, &last_high));

        // Fewer than eight orders never qualifies.
        assert!(!high_fxx_only_at_end(&[0, 1, 2, 3, 4, 5, 6], &[0, 0, 0, 0, 0, 0, 0x30]));
    }
}
