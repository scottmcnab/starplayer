//! Byte-level fuzzing of the FastTracker 2 loader.

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::{arm_memory_cap, walk_module, walk_xm_cells};

fuzz_target!(|data: &[u8]| {
    arm_memory_cap(data.len());

    let probed = starplayer_xm::probe(data);
    if let Ok(module) = starplayer_xm::load(data) {
        assert!(probed, "a file the loader accepted must also probe as an XM");
        walk_module(&module);
        walk_xm_cells(&module);
    }
});
