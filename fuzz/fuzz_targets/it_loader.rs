//! Byte-level fuzzing of the Impulse Tracker loader.

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::{arm_memory_cap, walk_it_cells, walk_module};

fuzz_target!(|data: &[u8]| {
    arm_memory_cap(data.len());

    let probed = starplayer_it::probe(data);
    if let Ok(module) = starplayer_it::load(data) {
        assert!(probed, "a file the loader accepted must also probe as an IT");
        walk_module(&module);
        walk_it_cells(&module);
    }
});
