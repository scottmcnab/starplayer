# W15 — Folder libraries without an index CSV

| Field | Value |
|---|---|
| Milestone | Web player: music library |
| Status | Specified — implementation not started |
| Depends on | W7–W13 library, resolver, view and playlist work (in `complete/`) |
| Role | Behavioral specification; execute through [W15 master](W15-master-plan.md) |
| Coordinator | gpt-6.1-sol, xhigh effort |
| Workers | gpt-6.1-sol, high effort; eight tasks in master |
| Branch | `library-folder-plan` (planning only) |
| Verification | Web unit/headless tests, archive/web Rust tests, WASM packaging, repository CI |

This document retains the reviewed product behavior. The [master plan](W15-master-plan.md)
splits execution into eight tasks, fixes ownership and shared interfaces, and defines
coordinator review gates. Do not delegate this entire specification as one worker task.

## Context for a fresh agent

Read `AGENTS.md`, `apps/starplayer-web/LIBRARY-CSV.md`, and the completed W7–W13
plans. Relevant implementation is in `apps/starplayer-web/www/`: `library-source.js`,
`library-controls.js`, `library-index.js`, `library-view.js`, `track-resolver.js`,
`playlist.js`, `playlist-transfer.js`, `playlist-controls.js` and `app.js`.
Archive bindings are in `apps/starplayer-web/src/lib.rs`; reusable ZIP support is
in `crates/starplayer-archive/src/lib.rs`. Inspect the existing tests alongside them.

Today Open library folder discovers root CSVs, then displays “No root CSV found”
when none exists. The resolver already reads module bytes on demand. The user wants
ordinary directories to become libraries usable for playback and playlist building.
They selected automatic background discovery of archive members and on-demand module
metadata, with an explicit optional metadata scan.

Separate three costs: enumerate filenames progressively; inspect ZIP directories in
a worker; parse module metadata only on playback or explicit scan. Full filename
search eventually needs the whole tree enumerated, but useful results must appear
before completion. Do not parse every loose module on opening.

## Deliverables

### 1. Source enumeration and incremental library model

- Preserve explicit pending-index precedence. Auto-select a valid non-playlist root `index.csv`
  (case insensitive), otherwise the sole valid non-playlist index candidate; offer a
  chooser for multiple candidates. No-index local sources enter discovery.
  HTTP and streaming libraries retain their existing index requirement.
- Add an abortable asynchronous enumeration interface to directory and FileList
  sources. Recursively walk directory handles; yield batches while processing FileList
  entries. Read no loose module contents during enumeration. Reuse path validation.
- Recognize MOD/S3M/MTM/XM/IT using existing filename rules, plus ZIP containers.
  Ignore unrelated files. File metadata may supply size/date; format is provisional
  and title/channels/comment text remain empty until inspected.
- Add a library constructor and append/update methods sharing CSV normalization and
  search indexing; do not round-trip through generated CSV. Use stable row IDs and
  source/member locators, deduplicate locator keys, and maintain all search arrays,
  sort keys, comments, folder counts and available formats as rows change.
- Invalidate/update `playlist-transfer.js`'s WeakMap membership cache when rows arrive.
  Unknown metadata stays unknown, never a fabricated title or zero channel count.

### 2. Background ZIP discovery and metadata work

- Use one dedicated worker with its own WASM loader instance. Keep synchronous ZIP
  parsing and optional metadata work off the UI thread and isolated from playback's
  mutable loader. Enumerate loose files independently of the serial archive queue.
- Add a structured archive-listing export containing member paths, raw member-name
  bytes, sizes and inferred formats. Reuse existing ZIP decoders and resolver locators.
  Support ordinary ZIPs and one nested ZIP level, matching existing extraction support.
  Inspect central directories without extracting module members; listing inner ZIPs
  necessarily extracts the inner container. Preserve legacy filename handling.
- Retain current limits: 128 MiB source files and 64 MiB extracted entries. Process
  one archive at a time and release its bytes after discovery. Report corrupt,
  oversized, encrypted or unsupported containers/entries without aborting other work.
- Ordinary row selection and playlist addition read no module bytes. Successful
  playback enriches the matching row with actual format, title, channels and available
  instrument/sample names. Song-message extraction is not included in this task;
  messages remain searchable when supplied by a comments sidecar. Do not disturb
  playback to inspect metadata.
- Add Scan metadata: an explicit cancellable worker job over all discovered tracks,
  including new arrivals while the job is active. Interactive work takes precedence
  over queued scanning. Display that metadata-based searches are incomplete until
  metadata is available. Keep metadata in memory for this version.
