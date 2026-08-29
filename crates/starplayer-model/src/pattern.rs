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
/// formats. [`EffectNames::S3M`] is the one M1 needs; the MOD, MTM, XM and IT tables
/// arrive with those format crates.
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

    /// A table for a format whose commands carry no sub-command nybble.
    pub const fn flat(commands: &'static [(u8, &'static str)]) -> EffectNames {
        EffectNames { commands, subcommand_code: 0, subcommands: &[] }
    }

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
    fn a_note_cell_converts_to_a_core_note() {
        assert_eq!(NoteCell::Note(48).to_note(), Some(Note::new(48)));
        assert_eq!(NoteCell::Cut.to_note(), None);
        assert_eq!(NoteCell::None.to_note(), None);
        assert_eq!(NoteCell::Off.to_note(), None);
    }
}
