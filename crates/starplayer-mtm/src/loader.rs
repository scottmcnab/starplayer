//! Bounds-checked MultiTracker 1.0 loading with native three-byte pattern cells.

use alloc::vec;
use alloc::vec::Vec;

use starplayer_core::fixed::{bipolar_from_ratio, unit_from_ratio};
use starplayer_core::{Error, I1F15, U0F16};
use starplayer_model::{InstrumentDef, LoopMode, Module, ModuleBuilder, ModuleFlags, ModuleFormat, ModuleHeader, ModuleReader, ORDER_END, SampleSpec};
use starplayer_mod::FINETUNE_REFERENCE_RATES;

use crate::pattern::{CELL_BYTES, ROWS};

const HEADER_BYTES: usize = 66;
const SAMPLE_HEADER_BYTES: usize = 37;
const ORDER_BYTES: usize = 128;
const TRACK_BYTES: usize = ROWS as usize * CELL_BYTES;
const PATTERN_CHANNELS: usize = 32;
const PATTERN_TABLE_BYTES: usize = PATTERN_CHANNELS * 2;
const FORMAT_TRACK_MASK: u32 = 0xFFFF;
const FORMAT_NATIVE_TEMPO_RESETS: u32 = 1 << 16;

/// Timing personality selected from the module's Fxx usage.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum TempoMode {
    /// MultiTracker resets BPM to 125 on speed changes and speed to 6 on BPM changes.
    #[default]
    MultiTracker,
    /// Dual Module Player's widespread ProTracker-style split, without counterpart resets.
    DualModulePlayer,
}

pub fn probe(bytes: &[u8]) -> bool { bytes.get(..4) == Some(b"MTM\x10") }

pub fn probe_reader<R: ModuleReader + ?Sized>(reader: &R) -> bool {
    let mut magic = [0u8; 4];
    reader.read_at(0, &mut magic).is_ok() && &magic == b"MTM\x10"
}

pub fn load(bytes: &[u8]) -> Result<Module, Error> { load_from(bytes) }

