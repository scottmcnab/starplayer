//! Structured fuzzing of the MOD loader: start from a module that loads, then disturb
//! its fields.
//!
//! Random bytes rarely survive `M.K.` at offset 1080, and the ones that do rarely survive
//! the pattern-count scan, so a byte-level fuzzer spends most of its budget in the first
//! hundred bytes of the file. Mutating a valid module reaches the sample loop clamps, the
//! FLT8 pairing and the order-list mapping immediately.

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::{Mutation, arm_memory_cap, walk_mod_cells, walk_module};

/// One base per structural variant the loader branches on: the golden fixture, the
/// smallest well-formed file, the paired-halves FLT8 layout and a wide channel count.
const BASES: &[&[u8]] = &[
    include_bytes!("../seeds/mod/synthetic-golden.mod"),
    include_bytes!("../seeds/mod/mk-minimal.mod"),
    include_bytes!("../seeds/mod/flt8-paired-halves.mod"),
    include_bytes!("../seeds/mod/chn16-two-digit.mod"),
];

fuzz_target!(|mutation: Mutation| {
    let bytes = mutation.apply(BASES);
    arm_memory_cap(bytes.len());

    if let Ok(module) = starplayer_mod::load(&bytes) {
        walk_module(&module);
        walk_mod_cells(&module);
    }
});
