# LF-M — Legacy library MOD compatibility

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

The owner reports AXELF.MOD, BLOODMNY.MOD, IMPACT.MOD, JM_JARRE.MOD, RSI1.MOD and
MFC2.MOD in old/mods.zip fail although original StarPlayer played them. Own
crates/starplayer-mod and format dispatch in crates/starplayer/src if needed.
Coordinate other crate/shared document edits with parent.

## Deliverables

- Reproduce and explain exact rejection and layout for all six archive members.
- Add narrowly justified support that loads and plays all six, retaining bounds checks,
  byte-slice and ModuleReader loading, and conservative format probing/dispatch.
- Add synthetic regressions for observed formats and malformed/truncated false positives.
- Record settled behavior, actual-file evidence and verification below. Send parent any
  accuracy-policy amendments needed for new supported format contracts/deviations.

## Research points

Inspect archive extraction vs dispatch errors, header signatures, order/sample layouts,
existing legacy support and original assembly loader. Do not mistake decode failures
for format errors. Inspect existing conformance and sample-truncation policy.

## Verification

Run complete starplayer-mod tests, relevant public-dispatch tests and no-default-feature
checks. Reproduce all six actual files via public dispatch and offline render enough
frames to show playback. Coordinate Cargo access; parent owns CI/WASM/browser checks.
Do not regenerate canonical goldens.

## Out of scope

UI tooltips, XM timeline, broad loader redesign.