- Preserve failed tracks with a visible error and retry route; one file's failure must
  not invalidate unrelated rows. A worker failure leaves existing rows usable and
  offers retry, without falling back to heavy synchronous UI-thread parsing.

### 3. Progressive view and lifecycle

- Publish tracks as discovered and allow immediate playback, searching and playlist
  addition. Add incremental view updates instead of repeatedly calling `setLibrary`,
  which currently resets selection, scope, expansion and search.
- Batch publications at most every 100 ms; preserve selected row IDs, keyboard focus,
  scope, tree expansion and visible scroll anchor. Keep sorting deterministic.
- Show counts, not a fictitious percentage: tracks found, directory discovery state,
  archives pending and issues. Empty complete libraries differ from empty in-progress
  libraries. Provide Stop discovery and Resume discovery, marking partial results.
- Keep Add selected unchanged. While discovery is incomplete, change folder-tree `+`
  titles and accessible labels to Add discovered tracks in [folder], including the
  All folders root. Add only the current result snapshot; add no new bulk control and
  never silently append later discoveries to a playlist.
- Cancel jobs when a replacement source is accepted; gate all async results on a
  library generation token. A cancelled folder picker preserves the current library.
- FileList's browser picker enumerates the entire hierarchy before delivering files.
  Explain that this initial browser-controlled delay cannot be removed; application
  processing afterward must still yield. Directory handles allow progressive walking.

### 4. Playlist identity and reconnect

- Generate a stable folder identity independent of scan order, metadata and contents.
  Persist it alongside a directory handle and reuse it on reconnect or reselection
  verified by `isSameEntry`. Do not equate directories by display name.
- Preserve existing playlist locator serialization and CSV identity behavior. Use the
  existing identity token field for folder identity without hashing directory contents.
- FileList selections and storage failures use session identities. After reselection,
  the existing explicit rebind operation matches relative locators. Report persistence
  failure without preventing use. Migrate existing stored handles compatibly.
- Saved entries may resolve through the reconnected source before discovery reaches
  their row. Distinguish not yet discovered from missing; do not disable playback
  solely because an entry has not appeared in the growing library.
- Reconnect re-enumerates; no persistent catalog, filesystem watcher or automatic disk
  writes. Keep existing playlist CSV import/export functional.

### 5. Explicit edge cases and integration contracts

**CSV selection.** Validate root headers before classifying candidates: require
`source_path`; retain comments-sidecar detection; exclude CSVs with the exported
playlist `library` column from discovery candidates entirely (even if named
`index.csv`). A folder containing only playlist-shaped CSVs enters discovery directly.
Such CSVs remain usable through explicit Open index CSV selection. Offer Browse folder without an
index in the candidate chooser. A malformed automatically selected index shows its
error and an explicit Browse folder instead action (no silent fallback). A malformed
manually selected index reports its error and requires the user to clear that pending
choice or choose Browse folder instead. Valid indexed libraries also offer Browse
folder without an index so a valid but incomplete CSV cannot trap the user. This
explicit action cancels pending index reads and starts a new library generation.

**Persistent identity.** Upgrade `starplayer-library` from v2 to v3 with a
`folderIdentities` store containing `{handle, identity}` keyed by identity token;
retain the existing `handles/root` last-reconnect slot and `indexes` store. Look up
all stored records with `isSameEntry` on folder acceptance; do not evict identities
automatically. Serialize lookup/create so concurrent opens cannot mint two identities
for one handle. Store a random 32-byte lowercase hex token in `identity.sha256` and
the folder label in `identity.name` (this is an opaque identity, not a content digest).
A legacy bare handle receives an identity on first discovered-library use; CSV
identities remain unchanged. Catch per-record comparison failures and storage denial.
If a folder gains a CSV, ordinary reopening selects the CSV identity and offers the
existing explicit rebind; choosing folder discovery reuses its folder identity.

**Partial availability and rebind.** Distinguish running, paused, complete and failed
with partial results. A matching folder identity always permits resolver playback of
saved locators; show not yet discovered until complete, then not found in scan where
appropriate. Split informational display status from the boolean playback gate in
`playlist-controls.js` and its `app.js`/playlist-playback wiring. Neither of these
scan statuses blocks explicit playback or auto-advance for a matching discovered-folder
identity: actual read/extraction failure is authoritative and follows existing error/
skip handling. Preserve existing unavailable gates for disconnected libraries, missing
uploads and indexed-library entries absent from their index. Enable rebind only when
discovery completes, explaining why it is pending while running, paused or failed;
then rebind the complete matching set with the existing action. Do not repeatedly
require clicks during discovery. Replace reconnect its original index wording with
reconnect its library. Imported playlist unavailable counts must also distinguish
pending discovery from confirmed absence.

