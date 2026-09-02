# M2-task-C9 — S3M conformance repairs (the M1 effect bugs the corpus found)

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Status | Outstanding |
| Depends on | C2a (harness repairs; the diagnoses below must be re-confirmed after it) |
| Blocks | M2 exit |
| Parallel with | C3b, C6a |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (conformance corpus, each case named below passing or re-diagnosed) |

## Context for a fresh agent

S3M was implemented in M1 (`crates/starplayer-s3m/`), before any oracle existed. M2's
conformance harness now runs it against libxmp's `test-dev/` dumps and OpenMPT's
compatibility fixtures, and finds **nine real S3M engine bugs**. No task owned them until
this one: they were filed in `conformance/known-failures.md` as `C2-S3M-001` through
`C2-S3M-008` and then left. The corpus currently stands at **S3M 2 passing of 17**.

**The reference is Scream Tracker 3.21.** The original DOS assembly in `STARPLAY-2.25s/`
is byte-identical to ST3 in its effect core, and
`plans/reference/original-s3mlib-analysis.md` is the written specification derived from
it — read §4 before touching an effect. The pinned libxmp is the executable oracle, and
for these cases it is running in **ST3.21 flow mode** (libxmp selects a flow mode from the
S3M `cwtv` field at `src/loaders/s3m_load.c:390-432`), so its expectations here **are**
ST3.21's behaviour, not a libxmp choice.

**Out of scope, and this matters:** six sibling pattern-loop cases —
`libxmp-s3m-pattern-loop-imf`, `-imf-breakjump`, `-mpt`, `-mpt-breakjump`, `-st301`,
`-st301-breakjump` — encode **other trackers' dialects** (Imago Orpheus, ModPlug 1.16 and
Scream Tracker 3.01 respectively). They are not ST3.21 bugs and they are not yours; C5
owns them through its `FormatDialect`/`QuirkSet` work. `conformance/known-failures.md`'s
`C2-S3M-001` currently lumps all eight together and asks to "resolve … for each
compatibility mode", which is not a policy StarPlayer has. Reduce that record to the two
ST3.21 cases as part of this task.

Run the corpus with the harness binary's `--corpus <path to test-dev>`; a pinned cached
copy is at
`/home/scott/projects/starplayer-m2-c4/target/conformance/corpora/libxmp-6ec0ba21b1b28f91e22b68a51d59207c6bbf6139/test-dev`,
or `cargo xtask conformance` fetches it.

**Re-confirm every first divergence below after C2a lands.** C2a fixes an aligner that
mis-pairs across order jumps, a position comparison that is not loop-aware, and a tick
budget that truncates long captures — three of the messages quoted below are alignment
messages and may change shape (though not, for these cases, their underlying cause: these
are control-flow and pitch-domain differences, not comparator artefacts). If a case turns
out to be a comparator artefact after all, say so and close it rather than changing the
engine.

## Deliverables

Each item below quotes its **current first divergence verbatim** from the pinned run on
`m2`. Fix the engine, add the named regression test, and report the case's new status.

1. **`libxmp-s3m-pattern-loop-st321` — ST3.21 pattern loop restarts row 0.**
   > trace alignment diverged at actual tick 30: expected row 2 frame 0, found row 0
   > frame 0 with active channels

   StarPlayer restarts at row 0 where ST3.21 **continues the loop** and reaches row 2.
   Resolve `SBx`'s loop-start latch and loop-counter semantics against ST3.21: which
   channel's counter governs, when the start row is set versus reused, and what happens on
   the final iteration.
   *Test:* a fixture asserting the exact row sequence produced by an `SB0`/`SBn` pair.

2. **`libxmp-s3m-pattern-loop-st321-breakjump` — same, with a break/jump on the loop row.**
   > trace alignment diverged at actual tick 24: expected row 2 frame 0, found row 0
   > frame 0 with active channels

   StarPlayer restarts row 0 instead of following the ST3.21 break/jump. Resolve the
   same-row interaction between `SBx` and `Bxx`/`Cxx`: which wins, and in what channel
   order. This is the same root as item 1 and probably fixed with it — but assert both.
   *Test:* row sequence for `SBx` co-resident with `Cxx` on one row, and with `Bxx`.

3. **`openmpt-s3m-parameter-memory` — effect memory sharing and continuation.**
   > trace alignment diverged at actual tick 100: expected row 21 frame 0, found row 16
   > frame 4 with active channels

   The active row/frame sequence has already diverged by row 16. ST3 shares one parameter
   memory across several effect families and continues an effect with a zero parameter in
   specific cases only. Resolve which effects share memory, whether the memory is written
   on a zero parameter, and whether a memory is per channel or per effect.
   (`known-failures.md` §C2-S3M-004.)
   *Test:* a table-driven test, one row per effect family, asserting the parameter used on
   a following zero-parameter row.

4. **`openmpt-s3m-pattern-delays-retrigger` — row delay tick numbering.**
   > trace alignment diverged at actual tick 12: expected row 1 frame 0, found row 0
   > frame 12 with active channels

   StarPlayer is still on row 0 at frame 12 where the oracle has advanced to row 1
   frame 0. Resolve first-tick processing and tick numbering across `S6x` row-delay
   repetitions, including how retrigger (`Qxy`) interacts with a delayed row.
   (§C2-S3M-005.)
   *Test:* assert the tick-in-row sequence and the retrigger points across an `S6x`
   repetition.

