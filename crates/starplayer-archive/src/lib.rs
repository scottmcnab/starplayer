//! Reusable module archive discovery and extraction.
//!
//! The archive layer is deliberately format-agnostic and extension-driven. Format
//! loaders remain responsible for validating the extracted bytes.

#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt::{self, Display, Formatter};
use std::io::{Cursor, Read};

use starplayer_model::ModuleFormat;
use zip::read::ZipFile;
use zip::result::ZipError;
use zip::{CompressionMethod, ZipArchive};

/// Maximum uncompressed size accepted for one module archive entry.
pub const MAX_ENTRY_BYTES: u64 = 64 << 20;

/// Maximum source size accepted for structured archive discovery.
pub const MAX_SOURCE_BYTES: u64 = 128 << 20;

/// One playable module discovered in an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Entry index in the ZIP central directory.
    pub index: usize,
    /// Entry name as recorded in the archive.
    pub name: String,
    /// Declared uncompressed size.
    pub size: u64,
    /// Module format inferred from the file extension.
    pub format: ModuleFormat,
}

/// The extension-inferred kind of a structured directory entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogKind {
    /// A native module; its contents have not been parsed.
    Module(ModuleFormat),
    /// A ZIP container.
    Zip,
}

/// A recognized central-directory entry, including entries that cannot be extracted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    /// Central-directory index; use this rather than a decoded name for extraction.
    pub index: usize,
    /// Decoded name, preserving case and separators.
    pub name: String,
    /// Original member-name bytes, before decoding.
    pub raw_name: Vec<u8>,
    /// Declared uncompressed size.
    pub size: u64,
    /// Module or container inferred from the extension.
    pub kind: CatalogKind,
    /// Empty when extractable; otherwise the known size/method/encryption failure.
    pub error: String,
}

/// One discovered module locator, relative to an outer ZIP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogModule {
    /// Empty for direct members, otherwise the outer member naming the inner ZIP.
    pub container_member: String,
    /// The module entry in its own container.
    pub member: CatalogEntry,
}

/// A recoverable directory, container or duplicate-locator problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogIssue {
    /// Path relative to the outer ZIP.
    pub path: String,
    /// Human-readable explanation.
    pub reason: String,
}

/// A directory catalog of modules through at most one inner ZIP level.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArchiveCatalog {
    /// Modules in central-directory traversal order, including unextractable members.
    pub entries: Vec<CatalogModule>,
    /// Recoverable problems; an invalid outer archive instead returns an error.
    pub issues: Vec<CatalogIssue>,
}

/// A failure to inspect or extract a module archive.
#[derive(Debug)]
pub enum ArchiveError {
    /// The bytes do not begin with a ZIP archive signature.
    NotAnArchive,
    /// The ZIP structure or an entry stream is malformed.
    Corrupt(String),
    /// An entry uses compression or encryption this crate does not support.
    UnsupportedMethod(String),
    /// An entry exceeds [`MAX_ENTRY_BYTES`].
    TooLarge { name: String, size: u64 },
    /// The archive contains no recognised module entries.
    NoModules,
    /// A structured catalog source exceeds [`MAX_SOURCE_BYTES`].
    SourceTooLarge { size: u64 },
    /// The requested central-directory index does not exist.
    NoSuchEntry,
    /// More than one entry matches a normalised name.
    AmbiguousEntry(String),
}

impl Display for ArchiveError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ArchiveError::NotAnArchive => formatter.write_str("not a ZIP archive"),
            ArchiveError::Corrupt(message) => write!(formatter, "corrupt ZIP archive: {message}"),
            ArchiveError::UnsupportedMethod(message) => write!(formatter, "unsupported ZIP method: {message}"),
            ArchiveError::TooLarge { name, size } => write!(formatter, "archive entry `{name}` is too large ({size} bytes)"),
            ArchiveError::SourceTooLarge { size } => write!(formatter, "archive source is too large ({size} bytes; limit {MAX_SOURCE_BYTES})"),
            ArchiveError::NoModules => formatter.write_str("the archive contains no playable modules"),
            ArchiveError::AmbiguousEntry(names) => write!(formatter, "ambiguous archive entry: {names}"),
            ArchiveError::NoSuchEntry => formatter.write_str("the archive entry does not exist"),
        }
    }
}

