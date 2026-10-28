//! Page-side native module loader and display view.
//!
//! This is deliberately a second wasm instance, separate from the AudioWorklet engine.
//! It validates untrusted bytes and exposes title/instrument metadata plus windowed
//! [`PatternCell`](starplayer::model::PatternCell) rows on the page thread. A failed load
//! never reaches the worklet, so it cannot disturb the live audio graph.

#![deny(unsafe_code)]

use std::cell::RefCell;
use std::vec::Vec;

use starplayer::model::{EffectNames, Module, ModuleFormat, NoteCell, PatternCell, PatternId, it_command_code, s3m_command_code};
use starplayer::mod_file::PatternView as ModPatternView;
use starplayer::mtm::PatternView as MtmPatternView;
use starplayer::xm::PatternView as XmPatternView;
use starplayer::it::PatternView as ItPatternView;
use starplayer::s3m::PatternView as S3mPatternView;
use starplayer_archive::{ArchiveError, extract, find_entry, is_zip, list_modules};

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

/// Source PCM only: native maps name one-based sample IDs, while older formats
/// and IT sample mode name one direct sample. Guard frames are not source frames.
fn instrument_source_frames(module: &Module, index: usize) -> u32 {
    let Some(instrument) = module.instruments().get(index) else { return 0; };
    let native_map = match module.header().format {
        ModuleFormat::Xm => true,
        ModuleFormat::It => starplayer::it::ItFormatExtra::from_header(module.header()).is_instrument_mode(),
        _ => false,
    };
    if !native_map {
        return instrument.sample.and_then(|sample| module.sample(sample)).map(|sample| sample.length_frames()).unwrap_or(0);
    }
    let mut frames = 0u32;
    for (note, &sample) in instrument.note_sample_map.iter().enumerate() {
        if sample == 0 || instrument.note_sample_map[..note].contains(&sample) { continue; }
        if let Some(sample) = module.sample(starplayer::core::SampleId(sample - 1)) {
            frames = frames.saturating_add(sample.length_frames());
        }
    }
    frames
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
            ModuleFormat::Mod | ModuleFormat::Mtm => {
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
            ModuleFormat::Xm => {
                // XM names its commands `0`..`9`, `A`..`F` and then `G`..`X`, and its `Exy`
                // sub-commands the way MOD does.
                for code in 0..=0x21u8 {
                    if code == 0xE {
                        for nybble in 0..=0x0Fu8 {
                            if let Some(name) = EffectNames::XM.name(code, nybble << 4) {
                                records.push_str(&format!("{code}:{nybble}:{name}\n"));
                            }
                        }
                    } else if let Some(name) = EffectNames::XM.name(code, 1) {
                        records.push_str(&format!("{code}:-1:{name}\n"));
                    }
                }
            }
            ModuleFormat::It => {
                let subcommand_code = it_command_code(b'S');
                for code in 1..=26u8 {
                    if code == subcommand_code {
                        for nybble in 0..=0x0Fu8 {
                            if let Some(name) = EffectNames::IT.name(code, nybble << 4) {
                                records.push_str(&format!("{code}:{nybble}:{name}\n"));
                            }
                        }
                    } else if let Some(name) = EffectNames::IT.name(code, 0) {
                        records.push_str(&format!("{code}:-1:{name}\n"));
                    }
                }
            }
        }
        records
    })
}

fn nested_archive_bytes(bytes: &[u8], container_index: usize) -> Result<Vec<u8>, String> {
    let inner = extract(bytes, container_index).map_err(|error| error.to_string())?;
    if !is_zip(&inner) { return Err("nested container is not a ZIP archive".to_string()); }
    Ok(inner)
}

fn extract_nested(bytes: &[u8], container_index: i32, member_index: u32) -> Result<Vec<u8>, String> {
    let inner;
    let archive = if container_index < 0 { bytes } else {
        inner = nested_archive_bytes(bytes, container_index as usize)?;
        &inner
    };
    extract(archive, member_index as usize).map_err(|error| error.to_string())
}

fn extract_path(bytes: &[u8], container_member: &str, member_path: &str, member_raw_hex: &str) -> Result<Vec<u8>, String> {
    let raw_name = decode_member_raw_hex(member_raw_hex)?;
    let inner;
    let archive = if container_member.is_empty() { bytes } else {
        let index = find_entry(bytes, container_member, None).map_err(|error| error.to_string())?;
        inner = nested_archive_bytes(bytes, index)?;
        &inner
    };
    let index = find_entry(archive, member_path, raw_name.as_deref()).map_err(|error| error.to_string())?;
    extract(archive, index).map_err(|error| error.to_string())
}

fn decode_member_raw_hex(member_raw_hex: &str) -> Result<Option<Vec<u8>>, String> {
    let raw_name = if member_raw_hex.is_empty() { None } else {
        if !member_raw_hex.len().is_multiple_of(2) || !member_raw_hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("member raw name must contain an even number of hexadecimal digits".to_string());
        }
        Some(member_raw_hex.as_bytes().chunks_exact(2).map(|pair| {
            let digit = |byte: u8| if byte <= b'9' { byte - b'0' } else { byte.to_ascii_lowercase() - b'a' + 10 };
            digit(pair[0]) * 16 + digit(pair[1])
        }).collect::<Vec<_>>())
    };
    Ok(raw_name)
}

struct CachedInspectionArchive {
    index: usize,
    bytes: Vec<u8>,
}

/// One immutable group snapshot, independent of the page's retained playback module.
/// Source ownership enters once; lookups borrow it and only one inner archive survives
/// between calls. Returned member bytes belong to the caller, never to this session.
struct ArchiveSession {
    source: Vec<u8>,
    inner: Option<CachedInspectionArchive>,
    #[cfg(test)]
    statistics: std::rc::Rc<RefCell<SessionStatistics>>,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct SessionStatistics {
    source_bytes: usize,
    inner_bytes: usize,
    source_capacity: usize,
    inner_capacity: usize,
    peak_owned_capacity: usize,
    peak_owned_bytes: usize,
    inner_extractions: usize,
}

impl ArchiveSession {
    fn new(bytes: Vec<u8>) -> Result<Self, String> {
        let source = starplayer_archive::prepare_owned_archive(bytes).map_err(|error| error.to_string())?;
        let session = Self {
            source, inner: None,
            #[cfg(test)]
            statistics: Default::default(),
        };
        #[cfg(test)]
        session.record_owned_bytes();
        Ok(session)
    }