pub fn load_from<R: ModuleReader + ?Sized>(reader: &R) -> Result<Module, Error> {
    let source = Source { reader };
    let header: [u8; HEADER_BYTES] = source.array(0)?;
    if header.get(..4) != Some(b"MTM\x10") { return Err(Error::BadMagic); }

    let track_count = le_u16(&header, 24)?;
    let pattern_count = header.get(26).copied().unwrap_or(0) as usize + 1;
    // The stored order table is 128 entries; libxmp and the DOS original both keep
    // playing a file whose declared last order runs past it rather than refusing to load.
    let last_order = (header.get(27).copied().unwrap_or(0) as usize).min(ORDER_BYTES - 1);
    let comment_length = le_u16(&header, 28)? as usize;
    let sample_count = header.get(30).copied().unwrap_or(0) as usize;
    if sample_count > 63 { return Err(Error::Invalid("MTM sample count must be 0..=63")); }
    // Byte 31 is documented "always zero". libxmp reads it into `mfh.attr` and never
    // looks at it again, and so does the original; a stray value is not a refusal.
    let _module_attributes = header.get(31).copied().unwrap_or(0);
    if header.get(32).copied().unwrap_or(0) != ROWS as u8 { return Err(Error::Unsupported("MTM tracks not containing 64 rows")); }
    let channel_count = header.get(33).copied().unwrap_or(0);
    if !(1..=32).contains(&channel_count) { return Err(Error::Invalid("MTM channel count must be 1..=32")); }

    let sample_headers_bytes = checked_mul(sample_count, SAMPLE_HEADER_BYTES, "MTM sample headers")?;
    let order_offset = checked_add(HEADER_BYTES, sample_headers_bytes, "MTM order table")?;
    let track_offset = checked_add(order_offset, ORDER_BYTES, "MTM track data")?;
    let tracks_bytes = checked_mul(track_count as usize, TRACK_BYTES, "MTM track data")?;
    let pattern_table_offset = checked_add(track_offset, tracks_bytes, "MTM pattern table")?;
    let pattern_tables_bytes = checked_mul(pattern_count, PATTERN_TABLE_BYTES, "MTM pattern table")?;
    let comment_offset = checked_add(pattern_table_offset, pattern_tables_bytes, "MTM comment")?;
    let sample_data_offset = checked_add(comment_offset, comment_length, "MTM sample data")?;
    if sample_data_offset > source.len() {
        return Err(Error::Truncated { offset: comment_offset.min(source.len()), needed: sample_data_offset.saturating_sub(comment_offset.min(source.len())) });
    }

    let orders = source.with_slice(order_offset, ORDER_BYTES, <[u8]>::to_vec)?;
    let tracks = source.with_slice(track_offset, tracks_bytes, <[u8]>::to_vec)?;
    let pattern_tables = source.with_slice(pattern_table_offset, pattern_tables_bytes, <[u8]>::to_vec)?;
    let mut patterns = Vec::with_capacity(pattern_count);
    for pattern_index in 0..pattern_count {
        let table_start = pattern_index * PATTERN_TABLE_BYTES;
        let table = pattern_tables.get(table_start..table_start + PATTERN_TABLE_BYTES)
            .ok_or(Error::Truncated { offset: pattern_table_offset + table_start, needed: PATTERN_TABLE_BYTES })?;
        patterns.push(expand_pattern(&tracks, track_count, table, channel_count));
    }
    let tempo_mode = detect_tempo_mode(&tracks, &patterns, channel_count);

    let mut builder = ModuleBuilder::new();
    for pattern in &patterns { builder.add_pattern(pattern, ROWS, channel_count)?; }

    // Preflight every declared sample span before reading or allocating any sample PCM.
    // In particular, a tiny malformed file can declare a u32::MAX sample length; letting
    // `with_slice` see that out-of-range request would otherwise attempt an enormous
    // fallback allocation before `read_at` could report truncation.
    let mut sample_headers = Vec::with_capacity(sample_count);
    let mut required_sample_end = sample_data_offset;
    for sample_index in 0..sample_count {
        let header_offset = HEADER_BYTES + sample_index * SAMPLE_HEADER_BYTES;
        let sample_header: [u8; SAMPLE_HEADER_BYTES] = source.array(header_offset)?;
        if sample_header.get(36).copied().unwrap_or(0) & 1 != 0 {
            return Err(Error::Unsupported("16-bit MTM samples"));
        }
        let declared_length = usize::try_from(le_u32(&sample_header, 22)?).map_err(|_| Error::TooLarge("MTM sample"))?;
        let remaining = source.len().saturating_sub(required_sample_end);
        if declared_length > remaining {
            return Err(Error::Truncated { offset: required_sample_end, needed: declared_length });
        }
        required_sample_end = checked_add(required_sample_end, declared_length, "MTM sample data")?;
        sample_headers.push(sample_header);
    }

    let mut declared_sample_offset = sample_data_offset;
    for sample_header in sample_headers {
        let name = starplayer_model::decode_cp437(sample_header.get(..22).unwrap_or(&[]));
        let declared_length = usize::try_from(le_u32(&sample_header, 22)?).map_err(|_| Error::TooLarge("MTM sample"))?;
        let declared_loop_start = usize::try_from(le_u32(&sample_header, 26)?).map_err(|_| Error::TooLarge("MTM sample loop"))?;
        let declared_loop_end = usize::try_from(le_u32(&sample_header, 30)?).map_err(|_| Error::TooLarge("MTM sample loop"))?;
        let finetune = sample_header.get(34).copied().unwrap_or(0) & 15;
        let volume = sample_header.get(35).copied().unwrap_or(0).min(64);
        // MTM bytes are unsigned. Internal PCM is signed i16, so 0x80 is exact zero.
        let pcm: Vec<i16> = source.with_slice(declared_sample_offset, declared_length, |raw| {
            raw.iter().map(|byte| (*byte as i16 - 128) * 256).collect()
        })?;
        declared_sample_offset = checked_add(declared_sample_offset, declared_length, "MTM sample data")?;

        let loop_start = declared_loop_start.min(pcm.len());
        let loop_end = declared_loop_end.min(pcm.len());
        let loops = declared_loop_end.saturating_sub(declared_loop_start) > 4 && loop_start < loop_end;
        let specification = SampleSpec {
            name: name.clone(),
            loop_mode: if loops { LoopMode::Forward } else { LoopMode::None },
            loop_start: if loops { loop_start as u32 } else { 0 },
            loop_end: if loops { loop_end as u32 } else { 0 },
            default_volume: unit_from_ratio(volume as u32, 64),
            // MultiTracker uses the same signed-nibble C2SPD table as MOD, not S3M's S2x table.
            reference_rate_hz: FINETUNE_REFERENCE_RATES[finetune as usize],
        };
        let sample = builder.add_sample(&pcm, specification)?;
        builder.add_instrument(InstrumentDef::from_sample(&name, sample, U0F16::MAX))?;
    }

    let mut playable_orders = Vec::with_capacity(last_order + 2);
    for order in orders.iter().take(last_order + 1) {
        // An order naming a pattern that was never stored is stepped over, the same
        // recovery the track table already uses for an out-of-range track reference.
        // libxmp tolerates it too; refusing the whole module does not.
        if *order as usize >= pattern_count { playable_orders.push(starplayer_model::ORDER_MARKER); continue; }
        playable_orders.push(*order as u16);
    }
    playable_orders.push(ORDER_END);
    builder.set_orders(&playable_orders);
    let mut format_extra = track_count as u32 & FORMAT_TRACK_MASK;
    if tempo_mode == TempoMode::MultiTracker { format_extra |= FORMAT_NATIVE_TEMPO_RESETS; }
    builder.set_header(ModuleHeader {
        title: starplayer_model::decode_cp437(header.get(4..24).unwrap_or(&[])).into_boxed_str(),
        format: ModuleFormat::Mtm,
        channel_count,
        initial_speed: 6,
        initial_tempo: 125,
        global_volume: U0F16::MAX,
        master_volume: U0F16::MAX,
        default_pan: header.get(34..66).unwrap_or(&[]).iter().take(channel_count as usize)
            .map(|pan| mtm_pan(*pan)).collect::<Vec<_>>().into_boxed_slice(),
        // ConvertMTM's dummy period always disabled Amiga limits as a side effect. MTM
        // is a PC tracker format, so state that rule directly instead of reproducing it.
        flags: ModuleFlags { amiga_limits: false, linear_slides: false, fast_volume_slides: false, stereo: true },
        // MultiTracker is its own dialect of the MOD command vocabulary: `Dxx` is
        // hexadecimal, `F00` is a no-op and there is no queued sample swap.
        dialect: starplayer_core::quirks::FormatDialect::MultiTracker,
        format_extra,
    });
    builder.build()
}