5. **`openmpt-s3m-portamento-sample-change` — instrument latch during tone portamento.**
   > first divergence at tick 6 channel 0 field instrument: expected 1, actual 2
   > (tolerance 0)
   > summary: 378 divergent tick(s), 668 divergent channel record(s); expected 384

   A `Gxx` row naming instrument 2 changes StarPlayer's sounding instrument identity from
   1 to 2; ST3 keeps voice instrument 1 and only adopts the new instrument's **volume**.
   Resolve ST3's instrument/sample latching during tone portamento. (§C2-S3M-007.)
   *Test:* after `Gxx` with a new instrument number, the sounding sample is unchanged and
   the channel volume is the new instrument's default.

6. **`libxmp-s3m-sample-portamento` — portamento continuation after an instrument change.**
   > trace alignment diverged at actual tick 21: expected row 16 frame 0, found row 3
   > frame 3 with active channels

   StarPlayer leaves a voice active on row 3 where the oracle's next active record is
   row 16 — a voice-lifetime difference, so the sample being played (or its loop state) is
   wrong after the change. Resolve tone-portamento continuation after an instrument change
   and the voice lifetime it produces. Closely related to item 5; fix them together but
   assert them separately. (§C2-S3M-008.)
   *Test:* voice lifetime and sample identity across an instrument-changing `Gxx`.

7. **`openmpt-s3m-amiga-limits` — Amiga-limited period and step.**
   > first divergence at tick 1 channel 0 field position: expected 236223201280, actual
   > 240861765150 (tolerance 4294967296)
   > summary: 370 divergent tick(s), 739 divergent channel record(s); expected 384

   Tick 0 agrees; from tick 1 the sample advances at a different rate after clamping, and
   the traced period is also one native unit low. Resolve the S3M Amiga-limit period
   derivation and the step it produces — clamp bounds, the order of clamp versus finetune,
   and rounding. (§C2-S3M-002.)
   *Test:* period and step at the clamp bounds, asserted as exact values.

8. **`openmpt-s3m-frequency-limits` — the high-frequency voice-stop boundary.**
   > trace alignment diverged at actual tick 32: expected row 16 frame 0, found row 5
   > frame 2 with active channels

   A StarPlayer voice remains active above ST3's high-frequency cutoff, where ST3 stops
   it. Resolve the boundary condition and whether it stops the voice or merely clamps.
   (§C2-S3M-003.)
   *Test:* a voice driven above the cutoff stops, and one just below it does not.

9. **`openmpt-s3m-period-limit` — lower period limit and note identity.**
   > first divergence at tick 0 channel 0 field note: expected 101, actual 66
   > (tolerance 0)
   > summary: 384 divergent tick(s), 768 divergent channel record(s); expected 384

   Every tick diverges, from the very first: StarPlayer reports note F#5 where the libxmp
   mixer voice reports F-8. Resolve the ST3 **lower** output-period limit, the note
   identity reported at that limit, and the zero-cut boundary. Note that the note field is
   a projection of the mixer's pitch, so this is a pitch-domain bug, not a display bug.
   (§C2-S3M-006.) Check the projection saturation fix from C2a deliverable 7 has landed
   before concluding the magnitude.
   *Test:* the derived period and reported note at and below the limit.

10. **Update `conformance/known-failures.md` and `conformance/exclusions.tsv`.** Reduce
    `C2-S3M-001` to the two ST3.21 cases, delete each record you have fixed, and repoint
    the six dialect cases at `plans/engine/M2-task-C5-quirks-and-tempo-models.md`. Every
    case that still fails keeps a record whose text is the **new** first divergence, not
    this task file's.

11. **Record any deviation you deliberately keep** in `plans/product/03-accuracy-policy.md`
    §3, in the same commit as the code. A case that cannot be made to agree because ST3.21
    itself is defective is a deviation with a policy entry, not a silent exclusion.

## Research points

1. Whether items 1 and 2 share a single root cause in the `SBx` handler, and whether the
   fix also moves the six C5 dialect cases (it should not — if it does, the dialect
   selection is doing work that belongs to the shared path, and C5 needs to know).
2. Whether items 5 and 6 are one bug. The evidence suggests one latch decision produces
   both the wrong instrument identity and the wrong voice lifetime.
3. Whether items 7, 8 and 9 share a single period-domain clamp helper. Three separate
   clamp bugs in the same crate is unlikely; one helper with the wrong bounds is likely.
4. What `STARPLAY-2.25s/` does at each of these decision points, and whether it agrees with
   ST3.21. Where the original is defective, the canonical behaviour wins (CLAUDE.md design
   goal 6) and the deviation goes in the policy.

## Verification

- Every case named above either passes, or carries a re-diagnosed record whose reason is
  its **new** first divergence and whose owner is named.
- `cargo xtask conformance` reports an S3M pass count of at least 11 of 17 (the six
  dialect cases remain C5's).
- `cargo xtask conformance --strict` (C2a deliverable 5) exits zero for S3M, or names
  exactly the records that remain and why.
- A regression test per numbered item, asserting exact values or an exact row/tick
  sequence — not merely "does not panic".
- `cargo test --workspace`, `cargo xtask ci --job clippy`, `--job no-std-check`.
- `cargo xtask goldens --check`: the S3M goldens **will** move if these fixes change
  audio, which several of them must. Regenerate them **in the same commit**, and state in
  the commit message which fixture hashes changed and which deliverable changed them. A
  golden regeneration without that statement is not acceptable.
- The buffer-size-independence test still holds at block sizes 1, 3, 64, 128, 4096, 8191.

## Out of scope

The six non-ST3.21 pattern-loop dialect cases (C5). MOD and MTM behaviour (C3b). Harness
comparator repairs (C2a) — if you find another comparator artefact, report it to C2a and
leave the engine alone. New S3M features; this task only makes existing effects correct.
