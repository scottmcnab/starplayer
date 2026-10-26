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

#[test]
fn zip_container_indices_include_non_module_slots_and_skip_resources() {
    let bytes = archive(&[
        ("notes.txt", b"notes", CompressionMethod::Stored),
        ("inner.ZIP", b"PK", CompressionMethod::Stored),
        ("__MACOSX/hidden.zip", b"PK", CompressionMethod::Stored),
        ("REFLEX.S3M", b"module", CompressionMethod::Stored),
    ], &["directory.zip/"]);
    assert_eq!(starplayer_archive::list_zip_containers(&bytes).unwrap(), vec![2]);
}


#[test]
fn structured_catalog_reads_direct_nested_and_inner_only_archives() {
    use starplayer_archive::{CatalogKind, catalog, list_catalog_entries};
    let inner = archive(&[("inner.IT", b"not a module", CompressionMethod::Deflated)], &[]);
    let outer = archive(&[("direct.MOD", b"not a module", CompressionMethod::Stored), ("folder/inner.zip", &inner, CompressionMethod::Deflated)], &[]);
    let directory = list_catalog_entries(&outer).unwrap();
    assert_eq!(directory.len(), 2);
    assert_eq!(directory[1].kind, CatalogKind::Zip);
    assert_eq!(directory[1].raw_name, b"folder/inner.zip");
    let result = catalog(&outer).unwrap();
    assert!(result.issues.is_empty());
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.entries[0].container_member, "");
    assert_eq!(result.entries[1].container_member, "folder/inner.zip");
    assert_eq!(result.entries[1].member.name, "inner.IT");
    let inner_only = archive(&[("only.zip", &inner, CompressionMethod::Stored)], &[]);
    assert_eq!(catalog(&inner_only).unwrap().entries.len(), 1);
    let mut bad_local_header = archive(&[("central-only.mod", b"ok", CompressionMethod::Stored)], &[]);
    bad_local_header[26..28].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(catalog(&bad_local_header).unwrap().entries.len(), 1);
    // Even invalid compressed module streams are listed; cataloging never inflates them.
    for method in [0, 1, 2, 3, 4, 5, 6, 8] {
        let broken = legacy_encode::archive(&[("broken.mod", b"module", method, 0, &[255])]);
        assert!(catalog(&broken).unwrap().entries[0].member.error.is_empty());
    }
}

#[test]
fn structured_catalog_keeps_unplayable_modules_and_reports_container_failures() {
    use starplayer_archive::catalog;
    let mut bytes = legacy_encode::archive(&[
        ("large.mod", b"x", 8, 0, &[255]),
        ("encrypted.s3m", b"x", 0, 1, b"x"),
        ("unsupported.it", b"x", 9, 0, b"x"),
        ("unsupported.zip", b"x", 12, 0, b"x"),
        ("encrypted.zip", b"x", 0, 1, b"x"),
        ("broken.zip", b"not a zip", 0, 0, b"not a zip"),
        ("good.mtm", b"ok", 0, 0, b"ok"),
    ]);
    set_central_directory_size(&mut bytes, (MAX_ENTRY_BYTES + 1) as u32);
    let result = catalog(&bytes).unwrap();
    assert_eq!(result.entries.len(), 4);
    for (index, reason) in ["too large", "encrypted", "unsupported"].iter().enumerate() { assert!(result.entries[index].member.error.contains(reason)); }
    assert!(result.entries[3].member.error.is_empty());
    assert_eq!(result.issues.len(), 3);
    assert_eq!(result.issues[2].path, "broken.zip");
    let mut oversized_container = archive(&[("large.zip", b"invalid", CompressionMethod::Stored)], &[]);
    set_central_directory_size(&mut oversized_container, (MAX_ENTRY_BYTES + 1) as u32);
    assert!(catalog(&oversized_container).unwrap().issues[0].reason.contains("too large"));
}

#[test]
fn structured_catalog_preserves_other_results_on_corrupt_inner_and_reports_depth() {
    use starplayer_archive::catalog;
    let inner = archive(&[("nested.xm", b"module", CompressionMethod::Stored), ("deeper.zip", b"never extracted", CompressionMethod::Stored)], &[]);
    let mut corrupt = inner.clone();
    corrupt.truncate(corrupt.len() - 12);
    let outer = archive(&[("direct.mod", b"ok", CompressionMethod::Stored), ("corrupt.zip", &corrupt, CompressionMethod::Stored), ("good.zip", &inner, CompressionMethod::Stored)], &[]);
    let result = catalog(&outer).unwrap();
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.issues.len(), 2);
    assert_eq!(result.issues[0].path, "corrupt.zip");
    assert_eq!(result.issues[1].path, "good.zip/deeper.zip");
    assert!(result.issues[1].reason.contains("deeper"));
    assert!(matches!(catalog(&corrupt), Err(ArchiveError::Corrupt(_))));
    assert!(matches!(catalog(b"module"), Err(ArchiveError::NotAnArchive)));
    assert!(catalog(&archive(&[], &[])).unwrap().entries.is_empty());
}

