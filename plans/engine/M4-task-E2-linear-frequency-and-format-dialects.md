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

## Research resolution

Recorded at implementation time; each answer is the decision, not a summary of the
question.

### 1. Const-evaluated or literal? — **`const fn`**, for the linear-frequency table

FT2-clone's `logTab` is itself not a hand-transcribed literal: `calcMiscReplayerVars`
(`src/ft2_replayer.c` line 2909) computes it at the real player's own startup with
`logTab[i] = (uint32_t)round(16777216.0 * exp2(i * (1.0 / 768.0)))`. So "match FT2's
table" and "match the formula, `f64::exp2`-checked" are the same requirement, not two —
there is no separate literal FT2 ships that a `const fn` could disagree with at a
rounding boundary.

A fixed-point Taylor series for `exp` — `2^x = exp(x·ln2)`, evaluated as `Σ zⁿ⁄n!` in
Q0.60 `i128` arithmetic, the same widening idiom `starplayer-mixer::gain::sin_q30` uses
for the pan table's sine, just wider — was checked by brute-force search over the term
count in a throwaway script before being written into `starplayer-core`: eight terms
leaves 308 of the 768 entries wrong, the count falls fast, and by twelve terms every one
of the 768 entries matches `round(16777216 · 2^(i/768))` exactly, computed independently
in arbitrary-precision decimal. `EXP2_SERIES_TERMS` is set to fourteen, two terms of
margin, since the whole computation happens once, in the const evaluator, and costs
nothing at run time either way. `exp2_fraction_q60` in `tables.rs` is that series;
`linear_frequency_table_matches_ft2s_logtab_formula` is the required
`#[cfg(test)]`/`f64::exp2` check, over all 768 entries, and it passes. No literal table
was needed for [`LINEAR_FREQUENCY_TABLE`].

### 2. Are OpenMPT's Q16.16 slide tables derivable from the Q8.24 table? — **no; all four are literals**

Fetched `soundlib/Tables.cpp` from `OpenMPT/openmpt` (GitHub, `master`) directly, rather
than guessing at the values. Two findings, both against the actual table:

* **The derivation the research point names does not work.** `table[4n] >> 8` (truncating)
  disagrees with `LinearSlideUpTable` at 125 of its 256 entries; even the more generous
  *rounding* shift `(table[4n] + 128) >> 8` still disagrees at one (`n = 107`: 96437
  against OpenMPT's 96436). An independent per-entry Taylor-series evaluation (the same
  `exp2_fraction_q60`, called with denominator 192 instead of 768) reproduces
  `LinearSlideUpTable`, `LinearSlideDownTable` and `FineLinearSlideUpTable` exactly — so
  the shift-derivation's mismatch is double rounding (Q8.24's own rounding, then the
  shift's), not a disagreement about the underlying value — but that still is not the
  same thing as deriving the Q16.16 tables *from* the already-rounded Q8.24 table.
* **`FineLinearSlideDownTable` cannot be derived from the formula at all, by any method.**
  OpenMPT's own source comment above the table reads: "Note that there are a few errors
  in this table (typos?), but well, this table comes straight from Impulse Tracker's
  source" — and lists three: entry 0 is `65535` where `round(65536·2^0)` is `65536`
  (OpenMPT's comment guesses this is deliberate so the value fits 16 bits, and the entry
  is never read since a zero-step slide changes nothing); entry 11 is `64888` where the
  formula gives `64889`; entry 15 is `64645` where the formula gives `64655`. These are
  not OpenMPT's coding defects to fix — accuracy policy §0 already names "the format
  specifications and OpenMPT's documented compatibility behaviour" as the XM/IT
  reference, and this table *is* that behaviour, verbatim from real Impulse Tracker.
  Reproducing the typos is therefore the canonical choice, not a deviation from it, and
  no `const fn` — a Taylor series or otherwise — can produce a wrong answer on purpose.

So the research point's own instruction applies directly: ship OpenMPT's literals as
their own tables. All **four** are literal `[u32; N]` arrays transcribed from
`Tables.cpp` (`LinearSlideUpTable` lines 540–579, `LinearSlideDownTable` lines 583–612,
`FineLinearSlideUpTable` lines 516–521, `FineLinearSlideDownTable` lines 524–534) — not
three computed and one pasted — so all four are sourced and auditable the same way, and
`linear_slide_tables_match_the_formula_except_where_it_documents_a_typo` checks every
entry of all four against `f64::exp2`, pinning the three typo'd entries by value so a
future edit cannot silently "correct" them back to the formula.

### 3. XM dialect evidence — recorded from `Load_xm.cpp`'s `madeWith` classification

Fetched `soundlib/Load_xm.cpp` from `OpenMPT/openmpt` (GitHub, `master`). The tracker
names/markers it distinguishes for XM, and what StarPlayer's four new variants map to:

