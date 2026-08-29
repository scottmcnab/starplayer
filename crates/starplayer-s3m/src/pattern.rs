//! S3M's **native** pattern cell, the unpacker that produces it, and the read-side view
//! over a loaded [`Module`]'s blob.
//!
//! # The stored form
//!
//! A pattern in the file is a packed byte stream (a channel mask per event, a `0` byte
//! per row); a pattern in [`Module::blob`](starplayer_model::Module::blob) is a **fixed
//! stride** array of [`S3mCell`]s: [`ROWS`] rows of
//! [`S3mHeader::addressed_channels`](crate::S3mHeader::addressed_channels) cells,
//! row-major, [`CELL_BYTES`] bytes each. The effect processor indexes it as
//! `row * channels + channel` and never walks a variable-length stream on the audio
//! thread, which is what keeps a row lookup `O(1)` and panic-free.
//!
//! Unpacking costs `64 * channels * 5` bytes a pattern — 2.5 KB for eight channels,
//! against roughly 400 bytes packed. That is the trade the architecture asks for: load
//! time is not real time, and `render()` may not allocate or walk.
//!
//! This is *not* a lowering into a shared cell type (design goal 7). `S3mCell` is S3M's
//! own encoding, byte for byte — a Scream Tracker 3 note byte, a Scream Tracker 3 command
//! letter — and only [`S3mCell::display`] ever leaves it.

use alloc::vec;
use alloc::vec::Vec;
use starplayer_model::{EffectCell, EffectNames, Module, NoteCell, PatternCell, PatternId};

/// Rows in every S3M pattern. The format has no other value.
pub const ROWS: u16 = 64;

/// Bytes one [`S3mCell`] occupies in the module blob.
pub const CELL_BYTES: usize = 5;

/// Note byte meaning "this cell has no note" — Scream Tracker 3's empty note column.
pub const NOTE_NONE: u8 = 255;

/// Note byte meaning "cut the voice now" — Scream Tracker 3's `^^`.
pub const NOTE_CUT: u8 = 254;

/// Instrument byte meaning "this cell has no instrument". Instruments are one-based in
/// the file, so zero is the empty column.
pub const INSTRUMENT_NONE: u8 = 0;

/// Volume byte meaning "this cell has no volume column".
pub const VOLUME_NONE: u8 = 255;

/// Command byte meaning "this cell has no effect". Command codes are 1..=26 for `A`..=`Z`,
/// so zero is free.
pub const COMMAND_NONE: u8 = 0;

/// Highest command code the S3M format defines: `Z`.
pub const COMMAND_MAX: u8 = 26;

/// One cell of one channel of one row, in Scream Tracker 3's own encoding.
///
/// # Encoding
///
/// | Field | Empty | Otherwise |
/// |---|---|---|
/// | `note` | [`NOTE_NONE`] (255) | [`NOTE_CUT`] (254), or `(octave << 4) \| semitone` |
/// | `instrument` | [`INSTRUMENT_NONE`] (0) | one-based instrument number |
/// | `volume` | [`VOLUME_NONE`] (255) | 0..=64 |
/// | `command` | [`COMMAND_NONE`] (0) | 1..=26 for `A`..=`Z` |
/// | `info` | — | the command's parameter byte, meaningless without a command |
///
/// The file itself spells "no command" as the absence of the command bit in the packed
/// mask rather than as a value; the original's tracker used 255 for it in its channel
/// state. Zero is used here because it is the value the format leaves unused, so an empty
/// cell is five bytes of `[255, 0, 255, 0, 0]` and a memset-shaped default is meaningful.
///
/// Nothing is clamped: a `volume` of 200 or a `command` of 31 reaches the effect
/// processor exactly as the file spelled it, and deciding what to do about it is the
/// effect processor's business.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct S3mCell {
    /// The note column. [`NOTE_NONE`], [`NOTE_CUT`], or `(octave << 4) | semitone`.
    pub note: u8,
    /// The instrument column, one-based; [`INSTRUMENT_NONE`] when empty.
    pub instrument: u8,
    /// The volume column, 0..=64; [`VOLUME_NONE`] when empty.
    pub volume: u8,
    /// The effect column: [`COMMAND_NONE`], or 1..=26 for `A`..=`Z`.
    pub command: u8,
    /// The effect's parameter byte.
    pub info: u8,
}

impl S3mCell {
    /// A cell with every column empty — what an unpacked pattern is filled with before
    /// the packed stream writes over it.
    pub const EMPTY: S3mCell = S3mCell {
        note: NOTE_NONE,
        instrument: INSTRUMENT_NONE,
        volume: VOLUME_NONE,
        command: COMMAND_NONE,
        info: 0,
    };