impl std::error::Error for ArchiveError {}

/// Return whether the bytes begin with a local-file or empty-archive ZIP signature.
pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06")
}

/// Inspect recognized module and ZIP entries without decompressing their contents.
/// Unlike [`list_modules`], known unextractable entries are retained with an error.
pub fn list_catalog_entries(bytes: &[u8]) -> Result<Vec<CatalogEntry>, ArchiveError> {
    catalog_directory(bytes).map(|(entries, _)| entries)
}

fn catalog_directory(bytes: &[u8]) -> Result<(Vec<CatalogEntry>, Vec<CatalogIssue>), ArchiveError> {
    if bytes.len() as u64 > MAX_SOURCE_BYTES { return Err(ArchiveError::SourceTooLarge { size: bytes.len() as u64 }); }
    let (archive, duplicate_names) = open_archive_with_duplicates(bytes)?;
    // A truncated outer ZIP must not silently become a valid ZIP embedded in one
    // of its stored members when the decoder searches backward for another footer.
    if archive.offset() != 0 { return Err(ArchiveError::Corrupt("central directory does not describe the outer ZIP".into())); }
    let mut entries = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_data(index).map_err(map_zip_error)?;
        let name = entry.name().map_err(map_zip_error)?.into_owned();
        if entry.is_dir() || is_resource_fork(&name.replace('\\', "/")) { continue; }
        let kind = if let Some(format) = format_from_name(&name) { CatalogKind::Module(format) }
            else if name.rsplit_once('.').is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("zip")) { CatalogKind::Zip }
            else { continue };
        let error = if entry.size() > MAX_ENTRY_BYTES {
            ArchiveError::TooLarge { name: name.clone(), size: entry.size() }.to_string()
        } else { ensure_supported_method(entry.compression(), entry.encrypted(), entry.name_raw()).err().map(|error| error.to_string()).unwrap_or_default() };
        entries.push(CatalogEntry { index, name, raw_name: entry.name_raw().to_vec(), size: entry.size(), kind, error });
    }
    let issues = duplicate_names.into_iter().filter_map(|raw_name| {
        entries.iter().find(|entry| entry.raw_name == raw_name).map(|entry| CatalogIssue {
            path: entry.name.clone(), reason: "duplicate raw locator; keeping the first central-directory entry".into(),
        })
    }).collect();
    Ok((entries, issues))
}

/// Catalog an outer ZIP and one inner ZIP level without extracting any module.
/// Inner containers are extracted only after size/method and resolver-identity checks.
/// Duplicate decoded locator names keep the first directory entry, even if unplayable.
pub fn catalog(bytes: &[u8]) -> Result<ArchiveCatalog, ArchiveError> {
    let (outer, issues) = catalog_directory(bytes)?;
    let mut catalog = ArchiveCatalog { entries: Vec::new(), issues };
    let mut seen = HashSet::new();
    for entry in outer {
        if !seen.insert(entry.name.clone()) {
            catalog.issues.push(CatalogIssue { path: entry.name, reason: "duplicate decoded locator; keeping the first central-directory entry".into() });
            continue;
        }
        if matches!(entry.kind, CatalogKind::Module(_)) {
            catalog.entries.push(CatalogModule { container_member: String::new(), member: entry });
            continue;
        }
        if !entry.error.is_empty() {
            catalog.issues.push(CatalogIssue { path: entry.name, reason: entry.error });
            continue;
        }
        // Container locators have no raw-name field in the existing wire format.
        // Exact/raw/encoded precedence in find_entry must resolve this very slot.
        match find_entry(bytes, &entry.name, None) {
            Ok(index) if index == entry.index => (),
            result => {
                let reason = match result { Ok(_) => "container path resolves to a different central-directory entry".into(), Err(error) => error.to_string() };
                catalog.issues.push(CatalogIssue { path: entry.name, reason });
                continue;
            }
        }
        let inner = match extract(bytes, entry.index).and_then(|inner| catalog_directory(&inner)) {
            Ok((inner, issues)) => {
                catalog.issues.extend(issues.into_iter().map(|issue| CatalogIssue { path: format!("{}/{}", entry.name, issue.path), reason: issue.reason }));
                inner
            },
            Err(error) => { catalog.issues.push(CatalogIssue { path: entry.name, reason: error.to_string() }); continue; }
        };
        let mut seen_inner = HashSet::new();
        for member in inner {
            let path = format!("{}/{}", entry.name, member.name);
            if !seen_inner.insert(member.name.clone()) {
                catalog.issues.push(CatalogIssue { path, reason: "duplicate decoded locator; keeping the first central-directory entry".into() });
            } else if member.kind == CatalogKind::Zip {
                catalog.issues.push(CatalogIssue { path, reason: "deeper ZIP nesting is not supported".into() });
            } else {
                catalog.entries.push(CatalogModule { container_member: entry.name.clone(), member });
            }
        }
    }
    Ok(catalog)
}

