//! Page-side native module loader and display view.
//!
//! This is deliberately a second wasm instance, separate from the AudioWorklet engine.
//! It validates untrusted bytes and exposes title/instrument metadata plus windowed
//! [`PatternCell`](starplayer::model::PatternCell) rows on the page thread. A failed load
//! never reaches the worklet, so it cannot disturb the live audio graph.

#![deny(unsafe_code)]

use std::cell::RefCell;
use std::vec::Vec;

use starplayer::model::{EffectNames, Module, ModuleFormat, NoteCell, PatternCell, PatternId, s3m_command_code};
use starplayer::mod_file::PatternView as ModPatternView;
use starplayer::s3m::PatternView as S3mPatternView;
use starplayer_archive::{ArchiveError, extract, is_zip, list_modules};

const DISPLAY_CELL_BYTES: usize = 5;
const NOTE_NONE: u8 = 255;
const NOTE_CUT: u8 = 254;
const NOTE_OFF: u8 = 253;
const VALUE_NONE: u8 = 255;

thread_local! {
    static MODULE: RefCell<Option<Module>> = const { RefCell::new(None) };
}

fn with_module<T>(fallback: T, action: impl FnOnce(&Module) -> T) -> T {
    MODULE.with(|cell| match cell.try_borrow() {
        Ok(slot) => slot.as_ref().map(action).unwrap_or(fallback),
        Err(_) => fallback,
    })
}

fn inspect(bytes: &[u8]) -> Result<(), String> {
    // `load(&[u8])` is ModuleReader's borrowing fast path: the loader borrows pattern and
    // sample ranges directly instead of first copying them into intermediate buffers.
    let module = starplayer::load(bytes).map_err(|error| error.to_string())?;
    MODULE.with(|cell| {
        let mut slot = cell.try_borrow_mut().map_err(|_| String::from("the page-side loader is busy"))?;
        *slot = Some(module);
        Ok(())
    })
}

/// Serialise the whole English effect-name table once, so the page reads the names the
/// engine reports instead of keeping a second copy of them in JavaScript.
///
/// One `code:nybble:name` record per line. `nybble` is `-1` when the name depends only on
/// the command code, and the high nybble of the parameter for S3M's `S` sub-commands —
/// the same two cases [`EffectNames::name`] itself distinguishes.
fn effect_name_records() -> String {
    with_module(String::new(), |module| {
        let mut records = String::new();
        match module.header().format {
            ModuleFormat::S3m => {
                let subcommand_code = s3m_command_code(b'S');
                for code in 1..=26u8 {
                    if code == subcommand_code {
                        for nybble in 0..=0x0Fu8 {
                            if let Some(name) = EffectNames::S3M.name(code, nybble << 4) {
                                records.push_str(&format!("{code}:{nybble}:{name}\n"));
                            }
                        }
                    } else if let Some(name) = EffectNames::S3M.name(code, 0) {
                        records.push_str(&format!("{code}:-1:{name}\n"));
                    }
                }
            }
            ModuleFormat::Mod => {
                for code in 0..=0x0Fu8 {
                    if code == 0xE {
                        for nybble in 0..=0x0Fu8 {
                            if let Some(name) = EffectNames::MOD.name(code, nybble << 4) {
                                records.push_str(&format!("{code}:{nybble}:{name}\n"));
                            }
                        }
                    } else if let Some(name) = EffectNames::MOD.name(code, 1) {
                        records.push_str(&format!("{code}:-1:{name}\n"));
                    }
                }
            }
            _ => {}
        }
        records
    })
}

fn archive_module_records(bytes: &[u8]) -> Result<String, ArchiveError> {
    let entries = match list_modules(bytes) {
        Ok(entries) => entries,
        Err(ArchiveError::NoModules) => Vec::new(),
        Err(error) => return Err(error),
    };
    let mut records = String::new();
    for entry in entries.into_iter().filter(|entry| matches!(entry.format, ModuleFormat::S3m | ModuleFormat::Mod)) {
        let safe_name = entry.name.replace(['\t', '\r', '\n'], " ");
        records.push_str(&format!("{}\t{}\t{}\n", entry.index, safe_name, entry.size));
    }
    Ok(records)
}

