# W15 — Index-free folder libraries: coordinator master plan

| Field | Value |
|---|---|
| Status | Implementation in progress — A/B/C/D1/D2 reviewed and integrated; E/F dispatched |
| Coordinator | gpt-6.1-sol, **xhigh** effort |
| Workers | gpt-6.1-sol, **high** effort for all tasks |
| Planning branch | `library-folder-plan` |
| Behavioral specification | [Folder library specification](W15-task-folder-library-discovery.md) |
| Deliverable | Progressive local folder/ZIP libraries, lazy metadata, usable saved playlists |

## Context for a fresh coordinator

Read `AGENTS.md` and the linked behavioral specification in full. That specification
is the source of truth for product behavior; this master fixes task ownership and
interfaces. Do not reinterpret the feature as eager module parsing or lazy directory
expansion. The owner chose automatic background ZIP discovery and metadata on playback
or explicit scan. No implementation is authorized by the planning commit itself.

On a later implementation instruction, use this master to dispatch the task files.
Each worker reads its task plus the behavioral specification and this master's shared
contracts; no conversation history is needed. High effort is recommended for workers
because even the foundation tasks have compatibility and cancellation edge cases.
Use xhigh for the coordinator's dependency, integration and final diff reviews. These
are task-specific recommendations, not a requirement to increase scope.

## Task graph and ownership

| Task | Deliverable | Dependencies | Owned production area |
|---|---|---|---|
| [A](W15-task-A-local-sources.md) | Enumeration, CSV classification, folder identity | None | `library-source.js` and new identity helper |
| [B](W15-task-B-archive-metadata-bindings.md) | Structured ZIP listing and module metadata bindings | None | archive crate, web Rust bindings |
| [C](W15-task-C-incremental-library-model.md) | Incremental rows, search, comments and locator membership | None | `library-index.js`, new pure model helpers |
| [D1](W15-task-D1-worker-runtime.md) | Worker transport, scheduling, hashing and recycling | B landed | new worker/client/runtime modules |
| [D2](W15-task-D2-discovery-controller.md) | Discovery/metadata lifecycle and source controls | A, C, D1 landed | new discovery controller, `library-controls.js` |
| [E](W15-task-E-progressive-library-view.md) | Incremental browser UI, controls markup and error details | D2 landed | `library-view.js`, `index.html`, CSS |
| [F](W15-task-F-playlist-playback-integration.md) | Playlist persistence, gating, metadata and app wiring | D2 landed | playlist JS, `app.js`, `playback-controller.js` |
| [G](W15-task-G-integration-verification.md) | Real-handle browser tests, stress checks, packaging and docs | E, F landed | headless harness, packaging fixes, user docs |

Run A/B/C concurrently (three workers plus coordinator). Once B is reviewed and
integrated, its freed slot can run D1 while A/C finish. Start D2 after A/C/D1 are
integrated. Run E/F concurrently after D2; F uses E's fixed interface below and verifies
app integration once E lands. G is last. Sequential A/B/C/D1/D2/E/F/G is also valid. No worker edits another task's
owned files without a coordinator-approved ownership transfer recorded in its task.
Each task owns its new unit tests and narrowly related existing unit tests. G owns
shared `headless.mjs`; E extracts pure view-state helpers for Node tests and hands G
named mandatory DOM scenarios. After B lands, F may adapt only app.js source-contract
tests in web `src/lib.rs`; E reports HTML source-contract changes to F, the sole writer
of that test module during E/F's parallel wave. E and F both run web Rust tests.
E owns in-page help in index.html; G owns LIBRARY-CSV.md and reports any remaining
in-page help fixes to E. Coordinator owns plans/README.md and all plan status/archive
moves. These are explicit ownership transfers, not permission for unrelated edits.

## Shared contracts and handoffs

These names and shapes are the implementation contract. The producer implements and
tests them; consumers use them. If repository constraints require a change, the
coordinator updates this master and all affected task specs before dispatching a
consumer. Do not leave parallel workers to invent incompatible APIs.

- **Local sources (A):** `source.enumerate({signal})` is an async iterator yielding
  `{kind:'file', path, size, timestamp}` or `{kind:'issue', path, reason}`. File records
  include all regular files; D2 filters extensions. `size` is a number or null;
  `timestamp` is ISO-8601 UTC text derived from lastModified or empty when unknown.
  They may be null/empty when obtaining metadata would add unnecessary I/O. No content reads. Pausing means
  D2 stops requesting `next()`; abort ends iteration; retry may restart with deduplication.
  FileList map construction also yields, not just subsequent enumeration. Existing
  `readFile`, `size`, root index/comment candidate properties remain compatible.
