# W11 — Playlist uploads, persistence and CSV import/export

| Field | Value |
|---|---|
| Milestone | Web player: music library and playlist (W7–W11, branch `library-playlist`) |
| Status | Implemented 2026-10-21 on `library-playlist`; owner browser check outstanding |
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

## Research-point results

W11 specifies no separate research questions. Implementation checks established:

* `archive_modules` intentionally returns playable modules only, omitting ZIP
  containers. A separate `list_zip_containers` / `archive_containers` API now lists
  immediate inner ZIPs by central-directory index without decompressing them;
  the existing module-list and extraction interfaces are unchanged. Resource
  forks and directories are skipped; supported-method checks precede extraction.
* Real Chromium 156 IndexedDB passes state/current-id and Blob-byte round trips,
  shared-reference retention, last-reference deletion, orphan eviction, atomic
  over-cap refusal, missing bytes, and upgrade from a seeded version-1 database.
  Tests use a 4-byte cap to exercise boundaries without filling browser storage.
  Unavailable IndexedDB, failed open, and an injected `QuotaExceededError` all
  preserve session use. The fallback notice fires once per store instance.
* The final Node run measured 37,674 synthetic records: parse/normalise **273.5 ms**,
  search **3.9 ms**, title sort **26.1 ms**, tree **29.4 ms**. All budgets passed.
  The existing read-only Hornet compatibility check found **9,585 rows, zero
  issues**, **118.0 ms** including read; no external metadata was copied or modified.
* A browser test exposed a stale-telemetry race after the W10 sample-rate rebuild:
  checking only `playing` could seek using the previous graph's slider length.
  The harness now waits for generation 1 on the rebuilt graph and a painted slider
  matching its length. It still seeks without pressing Play, preserving coverage
  of automatic advance after a user seek. Only Stop suppresses advance until Play.

## Implementation results

Implemented W11 in this worktree, without staging or committing.

Files and responsibilities:

* New `www/playlist-store.js`: version-2 `starplayer-playlist` database with
  `state`, `blobs`, and `libraries` stores; version-1 upgrade; ordered state,
  current entry id and loop restoration; queued atomic blob/state writes; derived
  entry references, last-reference deletion, 256 MiB cap, orphan cleanup, and
  session-only recovery from storage failures. Blob objects are retained without
  reading their bytes during restoration.
* New `www/playlist-controls.js`: multi-file input and Playlist-panel drops,
  shared ZIP blobs, one nested ZIP level, storage subscription, missing/disconnected
  diagnostics, explicit rebind action, CSV download/import and user messages.
  Upload writes are queued before publishing new ids so later edits use the same
  ordered state; cap refusal rolls back only the refused entries.
* New `www/playlist-transfer.js`: pure key matching, rebind selection/application,
  export with upload exclusion count, and import preserving duplicate order while
  reporting absent keys and CSV issues. W7 remains the CSV parser/writer.
* `www/playlist.js`, `www/playlist-playback.js`, `www/track-resolver.js`: model
  restoration, diagnostic text, skipping unavailable entries without consuming
  the load-failure budget, bounded traversal when all entries are unavailable,
  and upload resolution by numeric indices with size checks, abort checks and
  caller-owned copies. Upload prefetch is explicitly routed, including when no
  library is connected.
* `www/app.js`, `www/index.html`: small integration changes and playlist controls;
  logic lives in the new modules. Upload enumeration waits for loader readiness.
  Existing playback-controller interfaces and seek/Stop policy are preserved.
* `crates/starplayer-archive/src/lib.rs`, its `tests/archive.rs`, and
  `apps/starplayer-web/src/lib.rs`: the thin container-index listing API and its
  regression test, including non-module directory slots and resource filtering.
* New `test/playlist-store.test.mjs` runs its exported checks in `headless.mjs`
  against real Chromium IndexedDB, avoiding an incomplete shim or npm dependency.
  New `test/playlist-transfer.test.mjs` covers A–B–A, Unicode/raw hex, upload
  exclusion counts, missing import keys and explicit rebinding.
  Existing resolver/controller tests cover upload copies, limits, aborts, missing
  blobs, unavailable skipping and termination. `test/headless.mjs` covers module
  input, dropped two-member ZIP, reload/current-id/loop persistence, actual playback
  of all three restored uploads, library disconnection/reconnection, and a dropped
  nested ZIP that actually plays. Only REFLEX.S3M and PETRI.S3M are music fixtures.
* `LIBRARY-CSV.md` and the web README document persistence, uploads, storage limits,
  session-only recovery, explicit identity rebinding and CSV exchange.

Final verification, with builds and browser runs kept serial:

* `node --test apps/starplayer-web/test/*.test.mjs`: **47 passed, 0 failed,
  0 skipped**. IndexedDB checks are additionally executed by the browser harness.
* `cargo xtask wasm`: passed; packaged all new modules and the two permitted S3M
  fixtures.
* `STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`:
  **all modes passed** (SAB, forced fallback, plain HTTP), including the complete
  existing playback/UI suite and the extended upload/persistence scenario.
  Real IndexedDB checks passed independently in all three modes. Library first
  paint for 37,674 rows was **134.7 / 127.4 / 129.6 ms** respectively, with at most
  **20 mounted track rows** and average scroll frames **16.61–16.62 ms**.

* `cargo xtask ci`: **11 jobs passed**, exit 0: host-tests, conformance,
  rt-safety, goldens, simd, fma-check, trace-zero-cost, wasm-build, no-std-check,
  clippy, no-std-purity. The new archive-index test passed; no new conformance
  failures were introduced.
* `git diff --check`: passed. Nothing staged; no commit created. No `cargo fmt`,
  unsafe blocks or npm dependencies introduced. Historical sources, the Hornet
  mirror and ModDNA were not modified. No other plan was changed.

Final logs: `target/w11-node.log`, `target/w11-wasm.log`,
`target/w11-headless.log`, `target/w11-ci.log`. The intermediate focused browser
checks are in `target/w11-library-debug.log` and `target/w11-fallback-debug.log`;
final acceptance evidence is the complete successful three-mode headless log.

No W11 implementation items remain open. Owner browser acceptance remains: add a
Hornet music disk and a dropped file, reload/reconnect and check playback, then
export and re-import the playlist. Actual browser storage eviction and disk
exhaustion are represented by deterministic missing-blob and injected quota tests,
not by filling or clearing the owner's browser storage.
