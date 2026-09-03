//! XM's **native** pattern cell, the unpacker that produces it, and the read-side view
//! over a loaded [`Module`]'s blob.
//!
//! # The stored form
//!
//! A pattern in the file is a packed byte stream — one entry per cell, in row-major
//! order, each entry either a five-field record or a mask byte naming the fields that
//! follow. A pattern in [`Module::blob`](starplayer_model::Module::blob) is a **fixed
//! stride** array of [`XmCell`]s: `rows` rows of `channels` cells, row-major,
//! [`CELL_BYTES`] bytes each, holding the file's own byte values unchanged. The effect
//! processor indexes it as `row * channels + channel` and never walks a variable-length
//! stream on the audio thread.
//!
//! Unlike S3M's stream, XM's is dense: every cell of every row has exactly one entry, and
//! there is no per-row terminator and no channel number. A stream that ends early
//! therefore leaves the *rest of the pattern* empty rather than the rest of a row.
//!
//! This is *not* a lowering into a shared cell type (design goal 7). [`XmCell`] is
//! FastTracker 2's own encoding, byte for byte — an XM note number, an XM effect code, an
//! XM volume-column byte — and only [`XmCell::display`] ever leaves it.

use alloc::vec;
use alloc::vec::Vec;
use starplayer_model::{EffectCell, EffectNames, Module, NoteCell, PatternCell, PatternId};

/// Bytes one [`XmCell`] occupies in the module blob.
pub const CELL_BYTES: usize = 5;

/// Rows a pattern gets when its header says zero — FastTracker 2's own default, and what
/// OpenMPT substitutes.
pub const DEFAULT_ROWS: u16 = 64;

/// Rows a pattern may have. The format's own limit; the field is a `u16`, so a larger
/// value is clamped to this.
pub const MAX_ROWS: u16 = 256;

/// Note byte meaning "this cell has no note".
pub const NOTE_NONE: u8 = 0;

/// Lowest playable note byte, C-0.
pub const NOTE_MIN: u8 = 1;

/// Highest playable note byte, B-7.
pub const NOTE_MAX: u8 = 96;

/// Note byte meaning "release the voice" — FastTracker 2's `===`.
pub const NOTE_KEY_OFF: u8 = 97;

/// Instrument byte meaning "this cell has no instrument". Instruments are one-based in
/// the file, so zero is the empty column.
pub const INSTRUMENT_NONE: u8 = 0;

/// Volume-column byte meaning "this cell has no volume column".
pub const VOLUME_NONE: u8 = 0;

/// Lowest volume-column byte that sets a volume; the volume is the byte minus this.
pub const VOLUME_SET_MIN: u8 = 0x10;

/// Highest volume-column byte that sets a volume.
pub const VOLUME_SET_MAX: u8 = 0x50;

/// Packed-stream mask bit that says the byte is a mask rather than a note.
pub const MASK_IS_MASK: u8 = 0x80;

/// Packed-stream mask bit: a note byte follows.
pub const MASK_NOTE: u8 = 0x01;

/// Packed-stream mask bit: an instrument byte follows.
pub const MASK_INSTRUMENT: u8 = 0x02;

/// Packed-stream mask bit: a volume-column byte follows.
pub const MASK_VOLUME: u8 = 0x04;

/// Packed-stream mask bit: an effect byte follows.
pub const MASK_EFFECT: u8 = 0x08;

/// Packed-stream mask bit: an effect-parameter byte follows.
pub const MASK_PARAMETER: u8 = 0x10;

/// One cell of one channel of one row, in FastTracker 2's own encoding.
///
/// # Encoding
///
/// | Field | Empty | Otherwise |
/// |---|---|---|
/// | `note` | [`NOTE_NONE`] (0) | 1..=96 for C-0..B-7, or [`NOTE_KEY_OFF`] (97) |
/// | `instrument` | [`INSTRUMENT_NONE`] (0) | one-based instrument number |
/// | `volume` | [`VOLUME_NONE`] (0) | `0x10`..=`0x50` sets a volume, `0x60`..=`0xFF` is a volume-column effect |
/// | `effect` | — | `0x00`..=`0x21` for `0`..`9`, `A`..`F`, `G`..`X` |
/// | `parameter` | — | the effect's parameter byte |
///
/// The effect column has no "empty" value of its own: `0x00` is arpeggio, whose no-op
/// parameter is `0x00`. So an empty effect column and `000` are the same five bytes, which
/// is exactly what FastTracker 2 stores, and [`XmCell::display`] renders the pair as no
/// effect when both are zero — the test OpenMPT's `ReadXMPatterns` applies.
///
/// Nothing is clamped: a note of 200 or an effect of 0x7F reaches the effect processor
/// exactly as the file spelled it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct XmCell {
    /// The note column: [`NOTE_NONE`], 1..=96, or [`NOTE_KEY_OFF`].
    pub note: u8,
    /// The instrument column, one-based; [`INSTRUMENT_NONE`] when empty.
    pub instrument: u8,
    /// The volume column, in FastTracker 2's own encoding.
    pub volume: u8,
    /// The effect column, `0x00`..=`0x21`.
    pub effect: u8,
    /// The effect's parameter byte.
    pub parameter: u8,
}

