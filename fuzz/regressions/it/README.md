# `it` crash regressions

Inputs a fuzzer once crashed, hung or OOMed the Impulse Tracker loader on. Copy the
artifact `cargo fuzz` wrote into `fuzz/artifacts/<target>/` here, give it a name that says
what it was, and fix the loader in the same commit.

`crates/starplayer-offline/tests/fuzz_seeds.rs` replays every file in this directory
through all four loaders on the **pinned stable toolchain**, so the regression is checked
by `cargo test --workspace` on every commit, not only when someone next runs the fuzzer.

IT has two decoders no other format has — the packed pattern stream's five memories, and
the IT 2.14 / 2.15 sample decompressor — and two production allocation caps that guard
them (`starplayer_it::loader`'s decoded-pattern and decoded-PCM budgets). An artifact that
trips either cap belongs here even if it only ever returned `Err`.
