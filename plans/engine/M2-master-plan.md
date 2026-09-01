# M2 — MOD + MTM native, plus the accuracy machinery

| Field | Value |
|---|---|
| Goal | Three formats, and **proof** that all three are right |
| Estimate | 2u |
| Depends on | M1 |
| Blocks | M3, M5 |
| Foundation docs | [03-accuracy-policy](../product/03-accuracy-policy.md), [original-s3mlib-analysis](../reference/original-s3mlib-analysis.md) |

## Why this milestone exists

Two things arrive together, deliberately.

**MOD and MTM get native effect processors.** The original converted both to S3M in
memory before the player saw them, and that is precisely why its MOD playback was
inaccurate (`plans/reference/original-s3mlib-analysis.md` §1). We do not repeat it
(`plans/product/00-vision.md` decision 4). What we *do* take from the original is its
conversion tables, read as MOD and MTM **semantics** rather than as a lowering step: the
two finetune tables, the LRRL channel map, the loop-length > 4 gate, the Amiga-limits
derivation, and the sign conventions.

**The accuracy machinery lands now rather than in M1.** Building a conformance harness
before there is anything to test spends a month and produces silence; building it with
only one format gives it nothing to compare. With three formats and an oracle it earns
its keep immediately — and libxmp's `test-dev/` suite turns out to ship *frame-by-frame
expected channel state*, which is exactly the shape of our trace format.

## Tasks

| Task | Summary | Depends on | Parallel with |
|---|---|---|---|
| [C1](complete/M2-task-C1-trace-format.md) ✅ | Per-tick state trace format and the differ | M1 | C6 |
| [C2](complete/M2-task-C2-conformance-harness.md) ✅ | libxmp `test-dev/` + OpenMPT corpora in CI | C1 | C3, C4 |
| [C3](M2-task-C3-mod-loader-and-effects.md) | MOD: loader + ProTracker effect processor | M1 | C4 |
| [C4](M2-task-C4-mtm-loader-and-effects.md) | MTM: loader + effect processor | M1 | C3 |
| [C5](M2-task-C5-quirks-and-tempo-models.md) | `QuirkSet` and `TempoModel` wired end to end | C3 | C6 |
| [C6](complete/M2-task-C6-fixed-point-mixer.md) ✅ | The fixed-point mixer — the bit-exact reference | M1 | C1, C3, C4 |
| [C7](M2-task-C7-fuzzing-and-rt-safety.md) | Loader fuzzing + the allocator hook in CI | C3, C4 | C5 |
| [C8](M2-task-C8-dos-reference-harness.md) | **Deferred**, pull-driven — see its trigger | — | — |

C8 is deliberately not scheduled. It has an explicit trigger and stays in the backlog
until that trigger fires.

## Exit criteria

1. MOD and MTM load and play natively, without any S3M conversion path.
2. `cargo xtask goldens` regenerates golden hashes; CI verifies them.
3. The libxmp `test-dev/` corpus passes for MOD, S3M and MTM, with any known-failing case
   listed with a reason rather than silently skipped.
4. Cross-target hash equality holds on the fixed-point path (x86-64, aarch64, wasm32).
5. Loader fuzzing runs in CI and no input panics or OOMs.
6. The allocator hook proves no allocation inside `render()` across the whole corpus.
7. Every new deviation found is recorded in `plans/product/03-accuracy-policy.md` §3.

## A note on the oracle

libxmp's `test-dev/` suite is the single highest-value input to this milestone: hundreds
of purpose-built modules that isolate one behaviour each, **with expected per-frame
channel-state dumps**. Mine it *before* writing MOD effect code, not after — it will tell
you what the answer is supposed to be while the code is still cheap to shape.