impl XmCell {
    /// A cell with every column empty — five zero bytes, which is what an unpacked
    /// pattern starts as.
    pub const EMPTY: XmCell = XmCell { note: 0, instrument: 0, volume: 0, effect: 0, parameter: 0 };

    /// Read a cell from the [`CELL_BYTES`] bytes that hold it, or `None` if there are
    /// fewer than that.
    pub fn from_bytes(bytes: &[u8]) -> Option<XmCell> {
        match bytes.get(..CELL_BYTES)? {
            [note, instrument, volume, effect, parameter] => Some(XmCell {
                note: *note,
                instrument: *instrument,
                volume: *volume,
                effect: *effect,
                parameter: *parameter,
            }),
            _ => None,
        }
    }

    /// The [`CELL_BYTES`] bytes this cell is stored as.
    pub const fn to_bytes(self) -> [u8; CELL_BYTES] {
        [self.note, self.instrument, self.volume, self.effect, self.parameter]
    }

    /// Whether every column is empty.
    pub const fn is_empty(&self) -> bool {
        self.note == NOTE_NONE
            && self.instrument == INSTRUMENT_NONE
            && self.volume == VOLUME_NONE
            && self.effect == 0
            && self.parameter == 0
    }

    /// Whether the note column names a playable note rather than nothing or a key-off.
    pub const fn has_note(&self) -> bool { self.note >= NOTE_MIN && self.note <= NOTE_MAX }

    /// Whether the note column is FastTracker 2's `===`.
    pub const fn is_key_off(&self) -> bool { self.note == NOTE_KEY_OFF }

    /// The note as a linear semitone index with C-0 as zero — the numbering
    /// [`NoteCell::Note`] uses — or `None` for an empty column, a key-off, or a byte
    /// past `96`.
    pub const fn linear_semitone(&self) -> Option<u8> {
        match self.has_note() {
            true => Some(self.note - 1),
            false => None,
        }
    }

    /// The note's octave, C-0 being octave 0, or `None` when there is no playable note.
    pub const fn octave(&self) -> Option<u8> {
        match self.linear_semitone() {
            Some(semitone) => Some(semitone / 12),
            None => None,
        }
    }

    /// The note's semitone within its octave, or `None` when there is no playable note.
    pub const fn semitone_in_octave(&self) -> Option<u8> {
        match self.linear_semitone() {
            Some(semitone) => Some(semitone % 12),
            None => None,
        }
    }

    /// The volume the volume column sets, 0..=64, or `None` when the column holds
    /// nothing or holds an effect rather than a volume.
    pub const fn set_volume(&self) -> Option<u8> {
        match self.volume >= VOLUME_SET_MIN && self.volume <= VOLUME_SET_MAX {
            true => Some(self.volume - VOLUME_SET_MIN),
            false => None,
        }
    }

    /// The English name of the volume column's effect, or `None` when the column is
    /// empty. A volume-*setting* column names itself, `"set volume"`.
    pub fn volume_effect_name(&self) -> Option<&'static str> {
        match self.volume {
            VOLUME_NONE => None,
            volume => EffectNames::XM_VOLUME_COLUMN.name(volume >> 4, volume),
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
                NOTE_KEY_OFF => NoteCell::Off,
                _ => match self.linear_semitone() {
                    Some(semitone) => NoteCell::Note(semitone),
                    None => NoteCell::None,
                },
            },
            instrument: match self.instrument {
                INSTRUMENT_NONE => None,
                instrument => Some(instrument),
            },
            volume: match self.volume {
                VOLUME_NONE => None,
                volume => Some(volume),
            },
            // FastTracker 2 has no empty effect column: `000` and "nothing" are the same
            // five bytes, and OpenMPT's `ReadXMPatterns` makes the same test.
            effect: match self.effect == 0 && self.parameter == 0 {
                true => None,
                false => Some(EffectCell::new(self.effect, self.parameter, &EffectNames::XM)),
            },
        }
    }
}

