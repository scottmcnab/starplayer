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

#[path = "support/legacy_encode.rs"]
mod legacy_encode;

#[test]
fn legacy_entries_round_trip() {
    let input = legacy_encode::payload();
    for (method, flags, compressed) in legacy_streams() {
        let bytes = legacy_encode::archive(&[("reflex.s3m", input, method, flags, &compressed)]);
        assert_eq!(list_modules(&bytes).unwrap()[0].size, input.len() as u64);
        assert_eq!(extract(&bytes, 0).unwrap(), input, "method {method}, flags {flags}");
    }
}

#[test]
fn shrink_code_8191_survives_initial_allocation_and_partial_clear() {
    for partial_clear in [false, true] {
        let (input, compressed) = legacy_encode::shrink_max_code(partial_clear);
        let bytes = legacy_encode::archive(&[("max-code.s3m", &input, 1, 0, &compressed)]);
        assert_eq!(extract(&bytes, 0).unwrap(), input);
    }
}

fn legacy_streams() -> Vec<(u16, u16, Vec<u8>)> {
    let input = legacy_encode::payload();
    let mut streams = vec![(1, 0, legacy_encode::shrink(input))];
    for flags in [0, 2, 4, 6] { streams.push((6, flags, legacy_encode::implode(input, flags))); }
    for method in 2..=5 { streams.push((method, 0, legacy_encode::reduce(input))); }
    streams
}

#[test]
fn imploded_declared_oversize_is_refused_before_reading() {
    // Invalid stream, small local size, oversized central size: the guard wins.
    let mut bytes = legacy_encode::archive(&[("bomb.s3m", b"x", 6, 0, &[255])]);
    set_central_directory_size(&mut bytes, (MAX_ENTRY_BYTES + 1) as u32);
    assert!(matches!(extract(&bytes, 0), Err(ArchiveError::TooLarge { size, .. }) if size == MAX_ENTRY_BYTES + 1));
}

#[test]
fn legacy_decoder_uses_central_size_even_when_local_size_differs() {
    let input = legacy_encode::payload();
    for (method, flags, compressed) in legacy_streams() {
        let mut bytes = legacy_encode::archive(&[("reflex.s3m", input, method, flags, &compressed)]);
        bytes[22..26].copy_from_slice(&((MAX_ENTRY_BYTES + 1) as u32).to_le_bytes());
        assert_eq!(extract(&bytes, 0).unwrap(), input, "method {method}, flags {flags}");
    }
}

#[test]
fn truncated_imploded_stream_is_corrupt() {
    let input = legacy_encode::payload();
    let compressed = legacy_encode::implode(input, 0);
    let bytes = legacy_encode::archive(&[("reflex.s3m", input, 6, 0, &compressed[..compressed.len() / 2])]);
    assert!(matches!(extract(&bytes, 0), Err(ArchiveError::Corrupt(_))));
}

#[test]
fn mixed_deflated_and_imploded_modules_are_listed() {
    use flate2::{Compression, write::DeflateEncoder};
    let input = legacy_encode::payload();
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input).unwrap();
    let deflated = encoder.finish().unwrap();
    let imploded = legacy_encode::implode(input, 0);
    let bytes = legacy_encode::archive(&[("first.s3m", input, 8, 0, &deflated), ("second.s3m", input, 6, 0, &imploded)]);
    assert_eq!(list_modules(&bytes).unwrap().len(), 2);
    assert_eq!(extract(&bytes, 0).unwrap(), input);
    assert_eq!(extract(&bytes, 1).unwrap(), input);
}