    /// Read a cell from the [`CELL_BYTES`] bytes that hold it, or `None` if there are
    /// fewer than that.
    pub fn from_bytes(bytes: &[u8]) -> Option<S3mCell> {
        match bytes.get(..CELL_BYTES)? {
            [note, instrument, volume, command, info] => Some(S3mCell {
                note: *note,
                instrument: *instrument,
                volume: *volume,
                command: *command,
                info: *info,
            }),
            _ => None,
        }
    }

    /// The [`CELL_BYTES`] bytes this cell is stored as.
    pub const fn to_bytes(self) -> [u8; CELL_BYTES] {
        [self.note, self.instrument, self.volume, self.command, self.info]
    }

    /// Whether every column is empty.
    pub const fn is_empty(&self) -> bool {
        self.note == NOTE_NONE
            && self.instrument == INSTRUMENT_NONE
            && self.volume == VOLUME_NONE
            && self.command == COMMAND_NONE
    }

    /// The note's octave nibble, or `None` when the cell has no playable note.
    pub const fn octave(&self) -> Option<u8> {
        match self.note {
            NOTE_NONE | NOTE_CUT => None,
            note => Some(note >> 4),
        }
    }

    /// The note's semitone-within-octave nibble, or `None` when the cell has no playable
    /// note.
    pub const fn semitone_in_octave(&self) -> Option<u8> {
        match self.note {
            NOTE_NONE | NOTE_CUT => None,
            note => Some(note & 0x0F),
        }
    }

    /// The note as a linear semitone index with C-0 as zero — the numbering
    /// [`NoteCell::Note`] uses.
    ///
    /// `None` for an empty cell or a note cut. The arithmetic cannot overflow: the
    /// largest note byte is `0xFD`, giving `15 * 12 + 13 = 193`.
    pub const fn linear_semitone(&self) -> Option<u8> {
        match self.note {
            NOTE_NONE | NOTE_CUT => None,
            note => Some((note >> 4) * 12 + (note & 0x0F)),
        }
    }

    /// The display-only view of this cell, with its effect named in English.
    ///
    /// Nothing in the audio path calls this; it is what a pattern view or a channel
    /// readout renders (`plans/reference/original-star-ui.md` §2.3).
    pub fn display(&self) -> PatternCell {
        PatternCell {
            note: match self.note {
                NOTE_NONE => NoteCell::None,
                NOTE_CUT => NoteCell::Cut,
                _ => NoteCell::Note(self.linear_semitone().unwrap_or(0)),
            },
            instrument: match self.instrument {
                INSTRUMENT_NONE => None,
                instrument => Some(instrument),
            },
            volume: match self.volume {
                VOLUME_NONE => None,
                volume => Some(volume),
            },
            effect: match self.command {
                COMMAND_NONE => None,
                command => Some(EffectCell::new(command, self.info, &EffectNames::S3M)),
            },
        }
    }
}

impl Default for S3mCell {
    fn default() -> S3mCell { S3mCell::EMPTY }
}

/// Unpack one pattern's packed byte stream into `rows * channels` [`S3mCell`]s, stored
/// row-major at [`CELL_BYTES`] bytes each.
///
/// `packed` is the stream **after** the two-byte packed-length field, which is not part
/// of it (the length counts itself; see [`crate::loader`]).
///
/// # What a malformed stream does
///
/// Nothing here can fail, because there is no failure a tracker would not have played
/// through:
///
/// * a stream that ends before 64 rows leaves the remaining rows empty;
/// * a stream that describes more than 64 rows is cut off at 64;
/// * an event naming a channel at or past `channels` is *parsed* — its bytes are
///   consumed, so the rest of the row still decodes — and then discarded, exactly as the
///   original discarded events above `_TotalChanNum`;
/// * two events for one channel in one row leave the last one standing.
pub fn unpack(packed: &[u8], rows: u16, channels: u8) -> Vec<u8> {
    let mut cells = vec![0u8; rows as usize * channels as usize * CELL_BYTES];
    for cell in cells.chunks_exact_mut(CELL_BYTES) {
        cell.copy_from_slice(&S3mCell::EMPTY.to_bytes());
    }

    let mut position = 0usize;
    let mut row = 0u16;
    while row < rows {
        let Some(mask) = packed.get(position).copied() else { break };
        position += 1;

        if mask == 0 {
            row += 1;
            continue;
        }

        let channel = mask & 0x1F;
        let mut cell = S3mCell::EMPTY;
        if mask & 0x20 != 0 {
            cell.note = packed.get(position).copied().unwrap_or(NOTE_NONE);
            cell.instrument = packed.get(position + 1).copied().unwrap_or(INSTRUMENT_NONE);
            position += 2;
        }
        if mask & 0x40 != 0 {
            cell.volume = packed.get(position).copied().unwrap_or(VOLUME_NONE);
            position += 1;
        }
        if mask & 0x80 != 0 {
            cell.command = packed.get(position).copied().unwrap_or(COMMAND_NONE);
            cell.info = packed.get(position + 1).copied().unwrap_or(0);
            position += 2;
        }

        if channel < channels {
            let offset = (row as usize * channels as usize + channel as usize) * CELL_BYTES;
            if let Some(destination) = cells.get_mut(offset..offset + CELL_BYTES) {
                destination.copy_from_slice(&cell.to_bytes());
            }
        }
    }
    cells
}

