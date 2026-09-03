//! Structured fuzzing of the XM loader. See `mod_structured.rs` for why.

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::{Mutation, arm_memory_cap, walk_module, walk_xm_cells};

/// The synthesised layouts, one per structural family the loader has a branch for: the
/// ordinary case, a 16-bit ping-pong loop, an instrument with no samples, an all-empty
/// pattern with an order past the pattern count, the header's maximum counts, and the
/// pre-1.04 layout whose PCM sits after the patterns.
const BASES: &[&[u8]] = &[
    include_bytes!("../seeds/xm/minimal.xm"),
    include_bytes!("../seeds/xm/pingpong-16bit.xm"),
    include_bytes!("../seeds/xm/zero-sample-instrument.xm"),
    include_bytes!("../seeds/xm/empty-pattern.xm"),
    include_bytes!("../seeds/xm/maximum-counts.xm"),
    include_bytes!("../seeds/xm/version-1.02.xm"),
];

fuzz_target!(|mutation: Mutation| {
    let bytes = mutation.apply(BASES);
    arm_memory_cap(bytes.len());

    if let Ok(module) = starplayer_xm::load(&bytes) {
        walk_module(&module);
        walk_xm_cells(&module);
    }
});
