# Fixture trim — keep REFLEX and PETRI, remove ARMANI, MOVEMENT and NICETUNE

| Field | Value |
|---|---|
| Milestone | Repository hygiene before going public |
| Status | In progress 2026-09-07 |
| Depends on | — |
| Blocks | Making the repository public (`ARMANI.S3M` must also be purged from git history, a separate owner step) |
| Recommended model | Claude Opus |
| Verified by | agent (full `cargo xtask ci` job set + goldens check + web harness) |

## Context for a fresh agent

`crates/starplayer-s3m/tests/fixtures/` holds five S3M modules used as loader, effect,
golden, perceptual and web-player fixtures. The owner has decided to keep only
**`REFLEX.S3M`** and **`PETRI.S3M`**:

- `ARMANI.S3M` was **not written by the owner** and cannot be in a public repository.
- `MOVEMENT.S3M` (58 KB) is one pattern, two channels, one effect type; its only unique
  property, the mono master-volume bit, is already covered by the synthetic
  `a_mono_module_gets_the_empty_pan_table_that_means_centred` test in `tests/fixtures.rs`.
- `NICETUNE.S3M` (54 KB) declares 16 channels but plays notes on only 7; its unique
  loader coverage (a 16-entry channel-settings table, Amiga-limits flag) is header-only.

The repository is meant to stay compact; do not add any replacement module. Every
reference to the three files must go, and every test that used one of them as a generic
"some real module" must switch to `REFLEX.S3M` or `PETRI.S3M`. Both of those end by
running out of order list (`EndReason::Ended`), which is what the NICETUNE-based tests
assert.

`plans/` archives (`plans/engine/complete/*.md`) that quote measurements from the removed
files are historical records: **leave them untouched**. The DOS source trees `STARPLAY/`
and `STARPLAY-2.25s/` are read-only.

## Deliverables

1. **Delete** `crates/starplayer-s3m/tests/fixtures/{ARMANI,MOVEMENT,NICETUNE}.S3M` and
   `goldens/s3m/{armani,movement,nicetune}__i16_mono_44100_linear.sha256` with `git rm`.
2. **`crates/starplayer-s3m/tests/fixtures.rs`**: remove the three `include_bytes!`
   constants and their `Expectation` blocks. Add one synthetic test using the existing
   `synthetic_s3m(stereo, settings, pan_block)` helper with a full 16-entry channel-settings
   table (`[0, 8, 1, 9, 2, 10, 3, 11, 4, 12, 5, 13, 6, 14, 7, 15]`, the layout NICETUNE
   had) asserting 16 derived pan nibbles alternate left (3) / right (12). If the helper
   cannot take 16 entries, extend it minimally.
3. **`crates/starplayer-s3m/tests/effects.rs`**: replace the MOVEMENT determinism test
   with the same assertion over `PETRI.S3M` (rename accordingly).
4. **`crates/starplayer-offline/src/lib.rs`**: trim `GOLDEN_CORPUS` to REFLEX and PETRI.
   Re-target `nicetune_ends_when_its_order_list_runs_out_at_the_length_it_always_had` at
   `PETRI.S3M`: keep the `EndReason::Ended` and `loop_length_frames() == None` assertions,
   re-measure the end frame and duration and pin the new numbers, and rewrite the doc
   comment so it no longer claims to be "the number the build before D2 reported" — say
   instead that it pins the length measured on 2026-09-07 when the fixture set was trimmed.
   Any other reference to the removed files in this crate goes too.
5. **`crates/starplayer-offline/src/bin/starplayer-goldens.rs`**,
   **`crates/starplayer-testkit/src/bin/starplayer-perceptual/main.rs`**,
   **`apps/starplayer-cli/tests/golden_reproduction.rs`**: drop the three entries.
6. **`apps/starplayer-cli/src/render.rs`** module doc (~line 22): the paragraph explaining
   why `MOVEMENT.S3M`'s 9.32 s pass does not reproduce its golden is now about a file that
   no longer exists. Rewrite it to make the same point generically (a song whose own pass
   is shorter than ten seconds cannot be reproduced through the general knobs) without
   naming a removed fixture. Keep the reasoning; do not change behaviour.
7. **`crates/starplayer-host/tests/{callback_allocation,live_input,player}.rs`**: switch
   `FIXTURE` to `PETRI.S3M`; fix the one assertion message naming NICETUNE.
8. **`xtask/src/main.rs`** `copy_fixture_modules` (~line 2200): the packaged list becomes
   `["PETRI.S3M", "REFLEX.S3M"]`; fix the "five licensed test modules" doc comment.
9. **Web player**: `apps/starplayer-web/test/headless.mjs` `SECOND_MODULE` and
   `apps/starplayer-web/test/worklet-harness.mjs` `secondFixture` become `PETRI.S3M`
   (and any assertion text that names MOVEMENT). `apps/starplayer-web/README.md`'s fixture
   sentence lists `PETRI` and `REFLEX` only.
10. **`crates/starplayer-s3m/tests/fixtures/README.md`**: rewrite for two files. Keep the
    provenance statement (owner-authored, 1994–96, Scream Tracker 3), the deferred-licence
    paragraph, and the table rows for REFLEX and PETRI. Drop "the five smallest" wording.
    Add one sentence: the set was trimmed on 2026-09-07 to keep the repository compact;
    header-level cases the removed files covered (mono flag, 16-channel settings table) live
    in synthetic tests in `fixtures.rs`.
11. Search the whole tree (excluding `target/`, `.git/`, `plans/`, `STARPLAY*/`,
    `apps/starplayer-web/dist/`) for `ARMANI|MOVEMENT|NICETUNE|armani|movement|nicetune`
    case-sensitively where it means the fixture (ignore the English word "movement" in
    `starplayer-dsp/src/ramp.rs`, `starplayer-mixer/src/gain.rs`, `voice.rs`) and confirm
    nothing remains.

## Verification

Run from the worktree root; all must pass:

```text
cargo test -p starplayer-s3m
cargo test -p starplayer-offline
cargo test -p starplayer-host
cargo test -p starplayer-cli
cargo xtask goldens --check
cargo xtask ci --job host-tests
cargo xtask ci --job goldens
cargo xtask ci --job clippy
cargo xtask wasm && node apps/starplayer-web/test/worklet-harness.mjs && node apps/starplayer-web/test/headless.mjs
```

`cargo xtask goldens --check` must pass **without** regenerating any golden: the two
remaining hashes are unchanged by this task. If it reports a mismatch, stop and report
rather than regenerating. The headless harness can flake in `sab` mode under load; rerun
once before treating a failure as real.

## Out of scope

- Purging `ARMANI.S3M` from git history (`git filter-repo`) — an owner step after merge.
- Adding any new module fixture.
- Editing archived plans under `plans/*/complete/`.