pub fn track_count(module: &Module) -> Option<u16> {
    (module.header().format == ModuleFormat::Mtm).then_some((module.header().format_extra & FORMAT_TRACK_MASK) as u16)
}

pub fn tempo_mode(module: &Module) -> Option<TempoMode> {
    (module.header().format == ModuleFormat::Mtm).then_some(if module.header().format_extra & FORMAT_NATIVE_TEMPO_RESETS != 0 {
        TempoMode::MultiTracker
    } else {
        TempoMode::DualModulePlayer
    })
}

fn expand_pattern(tracks: &[u8], track_count: u16, table: &[u8], channel_count: u8) -> Vec<u8> {
    let mut pattern = vec![0; ROWS as usize * channel_count as usize * CELL_BYTES];
    for channel in 0..channel_count as usize {
        let reference_offset = channel * 2;
        let track_number = table.get(reference_offset..reference_offset + 2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]])).unwrap_or(0);
        // libxmp and the original both treat track zero as empty. libxmp also maps an
        // invalid reference to zero, which is the safe recovery used here.
        if track_number == 0 || track_number > track_count { continue; }
        let source_start = (track_number as usize - 1) * TRACK_BYTES;
        let Some(track) = tracks.get(source_start..source_start + TRACK_BYTES) else { continue };
        for row in 0..ROWS as usize {
            let source_cell = row * CELL_BYTES;
            let target_cell = (row * channel_count as usize + channel) * CELL_BYTES;
            pattern[target_cell..target_cell + CELL_BYTES].copy_from_slice(&track[source_cell..source_cell + CELL_BYTES]);
        }
    }
    pattern
}

fn detect_tempo_mode(tracks: &[u8], patterns: &[Vec<u8>], channel_count: u8) -> TempoMode {
    let mut low = false;
    let mut high = false;
    for cell in tracks.chunks_exact(CELL_BYTES) {
        if cell[1] & 15 == 0xF {
            if cell[2] < 0x20 { low = true; } else { high = true; }
        }
    }
    if !(low && high) { return TempoMode::MultiTracker; }
    for pattern in patterns {
        for row in pattern.chunks_exact(channel_count as usize * CELL_BYTES) {
            let mut row_low = false;
            let mut row_high = false;
            for cell in row.chunks_exact(CELL_BYTES) {
                if cell[1] & 15 == 0xF {
                    if cell[2] < 0x20 { row_low = true; } else { row_high = true; }
                }
            }
            if row_low && row_high { return TempoMode::DualModulePlayer; }
        }
    }
    TempoMode::MultiTracker
}

