//! [`PatternIndex`] — where a pattern's **native** bytes live in the module blob — and
//! the display-only [`PatternCell`] view a UI renders.
//!
//! # Why there is no shared executable cell type
//!
//! The model stores each format's pattern data exactly as the file spells it and does not
//! know its layout: S3M's packed channel-mask stream, ProTracker's fixed four-byte cells
//! and MTM's three-byte cells stay as they are, and each format crate decodes its own.
//! The original converted MOD and MTM into S3M before the player ever saw them, and that
//! is precisely why its MOD playback was inaccurate
//! (`plans/reference/original-s3mlib-analysis.md` §1; design goal 7).
//!
//! [`PatternCell`] is the deliberate exception, and it is **display-only**: a format crate
//! decodes one cell into it for a pattern view or a channel readout, and nothing in the
//! audio path ever constructs one.

use starplayer_core::Note;

/// Index of a pattern within [`Module::patterns`](crate::Module::patterns).
///
/// Zero-based, unlike some formats' one-based pattern numbering in order lists.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PatternId(pub u16);

/// Where one pattern's native bytes live in [`Module::blob`](crate::Module::blob).
///
/// `length_bytes` is what the loader wrote, so the model can bounds-check the region
/// without understanding a single byte inside it — and so a format decoder gets a slice
/// that ends where the pattern ends rather than one that runs to the end of the blob.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PatternIndex {
    blob_offset: u32,
    length_bytes: u32,
    rows: u16,
    channels: u8,
}

impl PatternIndex {
    /// Crate-private: patterns are added through
    /// [`ModuleBuilder::add_pattern`](crate::ModuleBuilder::add_pattern), which is what
    /// keeps the region inside the blob.
    pub(crate) const fn new(blob_offset: u32, length_bytes: u32, rows: u16, channels: u8) -> PatternIndex {
        PatternIndex { blob_offset, length_bytes, rows, channels }
    }

    /// Offset of the pattern's first byte within [`Module::blob`](crate::Module::blob).
    pub const fn blob_offset(&self) -> u32 { self.blob_offset }

    /// Length of the pattern's native data in bytes.
    pub const fn length_bytes(&self) -> u32 { self.length_bytes }

    /// Rows in the pattern.
    pub const fn rows(&self) -> u16 { self.rows }

    /// Channels the pattern's data covers.
    ///
    /// This is what the *pattern* stores, which is not always the song's channel count —
    /// an S3M pattern's packed stream may carry channels the header has disabled — so the
    /// builder deliberately does not require the two to agree.
    pub const fn channels(&self) -> u8 { self.channels }
}

/// The note column of a [`PatternCell`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum NoteCell {
    /// Empty column.
    #[default]
    None,
    /// Note cut: stop the voice immediately (S3M `^^`, IT `^^^`).
    Cut,
    /// Note off: release the voice (XM/IT `===`).
    Off,
    /// A note, as a linear semitone index with C-0 as 0 — the same numbering as
    /// [`starplayer_core::Note`]. Format crates convert from their own encoding
    /// (S3M's packed octave/note nybbles, MOD's period lookup) when they decode a cell.
    Note(u8),
}

impl NoteCell {
    /// The core [`Note`] this cell names, if it names one.
    pub const fn to_note(self) -> Option<Note> {
        match self {
            NoteCell::Note(semitone) => Some(Note::new(semitone)),
            _ => None,
        }
    }
}

/// The effect column of a [`PatternCell`], with the effect's English name resolved.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EffectCell {
    /// The format's own command code — for S3M, 1..=26 for `A`..`Z`.
    pub code: u8,
    /// The command's parameter byte.
    pub param: u8,
    /// The effect spelled out in English, or `""` for a command with no name in the
    /// table. See [`EffectNames`].
    pub name: &'static str,
}

impl EffectCell {
    /// Resolve `code` and `param` against a name table.
    pub fn new(code: u8, param: u8, names: &EffectNames) -> EffectCell {
        EffectCell { code, param, name: names.name(code, param).unwrap_or("") }
    }
}