/// List supported, unencrypted module entries in central-directory order.
///
/// Unsupported module entries are skipped. If all module entries are unsupported,
/// return [`ArchiveError::UnsupportedMethod`] with their methods and names; an archive
/// without recognised module entries returns [`ArchiveError::NoModules`].
pub fn list_modules(bytes: &[u8]) -> Result<Vec<ArchiveEntry>, ArchiveError> {
    let mut archive = open_archive(bytes)?;
    let mut modules = Vec::new();
    let mut unsupported = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
        let name = entry.name().map_err(map_zip_error)?.into_owned();
        if entry.is_dir() || is_resource_fork(&name) {
            continue;
        }
        let Some(format) = format_from_name(&name) else { continue };
        if let Err(ArchiveError::UnsupportedMethod(reason)) = ensure_supported(&entry) {
            unsupported.push(reason);
            continue;
        }
        modules.push(ArchiveEntry { index, name, size: entry.size(), format });
    }
    if !modules.is_empty() {
        Ok(modules)
    } else if !unsupported.is_empty() {
        Err(ArchiveError::UnsupportedMethod(unsupported.join("; ")))
    } else {
        Err(ArchiveError::NoModules)
    }
}

/// List immediate ZIP containers by central-directory index without extracting them.
/// A container this crate cannot extract is skipped, as `list_modules` skips modules.
pub fn list_zip_containers(bytes: &[u8]) -> Result<Vec<usize>, ArchiveError> {
    let mut archive = open_archive(bytes)?;
    let mut containers = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
        let name = entry.name().map_err(map_zip_error)?;
        if !entry.is_dir() && !is_resource_fork(&name) && name.rsplit_once('.').is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("zip")) && ensure_supported(&entry).is_ok() {
            containers.push(index);
        }
    }
    Ok(containers)
}

/// Extract one supported, unencrypted central-directory entry.
///
/// Stored, Deflate, Shrink, Implode and Reduce (factors 1–4) are supported.
/// Check the central-directory size before reading. Legacy decoders buffer compressed
/// input and retain decoded output; a streaming size guard also limits the output.
pub fn extract(bytes: &[u8], index: usize) -> Result<Vec<u8>, ArchiveError> {
    let mut archive = open_archive(bytes)?;
    if index >= archive.len() {
        return Err(ArchiveError::NoSuchEntry);
    }
    let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
    ensure_supported(&entry)?;
    let name = entry.name().map_err(map_zip_error)?.into_owned();
    let declared_size = entry.size();
    if declared_size > MAX_ENTRY_BYTES {
        return Err(ArchiveError::TooLarge { name, size: declared_size });
    }

    drop(entry);
    let entry = archive.by_index(index).map_err(map_zip_error)?;
    let initial_capacity = usize::try_from(declared_size).unwrap_or(usize::MAX);
    let mut output = Vec::with_capacity(initial_capacity);
    entry.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut output)
        .map_err(|error| ArchiveError::Corrupt(error.to_string()))?;
    if output.len() as u64 > MAX_ENTRY_BYTES {
        return Err(ArchiveError::TooLarge { name, size: output.len() as u64 });
    }
    Ok(output)
}

