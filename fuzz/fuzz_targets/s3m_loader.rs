//! Byte-level fuzzing of the Scream Tracker 3 loader.

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::{arm_memory_cap, walk_module, walk_s3m_cells};

fuzz_target!(|data: &[u8]| {
    arm_memory_cap(data.len());

    let probed = starplayer_s3m::probe(data);
    if let Ok(module) = starplayer_s3m::load(data) {
        assert!(probed, "a file the loader accepted must also probe as an S3M");
        walk_module(&module);
        walk_s3m_cells(&module);
    }
});
