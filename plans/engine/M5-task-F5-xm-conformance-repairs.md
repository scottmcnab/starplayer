# M5 — F5: XM conformance repairs

| Field | Value |
|---|---|
| Milestone | M5 ([master plan](M5-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready — dispatch after G3 lands (one worker at a time) |
| Depends on | F2 landed (72 of 93 XM cases pass; 20 recorded as `F2-XM-001`..`F2-XM-012` in `conformance/known-failures.md`) |
| Blocks | M5 exit (`--strict` must name no XM known failure that is not a documented dialect gap) |
| Parallel with | G3 (different crates), the G6 IT repairs |
| Recommended model | Claude Opus (accuracy work against two disagreeing references) |
| Verified by | agent (`cargo xtask conformance --offline --strict` reporting only the pre-existing `C2-S3M-009` and any XM entry this task converts into a documented dialect gap), then reviewer, then the owner listens |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first.

Task F2 landed FastTracker 2 playback in `crates/starplayer-xm` and wired 93 XM cases
into the conformance harness: 72 pass (46 of them with a per-field waiver naming an
accuracy-policy entry D42–D47), one is an accepted deviation, and **20 are executed every
run and recorded as known failures** `F2-XM-001`..`F2-XM-012` in
`conformance/known-failures.md`. This task is to F2 what M2-C9 was to B4: settle each of
the twelve entries by fixing the processor, by recording a deviation with an
accuracy-policy entry (D48 onward), or by turning a tracker-specific behaviour into a
`FormatDialect`/`QuirkSet` field the way C5 did — never by hiding a case.

The references are unchanged: ft2-clone's `ft2_replayer.c` is the specification of what
FastTracker 2 does; OpenMPT's `Snd_fx.cpp`/`Sndfile.h` (`kFT2*`) and its wiki name the
compatibility behaviours; libxmp's dumps are the oracle, and where libxmp follows OpenMPT
rather than FT2 the case is a deviation, not a bug. Read
`plans/engine/complete/M5-task-F2-xm-playback.md` (its research resolutions, especially 2
and 5), `plans/engine/complete/M2-task-C9-s3m-conformance-repairs.md` (the shape of a
repair pass) and `plans/engine/complete/M2-task-C5-quirks-and-tempo-models.md` (how a
dialect becomes a `QuirkSet` field: a policy entry, a detection rule, a test that flips
exactly the cases that name it).

## Deliverables

1. **Each `F2-XM-*` entry settled**, in this order of preference: a processor fix that
   makes the case pass with no waiver; a waiver with a D48+ policy entry citing both
   sources by file and line; a `FormatDialect` variant / `QuirkSet` field for a
   tracker-specific behaviour (`F2-XM-001` ModPlug/MadTracker/rst tone portamento,
   `F2-XM-002` ModPlug/Skale pattern flow, `F2-XM-007` Skale offset) detected from the
   tracker name the loader already classifies. An entry that cannot be settled against a
   real FastTracker 2 (F2 named `F2-XM-003`) stays a known failure with the reason
   sharpened and a note of what evidence would settle it.
2. **`F2-XM-012`** (a zero-byte oracle in the pinned corpus): confirm it is the corpus,
   not the harness, and make it an accepted harness record like `C2-MOD-001`.
3. `--strict` output: only `C2-S3M-009` plus whatever this task explicitly leaves as a
   sharpened known failure.
4. No change to the MOD/S3M/MTM results (33 of 47) or to any golden except the XM one if
   a fix legitimately changes the synthetic fixture's render (then regenerate it and say
   why in the commit).
5. Docs: policy §1/§3 updated, `known-failures.md` pruned, `plans/README.md` M5 row,
   `M5-master-plan.md` exit criteria.

## Research points

1. For each dialect candidate, confirm from `Load_xm.cpp` what OpenMPT keys the
   behaviour on (tracker name, version, both) and reuse E2's `FormatDialect` variants.
2. `F2-XM-009` (`kFT2LoopE60Restart`, the stale break position): decide whether it is
   FT2 behaviour the canonical profile should reproduce (it is FT2's) or a bug the policy
   records — the §1 "deliberate quirks reproduced" test applies.
3. `F2-XM-008` vibrato ramp amplitude and `F2-XM-011` the looping envelope after 240
   ticks: check ft2-clone line by line before touching the processor.

## Verification

```sh
cargo test -p starplayer-xm
cargo xtask conformance --offline
cargo xtask conformance --offline --strict      # report exactly what it still names
cargo xtask goldens --check
cargo test --workspace
cargo xtask ci --job rt-safety
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
```

Report the before/after XM table, every entry's disposition, and every policy entry or
quirk field added. **Do not commit** — the reviewer commits.

## Out of scope

IT (G3/G6). New XM features. Changing tolerances the D42/D44 arithmetic does not justify.