/// Unpack one pattern's packed byte stream into `rows * channels` [`XmCell`]s, stored
/// row-major at [`CELL_BYTES`] bytes each.
///
/// `packed` is the pattern's packed data, the `packed_size` bytes that follow its header.
///
/// # What a malformed stream does
///
/// Nothing here can fail, because there is no failure FastTracker 2 would not have played
/// through:
///
/// * a stream that ends before the last cell leaves every remaining cell empty — the
///   stream is dense and unlabelled, so there is nothing to resynchronise to;
/// * a stream longer than the pattern is ignored past the last cell;
/// * a truncated final record reads its missing bytes as zero, which is the empty column
///   in every one of the five fields.
pub fn unpack(packed: &[u8], rows: u16, channels: u8) -> Vec<u8> {
    let cell_count = rows as usize * channels as usize;
    let mut cells = vec![0u8; cell_count * CELL_BYTES];

    let mut position = 0usize;
    for index in 0..cell_count {
        let Some(first) = packed.get(position).copied() else { break };
        position += 1;

        let mut cell = XmCell::EMPTY;
        let mask = match first & MASK_IS_MASK != 0 {
            // A mask byte: the bits say which of the five fields follow.
            true => first,
            // Not a mask: the byte is the note and all five fields are present.
            false => {
                cell.note = first;
                MASK_INSTRUMENT | MASK_VOLUME | MASK_EFFECT | MASK_PARAMETER
            }
        };

        let take = |position: &mut usize| {
            let byte = packed.get(*position).copied().unwrap_or(0);
            *position += 1;
            byte
        };
        if mask & MASK_NOTE != 0 {
            cell.note = take(&mut position);
        }
        if mask & MASK_INSTRUMENT != 0 {
            cell.instrument = take(&mut position);
        }
        if mask & MASK_VOLUME != 0 {
            cell.volume = take(&mut position);
        }
        if mask & MASK_EFFECT != 0 {
            cell.effect = take(&mut position);
        }
        if mask & MASK_PARAMETER != 0 {
            cell.parameter = take(&mut position);
        }

        let offset = index * CELL_BYTES;
        if let Some(destination) = cells.get_mut(offset..offset + CELL_BYTES) {
            destination.copy_from_slice(&cell.to_bytes());
        }
    }
    cells
}

