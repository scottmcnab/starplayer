# LF-X — XM pattern-loop timeline duration

| Field | Value |
|---|---|
| Status | In progress; coordinator review required |
| Baseline | main 556ed6a |
| Least-capable safe worker | GPT-6.1-sol (high) |
| Coordinator | library-playback-fixes; workers leave changes uncommitted |

## Context for a fresh agent

Read AGENTS.md first. Work only in the assigned sibling worktree. Historical
STARPLAY/ and STARPLAY-2.25s/ are read-only. No cargo fmt, unrelated edits, broad
staging, commits, merge, push or deployment. Leave changes uncommitted for review.
Preserve no_std, allocation-free render, sample-exact events, native formats and
block-size determinism. Library input is read-only under
/mnt/c/Users/scott/Documents/Projects/music. Never commit music files; use synthetic
regressions. Coordinate Cargo access with parent, using CARGO_BUILD_JOBS=2,
CARGO_INCREMENTAL=0, RUST_TEST_THREADS=2, CARGO_TARGET_DIR=/tmp/starplayer-lf-target.

The owner reports MODS/XM/HNZ-SAS.ZIP displays 0:08 but plays much longer; the archive
contains HNZ-SAS.XM (40,551 bytes). Own crates/starplayer-xm, engine timeline/sequencer
if necessary, and focused regressions. MOD worker owns mod crate/facade dispatch.

## Deliverables

- Reproduce 8-second duration and explain exact disagreement with playback.
- Repair duration and underlying loop-arrival classification if needed, using real
  processor for scan/live playback and retaining bounded termination and RT safety.
- Add small synthetic regression for observed flow, matching scan and actual sequencer
  end notification, plus relevant nearby loop/end behavior.
- Record before/after duration, end reason, effect evidence and tests below. Send parent
  required foundation-document amendments rather than editing shared docs concurrently.

## Research points

Inspect XM E6x loops, pattern breaks/jumps, RowArrival, LoopDetector, repeated rows,
loop counters and existing contract/tests. Distinguish scanner, flow-processor and UI
metadata defects. Do not alter canonical effect semantics without repository evidence;
external research, if necessary, must rely on primary sources.

## Verification

Run focused and complete XM/affected engine tests, relevant RT/determinism tests and
no-default-feature checks. Re-run real archive duration via public API, compare actual
sequencing end; never commit the original file. Coordinate Cargo access. Parent owns
combined CI, WASM and browser checks; do not regenerate goldens.

## Out of scope

MOD compatibility, UI redesign, scheduling policy changes.