    fn extract_member(&mut self, container_member: &str, member_path: &str, member_raw_hex: &str) -> Result<Vec<u8>, String> {
        let raw_name = decode_member_raw_hex(member_raw_hex)?;
        let archive = if container_member.is_empty() {
            self.clear_inner();
            &self.source
        } else {
            let index = match find_entry(&self.source, container_member, None) {
                Ok(index) => index,
                Err(error) => { self.clear_inner(); return Err(error.to_string()); },
            };
            if !self.inner.as_ref().is_some_and(|inner| inner.index == index) {
                // Release before decompression: switching must never retain two inners,
                // including when a replacement container is corrupt or too large.
                self.clear_inner();
                #[cfg(test)]
                { self.statistics.borrow_mut().inner_extractions += 1; }
                let bytes = nested_archive_bytes(&self.source, index)?;
                let bytes = starplayer_archive::prepare_owned_archive(bytes).map_err(|error| error.to_string())?;
                self.inner = Some(CachedInspectionArchive { index, bytes });
                #[cfg(test)]
                self.record_owned_bytes();
            }
            &self.inner.as_ref().ok_or_else(|| "no nested archive is cached".to_string())?.bytes
        };
        let index = find_entry(archive, member_path, raw_name.as_deref()).map_err(|error| error.to_string())?;
        extract(archive, index).map_err(|error| error.to_string())
    }

    fn clear_inner(&mut self) {
        self.inner = None;
        #[cfg(test)]
        self.record_owned_bytes();
    }

    #[cfg(test)]
    fn record_owned_bytes(&self) {
        let mut statistics = self.statistics.borrow_mut();
        statistics.source_bytes = self.source.len();
        statistics.inner_bytes = self.inner.as_ref().map_or(0, |inner| inner.bytes.len());
        statistics.source_capacity = self.source.capacity();
        statistics.inner_capacity = self.inner.as_ref().map_or(0, |inner| inner.bytes.capacity());
        statistics.peak_owned_capacity = statistics.peak_owned_capacity.max(statistics.source_capacity + statistics.inner_capacity);
        statistics.peak_owned_bytes = statistics.peak_owned_bytes.max(statistics.source_bytes + statistics.inner_bytes);
    }
}

#[cfg(test)]
impl Drop for ArchiveSession {
    fn drop(&mut self) {
        let mut statistics = self.statistics.borrow_mut();
        statistics.source_bytes = 0;
        statistics.inner_bytes = 0;
        statistics.source_capacity = 0;
        statistics.inner_capacity = 0;
    }
}

fn archive_module_records(bytes: &[u8]) -> Result<String, ArchiveError> {
    let entries = match list_modules(bytes) {
        Ok(entries) => entries,
        Err(ArchiveError::NoModules) => Vec::new(),
        Err(error) => return Err(error),
    };
    let mut records = String::new();
    for entry in entries.into_iter().filter(|entry| matches!(entry.format, ModuleFormat::S3m | ModuleFormat::Mod | ModuleFormat::Mtm | ModuleFormat::Xm | ModuleFormat::It)) {
        let safe_name = entry.name.replace(['\t', '\r', '\n'], " ");
        records.push_str(&format!("{}\t{}\t{}\n", entry.index, safe_name, entry.size));
    }
    Ok(records)
}

fn format_token(format: ModuleFormat) -> &'static str {
    match format { ModuleFormat::Mod => "mod", ModuleFormat::S3m => "s3m", ModuleFormat::Mtm => "mtm", ModuleFormat::Xm => "xm", ModuleFormat::It => "it" }
}

fn archive_catalog_json(bytes: &[u8]) -> Result<String, String> {
    let catalog = starplayer_archive::catalog(bytes).map_err(|error| error.to_string())?;
    let entries: Vec<_> = catalog.entries.into_iter().map(|entry| {
        let starplayer_archive::CatalogKind::Module(format) = entry.member.kind else { unreachable!("catalog contains only modules") };
        let raw_hex: String = entry.member.raw_name.iter().map(|byte| format!("{byte:02x}")).collect();
        serde_json::json!({
            "containerMember": entry.container_member, "memberPath": entry.member.name,
            "rawHex": raw_hex, "size": entry.member.size, "format": format_token(format),
            "playable": entry.member.error.is_empty(), "error": entry.member.error,
        })
    }).collect();
    let issues: Vec<_> = catalog.issues.into_iter().map(|issue| serde_json::json!({"path": issue.path, "reason": issue.reason})).collect();
    serde_json::to_string(&serde_json::json!({"entries": entries, "issues": issues})).map_err(|error| error.to_string())
}