#[test]
fn structured_catalog_preserves_cp437_raw_names_and_exact_case_variants() {
    use starplayer_archive::{catalog, find_entry};
    let inner = archive(&[("song.mod", b"ok", CompressionMethod::Stored)], &[]);
    let mut outer = archive(&[("x.zip", &inner, CompressionMethod::Stored), ("A.ZIP", &inner, CompressionMethod::Stored), ("a.zip", &inner, CompressionMethod::Stored), ("x.mod", b"ok", CompressionMethod::Stored)], &[]);
    unflagged_name(&mut outer, b"x.zip", b"\x82.zip");
    unflagged_name(&mut outer, b"x.mod", b"\x82.mod");
    let result = catalog(&outer).unwrap();
    assert!(result.issues.is_empty());
    assert_eq!(result.entries.len(), 4);
    assert_eq!(result.entries[0].container_member, "é.zip");
    assert_eq!(result.entries[1].container_member, "A.ZIP");
    assert_eq!(result.entries[2].container_member, "a.zip");
    assert_eq!(result.entries[3].member.raw_name, b"\x82.mod");
    for entry in result.entries.iter().filter(|entry| entry.container_member.is_empty()) {
        assert_eq!(find_entry(&outer, &entry.member.name, Some(&entry.member.raw_name)).unwrap(), entry.member.index);
    }
}

#[test]
fn structured_catalog_keeps_first_decoded_locator_and_rejects_wrong_container_resolution() {
    use starplayer_archive::catalog;
    // Two different raw names decode to the same CP437 name: UTF-8-flagged versus
    // unflagged. They must retain the first directory entry, not the later payload.
    let inner = archive(&[("inside.mod", b"ok", CompressionMethod::Stored)], &[]);
    let mut outer = legacy_encode::archive(&[
        ("é.zip", &inner, 0, 0x800, &inner),
        ("x.zip", &inner, 0, 0, &inner),
        ("é.mod", b"first", 0, 0x800, b"first"),
        ("x.mod", b"second", 0, 0, b"second"),
    ]);
    unflagged_name(&mut outer, b"x.zip", b"\x82.zip");
    unflagged_name(&mut outer, b"x.mod", b"\x82.mod");
    let result = catalog(&outer).unwrap();
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.entries[1].member.size, 5);
    assert_eq!(result.issues.len(), 2);
    assert!(result.issues.iter().all(|issue| issue.reason.contains("duplicate")));
    // Raw C3 A9 decodes as é under the ZIP decoder, but is also the CP437
    // encoding of ├⌐. The earlier encoded match shadows that later UTF-8 name.
    let mut ambiguous = legacy_encode::archive(&[
        ("xx.zip", &inner, 0, 0, &inner),
        ("├⌐.zip", &inner, 0, 0x800, &inner),
        ("direct.it", b"ok", 0, 0, b"ok"),
    ]);
    unflagged_name(&mut ambiguous, b"xx.zip", b"\xc3\xa9.zip");
    let result = catalog(&ambiguous).unwrap();
    assert_eq!(result.entries.len(), 2);
    assert_eq!(result.entries[0].container_member, "é.zip");
    assert_eq!(result.issues.len(), 1);
    assert_eq!(result.issues[0].path, "├⌐.zip");
    assert!(result.issues[0].reason.contains("different central-directory"));
}

#[test]
fn structured_catalog_checks_source_limit_before_zip_parsing() {
    use starplayer_archive::{MAX_SOURCE_BYTES, catalog};
    let bytes = vec![0; MAX_SOURCE_BYTES as usize + 1];
    assert!(matches!(catalog(&bytes), Err(ArchiveError::SourceTooLarge { size }) if size == MAX_SOURCE_BYTES + 1));
}


fn zip64_footer(mut bytes: Vec<u8>) -> Vec<u8> {
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
    bytes.extend_from_slice(&count.to_le_bytes());
    bytes.extend_from_slice(&count.to_le_bytes());
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&offset.to_le_bytes());
    bytes.extend_from_slice(b"PK\x06\x07");
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&(end as u64).to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    footer[8..20].fill(255);
    bytes.extend_from_slice(&footer);
    bytes
}