/// One cell of one channel of one row — **display only**.
///
/// A format crate produces these for a pattern view or a channel readout. Nothing in the
/// audio path reads a `PatternCell`: the effect processors decode the native bytes
/// themselves.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PatternCell {
    /// The note column.
    pub note: NoteCell,
    /// The instrument column, as the file numbers it (usually one-based), or `None` when
    /// the column is empty.
    pub instrument: Option<u8>,
    /// The volume column, in the format's own units (S3M and IT 0..64), or `None`.
    pub volume: Option<u8>,
    /// The effect column, or `None`.
    pub effect: Option<EffectCell>,
}

/// A table of English effect names, indexed by command code.
///
/// The original spelled every active channel's effect out in words instead of showing raw
/// hex — `plans/reference/original-star-ui.md` §2.3 calls it the single most charming idea
/// in the program, and both modern UIs want it. This is that table.
///
/// One table per format, because the same letter means different things in different
/// formats. [`EffectNames::S3M`] and [`EffectNames::MOD`] are available; MTM, XM and IT
/// tables arrive with those format crates.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct EffectNames {
    commands: &'static [(u8, &'static str)],
    subcommand_code: u8,
    subcommands: &'static [(u8, &'static str)],
}

impl EffectNames {
    /// Scream Tracker 3's effect names, verbatim from the original's two tables.
    pub const S3M: EffectNames = EffectNames {
        commands: S3M_COMMAND_NAMES,
        subcommand_code: S3M_SUBCOMMAND_CODE,
        subcommands: S3M_SUBCOMMAND_NAMES,
    };

    /// ProTracker command and `E` sub-command names.
    pub const MOD: EffectNames = EffectNames {
        commands: MOD_COMMAND_NAMES,
        subcommand_code: 0xE,
        subcommands: MOD_SUBCOMMAND_NAMES,
    };

    /// A table for a format whose commands carry no sub-command nybble.
    pub const fn flat(commands: &'static [(u8, &'static str)]) -> EffectNames {
        EffectNames { commands, subcommand_code: 0, subcommands: &[] }
    }

    // ── XM (M5-F1) ──────────────────────────────────────────────────────────────────
    //
    // Its own block so the IT table (M6-G1) lands beside it without a merge conflict.

    /// FastTracker 2's effect names and its `E` sub-command names.
    ///
    /// Codes are the byte an XM pattern cell stores: `0x0`..=`0xF` for `0`..`9` and
    /// `A`..`F`, then `0x10`..=`0x21` for `G`..`X`. The names are the ones FastTracker 2's
    /// own `xm.txt` gives, so `1` is "porta up" rather than "portamento up".
    pub const XM: EffectNames = EffectNames {
        commands: XM_COMMAND_NAMES,
        subcommand_code: 0x0E,
        subcommands: XM_SUBCOMMAND_NAMES,
    };

    /// FastTracker 2's **volume column**, keyed by the high nybble of the column byte.
    ///
    /// The volume column is a second, parallel effect column with an encoding of its own,
    /// so it needs a table of its own: look a byte up as
    /// `EffectNames::XM_VOLUME_COLUMN.name(byte >> 4, byte)`. A byte below `0x10` does
    /// nothing and has no name.
    pub const XM_VOLUME_COLUMN: EffectNames = EffectNames::flat(XM_VOLUME_COLUMN_NAMES);

    /// The English name for a command, or `None` if the table has no entry for it.
    ///
    /// A command equal to the table's sub-command code (S3M's `S`) is looked up by the
    /// high nybble of `param` instead, which is how `S8` becomes `channel pan`.
    pub fn name(&self, code: u8, param: u8) -> Option<&'static str> {
        let (table, key) = match code == self.subcommand_code && !self.subcommands.is_empty() {
            true => (self.subcommands, param >> 4),
            false => (self.commands, code),
        };
        table.iter()
            .find(|(entry_key, _)| *entry_key == key)
            .map(|(_, name)| *name)
    }
}

/// The command code the S3M pattern format uses for a command letter: 1..=26 for
/// `A`..`Z`. Handy for a loader or a UI that has a letter and wants the code.
pub const fn s3m_command_code(letter: u8) -> u8 { letter.wrapping_sub(b'A').wrapping_add(1) }

