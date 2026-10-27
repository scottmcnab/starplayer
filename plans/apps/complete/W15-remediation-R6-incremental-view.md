# W15 remediation R6 — Targeted view updates, bounded issue UI and actionable status

| Field | Value |
|---|---|
| Status | **Landed on `main` via `ece531f` (2026-10-27) — reviewed/verified; manual feature acceptance pending** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | R2/R3/R5 |
| Ownership | library-view.js, library-view-state.js, index.html/CSS after R3, pure view tests |
| Historical branch / sibling worktree | `w15-remediation-r6` / `../starplayer-w15-remediation-r6` |
| Authorization now | Implementation authorized; worker must not stage, commit or merge |

## Context for a fresh agent

Read AGENTS.md, [remediation master](W15-remediation-master-plan.md), and the
[archived W15 master](W15-master-plan.md) with its linked implementation
evidence. The remediation contracts explicitly supersede only listed archived clauses.
Feature baseline is b3faf9e; execution starts from the integrated planning tip.
Task status and verification are updated by the coordinator.
Do not rely on chat history or temporary review scripts. JS paths are under
`apps/starplayer-web/www/`; tests under `apps/starplayer-web/test/`. During execution,
worker reports diff/results to coordinator, does not commit or edit another task's
files. Preserve engine/RT/no_std invariants.

Finding #6 is full search/sort/tree work for row/metadata batches and full issue JSON
serialization on status ticks; status-only updates already skip some heavy work.
R2 emits typed deltas and corrected counts. Do not solve this by raising debounce.

## Deliverables

- Implement master E per-row contribution/sort/filter caches and incremental merged
  result order. Use added/fields to skip irrelevant work, update tree ancestor counts
  only on row structural changes, and repaint visible changed rows for display-only
  updates. Status-only notifications perform no full row/issue/tree scans.
- User-initiated query/sort/format/scope changes and sidecar replacement keep today's
  synchronous full search+sort (master E, amended). Do not build sliced or cooperative
  sorting. Preserve anchor/offset/focus/selection, expanded tree and click-time folder
  adds across both full and incremental updates.
- Render issue list only expanded/current 100-item page and issue revision changes;
  accessible Previous/Next/total. Do not JSON.stringify full list. Limit issue DOM and
  keep focus on navigation while new issues arrive. Tree updates preserve expanded
  nodes and update only affected visible nodes. Metadata label updates skip unchanged DOM.
- Consume scanActionable, excluded and sidecar failed/excluded counts: Scan disabled
  for excluded/error-only with a clear non-actionable/retry explanation; show pending
  playback digests and current-version refreshPending separately from uninspected/
  missing metadata. A retained old coherent ready+SHA snapshot can await refresh;
  obsolete-only digest work must not make the latest verified row incomplete. Retain
  row Retry metadata and canPlay behavior. Keep R3
  selection-status separate from accepted progress. No visibility listener (master B,
  amended). If an incremental batch must yield, use R5's integrated helper; no private
  alternative scheduler.
- Add pure helper operation counters/tests for full sorts, comparisons, tree builds,
  issue materialization and row evaluation. Test instrumentation is dependency-injected,
  not a production UI/debug panel. R7 performs actual DOM/browser evidence.

## Research points

Reuse library-index query semantics and stable tie-break. Extract row predicate/
comparison helpers if required via a narrow R2→R6 handoff after R2 completes; preserve
public search() tests. No new framework/dependency.

## Verification

Run view-state/library-view/library-index Node tests with --test-isolation=none.
For 100k synthetic rows + 10k issues, assert status-only ticks do 0 full sorts/tree builds/
issue serializations; 100 title patches under name sort with no title query cause no
reorder/tree rebuild; structural additions update correct ancestors; title-filter/sort
changes reevaluate relevant rows and correct merge. Test issue page bounds, row-delta
coalescing, sidecar-wide changes, all-excluded/error-only counts, obsolete context and
disposal; published rows/status/issue revision must agree, not merely progress labels.
Hand R7 browser scenarios for open tree, scroll anchor, keyboard focus, search input
while updates arrive and performance instrumentation. Shared headless remains R7-owned.

## Out of scope

Playback/source selection/model counter policy changes, runtime scheduling changes,
metadata eager reads, removing search/keyboard/ARIA behavior, production framework swap.

## Execution evidence (2026-10-27)

Integrated as `3b8ebe4`. Coordinator independently passed all 50 focused index/model/view tests after final pruning/focus review. Operation evidence proves no global work on unchanged status or irrelevant title batches, one merge for relevant sort-key changes and 100 issue items per page. Actual DOM/browser gates remain R7.
`git diff --check` passed. Logs: `/tmp/starplayer-w15-remediation-evidence/`.

## Retained browser follow-up (2026-10-27)

The full indexed-library suite found offscreen keyboard selection skipped the
selectedPlayable recount. [R6a](W15-remediation-R6a-keyboard-selection.md) owns the
narrow keyboard-path fix; regular paints remain bounded. Existing Ctrl+A/Shift-range
assertions remain required, with explicit count regressions added by R7.

R6a is integrated as `8519ff2`; coordinator independently verified keyboard counts
in Chromium and all 50 focused tests. Final combined R7 acceptance remains pending.

Final combined Node/Rust/WASM/CI, Chromium and Pages gates passed; see
[R7 final evidence](W15-remediation-R7-integration-evidence.md#final-execution-evidence-2026-10-27).
Native-picker, real-collection and device acceptance remain owner checks.
