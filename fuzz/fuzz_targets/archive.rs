//! Byte-level ZIP listing and extraction, including legacy PKZIP methods.
#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_archive::{extract, list_modules};
use starplayer_fuzz::arm_memory_cap;

fuzz_target!(|data: &[u8]| {
    arm_memory_cap(data.len());
    if let Ok(entries) = list_modules(data) {
        for entry in entries { let _ = extract(data, entry.index); }
    }
});
