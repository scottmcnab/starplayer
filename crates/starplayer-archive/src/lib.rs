//! Reusable module archive discovery and extraction.
//!
//! The archive layer is deliberately format-agnostic and extension-driven. Format
//! loaders remain responsible for validating the extracted bytes.

#![forbid(unsafe_code)]

use std::fmt::{self, Display, Formatter};
use std::io::{Cursor, Read};

use starplayer_model::ModuleFormat;
use zip::read::ZipFile;
use zip::result::ZipError;
use zip::{CompressionMethod, ZipArchive};

/// Maximum uncompressed size accepted for one module archive entry.
pub const MAX_ENTRY_BYTES: u64 = 64 << 20;

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

fn open_archive(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>, ArchiveError> {
    if !is_zip(bytes) {
        return Err(ArchiveError::NotAnArchive);
    }
    ZipArchive::new(Cursor::new(bytes)).map_err(map_zip_error)
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
    match entry.compression() {
        CompressionMethod::Stored | CompressionMethod::Deflated | CompressionMethod::Shrink
        | CompressionMethod::Implode | CompressionMethod::Reduce(1..=4) if !entry.encrypted() => Ok(()),
        method => Err(ArchiveError::UnsupportedMethod(format!("{method:?}{} for `{}`", if entry.encrypted() { " (encrypted)" } else { "" }, String::from_utf8_lossy(entry.name_raw())))),
    }
}

fn map_zip_error(error: ZipError) -> ArchiveError {
    match error {
        ZipError::UnsupportedArchive(message) => ArchiveError::UnsupportedMethod(message.to_string()),
        other => ArchiveError::Corrupt(other.to_string()),
    }
}