- **Identity (A):** `folderIdentity(source)` resolves `{identity, storageFailed}` using
  the v3 registry specified in the behavioral spec. FileList/storage denial gets a
  session token; repeated requests for the same accepted source reuse it. Keep the
  last-handle reconnect API backward compatible. Concurrent lookup/create is serialized
  in the page; cross-tab coordination is not a new requirement for this version.
- **Bindings (B):** add `archive_catalog(bytes)` returning a JSON string with
  `{entries, issues}` for one outer ZIP and at most one inner ZIP level. Entry fields
  are `{containerMember, memberPath, rawHex, size, format, playable, error}`; `format` is a lowercase
  mod/s3m/mtm/xm/it token, `size` a number, `error` empty on playable entries; issues
  are `{path, reason}` relative to that archive. No module member extraction. The
  catalog layer itself handles supported inner containers and reports skipped deeper
  containers. Invalid/corrupt outer ZIP input throws a binding error; corrupt inner
  containers become path issues while preserving valid outer/other-inner entries. Add `module_metadata()` returning JSON for the already inspected module:
  `{format,title,channels,instrumentNames,sampleNames,songMessage:''}`. Name fields are
  newline-separated in original slot order, matching existing comment indexing; metadata
  `format` uses the same lowercase tokens, `channels` is a positive integer.
- **Model (C):** `createDiscoveredLibrary({name})`,
  `appendDiscoveredRows(library, records)`, `updateDiscoveredRows(library, updates)`;
  updates identify a locator, not an arrival index. Return changed row ID arrays.
  Preserve the CSV row shape, add row `metadataState` (unknown/ready/error), `error`,
  and a monotonically increasing `revision`. Expose `library.locatorIds` keyed by the
  existing JSON `[sourcePath,containerMember,memberPath]` tuple. Retain sidecars and
  rebuild only affected row indexes where practical. Unknown updates for not-yet-seen
  rows are held by D2, not by the model. C exposes a shared locator-key helper; F reuses
  it and invalidates legacy WeakMap membership by revision.
- **Rows at the D2/C boundary:** D2 maps A/B records to `DiscoveredRecord`:
  required strings `sourcePath`, `containerMember`, `memberPath`, `rawHex` (optional
  member fields default empty); lowercase `format`; `size:number|null`; `timestamp`
  ISO-8601 UTC string or empty; `playable:boolean` (D2 sets from recognized format and
  B status); `error:string` default empty. Optional metadata fields default to
  `title:''`, `channels:null`, `tracker:''`, `sha256:''`, `instrumentNames:''`,
  `sampleNames:''`, `songMessage:''`, `metadataState:'unknown'`. C derives id/name/folder,
  sets sourceUrl empty, and validates numeric/text types before indexing. No archive
  timestamp is synthesized. A `RowUpdate` is `{locator,patch}`; locator carries the
  three path-key strings, patch contains only title, format, channels, tracker, sha256,
  instrumentNames, sampleNames, songMessage, metadataState and error. Ready enrichment
  clears a metadata error. Patches never turn a structurally unplayable entry playable
  or change its locator, size, timestamp or ID. Missing patch fields retain old values.
- **Worker runtime (D1):** `createLibraryWorkerRuntime({source,workerFactory})` returns
  `enqueue(job) -> {id,result,cancel}`, `pauseCatalog()`, `resumeCatalog()`,
  `cancelMetadata()`, `dispose()`. The promise resolves `{generation,kind,payload}` or
  rejects `{name,code,message}`. Codes are `aborted` (explicit cancel/supersession),
  `disposed`, `source-too-large`, `read-failed`, `job-failed` (catalog/inspect/hash or
  payload decoding error), and `worker-failed` (startup/crash/transport failure).
  name is AbortError for aborted/disposed and Error otherwise. Only worker-failed
  suspends the queue; other failures settle that job and continue unrelated work.
  D2 maps catalog source/read/job failures to an issue at its outer sourcePath, inspect
  source/read/job failures to metadataState error on its locator, and hash failures
  to an issue at the locator's joined source/container/member path with SHA still
  unknown (retain ready metadata). D2 retains the locator in the hash-job closure
  so an error can be attributed without changing the hash bytes-only request. Aborted/disposed
  are control flow, never user-facing track errors. Runtime recreation disposes old
  pending promises; D2 handles those rejections and resubmits from its own unfinished
  locator set, once. Jobs carry generation and kind: `catalog` plus sourcePath,
  `inspect` plus library locator, or `hash` plus borrowed bytes. Catalog uses B's export;
  inspect resolves module bytes in the worker using existing archive path extraction,
  calls inspect_s3m/module_metadata and computes SHA-256; hash returns SHA-256 only.
  Payloads are B catalog, `{metadata,sha256}`, or `{sha256}` respectively. Source size
  and read happen on dispatch (before transfer), with limits and cancellation checks.
  Runtime owns queue/job IDs/recycling; D2 owns scan completion, per-locator results
  and generations. Worker messages echo job ID and generation. Priorities are hash,
  catalog, inspect; queued catalog jobs pause independently of inspect. Cancelling an
  active job terminates/recreates its worker and preserves unrelated queued jobs.
  Superseding hash cancels the previous hash before retaining one replacement buffer;
  its copy is owned by D1, never transferred from the caller. Ordinary pending jobs
  store locators, not buffers. `cancelMetadata` cancels inspect jobs only; disposal
  cancels everything. Worker startup errors reject the active job and leave queued
  jobs suspended until D2 explicitly retries by recreating the runtime. An idle
  worker failure is reported once through a queued or next-enqueued job promise,
  including paused catalogs, so later work cannot hang without a Retry state.