/// S3M command code for `S`, the sub-command escape.
const S3M_SUBCOMMAND_CODE: u8 = s3m_command_code(b'S');

/// S3M command names, `original-star-ui.md` §2.3. Codes are 1..=26 for `A`..`Z`, the
/// numbering the S3M pattern format itself uses.
const S3M_COMMAND_NAMES: &[(u8, &str)] = &[
    (s3m_command_code(b'A'), "change speed"),
    (s3m_command_code(b'B'), "jump to order"),
    (s3m_command_code(b'C'), "break pattern"),
    (s3m_command_code(b'D'), "volume slide"),
    (s3m_command_code(b'E'), "slide down"),
    (s3m_command_code(b'F'), "slide up"),
    (s3m_command_code(b'G'), "portamento"),
    (s3m_command_code(b'H'), "vibrato"),
    (s3m_command_code(b'I'), "tremor"),
    (s3m_command_code(b'J'), "arpeggio"),
    (s3m_command_code(b'K'), "vibrato & vol. slide"),
    (s3m_command_code(b'L'), "porta & vol. slide"),
    (s3m_command_code(b'O'), "sample offset"),
    (s3m_command_code(b'Q'), "note retrigger"),
    (s3m_command_code(b'R'), "tremolo"),
    (s3m_command_code(b'T'), "change tempo"),
    (s3m_command_code(b'U'), "fine vibrato"),
    (s3m_command_code(b'V'), "global volume"),
    (s3m_command_code(b'X'), "fine channel pan"),
];

/// S3M `Sx` sub-command names, keyed by the high nybble of the parameter.
const S3M_SUBCOMMAND_NAMES: &[(u8, &str)] = &[
    (0x1, "glissando control"),
    (0x2, "set finetune"),
    (0x3, "set vibrato waveform"),
    (0x4, "set tremolo waveform"),
    (0x8, "channel pan"),
    (0xB, "pattern loop"),
    (0xC, "note cut"),
    (0xD, "note delay"),
    (0xE, "pattern delay"),
];

const MOD_COMMAND_NAMES: &[(u8, &str)] = &[
    (0x0, "arpeggio"),
    (0x1, "portamento up"),
    (0x2, "portamento down"),
    (0x3, "tone portamento"),
    (0x4, "vibrato"),
    (0x5, "tone porta & volume slide"),
    (0x6, "vibrato & volume slide"),
    (0x7, "tremolo"),
    (0x8, "channel pan"),
    (0x9, "sample offset"),
    (0xA, "volume slide"),
    (0xB, "position jump"),
    (0xC, "set volume"),
    (0xD, "pattern break"),
    (0xF, "set speed/tempo"),
];

const MOD_SUBCOMMAND_NAMES: &[(u8, &str)] = &[
    (0x0, "set filter"),
    (0x1, "fine portamento up"),
    (0x2, "fine portamento down"),
    (0x3, "glissando control"),
    (0x4, "set vibrato waveform"),
    (0x5, "set finetune"),
    (0x6, "pattern loop"),
    (0x7, "set tremolo waveform"),
    (0x8, "channel pan"),
    (0x9, "note retrigger"),
    (0xA, "fine volume slide up"),
    (0xB, "fine volume slide down"),
    (0xC, "note cut"),
    (0xD, "note delay"),
    (0xE, "pattern delay"),
    (0xF, "invert loop"),
];

// ── XM (M5-F1) ──────────────────────────────────────────────────────────────────────
//
// Its own block so the IT tables (M6-G1) land beside it without a merge conflict.

/// The code an XM pattern cell stores for an effect letter: `0`..`9` and `A`..`F` are
/// `0x0`..=`0xF`, and `G`..`Z` continue at `0x10`. Handy for a loader or a UI that has a
/// letter and wants the code.
pub const fn xm_command_code(letter: u8) -> u8 {
    match letter {
        b'0'..=b'9' => letter - b'0',
        b'A'..=b'Z' => letter - b'A' + 10,
        _ => 0,
    }
}

