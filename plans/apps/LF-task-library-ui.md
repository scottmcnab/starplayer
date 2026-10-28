# LF-U — Compact warnings, Library details and audition order

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

Own web UI issues 1, 3, 4 and 5. Engine workers handle MOD rejection and XM duration.
Relevant www code: library-view.js, library-selection.js, library-view-state.js,
playlist.js, app.js, index.html, style.css. Use existing Node DOM/browser harnesses.

## Deliverables

- Every unplayable/error Library track's exclamation marker shows the specific failure
  reason in a tooltip. Ensure marker/cell targets do not mask full diagnostics.
- Playlist unavailable errors ("library not connected") become a yellow exclamation
  immediately left of filename with full error tooltip. Diagnostics must not determine
  Filename width. Preserve playing/failure markers, full filename/source tooltips,
  accessibility, 32px rows and contained phone scrolling.
- Allow clearing Library selection and hide lower instrument/comment details when none
  selected: checkbox/header/Escape and suitable ordinary click gesture. Keep keyboard,
  ranges, bulk add and hidden-selection behavior; stale focus is not selection.
- Lower Library instrument/comments box gets an accessible collapsible header, expanded
  by default, with collapsed state retained across row changes during page session.
- Instruments play/audition column moves immediately before checkbox in header and body.
  Keep checkbox input isolation, playback policies and audition behavior.
- Update existing help/README if affected and record verification below. Parent owns
  plans/README.md and shared foundation docs.

## Research points

Inspect selection helper: it may retain focus after deselection for keyboard navigation,
but details must hide. Preserve Library/Playlist selection differences. Inspect actual
metadata error propagation and marker tooltip targets. Prefer native details/summary
if fitting markup. Update meaningful column-index checks in browser harnesses.

## Verification

Run node --check touched JS, git diff --check and full serialized Node suite:
node --test --test-concurrency=1 apps/starplayer-web/test/*.test.mjs.
Add focused behavior coverage for warning tooltips and selection/disclosure changes;
avoid redundant tests mirroring trivial markup. Parent owns WASM/browser checks:
phone/desktop widths, tooltip reasons, selection/deselection, collapse persistence,
header/body column order and audition. Coordinate any browser/build needs.

### Worker evidence (2026-10-29)

- Implemented explicit Library error-marker tooltips, compact yellow Playlist
  availability markers, selected-row details with native disclosure persistence,
  sole-row click deselection, Escape clearing from native checkboxes, and Play
  before note-overlap checkbox in both table headings and rows.
- Updated existing Library/Playlist/instrument browser scenarios for the new
  behavior; coordinator will run the packaged WASM/browser checks.
- Focused Node selection/rendering/control checks: 41 passed. Full serialized Node
  suite: 394 passed, zero failed (about 9.5 seconds).
- `node --check` passed for all touched JavaScript/test files; `git diff --check` passed.

## Out of scope

Engine/duration/loaders, persistence schema, Sites hosting/deployment.
