# M1-task-B10 — Legacy ZIP methods: implode and shrink (`starplayer-archive`)

| Field | Value |
|---|---|
| Milestone | M1 follow-up ([master plan](../M1-master-plan.md)); extends [B8](M1-task-B8-archive-zip.md) |
| Depends on | B8 (landed) |
| Blocks | — (the CLI and web player gain it automatically through `starplayer-archive`) |
| Recommended model | gpt-6.1-sol |
| Verified by | agent (unit tests, Hornet corpus check, wasm size) + **owner (opens an old Hornet zip in the web player and the CLI)** |
| Status | **Implemented 2026-10-21**; owner acceptance open |

## Context for a fresh agent

Read `AGENTS.md` first (full variable names, compact formatting, one-line asserts, no
hand-written `unsafe`, no `cargo fmt`, never `git add -A`). Then read
`crates/starplayer-archive/src/lib.rs`, `crates/starplayer-archive/tests/archive.rs`, the
`zip` and `flate2` entries in the workspace `Cargo.toml` (with their comment about
keeping the read-only path on pure-Rust `miniz_oxide`), and
[B8](M1-task-B8-archive-zip.md). `cargo xtask ci` is green and must stay
green, and `cargo xtask wasm` must still build both wasm instances.

## Finding (2026-10-20, StarPlayer HEAD `8f1ea42`)

StarPlayer cannot open ZIP entries compressed with the pre-deflate PKZIP methods:

- `ensure_supported` in `crates/starplayer-archive/src/lib.rs` accepts only
  `CompressionMethod::Stored` and `CompressionMethod::Deflated`. Anything else returns
  `ArchiveError::UnsupportedMethod`. `list_modules` calls it for each module entry, so a
  single legacy entry fails the listing of the **whole** archive, not just that entry.
- The workspace depends on `zip = { version = "6", default-features = false, features =
  ["deflate-flate2"] }`. `zip` 6.0.0 already includes decoders for method 1 (Shrink),
  methods 2–5 (Reduce) and method 6 (Implode) in `src/legacy/`, behind the optional
  feature **`legacy-zip`**. That feature only adds `bitstream-io` (4.5.x, MIT/Apache-2.0,
  MSRV 1.83). StarPlayer does not enable it.

Real-world impact, from the ModDNA index of the local Hornet mirror
(`../moddna/index/hornet/hornet-all.csv`, 2026-10-20): 206 ZIP members in 124
archives use legacy methods (180 implode, 26 shrink; no reduce). 121 of them are
modules: 36 XM, 34 S3M (3 of them shrunk), 31 MOD and 20 IT, imploded unless noted.
Examples: `songs/1994/brother.zip`, `songs/1995/a/alien.zip`,
`contests/tt/tt06vote.zip`. Because listing fails per archive, any of those 124 ZIPs
whose module entries include a legacy one cannot be opened at all today, even where
their other entries are deflated.

ModDNA's own Python decoders for both methods were checked against Info-ZIP `unzip -p`
on all 206 members (byte-identical, CRC-valid). This gives an independent oracle for
this task's corpus check.

## Owner decisions (2026-10-21)

1. **Use `zip`'s `legacy-zip` feature** rather than writing decoders here. Fall back to
   our own decoders only if the corpus check below finds `zip` output wrong (report the
   discrepancy upstream), and **only after asking the owner**.
2. **Per-entry failure: skip with reason.** `list_modules` no longer fails the whole
   archive on one unsupported module entry. Unsupported module entries (encrypted,
   Deflate64, bzip2, LZMA, …) are skipped and the remaining playable modules are listed.
   `ArchiveEntry` and the CLI/web call sites are unchanged. If no playable module remains
   **and at least one module entry was skipped as unsupported**, `list_modules` returns
   `ArchiveError::UnsupportedMethod` naming the skipped entries' methods (not
   `NoModules`), so the user still sees why the archive cannot play. An archive with no
   module entries at all still returns `NoModules`. `extract` keeps refusing an
   unsupported entry with `UnsupportedMethod`. Update the B8 wording if it states the
   old all-or-nothing rule.