* **`FastTracker v2.00   `** (20 bytes, space-padded) with header size 276 — genuine FT2
  or a clone claiming to be one. OpenMPT further splits this by format version
  (`< 0x0104` is "FT2 generic, confirmed") and by null-padding patterns in the song name
  into "FT2 generic" versus "FT2 clone" versus PlayerPRO (lines 619–644) — PlayerPRO in
  particular disguises itself this way rather than writing its own tracker name.
  StarPlayer's `FastTracker2` keeps all of these as one variant; the behaviours OpenMPT
  keys off the split are FT2-specific volume-ramping timing (`kFT2VolumeRamping`,
  version-gated) and mix-level compatibility (`MixLevels::CompatibleFT2`), neither of
  which StarPlayer models yet.
* **`FastTracker v 2.00  `** (note the extra mid-string space) — ModPlug Tracker 1.x
  before it wrote its own name, disguising itself as FT2 with a deliberate typo
  (line 645, `verOldModPlug`). StarPlayer's `ModPlugXm`.
* **`MilkyTracker `** prefix (line 659) — `MilkyTracker`. Versions before 0.90.87 leave
  the rest of the field blank; from 0.90.87 on it also matches FT2's panning scheme,
  which is the playback behaviour the split exists to key off (not yet modelled).
* **`OpenMPT `** prefix (line 656) — `OpenMptXm`. Also covers ModPlug Tracker 1.17+,
  which shares OpenMPT's save code from that version on; OpenMPT keys its own mix-level
  choice (`Compatible` vs `CompatibleFT2`) off the embedded version number here.
* Other names it recognises but StarPlayer does not carry a variant for yet: `NitroTracker`
  (song-name based, line 242), `Fasttracker II clone` (an explicit FT2-clone-confirmed
  marker, line 667), `MadTracker 2.0` (line 671, further split registered/unregistered),
  `Skale Tracker` / `Sk@le Tracker` (line 682), `*Converted …-File*` (Digitrakker
  conversions, line 687). All fall to `FormatDialect::Unknown` (canonical) until an XM
  corpus case asks for one of them by name.
* Behaviours OpenMPT gates on `madeWith`, beyond the mix-level/ramping ones above:
  `kFT2PortaNoNote`, `kFT2Arpeggio` and `kFT2ST3OffsetOutOfRange` are reset (disabled) for
  MilkyTracker even though it claims FT2 compatibility, and `verEmptyOrders` controls
  whether an empty order list is accepted (FT2 itself just plays pattern 0) — none of
  these are `QuirkSet` fields today, per the "no new field without a corpus case" rule;
  this list is what F5 should check the pinned XM corpus against first.

### IT dialect detection: a correction to the task's own text

Deliverable 2 states `ImpulseTracker — Cwt/v in 0x1000..=0x1FFF from IT itself`. Fetching
`soundlib/Load_it.cpp` (`OpenMPT/openmpt`, `master`) for the `OpenMptIt` / `SchismTracker`
/ `ModPlugIt` detection rules the same deliverable asks for turned up that this range is
wrong — `Load_it.cpp`'s own classifier switches on `cwtv >> 12` (line 1221) and case `1`,
covering exactly `0x1000..=0x1FFF`, is **Schism Tracker** (`GetSchismTrackerVersion`,
lines 365–392); case `0`, `0x0000..=0x0FFF`, is genuine Impulse Tracker
(`GetImpulseTrackerVersion`, lines 344–361) and a handful of small clones OpenMPT
disambiguates by exact `cwtv`/`cmwt`/`reserved` combinations within that same nibble. The
two variants' ranges in the task text are swapped relative to the actual source. Since
deliverable 2 explicitly defers three of the four IT variants to "as OpenMPT's
`Load_it.cpp` classifies them," the corrected, source-verified mapping was implemented
for all four rather than the task text's literal (and here incorrect) range:

* `ImpulseTracker` — `Cwt/v` high nibble `0` (`0x0000..=0x0FFF`).
* `SchismTracker` — `Cwt/v` high nibble `1` (`0x1000..=0x1FFF`).
* `OpenMptIt` — `Cwt/v` `0x5000..=0x5FFF` with reserved field `OMPT`, or the `0x0888`
  markers OpenMPT 1.17 wrote before that scheme (lines 469–520).
* `ModPlugIt` — the `0x5000` nibble *without* the `OMPT` marker, or one of several
  specific early `cwtv`/`cmwt`/`reserved` combinations for ModPlug 1.09–1.16
  (lines 503–520, 727–745).

Every doc comment on the eight new `FormatDialect` variants cites the `Load_xm.cpp` /
`Load_it.cpp` line numbers this research was read from, so F1/G1 can re-check them
directly against the source rather than against this summary.

## Out of scope

Any consumer of the table. XM's Amiga-mode finetune period table (it belongs in
`starplayer-xm`). `TempoModel::ItModern` (G4). Any new `QuirkSet` field.
