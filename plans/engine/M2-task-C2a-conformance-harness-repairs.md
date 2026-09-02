# M2-task-C2a — Conformance harness repairs and honest reporting

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Status | Outstanding |
| Depends on | C2 (harness exists and runs) |
| Blocks | C9, C5, M2 exit |
| Parallel with | C6a |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (corpus re-run, exclusions rewritten from real results) |

## Context for a fresh agent

The C2 conformance harness compares StarPlayer's C1 per-tick trace against libxmp's
`test-dev/` frame-state dumps. It runs, it is pinned, and it is green — and the green is
not meaningful. On the current `m2` branch the corpus stands at **3 passing of 47**
(MOD 0/27, S3M 2/17, MTM 1/3) with **44 exclusions**, and `cargo xtask conformance` exits
zero because nothing is classified as a failure.

Some of those exclusions are real engine gaps (C3b and C9 own those). Several are
**harness defects that mis-diagnose working engine behaviour as a deviation**, and one
whole family of exclusion reasons cited a root cause that is factually false: eight MOD
rows blamed "libxmp uses the NTSC 8363 C-4 rate". The pinned libxmp
(`src/loaders/mod_load.c:1114-1123`, `src/load_helpers.c:302`) defaults `c4rate` to
`C4_PAL_RATE` (8287) and selects NTSC only for tracker ids ScreamTracker3 / FastTracker /
TakeTracker / ModsGrave, or when `chn > 4`. Every one of those eight fixtures is a
4-channel `M.K.` file identified as ProTracker or OpenMPT, so **libxmp ran them at PAL**,
exactly as StarPlayer does. The dumps confirm it: `ptoffset.data` advances 218 frames per
tick at period 325.3 (PAL predicts 218.1, NTSC 220.1) and `finetune.data` channel 1
advances 436 (PAL 436.1, NTSC 440.1).

Your job is the harness, not the engine. **Do not change effect behaviour in any format
processor to make a case pass.** If a case still fails after these repairs, it is a real
gap and belongs to C3b (MOD/MTM) or C9 (S3M); record it as such.

A parallel documentation pass owns `plans/product/03-accuracy-policy.md`,
`plans/reference/format-notes-mod.md` and the `conformance/` markdown; coordinate before
editing those, but `conformance/exclusions.tsv` is yours to rewrite at the end of this
task because only the re-run can say what the new reasons are.

### Running the corpus

The harness binary takes `--corpus <path to test-dev>`. A cached pinned copy exists at:

```
/home/scott/projects/starplayer-m2-c4/target/conformance/corpora/libxmp-6ec0ba21b1b28f91e22b68a51d59207c6bbf6139/test-dev
```

`cargo xtask conformance` fetches and caches it if that path is not available. Use the
cached copy while iterating; run through `cargo xtask conformance` at least once before
you finish.

## Deliverables

1. **Align traces by time, not by `(row, frame)`.**
   `crates/starplayer-testkit/src/conformance.rs:409-421` walks StarPlayer ticks forward
   until `(position.row, tick_in_row)` equals the libxmp record's `(row, frame)`, and
   returns `Err(String)` the moment it steps over a tick with an active channel. libxmp
   records carry `time_ms` but **no order index**, so a jump to another order's row 0
   mis-pairs: `openmpt-mod-pattern-jump` currently reports "trace alignment diverged at
   actual tick 6: expected row 1 frame 0, found row 0 frame 0 with active channels",
   because order 0 row 0 was paired with libxmp's order 1 row 0. libxmp's dump has active
   voices on the jump destination at positions 0..1090 that match StarPlayer's exactly.

   Convert `time_ms` to a frame (the projection already does this for the expected tick),
   find the StarPlayer tick whose **end frame** lies within the existing 45-frame `frame`
   tolerance, and then compare `row` and `tick_in_row` as **ordinary trace fields** so a
   mismatch becomes a `TraceDiff` entry. A pairing that cannot be found must also be
   reported through `TraceDiff`, never as `Err` — C2's promise of "a first-divergence
   report naming the effect" is not delivered for the most common failure class today.

2. **Make the position comparison loop-aware.**
   `conformance.rs:398` sets a flat `position` tolerance of one integer source frame
   (`1u64 << 32`) and the differ applies it linearly. libxmp's own comparator
   (`compare_mixer_data.c:78-82`) additionally accepts start/end equivalence at a loop
   boundary. `openmpt-mod-portamento-target` fails on exactly this: "first divergence at
   tick 11 channel 0 field position: expected 270582939648, actual 0" — the exact position
   is 64.003 in a 64-frame loop, StarPlayer reports 0 and libxmp 63, and `|0 - 63|` blows
   the bound. Only 3 of 384 ticks diverge in that case.

   The harness has the loaded `Module`, so look up the voice's loop span from the trace's
   `smp`/sample field and compare **circular** distance inside the loop. If plumbing the
   span through is disproportionate, accept `{loop_start, loop_end - 1, loop_end}` as
   mutually equivalent; state in a comment which you did and why.