/// Find a member without decompressing any entry. Exact raw names take precedence
/// over UTF-8/CP437 candidates, then a unique case/separator-normalised match.
pub fn find_entry(bytes: &[u8], name: &str, raw_name: Option<&[u8]>) -> Result<usize, ArchiveError> {
    let mut archive = open_archive(bytes)?;
    let encoded = encode_cp437(name);
    let normalised = name.replace('\\', "/").to_lowercase();
    let mut raw_match = None;
    let mut encoded_match = None;
    let mut fallback = Vec::new();
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(map_zip_error)?;
        let decoded = entry.name().map_err(map_zip_error)?;
        if entry.is_dir() || is_resource_fork(&decoded.replace('\\', "/")) { continue; }
        if raw_match.is_none() && raw_name.is_some_and(|raw| entry.name_raw() == raw) { raw_match = Some(index); }
        if encoded_match.is_none() && (entry.name_raw() == name.as_bytes() || encoded.as_deref().is_some_and(|raw| entry.name_raw() == raw)) {
            encoded_match = Some(index);
        }
        if decoded.replace('\\', "/").to_lowercase() == normalised { fallback.push((index, decoded.into_owned())); }
    }
    if let Some(index) = raw_match.or(encoded_match) { return Ok(index); }
    match fallback.as_slice() {
        [] => Err(ArchiveError::NoSuchEntry),
        [(index, _)] => Ok(*index),
        _ => Err(ArchiveError::AmbiguousEntry(fallback.into_iter().map(|(_, name)| name).collect::<Vec<_>>().join(", "))),
    }
}

fn encode_cp437(name: &str) -> Option<Vec<u8>> {
    const HIGH: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■ ";
    name.chars().map(|character| {
        if character.is_ascii() { Some(character as u8) }
        else { HIGH.chars().position(|candidate| candidate == character).map(|index| (index + 128) as u8) }
    }).collect()
}

fn open_archive(bytes: &[u8]) -> Result<ZipArchive<Cursor<Cow<'_, [u8]>>>, ArchiveError> {
    open_archive_with_duplicates(bytes).map(|(archive, _)| archive)
}

type OpenArchive<'a> = (ZipArchive<Cursor<Cow<'a, [u8]>>>, Vec<Vec<u8>>);

fn directory_error() -> ArchiveError { ArchiveError::Corrupt("invalid duplicate-bearing central directory/footer".into()) }

fn checked_range(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8], ArchiveError> {
    bytes.get(offset..offset.checked_add(length).ok_or_else(directory_error)?).ok_or_else(directory_error)
}

fn directory_word(bytes: &[u8], offset: usize) -> Result<u16, ArchiveError> {
    Ok(u16::from_le_bytes(checked_range(bytes, offset, 2)?.try_into().map_err(|_| directory_error())?))
}

fn directory_qword(bytes: &[u8], offset: usize) -> Result<u64, ArchiveError> {
    Ok(u64::from_le_bytes(checked_range(bytes, offset, 8)?.try_into().map_err(|_| directory_error())?))
}

fn put_directory_field(bytes: &mut [u8], offset: usize, value: &[u8]) -> Result<(), ArchiveError> {
    bytes.get_mut(offset..offset.checked_add(value.len()).ok_or_else(directory_error)?).ok_or_else(directory_error)?.copy_from_slice(value);
    Ok(())
}

/// Normalize duplicate raw names once in a caller-owned archive buffer.
///
/// This intentionally shares the legacy listing/extraction policy: the first central
/// directory entry wins. Compaction changes only directory/footer bytes, preserves the
/// allocation and local data offsets, and lets subsequent borrowed lookups avoid a
/// source-sized compatibility copy. No entry is decompressed here.
pub fn prepare_owned_archive(mut bytes: Vec<u8>) -> Result<Vec<u8>, ArchiveError> {
    if bytes.len() as u64 > MAX_SOURCE_BYTES { return Err(ArchiveError::SourceTooLarge { size: bytes.len() as u64 }); }
    if !is_zip(&bytes) { return Err(ArchiveError::NotAnArchive); }
    let archive = ZipArchive::new(Cursor::new(Cow::Borrowed(bytes.as_slice()))).map_err(map_zip_error)?;
    if archive.offset() != 0 { return Err(ArchiveError::Corrupt("central directory does not describe the outer ZIP".into())); }
    let directory = duplicate_directory(&bytes, &archive)?;
    drop(archive);
    if let Some(directory) = directory { compact_duplicate_directory(&mut bytes, &directory)?; }
    Ok(bytes)
}

