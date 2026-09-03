//! [`XmPatternData`] — the engine seam's read side over a loaded XM.
//!
//! This is the whole of the engine seam task F1 provides. The [`TrackerProcessor`] that
//! reads these bytes is task F3's; nothing here interprets a cell.
//!
//! [`TrackerProcessor`]: starplayer_engine::TrackerProcessor

use starplayer_engine::{OrderEntry, PatternData};
use starplayer_model::{Module, OrderEntry as ModelOrderEntry, PatternId};
use starplayer_rt::Arc;

use crate::pattern::CELL_BYTES;

/// A loaded XM, read through the engine's [`PatternData`] seam.
///
/// One row is `channel_count × `[`CELL_BYTES`] contiguous bytes, so a row lookup is a
/// slice of a slice: no scan, no allocation and no panic, which is what
/// [`PatternData::row_bytes`] is required to be.
pub struct XmPatternData(pub Arc<Module>);

impl PatternData for XmPatternData {
    fn order_count(&self) -> u16 { self.0.orders().len().min(u16::MAX as usize) as u16 }

    fn order(&self, order: u16) -> Option<OrderEntry> {
        match self.0.order_entry(order as usize)? {
            ModelOrderEntry::Pattern(pattern) => Some(OrderEntry::Pattern(pattern.0)),
            // An XM order list has no marker or terminator of its own — an order naming a
            // pattern the file does not store becomes the loader's shared empty pattern
            // instead — so these two arms exist only for a module some other crate built.
            ModelOrderEntry::Marker => Some(OrderEntry::Skip),
            ModelOrderEntry::End => Some(OrderEntry::End),
        }
    }

    fn channel_count(&self) -> u8 { self.0.header().channel_count }

    fn rows_in_pattern(&self, pattern: u16) -> Option<u16> {
        self.0.pattern(PatternId(pattern)).map(|index| index.rows())
    }

    fn row_bytes(&self, pattern: u16, row: u16) -> Option<&[u8]> {
        let pattern_id = PatternId(pattern);
        let pattern_index = self.0.pattern(pattern_id)?;
        if row >= pattern_index.rows() {
            return None;
        }
        let row_length = pattern_index.channels() as usize * CELL_BYTES;
        let start = row as usize * row_length;
        self.0.pattern_bytes(pattern_id)?.get(start..start + row_length)
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::pattern::{MASK_IS_MASK, MASK_NOTE, XmCell};
    use alloc::vec;
    use alloc::vec::Vec;
    use starplayer_model::{ModuleBuilder, ModuleFormat, ModuleHeader};

    /// A two-row, two-channel XM-shaped module built directly, so the seam is tested
    /// without a file.
    fn module() -> Arc<Module> {
        let packed = [
            MASK_IS_MASK | MASK_NOTE, 49, MASK_IS_MASK | MASK_NOTE, 50,
            MASK_IS_MASK | MASK_NOTE, 51, MASK_IS_MASK | MASK_NOTE, 52,
        ];
        let cells = crate::pattern::unpack(&packed, 2, 2);
        let mut builder = ModuleBuilder::new();
        builder.add_pattern(&cells, 2, 2).expect("a valid pattern");
        builder.set_orders(&[0, 0]);
        builder.set_header(ModuleHeader::new(ModuleFormat::Xm, 2));
        Arc::new(builder.build().expect("every invariant holds"))
    }

    #[test]
    fn a_row_is_one_fixed_stride_slice_per_row() {
        let data = XmPatternData(module());

        assert_eq!(data.order_count(), 2);
        assert_eq!(data.order(0), Some(OrderEntry::Pattern(0)));
        assert_eq!(data.order(2), None);
        assert_eq!(data.channel_count(), 2);
        assert_eq!(data.rows_in_pattern(0), Some(2));
        assert_eq!(data.rows_in_pattern(1), None);

        let row = data.row_bytes(0, 1).expect("row 1 exists");
        assert_eq!(row.len(), 2 * CELL_BYTES);
        let notes: Vec<u8> = row.chunks_exact(CELL_BYTES).filter_map(XmCell::from_bytes).map(|cell| cell.note).collect();
        assert_eq!(notes, vec![51, 52]);
    }

    #[test]
    fn a_row_or_pattern_that_does_not_exist_answers_none_rather_than_panicking() {
        let data = XmPatternData(module());
        assert_eq!(data.row_bytes(0, 2), None);
        assert_eq!(data.row_bytes(1, 0), None);
        assert_eq!(data.row_bytes(u16::MAX, u16::MAX), None);
    }
}