3. **Model D18 as an adapter projection rule, not a per-case exclusion.**
   libxmp drops a voice flagged `NOTE_SAMPLE_END` *after* mixing the frame; C1 snapshots
   voice state *before* the interval. The result is a StarPlayer voice that is legitimately
   still active at the event boundary where libxmp has already emitted nothing. This is
   the mechanism behind `libxmp-mtm-tempo` ("trace alignment diverged at actual tick 393:
   expected row 16 frame 0, found row 12 frame 4 with active channels") and
   `openmpt-mod-instrument-swap-retrigger` ("diverged at actual tick 23: expected row 4
   frame 0, found row 1 frame 8"), and probably `openmpt-mod-sample-offset`.

   In `project_libxmp_dump`, when **all** of the following hold — the StarPlayer voice is
   active, libxmp has no record for that channel on that tick, the sample region is
   one-shot, and `position + step * tick_frames >= region end` — project the pair as
   matching. Comment it as the D18 projection and name the accuracy-policy entry. Then
   re-run: `libxmp-mtm-tempo` is expected to pass and its exclusion to be deleted.
   `libxmp-mtm-tempo-two` (D19, exact rational tick timing accumulating two source frames
   by tick 53) is a genuine timing-representation deviation and **stays excluded**.

4. **Add a per-field waiver column to `conformance/exclusions.tsv`.**
   `openmpt-mod-delay-break` and `openmpt-mod-vibrato-reset` are both excluded whole on a
   tick-0 `frame` mismatch alone (expected 3308, actual 882, tolerance 45) — D15, libxmp
   timestamps command tick zero at the new BPM while ProTracker first spends the old CIA
   interval. StarPlayer's CIA latch semantics are right, but excluding the entire case
   means **the effect the fixture exists to test is never compared**.

   Extend the TSV with a waiver column (for example a fourth field `waive=frame`, empty
   for ordinary exclusions) that makes the differ ignore **only that field for that case**
   and enforce every other field. Keep the format validator's "no entry without a reason"
   rule and extend it so an unknown field name in a waiver is a hard error. Convert the
   two D15 rows to waivers and report what the rest of those traces then say.

5. **A strict mode and an honest summary.**
   `crates/starplayer-testkit/src/bin/starplayer-conformance.rs:105` returns success on
   `failed == 0`, so CI is green while 15 of the 44 exclusions point at
   `conformance/known-failures.md`, whose own preamble says every record **blocks the M2
   exit**. Split the summary's `excluded` column into **accepted deviation** (the
   reference resolves into `plans/product/03-accuracy-policy.md`) and **known failure**
   (the reference resolves into `conformance/known-failures.md`), print a per-format pass
   rate, and add `--strict`, which exits non-zero if any known-failure exclusion is
   present. Wire `--strict` into `cargo xtask conformance --strict`. CI keeps running the
   non-strict form (informational) until C5, C7 and C9 land; say so in `conformance/README.md`.

6. **Derive the tick budget from the oracle's timeline.**
   `starplayer-conformance.rs:227` captures `oracle.len() + 256` ticks. libxmp emits no
   line for a tick with no mapped active voice, so the oracle length is a **lower bound**
   on the tick count, not an estimate of it. `libxmp-s3m-pattern-loop-mpt-breakjump` is
   currently excluded with the reason "trace ended before libxmp row 2 frame 0 at 1350 ms"
   — that is this truncation message, filed under `C2-S3M-001` as if it were an engine
   bug.

   Compute the budget from the oracle's **last `time_ms`**: frames needed at that time
   divided by the shortest possible tick length at the module's fastest legal BPM, plus a
   margin. Report budget exhaustion as a **harness error** that fails the run, never as a
   case result, so it can never again be mistaken for a divergence. Re-run that case and
   record its real first divergence (it belongs to C5's S3M pattern-loop dialect work, not
   to C9).

7. **Fix the note projection's saturating subtraction.**
   `conformance.rs:485-492` (`project_note`) and `:449` subtract 24 (MOD) or 12 (S3M/MTM)
   with `saturating_sub` on `u8`, so every note below the projection offset collapses to 0
   and compares equal to every other such note. Do the shift in `i16` (or another signed
   type) and carry the out-of-range case explicitly rather than clamping it into a value
   that silently matches.

8. **Fix the differ's summary counting.**
   `crates/starplayer-testkit/src/lib.rs:411` adds the *difference in tick counts* to
   `divergent_ticks`, conflating "these ticks disagree" with "one trace is longer". Report
   the length difference as its own field in the summary line and leave `divergent ticks`
   meaning what its name says.

9. **Retire the dead format gate and keep `conformance/` truthful.**
   The 2026-09-02 review commit already rewrote `conformance/README.md` (no MTM gate,
   MOD *and* MTM floor the compared position, PAL not NTSC), removed the `C4-MTM-001/002`
   sections from `conformance/known-failures.md`, and noted the `mpt-breakjump` budget
   truncation. What remains is code: `pending_integrations` is empty and the GATED branch in
   `starplayer-conformance.rs` is unreachable, so either delete it or make the summary say
   honestly that the column is always zero. After deliverables 1-8 land, re-read both
   documents and correct any sentence they have made stale.

10. **Re-run and rewrite the exclusions from real results.** This is the final deliverable
    and the one that makes the rest worth doing. Run the full corpus, then rewrite every
    remaining row of `conformance/exclusions.tsv` so its reason states what the harness
    actually observed after your repairs. Expected outcome:

    | Case | Expected after this task |
    |---|---|
    | `openmpt-mod-pattern-jump` | **passes** (deliverable 1) |
    | `openmpt-mod-portamento-target` | **passes** (deliverable 2) |
    | `openmpt-mod-instrument-swap-retrigger` | **passes** (deliverable 3) |
    | `libxmp-mtm-tempo` | **passes** (deliverable 3), exclusion deleted |
    | `openmpt-mod-delay-break`, `openmpt-mod-vibrato-reset` | remain excluded only on the `frame` field; the rest of each trace is enforced and reported |
    | `libxmp-s3m-pattern-loop-mpt-breakjump` | real first divergence recorded, tracked by C5 |
    | `openmpt-mod-sample-offset` | re-diagnosed; see research point 2 |

    Rows that survive must carry a **true** reason. In particular, delete every use of the
    "libxmp uses NTSC" wording. The three remaining finetune-family cases
    (`openmpt-mod-finetune`, `openmpt-mod-amiga-limits-finetune`,
    `openmpt-mod-instrument-swap`) are an oracle-representation deviation: libxmp derives
    finetuned periods continuously as `428 * 2^(-(note + finetune/128)/12)` (for example
    162.65, 453.45) while ProTracker uses the sixteen integer finetune tables (163, 453),
    so the position drifts about one frame per tick on finetuned samples. Point them at
    the rewritten D14. `openmpt-mod-instrument-volume` (first divergence only at tick 383)
    is the same family through libxmp's **integer** C4 rate: 8287 * 428 = 3 546 836 Hz
    against the exact PAL 3 546 895 Hz, 17 ppm. Fold it into the same entry.

    Point the six `libxmp-s3m-pattern-loop-{imf,mpt,st301}*` cases and the five
    `libxmp-mod-pattern-{jump-octalyser-break,loop-dt,loop-dt-breakjump,loop-octalyser,
    loop-octalyser-breakjump}` cases at
    `plans/engine/M2-task-C5-quirks-and-tempo-models.md` as their tracking reference: they
    are other trackers' dialects and C5 owns them. The two `libxmp-s3m-pattern-loop-st321*`
    cases stay real bugs and point at
    `plans/engine/M2-task-C9-s3m-conformance-repairs.md`.

## Research points

1. Whether the loop span is already reachable from the trace record, or whether the
   harness must consult the `Module` it loaded. Prefer the cheaper of the two; do not add
   a field to the C1 trace format for this (that would invalidate every committed
   expectation for a comparison-only concern).
2. `openmpt-mod-sample-offset` currently reports "trace alignment diverged at actual tick
   45: expected row 10 frame 0, found row 7 frame 3 with active channels". After
   deliverables 1 and 3, decide whether the remaining difference is a real `9xx`
   retained-pointer difference or the loop-gate/one-shot difference that C3b fixes. **Do
   not accept it as a deviation until it is explained**; if it is an engine gap, hand it to
   C3b with the evidence.
3. Whether `--strict` belongs on the `main` CI job now or only once C5/C7/C9 land. Default
   to informational, but make the switch a one-line change and say where it is.

## Verification

- `cargo test --workspace` passes.
- `cargo xtask conformance` runs the whole pinned corpus and prints a per-format table
  with pass, accepted-deviation, known-failure and gated counts, plus a pass rate.
- The four cases listed as "passes" above pass; the summary's total pass count rises from
  3 to at least 7.
- `cargo xtask conformance --strict` exits non-zero while known-failure exclusions remain,
  and the message names them.
- A deliberately introduced one-field difference in a MOD effect still produces a
  first-divergence report naming the tick, channel and field — now including the
  `row`/`tick_in_row` fields, which previously produced a string error. Revert it.
- An `exclusions.tsv` row with a waiver naming a field that does not exist fails the
  format validation.
- The exclusions file contains no occurrence of "NTSC".
- `cargo xtask ci --job clippy` and `--job no-std-check` stay green.

## Out of scope

Any change to a format processor, loader or mixer — this task must not move engine
behaviour. Fixing the S3M failures (C9), the ProTracker fidelity bugs (C3b), or the
tracker dialects (C5). Rewriting `plans/product/03-accuracy-policy.md`,
`plans/reference/format-notes-mod.md` or `conformance/known-failures.md` beyond the two
consistency edits named in deliverable 9. Acquiring new corpora.
