//! MultiTracker's native three-byte cell and fixed 64-row pattern view.

use starplayer_model::{EffectCell, EffectNames, Module, NoteCell, PatternCell, PatternId};

pub const ROWS: u16 = 64;
pub const CELL_BYTES: usize = 3;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MtmCell {
    pub pitch: u8,
    pub instrument: u8,
    pub effect: u8,
    pub param: u8,
}

impl MtmCell {
    pub const EMPTY: MtmCell = MtmCell { pitch: 0, instrument: 0, effect: 0, param: 0 };

    pub fn from_bytes(bytes: &[u8]) -> Option<MtmCell> {
        match bytes {
            [first, second, third] => Some(MtmCell {
                pitch: first >> 2,
                instrument: ((first & 3) << 4) | (second >> 4),
                effect: second & 15,
                param: *third,
            }),
            _ => None,
        }
    }

    pub const fn to_bytes(self) -> [u8; CELL_BYTES] {
        [((self.pitch & 0x3F) << 2) | ((self.instrument >> 4) & 3), ((self.instrument & 15) << 4) | (self.effect & 15), self.param]
    }

    /// Convert MTM's one-based pitch domain using the format's documented quotient and
    /// remainder rule: octave = pitch / 12 + 2, note = pitch % 12.
    pub const fn linear_note(self) -> Option<u8> {
        if self.pitch == 0 { None } else { Some(self.pitch.saturating_add(24)) }
    }

    pub fn display(self) -> PatternCell {
        PatternCell {
            note: self.linear_note().map(NoteCell::Note).unwrap_or(NoteCell::None),
            instrument: (self.instrument != 0).then_some(self.instrument),
            volume: None,
            effect: (self.effect != 0 || self.param != 0).then_some(EffectCell::new(self.effect, self.param, &EffectNames::MOD)),
        }
    }
}

pub struct PatternView<'module> {
    cells: &'module [u8],
    channels: u8,
}

impl<'module> PatternView<'module> {
    pub fn new(module: &'module Module, pattern: PatternId) -> Option<PatternView<'module>> {
        let index = module.pattern(pattern)?;
        if index.rows() != ROWS { return None; }
        let expected = ROWS as usize * index.channels() as usize * CELL_BYTES;
        let cells = module.pattern_bytes(pattern)?;
        if cells.len() != expected { return None; }
        Some(PatternView { cells, channels: index.channels() })
    }

    pub const fn rows(&self) -> u16 { ROWS }
    pub const fn channels(&self) -> u8 { self.channels }

    pub fn cell(&self, row: u16, channel: u8) -> Option<MtmCell> {
        if row >= ROWS || channel >= self.channels { return None; }
        let index = (row as usize * self.channels as usize + channel as usize) * CELL_BYTES;
        MtmCell::from_bytes(self.cells.get(index..index + CELL_BYTES)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_cell_round_trips_and_uses_the_mtm_note_axis() {
        let cell = MtmCell { pitch: 12, instrument: 63, effect: 0xF, param: 0x7D };
        assert_eq!(cell.to_bytes(), [0x33, 0xFF, 0x7D]);
        assert_eq!(MtmCell::from_bytes(&cell.to_bytes()), Some(cell));
        assert_eq!(cell.linear_note(), Some(36));
    }

    #[test]
    fn native_packing_masks_values_to_their_declared_bit_widths() {
        let cell = MtmCell { pitch: 0xFF, instrument: 0xFF, effect: 0xFF, param: 0xFF };
        assert_eq!(cell.to_bytes(), [0xFF, 0xFF, 0xFF]);
        assert_eq!(MtmCell::from_bytes(&cell.to_bytes()), Some(MtmCell { pitch: 63, instrument: 63, effect: 15, param: 255 }));
    }
}
