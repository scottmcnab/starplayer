# M4 — E2: Linear-frequency table and the XM/IT format dialects

| Field | Value |
|---|---|
| Milestone | M4-lite ([master plan](M4-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | M2 complete |
| Blocks | F2 (XM runtime), G3 (IT runtime) |
| Parallel with | E1, D6, D7 |
| Recommended model | GPT-5.6-sol or Claude Sonnet (core tables and an enum; no RT path) |
| Verified by | agent (`cargo test --workspace`, `cargo xtask goldens --check`, `cargo xtask ci --job no-std-check`, `--job clippy`), then reviewer |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there are non-negotiable, and design goal 5 in particular: **no
transcendental functions in the real-time path; tables only**, so the fixed-point path is
bit-identical on x86, ARM and WASM.

MOD, S3M and MTM derive pitch from Amiga periods. XM in "linear frequency" mode and IT's
linear slides instead work in 1/64-semitone (XM) or 1/768-octave (IT) units and need
`2^(x)` for fractional `x`. Both formats are about to be implemented concurrently (M5 and
M6), so the table they share lands here, once, in `starplayer-core`.

The same is true of `FormatDialect`: it is one enum in `starplayer-core`, and XM and IT
both need variants. Adding them in one place now takes the merge conflict once. **No new
`QuirkSet` fields are added here**: task C5 established that every field must name an
accuracy-policy entry and be observed by a test, and no XM/IT corpus case has been run
yet. The variants all map to canonical behaviour until F5/G4/G6 name a quirk.

### Code you must read before changing anything

- `crates/starplayer-core/src/tables.rs` — how the existing tables are documented and
  tested; `ST3_FREQUENCY_NUMERATOR`'s docs are the standard for a constant's provenance.
- `crates/starplayer-mixer/src/gain.rs` — the `const fn` Taylor-series precedent for
  building a table without libm. Read it to decide research point 1.
- `crates/starplayer-core/src/quirks.rs` — `QuirkSet`, `FormatDialect`,
  `FormatDialect::quirks`, `QuirkSelection`, and the test that pins canonical versus
  classic profiles.
- `crates/starplayer-core/src/note.rs` — `Note`, `Period`.
- `plans/engine/complete/M2-task-C5-quirks-and-tempo-models.md` — the rules for quirks and
  dialects ("a quirk that cannot be expressed as a `QuirkSet` field is a format
  difference").
- `plans/product/03-accuracy-policy.md` §2 *Tracker dialects*.

### The reference tables

- FT2 (`ft2-clone`, `ft2_replayer.c`): `logTab[768]` with
  `logTab[i] = round(16777216 · 2^(i/768))`, i.e. `2^(i/768)` in **Q8.24**, read as
  `logTab[period % 768] >> ((14 − period / 768) & 31)` to get an integer Hz.
- OpenMPT (`Tables.cpp`): `LinearSlideUpTable[256]` = `65536 · 2^(i/192)`,
  `LinearSlideDownTable[256]` = `65536 · 2^(−i/192)`, `FineLinearSlideUpTable[16]` =
  `65536 · 2^(i/768)`, `FineLinearSlideDownTable[16]` = `65536 · 2^(−i/768)`, all rounded
  independently.

## Deliverables

### 1. `starplayer_core::tables::LINEAR_FREQUENCY_TABLE: [u32; 768]`

`2^(i/768)` in Q8.24 for `i` in `0..768`, entry-for-entry equal to FT2's `logTab`. Provide
`pub const fn linear_frequency_q24(units: u32) -> u32` that splits `units` into
`(octave, index)` and shifts, so the XM crate does not repeat the `>> ((14 − …) & 31)`
idiom by hand.

Also provide the IT slide helpers as thin table reads: `linear_slide_up_q16(steps: u8) -> u32`
(`2^(steps/192)` in Q16.16, `steps` in `0..=255`) and `fine_linear_slide_up_q16(steps: u8)`
(`2^(steps/768)`, `0..=15`), and their `_down_` counterparts. **Research point 2 decides
whether these are derived from the 768-entry table or are their own literal tables.**

### 2. XM and IT `FormatDialect` variants

Add, with detection rules in the docs (the loaders implement them in F1/G1):

- `FastTracker2` — tracker name `FastTracker v2.00` and version `0x0104`;
- `MilkyTracker`, `ModPlugXm` (ModPlug Tracker 1.x), `OpenMptXm` — from the tracker name;
- `ImpulseTracker` — `Cwt/v` in `0x1000..=0x1FFF` from IT itself;
- `OpenMptIt`, `SchismTracker`, `ModPlugIt` — from `Cwt/v`, `Cmwt` and the reserved
  fields as OpenMPT's `Load_it.cpp` classifies them;
- keep `Unknown` for anything else.

`FormatDialect::quirks` maps every new variant to `QuirkSet::profile_default()`. Add a doc
comment on the enum saying that XM/IT fields arrive with the corpus case that needs them.

### 3. Documentation

`plans/product/03-accuracy-policy.md` §2 *Tracker dialects*: one sentence noting the XM/IT
dialects exist and map to canonical behaviour pending M5/M6 corpus evidence.

## Research points

1. **Const-evaluated or literal?** `gain.rs` builds its sine table with a `const fn`
   Taylor series. A `2^(x)` series in Q8.24 must reproduce FT2's *rounded* values exactly
   at every one of 768 entries, including the ones that sit on a rounding boundary. Decide
   between a `const fn` and a literal table, and either way add a `#[cfg(test)]` (std,
   `f64::exp2`) test that checks every entry against `round(16777216 · 2^(i/768))`. If a
   `const fn` cannot hit every entry, use literals and say why in the docs.
2. **Are OpenMPT's Q16.16 slide tables derivable from the Q8.24 table?** `table[4i] >> 8`
   truncates where OpenMPT rounds. Check all 256 + 16 entries in a test; if any differ,
   ship OpenMPT's literals as their own tables so IT matches its reference exactly.
3. **XM dialect evidence.** Read OpenMPT `Load_xm.cpp`'s `madeWith` classification and
   record which tracker names it distinguishes and which playback behaviours it keys off
   them, so F3/F5 know what a dialect may later need to carry.

## Verification

```sh
cargo test --workspace
cargo xtask goldens --check
cargo xtask ci --job no-std-check
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
```

The `canonical_and_classic_differ_only_where_the_policy_says_so` test must still pass
unchanged: no `QuirkSet` field was added.

Report the exact commands run and their results. **Do not commit** — the reviewer commits.

## Out of scope

Any consumer of the table. XM's Amiga-mode finetune period table (it belongs in
`starplayer-xm`). `TempoModel::ItModern` (G4). Any new `QuirkSet` field.
