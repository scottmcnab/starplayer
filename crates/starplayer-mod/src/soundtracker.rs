//! Explicit loading for the tagless 15-sample Soundtracker layout.
//!
//! With no signature, autodetection must inspect the complete header and patterns.
//! This deliberately accepts a conservative subset: complete sample bodies, ordinary
//! Amiga periods and the 0/1/2/C/E0x/F commands found in the legacy library. It does
//! not claim every later tracker extension or damaged Soundtracker rip.

use starplayer_core::{Error, quirks::FormatDialect};
use starplayer_model::{Module, ModuleReader};
use crate::loader::{LoadOptions, ModLayout, be_u16, load_layout_from};
use crate::processor::EffectCell;
use crate::tables::PROTRACKER_PERIODS;

const HEADER_BYTES: usize = 600;
const PATTERN_BYTES: usize = 1024;

/// A conservative, allocation-free structural probe, independent of filename.
pub fn probe(bytes: &[u8]) -> bool { probe_reader(bytes) }
pub fn probe_reader<R: ModuleReader + ?Sized>(reader: &R) -> bool { layout_from_reader(reader).is_ok() }

pub fn load(bytes: &[u8]) -> Result<Module, Error> { load_from(bytes) }
pub fn load_with_options(bytes: &[u8], options: LoadOptions) -> Result<Module, Error> { load_from_with_options(bytes, options) }
pub fn load_from<R: ModuleReader + ?Sized>(reader: &R) -> Result<Module, Error> { load_from_with_options(reader, LoadOptions::default()) }
pub fn load_from_with_options<R: ModuleReader + ?Sized>(reader: &R, options: LoadOptions) -> Result<Module, Error> {
    let layout = layout_from_reader(reader)?;
    load_layout_from(reader, options, layout)
}

fn valid_name(bytes: &[u8]) -> bool { bytes.iter().all(|byte| *byte == 0 || (0x20..=0x7e).contains(byte)) }

pub(crate) fn header_layout_from_reader<R: ModuleReader + ?Sized>(reader: &R) -> Result<ModLayout, Error> {
    // Tagged MODs must retain their own layout even if their first 600 bytes happen
    // to look plausible here. Strong signatures in the public facade win earlier.
    if crate::loader::probe_reader(reader) { return Err(Error::BadMagic); }
    let mut header = [0u8; HEADER_BYTES];
    reader.read_at(0, &mut header)?;
    let song_length = header.get(470).copied().unwrap_or(0) as usize;
    let tempo = header.get(471).copied().unwrap_or(0);
    if song_length == 0 || song_length > 128 || !valid_name(header.get(..20).ok_or(Error::BadMagic)?) { return Err(Error::BadMagic); }
    let orders = header.get(472..600).ok_or(Error::BadMagic)?;
    if orders.iter().any(|order| *order >= 128) { return Err(Error::BadMagic); }
    let pattern_count = orders.iter().copied().max().unwrap_or(0) as usize + 1;
    let mut sample_bytes = 0usize;
    let mut ultimate = tempo >= 64;
    for index in 0..15 {
        let offset = 20 + index * 30;
        let sample = header.get(offset..offset + 30).ok_or(Error::BadMagic)?;
        let length_words = be_u16(sample, 22)? as usize;
        let loop_start = be_u16(sample, 26)? as usize;
        let loop_words = be_u16(sample, 28)? as usize;
        let finetune = sample.get(24).copied().unwrap_or(0);
        let volume = sample.get(25).copied().unwrap_or(0);
        if !valid_name(sample.get(..22).ok_or(Error::BadMagic)?) || finetune > 15 || volume > 64
            || length_words > 0x8000 || loop_words > 0x8000 || loop_start > length_words * 2
            || (loop_words > 1 && loop_start + loop_words * 2 > length_words * 2) {
            return Err(Error::BadMagic);
        }
        sample_bytes += length_words * 2;
        ultimate &= finetune == 0 && length_words <= 0x1387 && loop_start <= 9999 && loop_words <= 0x1387;
    }
    if sample_bytes < 8 { return Err(Error::BadMagic); }
    let expected_bytes = HEADER_BYTES + pattern_count * PATTERN_BYTES + sample_bytes;
    // Tagged MOD retains its permissive sample-tail policy; signatureless detection
    // requires the whole declared body so random/truncated inputs cannot pass on a
    // plausible header alone. Trailing bytes are harmless (Blood Money has 24).
    if reader.len() < expected_bytes { return Err(Error::Truncated { offset: HEADER_BYTES, needed: expected_bytes - HEADER_BYTES }); }
    Ok(ModLayout {
        channels: 4, soundtracker: true, paired_four_channel_patterns: false, extra_header_bytes: 0,
        dialect: if ultimate { FormatDialect::UltimateSoundtracker } else { FormatDialect::Soundtracker },
    })
}

pub(crate) fn layout_from_reader<R: ModuleReader + ?Sized>(reader: &R) -> Result<ModLayout, Error> {
    let mut layout = header_layout_from_reader(reader)?;
    let mut orders = [0u8; 128];
    reader.read_at(472, &mut orders)?;
    let pattern_count = orders.iter().copied().max().unwrap_or(0) as usize + 1;
    let mut pattern = [0u8; PATTERN_BYTES];
    let mut has_note = false;
    for index in 0..pattern_count {
        reader.read_at(HEADER_BYTES + index * PATTERN_BYTES, &mut pattern)?;
        for bytes in pattern.chunks_exact(4) {
            let cell = crate::ModCell::from_bytes(bytes).ok_or(Error::BadMagic)?;
            scan_cell(&mut layout, cell)?;
            has_note |= cell.period != 0;
        }
    }
    if !has_note { return Err(Error::BadMagic); }
    Ok(layout)
}

pub(crate) fn scan_cell(layout: &mut ModLayout, cell: crate::ModCell) -> Result<(), Error> {
    if cell.instrument > 15 || (cell.period != 0 && !PROTRACKER_PERIODS[0].contains(&cell.period)) { return Err(Error::BadMagic); }
    match cell.effect {
        0 | 1 | 2 | 0xC | 0xF => {}
        0xE if cell.param <= 1 => {}
        _ => return Err(Error::Unsupported("Soundtracker effect")),
    }
    let ultimate = match cell.effect {
        0 => cell.param == 0,
        1 => cell.param != 0,
        2 => cell.param & 0x0f == 0 || cell.param & 0xf0 == 0,
        _ => false,
    };
    if !ultimate { layout.dialect = FormatDialect::Soundtracker; }
    Ok(())
}

/// Decode UST's command vocabulary at execution time; native packed cells remain intact.
pub(crate) fn ultimate_effect(mut cell: EffectCell) -> EffectCell {
    match cell.effect {
        1 => cell.effect = 0,
        2 if cell.param & 0xf0 == 0 => cell.effect = 1,
        2 => cell.param >>= 4,
        _ => {}
    }
    cell
}

// Low five format_extra bits belong to timing evidence. Keep legacy restart separate.
pub(crate) fn encode_restart(layout: ModLayout, header: &[u8]) -> u32 {
    if layout.dialect != FormatDialect::Soundtracker { return 0; }
    let restart = header.get(471).copied().unwrap_or(0);
    let length = header.get(470).copied().unwrap_or(0);
    if restart < length { (restart as u32) << 8 } else { 0 }
}

/// The valid restart in a later Soundtracker header; UST's same byte is BPM instead.
pub fn restart_order(module: &Module) -> u16 {
    if module.header().dialect == FormatDialect::Soundtracker { ((module.header().format_extra >> 8) & 0x7f) as u16 } else { 0 }
}
