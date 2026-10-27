# W16 — Library selection checkboxes

| Field | Value |
|---|---|
| Milestone | Web player maintenance; Library and playlist UI |
| Status | Implemented and verified 2026-10-27; archived with the main landing |
| Depends on | W9 Library view and W15 incremental/virtualized selection behavior |
| Recommended model | GPT-6.1-sol |
| Verified by | Focused Node selection/view-state tests; packaged Chromium checks in SAB, fallback and plain modes |

## Context for a fresh agent

The owner wants to select multiple Library tracks on a phone without a keyboard. Add
Gmail-style native checkboxes before track names, including a select-all checkbox in
the heading, and replace the per-track `+` buttons with the existing toolbar action
renamed **Add to Playlist**. That action is enabled when at least one playable track
is selected in the current result, preserving the existing rule that unplayable or
filtered-out tracks cannot be added. Folder-tree `+` buttons are a separate feature
and stay as they are.

The grid is virtualized at a fixed 32 px row height. Selection is independent of DOM
rows, survives sorting/filtering/folder changes, and actions snapshot playable tracks
in current result order. Ctrl/Meta+A already selects all current results, including
offscreen rows; the new heading checkbox must offer the same scope. Explicit user
selection changes and full query changes may recount selected IDs, but scrolling,
status updates and metadata batches must retain their existing bounded/incremental
work. Read AGENTS.md and W15 remediation R6/R6a evidence before changing these paths.

Relevant implementation: `www/library-selection.js` holds pure state operations;
`www/library-view.js` renders rows and maintains `selectedPlayable`, updated both on
selection mutations and incrementally for changed metadata IDs. Native row controls
also need focus restoration when their row is replaced or virtualized away.

## Deliverables

### Selection and rendering

- Add one native checkbox in a leading selection cell on every rendered track row;
  remove the trailing per-row Add cell/button. Keep row height and virtualization
  limits unchanged. Adjust normal/comment-match grid columns and accessible column
  counts so headings and rows align. Retain all metadata columns and sorting.
- Checkboxes reflect the existing selected-ID state, including keyboard selection.
  Tapping/clicking a checkbox toggles that row without clearing other selected IDs.
  Keep ordinary row click, Ctrl/Meta-click, Shift-click, keyboard navigation, Enter,
  double-click playback, +/Insert and Escape behavior. Checkbox clicks, label clicks
  and double-clicks must never fall through to row selection replacement or playback.
- Keep native row checkboxes available for all displayed rows, including unplayable
  rows, because row selection also drives details and existing Ctrl+A includes them.
  Preserve the rule that the toolbar action and playlist snapshots use only playable
  selected rows in the current result. Metadata-error rows that remain playable can
  still be added. The help text must keep the playable/current-result rule clear.
- Add a labelled native heading checkbox (`library-select-all`) before Name, with
  an accessible name **Select all tracks in current results** and a short tooltip.
  It is unchecked when none are selected, mixed/indeterminate when some are selected,
  checked when every current-result ID is selected, and disabled for an empty result.
- Checking the heading selects all current results (not just rendered rows); unchecking
  it clears only those result IDs and preserves selections outside the result. Tapping
  a mixed checkbox selects all current results, like Gmail. Later discoveries remain
  unselected; an all-selected heading becomes mixed when new results arrive.
- Share selection state with Ctrl/Meta+A and the existing toolbar. Add a small pure
  deselect-current-results helper if useful, without redesigning the state model.
  Maintain a selected-in-result cache/count incrementally alongside selectedPlayable
  so heading updates do not add full-result scans to metadata/status/scroll paths.
- Preserve the focused row checkbox through metadata replacement/repaint when still
  rendered, falling back to the grid if it leaves the virtualized window. Native Space
  activation must toggle exactly once. Keep header keyboard focus during view changes.
- Make checkbox cells easy to tap using the full leading cell as a label/hit target,
  at least 40 px wide and the existing 32 px row height; use a clearly visible checkbox.
  Keep leftmost selection/name cells reachable at mobile width within the existing
  horizontally scrollable grid, without overflowing the page. Do not expand all rows
  in the DOM or introduce a custom checkbox widget.

### Toolbar, documentation and tests

