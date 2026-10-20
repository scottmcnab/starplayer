# W8 — Library sources, track resolver and the dev-server library route

| Field | Value |
|---|---|
| Milestone | Web player: music library and playlist (W7–W11, branch `library-playlist`) |
| Status | Specified 2026-10-21; not started |
| Depends on | [W7](W7-task-library-index-and-member-lookup.md) (interfaces, `library-index.js`, `archive_extract_path`) |
| Blocks | [W9](W9-task-library-view.md), [W10](W10-task-playback-controller-and-playlist.md), [W11](W11-task-playlist-uploads-and-persistence.md) |
| Recommended model | gpt-6.1-sol |
| Verified by | agent (`node --test apps/starplayer-web/test/*.test.mjs`, `cargo xtask wasm`, `cargo xtask ci`) |

## Context for a fresh agent

Read `CLAUDE.md` (no `cargo fmt`, full variable names, compact formatting; do **not**
commit — the coordinator reviews and commits). Then read
[W7](W7-task-library-index-and-member-lookup.md) in full — especially **Interfaces**,
which this task implements for `library-source.js` and `track-resolver.js` — plus
`apps/starplayer-web/LIBRARY-CSV.md`, `apps/starplayer-web/www/library-index.js`,
`apps/starplayer-web/dev-server.mjs`, `xtask/src/main.rs` (`run_serve`, ~line 2522) and
`apps/starplayer-web/www/app.js` (start-up, `populateFixtureMenu` ~line 2097 — the pattern
for a feature that hides itself when the server does not offer it).

The owner's library is a local mirror at `/mnt/d/hornet-music` (WSL; Windows sees it as
`D:\hornet-music`, and the browser runs on Windows). A browser cannot read a path, so the
page reaches the files one of three ways, each an implementation of W7's
`LibrarySource`. This task provides the sources, the resolver that turns a library
locator into module bytes, and the dev-server route. It adds **no visible library UI**
beyond the minimal source controls described below; W9 builds the browser on top.

## Deliverables

### 1. `apps/starplayer-web/www/library-source.js`

* **Directory handle** (`kind: 'directory'`), File System Access API.
  * `openDirectorySource()` must be called directly from a click handler:
    `showDirectoryPicker({ mode: 'read' })` runs **first**, before any await on other
    work, because it needs transient user activation and a secure context. Feature-detect
    (`'showDirectoryPicker' in window && isSecureContext`).
  * `readFile(path)` walks `getDirectoryHandle` per segment then `getFileHandle(...).getFile()`.
    `size(path)` uses the `File`'s `size`.
  * Find the index: `index.csv` at the root (case-insensitive), else the only `*.csv`
    there, else return the list so the caller can ask the user to choose.
  * Persist the handle in IndexedDB (database `starplayer-library`, store `handles`,
    versioned). On start-up, if a stored handle exists, the page shows **Reconnect
    <name>**; the click calls `handle.requestPermission({ mode: 'read' })` and proceeds only
    on `'granted'`. Handle `AbortError` (cancel) and denial with a message, not an
    exception.