/// XM command names, verbatim from FastTracker 2's `xm.txt` "Standard effects" table.
///
/// `E` is missing on purpose: it is the sub-command escape and is resolved through
/// [`XM_SUBCOMMAND_NAMES`] instead.
const XM_COMMAND_NAMES: &[(u8, &str)] = &[
    (xm_command_code(b'0'), "arpeggio"),
    (xm_command_code(b'1'), "porta up"),
    (xm_command_code(b'2'), "porta down"),
    (xm_command_code(b'3'), "tone porta"),
    (xm_command_code(b'4'), "vibrato"),
    (xm_command_code(b'5'), "tone porta & volume slide"),
    (xm_command_code(b'6'), "vibrato & volume slide"),
    (xm_command_code(b'7'), "tremolo"),
    (xm_command_code(b'8'), "set panning"),
    (xm_command_code(b'9'), "sample offset"),
    (xm_command_code(b'A'), "volume slide"),
    (xm_command_code(b'B'), "position jump"),
    (xm_command_code(b'C'), "set volume"),
    (xm_command_code(b'D'), "pattern break"),
    (xm_command_code(b'F'), "set tempo/BPM"),
    (xm_command_code(b'G'), "set global volume"),
    (xm_command_code(b'H'), "global volume slide"),
    (xm_command_code(b'K'), "key off"),
    (xm_command_code(b'L'), "set envelope position"),
    (xm_command_code(b'P'), "panning slide"),
    (xm_command_code(b'R'), "multi retrig note"),
    (xm_command_code(b'T'), "tremor"),
    (xm_command_code(b'X'), "extra fine porta"),
];

/// XM `Ex` sub-command names, keyed by the high nybble of the parameter.
///
/// `E0` and `E8` are absent because `xm.txt` does not list them: FastTracker 2 implements
/// neither.
const XM_SUBCOMMAND_NAMES: &[(u8, &str)] = &[
    (0x1, "fine porta up"),
    (0x2, "fine porta down"),
    (0x3, "set gliss control"),
    (0x4, "set vibrato control"),
    (0x5, "set finetune"),
    (0x6, "set loop begin/loop"),
    (0x7, "set tremolo control"),
    (0x9, "retrig note"),
    (0xA, "fine volume slide up"),
    (0xB, "fine volume slide down"),
    (0xC, "note cut"),
    (0xD, "note delay"),
    (0xE, "pattern delay"),
];

