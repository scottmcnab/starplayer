# W9 — The library view: search, folders, selection, play and add

| Field | Value |
|---|---|
| Milestone | Web player: music library and playlist (W7–W11, branch `library-playlist`) |
| Status | **Complete** — landed on `main` 2026-10-23; owner-checked on the Hornet mirror 2026-10-23 after the tree and selection follow-ups |
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

## Research-point results

Measured in the real packaged page with Chrome 156.0.8078.4, using a generated
37,674-row CSV served through `--library`. The headless scenario replaces its temp
index and reopens Server library, so parsing, identity hashing and rendering all
use the normal source-control path. Generated libraries live under this worktree's
ignored `target/w9-verification/` and are removed after each scenario.

| Transport | CSV loaded → first table paint boundary | Input → filtered paint, five successive inputs | Scroll frame mean / maximum | Maximum rendered track rows |
|---|---:|---:|---:|---:|
| Shared memory | 119.2 ms | 89.0, 88.3, 94.0, 84.2, 84.0 ms | 16.61 / 19.34 ms | 20 |
| Forced postMessage | 120.8 ms | 90.2, 93.7, 85.9, 95.7, 84.1 ms | 16.62 / 21.94 ms | 20 |
| No isolation | 119.6 ms | 88.6, 89.4, 94.0, 84.2, 83.7 ms | 16.62 / 21.00 ms | 20 |

Timing starts when the CSV response's `arrayBuffer()` resolves, and ends at the
next animation-frame boundary after the table DOM is populated. Filter timings
start at each input event and include the 80 ms debounce, search, sorting and the
next animation-frame boundary. The inputs are `T`, `TR`, `TRA`, `TRACK3`, and
`TRACK37`, including queries matching the whole library. Scroll measurements use
120 successive animation frames with 1,000-pixel scroll steps; the mean is near
60 Hz with no observed long frame. These are headless frame-cadence measurements,
not a substitute for subjective owner/device testing. All first-paint and filter
measurements meet the 1.5 s and 100 ms targets.

## Implementation results

Implemented W9 without staging or committing. W7's index/source/locator contracts
and W8's resolver are retained. The task header and other plans were left untouched
as requested; only these results sections were appended.

Files and responsibilities:

* `www/library-view.js`: collapsible library browser wiring, debounced query and
  format/unplayable filtering, lazy folder/ZIP/nested-ZIP expansion, folder scope,
  sortable column headers, virtual grid, keyboard/mouse selection, play/add actions
  and playing-row reveal/highlight.
* `www/library-selection.js`: DOM-free selection transitions and bounded virtual
  window arithmetic, shared by the UI and Node regression tests.
* `www/playlist.js`: the complete W7 in-memory model interface, UUID entry ids,
  library locator construction and the minimal visible name/count list.
* `www/index.html`, `www/style.css`: Library disclosure, toolbar, hints, folder pane,
  accessible grid and playlist shell. Track rows are fixed at 32 px, with five-row
  overscan; horizontal table scrolling is contained inside the panel.
* `www/library-controls.js`: optional `onLibrary` notification after successful
  state replacement; existing callers and source state shape remain compatible.
* `www/app.js`: small exported `playLibraryRow` seam, early revision claiming,
  activation guards, current-row highlight updates and startup wiring. Library
  keyboard navigation is excluded from the live-input music keyboard.
* `test/library-view.test.mjs`: click/toggle/range/select-all, hidden selection
  preservation, keyboard extension/contraction, empty and 37,674-row virtual
  windows, plus playlist ordering, duplicates, navigation and subscriptions.
* `test/headless.mjs`: each existing mode also exercises a generated library with
  loose, ZIP and nested-ZIP REFLEX/PETRI fixtures and a missing member, plus the
  scale measurements, sort/keyboard/ARIA checks and unplayable metadata controls.
  It uses real DevTools double-clicks and checks the library at an actual 390 px
  viewport, not just against a potentially widened mobile `innerWidth`.
  `STARPLAYER_REQUIRE_CHROMIUM=1` makes missing Chromium fail. The known startup
  404 for W8's optional `/library-index.csv` probe is excluded specifically from
  console-error assertions; other console errors still fail. `--library-only`
  permits a focused rerun without the existing timed audio scenarios.