- **Context/runtime (D2):** discovered contexts retain existing
  `{source,library,identity,resolver}` and add `mode:'discovered'`, `discovery`, and
  `discoveryStatus`. Indexed contexts default to indexed mode and have no discovery
  controller. Status includes `phase` (running/paused/complete/failed), files examined,
  archives pending, tracks found, `issues:[{path,reason}]`, issue count, and metadata
  phase (idle/running/waiting/suspended/complete/cancelled), plus
  `runtimeError:string` empty when healthy. Include `metadataCounts` with ready,
  unknown, error and hashesPending counts, plus `metadataIncomplete:boolean` and
  sidecar joined/unmatched/awaitingHashes counts. D2 owns the one-line library-summary
  text; E owns detailed progress, incompleteness hints and the full issue list in
  separate elements. E does not write library-summary. Status snapshots are read-only
  to consumers; indexed-library status behavior stays compatible. On worker failure,
  set runtimeError and suspend an active metadata scan. Loose-file enumeration
  continues independently: discovery stays running (or paused if the user paused it).
  Once the walk ends, discovery becomes failed only if archive jobs remain suspended;
  otherwise it becomes complete. An already complete discovery stays complete.
  User pause intent wins over this transition and remains paused until Resume.
  retry recreates the runtime, clears runtimeError and resubmits unfinished catalog/
  inspect locators, preserving user pause intent; requeued catalog jobs stay paused
  while that intent holds. Resume with a runtime error resumes
  the loose walk only; the error's Retry control is still required for worker jobs. Discard failed
  hash payloads rather than retaining bytes; later playback or explicit metadata scan
  can supply the missing SHA. Metadata scans include ready rows with missing hashes
  (even though their title/channel inspection succeeded). E shows runtimeError and
  retry in library-progress separately from row errors, even after discovery completed.
  Controller methods: `pause()`, `resume()`, `retry()`, `scanMetadata()`,
  `cancelMetadata()`, `retryMetadata(locator)`, `acceptPlaybackMetadata(locator, metadata, bytes)`,
  `dispose()`. Every callback is generation-gated. `bytes` is borrowed by the method;
  copy any data needed asynchronously, never detach the caller's buffer.
  acceptPlaybackMetadata takes B's parsed metadata object, not its JSON string.
- **Notifications (D2):** keep `onLibrary(context)` for replacement only; add
  `onLibraryUpdate(context, {changedRowIds, statusChanged, metadataUpdates})` to
  `wireLibraryControls`. `metadataUpdates` is `[{locator,patch}]` using the RowUpdate
  fields above; context supplies library identity. Updates are partial patches, not
  full snapshots, even if the row has not arrived. Comments updates use the same incremental route
  for discovered contexts; preserve old indexed-library callbacks.
- **View (E):** keep `setLibrary(context)` and `setPlaying(id)`; add
  `updateLibrary(context, change)` that preserves UI state and ignores obsolete
  contexts. Controls markup IDs are `library-browse-folder`, `library-stop-discovery`,
  `library-resume-discovery`, `library-retry-discovery`, `library-scan-metadata`,
  `library-cancel-metadata`, `library-issues`, `library-progress`,
  `library-metadata-hint`. D2 binds top-level controls; E owns per-row
  metadata retry and markup/CSS. E forwards row retry to context.discovery.
