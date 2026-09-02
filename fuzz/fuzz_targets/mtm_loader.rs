//! Byte-level fuzzing of the MultiTracker loader.

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::{arm_memory_cap, walk_module, walk_mtm_cells};

fuzz_target!(|data: &[u8]| {
    arm_memory_cap(data.len());

    let probed = starplayer_mtm::probe(data);
    if let Ok(module) = starplayer_mtm::load(data) {
        assert!(probed, "a file the loader accepted must also probe as an MTM");
        walk_module(&module);
        walk_mtm_cells(&module);
    }
});
