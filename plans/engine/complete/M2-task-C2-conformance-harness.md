# M2-task-C2 — Conformance harness: libxmp and OpenMPT corpora

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Depends on | C1 (trace format) |
| Blocks | M2 exit |
| Parallel with | C3, C4 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (CI green, with any exclusions justified) |

## Context for a fresh agent

Two public corpora exist that are purpose-built for exactly this problem, and using them
is far cheaper than inventing test modules:

1. **libxmp's `test-dev/` suite** — hundreds of tiny modules, each isolating one
   behaviour, **shipped with frame-by-frame expected channel-state dumps**. It is both
   corpus and oracle. This is the higher-value of the two.
2. **OpenMPT's `test_*.{mod,s3m,xm,it}` collection** — one compatibility quirk per file,
   with expected behaviour documented on the OpenMPT wiki. Less machine-readable, more
   authoritative on edge cases.

Neither is vendored into this repo. Decide and document how they are obtained — a
submodule, a fetch step in `xtask`, or a pinned download with checksums — with the
constraint that **CI must be reproducible and must not depend on a live third-party
service at test time**.

## Deliverables

1. **Corpus acquisition** in `xtask`, pinned by commit or checksum, cached in CI.

2. **The harness** in `starplayer-testkit`: for each module, render N ticks with tracing
   on, compare against the expected state via the C1 differ, and report pass/fail with
   the first divergence.

3. **An exclusions file** — a checked-in list of known-failing cases, each with:
   the module, the behaviour it tests, why it fails, and either the accuracy-policy entry
   that justifies it or the issue that tracks fixing it. **A case may not be excluded
   without a written reason.** This file is the honest measure of where the engine stands.

4. **CI integration**: the harness runs on every commit for the formats implemented so
   far, and the exclusions file must not grow without a reason line.

5. **A summary report** — `cargo xtask conformance` prints pass/fail/excluded counts per
   format, so progress is visible at a glance.

## Research points

1. libxmp's expected-dump format and its exact semantics (which frame indices, what the
   fields mean, how it represents "no note"). Get this right once; everything depends on
   it.
2. Whether libxmp's expectations encode *libxmp's* behaviour or *the tracker's*. Where
   they encode a libxmp choice we disagree with, that is an exclusion with a reason, not
   a bug.
3. Licensing of both corpora, for the eventual public release. Note what is found; the
   licence decision itself is deferred (`plans/product/00-vision.md` decision 7).

## Verification

- The harness runs the full corpus for MOD, S3M and MTM and produces a stable
  pass/fail/excluded count.
- Deliberately break one effect; the harness fails with a first-divergence report naming
  that effect. Revert.
- The exclusions file's format is validated — an entry without a reason fails CI.

## Post-landing notes — 2026-09-02 branch review

**Deliverable 1 was delivered by substitution, not as written.** OpenMPT's
`test_*.{mod,s3m}` collection was not acquired from OpenMPT. The pinned libxmp `test-dev/`
tree vendors copies of those fixtures, and the harness uses those: **45 MOD/S3M modules,
of which 22 carry frame-state oracles and 23 are documented-only** (no expected dump, so
the harness inventories them but cannot compare them). This is a scoped deviation from the
deliverable — one pinned upstream instead of two — and it is recorded here rather than
counted as complete. Acquiring OpenMPT directly, and gaining oracles for the 23
documented-only fixtures, remains open.

**The harness overstates where the engine stands.** At the time of this review the corpus
was 3 passing of 47 (MOD 0/27, S3M 2/17, MTM 1/3) with 44 exclusions, and
`cargo xtask conformance` exited zero, because success is `failed == 0` and every failure
had been converted into an exclusion — 15 of them pointing at
`conformance/known-failures.md`, whose own preamble says each record blocks the M2 exit.
Several exclusion reasons were also wrong: eight MOD rows blamed an NTSC sample rate that
the pinned libxmp demonstrably does not select for those files.

Harness repairs — time-based alignment, loop-aware position comparison, the D18 adapter
projection, per-field waivers, a `--strict` mode with an honest summary, an oracle-derived
tick budget, and the exclusions rewrite that follows from re-running — are tracked by
[M2-task-C2a](../M2-task-C2a-conformance-harness-repairs.md). Deliverable 5's summary
report is superseded by C2a deliverable 5.

## Out of scope

XM and IT corpora (they arrive with M5 and M6, using this same harness). Golden WAVs
(C6). Perceptual comparison (M3).