pub(crate) fn mtm_pan(value: u8) -> I1F15 { bipolar_from_ratio(value.min(15) as i32 * 2 - 15, 15) }

fn le_u16(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    match bytes.get(offset..offset + 2) {
        Some([low, high]) => Ok(u16::from_le_bytes([*low, *high])),
        _ => Err(Error::Truncated { offset, needed: 2 }),
    }
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    match bytes.get(offset..offset + 4) {
        Some([a, b, c, d]) => Ok(u32::from_le_bytes([*a, *b, *c, *d])),
        _ => Err(Error::Truncated { offset, needed: 4 }),
    }
}

fn checked_add(left: usize, right: usize, what: &'static str) -> Result<usize, Error> {
    left.checked_add(right).ok_or(Error::TooLarge(what))
}

fn checked_mul(left: usize, right: usize, what: &'static str) -> Result<usize, Error> {
    left.checked_mul(right).ok_or(Error::TooLarge(what))
}

struct Source<'reader, R: ModuleReader + ?Sized> { reader: &'reader R }

impl<R: ModuleReader + ?Sized> Source<'_, R> {
    fn len(&self) -> usize { self.reader.len() }

    fn array<const N: usize>(&self, offset: usize) -> Result<[u8; N], Error> {
        let mut result = [0; N];
        self.reader.read_at(offset, &mut result)?;
        Ok(result)
    }

    fn with_slice<T>(&self, offset: usize, length: usize, consume: impl FnOnce(&[u8]) -> T) -> Result<T, Error> {
        if let Some(slice) = self.reader.slice_at(offset, length) { return Ok(consume(slice)); }
        let mut buffer = vec![0; length];
        self.reader.read_at(offset, &mut buffer)?;
        Ok(consume(&buffer))
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::{MtmCell, PatternView};
    use starplayer_core::SampleId;
    use starplayer_model::PatternId;

    #[derive(Copy, Clone)]
    struct TestSample {
        length: u32,
        loop_start: u32,
        loop_end: u32,
        finetune: u8,
        volume: u8,
        attribute: u8,
    }

    fn synthetic_mtm(samples: &[TestSample], tracks: &[[u8; TRACK_BYTES]], pattern_tracks: &[[u16; 32]], channels: u8) -> Vec<u8> {
        let sample_headers_bytes = samples.len() * SAMPLE_HEADER_BYTES;
        let order_offset = HEADER_BYTES + sample_headers_bytes;
        let track_offset = order_offset + ORDER_BYTES;
        let pattern_offset = track_offset + tracks.len() * TRACK_BYTES;
        let sample_offset = pattern_offset + pattern_tracks.len() * PATTERN_TABLE_BYTES;
        let sample_bytes: usize = samples.iter().map(|sample| sample.length as usize).sum();
        let mut bytes = vec![0; sample_offset + sample_bytes];
        bytes[..4].copy_from_slice(b"MTM\x10");
        bytes[4..13].copy_from_slice(b"unit test");
        bytes[24..26].copy_from_slice(&(tracks.len() as u16).to_le_bytes());
        bytes[26] = pattern_tracks.len().saturating_sub(1) as u8;
        bytes[27] = pattern_tracks.len().saturating_sub(1) as u8;
        bytes[30] = samples.len() as u8;
        bytes[32] = ROWS as u8;
        bytes[33] = channels;
        for channel in 0..32 { bytes[34 + channel] = (channel % 16) as u8; }
        for (index, sample) in samples.iter().enumerate() {
            let offset = HEADER_BYTES + index * SAMPLE_HEADER_BYTES;
            bytes[offset..offset + 6].copy_from_slice(b"sample");
            bytes[offset + 22..offset + 26].copy_from_slice(&sample.length.to_le_bytes());
            bytes[offset + 26..offset + 30].copy_from_slice(&sample.loop_start.to_le_bytes());
            bytes[offset + 30..offset + 34].copy_from_slice(&sample.loop_end.to_le_bytes());
            bytes[offset + 34] = sample.finetune;
            bytes[offset + 35] = sample.volume;
            bytes[offset + 36] = sample.attribute;
        }
        for pattern in 0..pattern_tracks.len() { bytes[order_offset + pattern] = pattern as u8; }
        for (index, track) in tracks.iter().enumerate() {
            let offset = track_offset + index * TRACK_BYTES;
            bytes[offset..offset + TRACK_BYTES].copy_from_slice(track);
        }
        for (pattern, references) in pattern_tracks.iter().enumerate() {
            let offset = pattern_offset + pattern * PATTERN_TABLE_BYTES;
            for (channel, reference) in references.iter().enumerate() {
                bytes[offset + channel * 2..offset + channel * 2 + 2].copy_from_slice(&reference.to_le_bytes());
            }
        }
        let mut cursor = sample_offset;
        for sample in samples {
            for position in 0..sample.length as usize {
                bytes[cursor + position] = if position == 0 { 0x80 } else { 0xFF };
            }
            cursor += sample.length as usize;
        }
        bytes
    }

    fn track(first: MtmCell) -> [u8; TRACK_BYTES] {
        let mut track = [0; TRACK_BYTES];
        track[..CELL_BYTES].copy_from_slice(&first.to_bytes());
        track
    }

    fn references(first: u16, second: u16) -> [u16; 32] {
        let mut references = [0; 32];
        references[0] = first;
        references[1] = second;
        references
    }

    #[test]
    fn native_layout_decodes_unsigned_pcm_pan_finetune_and_disables_amiga_limits() {
        let sample = TestSample { length: 8, loop_start: 0, loop_end: 5, finetune: 15, volume: 80, attribute: 0 };
        let note = MtmCell { pitch: 12, instrument: 1, effect: 0, param: 0 };
        let module = load(&synthetic_mtm(&[sample], &[track(note)], &[references(1, 0)], 2)).expect("valid MTM");
        assert_eq!(module.header().format, ModuleFormat::Mtm);
        assert_eq!(module.header().title.as_ref(), "unit test");
        assert_eq!(module.header().channel_count, 2);
        assert_eq!(track_count(&module), Some(1));
        assert_eq!(module.patterns().len(), 1);
        assert_eq!(module.sample_pcm(SampleId(0)).and_then(|pcm| pcm.first()).copied(), Some(0), "unsigned 0x80 is silence");
        assert_eq!(module.sample(SampleId(0)).map(|entry| entry.reference_rate_hz()), Some(FINETUNE_REFERENCE_RATES[15]));
        assert_eq!(module.sample(SampleId(0)).map(|entry| entry.default_volume()), Some(U0F16::MAX));
        assert_eq!(module.sample(SampleId(0)).map(|entry| entry.loop_mode()), Some(LoopMode::Forward));
        assert!(module.header().channel_pan(0).is_some_and(|pan| pan < I1F15::ZERO));
        assert!(!module.header().flags.amiga_limits);
    }

    #[test]
    fn track_indirection_shares_native_cells_and_zero_is_empty() {
        let note = MtmCell { pitch: 24, instrument: 3, effect: 0xA, param: 0x0F };
        let module = load(&synthetic_mtm(&[], &[track(note)], &[references(1, 0), references(1, 0)], 2)).expect("valid MTM");
        let first = PatternView::new(&module, PatternId(0)).expect("pattern zero");
        let second = PatternView::new(&module, PatternId(1)).expect("pattern one");
        assert_eq!(first.cell(0, 0), Some(note));
        assert_eq!(second.cell(0, 0), Some(note));
        assert_eq!(first.cell(0, 1), Some(MtmCell::EMPTY));
    }

    #[test]
    fn loop_gate_requires_more_than_four_bytes() {
        let four = TestSample { length: 8, loop_start: 1, loop_end: 5, finetune: 0, volume: 64, attribute: 0 };
        let five = TestSample { length: 8, loop_start: 1, loop_end: 6, finetune: 0, volume: 64, attribute: 0 };
        let module = load(&synthetic_mtm(&[four, five], &[], &[references(0, 0)], 1)).expect("valid MTM");
        assert_eq!(module.sample(SampleId(0)).map(|entry| entry.loop_mode()), Some(LoopMode::None));
        assert_eq!(module.sample(SampleId(1)).map(|entry| entry.loop_mode()), Some(LoopMode::Forward));
    }

    #[test]
    fn sixteen_bit_samples_are_an_explicit_researched_exclusion() {
        let sample = TestSample { length: 8, loop_start: 0, loop_end: 0, finetune: 0, volume: 64, attribute: 1 };
        assert_eq!(load(&synthetic_mtm(&[sample], &[], &[references(0, 0)], 1)), Err(Error::Unsupported("16-bit MTM samples")));
    }

    #[test]
    fn a_tiny_file_with_a_u32_max_sample_is_rejected_before_sample_allocation() {
        let sample = TestSample { length: 0, loop_start: 0, loop_end: 0, finetune: 0, volume: 64, attribute: 0 };
        let mut bytes = synthetic_mtm(&[sample], &[], &[references(0, 0)], 1);
        bytes[HEADER_BYTES + 22..HEADER_BYTES + 26].copy_from_slice(&u32::MAX.to_le_bytes());
        let sample_data_offset = HEADER_BYTES + SAMPLE_HEADER_BYTES + ORDER_BYTES + PATTERN_TABLE_BYTES;
        assert_eq!(load(&bytes), Err(Error::Truncated { offset: sample_data_offset, needed: u32::MAX as usize }));
    }

    #[test]
    fn tempo_personality_detects_native_and_same_row_dmp_usage() {
        let low = track(MtmCell { effect: 0xF, param: 6, ..MtmCell::EMPTY });
        let mut high = track(MtmCell { effect: 0xF, param: 125, ..MtmCell::EMPTY });
        high[..CELL_BYTES].fill(0);
        high[CELL_BYTES..CELL_BYTES * 2].copy_from_slice(&MtmCell { effect: 0xF, param: 125, ..MtmCell::EMPTY }.to_bytes());
        let native = load(&synthetic_mtm(&[], &[low, high], &[references(1, 2)], 2)).expect("native timing");
        assert_eq!(tempo_mode(&native), Some(TempoMode::MultiTracker));

        let same_row_high = track(MtmCell { effect: 0xF, param: 125, ..MtmCell::EMPTY });
        let dmp = load(&synthetic_mtm(&[], &[low, same_row_high], &[references(1, 2)], 2)).expect("DMP timing");
        assert_eq!(tempo_mode(&dmp), Some(TempoMode::DualModulePlayer));
    }

    #[test]
    fn version_rows_channels_and_sample_width_are_gated() {
        let valid = synthetic_mtm(&[], &[], &[references(0, 0)], 1);
        assert!(probe(&valid));
        let mut version = valid.clone();
        version[3] = 0x11;
        assert!(!probe(&version));
        assert_eq!(load(&version), Err(Error::BadMagic));
        let mut rows = valid.clone();
        rows[32] = 32;
        assert_eq!(load(&rows), Err(Error::Unsupported("MTM tracks not containing 64 rows")));
        let mut channels = valid;
        channels[33] = 0;
        assert_eq!(load(&channels), Err(Error::Invalid("MTM channel count must be 1..=32")));
    }

    // ── C3b: the loader is no longer stricter than every reference ─────────────────

    #[test]
    fn a_last_order_past_the_stored_table_clamps_instead_of_erroring() {
        let note = MtmCell { pitch: 12, instrument: 0, effect: 0, param: 0 };
        let mut bytes = synthetic_mtm(&[], &[track(note)], &[references(1, 0)], 1);
        bytes[27] = 200;
        let module = load(&bytes).expect("libxmp and the DOS original both keep playing");
        assert_eq!(module.orders().len(), ORDER_BYTES + 1, "the 128 stored entries plus the end marker");
        assert_eq!(module.order_entry(0), Some(starplayer_model::OrderEntry::Pattern(PatternId(0))));
    }

    #[test]
    fn a_nonzero_module_attribute_byte_is_ignored_rather_than_refused() {
        let mut bytes = synthetic_mtm(&[], &[], &[references(0, 0)], 1);
        bytes[31] = 0x5A;
        assert!(load(&bytes).is_ok(), "byte 31 is documented always-zero and read-and-ignored by libxmp");
    }

    #[test]
    fn an_order_naming_an_unstored_pattern_is_stepped_over() {
        let note = MtmCell { pitch: 12, instrument: 0, effect: 0, param: 0 };
        let mut bytes = synthetic_mtm(&[], &[track(note)], &[references(1, 0), references(1, 0)], 1);
        bytes[27] = 2;
        // No sample headers, so the order table starts immediately after the header.
        bytes[HEADER_BYTES + 2] = 9;
        let module = load(&bytes).expect("an out-of-range order is recovery, not a refusal");
        assert_eq!(module.order_entry(0), Some(starplayer_model::OrderEntry::Pattern(PatternId(0))));
        assert_eq!(module.order_entry(1), Some(starplayer_model::OrderEntry::Pattern(PatternId(1))));
        assert_eq!(module.order_entry(2), Some(starplayer_model::OrderEntry::Marker), "the same recovery the track table already uses");
        assert_eq!(module.order_entry(3), Some(starplayer_model::OrderEntry::End));
    }
}