/// A read-only view over one unpacked pattern inside a loaded [`Module`].
///
/// This is how a pattern display reads XM pattern data: construct one per pattern (it is
/// a slice and two integers; constructing it allocates nothing and cannot panic), then
/// index it per row. Every accessor returns [`Option`], because it may be read from inside
/// `render()` where a panic is fatal (architecture §8).
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

    /// Rows in this pattern.
    pub const fn rows(&self) -> u16 { self.rows }

    /// Channel columns this pattern stores, which is the song's channel count for every
    /// pattern this crate loads.
    pub const fn channels(&self) -> u8 { self.channels }

    /// The raw cell bytes, `rows * channels * CELL_BYTES` of them.
    pub const fn cells(&self) -> &'module [u8] { self.cells }

    /// One cell, or `None` if `row` or `channel` is out of range.
    pub fn cell(&self, row: u16, channel: u8) -> Option<XmCell> {
        if row >= self.rows || channel >= self.channels {
            return None;
        }
        let offset = (row as usize * self.channels as usize + channel as usize) * CELL_BYTES;
        XmCell::from_bytes(self.cells.get(offset..offset + CELL_BYTES)?)
    }

    /// Every cell of one row, left channel to right, or `None` if `row` is out of range.
    pub fn row(&self, row: u16) -> Option<impl Iterator<Item = XmCell> + 'module> {
        if row >= self.rows {
            return None;
        }
        let start = row as usize * self.channels as usize * CELL_BYTES;
        let length = self.channels as usize * CELL_BYTES;
        let bytes = self.cells.get(start..start + length)?;
        Some(bytes.chunks_exact(CELL_BYTES).filter_map(XmCell::from_bytes))
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_cell_round_trips_through_its_bytes() {
        assert_eq!(XmCell::from_bytes(&XmCell::EMPTY.to_bytes()), Some(XmCell::EMPTY));
        assert_eq!(XmCell::EMPTY.to_bytes(), [0, 0, 0, 0, 0]);
        assert!(XmCell::EMPTY.is_empty());
        assert_eq!(XmCell::default(), XmCell::EMPTY);
        assert_eq!(XmCell::from_bytes(&[1, 2, 3, 4]), None);
    }

    #[test]
    fn a_note_byte_is_one_based_from_c_zero() {
        let cell = XmCell { note: 49, ..XmCell::EMPTY };
        assert_eq!(cell.linear_semitone(), Some(48), "note 49 is C-4");
        assert_eq!(cell.octave(), Some(4));
        assert_eq!(cell.semitone_in_octave(), Some(0));
        assert!(cell.has_note());

        assert_eq!(XmCell { note: 1, ..XmCell::EMPTY }.linear_semitone(), Some(0), "note 1 is C-0");
        assert_eq!(XmCell { note: 96, ..XmCell::EMPTY }.linear_semitone(), Some(95), "note 96 is B-7");
        assert_eq!(XmCell { note: NOTE_KEY_OFF, ..XmCell::EMPTY }.linear_semitone(), None);
        assert!(XmCell { note: NOTE_KEY_OFF, ..XmCell::EMPTY }.is_key_off());
        assert_eq!(XmCell { note: 200, ..XmCell::EMPTY }.linear_semitone(), None, "a byte past 97 names no note");
        assert_eq!(XmCell::EMPTY.linear_semitone(), None);
    }

    #[test]
    fn the_volume_column_splits_into_a_volume_and_an_effect_half() {
        assert_eq!(XmCell { volume: 0x10, ..XmCell::EMPTY }.set_volume(), Some(0));
        assert_eq!(XmCell { volume: 0x30, ..XmCell::EMPTY }.set_volume(), Some(32));
        assert_eq!(XmCell { volume: 0x50, ..XmCell::EMPTY }.set_volume(), Some(64));
        assert_eq!(XmCell { volume: 0x51, ..XmCell::EMPTY }.set_volume(), None);
        assert_eq!(XmCell { volume: 0x0F, ..XmCell::EMPTY }.set_volume(), None);
        assert_eq!(XmCell { volume: 0xC8, ..XmCell::EMPTY }.set_volume(), None);
    }

    #[test]
    fn the_volume_column_names_its_effect_in_english() {
        assert_eq!(XmCell { volume: 0x00, ..XmCell::EMPTY }.volume_effect_name(), None);
        assert_eq!(XmCell { volume: 0x30, ..XmCell::EMPTY }.volume_effect_name(), Some("set volume"));
        assert_eq!(XmCell { volume: 0x64, ..XmCell::EMPTY }.volume_effect_name(), Some("volume slide down"));
        assert_eq!(XmCell { volume: 0x9F, ..XmCell::EMPTY }.volume_effect_name(), Some("fine volume slide up"));
        assert_eq!(XmCell { volume: 0xC8, ..XmCell::EMPTY }.volume_effect_name(), Some("set panning"));
        assert_eq!(XmCell { volume: 0xF3, ..XmCell::EMPTY }.volume_effect_name(), Some("tone portamento"));
    }

    #[test]
    fn a_cell_displays_with_its_effect_named_in_english() {
        let cell = XmCell { note: 49, instrument: 3, volume: 0x30, effect: 0x0A, parameter: 0xF0 };
        let display = cell.display();

        assert_eq!(display.note, NoteCell::Note(48));
        assert_eq!(display.instrument, Some(3));
        assert_eq!(display.volume, Some(0x30));
        assert_eq!(display.effect.map(|effect| effect.name), Some("volume slide"));

        assert_eq!(XmCell { note: NOTE_KEY_OFF, ..XmCell::EMPTY }.display().note, NoteCell::Off);
        assert_eq!(XmCell::EMPTY.display(), PatternCell::default());
    }

    #[test]
    fn an_effect_column_of_zero_with_a_parameter_is_still_an_arpeggio() {
        assert_eq!(XmCell { effect: 0, parameter: 0, ..XmCell::EMPTY }.display().effect, None);
        let arpeggio = XmCell { effect: 0, parameter: 0x37, ..XmCell::EMPTY }.display().effect;
        assert_eq!(arpeggio.map(|effect| effect.name), Some("arpeggio"));
    }

    #[test]
    fn an_unmasked_byte_is_a_note_with_all_five_fields_following() {
        // Two cells: one unpacked record, then one mask with only a volume column.
        let packed = [49, 1, 0x30, 0x0A, 0xF0, MASK_IS_MASK | MASK_VOLUME, 0x42];
        let cells = unpack(&packed, 1, 2);
        let view = PatternView::from_cells(&cells, 1, 2).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0), Some(XmCell { note: 49, instrument: 1, volume: 0x30, effect: 0x0A, parameter: 0xF0 }));
        assert_eq!(view.cell(0, 1), Some(XmCell { volume: 0x42, ..XmCell::EMPTY }));
    }

    #[test]
    fn a_mask_selects_exactly_the_fields_that_follow() {
        let packed = [
            MASK_IS_MASK | MASK_NOTE | MASK_INSTRUMENT, 49, 7,
            MASK_IS_MASK | MASK_EFFECT | MASK_PARAMETER, 0x0F, 0x06,
            MASK_IS_MASK, // no fields at all: an entirely empty cell that still costs a byte
            MASK_IS_MASK | MASK_NOTE, NOTE_KEY_OFF,
        ];
        let cells = unpack(&packed, 1, 4);
        let view = PatternView::from_cells(&cells, 1, 4).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0), Some(XmCell { note: 49, instrument: 7, ..XmCell::EMPTY }));
        assert_eq!(view.cell(0, 1), Some(XmCell { effect: 0x0F, parameter: 0x06, ..XmCell::EMPTY }));
        assert_eq!(view.cell(0, 2), Some(XmCell::EMPTY));
        assert_eq!(view.cell(0, 3), Some(XmCell { note: NOTE_KEY_OFF, ..XmCell::EMPTY }));
    }

    #[test]
    fn the_stream_is_dense_so_cells_fill_row_major() {
        let packed = [
            MASK_IS_MASK | MASK_NOTE, 1, MASK_IS_MASK | MASK_NOTE, 2,
            MASK_IS_MASK | MASK_NOTE, 3, MASK_IS_MASK | MASK_NOTE, 4,
        ];
        let cells = unpack(&packed, 2, 2);
        let view = PatternView::from_cells(&cells, 2, 2).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0).map(|cell| cell.note), Some(1));
        assert_eq!(view.cell(0, 1).map(|cell| cell.note), Some(2));
        assert_eq!(view.cell(1, 0).map(|cell| cell.note), Some(3));
        assert_eq!(view.cell(1, 1).map(|cell| cell.note), Some(4));
    }

    #[test]
    fn a_stream_that_ends_early_leaves_every_remaining_cell_empty() {
        let packed = [MASK_IS_MASK | MASK_NOTE, 49];
        let cells = unpack(&packed, 64, 4);
        let view = PatternView::from_cells(&cells, 64, 4).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0), Some(XmCell { note: 49, ..XmCell::EMPTY }));
        assert_eq!(view.cell(0, 1), Some(XmCell::EMPTY));
        assert_eq!(view.cell(63, 3), Some(XmCell::EMPTY));
        assert_eq!(cells.len(), 64 * 4 * CELL_BYTES);
    }

    #[test]
    fn a_truncated_final_record_reads_its_missing_bytes_as_empty_columns() {
        let packed = [MASK_IS_MASK | MASK_NOTE | MASK_INSTRUMENT | MASK_VOLUME, 49];
        let cells = unpack(&packed, 1, 1);
        let view = PatternView::from_cells(&cells, 1, 1).expect("the view covers the cells");

        assert_eq!(view.cell(0, 0), Some(XmCell { note: 49, ..XmCell::EMPTY }));
    }

    #[test]
    fn a_stream_longer_than_the_pattern_is_ignored_past_the_last_cell() {
        let packed = [MASK_IS_MASK | MASK_NOTE, 49, MASK_IS_MASK | MASK_NOTE, 50];
        let cells = unpack(&packed, 1, 1);
        assert_eq!(cells.len(), CELL_BYTES);
        assert_eq!(XmCell::from_bytes(&cells).map(|cell| cell.note), Some(49));
    }

    #[test]
    fn an_empty_pattern_is_all_empty_cells() {
        let cells = unpack(&[], 64, 8);
        assert_eq!(cells.len(), 64 * 8 * CELL_BYTES);
        assert!(cells.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn a_row_iterates_left_to_right() {
        let packed = [
            MASK_IS_MASK | MASK_NOTE, 1, MASK_IS_MASK | MASK_NOTE, 2, MASK_IS_MASK | MASK_NOTE, 3,
        ];
        let cells = unpack(&packed, 1, 3);
        let view = PatternView::from_cells(&cells, 1, 3).expect("the view covers the cells");

        let row: Vec<XmCell> = view.row(0).expect("row 0 exists").collect();
        assert_eq!(row.len(), 3);
        assert_eq!(row[0].note, 1);
        assert_eq!(row[2].note, 3);
        assert!(view.row(1).is_none());
        assert_eq!(view.cell(0, 3), None);
        assert_eq!(view.rows(), 1);
        assert_eq!(view.channels(), 3);
        assert_eq!(view.cells().len(), 3 * CELL_BYTES);
    }
}