/// A read-only view over one unpacked pattern inside a loaded [`Module`].
///
/// This is how the effect processor reads pattern data: construct one per pattern change
/// (it is two `usize`s and a slice; constructing it allocates nothing and cannot panic),
/// then index it per row. Every accessor returns [`Option`], because it is read from
/// inside `render()` where a panic is fatal (architecture §8).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PatternView<'module> {
    cells: &'module [u8],
    rows: u16,
    channels: u8,
}

impl<'module> PatternView<'module> {
    /// Borrow pattern `id` out of `module`.
    ///
    /// `None` if `id` names no pattern, or if the pattern's region is smaller than
    /// `rows * channels` cells — which a module built by [`crate::load`] never is, but
    /// which a module hand-built by another crate could be.
    pub fn new(module: &'module Module, id: PatternId) -> Option<PatternView<'module>> {
        let index = module.pattern(id)?;
        let bytes = module.pattern_bytes(id)?;
        let needed = index.rows() as usize * index.channels() as usize * CELL_BYTES;
        Some(PatternView { cells: bytes.get(..needed)?, rows: index.rows(), channels: index.channels() })
    }

    /// Build a view directly over unpacked bytes — what [`unpack`] returns.
    pub fn from_cells(cells: &'module [u8], rows: u16, channels: u8) -> Option<PatternView<'module>> {
        let needed = rows as usize * channels as usize * CELL_BYTES;
        Some(PatternView { cells: cells.get(..needed)?, rows, channels })
    }

    /// Rows in this pattern — [`ROWS`] for anything this crate loaded.
    pub const fn rows(&self) -> u16 { self.rows }

    /// Channel columns this pattern stores. This is
    /// [`S3mHeader::addressed_channels`](crate::S3mHeader::addressed_channels), which
    /// equals the song's channel count for every well-formed file.
    pub const fn channels(&self) -> u8 { self.channels }

    /// The raw cell bytes, `rows * channels * CELL_BYTES` of them.
    pub const fn cells(&self) -> &'module [u8] { self.cells }

    /// One cell, or `None` if `row` or `channel` is out of range.
    pub fn cell(&self, row: u16, channel: u8) -> Option<S3mCell> {
        if row >= self.rows || channel >= self.channels {
            return None;
        }
        let offset = (row as usize * self.channels as usize + channel as usize) * CELL_BYTES;
        S3mCell::from_bytes(self.cells.get(offset..offset + CELL_BYTES)?)
    }

    /// Every cell of one row, left channel to right, or `None` if `row` is out of range.
    pub fn row(&self, row: u16) -> Option<impl Iterator<Item = S3mCell> + 'module> {
        if row >= self.rows {
            return None;
        }
        let start = row as usize * self.channels as usize * CELL_BYTES;
        let length = self.channels as usize * CELL_BYTES;
        let bytes = self.cells.get(start..start + length)?;
        Some(bytes.chunks_exact(CELL_BYTES).filter_map(S3mCell::from_bytes))
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use starplayer_model::s3m_command_code;

    #[test]
    fn an_empty_cell_round_trips_through_its_bytes() {
        assert_eq!(S3mCell::from_bytes(&S3mCell::EMPTY.to_bytes()), Some(S3mCell::EMPTY));
        assert_eq!(S3mCell::EMPTY.to_bytes(), [255, 0, 255, 0, 0]);
        assert!(S3mCell::EMPTY.is_empty());
        assert_eq!(S3mCell::from_bytes(&[1, 2, 3, 4]), None);
    }

    #[test]
    fn a_note_byte_splits_into_octave_and_semitone() {
        let cell = S3mCell { note: 0x45, ..S3mCell::EMPTY };
        assert_eq!(cell.octave(), Some(4));
        assert_eq!(cell.semitone_in_octave(), Some(5));
        assert_eq!(cell.linear_semitone(), Some(53));

        assert_eq!(S3mCell { note: NOTE_CUT, ..S3mCell::EMPTY }.linear_semitone(), None);
        assert_eq!(S3mCell::EMPTY.linear_semitone(), None);
    }

    #[test]
    fn a_cell_displays_with_its_effect_named_in_english() {
        let cell = S3mCell { note: 0x40, instrument: 3, volume: 32, command: s3m_command_code(b'D'), info: 0x0F };
        let display = cell.display();

        assert_eq!(display.note, NoteCell::Note(48));
        assert_eq!(display.instrument, Some(3));
        assert_eq!(display.volume, Some(32));
        assert_eq!(display.effect.map(|effect| effect.name), Some("volume slide"));

        assert_eq!(S3mCell { note: NOTE_CUT, ..S3mCell::EMPTY }.display().note, NoteCell::Cut);
        assert_eq!(S3mCell::EMPTY.display(), PatternCell::default());
    }

    #[test]
    fn the_unpacker_places_every_combination_of_the_three_optional_groups() {
        // Row 0: channel 1 note+instrument; channel 2 volume only; channel 3 command only.
        // Row 1: channel 0 all three.
        let packed = [
            0x21, 0x40, 0x01, // 0x20 | 1 -> note C-4, instrument 1
            0x42, 0x20, // 0x40 | 2 -> volume 32
            0x83, 0x01, 0x06, // 0x80 | 3 -> command A, info 6
            0x00, // end of row 0
            0xE0, 0x51, 0x02, 0x10, 0x04, 0x0F, // 0x20|0x40|0x80 | 0
            0x00, // end of row 1
        ];
        let cells = unpack(&packed, 2, 4);
        let view = PatternView::from_cells(&cells, 2, 4).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0), Some(S3mCell::EMPTY));
        assert_eq!(view.cell(0, 1), Some(S3mCell { note: 0x40, instrument: 1, ..S3mCell::EMPTY }));
        assert_eq!(view.cell(0, 2), Some(S3mCell { volume: 0x20, ..S3mCell::EMPTY }));
        assert_eq!(view.cell(0, 3), Some(S3mCell { command: 1, info: 6, ..S3mCell::EMPTY }));
        assert_eq!(view.cell(1, 0), Some(S3mCell { note: 0x51, instrument: 2, volume: 0x10, command: 4, info: 0x0F }));
        assert_eq!(view.cell(1, 1), Some(S3mCell::EMPTY));
        assert_eq!(view.cell(2, 0), None);
        assert_eq!(view.cell(0, 4), None);
    }