#[test]
fn unsupported_entries_are_skipped_with_reasons_when_none_remain() {
    let bytes = legacy_encode::archive(&[
        ("deflate64.mod", b"x", 9, 0, b"x"),
        ("encrypted.s3m", b"x", 0, 1, b"x"),
        ("bzip2.it", b"x", 12, 0, b"x"),
        ("lzma.xm", b"x", 14, 0, b"x"),
        ("readme.txt", b"x", 98, 0, b"x"),
    ]);
    let Err(ArchiveError::UnsupportedMethod(reason)) = list_modules(&bytes) else { panic!("expected unsupported methods") };
    for name in ["deflate64.mod", "encrypted.s3m", "bzip2.it", "lzma.xm", "Unsupported(9)", "encrypted", "Unsupported(12)", "Unsupported(14)"] { assert!(reason.contains(name), "{reason}"); }
    assert!(!reason.contains("readme"));
    for index in 0..4 { assert!(matches!(extract(&bytes, index), Err(ArchiveError::UnsupportedMethod(_)))); }
    let mixed = legacy_encode::archive(&[("bad.mod", b"x", 9, 0, b"x"), ("good.s3m", b"ok", 0, 0, b"ok"), ("encrypted.it", b"x", 0, 1, b"x")]);
    let modules = list_modules(&mixed).unwrap();
    assert_eq!(modules.len(), 1);
    assert_eq!(modules[0].index, 1);
    assert_eq!(extract(&mixed, modules[0].index).unwrap(), b"ok");
    let notes = legacy_encode::archive(&[("notes.txt", b"x", 9, 0, b"x")]);
    assert!(matches!(list_modules(&notes), Err(ArchiveError::NoModules)));
}

#[test]
fn legacy_mutations_never_panic() {
    let input = legacy_encode::payload();
    let mut panics = Vec::new();
    for (method, flags, compressed) in legacy_streams() {
        // Preserve the ZIP directory while truncating the decoder's actual input.
        for length in 0..=compressed.len() {
            let bytes = legacy_encode::archive(&[("reflex.s3m", input, method, flags, &compressed[..length])]);
            if std::panic::catch_unwind(|| extract(&bytes, 0)).is_err() {
                panics.push(format!("method {method}, flags {flags}, truncate {length}"));
            }
        }
        for position in (0..compressed.len()).step_by(3) {
            for mask in [1, 8, 128, 252, 255] {
                let mut mutated = compressed.clone();
                mutated[position] ^= mask;
                let bytes = legacy_encode::archive(&[("reflex.s3m", input, method, flags, &mutated)]);
                if std::panic::catch_unwind(|| extract(&bytes, 0)).is_err() {
                    panics.push(format!("method {method}, flags {flags}, position {position}, mask {mask}"));
                }
            }
        }
    }
    assert!(panics.is_empty(), "decoder panics: {panics:?}");
}

/// Optional scratch output for the independent Info-ZIP oracle and fuzz seed generation.
#[test]
#[ignore = "writes generated archives under target/ for manual oracle checks"]
fn write_legacy_oracle_archives() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/legacy-zip-oracle");
    std::fs::create_dir_all(&directory).unwrap();
    let input = legacy_encode::payload();
    std::fs::write(directory.join("payload.bin"), input).unwrap();
    for (method, flags, compressed) in legacy_streams() {
        let bytes = legacy_encode::archive(&[("reflex.s3m", input, method, flags, &compressed)]);
        std::fs::write(directory.join(format!("method-{method}-flags-{flags}.zip")), bytes).unwrap();
    }
    let (max_code_input, compressed) = legacy_encode::shrink_max_code(true);
    std::fs::write(directory.join("shrink-max-code.zip"), legacy_encode::archive(&[("max-code.s3m", &max_code_input, 1, 0, &compressed)])).unwrap();
    std::fs::write(directory.join("max-code-payload.bin"), max_code_input).unwrap();
    for (name, method) in [("stored", CompressionMethod::Stored), ("deflated", CompressionMethod::Deflated)] {
        std::fs::write(directory.join(format!("{name}.zip")), archive(&[("reflex.s3m", input, method)], &[])).unwrap();
    }
}

// Regression for the Implode tree-count overflow fixed in zip 9.0.1 (zip2 PR #1028).
#[test]
fn malformed_implode_tree_count_is_corrupt() {
    let bytes = legacy_encode::archive(&[("x.mod", b"x", 6, 0, &[255])]);
    assert!(matches!(extract(&bytes, 0), Err(ArchiveError::Corrupt(_))));
}

