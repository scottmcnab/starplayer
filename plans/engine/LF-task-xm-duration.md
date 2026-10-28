# LF-X — XM restart-position timeline duration

| Field | Value |
|---|---|
| Status | Implemented and verified; coordinator review required |
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
- Repair the first-pass/restart-position disagreement through the shared sequencer,
  using the real processor for scan/live playback and retaining bounded termination
  and RT safety.
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

## Implementation evidence

The defect is constructor positioning, not an `E6x` loop or an XM loader effect.
`PatternSequencer::try_new` used `settings.restart_order` for the first order as well
as for wrapping. The XM header preserves restart position 17 correctly; the initial
scan therefore started at order 17. The host's `build_source_with_cursor` separately
uses `seek_order_at(0, frame)` for `SeekKind::None`, so actual host playback includes
the introduction and the middle of the song which its duration omitted.

The constructor now begins at order zero's first playable entry, and retains the
configured restart order for subsequent wraps. This is a construction-time change;
render allocation, detector state, tick timing and native XM effect semantics are
unchanged. The existing bounded retry still stops an invalid restart.

### Real archive audit

Read-only source: `MODS/XM/HNZ-SAS.ZIP`, member `HNZ-SAS.XM` (40,551 bytes), title
`scotch and sand`, FastTracker2. Header: 12 channels, 19 sequential orders 0..18,
restart 17, speed 3, BPM 114. Order 0/pattern 0 has `D00` on row 3/channel 6;
patterns 1..14 have `F03` on rows 0 and 32/channels 0 and 1. There are no `E6x`
commands. One full pass is four introduction rows plus 18 full 64-row patterns:
1,156 rows, 3,468 musical ticks. The dispatch at the wrap reports the end before
playing the restart row.

Measured at 44,100 Hz via public `load` / `scan_song` / `NativeSequencer` APIs,
using a temporary harness under `/tmp` and no mixing:

| Measurement | Before | After |
|---|---|---|
| Scanned duration | 371,368 frames; 8.421043084 s | 3,353,921 frames; 76.052630385 s |
| End reason | `Ended` | `Ended` |
| First scanned row | order 17, row 0 | order 0, row 0 |
| Timeline marks | 128 | 1,156 |
| First order's timeline frame | absent | 0 |
| Restart order's timeline frame | 0 | 2,982,552 |
| Live end without an initial seek | `Wrapped` at 371,368 | `Wrapped` at 3,353,921 |
| Live end with host-style seek to order 0 | `Wrapped` at 3,353,921 | `Wrapped` at 3,353,921 |
| Wrap destination | order 17, row 0 | order 17, row 0 |

The baseline CLI independently reports `0:08.421`; the corrected duration rounds to
`1:16.053`. The temporary harness and original music are not repository artifacts.

### Regression and verification results

- `crates/starplayer/tests/xm_restart.rs` builds a three-order synthetic FastTracker2
  XM with restart 2 and an introduction shortened by `D00`. Public normal/fallible
  scans agree at 600 frames/1,000 Hz, record order 0 at frame zero and all reached
  orders, and the live sequencer reports `Wrapped` at exactly frame 600 under
  `AtEnd::Stop`, without a corrective initial seek. Both XM crate constructors
  also begin at order 0. Negative control with the original constructor fails both
  tests: scan 240 instead of 600 frames, initial order 2 instead of 0.
- Shared engine regressions now scan/play identical nonzero restart settings and
  verify three consecutive wraps and clock rebases, with no workaround initial seek.
  The unplayed restart-row case still passes. An invalid restart regression verifies
  the complete first pass plays and the bounded wrap retry stops.
- `cargo test -p starplayer --test xm_restart`: 2 passed.
- `cargo test -p starplayer-xm -p starplayer-engine`: all passed, including 122 XM
  unit tests, 95 engine unit tests, 22 block-size determinism tests and existing
  pattern-loop/end/budget regressions. The corpus integration skips because the
  sibling worktree does not contain the pinned corpus cache.
- `cargo test -p starplayer-offline --test render_allocation`: 11 passed; corpus
  cases skip without the sibling corpus cache.
- `cargo check -p starplayer-engine -p starplayer-xm -p starplayer
  --no-default-features --features starplayer/xm --target riscv32imac-unknown-none-elf`:
  passed.
- `cargo clippy -p starplayer-engine -p starplayer-xm -p starplayer --all-targets
  -- -D warnings`: passed.

Every Cargo command used `CARGO_BUILD_JOBS=2`, `CARGO_INCREMENTAL=0`,
`RUST_TEST_THREADS=2`, `CARGO_TARGET_DIR=/tmp/starplayer-lf-target` with the shared
slot coordinated. Parent owns combined CI, WASM, browser validation and the shared
foundation amendment: architecture section 4.1 should explicitly distinguish the
initial order-zero pass from subsequent configured restart-position wraps.