    #[test]
    fn a_stream_that_ends_early_leaves_the_remaining_rows_empty() {
        let packed = [0x21, 0x40, 0x01];
        let cells = unpack(&packed, 64, 2);
        let view = PatternView::from_cells(&cells, 64, 2).expect("the view covers the cells");

        assert_eq!(view.cell(0, 1), Some(S3mCell { note: 0x40, instrument: 1, ..S3mCell::EMPTY }));
        assert_eq!(view.cell(63, 1), Some(S3mCell::EMPTY));
        assert_eq!(cells.len(), 64 * 2 * CELL_BYTES);
    }

    #[test]
    fn an_event_for_a_channel_past_the_end_is_consumed_and_discarded() {
        // Channel 9 does not exist in a two-channel pattern, but its three bytes still
        // have to be stepped over or the rest of the row decodes as garbage.
        let packed = [0xA9, 0x40, 0x01, 0x02, 0x03, 0x21, 0x41, 0x05, 0x00];
        let cells = unpack(&packed, 1, 2);
        let view = PatternView::from_cells(&cells, 1, 2).expect("the view covers the cells");

        assert_eq!(view.cell(0, 1), Some(S3mCell { note: 0x41, instrument: 5, ..S3mCell::EMPTY }));
        assert_eq!(view.cell(0, 0), Some(S3mCell::EMPTY));
    }

    #[test]
    fn a_stream_longer_than_the_pattern_is_cut_off_at_the_last_row() {
        let packed = [0x00, 0x00, 0x21, 0x40, 0x01];
        let cells = unpack(&packed, 2, 1);
        let view = PatternView::from_cells(&cells, 2, 1).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0), Some(S3mCell::EMPTY));
        assert_eq!(view.cell(1, 0), Some(S3mCell::EMPTY));
    }

    #[test]
    fn a_truncated_event_at_the_very_end_reads_as_empty_columns() {
        let packed = [0x20];
        let cells = unpack(&packed, 1, 1);
        let view = PatternView::from_cells(&cells, 1, 1).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0), Some(S3mCell::EMPTY));
    }

    #[test]
    fn a_row_iterates_left_to_right() {
        let packed = [0x20, 0x40, 0x01, 0x21, 0x41, 0x02, 0x00];
        let cells = unpack(&packed, 1, 3);
        let view = PatternView::from_cells(&cells, 1, 3).expect("the view covers the cells");

        let row: Vec<S3mCell> = view.row(0).expect("row 0 exists").collect();
        assert_eq!(row.len(), 3);
        assert_eq!(row[0].note, 0x40);
        assert_eq!(row[1].note, 0x41);
        assert_eq!(row[2], S3mCell::EMPTY);
        assert!(view.row(1).is_none());
    }
}
