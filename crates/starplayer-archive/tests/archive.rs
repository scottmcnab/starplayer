use std::io::{Cursor, Write};

use starplayer_archive::{ArchiveError, MAX_ENTRY_BYTES, extract, is_zip, list_modules};
use starplayer_model::ModuleFormat;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn archive(entries: &[(&str, &[u8], CompressionMethod)], directories: &[&str]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    for directory in directories {
        writer.add_directory(*directory, SimpleFileOptions::default()).unwrap();
    }
    for (name, bytes, method) in entries {
        let options = SimpleFileOptions::default().compression_method(*method);
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn set_central_directory_size(bytes: &mut [u8], size: u32) {
    let central = bytes.windows(4).position(|window| window == b"PK\x01\x02").unwrap();
    bytes[central + 24..central + 28].copy_from_slice(&size.to_le_bytes());
}

#[test]
fn listing_is_case_insensitive_and_skips_non_modules() {
    let bytes = archive(
        &[
            ("music/REFLEX.S3M", b"reflex", CompressionMethod::Stored),
            ("second.s3M", b"second", CompressionMethod::Deflated),
            ("__MACOSX/._x.s3m", b"fork", CompressionMethod::Stored),
            ("readme.txt", b"notes", CompressionMethod::Stored),
        ],
        &["empty.s3m/"],
    );
    let modules = list_modules(&bytes).unwrap();
    assert_eq!(modules.len(), 2);
    assert_eq!(modules[0].name, "music/REFLEX.S3M");
    assert_eq!(modules[0].size, 6);
    assert_eq!(modules[0].format, ModuleFormat::S3m);
    assert_eq!(modules[1].name, "second.s3M");
}

#[test]
fn stored_and_deflated_entries_round_trip() {
    let stored = b"stored module bytes";
    let deflated = b"deflated module bytes repeated repeated repeated";
    let bytes = archive(
        &[
            ("stored.s3m", stored, CompressionMethod::Stored),
            ("deflated.S3M", deflated, CompressionMethod::Deflated),
        ],
        &[],
    );
    let modules = list_modules(&bytes).unwrap();
    assert_eq!(extract(&bytes, modules[0].index).unwrap(), stored);
    assert_eq!(extract(&bytes, modules[1].index).unwrap(), deflated);
}

#[test]
fn all_declared_module_extensions_are_classified() {
    let bytes = archive(
        &[
            ("one.mod", b"1", CompressionMethod::Stored),
            ("two.MTM", b"2", CompressionMethod::Stored),
            ("three.xm", b"3", CompressionMethod::Stored),
            ("four.IT", b"4", CompressionMethod::Stored),
        ],
        &[],
    );
    let formats: Vec<ModuleFormat> = list_modules(&bytes).unwrap().into_iter().map(|entry| entry.format).collect();
    assert_eq!(formats, [ModuleFormat::Mod, ModuleFormat::Mtm, ModuleFormat::Xm, ModuleFormat::It]);
}

#[test]
fn declared_oversize_is_refused_before_inflating() {
    let mut bytes = archive(&[("bomb.s3m", b"not actually large", CompressionMethod::Deflated)], &[]);
    let name_length = u16::from_le_bytes([bytes[26], bytes[27]]) as usize;
    let extra_length = u16::from_le_bytes([bytes[28], bytes[29]]) as usize;
    bytes[30 + name_length + extra_length] ^= 0xFF;
    set_central_directory_size(&mut bytes, (MAX_ENTRY_BYTES + 1) as u32);
    let error = extract(&bytes, 0).unwrap_err();
    assert!(matches!(error, ArchiveError::TooLarge { ref name, size } if name == "bomb.s3m" && size == MAX_ENTRY_BYTES + 1));
}

#[test]
fn a_truncated_archive_is_corrupt_without_panicking() {
    let mut bytes = archive(&[("song.s3m", b"module", CompressionMethod::Deflated)], &[]);
    bytes.truncate(bytes.len() - 12);
    assert!(matches!(list_modules(&bytes), Err(ArchiveError::Corrupt(_))));
}

#[test]
fn an_s3m_is_not_a_zip() {
    let mut bytes = vec![0u8; 96];
    bytes[44..48].copy_from_slice(b"SCRM");
    assert!(!is_zip(&bytes));
    assert!(matches!(list_modules(&bytes), Err(ArchiveError::NotAnArchive)));
}

#[test]
fn missing_and_empty_entries_have_specific_errors() {
    let bytes = archive(&[("readme.txt", b"notes", CompressionMethod::Stored)], &[]);
    assert!(matches!(list_modules(&bytes), Err(ArchiveError::NoModules)));
    assert!(matches!(extract(&bytes, 2), Err(ArchiveError::NoSuchEntry)));
}
