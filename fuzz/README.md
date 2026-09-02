# StarPlayer loader fuzzing

`cargo-fuzz` targets for the three loaders, plus the seed corpus they start from. This is
M2-task-C7; the invariant is the one in that task file and in `plans/product/02-roadmap.md`
T7: **a loader never panics, never OOMs, and always returns either `Err` or a
clamped-but-valid `Module`.**

## Layout

| Path | What it is |
|---|---|
| `fuzz_targets/{mod,s3m,mtm}_loader.rs` | byte-level: arbitrary bytes into one loader |
| `fuzz_targets/{mod,s3m,mtm}_structured.rs` | structured: a valid module with its fields disturbed |
| `src/lib.rs` | the memory cap, the mutation program, the post-load walk |
| `seeds/<format>/` | committed seeds — ours or the owner's, never third-party bytes |
| `regressions/<format>/` | inputs that once crashed a loader; replayed by the stable test suite |
| `dictionaries/tracker.dict` | the format magic values, so mutation finds a loader body at all |

## Running it

```sh
cargo xtask fuzz --seed                 # fill fuzz/corpus/* from seeds, fixtures and the pinned corpus
cargo xtask ci --job fuzz-smoke         # the bounded per-commit run, all six targets
cargo xtask fuzz --target s3m_loader --seconds 900   # a long run on one target
```

All three need a **nightly** toolchain and `cargo install cargo-fuzz`: libFuzzer needs
`-Zsanitizer` and `-Cpasses=sancov-module`, which stable does not accept. That is why the
fuzz job is the one CI job not on the pinned toolchain, and why the corpus is *also*
replayed by `crates/starplayer-offline/tests/fuzz_seeds.rs`, which is pinned-stable and
runs in `cargo test --workspace`.

## Corpus licensing

Only two kinds of bytes are committed here: modules this repository generates (the C6a
synthesised MOD and MTM, and the small hand-built edge cases from C3b and C5) and the
repository owner's own S3Ms, which already live in `crates/starplayer-s3m/tests/fixtures/`.
The pinned libxmp corpus is **not** committed — `cargo xtask fuzz --seed` copies it into
the working corpus out of the conformance cache at run time, and the working corpus is
git-ignored.
