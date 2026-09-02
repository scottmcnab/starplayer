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
| [C3](complete/M2-task-C3-mod-loader-and-effects.md) ✅ | MOD: loader + ProTracker effect processor | M1 | C4 |
| [C3a](complete/M2-task-C3a-mod-headphone-panning.md) ✅ | Web option for S3M-width MOD headphone panning | C3, C4 | C5, C7 |
| [C4](complete/M2-task-C4-mtm-loader-and-effects.md) ✅ | MTM: loader + effect processor | M1 | C3 |
| [C5](M2-task-C5-quirks-and-tempo-models.md) | `QuirkSet`, `FormatDialect` and `TempoModel` wired end to end | C3b ✅, C2a ✅ | C7 |
| [C6](complete/M2-task-C6-fixed-point-mixer.md) ✅ | The fixed-point mixer — the bit-exact reference | M1 | C1, C3, C4 |
| [C7](M2-task-C7-fuzzing-and-rt-safety.md) | Loader fuzzing + the allocator hook in CI | C3, C4 | C5 |
| [C8](M2-task-C8-dos-reference-harness.md) | **Deferred**, pull-driven — see its trigger | — | — |
| [C2a](complete/M2-task-C2a-conformance-harness-repairs.md) ✅ | Harness repairs, `--strict`, exclusions rewritten from real results | C2 | C6a |
| [C3b](complete/M2-task-C3b-protracker-fidelity-repairs.md) ✅ | ProTracker fidelity + MTM repairs, and the voice-boundary sample swap | C3, C4 | C9, C6a |
| [C6a](complete/M2-task-C6a-golden-and-build-hygiene.md) ✅ | Trace-feature hygiene, MOD/MTM goldens, `fma-check`, the web panning race | C6, C1, C3a | C2a, C3b, C9 |
| [C9](M2-task-C9-s3m-conformance-repairs.md) | The nine S3M effect bugs the corpus found (M1 inheritance) | C2a | C3b, C6a |

C2a, C3b, C6a and C9 are the follow-ups opened by the 2026-09-02 branch review. C2a comes
first: it changes which cases actually fail, so C3b and C9 must not be diagnosed against
the pre-repair harness.

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

## Status (2026-09-02 review, updated after C2a)

Honest position, so that nothing downstream plans against a number that is not true.

**Conformance: 14 of 47 cases pass after C3b** — MOD **11 of 27**, S3M **2 of 17**, MTM
**1 of 3** — with **13 accepted deviations** and **20 known failures** (5 MOD tracker
dialects for C5, 15 S3M for C5 and C9). No MOD or MTM known failure remains. Seven of the
MOD passes waive `position` alone under D14 and enforce every other field. `cargo xtask conformance` exits
zero in its informational form; `--strict` exits non-zero on the 28 known failures and is
the M2 exit gate once C3b, C5 and C9 land. Before C2a the standing was 3 of 47 with 44
undifferentiated exclusions, eight of them citing an NTSC rate libxmp never selected.

| Exit criterion | Status | Closed by |
|---|---|---|
| 1. MOD and MTM native, no S3M conversion | **Met** | C3, C4 |
| 2. `cargo xtask goldens` regenerates; CI verifies | **Met** — S3M owner fixtures plus synthesised MOD and MTM fixtures | C6a (landed) |
| 3. `test-dev/` corpus passes for MOD, S3M, MTM | **Not met** — 14/47 after C3b; every remaining MOD/MTM exclusion is an accepted deviation or a C5 dialect | C9 (S3M), C5 (dialects) |
| 4. Cross-target hash equality on the fixed path | **Met** for all three formats on x86-64 and wasm32; the ARM64 leg is CI-only | C6a (landed) |
| 5. Loader fuzzing in CI, no panic or OOM | **Not met** | C7 |
| 6. Allocator hook proves no allocation in `render()` | **Not met** | C7 |
| 7. Every new deviation recorded in the accuracy policy | **Met for the current standing** — every exclusion reason records a first divergence the repaired harness observed | C2a (landed) |

Landed: C1, C2, C2a, C3, C3a, C4, C6, C6a, C3b. Outstanding: C5, C7, C9. Deferred: C8.

## A note on the oracle

libxmp's `test-dev/` suite is the single highest-value input to this milestone: hundreds
of purpose-built modules that isolate one behaviour each, **with expected per-frame
channel-state dumps**. Mine it *before* writing MOD effect code, not after — it will tell
you what the answer is supposed to be while the code is still cheap to shape.
