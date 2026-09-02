//! Structured fuzzing of the S3M loader. See `mod_structured.rs` for why.

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::{Mutation, arm_memory_cap, walk_module, walk_s3m_cells};

/// The three synthesised layouts, plus one of the repository owner's own 1994–96 modules
/// — the only real Scream Tracker 3 output in the tree, and the only base whose packed
/// patterns and sample headers were written by the tracker rather than by us.
const BASES: &[&[u8]] = &[
    include_bytes!("../seeds/s3m/minimal.s3m"),
    include_bytes!("../seeds/s3m/default-pan-block.s3m"),
    include_bytes!("../seeds/s3m/sixteen-channels.s3m"),
    include_bytes!("../../crates/starplayer-s3m/tests/fixtures/REFLEX.S3M"),
];

fuzz_target!(|mutation: Mutation| {
    let bytes = mutation.apply(BASES);
    arm_memory_cap(bytes.len());

    if let Ok(module) = starplayer_s3m::load(&bytes) {
        walk_module(&module);
        walk_s3m_cells(&module);
    }
});