struct DuplicateDirectory {
    directory_start: usize,
    records: Vec<std::ops::Range<usize>>,
    position: usize,
    duplicates: Vec<Vec<u8>>,
}

/// zip's name map replaces identical raw names with the last payload. Preserve the
/// first central record for both legacy and session callers. Unknown duplicate-bearing
/// footer layouts fail explicitly rather than being guessed.
fn open_archive_with_duplicates(bytes: &[u8]) -> Result<OpenArchive<'_>, ArchiveError> {
    if !is_zip(bytes) { return Err(ArchiveError::NotAnArchive); }
    let archive = ZipArchive::new(Cursor::new(Cow::Borrowed(bytes))).map_err(map_zip_error)?;
    let Some(directory) = duplicate_directory(bytes, &archive)? else { return Ok((archive, Vec::new())); };
    drop(archive);
    let mut normalized = bytes.to_vec();
    compact_duplicate_directory(&mut normalized, &directory)?;
    let archive = ZipArchive::new(Cursor::new(Cow::Owned(normalized))).map_err(map_zip_error)?;
    Ok((archive, directory.duplicates))
}

fn duplicate_directory(bytes: &[u8], archive: &ZipArchive<Cursor<Cow<'_, [u8]>>>) -> Result<Option<DuplicateDirectory>, ArchiveError> {
    if archive.is_empty() { return Ok(None); }
    let directory_start = usize::try_from(archive.central_directory_start()).map_err(|_| directory_error())?;
    let last_header = (0..archive.len()).try_fold(0u64, |last, index| archive.by_index_data(index).map(|entry| last.max(entry.central_header_start()))).map_err(map_zip_error)?;
    let last_header = usize::try_from(last_header).map_err(|_| directory_error())?;
    let mut position = directory_start;
    let mut seen = HashSet::new();
    let mut records = Vec::new();
    let mut duplicates = Vec::new();
    while position <= last_header {
        let header = checked_range(bytes, position, 46)?;
        if !header.starts_with(b"PK\x01\x02") { return Err(directory_error()); }
        let name_length = directory_word(header, 28)? as usize;
        let extra_length = directory_word(header, 30)? as usize;
        let comment_length = directory_word(header, 32)? as usize;
        let record_length = 46usize.checked_add(name_length).and_then(|length| length.checked_add(extra_length)).and_then(|length| length.checked_add(comment_length)).ok_or_else(directory_error)?;
        let record = checked_range(bytes, position, record_length)?;
        let name = checked_range(record, 46, name_length)?;
        if seen.insert(name) { records.push(position..position + record_length); } else { duplicates.push(name.to_vec()); }
        position = position.checked_add(record_length).ok_or_else(directory_error)?;
    }
    if duplicates.is_empty() { return Ok(None); }
    Ok(Some(DuplicateDirectory { directory_start, records, position, duplicates }))
}