- Rename `library-add-selected` to visible **Add to Playlist**, retaining its ID and
  existing add/persistence behavior. Selection remains after adding, as it does now.
  Update the on-page selection hint to lead with checkbox/select-all instructions;
  preserve a concise keyboard hint. Update the web README and plans index. Add a brief
  W16 follow-up reference to the archived W9 plan rather than rewriting its history.
- Migrate existing browser assertions and interactions that depend on per-row
  `[data-add]`/track buttons to row checkboxes and the toolbar. Preserve their original
  coverage for multi-add ordering, focus restoration, metadata-error playability,
  unplayable exclusion, no selection/add I/O, actual Shift-click ranges, and offscreen
  Ctrl+A. Find click-coordinate tests that would now hit the new selection column and
  explicitly target the name cell where a normal row click was intended.
- Add meaningful Node coverage for deselecting the current result while retaining
  hidden IDs if a pure helper is added. Add browser coverage for default/empty disabled
  states; independent checkbox toggles; none/mixed/all header states; select/deselect
  all current filtered and folder-scoped results including offscreen rows; filtered
  hidden-selection retention; no auto-selection of newly discovered rows; checkbox
  checked state/focus after metadata and scroll; and native Space without double toggle.
- Add a real CDP touch-tap scenario at the existing phone viewport. Select multiple
  row checkboxes without modifiers, tap header select-all and clear it, tap Add to
  Playlist, and verify the playlist receives the selected playable tracks in result
  order. Confirm checkbox taps do not start playback and the page does not overflow.
  Reuse existing fixtures/harness helpers; do not add production test-only APIs.

## Research points

- Inspect every `selectedPlayable` maintenance path, including full refresh,
  incremental membership changes and setLibrary. Keep heading state synchronized in
  those same paths; a scroll repaint must not recount all selected IDs.
- Audit native checkbox default events, label-generated clicks and bubbling so a
  touch tap and Space each toggle once. Preserve selection focus and details behavior.
- Review existing keyboard offscreen-focus regression and W15 incremental performance
  assertions. Changes to row layout must retain their guarantees, not weaken tests.
- Browser tests recreate the Library view by cloning its panel and clearing the
  heading. Build the header checkbox through createLibraryView, consistent with the
  other heading cells, so these fixtures continue to work.

## Verification

- `git diff --check`; `node --check` on edited JavaScript modules/harnesses.
- `node --test --test-isolation=none apps/starplayer-web/test/library-view.test.mjs apps/starplayer-web/test/library-view-state.test.mjs`.
- `cargo xtask wasm`, then
  `STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --seconds 5`
  in all default SAB, forced-fallback and plain modes. This includes the retained W15
  metadata/scroll/large-index keyboard regressions, plus new touch coverage.
- Parent reviews the diff and independently runs these checks before committing.
  Parent records verification results and archives this plan when landed on main.

## Out of scope

Changing folder-tree add controls, playlist semantics or deduplication, selection
persistence, automatically selecting future discoveries, changing playable-format
policy, new data reads on selection/add, engine/RT changes, historical sources,
deployment, broad refactoring or formatting, and cargo fmt.

## Verification results — 2026-10-27

- Parent reviewed the production diff, native event handling, shared selected-ID state,
  selected-in-result cache maintenance, fixed row height, and focus restoration.
- `git diff --check` and syntax checks of both production modules and both edited
  browser harnesses passed. Packaged HTML, CSS and selection/view sources matched the
  reviewed source files byte for byte.
- The focused Node selection/view-state command passed all 22 tests, including the
  existing 100,000-row incremental-operation bounds and the new scoped-clear case.
- `cargo xtask wasm` passed. No Rust, public engine/WASM interface, or historical
  assembly files changed.
- Focused SAB view and library browser checks passed, including the new native Space
  and real CDP touch-tap cases at 390 px.
- Parent independently passed
  `STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --seconds 5`
  in all three SAB, forced-fallback and plain modes using Chrome 156.0.8078.4.
  This retained real Shift-click/double-click playback, 37,674-row keyboard selection,
  playlist ordering, unplayable exclusion and W15 metadata/focus/incremental checks.
- Every mode passed row/header checkbox touch selection and clear/add actions,
  none/mixed/all/empty header states, offscreen and filtered/folder-scoped selection,
  hidden-ID retention, newly discovered rows remaining unchecked, metadata/scroll
  focus restoration, and page-width checks.
- Full passing browser evidence: `/tmp/starplayer-library-checkboxes-verified.log`.
