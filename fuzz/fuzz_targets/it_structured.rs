//! Structured fuzzing of the IT loader. See `mod_structured.rs` for why.

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::{Mutation, arm_memory_cap, walk_it_cells, walk_module};

/// The synthesised layouts of `fuzz/seeds/it/`, which between them reach the sample-mode
/// and instrument-mode paths, both instrument layouts, both decompressors, the sustain
/// loop, the MIDI macro block and the widest pattern the loader accepts. There is no real
/// Impulse Tracker output in the tree to add to them: the pinned libxmp corpus is not ours
/// to commit, and `cargo xtask fuzz --seed` copies it into the byte-level target's working
/// corpus at run time instead.
const BASES: &[&[u8]] = &[
    include_bytes!("../seeds/it/minimal.it"),
    include_bytes!("../seeds/it/instrument-mode.it"),
    include_bytes!("../seeds/it/compressed-8bit.it"),
    include_bytes!("../seeds/it/compressed-16bit.it"),
    include_bytes!("../seeds/it/sustain-ping-pong.it"),
    include_bytes!("../seeds/it/midi-macros.it"),
    include_bytes!("../seeds/it/maximum-counts.it"),
    include_bytes!("../seeds/it/old-instruments.it"),
];

fuzz_target!(|mutation: Mutation| {
    let bytes = mutation.apply(BASES);
    arm_memory_cap(bytes.len());

    if let Ok(module) = starplayer_it::load(&bytes) {
        walk_module(&module);
        walk_it_cells(&module);
    }
});