fn compact_duplicate_directory(bytes: &mut Vec<u8>, directory: &DuplicateDirectory) -> Result<(), ArchiveError> {
    let DuplicateDirectory { directory_start, records, position, .. } = directory;
    let (directory_start, position) = (*directory_start, *position);
    let directory_size = records.iter().try_fold(0usize, |size, record| size.checked_add(record.len())).ok_or_else(directory_error)?;
    let removed = position.checked_sub(directory_start).and_then(|size| size.checked_sub(directory_size)).ok_or_else(directory_error)?;
    let footer = bytes.get_mut(position..).ok_or_else(directory_error)?;
    let mut footer_position = 0usize;
    if footer.starts_with(b"PK\x05\x05") {
        footer_position = 6usize.checked_add(directory_word(footer, 4)? as usize).ok_or_else(directory_error)?;
    }
    if checked_range(footer, footer_position, 4)? == b"PK\x06\x06" {
        checked_range(footer, footer_position, 56)?;
        let zip64_length = usize::try_from(directory_qword(footer, footer_position + 4)?).map_err(|_| directory_error())?;
        if zip64_length < 44 { return Err(directory_error()); }
        put_directory_field(footer, footer_position + 24, &(records.len() as u64).to_le_bytes())?;
        put_directory_field(footer, footer_position + 32, &(records.len() as u64).to_le_bytes())?;
        put_directory_field(footer, footer_position + 40, &(directory_size as u64).to_le_bytes())?;
        footer_position = footer_position.checked_add(12).and_then(|offset| offset.checked_add(zip64_length)).ok_or_else(directory_error)?;
        if checked_range(footer, footer_position, 4)? != b"PK\x06\x07" { return Err(directory_error()); }
        checked_range(footer, footer_position, 20)?;
        let zip64_offset = directory_qword(footer, footer_position + 8)?.checked_sub(removed as u64).ok_or_else(directory_error)?;
        put_directory_field(footer, footer_position + 8, &zip64_offset.to_le_bytes())?;
        footer_position = footer_position.checked_add(20).ok_or_else(directory_error)?;
    }
    if checked_range(footer, footer_position, 4)? != b"PK\x05\x06" { return Err(directory_error()); }
    checked_range(footer, footer_position, 22)?;
    let count = u16::try_from(records.len()).unwrap_or(u16::MAX);
    put_directory_field(footer, footer_position + 8, &count.to_le_bytes())?;
    put_directory_field(footer, footer_position + 10, &count.to_le_bytes())?;
    put_directory_field(footer, footer_position + 12, &u32::try_from(directory_size).map_err(|_| directory_error())?.to_le_bytes())?;
    // Move retained directory records and the patched footer forward in the original
    // allocation. Local payload data is untouched and no second source buffer exists.
    let mut destination = directory_start;
    for record in records {
        bytes.copy_within(record.clone(), destination);
        destination += record.len();
    }
    let footer_length = bytes.len() - position;
    bytes.copy_within(position.., destination);
    bytes.truncate(destination + footer_length);
    Ok(())
}

fn format_from_name(name: &str) -> Option<ModuleFormat> {
    let extension = name.rsplit_once('.')?.1;
    if extension.eq_ignore_ascii_case("s3m") {
        Some(ModuleFormat::S3m)
    } else if extension.eq_ignore_ascii_case("mod") {
        Some(ModuleFormat::Mod)
    } else if extension.eq_ignore_ascii_case("mtm") {
        Some(ModuleFormat::Mtm)
    } else if extension.eq_ignore_ascii_case("xm") {
        Some(ModuleFormat::Xm)
    } else if extension.eq_ignore_ascii_case("it") {
        Some(ModuleFormat::It)
    } else {
        None
    }
}

fn is_resource_fork(name: &str) -> bool {
    name.split('/').next().is_some_and(|component| component.eq_ignore_ascii_case("__MACOSX"))
}

fn ensure_supported<R: Read>(entry: &ZipFile<'_, R>) -> Result<(), ArchiveError> {
    ensure_supported_method(entry.compression(), entry.encrypted(), entry.name_raw())
}

fn ensure_supported_method(method: CompressionMethod, encrypted: bool, raw_name: &[u8]) -> Result<(), ArchiveError> {
    match method {
        CompressionMethod::Stored | CompressionMethod::Deflated | CompressionMethod::Shrink
        | CompressionMethod::Implode | CompressionMethod::Reduce(1..=4) if !encrypted => Ok(()),
        method => Err(ArchiveError::UnsupportedMethod(format!("{method:?}{} for `{}`", if encrypted { " (encrypted)" } else { "" }, String::from_utf8_lossy(raw_name)))),
    }
}

fn map_zip_error(error: ZipError) -> ArchiveError {
    match error {
        ZipError::UnsupportedArchive(message) => ArchiveError::UnsupportedMethod(message.to_string()),
        other => ArchiveError::Corrupt(other.to_string()),
    }
}