* **FileList** (`kind: 'filelist'`), `<input type="file" webkitdirectory>`, for browsers
  without the picker (Firefox, Safari).
  * Build a `Map(relativePath → File)` by removing **exactly the first component** (the
    selected folder's own name) from each `webkitRelativePath`. Do not read any file
    while building the map. Locate the CSV the same way as above.
  * Not reconnectable after a reload; the doc says the folder must be chosen again.
* **HTTP** (`kind: 'http'`): `readFile(path)` fetches `/library/` + the path with **each
  segment** `encodeURIComponent`-encoded (names contain spaces, `#`, `%`, `?`), using the
  `signal`. `size(path)` uses a `HEAD` request's `Content-Length`.
* **Open index CSV…**: read a CSV chosen with a plain file input, then ask for the library
  root with a separate click (directory picker or webkitdirectory), because the file
  picker's activation has been consumed.
* Every source reports a `label` (folder name or `server library`).

### 2. `apps/starplayer-web/www/track-resolver.js`

`createResolver({ source, loader, cacheBytes = 64 MiB, maxFileBytes = 128 MiB })` per
W7's interface, where `loader` is the page-side wasm module (`Loader` in `app.js`).

* `resolve(locator, {signal})`: library locators only (upload locators are W11's and
  should throw a clear "not supported by this resolver" until then).
  * Check `source.size(sourcePath)` against `maxFileBytes` **before** reading.
  * Loose file (`memberPath` blank) → read and return it.
  * Otherwise get the archive bytes from the cache or `source.readFile`, then
    `loader.archive_extract_path(bytes, containerMember, memberPath, rawHex)`, and return
    a fresh `ArrayBuffer` of the result.
  * The cache is an LRU keyed by `sourcePath`, holding immutable `Uint8Array`s, capped at
    `cacheBytes` total; an archive larger than the cap is used once and not cached. Never
    hand a cached buffer to a caller.
  * At most one read-and-extract runs at a time (a simple promise queue);
    `signal.aborted` is checked after each await and throws an `AbortError`.
  * Error messages name the row (`sourcePath` and member) and the cause: file missing,
    member missing (`archive entry does not exist`), ambiguous member, unsupported method,
    too large.
* `prefetch(locator)`: warm the archive cache only; never returns bytes; swallows errors.
* `clear()`: drop the cache (called when the source changes).

### 3. Dev-server library route (`dev-server.mjs`) and `cargo xtask serve`

* New options `--library <dir>` and `--library-index <file>` (default
  `<dir>/index.csv`). Both optional; without `--library` nothing changes.
* `GET`/`HEAD /library-index.csv` → that one file (an explicit mapping, not a directory),
  `text/csv; charset=utf-8`. 404 when `--library` is not given.
* `GET`/`HEAD /library/<path>` → files under `<dir>`, read-only, any other method 405.
  **Confinement:** decode the path per segment inside `try` (malformed percent-encoding
  → 400), reject empty, `.` and `..` segments and NUL, join under the root, then
  `realpath` both the root (once at start-up) and the target and require the target to be
  the root or start with `root + sep`. This rejects symlink escapes and prefix siblings
  (`/mnt/d/hornet-music-other`). Not a regular file → 404.
* Harden the existing document-root route the same way: move `decodeURIComponent` inside
  the `try` so a malformed URL gets a 400 rather than an unhandled rejection.
* Same COOP/COEP/CORP headers as every other response. Log the library root at start-up.
* `xtask/src/main.rs` `run_serve` (and `run_wasm --serve` if it forwards serve options):
  accept `--library <dir>` and `--library-index <file>`, pass them through, and mention
  them in the usage text. Do not change other behaviour.
* Page start-up: `HEAD /library-index.csv`; on 200 offer the **server library** source,
  on anything else hide it (as `populateFixtureMenu` does).

### 4. Minimal page wiring

A small "Library" source row in the load panel of `index.html` with **Open library
folder**, **Reconnect …** (hidden unless a stored handle exists), **Open index CSV…** and
**Server library** (hidden unless offered). Opening a source parses the CSV with
`parseLibraryCsv` and stores `{ source, library, identity, resolver }` on the page state,
then shows a one-line summary (`9,585 tracks, 0 issues, from hornet-music`) and the first
few issues if any. W9 replaces the summary with the browser. Keep `app.js` changes to
this wiring; put the logic in the new modules.

### 5. Tests (`node --test`, no npm)

* `test/dev-server-library.test.mjs`: start `dev-server.mjs` on an ephemeral port with a
  temp library dir and assert: a file is served (GET and HEAD, correct length); a name with
  a space, `#` and `%` (segment-encoded) is served; `/library/../dev-server.mjs`,
  `%2e%2e%2f`, and `%2F`-smuggled traversal are refused; a symlink inside the library
  pointing outside it is refused; a prefix-sibling directory is unreachable; malformed
  `%zz` → 400; missing → 404; POST → 405; `/library-index.csv` served; without
  `--library` both routes 404.
* `test/library-source.test.mjs`: FileList path mapping with fake `File`-like objects
  (root component stripped exactly once, nested folders, CSV discovery: `index.csv`,
  single CSV, multiple CSVs).
* `test/track-resolver.test.mjs`: with a fake source and a fake loader — LRU eviction by
  bytes, oversize bypass, size cap checked before read, returned buffers are distinct
  copies, abort after read throws `AbortError`, queue serialises, error messages name the
  row.

## Research points

1. Confirm `FileSystemDirectoryHandle` objects survive IndexedDB structured clone in
   Chromium and that `queryPermission`/`requestPermission` behave as documented on
   reload. Note the result in this file.
2. Check whether `showDirectoryPicker` is usable from the cross-origin-isolated dev page
   (COOP/COEP) — it should be; record it.

## Verification

```text
node --test apps/starplayer-web/test/*.test.mjs
cargo xtask wasm
cargo xtask ci
```

Then, by hand if a browser is available: `cargo xtask serve --library /mnt/d/hornet-music
--library-index /home/scott/projects/moddna/index/hornet/hornet-modules.csv` and confirm
the summary line reads 9,585 tracks. Never write into `/mnt/d/hornet-music`.

## Out of scope

The library table/tree/search UI (W9), playing tracks and the playlist (W10), uploads and
persistence of the playlist (W11), fetching `source_url`.