#[test]
fn committed_archive_fuzz_seeds_extract() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/seeds/zip");
    for name in ["stored.zip", "deflated.zip", "imploded.zip", "shrunk.zip"] {
        let bytes = std::fs::read(directory.join(name)).unwrap();
        let modules = list_modules(&bytes).unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(extract(&bytes, modules[0].index).unwrap(), legacy_encode::payload());
    }
}

// The writer cannot express unflagged CP437 bytes that also form valid UTF-8.
fn unflagged_name(bytes: &mut [u8], original: &[u8], replacement: &[u8]) {
    assert_eq!(original.len(), replacement.len());
    for index in 0..bytes.len().saturating_sub(4) {
        let (flag_offset, name_offset) = match &bytes[index..index + 4] {
            b"PK\x03\x04" => (6, 30),
            b"PK\x01\x02" => (8, 46),
            _ => continue,
        };
        if bytes.get(index + name_offset..index + name_offset + original.len()) == Some(original) {
            bytes[index + flag_offset + 1] &= !8;
            bytes[index + name_offset..index + name_offset + replacement.len()].copy_from_slice(replacement);
        }
    }
}

#[test]
fn lookup_prioritises_raw_and_encoded_names_before_unique_fallback() {
    use starplayer_archive::find_entry;
    let bytes = archive(&[
        ("é.mod", b"utf8", CompressionMethod::Stored),
        ("Music/Other.MOD", b"case", CompressionMethod::Deflated),
        ("__MACOSX/skip.mod", b"fork", CompressionMethod::Stored),
        ("A.MOD", b"upper", CompressionMethod::Stored),
        ("a.mod", b"lower", CompressionMethod::Stored),
        ("🎵.mod", b"unicode", CompressionMethod::Stored),
    ], &["empty.mod/"]);
    assert_eq!(find_entry(&bytes, "é.mod", None).unwrap(), 1);
    assert_eq!(find_entry(&bytes, "🎵.mod", None).unwrap(), 6);
    assert_eq!(find_entry(&bytes, "é.mod", Some(b"absent.mod")).unwrap(), 1);
    assert_eq!(find_entry(&bytes, "music\\other.mod", None).unwrap(), 2);
    assert_eq!(find_entry(&bytes, "A.MOD", None).unwrap(), 4);
    assert_eq!(find_entry(&bytes, "A.MOD", Some(b"a.mod")).unwrap(), 5);
    assert!(matches!(find_entry(&bytes, "a.Mod", None), Err(ArchiveError::AmbiguousEntry(names)) if names.contains("A.MOD") && names.contains("a.mod")));
    for name in ["missing.mod", "empty.mod/", "__MACOSX/skip.mod"] {
        assert!(matches!(find_entry(&bytes, name, None), Err(ArchiveError::NoSuchEntry)));
    }
}

#[test]
fn lookup_matches_unflagged_cp437_even_when_bytes_are_valid_utf8() {
    use starplayer_archive::find_entry;
    let mut bytes = archive(&[("xx.mod", b"module", CompressionMethod::Stored)], &[]);
    unflagged_name(&mut bytes, b"xx.mod", b"\xc3\xa9.mod");
    assert_eq!(find_entry(&bytes, "├⌐.mod", None).unwrap(), 0);
    assert_eq!(find_entry(&bytes, "ignored", Some(b"\xc3\xa9.mod")).unwrap(), 0);
}

#[test]
fn lookup_does_not_inflate_legacy_streams() {
    use starplayer_archive::find_entry;
    for method in [0, 1, 2, 3, 4, 5, 6, 8] {
        let bytes = legacy_encode::archive(&[("broken.mod", b"module", method, 0, &[255])]);
        assert_eq!(find_entry(&bytes, "broken.mod", None).unwrap(), 0);
    }
}
