# W15 remediation R6a — Recount keyboard selection before virtualized repaint

| Field | Value |
|---|---|
| Status | **Integrated — reviewed; focused and final combined automated gates passed** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | Integrated R6 |
| Ownership | apps/starplayer-web/www/library-view.js only |
| Branch / sibling worktree | `w15-remediation-r6a` / `../starplayer-w15-remediation-r6a` |
| Authorization | Implementation/integration authorized; worker must not stage, commit or merge |

## Context for a fresh agent

Read AGENTS.md, [master](W15-remediation-master-plan.md) and [R6](W15-remediation-R6-incremental-view.md).
R7's retained indexed-library Chromium suite fails its original Ctrl+A assertion:
37,674 playable tracks should be selected; the display instead says zero. Selection
IDs change correctly, but the R6 selectedPlayable cache is stale when keyboard focus
is outside the rendered window. R7 owns shared browser tests and is paused for this
narrow production correction.

## Deliverables

- In the grid keyboard path that repaints a virtualized focus target, recount selection
  before renderRows(), as the already-visible focus path does. renderRows() must keep
  its ordinary repaint no-recount behavior, so scrolling/status/metadata do not scan
  every selected row. A narrow `else { updateSelection(); renderRows(); }` is sufficient
  if confirmed against current code; no new selection abstraction or cache redesign.
- Preserve Ctrl/Meta+A, Escape, arrows/Home/End/PageUp/PageDown, Shift ranges, hidden-ID
  retention across filtering, Add selected and accessibility. Recount once per actual
  user selection mutation is allowed; retain incremental metadata operation bounds.
- Report the reviewed diff and focused results. Do not modify R7-owned headless files,
  model/controller/scheduler, plans, historical assembly, formatting or attribution.

## Research points

Follow selectedPlayable updates in updateSelection(), renderRows() and the keyboard
handler. Distinguish a selection mutation from rendering the same selection again.
R7 retains the existing 37,674-row Ctrl+A and Shift+Home assertions and adds explicit
End -> one selected, Escape -> none and offscreen-focus cases. No assertion is waived.

## Verification

Run `node --test --test-isolation=none apps/starplayer-web/test/library-index.test.mjs apps/starplayer-web/test/library-discovered-model.test.mjs apps/starplayer-web/test/library-view.test.mjs apps/starplayer-web/test/library-view-state.test.mjs`.
Verify a real Chromium isolated view with a synthetic result, focus outside the
rendered viewport, Ctrl+A/End/Shift+Home/Escape and correct count/Add selected state.
Use a temporary probe if needed; permanent browser regression belongs to R7. A copied
ignored dist may be used with this single source overlaid; verify source/dist bytes.
Do not share a cached xtask binary across worktrees without rebuilding it: workspace
root is embedded in that executable. R7 performs final packaging/full three-mode and
hidden/performance gates after reviewed integration.

## Out of scope

Full-query slicing, selection-policy changes, engine/RT work, broad DOM refactors,
headless edits, cargo fmt, commits or merges by the worker.

## Execution evidence (2026-10-27)

Integrated as `8519ff2`: one-line keyboard-path recount, with ordinary repaint
unchanged. Coordinator independently passed 50 focused Node tests and Chromium
37,674-row offscreen Ctrl+A, End, Shift+Home, Escape, Add selected and focus/ARIA
checks. Source/dist byte identity was verified. R7 passed full acceptance with the
retained original assertions plus additional keyboard count regressions.
Evidence: `/tmp/starplayer-w15-remediation-evidence/r6a-*.log`.

Final combined Node/Rust/WASM/CI, Chromium and Pages gates passed; see
[R7 final evidence](W15-remediation-R7-integration-evidence.md#final-execution-evidence-2026-10-27).
Native-picker, real-collection and device acceptance remain owner checks.
