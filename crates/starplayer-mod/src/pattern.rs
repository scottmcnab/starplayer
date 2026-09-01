//! MOD's native four-byte cell and fixed-stride pattern view.

use starplayer_model::{EffectCell, EffectNames, Module, NoteCell, PatternCell, PatternId};

use crate::tables::note_from_period;

pub const ROWS: u16 = 64;
pub const CELL_BYTES: usize = 4;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModCell {
    pub period: u16,
    pub instrument: u8,
    pub effect: u8,
    pub param: u8,
}

impl ModCell {
    pub const EMPTY: ModCell = ModCell { period: 0, instrument: 0, effect: 0, param: 0 };

    pub fn from_bytes(bytes: &[u8]) -> Option<ModCell> {
        match bytes {
            [first, second, third, fourth] => Some(ModCell {
                period: (((first & 15) as u16) << 8) | *second as u16,
                instrument: (first & 0xF0) | (third >> 4),
                effect: third & 15,
                param: *fourth,
            }),
            _ => None,
        }
    }

    pub const fn to_bytes(self) -> [u8; CELL_BYTES] {
        [
            (self.instrument & 0xF0) | ((self.period >> 8) as u8 & 15),
            self.period as u8,
            ((self.instrument & 15) << 4) | (self.effect & 15),
            self.param,
        ]
    }

    pub fn linear_note(self) -> Option<u8> { note_from_period(self.period) }

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

    pub fn cell(&self, row: u16, channel: u8) -> Option<ModCell> {
        if row >= ROWS || channel >= self.channels { return None; }
        let index = (row as usize * self.channels as usize + channel as usize) * CELL_BYTES;
        ModCell::from_bytes(self.cells.get(index..index + CELL_BYTES)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_cell_round_trips_without_lowering() {
        let cell = ModCell { period: 428, instrument: 31, effect: 0xE, param: 0xD3 };
        assert_eq!(cell.to_bytes(), [0x11, 0xAC, 0xFE, 0xD3]);
        assert_eq!(ModCell::from_bytes(&cell.to_bytes()), Some(cell));
        assert_eq!(cell.linear_note(), Some(48));
    }

    #[test]
    fn zero_zero_zero_is_not_an_arpeggio() {
        assert_eq!(ModCell::EMPTY.display().effect, None);
        assert_eq!(ModCell { param: 0x37, ..ModCell::EMPTY }.display().effect.map(|effect| effect.code), Some(0));
    }
}
