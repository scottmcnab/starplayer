//! Byte-level fuzzing of the ProTracker MOD loader.
//!
//! The contract, from M2-task-C7: arbitrary bytes in, and the loader either returns an
//! `Err` or a `Module` the rest of the engine can index — never a panic, never an
//! unbounded allocation.

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::{arm_memory_cap, walk_mod_cells, walk_module};
use starplayer_mod::{LoadOptions, StereoSeparation};

fuzz_target!(|data: &[u8]| {
    arm_memory_cap(data.len());

    let probed = starplayer_mod::probe(data);
    if let Ok(module) = starplayer_mod::load(data) {
        assert!(probed, "a file the loader accepted must also probe as a MOD");
        walk_module(&module);
        walk_mod_cells(&module);
    }

    // C3a's headphone panning is a second entry point into the same parse, and the only
    // one that reaches `default_pan` with a separation other than hard.
    let options = LoadOptions { stereo_separation: StereoSeparation::percent(60) };
    let _ = starplayer_mod::load_with_options(data, options);
});