* `README.md`: query syntax, selection/keyboard actions, hidden-selection rule,
  folder additions, playlist lifetime and required-Chromium switch.

Design decisions:

* Selection and anchor/focus use stable row ids, never rendered positions. Sorting,
  filtering and scoping preserve selection. Add actions intersect it with the
  current result and playable rows, retaining result order. Enter also refuses a
  focused row that has become hidden. A row's **+** adds the visible selection,
  selecting that row first if necessary. Folder additions ignore the query and
  use recursive playable rows in folder/name order.
* The grid is one focusable row-navigation widget. `aria-activedescendant` always
  references a currently rendered row, and keyboard navigation reveals it. Click
  selection updates existing DOM nodes so native double-clicks remain reliable.
  Scroll rendering is coalesced to animation frames and capped at 200 rows even
  for unusually large viewport inputs; the actual 320 px viewport rendered 20.
* `playLibraryRow` claims `moduleRevision` before loader/resolver I/O and passes it
  through activation rather than acquiring a later, misleading revision. Stale
  resolves and errors cannot replace newer intent. A failed archive lookup never
  activates bytes, so the previous module keeps playing. Changing library sources
  also prevents the old request from publishing a highlight into the new view.
* Playlist duplicates are deliberate. `current` is an entry or null; navigation
  returns the adjacent entry (or null at a boundary), without wrapping. Removing
  the current entry clears it; subscriptions return an unsubscribe function.
  W10 owns playback/auto-advance/repeat policy, and W11 owns persistence.

Final verification, with builds and headless runs executed sequentially:

```text
node --test apps/starplayer-web/test/*.test.mjs
  28 passed, 0 failed, 0 skipped
cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs
  WASM package built; headless harness: all modes passed
  sab, fallback, plain: existing audio checks and new library scenarios passed
  missing-member error named MISSING.S3M; Petrified stayed loaded and advancing
  strict library phone check: viewport 390 px, document width <= 390 px
cargo xtask ci
  xtask ci: 11 job(s) passed
  host-tests, conformance, rt-safety, goldens, simd, fma-check,
  trace-zero-cost, wasm-build, no-std-check, clippy, no-std-purity
git diff --check
  passed
```

Full verification logs are retained under `target/w9-verification/` (ignored).
Only REFLEX.S3M and PETRI.S3M supply module bytes for the new scenarios; scale and
unsupported-format rows are generated metadata, not additional module fixtures.
The Hornet mirror and ModDNA were not modified. No formatting run, unsafe code,
npm dependency, staged file or commit was introduced.

No W9 implementation items remain open. The owner's interactive Hornet-library
acceptance check remains outstanding. W10's playback controller, auto-advance,
full playlist editing and Repeat behavior, and W11's persistence/import/export,
remain intentionally out of scope.

## Owner review follow-up (2026-10-23)

First browser check against the Hornet mirror:

- Shift-click in the track table extended a page text selection across the rows, and Edge
  popped up its Copy/Search mini menu. Rows, the tree and the playlist are now
  `user-select: none`, and a modified or repeated mousedown on a row is default-prevented
  (the click handler still focuses the grid). The headless scenario Shift-clicks with real
  CDP mouse input and asserts the page selection stays empty; it fails without the fix.
- The folder pane rendered every node as three buttons. It is now a WAI-ARIA `tree` beside
  the table (stacked below 820 px): one tab stop, ↑/↓/Home/End move, →/← expand, collapse
  or go to the child or parent, Enter/Space/click scopes the table, ▸ or double-click
  toggles, and `+`/Insert or the row's hover **+** adds the folder. An "All folders" root
  item replaces the separate All button. Only expanded nodes are rendered (the Hornet tree
  has 8,797 nodes, at most 306 under one folder).
- Previous/next *track* moved from the playlist card to either end of the transport
  controls as ⏮/⏭ icon buttons (keeping the `playlist-previous`/`playlist-next` ids); they
  are disabled while the playlist is empty.
