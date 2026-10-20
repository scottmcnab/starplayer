# W9 — The library view: search, folders, selection, play and add

| Field | Value |
|---|---|
| Milestone | Web player: music library and playlist (W7–W11, branch `library-playlist`) |
| Status | Specified 2026-10-21; not started |
| Depends on | [W7](W7-task-library-index-and-member-lookup.md), [W8](W8-task-library-sources-and-resolver.md) |
| Blocks | [W10](W10-task-playback-controller-and-playlist.md) |
| Recommended model | gpt-6-astra |
| Verified by | agent (`node --test apps/starplayer-web/test/*.test.mjs`, `cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`), then the owner in a browser on the Hornet mirror |

## Context for a fresh agent

Read `CLAUDE.md` (full variable names, compact formatting; do **not** commit). Read
[W7](W7-task-library-index-and-member-lookup.md) (Interfaces, `library-index.js` API) and
[W8](W8-task-library-sources-and-resolver.md) (sources, resolver, the page state they
leave behind), then `apps/starplayer-web/www/{index.html,style.css,app.js}` and
`apps/starplayer-web/test/headless.mjs` (how the page is driven and asserted on: it
asserts on displayed text, builds ZIPs with `node:zlib`, and serves `dist/` through
`dev-server.mjs`).

After W8 the page can open a library and holds `{ source, library, identity, resolver }`
but shows only a summary line. This task builds the browser the owner asked for: "quick
filtering, such as searching tracks that match track file name, title, path, format, as
well as just browsing with optional directory structure", and quick adding to a playlist
("shift-select or ctrl-select many tracks in a music disk"). The library may hold 37k
rows; it must stay responsive.

## Deliverables

### 1. `apps/starplayer-web/www/library-view.js` and a library section

A collapsible **Library** section in `index.html` (a `<details>` or a panel near the load
panel; it replaces W8's summary line), styled in `style.css` in the page's existing
idiom, usable at phone width (390 px) with no horizontal page scroll.

* **Toolbar:** search box (filters on `input`, debounced ~80 ms, using W7's
  `compileQuery`/`search`; a hint shows the prefix syntax), format `<select>` (All + the
  formats present), **Show unplayable** checkbox (off by default), **Browse folders**
  toggle, result count (`1,234 of 9,585`).
* **Folder pane** (shown when Browse folders is on): W7's `buildTree`, lazily expanded
  (render only expanded nodes), each node with its playable `trackCount`; directories,
  ZIPs and nested ZIPs visually distinct. Selecting a node sets `scopeFolder`; a
  breadcrumb or **All** returns to the root. Off → flat list over the whole library.
* **Track table:** virtualised — fixed row height, a spacer for the full height, and only
  the visible rows plus a small overscan rendered (no more than ~200 row elements at any
  library size). Columns Name, Title, Format, Ch, Size, Date, Folder; clicking a header
  sorts (again → descending); `aria-sort` set. Unplayable rows greyed and not playable.
  Each row has a **+** button.
* **Selection** is a `Set` of row ids, preserved across sort, filter and scope changes
  (rows filtered out stay selected but are not acted on unless visible in the current
  result — state the rule in the UI hint and the README):
  * click selects one; Ctrl/⌘-click toggles; Shift-click selects the range from the anchor
    in the current result order; **Ctrl/⌘+A selects every row of the current filtered
    result** (not just rendered rows); Escape clears.
  * ↑/↓ move the focus row, Shift+↑/↓ extend, Home/End, PageUp/PageDown; the table is one
    focusable widget (`role="grid"` or a listbox pattern) with an `aria-activedescendant`
    so screen readers follow the focus row.
* **Actions:**
  * Enter or double-click on a playable row → play it now (see 2).
  * `+`, Insert, a row's **+** button, or an **Add selected** toolbar button → add the
    selected playable rows (in current result order) to the playlist.
  * **Add folder** on a tree node → add all its playable rows (folder then name order).
  * The playing row is highlighted and scrolled into view when it changes (if visible).

### 2. Playing and adding — the seams W10 completes

* Playing: add a small exported function in `app.js`, `playLibraryRow(row)`, that claims
  the existing revision mechanism as early as possible, resolves the bytes through the
  W8 resolver and feeds them to the existing `activateLoadedModule(buffer, label, audio)`.
  Label: `name — title (folder)`. W10 replaces this with the playback controller; keep it
  small and isolated.
* Adding: create `apps/starplayer-web/www/playlist.js` exporting the W7 **playlist model
  interface** (`createPlaylist()` with `add/remove/move/clear/current/setCurrent/next/
  previous/subscribe`, entries `{id, locator, display}`, ids from `crypto.randomUUID()`),
  in-memory only, with a minimal visible list (name + count) so adding can be seen and
  tested. Locators are W7's library locators, including `libraryIdentity` from W8's state.
  W10 builds the full playlist UI on this model.

### 3. Tests

* `test/library-view.test.mjs` (`node --test`): the selection model as a pure function
  module (`library-selection.js` or exported from `library-view.js` without DOM access):
  click / toggle / range / select-all-filtered / preservation across re-sort and
  re-filter / keyboard extend. The virtual-window arithmetic (first/last rendered index for
  scroll offset, height, overscan, row count 0 and 37,674).
* `test/headless.mjs`: a new library scenario, run in each mode the harness already runs.
  Build a synthetic library in a temp dir from the owner-authored fixtures only
  (`crates/starplayer-s3m/tests/fixtures/REFLEX.S3M` and `PETRI.S3M` — **never**
  `ARMANI.S3M`, which is third-party): a loose `loose/REFLEX.S3M`, `disks/disk1.zip`
  holding `PETRI.S3M` and `REFLEX.S3M`, `disks/outer.zip` holding `inner.zip` holding
  `PETRI.S3M`, and an `index.csv` with **shuffled minimal columns**
  (`member_path,title,source_path,container_member`) plus one row naming a missing
  member. Serve with `--library`. Assert: the row count; typing a filter narrows it; a
  tree scope narrows it; Shift-click and Ctrl-click selections and **Add selected** put the
  expected entries in the playlist list; double-click plays a ZIP row and a nested row
  (the now-playing title appears); the missing member shows an error naming it and the
  previous song keeps playing.
* `headless.mjs` currently exits 0 with `headless: skipped` when no Chromium is found
  (~line 1187). Add: when `STARPLAYER_REQUIRE_CHROMIUM=1`, a missing browser is a failure.

## Research points

1. Measure with a 37,674-row library in the real page (generate a CSV into the temp
   library): time from CSV loaded to first table paint, per-keystroke filter latency and
   scroll smoothness. Targets: first paint < 1.5 s, keystroke < 100 ms end to end. Record
   the numbers here.

## Verification

```text
node --test apps/starplayer-web/test/*.test.mjs
cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs
cargo xtask ci
```

Owner: `cargo xtask serve --library /mnt/d/hornet-music --library-index
/home/scott/projects/moddna/index/hornet/hornet-modules.csv`; search `format:xm ch:8`,
browse `songs/1995/`, shift-select a music disk's tracks and press `+`, double-click a
track and hear it.

## Out of scope

Auto-advance, the playback controller, playlist editing UI and the Repeat interaction
(W10); uploads, persistence and CSV import/export (W11).