/// XM volume-column names, keyed by the **high nybble** of the column byte. `$10`..`$50`
/// all set a volume, so five entries share one name; `$00`..`$0F` do nothing and have
/// none.
const XM_VOLUME_COLUMN_NAMES: &[(u8, &str)] = &[
    (0x1, "set volume"),
    (0x2, "set volume"),
    (0x3, "set volume"),
    (0x4, "set volume"),
    (0x5, "set volume"),
    (0x6, "volume slide down"),
    (0x7, "volume slide up"),
    (0x8, "fine volume slide down"),
    (0x9, "fine volume slide up"),
    (0xA, "set vibrato speed"),
    (0xB, "vibrato"),
    (0xC, "set panning"),
    (0xD, "panning slide left"),
    (0xE, "panning slide right"),
    (0xF, "tone portamento"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3m_commands_resolve_to_the_originals_english_names() {
        assert_eq!(EffectNames::S3M.name(s3m_command_code(b'A'), 0x06), Some("change speed"));
        assert_eq!(EffectNames::S3M.name(s3m_command_code(b'H'), 0x42), Some("vibrato"));
        assert_eq!(EffectNames::S3M.name(s3m_command_code(b'X'), 0x80), Some("fine channel pan"));
    }

    #[test]
    fn s3m_subcommands_resolve_by_the_high_nybble_of_the_parameter() {
        assert_eq!(EffectNames::S3M.name(S3M_SUBCOMMAND_CODE, 0x8F), Some("channel pan"));
        assert_eq!(EffectNames::S3M.name(S3M_SUBCOMMAND_CODE, 0xC3), Some("note cut"));
        assert_eq!(EffectNames::S3M.name(S3M_SUBCOMMAND_CODE, 0xEA), Some("pattern delay"));
    }

    #[test]
    fn an_unnamed_command_has_no_name() {
        assert_eq!(EffectNames::S3M.name(s3m_command_code(b'M'), 0), None, "S3M has no M command");
        assert_eq!(EffectNames::S3M.name(S3M_SUBCOMMAND_CODE, 0xF0), None, "SF is not in the original's table");
        assert_eq!(EffectNames::S3M.name(0, 0), None, "code 0 is an empty effect column");
    }

    #[test]
    fn an_effect_cell_carries_the_resolved_name() {
        let cell = EffectCell::new(s3m_command_code(b'D'), 0x0F, &EffectNames::S3M);
        assert_eq!(cell, EffectCell { code: 4, param: 0x0F, name: "volume slide" });
        assert_eq!(EffectCell::new(s3m_command_code(b'M'), 0, &EffectNames::S3M).name, "");
    }

    #[test]
    fn mod_commands_and_extended_commands_have_their_own_names() {
        assert_eq!(EffectNames::MOD.name(0, 0x37), Some("arpeggio"));
        assert_eq!(EffectNames::MOD.name(0xD, 0x31), Some("pattern break"));
        assert_eq!(EffectNames::MOD.name(0xE, 0xD3), Some("note delay"));
        assert_eq!(EffectNames::MOD.name(0xE, 0xFF), Some("invert loop"));
    }

    #[test]
    fn xm_command_codes_follow_the_bytes_the_pattern_format_stores() {
        assert_eq!(xm_command_code(b'0'), 0x00);
        assert_eq!(xm_command_code(b'9'), 0x09);
        assert_eq!(xm_command_code(b'A'), 0x0A);
        assert_eq!(xm_command_code(b'F'), 0x0F);
        assert_eq!(xm_command_code(b'G'), 0x10);
        assert_eq!(xm_command_code(b'X'), 0x21);
    }

    #[test]
    fn xm_commands_and_extended_commands_have_fast_tracker_twos_own_names() {
        assert_eq!(EffectNames::XM.name(xm_command_code(b'0'), 0x37), Some("arpeggio"));
        assert_eq!(EffectNames::XM.name(xm_command_code(b'4'), 0x42), Some("vibrato"));
        assert_eq!(EffectNames::XM.name(xm_command_code(b'F'), 0x7D), Some("set tempo/BPM"));
        assert_eq!(EffectNames::XM.name(xm_command_code(b'G'), 0x40), Some("set global volume"));
        assert_eq!(EffectNames::XM.name(xm_command_code(b'X'), 0x12), Some("extra fine porta"));
        assert_eq!(EffectNames::XM.name(xm_command_code(b'E'), 0xD3), Some("note delay"));
        assert_eq!(EffectNames::XM.name(xm_command_code(b'E'), 0x93), Some("retrig note"));
    }

    #[test]
    fn an_xm_command_fast_tracker_two_does_not_implement_has_no_name() {
        assert_eq!(EffectNames::XM.name(xm_command_code(b'I'), 0), None, "XM has no I command");
        assert_eq!(EffectNames::XM.name(xm_command_code(b'E'), 0x00), None, "xm.txt does not list E0");
        assert_eq!(EffectNames::XM.name(xm_command_code(b'E'), 0x80), None, "nor E8");
    }

    #[test]
    fn the_xm_volume_column_names_resolve_by_the_high_nybble() {
        let name = |byte: u8| EffectNames::XM_VOLUME_COLUMN.name(byte >> 4, byte);
        assert_eq!(name(0x00), None, "an empty volume column");
        assert_eq!(name(0x0F), None, "and everything below $10 does nothing");
        assert_eq!(name(0x10), Some("set volume"));
        assert_eq!(name(0x50), Some("set volume"));
        assert_eq!(name(0x6A), Some("volume slide down"));
        assert_eq!(name(0xB4), Some("vibrato"));
        assert_eq!(name(0xFF), Some("tone portamento"));
    }

    #[test]
    fn a_note_cell_converts_to_a_core_note() {
        assert_eq!(NoteCell::Note(48).to_note(), Some(Note::new(48)));
        assert_eq!(NoteCell::Cut.to_note(), None);
        assert_eq!(NoteCell::None.to_note(), None);
        assert_eq!(NoteCell::Off.to_note(), None);
    }
}
