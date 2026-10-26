# W15-E — Progressive library view and accessible controls

| Field | Value |
|---|---|
| Status | Complete — reviewed and integrated on `w15-folder-library`; final automated acceptance passed |
| Recommended model | gpt-6.1-sol |
| Recommended effort | **high** |
| Depends on | D2 integrated; parallel with F |
| Branch / sibling worktree | `w15-e` / `../starplayer-w15-e` |
| Ownership | `library-view.js`, `index.html`, library CSS and view unit tests |
| Coordinator | gpt-6.1-sol, xhigh; [master](W15-master-plan.md) |

## Context for a fresh agent

Read `AGENTS.md`, [W15 master](W15-master-plan.md), and the complete
[behavioral specification](W15-task-folder-library-discovery.md). The master defines
shared APIs; the behavioral specification defines edge cases and compatibility. Work
from this task file without relying on conversation history. Do not commit or start
another task. Report changes, verification results and handoff contracts to coordinator.
All paths below are repository-relative; web JS paths are under
`apps/starplayer-web/www/` unless stated otherwise.

The current setLibrary resets selection/search/tree; refresh and refreshComments
reset scroll. Implement an incremental path on top of D2's context/status/model, not
by repeatedly calling any reset path. F will own all app.js callback wiring.

## Deliverables

- Add master updateLibrary(context,change) preserving selection, focus, search, scope,
  expanded nodes and playing row. Recompute order/tree/filter options when needed,
  preserving visible anchor row and pixel offset; if the anchor disappears, choose
  nearest surviving row in previous order, then clamp scroll. Preserve selected IDs
  even when filtered out, with existing visible-result bulk semantics.
- Add controls with master's exact IDs and accessible labels. D2 binds top-level
  discovery/metadata/index-bypass controls; E exposes errors in library-issues and row
  details even without comments, tooltips, and row Retry metadata callback. Make
  complete empty, paused/failed partial, metadata waiting/suspended and active progress
  distinct. Show runtimeError/retry in library-progress separately from row errors,
  including when directory discovery has already completed.
- Refresh metadata search/format/comment controls incrementally, including first
  comments arrival. Re-sort on updated metadata using C tie-break; no scroll reset.
  Keep indexed library behavior and existing keyboard/virtualization functionality.
- Add discovered rows to playlists with display.metadataState. Preserve Add selected;
  update folder-tree + accessible labels/tooltips during incomplete discovery and use
  the click-time snapshot only. Known unextractable rows respect Show unplayable;
  failed metadata rows stay visible/playable. No file reads on select/add.
- Batch rendering consistent with D2 notifications; avoid rebuilding every row per
  item. Virtualized DOM remains bounded. Hand F the tested updateLibrary interface.

## Research points

Inspect focus/ARIA relationships and details-pane gating. Ensure tree count updates
and error details do not invalidate cached comment matches or collapse expansion.
There is no Node DOM harness for createLibraryView. Extract order/anchor/label/option
calculations into an E-owned pure helper module for Node tests. G owns mandatory real
DOM scenarios listed below; do not invent fake layout as browser verification.

## Verification

Run `node --test apps/starplayer-web/test/library-view.test.mjs` plus new pure
view-state helper tests and `cargo test -p starplayer-web`. Node tests cover anchor
selection/clamping, incremental sort/filter option and label computations, stable IDs,
folder snapshot selection and metadataState construction. Existing library-view tests
exercise selection helpers, not a DOM: do not claim scroll/focus acceptance from them.
Hand G these named mandatory Chromium scenarios: E-append-scrolled, E-anchor-removed,
E-selection-focus-tree-survive, E-first-comments-and-format-filter, E-error-details,
E-stale-context-ignored, E-folder-snapshot-add, E-row-retry-no-I/O. G verifies control
IDs, real focus/scroll, comment toggles, error visibility and controller callbacks.
E owns in-page help; if an HTML change breaks Rust source-contract assertions, hand
the exact test change to F (sole test-module writer during this parallel wave) and
rerun Rust tests after F integrates it.

## Out of scope

app.js/playlist-controls wiring, source controls logic, worker lifecycle, producer
model changes, headless harness, unrelated UI redesign.

## Verification evidence (2026-10-26)

Worker passed full web Node suite 173/173, view/helper tests 12/12, web Rust tests
14/14 plus docs, JS syntax checks and `git diff --check`. Coordinator independently
passed the 12 view/helper tests and reviewed the full production/help/test diff.
No Rust HTML assertion changes were needed; only F edits source-contract tests.

`updateLibrary` preserves view state and ignores obsolete contexts; progress and
issues remain separate from D2-owned summary text. Live unplayable count excludes
playable metadata errors. Retry callbacks capture their originating context.
G executed all eight mandatory Chromium scenarios above in SAB/fallback/plain,
including the live count in E-error-details. Helper tests establish calculations
only; G supplies the actual DOM acceptance.

Final integrated automated acceptance passed in G, including all three browser
modes and full 11-job repository CI. Remaining manual owner acceptance is recorded
in the [master](W15-master-plan.md). This task is archived with the completed
integration deliverable; `main` was not merged or deployed.