3. **Reduce (methods 2–5): accept.** It comes with the feature. Add a synthetic test only
   if a test-only reduce encoder is cheap; otherwise record here that reduce is accepted
   but untested against any corpus.

## Implementation notes (from reading `zip` 6.0.0 `src/legacy/`)

- The legacy decoders are **not streaming**: on the first `read` they `read_to_end` the
  whole compressed stream, `reserve(uncompressed_size)` and decode to the *declared*
  uncompressed size. So the `take(MAX_ENTRY_BYTES + 1)` guard in `extract` cannot bound
  memory for these methods; the **declared-size check before the first read is the real
  guard**. Confirm that the size `zip` hands the decoder is the same value
  `entry.size()` returns (central directory vs local header), and that `extract` checks
  it before any read. If they can differ, check both.
- `shrink.rs` `output_code` contains `unwrap()` and debug `assert!`s, and indexes `dst`
  by table positions. Find out whether malformed input can reach a panic (debug and
  release). A panic inside the page-side wasm aborts the player, so this matters. If a
  panic is reachable, do **not** patch `zip` or write our own decoders: record a minimal
  reproducer, report it in the final summary, and stop for an owner decision.

## Fixture approach (no DOSBox/PKZIP is available on the dev machine)

Use **test-only encoders** for shrink and implode (and reduce if cheap) under
`crates/starplayer-archive/tests/` (for example `tests/support/legacy_encode.rs`). They
build the ZIP entries in memory at test time, wrapped by a minimal hand-written ZIP
writer (local header + central directory + EOCD with the right method number, general
purpose flags for implode's 8 KiB-dictionary / 3-tree bits, and CRC-32). The encoders
need not compress well. A literal-heavy implode stream with valid Shannon–Fano trees is
fine, as long as it exercises at least one length/distance match. The input bytes are an
owner-authored module already in the tree, e.g.
`crates/starplayer-s3m/tests/fixtures/PETRI.S3M` or `REFLEX.S3M` (see that directory's
provenance README), or a truncated prefix of one to keep test time low. Record this
provenance in a comment at the top of the encoder module.

**Independent oracle:** once, write each generated archive to a scratch directory and
run Info-ZIP `unzip -t` and `unzip -p | sha256sum` on it (`/usr/bin/unzip` is
installed and supports shrink and implode; its build lacks Reduce). Record the result here. This proves
the encoders emit real PKZIP streams rather than streams that only `zip`'s own decoders
accept.

## Deliverables

1. Workspace `Cargo.toml`: add `legacy-zip` to the `zip` features, with a comment saying
   why. Keep `default-features = false` and the `miniz_oxide` path.
2. `ensure_supported` accepts `Shrink`, `Implode` (and `Reduce(_)` if decision 3 is yes)
   when the entry is not encrypted. The existing `MAX_ENTRY_BYTES` streaming guard stays
   in force for these methods.
3. Decision 2 implemented (or explicitly declined, with the reason recorded in this file).
4. Tests in `crates/starplayer-archive/tests/archive.rs`:
   - small committed fixtures: one imploded and one shrunk module entry. The `zip`
     crate cannot write these methods, so the fixtures must come from a licensed
     source, for example a module authored for the test and compressed with a period
     PKZIP 1.x/2.x under DOSBox, or bytes produced by a small test-only encoder. Record
     the fixture's provenance in the test or a README;
   - an imploded entry whose declared size exceeds `MAX_ENTRY_BYTES` is rejected;
   - a truncated imploded stream returns `ArchiveError::Corrupt`, not a panic;
   - an archive mixing deflated and imploded modules lists both.
5. A one-off corpus check (not a CI test, because Hornet files are not redistributable):
   extract every legacy member listed in the ModDNA index from `/mnt/d/hornet-music/`
   (read-only, in memory) and compare its SHA-256 with the `sha256` column of
   `hornet-all.csv`. Record the result (count matched, any failures) in this file.
6. Measure the change in the release `starplayer-web` page-side wasm size and record it.
   The worklet instance must not change, because it never sees a zip.
7. If a fuzz target covers archives, extend its seed corpus with the new fixtures.
   Otherwise note whether one is warranted.

## Implementation results (2026-10-21)

### Implementation and size guard

- Enabled `legacy-zip` for Shrink, Reduce (factors 1–4) and Implode. The workspace
  requires `zip` 9.0.1+; root and fuzz lockfiles resolve 9.0.5. Default features remain
  disabled, with the pure-Rust `deflate-flate2`/`rust_backend` path. `sha2` is a
  dev-dependency for the corpus checker. No decoder patch or replacement is needed.
- Listing uses raw metadata to skip unsupported/encrypted module entries while keeping
  playable entries' central indexes. If all modules are unsupported, it reports their
  names/methods; no module entries still means `NoModules`. Extraction refuses
  unsupported entries. B8 documents the updated dependency and listing behaviour.
- **The decoder receives the central-directory uncompressed size**, including ZIP64
  values, matching `ZipFile::size()`. The local header locates data without replacing
  that size. Tests verify a huge local size with a small central size extracts correctly
  for every legacy variant; an oversized central size returns `TooLarge` before reading.
  Legacy decoders buffer input and output on first read, so the declared-size check is
  essential: `take(MAX_ENTRY_BYTES + 1)` cannot bound their internal buffers.

### Fixtures, oracle and regression coverage

`tests/support/legacy_encode.rs` generates archives from the first 192 bytes of Scott
McNab's owner-authored REFLEX.S3M, with provenance recorded in source and the fuzz seed
README. No Hornet bytes were copied. Shrink covers literals; Implode covers matches
and flags 0, 2, 4, 6; Reduce covers empty follower sets and DLE escaping at factors 1–4.
Tests also cover mixed archives, unsupported/encrypted filtering, size guards, truncated
streams and the four generated fuzz seeds. `legacy_mutations_never_panic` runs in both
profiles, truncating every stream at each byte length and applying five XOR masks at
every third byte. Only the scratch oracle generator remains ignored.

Previously verified Info-ZIP `unzip -t` and `unzip -p` succeed for Stored, Deflate,
Shrink and all four Implode variants. Extracted SHA-256 equals the 192-byte payload:
`3088497de370bdb27d9e5d966998c88ade16562fa3b633ab24f5ff97baad25d9`.
The new `target/legacy-zip-oracle/shrink-max-code.zip` also passes `unzip -t`; `unzip -p`
matches its 7,937-byte payload, SHA-256
`1343f28b820329f7b0f9881fcc207e3d2fc98729a176d4c1bdc035956d68edbb`.
Reduce passes synthetic round-trip/mutation tests, but has no independent oracle:
installed Info-ZIP rejects it with exit 81, and the Hornet index has no Reduce members.

### Upstream defects found (zip 6.0.0, fixed in 9.0.1)

- zip2 PR #1028 fixes Implode's `u8` tree-count arithmetic overflow. A one-byte
  compressed stream `[0xff]` (method 6, flags 0, declared size 1) panicked in debug.
  `malformed_implode_tree_count_is_corrupt` now requires `ArchiveError::Corrupt`.
- zip2 PR #1029 fixes Shrink omitting LZW code 8191 from both initial allocation and
  the partial-clear free-code queue rebuild. This rejected valid Hornet members
  `songs/1996/m/mom_mzk.zip` / `MOM5.S3M` and `songs/1996/t/timeswtc.zip` /
  `TIMESWTC.S3M` with `Corrupt("invalid code")`; Info-ZIP output matched the index.
  `shrink_code_8191_survives_initial_allocation_and_partial_clear` uses
  `legacy_encode::shrink_max_code` to cover both paths. The reviewer verified it fails
  with `Corrupt("invalid code")` against 9.0.5 with the fix reverted and passes on 9.0.5.

### Fuzzing

The archive target is registered in fuzz Cargo.toml and xtask, with generated Stored,
Deflate, Implode and Shrink seeds. The reviewer-verified
`cargo xtask fuzz --target archive --seconds 60` run completed **487,833 runs in 61 s**,
no failures, peak RSS **496 MB**, using zip 9.0.5. The earlier crash artifact replayed
without failure and was deleted. Mutations stay in the ignored working corpus.

### Read-only Hornet corpus check

```sh
cargo run --release -p starplayer-archive --example legacy_corpus_check -- \
  /home/scott/projects/moddna/index/hornet/hornet-all.csv /mnt/d/hornet-music
```

Final run exited 0:

```text
total=206 matched=206 mismatched=0 failed=0 nested=0
modules: total=121 matched=121
```

All 206 members, including all 121 modules, match the index SHA-256 values. The corpus
exit criterion is met. The checker uses raw metadata for lookup and public `extract`;
module rows also use `list_modules`. Its quote-aware CSV parser has an example unit test.
Log: `target/b10-doc-corpus.log`.

### Release wasm sizes

`cargo xtask wasm` passed. Packaged files under `apps/starplayer-web/dist/`, compared
with the pre-B10 baseline:

| Instance / file | Before bytes | After bytes | Delta |
|---|---:|---:|---:|
| Page: `starplayer_web_bg.wasm` | 271,001 | 329,798 | +58,797 (+21.70%) |
| Worklet: `starplayer_host_wasm_bg.wasm` | 795,986 | 795,986 | 0 |

The worklet is byte-identical by size and SHA-256:
`e5abf2c4b20b6ef0515817a701d1997217887c2f726e2b07cff308dea2e0cd4e`.
Log: `target/b10-doc-wasm.log`.

### Verification and acceptance

- `cargo test -p starplayer-archive --locked`: passed, 17 tests, 1 ignored scratch
  generator (`target/b10-doc-debug.log`).
- `cargo test -p starplayer-archive --locked --release`: passed, 17 tests, 1 ignored
  scratch generator (`target/b10-doc-release.log`).
- `cargo clippy -p starplayer-archive --all-targets --locked -- -D warnings`: passed
  (`target/b10-doc-clippy.log`).
- `cargo test -p starplayer-archive --example legacy_corpus_check --locked`: passed,
  1 CSV parser test (`target/b10-doc-csv.log`).
- `cargo xtask ci`: **all 11 jobs passed**, exit 0: host tests, conformance, RT safety,
  goldens, SIMD, FMA audit, zero-cost trace audit, wasm builds, bare-metal checks,
  clippy and no-std purity. Used `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
  CARGO_PROFILE_TEST_DEBUG=0 CARGO_PROFILE_RELEASE_DEBUG=0` to limit disk use; no checks
  were skipped or weakened and no cache cleanup was needed. Log: `target/b10-doc-ci.log`.
- `cargo xtask wasm`: passed; sizes and unchanged worklet hash recorded above.
- `git diff --check`: passed.

- [x] All 121 module members and all 206 legacy members extract with matching SHA-256.
- [ ] Owner accepts the page wasm size increase.
- [ ] Owner opens `songs/1994/brother.zip` in the web player and with CLI `play`.

## Verification

```sh
cargo test -p starplayer-archive --locked
cargo test -p starplayer-archive --locked --release
cargo clippy -p starplayer-archive --all-targets --locked -- -D warnings
cargo xtask ci
cargo xtask wasm          # record page-side wasm size before and after
```

Then the corpus check from deliverable 5, and the owner opening `songs/1994/brother.zip`
(imploded MOD) in the web player and with `starplayer play`.

## Exit criteria

- All 121 legacy-compressed module members in the Hornet index extract with SHA-256
  values equal to the index (or every mismatch is explained and resolved).
- `cargo xtask ci` green; wasm builds; the size change is recorded and accepted by the
  owner.
- Fixture provenance recorded; no Hornet file committed.

## Out of scope

Encrypted ZIPs, Deflate64, bzip2, LZMA and other modern methods. LHA, ARJ and RAR
archives. Writing ZIPs. Any change to module loaders.