fn module_metadata_json() -> Result<String, String> {
    MODULE.with(|cell| {
        let slot = cell.try_borrow().map_err(|_| String::from("the page-side loader is busy"))?;
        let module = slot.as_ref().ok_or_else(|| String::from("no module has been inspected"))?;
        let instrument_names = module.instruments().iter().map(|instrument| instrument.name.as_ref()).collect::<Vec<_>>().join("\n");
        let sample_names = module.samples().iter().map(|sample| sample.name()).collect::<Vec<_>>().join("\n");
        serde_json::to_string(&serde_json::json!({
            "format": format_token(module.header().format), "title": module.header().title.as_ref(),
            "channels": module.header().channel_count, "instrumentNames": instrument_names,
            "sampleNames": sample_names, "songMessage": "",
        })).map_err(|error| error.to_string())
    })
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
            ModuleFormat::Mtm => {
                let Some(view) = MtmPatternView::new(module, PatternId(pattern)) else { return output };
                let rows = row_count.min(view.rows().saturating_sub(first_row));
                output.reserve(rows as usize * view.channels() as usize * DISPLAY_CELL_BYTES);
                for row in first_row..first_row.saturating_add(rows) {
                    for channel in 0..view.channels() { append_display(&mut output, view.cell(row, channel).map(|cell| cell.display()).unwrap_or_default()); }
                }
            }
            ModuleFormat::Xm => {
                let Some(view) = XmPatternView::new(module, PatternId(pattern)) else { return output };
                let rows = row_count.min(view.rows().saturating_sub(first_row));
                output.reserve(rows as usize * view.channels() as usize * DISPLAY_CELL_BYTES);
                for row in first_row..first_row.saturating_add(rows) {
                    for channel in 0..view.channels() { append_display(&mut output, view.cell(row, channel).map(|cell| cell.display()).unwrap_or_default()); }
                }
            }
            ModuleFormat::It => {
                let Some(view) = ItPatternView::new(module, PatternId(pattern)) else { return output };
                let rows = row_count.min(view.rows().saturating_sub(first_row));
                output.reserve(rows as usize * view.channels() as usize * DISPLAY_CELL_BYTES);
                for row in first_row..first_row.saturating_add(rows) {
                    for channel in 0..view.channels() { append_display(&mut output, view.cell(row, channel).map(|cell| cell.display()).unwrap_or_default()); }
                }
            }
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

    /// Bounded archive snapshot for one metadata-inspection group. On healthy cleanup,
    /// call `free()` to drop the source and cached inner. After a WASM trap, retire the
    /// worker instead of reentering the poisoned instance to free the session.
    #[wasm_bindgen]
    pub struct ArchiveInspectionSession {
        session: ArchiveSession,
    }

    #[wasm_bindgen]
    impl ArchiveInspectionSession {
        /// Copy the JS source into one Rust-owned Vec, transferred into the session
        /// without another source copy. Size and ZIP validation precede retention.
        #[wasm_bindgen(constructor)]
        pub fn new(bytes: Vec<u8>) -> Result<ArchiveInspectionSession, JsValue> {
            ArchiveSession::new(bytes).map(|session| Self { session }).map_err(|error| JsValue::from_str(&error))
        }

        /// Resolve existing locator fields and return one copied JS Uint8Array.
        /// Ordinary archive errors throw JS values; WASM traps remain RuntimeError.
        pub fn extract_member(&mut self, container_member: &str, member_path: &str, member_raw_hex: &str) -> Result<Vec<u8>, JsValue> {
            self.session.extract_member(container_member, member_path, member_raw_hex).map_err(|error| JsValue::from_str(&error))
        }
    }

    /// Validate and retain a supported native module. The previous valid module survives
    /// an error. The legacy export name is retained for the existing page glue.
    #[wasm_bindgen]
    pub fn inspect_s3m(bytes: &[u8]) -> Result<(), JsValue> {
        inspect(bytes).map_err(|message| JsValue::from_str(&message))
    }

    /// Identify ZIP bytes before the page asks the archive decoder to inspect them.
    #[wasm_bindgen]
    pub fn is_archive(bytes: &[u8]) -> bool { is_zip(bytes) }

    /// List immediate inner ZIPs by central-directory index.
    #[wasm_bindgen]
    pub fn archive_containers(bytes: &[u8]) -> Result<Vec<u32>, JsValue> {
        starplayer_archive::list_zip_containers(bytes).map(|indices| indices.into_iter().map(|index| index as u32).collect()).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// List playable modules, one tab record per line.
    #[wasm_bindgen]
    pub fn archive_modules(bytes: &[u8]) -> Result<String, JsValue> {
        archive_module_records(bytes).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Catalog module locators and issues through one inner ZIP level as JSON.
    #[wasm_bindgen]
    pub fn archive_catalog(bytes: &[u8]) -> Result<String, JsValue> {
        archive_catalog_json(bytes).map_err(|error| JsValue::from_str(&error))
    }

    /// Metadata for the retained inspected module, without parsing it again.
    #[wasm_bindgen]
    pub fn module_metadata() -> Result<String, JsValue> {
        module_metadata_json().map_err(|error| JsValue::from_str(&error))
    }

    /// Extract one central-directory entry into page-owned module bytes.
    #[wasm_bindgen]
    pub fn archive_extract(bytes: &[u8], index: u32) -> Result<Vec<u8>, JsValue> {
        extract(bytes, index as usize).map_err(|error| JsValue::from_str(&error.to_string()))
    }

    /// Resolve a CSV locator, optionally through one nested ZIP.
    #[wasm_bindgen]
    pub fn archive_extract_path(bytes: &[u8], container_member: &str, member_path: &str, member_raw_hex: &str) -> Result<Vec<u8>, JsValue> {
        extract_path(bytes, container_member, member_path, member_raw_hex).map_err(|error| JsValue::from_str(&error))
    }

    /// Extract an upload member by central-directory indices.
    #[wasm_bindgen]
    pub fn archive_extract_nested(bytes: &[u8], container_index: i32, member_index: u32) -> Result<Vec<u8>, JsValue> {
        extract_nested(bytes, container_index, member_index).map_err(|error| JsValue::from_str(&error))
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

    /// Whether the retained display module is a native MOD. Page preferences use this
    /// to avoid reloading S3M and MTM when the MOD-only panning checkbox changes.
    #[wasm_bindgen]
    pub fn module_is_mod() -> bool { with_module(false, |module| module.header().format == ModuleFormat::Mod) }

    #[wasm_bindgen]
    pub fn instrument_name(index: u32) -> String {
        with_module(String::new(), |module| {
            module.instruments().get(index as usize).map(|instrument| instrument.name.to_string()).unwrap_or_default()
        })
    }

    /// Pure metadata for any playable map key and independently for reference note60.
    #[wasm_bindgen]
    pub fn instrument_capability(index: u32) -> u32 {
        #[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
        { with_module(0, |module| {
            if index > u16::MAX as u32 { return 0; }
            let capability = starplayer::live_instrument_capability(module, starplayer::core::InstrumentId(index as u16));
            u32::from(capability.playable) | (u32::from(capability.one_shot) << 1) | (u32::from(capability.looping) << 2) | (u32::from(capability.preview_playable) << 3) | (u32::from(capability.preview_looping) << 4)
        }) }
        #[cfg(not(any(feature = "note-overlap", feature = "interactive-audition")))]
        { let _ = index; 0 }
    }

    /// Snapshot all key flags while this inspection module is still current.
    #[wasm_bindgen]
    pub fn instrument_note_capabilities(index: u32) -> Vec<u8> {
        #[cfg(any(feature = "note-overlap", feature = "interactive-audition"))]
        { with_module(Vec::new(), |module| {
            if index > u16::MAX as u32 { return Vec::new(); }
            (0..128).map(|note| {
                starplayer::live_instrument_note_flags(module, starplayer::core::InstrumentId(index as u16), note)
            }).collect()
        }) }
        #[cfg(not(any(feature = "note-overlap", feature = "interactive-audition")))]
        { let _ = index; Vec::new() }
    }

    #[wasm_bindgen]
    pub fn instrument_sample_length(index: u32) -> u32 {
        with_module(0, |module| instrument_source_frames(module, index as usize))
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

    fn minimal_mtm() -> Vec<u8> {
        const SAMPLE_HEADER: usize = 66;
        const ORDER_TABLE: usize = SAMPLE_HEADER + 37;
        const TRACK_DATA: usize = ORDER_TABLE + 128;
        const PATTERN_TABLE: usize = TRACK_DATA + 192;
        const SAMPLE_DATA: usize = PATTERN_TABLE + 64;
        let mut bytes = vec![0; SAMPLE_DATA + 8];
        bytes[..4].copy_from_slice(b"MTM\x10");
        bytes[4..14].copy_from_slice(b"native mtm");
        bytes[24..26].copy_from_slice(&1u16.to_le_bytes());
        bytes[30] = 1;
        bytes[32] = 64;
        bytes[33] = 1;
        bytes[34] = 8;
        bytes[SAMPLE_HEADER..SAMPLE_HEADER + 6].copy_from_slice(b"sample");
        bytes[SAMPLE_HEADER + 22..SAMPLE_HEADER + 26].copy_from_slice(&8u32.to_le_bytes());
        bytes[SAMPLE_HEADER + 35] = 64;
        bytes[TRACK_DATA..TRACK_DATA + 3].copy_from_slice(&starplayer::mtm::MtmCell { pitch: 12, instrument: 1, effect: 0xF, param: 6 }.to_bytes());
        bytes[PATTERN_TABLE..PATTERN_TABLE + 2].copy_from_slice(&1u16.to_le_bytes());
        bytes
    }

    fn source_frame_module(format: ModuleFormat, instrument_mode: bool) -> Module {
        use starplayer::{core::{SampleId, U0F16}, model::{InstrumentDef, ModuleBuilder, ModuleHeader, SampleSpec}};
        let mut builder = ModuleBuilder::new();
        builder.add_sample(&[1, 2, 3], SampleSpec::one_shot("first")).unwrap();
        builder.add_sample(&[1, 2, 3], SampleSpec::one_shot("same content, distinct ID")).unwrap();
        builder.add_sample(&[0; 7], SampleSpec::one_shot("unmapped")).unwrap();
        builder.add_sample(&[], SampleSpec::one_shot("empty")).unwrap();
        let mut mapped = InstrumentDef::from_sample("mapped", SampleId(2), U0F16::MAX);
        mapped.note_sample_map[..5].copy_from_slice(&[1, 1, 2, 4, 2]);
        builder.add_instrument(mapped.clone()).unwrap();
        mapped.sample = None;
        builder.add_instrument(mapped).unwrap();
        let empty_map = InstrumentDef::from_sample("empty map with direct sample", SampleId(0), U0F16::MAX);
        builder.add_instrument(empty_map).unwrap();
        let mut shared = InstrumentDef::from_sample("shared ID", SampleId(0), U0F16::MAX);
        shared.note_sample_map.fill(2);
        builder.add_instrument(shared).unwrap();
        let mut header = ModuleHeader::new(format, 1);
        if format == ModuleFormat::It && instrument_mode {
            header.format_extra = starplayer::it::ItFormatExtra { flags: 4, special: 0, old_instruments: false, has_midi_configuration: false }.encode();
        }
        builder.set_header(header);
        builder.build().unwrap()
    }

    #[test]
    fn source_frame_totals_use_only_distinct_native_mapped_ids() {
        for format in [ModuleFormat::Xm, ModuleFormat::It] {
            let module = source_frame_module(format, true);
            assert_eq!(instrument_source_frames(&module, 0), 6, "deduplicate IDs, not contents; exclude unmapped direct samples and guards");
            assert_eq!(instrument_source_frames(&module, 1), 6, "native maps need no direct reference");
            assert_eq!(instrument_source_frames(&module, 2), 0, "an empty native map never falls back to a direct sample");
            assert_eq!(instrument_source_frames(&module, 3), 3, "a shared sample is counted independently for each instrument");
            assert_eq!(instrument_source_frames(&module, usize::MAX), 0);
        }
    }

    #[test]
    fn direct_source_frames_preserve_legacy_and_it_sample_mode_lengths() {
        for format in [ModuleFormat::Mod, ModuleFormat::S3m, ModuleFormat::Mtm, ModuleFormat::It] {
            let module = source_frame_module(format, false);
            assert_eq!(instrument_source_frames(&module, 0), 7, "direct references ignore native maps");
            assert_eq!(instrument_source_frames(&module, 1), 0);
            assert_eq!(instrument_source_frames(&module, 2), 3);
            assert_eq!(instrument_source_frames(&module, usize::MAX), 0);
        }
        assert_eq!(exports::instrument_sample_length(u32::MAX), 0);
    }

    #[test]
    fn page_loader_exposes_metadata_and_pattern_cells() {
        assert!(inspect(FIXTURE).is_ok());
        assert!(!exports::module_is_mod());
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
        assert!(exports::module_is_mod());
        assert_eq!(pattern_window_bytes(0, 0, 1).get(..5), Some(&[48, 1, VALUE_NONE, 0xF, 6][..]));
        assert!(effect_name_records().contains("15:-1:set speed/tempo\n"));
    }

    #[test]
    fn page_loader_and_pattern_view_dispatch_to_native_mtm() {
        assert!(inspect(&minimal_mtm()).is_ok());
        assert_eq!(with_module(ModuleFormat::S3m, |module| module.header().format), ModuleFormat::Mtm);
        assert!(!exports::module_is_mod());
        assert_eq!(pattern_window_bytes(0, 0, 1).get(..5), Some(&[36, 1, VALUE_NONE, 0xF, 6][..]));
        assert!(effect_name_records().contains("15:-1:set speed/tempo\n"));
    }

    #[test]
    fn browser_refreshes_format_specific_effect_names_before_rendering_metadata() {
        let app = include_str!("../www/app.js");
        let activate = app.split_once("async function activateLoadedModule").expect("activateLoadedModule").1;
        let inspect = activate.find("Loader.inspect_s3m(new Uint8Array(moduleBuffer));").expect("inspect call");
        let refresh = activate.find("loadEffectNames();").expect("effect-name refresh");
        let metadata = activate.find("const metadata = readMetadata(moduleLabel);").expect("metadata render");
        assert!(inspect < refresh && refresh < metadata, "the newly inspected format table must be loaded before page rendering");
    }

    #[test]
    fn browser_enriches_only_the_committed_playback_with_a_synchronous_metadata_snapshot() {
        let app = include_str!("../www/app.js");
        let activate = app.split_once("async function activateLoadedModule").unwrap().1.split_once("async function playArchiveEntry").unwrap().0;
        let inspect = activate.find("Loader.inspect_s3m(new Uint8Array(moduleBuffer));").unwrap();
        let snapshot = activate.find("JSON.parse(Loader.module_metadata())").expect("structured snapshot parsed immediately");
        let audio = activate.find("await context.audio;").unwrap();
        let guard = activate.find("if (!context.isCurrent() || revision !== state.moduleRevision || node !== state.node)").expect("current installation guard");
        let commit = activate.find("libraryView.setPlaying(null);").expect("committed playback state");
        let enrich = activate.find("acceptPlaybackMetadata(context.locator, libraryMetadata, retainedBytes)").unwrap();
        assert!(inspect < snapshot && snapshot < audio && audio < guard && guard < commit && commit < enrich);
        assert!(activate[guard..commit].contains("return null;"), "cancelled/stale installations return before metadata enrichment");
        assert!(activate.contains("catch {}"), "optional metadata failures preserve playback");
        assert!(activate.contains("library === state.library"), "same identity in a newer source context must not receive old metadata");
        assert!(activate.contains("context.locator.libraryIdentity?.sha256"));
        assert!(app.contains("captureContext: locator => ({ libraryContext: locator.kind === 'library' ? state.library : null })"));
        assert!(app.contains("available: playlistControls.canPlay"), "discovery status text is not a playback gate");
        assert!(app.contains("onLibraryUpdate: (library, change) => { libraryView.updateLibrary(library, change); playlistControls.libraryUpdated(library, change); }"));
        assert!(app.contains("onComments: () => libraryView.refreshComments()"), "indexed comments retain their existing refresh path");
        assert!(app.contains("reconnect its library."));
        assert!(!app.contains("reconnect its original index."));
    }

    #[test]
    fn browser_visible_loader_copy_accepts_and_advertises_every_playable_format() {
        let index = include_str!("../www/index.html");
        assert!(index.contains("S3M, MOD, MTM, XM and IT modules"));
        assert!(index.contains("drop an S3M, MOD, MTM, XM, IT, or ZIP"));
        assert!(index.contains("Choose a module"));
        assert!(index.contains("Load an S3M, MOD, MTM, XM, IT, or ZIP"));
        assert!(index.contains("accept=\".s3m,.mod,.mtm,.xm,.it,.zip,audio/s3m,audio/mod,audio/x-mod,audio/mtm,audio/xm,audio/it,application/zip\""));
        assert!(index.contains("aria-label=\"S3M, MOD, MTM, XM, IT, or ZIP URL\""));
        assert!(index.contains("Drop .s3m, .mod, .mtm, .xm, .it, or .zip here"));
        assert!(index.contains("id=\"archive-tracks\""));
        assert!(index.contains("aria-label=\"Track from the last ZIP\""));
        assert!(index.contains("id=\"load-archive-track\""));
        assert!(index.contains(">Load from ZIP</button>"));
        assert!(index.contains("id=\"mod-headphone-panning\""));
        assert!(index.contains("Headphone-friendly MOD panning"));
        assert!(index.contains("S3M-style 60% stereo spacing"));

        let app = include_str!("../www/app.js");
        assert!(app.contains("No supported modules were found inside"));
        assert!(app.contains("Choose one of ${entries.length} modules in ${archiveLabel}"));
        assert!(!app.contains("No S3M modules were found inside"));
        assert!(!app.contains("S3M modules in ${archiveLabel}"));
        assert!(app.contains("saved.headphoneFriendlyModPanning === true"), "missing fields in older v1 objects must restore unchecked");
        assert!(app.contains("headphoneFriendlyModPanning: elements.modHeadphonePanning.checked"));
        assert!(app.contains("isMod: Loader.module_is_mod()"));
        assert!(app.contains("Track from ${archive.label}…"), "the dropdown placeholder names the retained archive");
    }

    #[test]
    fn browser_visible_loader_copy_advertises_the_progress_slider_and_repeat() {
        let index = include_str!("../www/index.html");
        assert!(index.contains("id=\"elapsed\""));
        assert!(index.contains("id=\"progress\""));
        assert!(index.contains("id=\"duration\""));
        assert!(index.contains("id=\"repeat\""));
        assert!(index.contains(" Repeat</label>"));
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
        writer.start_file("native.mtm", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"mtm").unwrap();
        writer.start_file("readme.txt", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"notes").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert_eq!(archive_module_records(&bytes).unwrap(), "0\tREFLEX.S3M\t3\n1\tnative.mod\t3\n2\tnative.mtm\t3\n");
    }

    fn test_archive(name: &str, content: &[u8]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file(name, SimpleFileOptions::default()).unwrap();
        writer.write_all(content).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn test_archive_members(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in entries {
            writer.start_file(*name, SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn replace_test_name(bytes: &mut [u8], old: &[u8], replacement: &[u8]) {
        assert_eq!(old.len(), replacement.len());
        let positions: Vec<_> = bytes.windows(old.len()).enumerate().filter_map(|(index, window)| (window == old).then_some(index)).collect();
        for position in positions { bytes[position..position + old.len()].copy_from_slice(replacement); }
    }

    fn test_zip64_footer(mut bytes: Vec<u8>) -> Vec<u8> {
        let end = bytes.len() - 22;
        let mut footer = bytes[end..].to_vec();
        let count = u16::from_le_bytes(footer[10..12].try_into().unwrap()) as u64;
        let size = u32::from_le_bytes(footer[12..16].try_into().unwrap()) as u64;
        let offset = u32::from_le_bytes(footer[16..20].try_into().unwrap()) as u64;
        bytes.truncate(end);
        bytes.extend_from_slice(b"PK\x06\x06");
        bytes.extend_from_slice(&44u64.to_le_bytes());
        bytes.extend_from_slice(&[45, 0, 45, 0]);
        bytes.extend_from_slice(&[0; 8]);
        for value in [count, count, size, offset] { bytes.extend_from_slice(&value.to_le_bytes()); }
        bytes.extend_from_slice(b"PK\x06\x07");
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&(end as u64).to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        footer[8..20].fill(255);
        bytes.extend_from_slice(&footer);
        bytes
    }

    #[test]
    fn archive_session_reuses_one_inner_replaces_it_and_releases_all_owned_bytes() {
        inspect(FIXTURE).unwrap();
        let metadata = module_metadata_json().unwrap();
        let first = test_archive_members(&[("first.mod", b"first"), ("second.it", b"second")]);
        let second = test_archive_members(&[("third.xm", b"third")]);
        let bytes = test_archive_members(&[("one.zip", &first), ("two.zip", &second), ("direct.s3m", b"direct")]);
        let source_pointer = bytes.as_ptr();
        let source_length = bytes.len();
        let mut session = ArchiveSession::new(bytes).unwrap();
        let statistics = session.statistics.clone();
        assert_eq!(session.source.as_ptr(), source_pointer, "constructor transfers the original allocation");
        let mut member = session.extract_member("one.zip", "first.mod", "").unwrap();
        member[0] = b'X';
        assert_eq!(session.extract_member("one.zip", "second.it", "").unwrap(), b"second");
        assert_eq!(session.extract_member("ONE.ZIP", "first.mod", "").unwrap(), b"first");
        assert_eq!(statistics.borrow().inner_extractions, 1, "aliases resolving to the same inner reuse it");
        assert_eq!(statistics.borrow().inner_bytes, first.len());
        assert!(session.extract_member("one.zip", "missing.mod", "").is_err());
        assert_eq!(statistics.borrow().inner_extractions, 1);
        assert_eq!(session.extract_member("two.zip", "third.xm", "").unwrap(), b"third");
        assert_eq!(statistics.borrow().inner_extractions, 2);
        assert_eq!(statistics.borrow().inner_bytes, second.len());
        assert_eq!(statistics.borrow().peak_owned_bytes, source_length + first.len().max(second.len()));
        assert_eq!(session.extract_member("", "direct.s3m", "").unwrap(), b"direct");
        assert_eq!(statistics.borrow().inner_bytes, 0);
        assert_eq!(session.extract_member("one.zip", "first.mod", "").unwrap(), b"first");
        assert_eq!(statistics.borrow().inner_extractions, 3, "direct extraction invalidates the inner cache");
        assert_eq!(session.source.as_ptr(), source_pointer, "extracts never replace or clone source ownership");
        assert_eq!(statistics.borrow().source_bytes, source_length);
        assert_eq!(module_metadata_json().unwrap(), metadata, "session extraction never changes playback inspection");
        eprintln!("synthetic group ownership/cache evidence: {:?}", statistics.borrow());
        drop(session); // wasm-bindgen's generated .free() drops this owned session.
        assert_eq!(statistics.borrow().source_bytes, 0);
        assert_eq!(statistics.borrow().inner_bytes, 0);
        assert_eq!(statistics.borrow().source_capacity, 0);
        assert_eq!(statistics.borrow().inner_capacity, 0);
    }

    #[test]
    fn archive_session_isolates_bad_members_and_invalidates_failed_container_switches() {
        let mut inner = test_archive_members(&[("good.mod", b"good"), ("large.mod", b"small"), ("crypt.mod", b"secret"), ("broken.it", b"broken")]);
        let central: Vec<_> = inner.windows(4).enumerate().filter_map(|(index, bytes)| (bytes == b"PK\x01\x02").then_some(index)).collect();
        inner[central[1] + 24..central[1] + 28].copy_from_slice(&((starplayer_archive::MAX_ENTRY_BYTES + 1) as u32).to_le_bytes());
        inner[central[2] + 8] |= 1;
        inner[central[3] + 16..central[3] + 20].copy_from_slice(&0u32.to_le_bytes()); // Wrong CRC is discovered while reading.
        let bytes = test_archive_members(&[("inner.zip", &inner), ("bad.zip", b"not a zip"), ("direct.mod", b"direct")]);
        let mut session = ArchiveSession::new(bytes).unwrap();
        for (member, reason) in [("large.mod", "too large"), ("crypt.mod", "encrypted"), ("broken.it", "corrupt")] {
            assert!(session.extract_member("inner.zip", member, "").unwrap_err().contains(reason));
            assert_eq!(session.extract_member("inner.zip", "good.mod", "").unwrap(), b"good");
        }
        assert_eq!(session.statistics.borrow().inner_extractions, 1);
        for invalid in ["a", "zz", "é"] { assert!(session.extract_member("inner.zip", "good.mod", invalid).unwrap_err().contains("hexadecimal")); }
        assert!(session.extract_member("bad.zip", "good.mod", "").unwrap_err().contains("not a ZIP"));
        assert_eq!(session.statistics.borrow().inner_bytes, 0);
        assert_eq!(session.extract_member("inner.zip", "good.mod", "").unwrap(), b"good");
        assert!(session.extract_member("missing.zip", "good.mod", "").is_err());
        assert_eq!(session.statistics.borrow().inner_bytes, 0);
        assert_eq!(session.extract_member("", "direct.mod", "").unwrap(), b"direct");
    }

    #[test]
    fn archive_session_enforces_source_and_container_caps_before_retention() {
        assert!(ArchiveSession::new(vec![0; starplayer_archive::MAX_SOURCE_BYTES as usize + 1]).err().unwrap().contains("source is too large"));
        assert!(ArchiveSession::new(b"not a zip".to_vec()).err().unwrap().contains("not a ZIP"));
        assert!(ArchiveSession::new(b"PK\x03\x04broken".to_vec()).is_err());
        let empty = ZipWriter::new(Cursor::new(Vec::new())).finish().unwrap().into_inner();
        let mut session = ArchiveSession::new(empty).unwrap();
        assert!(session.extract_member("", "missing.mod", "").is_err());
        assert_eq!(session.statistics.borrow().inner_extractions, 0);
        let mut bytes = test_archive("large.zip", b"small");
        let central = bytes.windows(4).position(|window| window == b"PK\x01\x02").unwrap();
        bytes[central + 24..central + 28].copy_from_slice(&((starplayer_archive::MAX_ENTRY_BYTES + 1) as u32).to_le_bytes());
        let mut session = ArchiveSession::new(bytes).unwrap();
        assert!(session.extract_member("large.zip", "x.mod", "").unwrap_err().contains("too large"));
        assert_eq!(session.statistics.borrow().inner_bytes, 0);
        assert!(session.statistics.borrow().peak_owned_bytes <= starplayer_archive::MAX_SOURCE_BYTES as usize);
    }

    #[test]
    fn archive_session_accepts_exact_source_cap_without_another_owned_source_copy() {
        let mut bytes = test_archive("small.mod", b"small");
        let central = bytes.windows(4).position(|window| window == b"PK\x01\x02").unwrap();
        let mut directory = bytes.split_off(central);
        let source_limit = starplayer_archive::MAX_SOURCE_BYTES as usize;
        let new_central = source_limit - directory.len();
        let footer = directory.len() - 22;
        directory[footer + 16..footer + 20].copy_from_slice(&(new_central as u32).to_le_bytes());
        bytes.reserve_exact(source_limit - bytes.len());
        bytes.resize(new_central, 0);
        bytes.extend_from_slice(&directory);
        assert_eq!(bytes.len(), source_limit);
        let original_pointer = bytes.as_ptr();
        let original_capacity = bytes.capacity();
        let mut session = ArchiveSession::new(bytes).unwrap();
        assert_eq!(session.source.as_ptr(), original_pointer);
        assert_eq!(session.statistics.borrow().source_bytes, source_limit);
        assert_eq!(session.statistics.borrow().source_capacity, original_capacity);
        assert_eq!(session.extract_member("", "small.mod", "").unwrap(), b"small");
        assert_eq!(session.statistics.borrow().peak_owned_bytes, source_limit);
        assert_eq!(session.statistics.borrow().peak_owned_capacity, original_capacity);
        eprintln!("exact source-cap evidence: {:?}", session.statistics.borrow());
    }

    #[test]
    fn archive_session_bounds_misdeclared_deflate_output_while_reading() {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer.start_file("bomb.mod", SimpleFileOptions::default().compression_method(CompressionMethod::Deflated)).unwrap();
        let chunk = [0; 8192];
        for _ in 0..starplayer_archive::MAX_ENTRY_BYTES / chunk.len() as u64 { writer.write_all(&chunk).unwrap(); }
        writer.write_all(b"x").unwrap();
        writer.start_file("good.it", SimpleFileOptions::default()).unwrap();
        writer.write_all(b"good").unwrap();
        let mut bytes = writer.finish().unwrap().into_inner();
        let central = bytes.windows(4).position(|window| window == b"PK\x01\x02").unwrap();
        bytes[central + 24..central + 28].copy_from_slice(&1u32.to_le_bytes());
        let mut session = ArchiveSession::new(bytes).unwrap();
        let error = session.extract_member("", "bomb.mod", "").unwrap_err();
        assert!(error.contains("larger than its declared uncompressed size"), "{error}");
        assert_eq!(session.extract_member("", "good.it", "").unwrap(), b"good");
        assert_eq!(session.statistics.borrow().inner_bytes, 0);
    }

    #[test]
    fn archive_session_preserves_raw_encoded_exact_and_ambiguous_name_precedence() {
        let bytes = test_archive_members(&[("folder/Same.mod", b"upper"), ("folder/same.mod", b"lower"), ("other.mod", b"raw")]);
        let mut session = ArchiveSession::new(bytes).unwrap();
        assert_eq!(session.extract_member("", "folder/Same.mod", "").unwrap(), b"upper");
        assert_eq!(session.extract_member("", "folder/same.mod", "").unwrap(), b"lower");
        assert_eq!(session.extract_member("", "folder/Same.mod", "6F746865722E6D6F64").unwrap(), b"raw");
        assert!(session.extract_member("", "FOLDER/SAME.MOD", "").unwrap_err().contains("ambiguous"));
        let mut inner = test_archive("xx.mod", b"cp437");
        patch_cp437_name(&mut inner);
        let mut outer = test_archive("xx.zip", &inner);
        patch_cp437_name(&mut outer);
        let mut session = ArchiveSession::new(outer).unwrap();
        assert_eq!(session.extract_member("├⌐.zip", "├⌐.mod", "").unwrap(), b"cp437");
        assert_eq!(session.extract_member("├⌐.zip", "display.mod", "c3a92e6d6f64").unwrap(), b"cp437");
        assert_eq!(session.statistics.borrow().inner_extractions, 1);
    }

    #[test]
    fn archive_session_and_legacy_upload_keep_first_raw_duplicate_in_classic_and_zip64() {
        let mut inner = test_archive_members(&[("same.it", b"first inner"), ("last.it", b"last inner")]);
        replace_test_name(&mut inner, b"last.it", b"same.it");
        let last_inner = test_archive("last.mod", b"last container");
        let mut outer = test_archive_members(&[("same.mod", b"first"), ("last.mod", b"last"), ("inner.zip", &inner), ("other.zip", &last_inner)]);
        replace_test_name(&mut outer, b"last.mod", b"same.mod");
        replace_test_name(&mut outer, b"other.zip", b"inner.zip");
        // Intentional existing compatibility: catalog and uploaded ZIP listing/index
        // extraction select the same first raw-name entry as the new session API.
        for bytes in [outer.clone(), test_zip64_footer(outer)] {
            assert_eq!(archive_module_records(&bytes).unwrap(), "0\tsame.mod\t5\n");
            assert_eq!(extract_nested(&bytes, -1, 0).unwrap(), b"first");
            assert_eq!(extract_nested(&bytes, 1, 0).unwrap(), b"first inner");
            assert_eq!(extract_path(&bytes, "inner.zip", "same.it", "").unwrap(), b"first inner");
            let pointer = bytes.as_ptr();
            let capacity = bytes.capacity();
            let mut session = ArchiveSession::new(bytes).unwrap();
            assert_eq!(session.source.as_ptr(), pointer, "duplicate compaction preserves source allocation");
            assert_eq!(session.source.capacity(), capacity);
            assert_eq!(session.extract_member("", "same.mod", "73616d652e6d6f64").unwrap(), b"first");
            assert_eq!(session.extract_member("inner.zip", "same.it", "").unwrap(), b"first inner");
            assert_eq!(session.extract_member("inner.zip", "same.it", "73616d652e6974").unwrap(), b"first inner");
            assert_eq!(session.statistics.borrow().inner_extractions, 1);
        }
    }

    fn patch_cp437_name(bytes: &mut [u8]) {
        for index in 0..bytes.len().saturating_sub(4) {
            let (flags, offset) = match &bytes[index..index + 4] {
                b"PK\x03\x04" => (6, 30),
                b"PK\x01\x02" => (8, 46),
                _ => continue,
            };
            bytes[index + flags + 1] &= !8;
            bytes[index + offset..index + offset + 2].copy_from_slice(&[0xC3, 0xA9]);
        }
    }

    #[test]
    fn archive_locators_resolve_plain_nested_and_cp437_names() {
        let mut inner = test_archive("xx.mod", b"module");
        patch_cp437_name(&mut inner);
        let mut outer = test_archive("xx.zip", &inner);
        patch_cp437_name(&mut outer);
        assert_eq!(extract_path(&inner, "", "├⌐.mod", "").unwrap(), b"module");
        assert_eq!(extract_path(&inner, "", "display.mod", "c3a92e6d6f64").unwrap(), b"module");
        assert_eq!(extract_path(&outer, "├⌐.zip", "├⌐.mod", "C3A92E6D6F64").unwrap(), b"module");
        assert_eq!(extract_nested(&outer, 0, 0).unwrap(), b"module");
        assert_eq!(extract_nested(&inner, -1, 0).unwrap(), b"module");
        assert_eq!(extract_nested(&inner, -8, 0).unwrap(), b"module");
        assert!(extract_nested(&inner, -1, 1).is_err());
        assert!(extract_path(&inner, "", "missing", "").is_err());
        for invalid in ["a", "zz", "é"] {
            assert!(extract_path(&inner, "", "├⌐.mod", invalid).unwrap_err().contains("hexadecimal"));
        }
    }

    #[test]
    fn archive_locators_reject_non_zip_and_oversized_containers() {
        let mut bytes = test_archive("inner.zip", b"not a ZIP");
        assert!(extract_path(&bytes, "inner.zip", "x.mod", "").unwrap_err().contains("not a ZIP"));
        assert!(extract_nested(&bytes, 0, 0).unwrap_err().contains("not a ZIP"));
        let central = bytes.windows(4).position(|window| window == b"PK\x01\x02").unwrap();
        bytes[central + 24..central + 28].copy_from_slice(&((starplayer_archive::MAX_ENTRY_BYTES + 1) as u32).to_le_bytes());
        assert!(extract_path(&bytes, "inner.zip", "x.mod", "").unwrap_err().contains("too large"));
        assert!(extract_nested(&bytes, 0, 0).unwrap_err().contains("too large"));
    }

    fn minimal_xm_metadata() -> Vec<u8> {
        let mut bytes = vec![0; 336];
        bytes[..17].copy_from_slice(b"Extended Module: ");
        bytes[17..26].copy_from_slice(b"native xm");
        bytes[37] = 0x1A;
        bytes[38..58].copy_from_slice(b"FastTracker v2.00   ");
        for (offset, value) in [(58, 0x0104u16), (64, 1), (68, 4), (70, 1), (72, 2), (76, 6), (78, 125)] {
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        bytes[60..64].copy_from_slice(&276u32.to_le_bytes());
        bytes.extend_from_slice(&[9, 0, 0, 0, 0, 64, 0, 0, 0]);
        let mut empty_instrument = vec![0; 33];
        empty_instrument[..4].copy_from_slice(&33u32.to_le_bytes());
        bytes.extend_from_slice(&empty_instrument);
        let mut instrument = vec![0; 263];
        instrument[..4].copy_from_slice(&263u32.to_le_bytes());
        instrument[4..9].copy_from_slice(b"lead\"");
        instrument[27..29].copy_from_slice(&2u16.to_le_bytes());
        instrument[29..33].copy_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&instrument);
        bytes.extend_from_slice(&[0; 40]);
        let mut sample = [0; 40];
        sample[18..25].copy_from_slice(b"sample\"");
        bytes.extend_from_slice(&sample);
        bytes
    }

    fn minimal_it_metadata() -> Vec<u8> {
        let mut bytes = vec![0; 192 + 1 + 4 * 6];
        bytes[..4].copy_from_slice(b"IMPM");
        bytes[4..13].copy_from_slice(b"native it");
        for (offset, value) in [(32, 1u16), (34, 3), (36, 3), (40, 0x0214), (42, 0x0214), (44, 5)] {
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        bytes[48..52].copy_from_slice(&[128, 48, 6, 125]);
        bytes[192] = 255;
        let instrument_offset = bytes.len() as u32;
        bytes[197..201].copy_from_slice(&instrument_offset.to_le_bytes());
        let mut instrument = vec![0; 554];
        instrument[..4].copy_from_slice(b"IMPI");
        instrument[24] = 128;
        instrument[32..37].copy_from_slice(b"lead\"");
        bytes.extend_from_slice(&instrument);
        let sample_offset = bytes.len() as u32;
        bytes[209..213].copy_from_slice(&sample_offset.to_le_bytes());
        let mut sample = vec![0; 80];
        sample[..4].copy_from_slice(b"IMPS");
        sample[17] = 64;
        sample[19] = 48;
        sample[20..27].copy_from_slice(b"sample\"");
        sample[60..64].copy_from_slice(&22050u32.to_le_bytes());
        bytes.extend_from_slice(&sample);
        bytes
    }

    #[test]
    fn structured_module_metadata_requires_an_inspected_module_and_reports_busy_state() {
        MODULE.with(|cell| *cell.borrow_mut() = None);
        assert!(module_metadata_json().unwrap_err().contains("no module"));
        MODULE.with(|cell| {
            let _borrow = cell.borrow_mut();
            assert!(module_metadata_json().unwrap_err().contains("busy"));
        });
    }

    #[test]
    fn structured_module_metadata_reports_all_native_formats_and_preserves_empty_slots() {
        let mut mod_bytes = minimal_mod();
        mod_bytes[20..25].copy_from_slice(b"lead\"");
        let mod_names = format!("lead\"{}", "\n".repeat(30));
        let cases = [
            (mod_bytes, "mod", "native mod", 4, mod_names.as_str(), mod_names.as_str()),
            (minimal_mtm(), "mtm", "native mtm", 1, "sample", "sample"),
            (minimal_xm_metadata(), "xm", "native xm", 4, "\nlead\"", "\nsample\""),
            (minimal_it_metadata(), "it", "native it", 1, "\nlead\"\n", "\nsample\"\n"),
        ];
        for (bytes, format, title, channels, instrument_names, sample_names) in cases {
            inspect(&bytes).unwrap();
            let metadata: serde_json::Value = serde_json::from_str(&module_metadata_json().unwrap()).unwrap();
            assert_eq!(metadata, serde_json::json!({"format": format, "title": title, "channels": channels, "instrumentNames": instrument_names, "sampleNames": sample_names, "songMessage": ""}));
        }
        inspect(FIXTURE).unwrap();
        let metadata: serde_json::Value = serde_json::from_str(&module_metadata_json().unwrap()).unwrap();
        assert_eq!(metadata["format"], "s3m");
        assert_eq!(metadata["title"], "Reflex");
        assert_eq!(metadata["channels"], 3);
        assert!(metadata["instrumentNames"].as_str().unwrap().ends_with("  jedi@tartarus.uwa.edu.au"));
        assert!(metadata["sampleNames"].as_str().unwrap().starts_with("This module started almost\n"));
        assert_eq!(metadata["songMessage"], "");
        let before = module_metadata_json().unwrap();
        assert!(inspect(b"invalid").is_err());
        assert_eq!(module_metadata_json().unwrap(), before);
    }

    #[test]
    fn archive_catalog_json_preserves_exact_names_and_legacy_raw_bytes() {
        let mut inner = test_archive("xx.mod", b"module");
        patch_cp437_name(&mut inner);
        let outer = test_archive("quotes\"\n.zip", &inner);
        let json = archive_catalog_json(&outer).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, serde_json::json!({"entries": [{"containerMember": "quotes\"\n.zip", "memberPath": "é.mod", "rawHex": "c3a92e6d6f64", "size": 6, "format": "mod", "playable": true, "error": ""}], "issues": []}));
        assert_eq!(extract_path(&outer, "quotes\"\n.zip", "├⌐.mod", "c3a92e6d6f64").unwrap(), b"module");
        let direct = test_archive("quotes\"\n.it", b"abc");
        let parsed: serde_json::Value = serde_json::from_str(&archive_catalog_json(&direct).unwrap()).unwrap();
        assert_eq!(parsed["entries"][0]["memberPath"], "quotes\"\n.it");
        assert_eq!(parsed["entries"][0]["format"], "it");
        assert_eq!(parsed["entries"][0]["containerMember"], "");
        assert!(archive_catalog_json(b"not a zip").unwrap_err().contains("not a ZIP"));
    }

}
