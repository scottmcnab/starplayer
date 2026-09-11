//! Byte-level fuzzing of the **module-image** reader (M8-I2).
//!
//! The image is what a device maps out of flash and hands straight to the engine, so its
//! reader carries the same invariant every loader does: arbitrary bytes produce either an
//! `Err` or a `Module` the rest of the engine can index — never a panic, never an OOM.
//! That matters more here than for a loader, not less: a firmware that plays a corrupted
//! flash partition has no operating system to catch it.
//!
//! The round trip is the second assertion. A module the reader accepted must survive
//! being written out and read back unchanged, which is what pins the writer and the reader
//! to one description of the format rather than two.

#![no_main]

use libfuzzer_sys::fuzz_target;
use starplayer_fuzz::{arm_memory_cap, walk_module};
use starplayer_model::Module;

fuzz_target!(|data: &[u8]| {
    arm_memory_cap(data.len());

    // `from_image_copied` rather than `from_image`: the borrowing constructors want a
    // `&'static [u8]`, and leaking every input would be an out-of-memory of the fuzzer's
    // own making. Only the storage differs — the parser, and every check in it, is shared.
    if let Ok(module) = Module::from_image_copied(data) {
        walk_module(&module);

        let rewritten = module.to_image();
        match Module::from_image_copied(&rewritten) {
            Ok(again) => assert!(again == module, "an image the reader accepted must survive a write and a read"),
            Err(error) => panic!("an image written from an accepted module did not read back: {error}"),
        }
    }
});