**Metadata and sidecars.** Add a structured accessor for the already inspected
module, usable by both main-thread playback and the worker, returning actual format,
title, channel count, instrument names and sample names. Keep songMessage empty here;
do not add a core model/loader message field. Read these getters after successful
main-thread inspection without reparsing. Cache enrichment by library identity plus
locator so playlist-origin playback and playback before row discovery enrich the
correct eventual row. Hash the module bytes (after archive extraction) with SHA-256
when already available for playback or optional scanning, off the UI thread; discovery
alone never hashes content. Retain an attached comments-sidecar map and rejoin a row
when its hash arrives. Show awaiting metadata/hashes rather than misleading zero-match
completion. Preserve sidecar precedence over extracted names and keep comments from
manual replacement of a sidecar consistent. Metadata scan must not call playback or
change the active module. Optional scans finish only once their queue is empty and
discovery completes; if discovery is paused/failed show waiting for discovery and
allow cancellation. On a new scan, process uninspected rows and rows whose metadata
is ready but SHA is still missing; failed rows are retried only by an explicit Retry
metadata action. Runtime failures suspend active metadata work and offer retry without
marking individual tracks corrupt; see the master for the error/status contract.

**Playlist display.** Store optional `metadataState` inside each entry
`display` (unknown, ready, error), which existing `playlist.add` already preserves.
Absent state means legacy. Add a `playlist.updateDisplay` method keyed by entry ID
that patches display fields, notifies subscribers once per batch and preserves the
current entry ID, order and locators; use it for metadata enrichment. New playlist
imports into a discovered library start with unknown metadata state regardless of
CSV claims, unless the matching row already has ready metadata from this session,
in which case copy that verified metadata/state. Imported CSV title/format may be
shown provisionally, but unknown state prevents export as detected format. Enrichment
also updates matching legacy entries, marking them ready; legacy entries that have
not been enriched retain their existing export behavior. Update all matching
entries' title/format when enrichment arrives and persist via normal playlist saves;
do not change IDs, order, selection or locators. Export an empty `format_detected`
for unknown/error discovered metadata, and the actual format for ready entries;
legacy entries keep existing export behavior. Playback labels omit empty titles and
their separators. Metadata-driven sorting/filtering refreshes results, preserving the
visible row anchor and selection even when rows move or leave a filter.

**Archive listing.** Add a reusable structured listing in `starplayer-archive` that
returns recognized module/container entries with raw names, decoded names, declared
sizes and status plus issues; leave existing public listing APIs compatible. Oversized,
encrypted and unsupported module members appear as unplayable rows with explanations;
container failures are path-based issues. Show track errors in the existing details
pane, making that pane available for focused rows with errors even without comments;
also put the explanation in the row tooltip. Keep member failures visible through
Show unplayable and its count. Use the same surface for module-inspection failures
and its Retry metadata action; do not put errors into metadata title fields. A failed
metadata inspection keeps an otherwise eligible row playable and visible in the
default view: it must not set `playable` false. Playback remains authoritative.
Archive members already known to be unextractable remain unplayable. List inner ZIPs even if an outer ZIP has no
direct module entries. Preserve first central-directory entry for duplicate decoded
locators and report later duplicates. Validate each inner container path with the
existing resolver lookup: if it cannot uniquely resolve to the entry being listed,
report and skip that container rather than extending playlist wire format. Keep
member raw bytes in existing rawHex. Do not treat all case variants as ambiguous:
existing exact/raw/encoded matches take precedence over normalized fallback.

**Scheduling, memory and interruption.** One active worker job, queued payloads are
paths/handles rather than file-byte buffers. Transfer buffers where safe; never detach
playback-owned bytes. Recycle the worker after an archive or metadata job whose input
is at least 32 MiB, and after each 100 jobs, to release retained WASM high-water memory.
Retain unfinished paths, not WASM state. Stop discovery suspends walking and archive
jobs; if interrupting synchronous work, terminate its worker and requeue that job.
Resume continues queued work with locator deduplication. Metadata work has its own
cancel control; interactive playback uses its existing independent resolver and never
waits behind background metadata parsing. Source replacement terminates the worker,
clears both queues and discards all prior generation results.

