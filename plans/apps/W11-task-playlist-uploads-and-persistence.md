# W11 — Playlist uploads, persistence and CSV import/export

| Field | Value |
|---|---|
| Milestone | Web player: music library and playlist (W7–W11, branch `library-playlist`) |
| Status | Specified 2026-10-21; not started |
| Depends on | [W10](W10-task-playback-controller-and-playlist.md) |
| Blocks | — |
| Recommended model | gpt-6.1-sol |
| Verified by | agent (`node --test apps/starplayer-web/test/*.test.mjs`, `cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`, `cargo xtask ci`), then the owner in a browser |

## Context for a fresh agent

Read `CLAUDE.md` (do **not** commit), [W7](W7-task-library-index-and-member-lookup.md)
(Interfaces and locators), [W8](W8-task-library-sources-and-resolver.md) (sources,
resolver, the IndexedDB handle store), [W10](W10-task-playback-controller-and-playlist.md)
and `apps/starplayer-web/LIBRARY-CSV.md`, then the current `www/playlist.js`,
`www/track-resolver.js`, `www/library-index.js` and `app.js`.

The owner wants to "add extra tracks by manually uploading from the browser", and chose
to keep everything across a reload: library entries as index keys, uploaded files' bytes
in IndexedDB, and the playlist exportable/importable as CSV.

## Deliverables

### 1. Uploads into the playlist

* **Add files…** (`<input type="file" multiple>`, same `accept` list as the existing
  picker) and dropping files onto the Playlist panel.
* A module file → one blob, one entry (`{kind:'upload', blobId, containerIndex: -1,
  memberIndex: -1}`).
* A ZIP → one blob; list its playable members (existing `archive_modules`) and add one
  entry per member, located by **central-directory index** (`memberIndex`) with
  `memberPath` kept for display and diagnostics. A member that is itself a ZIP is
  expanded one level (`containerIndex` = its index, `memberIndex` = the inner index).
  Display names are never identifiers (the archive record list sanitises tabs/newlines).
* Extend `track-resolver.js` to resolve upload locators from the blob store via
  W7's `archive_extract_nested`, with the same size cap, copy-on-return and abort rules.

### 2. Persistence (`www/playlist-store.js`, IndexedDB)

* Database `starplayer-playlist`, versioned schema with an `onupgradeneeded` path from
  version 1; stores: `state` (entries in order, current entry **id**, loop flag), `blobs`
  (`{blobId, name, size, type, bytes: Blob, addedAt}`), `libraries` (known library
  identities `{sha256, name, sourceKind, label}`).
* Writes are transactional: adding an upload writes the blob and the updated state in one
  transaction. Blobs are **reference-counted** by the entries that use them; removing the
  last entry that references a blob deletes it.
* A 256 MiB cap on stored blob bytes: evict only **unreferenced** blobs; an upload that
  would exceed the cap with only referenced blobs left is refused with a message (the
  file can still be played once without being added).
* `QuotaExceededError`, IndexedDB unavailable (private mode, blocked) or a failed open:
  show one message and continue session-only; nothing throws out of the page.
* On load: restore entries and current entry; an upload entry whose blob is missing (the
  browser evicted it) is shown **missing — re-add** and skipped by playback.

### 3. Library entries across reloads and libraries

* A library entry resolves only against an open library whose identity (`sha256`) matches
  its `libraryIdentity`. With no matching library open the entry shows **library not
  connected** (with the library's name) and is skipped by automatic advance.
* When a library is open whose identity differs but whose CSV contains the entry's key
  (`sourcePath, containerMember, memberPath`), offer one-click **Rebind N entries to
  <library>** — never rebind silently. Rebinding updates the stored identity.

### 4. CSV export and import

* **Export playlist** downloads a CSV via W7's `toPlaylistCsv`: columns
  `source_path,container_member,member_path,member_path_raw_hex,title,format_detected,library`,
  order and duplicates kept, `library` = the identity name. Uploads are skipped with a
  message saying how many.
* **Import playlist** reads a CSV with `parsePlaylistCsv` and appends its entries bound to
  the open library (rows whose key is absent from it are reported and still added as
  unavailable, so a later library can satisfy them via Rebind). Because the format is a
  library CSV too, an exported playlist can also be opened as a small library.
* Document both in `LIBRARY-CSV.md` and the web README.

### 5. Tests

* `test/playlist-store.test.mjs`: run the store against a small in-repo in-memory
  IndexedDB shim (no npm), or — if a faithful shim is too costly — move these cases into
  `headless.mjs` against real Chromium IndexedDB. Cover: round trip of entries/current
  id; refcount delete; cap eviction of unreferenced blobs only; refusal over cap;
  quota/open failure → session-only; schema upgrade from version 1; missing blob state.
* Rebind logic and export/import as pure functions in `node --test`: A–B–A order and
  duplicates survive; non-ASCII member with raw hex survives; uploads excluded with count.
* `headless.mjs` library scenario extended: upload a module file and a ZIP with two
  modules into the playlist, reload the page, and assert the playlist is restored and the
  uploaded module **and both ZIP members actually play** (titles appear), and the library
  entries show *library not connected* until the server library is reopened.

## Verification

```text
node --test apps/starplayer-web/test/*.test.mjs
cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs
cargo xtask ci
```

Owner: add a music disk from the Hornet library plus a dropped file, reload, check both
play; export the playlist and re-import it.

## Out of scope

Syncing playlists between browsers or devices; storing library CSVs in IndexedDB (they
are re-read from the source).