#[test]
fn duplicate_raw_names_keep_first_payload_in_classic_and_zip64_catalog_and_legacy_apis() {
    use starplayer_archive::{catalog, find_entry, list_zip_containers};
    let first_inner = archive(&[("first.it", b"first inner", CompressionMethod::Stored)], &[]);
    let last_inner = archive(&[("last.it", b"last inner", CompressionMethod::Stored)], &[]);
    let bytes = legacy_encode::archive(&[
        ("same.mod", b"first", 0, 0, b"first"),
        ("inner.zip", &first_inner, 0, 0, &first_inner),
        ("same.mod", b"last payload", 0, 0, b"last payload"),
        ("other.mtm", b"other", 0, 0, b"other"),
        ("inner.zip", &last_inner, 0, 0, &last_inner),
    ]);
    for bytes in [bytes.clone(), zip64_footer(bytes)] {
        let result = catalog(&bytes).unwrap();
        assert_eq!(result.entries.len(), 3);
        assert_eq!(result.issues.len(), 2);
        assert_eq!(result.entries[0].member.size, 5);
        assert_eq!(result.entries[1].member.name, "first.it");
        assert_eq!(extract(&bytes, find_entry(&bytes, "same.mod", Some(b"same.mod")).unwrap()).unwrap(), b"first");
        assert_eq!(list_zip_containers(&bytes).unwrap(), vec![1]);
        assert_eq!(extract(&bytes, 1).unwrap(), first_inner);
        let modules = list_modules(&bytes).unwrap();
        assert_eq!(modules.iter().map(|entry| entry.index).collect::<Vec<_>>(), vec![0, 2]);
        assert_eq!(extract(&bytes, modules[1].index).unwrap(), b"other");
        let mut truncated = bytes.clone();
        truncated.truncate(truncated.len() - 12);
        assert!(catalog(&truncated).is_err());
    }
}

#[test]
fn owned_duplicate_preparation_compacts_in_place_and_preserves_legacy_first_entry_policy() {
    use starplayer_archive::{find_entry, prepare_owned_archive};
    let bytes = legacy_encode::archive(&[("same.mod", b"first", 0, 0, b"first"), ("same.mod", b"last", 0, 0, b"last")]);
    for bytes in [bytes.clone(), zip64_footer(bytes)] {
        let original_pointer = bytes.as_ptr();
        let original_capacity = bytes.capacity();
        let original_length = bytes.len();
        let prepared = prepare_owned_archive(bytes).unwrap();
        assert_eq!(prepared.as_ptr(), original_pointer);
        assert_eq!(prepared.capacity(), original_capacity);
        assert!(prepared.len() < original_length);
        let modules = list_modules(&prepared).unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].size, 5);
        assert_eq!(extract(&prepared, find_entry(&prepared, "same.mod", Some(b"same.mod")).unwrap()).unwrap(), b"first");
        let again = prepare_owned_archive(prepared).unwrap();
        assert_eq!(again.as_ptr(), original_pointer);
        assert_eq!(again.capacity(), original_capacity);
    }
    assert!(matches!(prepare_owned_archive(vec![0; starplayer_archive::MAX_SOURCE_BYTES as usize + 1]), Err(ArchiveError::SourceTooLarge { .. })));
}

#[test]
fn duplicate_directory_mutations_return_errors_without_panics() {
    use starplayer_archive::catalog;
    let bytes = legacy_encode::archive(&[("same.mod", b"first", 0, 0, b"first"), ("same.mod", b"last", 0, 0, b"last")]);
    let directory = bytes.windows(4).position(|window| window == b"PK\x01\x02").unwrap();
    for index in directory..bytes.len() {
        for mask in [1, 128, 255] {
            let mut mutated = bytes.clone();
            mutated[index] ^= mask;
            assert!(std::panic::catch_unwind(|| catalog(&mutated)).is_ok(), "mutation at {index}, mask {mask}");
        }
    }
}


#[test]
fn duplicate_inner_names_report_paths_and_keep_first_payload() {
    use starplayer_archive::{catalog, find_entry};
    let inner = legacy_encode::archive(&[("same.it", b"first", 0, 0, b"first"), ("same.it", b"last", 0, 0, b"last")]);
    let outer = archive(&[("inner.zip", &inner, CompressionMethod::Stored)], &[]);
    let result = catalog(&outer).unwrap();
    assert_eq!(result.entries.len(), 1);
    assert_eq!(result.entries[0].member.size, 5);
    assert_eq!(result.issues[0].path, "inner.zip/same.it");
    assert_eq!(extract(&inner, find_entry(&inner, "same.it", Some(b"same.it")).unwrap()).unwrap(), b"first");
}