**View, errors and names.** Incremental updates must avoid both `setLibrary` and the
scroll-resetting branch of `refresh`/`refreshComments`. Update comment controls when
the first metadata arrives. Tie sort keys with the case-sensitive full locator tuple,
not arrival IDs. Discovery issues are `{path, reason}` and CSV issues retain
`{line, reason}`; show a short summary plus an expandable full issue list. Invalid
filesystem path segments, vanished entries or unreadable subdirectories are skipped
with issues, not fatal to the walk. A root permission failure leaves partial results
and a retry action. Show transient scan errors separately from track playback errors.

### 6. Documentation and delivery

Update `LIBRARY-CSV.md` and user-facing help for index-free libraries, progressive
results, metadata completeness, limits and reconnect behavior. Update the plans index.
Follow repository worktree/delegation/review conventions for implementation. Keep this
plan outstanding until implementation lands; historical assembly trees are read-only.
Do not run `cargo fmt`, change engine timing or add runtime dependencies unnecessarily.

## Research points

Resolve implementation details against current code, preserving the specified behavior:

- Confirm structured archive listing covers outer/inner paths and raw member names,
  unlike the current tab-delimited upload picker records. Preserve existing exports.
- Inspect module metadata accessors and add a structured worker-facing inspection
  result where current getters cannot provide format and instrument/sample names.
  Avoid affecting main-thread inspection state; implement the section 5 contracts.
- Check worker asset inclusion in `cargo xtask wasm`, dev serving and Pages packaging.
- Verify the specified v3 migration and 64-hex opaque folder token pass existing
  playlist round-trips without changing CSV identity generation.
- Ensure Stop/Resume does not duplicate rows or lose unfinished directory/archive work;
  terminate a worker if synchronous WASM must be interrupted, then recreate on resume.

## Verification

- Unit tests: recursive discovery; zero module-content reads for loose discovery;
  yielding batches; duplicate basenames in different folders; case handling; empty
  folders; CSV precedence; cancellation; stable IDs; incremental search and membership.
- Worker tests: normal/nested ZIPs, legacy/raw filenames, malformed containers, mixed
  supported/unsupported entries, size limits, cancellation and stale generation results;
  outer ZIPs with only nested modules,
  duplicate decoded names and unresolvable inner containers.
- Playback gating tests: informational discovery statuses never skip a saved matching
  folder entry on explicit play or auto-advance; indexed-library unavailable behavior
  remains unchanged.
- Metadata tests: unknown fields until parsed; successful enrichment; comment search;
  failed modules and retry; optional scan includes arrivals without changing playback.
- Identity tests: same handle reconnect/reselect, different same-named roots, storage
  denial/migration, A-to-B-to-A reselection, FileList reselection and explicit rebind,
  existing CSV identities.
- Browser harness: seed small real directory trees in the origin-private filesystem
  (`navigator.storage.getDirectory()`) and stub the picker to return those handles;
  verify IndexedDB handle persistence and `isSameEntry` across page reloads. Use fake
  in-memory directory-handle/FileList enumeration sources for the 40,000-row stress case rather than creating 40,000 OPFS
  files. Manual owner acceptance covers the native picker UI/permission prompts.
- Browser tests: playlist creation/playback before discovery completes; selection and
  scroll survive updates; stop/resume; source switch and picker cancellation; reconnect
  with saved entries before discovery; metadata enrichment; both local source types.
- Instrument reads: adding discovered tracks reads no contents; discovery reads no
  loose module contents. Exercise a synthetic 40,000-track tree and a slow archive job
  to verify progressive results and responsive controls. Fixtures are synthetic or
  existing approved fixtures; tests must not use the owner's music collection/network.
- Run `node --test apps/starplayer-web/test/*.test.mjs`, relevant archive/web Rust
  tests, `cargo xtask wasm`, `STARPLAYER_REQUIRE_CHROMIUM=1 node
  apps/starplayer-web/test/headless.mjs`, and `cargo xtask ci`. Verify Pages worker assets
  via `cargo xtask wasm --pages`. Summarize results before committing implementation.
- Owner acceptance: a real large folder without CSV opens progressively; build and
  replay a playlist of loose/nested-archive tracks; reconnect after page reload.

## Out of scope

Implementation in this planning commit; HTTP directory discovery; extracting song messages from module bytes; persistent catalog
or metadata caching; filesystem watching; archive formats/depths beyond existing ZIP
support; sniffing extensionless/misnamed files; automatically writing an index to disk;
engine/audio-path changes; deployment or merge to main.
