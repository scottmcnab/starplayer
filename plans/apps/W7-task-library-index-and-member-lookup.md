# W7 — Library CSV index and archive member lookup

| Field | Value |
|---|---|
| Milestone | Web player: music library and playlist (W7–W11, branch `library-playlist`) |
| Status | Implemented 2026-10-21 on `library-playlist` |
| Depends on | B8/B10 (`starplayer-archive`, landed) |
| Blocks | [W8](W8-task-library-sources-and-resolver.md), [W9](W9-task-library-view.md), [W10](W10-task-playback-controller-and-playlist.md), [W11](W11-task-playlist-uploads-and-persistence.md) |
| Recommended model | gpt-6.1-sol |
| Verified by | agent (`cargo test -p starplayer-archive -p starplayer-web`, `node --test apps/starplayer-web/test/library-index.test.mjs`, `cargo xtask ci`) |

## Context for a fresh agent

Read `CLAUDE.md` (no `cargo fmt`, full variable names, compact formatting, one-line
asserts, no `unsafe`, never `git add -A`; do **not** commit — the coordinator reviews and
commits). Then read `apps/starplayer-web/README.md`, `apps/starplayer-web/src/lib.rs`,
`crates/starplayer-archive/src/lib.rs` and its `tests/archive.rs`, and
`apps/starplayer-web/www/app.js:592-800` (the current load and ZIP path).

### The feature (W7–W11)

The web player today opens one ZIP at a time. W7–W11 let it browse and play a whole
music archive (for example a local mirror of the Hornet archive, 9.5k–37k modules) from a
CSV index, with a playlist that advances automatically. The CSV format comes from the
ModDNA project; its full column list is documented in
`/home/scott/projects/moddna/index/hornet/README.md` ("Columns") and a real example is
`/home/scott/projects/moddna/index/hornet/hornet-modules.csv` (9,585 rows; the header row
is reproduced below). Those files are third-party metadata: read them, never copy rows
into the repository.

```
source_path,container_member,member_path,entry_kind,extension,format_by_extension,size,compressed_size,crc32,timestamp,compression_method,encrypted,member_path_raw_hex,source_url,status,error,sha256,crc_verified,format_detected,format_variant,channels,title,tracker,instrument_slots,instruments,samples,sample_bytes,extended_status,extended_error
```

A row locates one module: a loose file (`member_path` blank), a ZIP member
(`source_path` is the ZIP, `member_path` the member), or a member of a nested ZIP
(`container_member` names the inner ZIP inside `source_path`).

W7 is the foundation: the Rust lookup, the pure-JS index module, the CSV contract
document, and the **interfaces** W8–W11 implement against (section "Interfaces" below —
W7 implements only `library-index.js`; the other modules are specified here so later
tasks agree).

### ZIP name decoding — why lookup compares raw bytes

ModDNA decodes member names as CP437 unless the entry's UTF-8 flag (general-purpose bit
11) is set, and records the raw bytes in `member_path_raw_hex` when the name is not plain
ASCII. The workspace's `zip` 9.0.5 (`Cargo.lock`) decodes differently: its `name()` tries
UTF-8 first regardless of the flag, then CP437
(`~/.cargo/registry/src/*/zip-9.0.5/src/types.rs:123`). An unflagged CP437 name whose
bytes happen to be valid UTF-8 therefore decodes differently in the two tools. Matching
must compare `ZipFile::name_raw()` against candidate byte strings, not decoded names.

## Deliverables

### 1. `starplayer_archive::find_entry`

```rust
pub fn find_entry(bytes: &[u8], name: &str, raw_name: Option<&[u8]>) -> Result<usize, ArchiveError>
```

Returns the central-directory index. Use raw (non-decompressing) entry access
(`by_index_raw`, as `list_modules` does since B10), skip directories and `__MACOSX`
entries, and match in this order, returning at the first stage that matches:

1. `raw_name` given → `name_raw() == raw_name`.
2. `name_raw()` equals `name.as_bytes()` (UTF-8) **or** the CP437 encoding of `name`
   (write a small `encode_cp437(&str) -> Option<Vec<u8>>` — the inverse of the standard
   CP437 table; `None` if a character has no CP437 code).