- **Playlist/app (F):** `playlist.updateDisplay(updates)` accepts `[{id,patch}]`, emits
  one change notification per batch, preserves current/order/locators. Keep textual
  `playlistControls.availability(entry)` and add boolean `canPlay(entry)`; wire only
  the latter to playback gating. `libraryChanged` handles replacement and
  `libraryUpdated(context,change)` handles incremental membership, metadata and status.
  F owns bootstrap wiring from D2 notifications into E and playlist controls, and the
  successful playback hook into D2. Capture B metadata synchronously after inspect_s3m,
  before any await; parse its JSON immediately and publish the object only after
  successful current/revision activation checks. If the accessor or JSON parsing
  fails, skip this optional enrichment and continue normal playback; it must not
  turn successful playback into a load failure.
  F may extend playback-controller activation context with the originating locator
  (captured by play(), not inferred afterward from current playlist selection). E is not responsible for editing `app.js`.

## Execution and review procedure

1. Start from the committed planning branch and ensure the user's GPT-6.1-sol policy
   in `AGENTS.md` is present in execution worktrees. Do not silently revert or stage
   unrelated owner edits. The owner model choice here takes precedence over stale
   policy text in another checkout.
2. Create an integration branch `w15-folder-library` from the planning tip, then task
   branches `w15-a`, `w15-b`, `w15-c`, `w15-d1`, `w15-d2`, `w15-e`, `w15-f`, `w15-g` with sibling worktrees `../starplayer-w15-a`, etc.
   Branch dependent tasks from the integrated prerequisites, not the original tip.
3. Workers implement only their task, run its Verification section, and report diff,
   commands/results, remaining limitations and handoff exports. Workers do not commit,
   merge, publish or change task scope. The coordinator reviews and commits explicit
   paths after checking status; follow AGENTS merge/squash rules from target checkout.
4. The coordinator checks each producer contract and tests before accepting consumers.
   Preserve baseline CSV/remote/upload behavior. Never weaken engine timing/no_std tests.
   No cargo fmt, attribution trailers, broad staging commands or historical-source edits.
5. If a task is incomplete, keep it outstanding and fix it before its consumers start.
   Update task status/evidence in the integration branch. Do not mark intermediate
   integration as landed; archive plans only after deliverables land and only owner
   acceptance remains. No deployment or merge to main without the later user's scope.

## Final acceptance gate

G runs all web unit tests, relevant Rust tests, WASM build and Chromium headless suite,
Pages packaging smoke check and `cargo xtask ci`, including real OPFS handle reconnect,
40,000-row progressive discovery, slow archive responsiveness, metadata/cancellation
and playlist compatibility. Coordinator reviews E/F integration specifically for
source replacement races, stale metadata, scroll/focus stability, deferred rebind and
informational status that must not block playback. Record verification and remaining
manual native-picker/real-library acceptance in this master. Planning-only validation
is document links, ownership/dependency checks and external review, not feature tests.

## Implementation evidence

Execution started 2026-10-26 from planning commit `3c59350`. Integration is
`w15-folder-library` in `../starplayer-w15-folder-library`; task worktrees follow
the branch names above. The owner’s uncommitted GPT-6.1-sol edit to `AGENTS.md`
is preserved in planning and execution worktrees and excluded from implementation
commits. All tasks use GPT-6.1-sol workers at high effort. A and C have been reviewed and
integrated as single-change squash commits (`72c15ea`, `f9fc9ec`). Their focused
tests also pass together in the integration checkout. B is reviewed and integrated; its archive/web Rust tests were independently
rerun by the coordinator. D1 now starts from the integrated producer contracts. Baseline web Node tests passed 78/78 with localhost
permissions (the sandbox-only dev-server failure was environmental).

Manual native-picker permissions, a real large folder, and loose/nested playlist
reconnect after reload remain owner acceptance, pending automated integration gates.

D1 is reviewed and integrated after an independent 37/37 runtime/resolver test
run. Worker assets and actual generated-WASM endpoint smoke checks passed. D2
starts from all integrated prerequisites; E/F remain gated on its review.

Coordinator review found and fixed idle-worker failure reporting before D2
integration. The producer follow-up and independent runtime/resolver rerun pass
43/43 tests; queued/next work reports one failure and remaining jobs stay suspended.

D2 is reviewed and integrated with 165/165 web Node tests and WASM/import checks
passed. Coordinator review repaired stale SHA/sidecar summary handling and found
the producer idle-crash follow-up above. E/F now run concurrently from this
integrated lifecycle; G remains gated on both reviewed UI/app changes.