fn pattern_window_bytes(pattern: u16, first_row: u16, row_count: u16) -> Vec<u8> {
    with_module(Vec::new(), |module| {
        let mut output = Vec::new();
        match module.header().format {
            ModuleFormat::S3m => {
                let Some(view) = S3mPatternView::new(module, PatternId(pattern)) else { return output };
                let rows = row_count.min(view.rows().saturating_sub(first_row));
                output.reserve(rows as usize * view.channels() as usize * DISPLAY_CELL_BYTES);
                for row in first_row..first_row.saturating_add(rows) {
                    for channel in 0..view.channels() { append_display(&mut output, view.cell(row, channel).map(|cell| cell.display()).unwrap_or_default()); }
                }
            }
            ModuleFormat::Mod => {
                let Some(view) = ModPatternView::new(module, PatternId(pattern)) else { return output };
                let rows = row_count.min(view.rows().saturating_sub(first_row));
                output.reserve(rows as usize * view.channels() as usize * DISPLAY_CELL_BYTES);
                for row in first_row..first_row.saturating_add(rows) {
                    for channel in 0..view.channels() { append_display(&mut output, view.cell(row, channel).map(|cell| cell.display()).unwrap_or_default()); }
                }
            }
            _ => {}
        }
        output
    })
}

fn append_display(output: &mut Vec<u8>, display: PatternCell) {
    let note = match display.note {
        NoteCell::None => NOTE_NONE,
        NoteCell::Cut => NOTE_CUT,
        NoteCell::Off => NOTE_OFF,
        NoteCell::Note(semitone) => semitone,
    };
    output.push(note);
    output.push(display.instrument.unwrap_or(0));
    output.push(display.volume.unwrap_or(VALUE_NONE));
    output.push(display.effect.map(|effect| effect.code).unwrap_or(0));
    output.push(display.effect.map(|effect| effect.param).unwrap_or(0));
}

#[allow(unsafe_code, reason = "`#[wasm_bindgen]` expands to unsafe ABI shims")]
mod exports {
    use super::*;
    use wasm_bindgen::prelude::{JsValue, wasm_bindgen};

    /// Validate and retain a supported native module. The previous valid module survives
    /// an error. The legacy export name is retained for the existing page glue.
    #[wasm_bindgen]
    pub fn inspect_s3m(bytes: &[u8]) -> Result<(), JsValue> {
        inspect(bytes).map_err(|message| JsValue::from_str(&message))
    }

    /// Identify ZIP bytes before the page asks the archive decoder to inspect them.
    #[wasm_bindgen]
    pub fn is_archive(bytes: &[u8]) -> bool { is_zip(bytes) }