3. Case-insensitive comparison of the decoded `name()` with `\` treated as `/` — accepted
   only if **exactly one** entry matches; more than one → new
   `ArchiveError::AmbiguousEntry(String)` naming the candidates.

Nothing matches → `ArchiveError::NoSuchEntry`. Add the new variant's `Display` arm.

### 2. Wasm export `archive_extract_path`

In `apps/starplayer-web/src/lib.rs`:

```rust
pub fn archive_extract_path(bytes: &[u8], container_member: &str, member_path: &str, member_raw_hex: &str) -> Result<Vec<u8>, JsValue>
```

* `container_member` non-empty → `find_entry` it in `bytes`, `extract` it (bounded by
  `MAX_ENTRY_BYTES`), require `is_zip` on the result (else a clear error), then continue
  in the inner bytes.
* `member_raw_hex` non-empty → decode hex (reject odd length / non-hex with an error) and
  pass as `raw_name`; it applies to `member_path` only.
* `find_entry` the member, then `extract`.
* Also add `archive_extract_nested(bytes, container_index: i32, member_index: u32)` —
  `container_index < 0` means no container — for uploads, which locate members by index
  (W11). Keep both exports thin over a plain Rust function with unit tests, as the file's
  existing `archive_module_records` does.

### 3. `apps/starplayer-web/www/library-index.js` (pure ES module, no DOM)

* `parseCsv(text)` — RFC 4180: quoted fields, doubled quotes, embedded commas and
  newlines, LF or CRLF, optional UTF-8 BOM, a trailing newline or not. One pass over the
  string with index scanning; no per-row regex, no `split` of the whole file into lines.
* `parseLibraryCsv(text, {name})` →
  `{ rows, issues, columns, identityName }`.
  * Header names trimmed and lower-cased; any order; unknown columns ignored; a duplicated
    known header → throw an `Error` naming it; missing `source_path` → throw.
  * Row filtering: skip `entry_kind == 'directory'`; skip rows whose `status` or
    `extended_status` is present, non-blank and not `ok`.
  * Path validation (`source_path`, `container_member`, `member_path` where non-blank):
    reject absolute paths, a drive letter (`C:`), `.` or `..` segments, backslashes, and a
    blank `source_path`; push `{line, reason}` to `issues` and skip the row.
  * Duplicate `(source_path, container_member, member_path)` key → issue, first wins.
  * Each row is normalised into a compact record:
    `{ id (0-based row index into rows), sourcePath, containerMember, memberPath,
    rawHex, name (last path segment of the innermost path), folder (the containing
    directory path, with a ZIP and a nested ZIP each treated as a folder level),
    title, format, playable, channels (number|null), size (number|null), timestamp,
    tracker, line }`.
  * `format` = first non-blank of `format_detected`, `format_by_extension`, the lower-case
    extension of the innermost path. `playable` = format in `mod s3m mtm xm it`.
  * Build a lower-cased search haystack per row once at parse time (name, title, folder
    path, format, tracker), stored in a parallel array, not re-built per search.
* `libraryIdentity(bytes, name)` — async; SHA-256 (`crypto.subtle.digest`) hex of the CSV
  bytes plus the name: `{ sha256, name }`. Works in Node ≥ 20 and browsers.
* `compileQuery(text)` and `search(library, query, {scopeFolder, showUnplayable, format,
  sort: {column, descending}})` → `Int32Array` of row ids.
  * Whitespace-separated tokens ANDed; `"quoted phrases"`; case-insensitive substring
    match against the haystack.
  * Field prefixes: `format:`, `title:`, `name:`, `path:` (folder or full path),
    `tracker:`, `ch:` (`ch:8` exact, `ch:>8`, `ch:<=4`).
  * `scopeFolder` restricts to rows whose folder equals it or is beneath it.
  * Sort columns: `name`, `title`, `format`, `channels`, `size`, `timestamp`, `folder`;
    stable, with `folder` then `name` as tie-breaks. Use a precomputed collation key
    rather than `localeCompare` per comparison.
* `buildTree(library)` → nested nodes `{ path, label, kind: 'dir'|'zip'|'nested',
  children, trackCount (playable, recursive) }`.
* Playlist CSV:
  * `parsePlaylistCsv(text)` — same parser and header rules, but **ordered and keeping
    duplicates**; returns `{ entries: [{ sourcePath, containerMember, memberPath, rawHex,
    title, format, library }], issues }`. The `library` column (optional) holds the
    originating library's identity name.
  * `toPlaylistCsv(entries)` — writes the header
    `source_path,container_member,member_path,member_path_raw_hex,title,format_detected,library`,
    quoting as RFC 4180 requires. Round-trips with `parsePlaylistCsv`.

### 4. `apps/starplayer-web/test/library-index.test.mjs` (`node --test`, no npm)

* The real Hornet header with a few synthetic rows (write them by hand — do not copy rows
  from the ModDNA file); shuffled column order; a minimal two-column CSV
  (`member_path,source_path`); quoting edge cases including a multi-line `title`; BOM and
  CRLF; each invalid-path case; duplicate key; duplicate header; missing `source_path`;
  status filtering; format fallback order; search prefixes, phrases, `ch:` comparisons,
  scope and sort; `buildTree` counts with a nested ZIP; an A–B–A playlist round trip
  including a non-ASCII member with `member_path_raw_hex`.
* **Scale budget.** Generate a 37,674-row CSV in the test (deterministic pseudo-random
  paths/titles, some quoted multi-line fields) and assert, on this machine: parse +
  normalise < 1000 ms; one `search` with a two-token query < 50 ms; a sort by title
  < 300 ms; `buildTree` < 300 ms. Print the measured times.
* If `/home/scott/projects/moddna/index/hornet/hornet-modules.csv` exists, parse it and
  assert 9,585 rows and zero issues; skip (with a message) otherwise.

### 5. Rust tests

In `crates/starplayer-archive/tests/archive.rs` (and unit tests in `starplayer-web` for
the export wrappers). Build ZIPs in memory with the `zip` writer where it can express the
case; hand-patch the name bytes and clear flag bit 11 in both local and central headers
where it cannot. Cover: exact UTF-8 match; an **unflagged** name whose raw bytes are valid
UTF-8 but were meant as CP437 (for example bytes `0xC3 0xA9`, which CP437 reads as `├⌐`),
matched via its CP437 decoding and via `raw_name`; a nested ZIP whose own name in the
outer archive is such a name; case-insensitive unique match; ambiguous match
(`A.MOD` and `a.mod`); missing member; directory skipped; nested container that is not a
ZIP; oversized nested container (declared size > `MAX_ENTRY_BYTES`) rejected.

### 6. `apps/starplayer-web/LIBRARY-CSV.md`

The public contract, written for someone building a CSV by hand without ModDNA. Link it
from `apps/starplayer-web/README.md`. Contents:

* Encoding and syntax (UTF-8, BOM allowed, RFC 4180, LF/CRLF, header row required, names
  case-insensitive, any order, unknown columns ignored, duplicate header is an error).
* **Required column:** `source_path` — relative to the library root, `/` separators; what
  makes a path invalid.
* **Locator columns** (optional): `member_path`, `container_member`,
  `member_path_raw_hex` (hex of the exact name bytes; when to supply it).
* **Display/filter columns** (optional): `title`, `format_detected`,
  `format_by_extension`, `extension`, `channels`, `tracker`, `size`, `timestamp`,
  `instruments`, `samples`, `sha256` — what each is used for.
* **Row filtering**: `entry_kind`, `status`, `extended_status`.
* Format resolution order and the playable set (MOD, S3M, MTM, XM, IT).
* Row key and duplicate handling; library identity (SHA-256 of the CSV bytes + its name;
  editing the CSV changes the identity — playlists then offer to rebind, W11).
* Where the CSV goes: the library root as `index.csv` is found automatically; otherwise it
  can be opened separately (W8).
* A minimal example (three rows: a loose module, a ZIP member, a nested member) and a
  note that ModDNA's `hornet_index.py` output loads unchanged.
* The playlist CSV format (same rules, ordered, duplicates kept, `library` column).

## Interfaces (fixed here; W8–W11 implement them)

* **`library-source.js`** (W8): a source is
  `{ kind: 'directory'|'filelist'|'http', label, readFile(path, {signal}) →
  Promise<ArrayBuffer>, size(path) → Promise<number|null> }`. `path` is a validated
  library-relative POSIX path. Errors are `Error`s whose message names the path.
* **`track-resolver.js`** (W8): `createResolver({ source, loader, cacheBytes })` →
  `{ resolve(locator, {signal}) → Promise<ArrayBuffer>, prefetch(locator), clear() }`.
  The buffer returned by `resolve` is a **fresh copy owned by the caller** (activation
  transfers it to the worklet). Cached archive bytes are immutable and never transferred.
* **Locators** (shared by W8–W11):
  * `{ kind: 'library', libraryIdentity: {sha256, name}, sourcePath, containerMember,
    memberPath, rawHex }`
  * `{ kind: 'upload', blobId, containerIndex (‑1 = none), memberIndex (‑1 = the blob is
    the module), memberPath (display/diagnostics only) }`
* **`playlist.js`** model (stub in W9, full in W10): `createPlaylist()` →
  `{ entries, add(items, {at}), remove(ids), move(id, toIndex), clear(), current,
  setCurrent(id), next(), previous(), subscribe(listener) }`; entries are
  `{ id (crypto.randomUUID()), locator, display: {name, title, format, folder} }`.
* **`playback-controller.js`** (W10): the single owner of what plays —
  `play(locator, {origin: 'library'|'playlist'|'file'|'fixture'|'url'|'archive', entryId})`
  claims an intent token before any I/O, `stop()`, `onTrackEnded(listener)`,
  `effectiveAtEnd()`, `playlistMode`.

## Research points

1. Confirm `by_index_raw` gives `name_raw()` without touching compressed data for every
   method B10 supports, and that `is_dir()` is available there.
2. Measure the scale budgets above; if parse misses 1000 ms, profile before restructuring
   (likely culprits: string concatenation per field, per-row object churn).

## Verification

```text
cargo test -p starplayer-archive -p starplayer-web
node --test apps/starplayer-web/test/library-index.test.mjs
cargo xtask ci
```

Report the measured scale timings and the Hornet-file parse result in the final summary.

## Out of scope

UI, sources, the dev server and playback (W8–W10); persistence (W11); 669/FAR/ULT/MED
playback; fetching `source_url`.

## Research-point results

1. Confirmed against zip 9.0.5: `read/zip_archive.rs::by_index_raw` constructs a
   `ZipFileReader::Raw` with borrowed name bytes. `types.rs::find_content` reads the
   fixed local header to locate the payload and returns a bounded raw reader; it
   neither reads compressed payload bytes nor constructs a decompressor.
   `read/zipfile.rs` exposes both `name_raw()` and `is_dir()` on this entry. The new
   lookup regression passes for methods 0, 1, 2, 3, 4, 5, 6 and 8 with deliberately
   invalid payload streams, covering every B10-supported compression method.
2. Final standalone Node run on this machine (37,674 synthetic records): parse and
   normalise **223.5 ms** (<1000), two-token search **3.7 ms** (<50), title sort
   **25.0 ms** (<300), tree construction **30.6 ms** (<300). No profiling or parser
   restructuring was necessary. The local Hornet CSV parsed read-only as **9,585
   rows, zero issues**, **77.4 ms** including the file read. No metadata rows were
   copied into the repository; fixtures and scale records are synthetic.

## Implementation results

Implemented W7's archive lookup, page-thread extraction exports, pure JavaScript
library/playlist data layer, synthetic regression/scale tests, and public CSV
contract linked from the web README. Existing app load/UI behavior is unchanged;
W8–W11's specified interfaces remain the downstream integration boundary.

Design decisions:

* Lookup scans raw entries and retains the first raw-byte and encoded-byte matches
  independently, so a later explicit raw match wins over an earlier UTF-8/CP437
  candidate. Only the decoded case/separator fallback requires uniqueness.
* Both extraction exports delegate to plain Rust functions. Inner ZIP extraction
  uses the existing size-limited extractor and validates the inner ZIP signature.
  Raw hex is validated before decoding and applies only to the final member.
* CSV scanning preserves embedded field text and records physical row-start lines.
  Browse ids are contiguous after filtering and deduplication. Search haystacks and
  lower-case lexical sort keys are parallel arrays computed once during parsing.
  Numeric nulls sort first ascending; ties use folder, name, then original row id.
* `buildTree` returns a root node (`path: ''`) with recursive playable counts and
  explicit ZIP/nested-ZIP nodes. Playlist import retains duplicate locators and
  their order. Library identity hashes exact CSV bytes followed by UTF-8 name bytes
  and returns `{sha256, name}`; these choices are documented in `LIBRARY-CSV.md`.

Final verification, builds run sequentially:

```text
cargo test -p starplayer-archive -p starplayer-web
  archive: 20 passed, 0 failed, 1 existing ignored manual oracle test
  web:     11 passed, 0 failed
  doc-tests: passed
node --test apps/starplayer-web/test/library-index.test.mjs
  11 passed, 0 failed, 0 skipped (timings and Hornet result above)
cargo xtask ci
  xtask ci: 11 job(s) passed
```

The initial CI run lacked the pinned libxmp corpus cache, so conformance and
real-time safety could not run. `cargo xtask conformance --fetch-only` downloaded
and checksum-verified it under this worktree's `target/`; the complete CI rerun
passed, including both previously blocked jobs. Verification logs are retained
under `target/w7-verification/` (ignored build artifacts).

No W7 implementation items remain open. UI, sources, resolver, playback and
persistence remain W8–W11 work. No staging, commits, formatting runs, unsafe blocks
or npm dependencies were introduced. ModDNA and the Hornet mirror were not modified.
