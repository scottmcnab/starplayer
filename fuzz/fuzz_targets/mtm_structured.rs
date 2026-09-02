//! Structured fuzzing of the MTM loader. See `mod_structured.rs` for why.

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::{Mutation, arm_memory_cap, walk_module, walk_mtm_cells};

/// The golden fixture, the smallest well-formed file, the Dual Module Player tempo split
/// and the widest channel count the format allows.
const BASES: &[&[u8]] = &[
    include_bytes!("../seeds/mtm/synthetic-golden.mtm"),
    include_bytes!("../seeds/mtm/minimal.mtm"),
    include_bytes!("../seeds/mtm/dmp-tempo-split.mtm"),
    include_bytes!("../seeds/mtm/thirty-two-channels.mtm"),
];

fuzz_target!(|mutation: Mutation| {
    let bytes = mutation.apply(BASES);
    arm_memory_cap(bytes.len());

    if let Ok(module) = starplayer_mtm::load(&bytes) {
        walk_module(&module);
        walk_mtm_cells(&module);
    }
});