    /// List the S3M and MOD entries this web player can offer, one tab record per line.
    #[wasm_bindgen]
    pub fn archive_modules(bytes: &[u8]) -> Result<String, JsValue> {
        archive_module_records(bytes).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Extract one central-directory entry into page-owned module bytes.
    #[wasm_bindgen]
    pub fn archive_extract(bytes: &[u8], index: u32) -> Result<Vec<u8>, JsValue> {
        extract(bytes, index as usize).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    #[wasm_bindgen]
    pub fn module_title() -> String { with_module(String::new(), |module| module.header().title.to_string()) }

    #[wasm_bindgen]
    pub fn module_channel_count() -> u32 { with_module(0, |module| module.header().channel_count as u32) }

    #[wasm_bindgen]
    pub fn module_order_count() -> u32 { with_module(0, |module| module.orders().len() as u32) }

    #[wasm_bindgen]
    pub fn module_pattern_count() -> u32 { with_module(0, |module| module.patterns().len() as u32) }

    #[wasm_bindgen]
    pub fn module_instrument_count() -> u32 { with_module(0, |module| module.instruments().len() as u32) }

    #[wasm_bindgen]
    pub fn instrument_name(index: u32) -> String {
        with_module(String::new(), |module| {
            module.instruments().get(index as usize).map(|instrument| instrument.name.to_string()).unwrap_or_default()
        })
    }

    #[wasm_bindgen]
    pub fn instrument_sample_length(index: u32) -> u32 {
        with_module(0, |module| {
            module.instruments().get(index as usize)
                .and_then(|instrument| instrument.sample)
                .and_then(|sample| module.sample(sample))
                .map(|sample| sample.length_frames())
                .unwrap_or(0)
        })
    }

    #[wasm_bindgen]
    pub fn pattern_row_count(pattern: u32) -> u32 {
        with_module(0, |module| module.pattern(PatternId(pattern as u16)).map(|index| index.rows() as u32).unwrap_or(0))
    }

    #[wasm_bindgen]
    pub fn pattern_channel_count(pattern: u32) -> u32 {
        with_module(0, |module| module.pattern(PatternId(pattern as u16)).map(|index| index.channels() as u32).unwrap_or(0))
    }

    /// Packed display cells: note, instrument, volume, effect code, effect parameter.
    #[wasm_bindgen]
    pub fn pattern_window(pattern: u32, first_row: u32, row_count: u32) -> Vec<u8> {
        pattern_window_bytes(pattern as u16, first_row as u16, row_count.min(u16::MAX as u32) as u16)
    }

    /// The current module format's effect-name table. See [`effect_name_records`].
    #[wasm_bindgen]
    pub fn effect_names() -> String { effect_name_records() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipWriter};

    const FIXTURE: &[u8] = include_bytes!("../../../crates/starplayer-s3m/tests/fixtures/REFLEX.S3M");

    fn minimal_mod() -> Vec<u8> {
        let mut bytes = vec![0; 1084 + 64 * 4 * 4 + 8];
        bytes[..10].copy_from_slice(b"native mod");
        bytes[42..44].copy_from_slice(&4u16.to_be_bytes());
        bytes[45] = 64;
        bytes[950] = 1;
        bytes[1080..1084].copy_from_slice(b"M.K.");
        bytes[1084..1088].copy_from_slice(&starplayer::mod_file::ModCell { period: 428, instrument: 1, effect: 0xF, param: 6 }.to_bytes());
        bytes
    }

    #[test]
    fn page_loader_exposes_metadata_and_pattern_cells() {
        assert!(inspect(FIXTURE).is_ok());
        assert!(with_module(0, |module| module.orders().len()) > 0);
        let window = pattern_window_bytes(0, 0, 8);
        assert!(!window.is_empty());
        assert_eq!(window.len() % DISPLAY_CELL_BYTES, 0);
    }

    #[test]
    fn the_effect_name_table_reaches_the_page_from_the_engine_s_own_table() {
        assert!(inspect(FIXTURE).is_ok());
        let records = effect_name_records();
        assert!(records.contains(&format!("{}:-1:vibrato\n", s3m_command_code(b'H'))), "{records}");
        assert!(records.contains(&format!("{}:12:note cut\n", s3m_command_code(b'S'))), "{records}");
        let codes: Vec<&str> = records.lines().filter_map(|line| line.split(':').next()).collect();
        assert!(!codes.contains(&s3m_command_code(b'M').to_string().as_str()), "S3M has no M command");
    }

    #[test]
    fn page_loader_and_pattern_view_dispatch_to_native_mod() {
        assert!(inspect(&minimal_mod()).is_ok());
        assert_eq!(with_module(ModuleFormat::S3m, |module| module.header().format), ModuleFormat::Mod);
        assert_eq!(pattern_window_bytes(0, 0, 1).get(..5), Some(&[48, 1, VALUE_NONE, 0xF, 6][..]));
        assert!(effect_name_records().contains("15:-1:set speed/tempo\n"));
    }

    #[test]
    fn browser_refreshes_format_specific_effect_names_before_rendering_metadata() {
        let app = include_str!("../www/app.js");
        let load_buffer = app.split_once("async function loadBuffer").expect("loadBuffer").1;
        let inspect = load_buffer.find("Loader.inspect_s3m(new Uint8Array(moduleBuffer));").expect("inspect call");
        let refresh = load_buffer.find("loadEffectNames();").expect("effect-name refresh");
        let metadata = load_buffer.find("const metadata = readMetadata(moduleLabel);").expect("metadata render");
        assert!(inspect < refresh && refresh < metadata, "the newly inspected format table must be loaded before page rendering");
    }

    #[test]
    fn malformed_input_does_not_replace_the_last_good_module() {
        assert!(inspect(FIXTURE).is_ok());
        let title = with_module(String::new(), |module| module.header().title.to_string());
        assert!(inspect(b"broken").is_err());
        assert_eq!(with_module(String::new(), |module| module.header().title.to_string()), title);
    }

    #[test]
    fn archive_modules_uses_tab_separated_page_records() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file("REFLEX.S3M", SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)).unwrap();
        writer.write_all(b"s3m").unwrap();
        writer.start_file("native.mod", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"mod").unwrap();
        writer.start_file("readme.txt", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"notes").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert_eq!(archive_module_records(&bytes).unwrap(), "0\tREFLEX.S3M\t3\n1\tnative.mod\t3\n");
    }
}
