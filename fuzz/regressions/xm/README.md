# `xm` crash regressions

Inputs a fuzzer once crashed, hung or OOMed a loader on. Copy the artifact `cargo fuzz`
wrote into `fuzz/artifacts/<target>/` here, give it a name that says what it was, and fix
the loader in the same commit.

`crates/starplayer-offline/tests/fuzz_seeds.rs` replays every file in this directory
through all four loaders on the **pinned stable toolchain**, so the regression is checked
by `cargo test --workspace` on every commit, not only when someone next runs the fuzzer.
