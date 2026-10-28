# W18-P — Compact Playlist table and bulk selection

| Field | Value |
|---|---|
| Status | Landed on main via merge 14bec2c; owner acceptance pending |
| Baseline | main 4d03bcb |
| Least-capable safe worker | GPT-6.1-sol (high) |
| Coordinator | W18 master; worker leaves changes uncommitted for review |

## Context for a fresh agent

Read AGENTS.md and W18 master. Owner wants Playlist to match Library density/selection.
Implement in assigned sibling worktree, no main merge, cargo fmt, broad staging,
unrelated edits, attribution or historical edits. Entire completed branch gets another
Astra medium review, arranged by coordinator. Instruments worker is independent;
limit HTML/CSS/README edits to Playlist sections and notify parent of shared files.

Relevant www/playlist.js (model and wirePlaylistList), library-selection.js and
library-view.js; playlist-controls.js persistence/upload cleanup; playlist-playback.js
navigation cancellation. Existing headless.mjs and w15-headless.mjs use old li/per-row-X
selectors and MUST be migrated without weakening checks. You may modify those tests;
avoid no-headless.mjs/w17-headless.mjs owned by Instruments task.

## Deliverables

- Semantic table with compact SINGLE LINE,32px rows, sticky header, columns checkbox /
  Filename / Title / Format / Folder. Ellipsis+full tooltips. Preserve current-playing,
  failure/warning and availability markers beside filename, complete diagnostics and
  source accessibility details. Phone horizontal scrolling contained within panel.
- Native selection checkboxes for ALL entries (including unavailable/offscreen and
  duplicate locators), keyed by entryUUID. Header none/mixed/all selects or clears
  all current entries; mixed click selects all. New entries unselected.
- Library-style selection: ordinary row click, Ctrl/Meta toggle, Shift range,
  arrows/Home/End/PageUp/PageDown, Ctrl/Meta+A, Escape. Native checkbox independently
  toggles; stop checkbox click, Space AND double-click propagation so no playback or
  row selection side effects. Maintain accessible selected/focus states, keyboard focus
  visibility and meaningful header/button names.
- Replace individual X with Remove selected toolbar button, disabled at zero. Delete
  removes current selection, empty is no-op. Both invoke ONE playlist.remove(ids),
  preserving model single-notify behavior, current/pending playback cancellation and
  upload last-reference cleanup. Selection-only changes must not write persistence.
- Selection independent of DOM and survives metadata/current/failure updates/reorder
  by stable UUID. Prune removed selected/anchor/focus IDs; restore focus to surviving
  neighbour or Add files when empty. Page reload starts with no selection.
- Preserve Enter/double-click playback, single-item drag and Alt+Up/Down reorder;
  don't add multi-item drag. Keep Clear all existing action if present. Clean up event
  listeners in dispose. Avoid rewriting generic library selection semantics that
  intentionally retain hiddenIDs; playlist must prune deleted IDs explicitly.
- Update relevant app help and Playlist README paragraphs. Coordinator owns foundation
  docs/index and Instruments worker owns Instruments help/README.

## Research points

- createPlaylist.remove uses a Set and emits once, removes currentId but keeps audio
  playing; playlist-playback subscription cancels current/pending navigation. Preserve.
- playlist-controls persistence deletes uploaded blobs only when last entry ref removed.
  Rebind/import can retain existing UUIDs; avoid accidental clearing of survivingIDs.
- Current renderer replaces children and restores focus; selection events should update
  native controls without focus loss. Native checkboxes need keyboard isolation.
- Existing Library selection helper supports UUID strings; it retains hidden IDs by
  design, so prune before row render/bulk removal. Determine viewport-based pageSize.
- Review key handling so app musical keyboard ignores playlist focus as before.

## Verification

- git diff --check and node --check touchedJS; full Node suite serialized.
- Model/DOM tests for none/mixed/all and mixed->all; offscreen, duplicate, unavailable;
  click/modifiers/ranges/keys, checkbox Space/click/dblclick isolation, emptyDelete,
  Remove selected single bulk notification, stable UUID selection/pruning/focus,
  reorder, metadata/playback updates, newentries unselected, no selection persistence.
- Removal tests current/pending playback cancellation, last uploaded blob cleanup,
  duplicate refs retained, single-item reorder/drag unchanged, dispose listeners.
- Coordinate Cargo/browser access with parent; if no slot, Node checks first.
  Packaged Chromium full playlist/library/W15 harness (update old selectors); native
  keyboard/pointer checks and desktop/390px phone screenshots,32px row measurements,
  no page overflow, filenames/markers visible and header/selection actions work.
- Worker leaves code uncommitted, reports evidence. Parent independently reviews and
  tests combined branch. Do not merge or archive plan as landed.

## Out of scope

Instruments/engine/wire changes, multi-drag, playlist sorting/filtering/virtualization,
persisting selection, new generic framework, main merge/push/deploy, independent W6
and owner scratch files, unrelated formatting or historical sources.

## Completion evidence

Implementation, independent verification and all resolved Astra findings are recorded
in [W18 review and verification](W18-review-and-verification.md). After review, the owner
authorized --no-ff landing and origin push. Merge 14bec2c lands the feature on main;
owner acceptance remains. No direct deployment was performed.
