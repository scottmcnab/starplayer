# W15-D2 — Discovery controller, metadata lifecycle and source controls

| Field | Value |
|---|---|
| Status | Implemented and coordinator-reviewed — integrated into `w15-folder-library` |
| Recommended model | gpt-6.1-sol |
| Recommended effort | **high** |
| Depends on | A, C and D1 integrated (D1 includes B) |
| Branch / sibling worktree | `w15-d2` / `../starplayer-w15-d2` |
| Ownership | new discovery controller, library-controls.js, controller/control tests |
| Coordinator | gpt-6.1-sol, xhigh; [master](W15-master-plan.md) |

## Context for a fresh agent

Read `AGENTS.md`, [W15 master](W15-master-plan.md), and the complete
[behavioral specification](W15-task-folder-library-discovery.md). The master defines
shared APIs; the behavioral specification defines edge cases and compatibility. Work
from this file without conversation history. Do not commit or start another task.
Report changes, verification and producer contracts to coordinator. JS paths below
are under `apps/starplayer-web/www/` unless otherwise stated.

A supplies enumeration/identity, C incremental models, and D1 worker jobs. Current
controls use revision counters and install a CSV context after parsing; preserve
picker cancellation and existing indexed/remote flows while adding discovery.

## Deliverables

- Implement master discovered context/controller/status/notification APIs. Accept a
  folder, get identity, install empty context immediately and enumerate; map A file
  records and B catalog results via D1 to the exact C DiscoveredRecord schema. Append
  five supported extensions immediately while ZIP jobs run independently. No loose
  content reads or hash/metadata scan on discovery, selection or playlist add.
- Own library generation, unfinished locators, pause/resume/retry, scan completion,
  issue list and metadata counts. Controller.pause backpressures enumeration and calls
  pauseCatalog; resume continues without duplicate rows. Root or worker failures keep
  valid rows and an explicit retry. On worker recreation resubmit unfinished jobs once;
  stale callbacks cannot append/update/complete the new generation.
- Implement explicit metadata scan/cancel/retry using D1 inspect jobs. Scan consumes
  uninspected rows and ready rows missing SHA, including new arrivals, waits if discovery is paused/failed, and
  completes only when discovery is complete and inspection queue empty. Do not retry
  metadata errors until Retry metadata. Stop discovery leaves inspect work for known
  rows running; cancelMetadata leaves archive discovery running.
- acceptPlaybackMetadata receives synchronously captured metadata and borrowed bytes.
  Update row/cache by identity+locator without reparsing, request D1 hash, and join
  retained sidecar when it arrives. Publish changes even before a row exists so F can
  update saved entries. Cache before-row updates for later append, discard stale
  generations, and preserve caller bytes. Hash supersession may leave SHA unknown;
  a later play or explicit metadata scan can supply it.
- Handle the master's failure codes explicitly: control-flow abort/dispose is silent;
  catalog failures become outer-source path issues; inspect failures set row metadata
  error without changing playability; hash failures add a joined source/container/member
  path issue using the retained locator, while leaving ready metadata/SHA unknown.
  Worker failure sets runtimeError and suspends metadata, but loose walking continues:
  discovery remains running (or user-paused). When the walk ends, mark failed only if
  archive jobs remain suspended, otherwise complete; preserve explicit pause intent.
  Resume resumes only the loose walk while runtimeError remains; Retry is required
  for worker jobs. Retry recreates/resubmits
  unfinished catalog/inspect jobs, preserving pause intent. Dispose rejections during
  retry must not become track errors. Drop failed hash payloads; do not retain bytes.
  Expose runtime error/retry even if directory discovery was already complete.
- Retain sidecar map/revision and join incrementally on hashes; a late hash uses the
  latest sidecar. Replace old sidecar contributions correctly. Track ready/unknown/error
  metadata, hashesPending and awaitingHashes; no zero-match completion claim while
  hashes are missing. No change to active playback state or loader instance.
- Bind E's exact top-level control IDs, tolerating absent markup until E lands.
  Implement CSV bypass/malformed-index actions/pending-index clearing and source
  replacement cancellation. D2 alone writes one-line library-summary; E renders
  detailed status and issue arrays via notifications. Keep indexed callback behavior.
- Notify onLibrary only on replacement, onLibraryUpdate for row/status/metadata changes,
  at most once per 100 ms except immediate completion. Dispose old context only after
  a replacement source is accepted; picker cancellation preserves old work. Source
  replacement cancels iterator/runtime and invalidates sidecar/hash/model callbacks.

## Research points

Check all existing source/index/comments revision paths, especially explicit CSV bypass
while another CSV is reading. Controller methods must be testable with injected A/C/D1
fakes. Check completion counts and how source permission failure differs from a
per-path issue. Use existing path validation and comments parser, never parallel copies.

## Verification

Run existing library-controls/source/index tests and new discovery-controller tests.
Cover progressive loose rows before slow ZIPs, zero loose content reads, byte schemas,
empty/partial/complete, stop/resume, root/worker retry, replacement/cancelled picker,
manual/automatic malformed CSV and playlist-CSV bypass, sidecar replacement, metadata
arrivals during scan, scan waiting/cancel/retry, stale callbacks, and metadata/hash
before row arrival, worker failure after discovery completion, paused runtime retry,
ready rows lacking SHA included in explicit scans, hash issue paths/counts, and
continued loose discovery plus final phase when the worker fails with/without pending
archives. Assert status counts/issues and notification batching. Use fake
D1 transport/controller jobs; D1 owns runtime tests and G owns browser integration.
Run `cargo xtask wasm` for packaged module imports. Hand E/F status/schema examples.

## Out of scope

D1 worker/runtime internals, app.js, library-view.js, HTML/CSS, playlist files, shared
headless harness, producer contract changes without coordinator review.

## Verification evidence (2026-10-26)

Worker final full Node suite passed 165/165 after the reviewed D1 idle-failure fix.
`cargo xtask wasm`, all ten recursively checked packaged module imports, and
`git diff --check` passed. Coordinator reviewed the complete controller/control
and test diffs, independently passed an earlier full suite 154/154 and the final
focused controller/control/source/model gate 78/78; subsequent added regressions
cover dropped hash payloads and row-only retries.

Context/status/notification schemas match the master. Status uses immutable
snapshots and `sidecarCounts:{joined,unmatched,awaitingHashes}`. Epoch and locator
versions gate stale results; playback clears old SHA before hashing new bytes.
Root/worker retry preserves pause and avoids resubmitting completed locators.
Before-row metadata notifications remain partial patches for F. E must retain
D2-controlled Browse folder visibility for indexed local contexts, never write
`library-summary`, and render separate progress/issues/per-row retry surfaces.
