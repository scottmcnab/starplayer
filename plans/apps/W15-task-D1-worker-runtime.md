# W15-D1 — Isolated worker transport, scheduling and recycling

| Field | Value |
|---|---|
| Status | Specified — not started |
| Recommended model | gpt-6.1-sol |
| Recommended effort | **high** |
| Depends on | B integrated; can overlap unfinished A/C |
| Branch / sibling worktree | `w15-d1` / `../starplayer-w15-d1` |
| Ownership | new worker script, runtime/client modules and runtime tests |
| Coordinator | gpt-6.1-sol, xhigh; [master](W15-master-plan.md) |

## Context for a fresh agent

Read `AGENTS.md`, [W15 master](W15-master-plan.md), and the complete
[behavioral specification](W15-task-folder-library-discovery.md). The master defines
shared APIs; the behavioral specification defines edge cases and compatibility. Work
from this file without conversation history. Do not commit or start another task.
Report changes, verification and producer contracts to coordinator. JS paths below
are under `apps/starplayer-web/www/` unless otherwise stated.

B supplies structured catalog and module metadata exports. D2 will own source choice,
scan states and model updates; this task is the isolated execution service. It uses
only existing source size/readFile capabilities, so it can start as soon as B lands.

## Deliverables

- Implement the master's createLibraryWorkerRuntime and job protocol exactly. Use
  one worker with its own WASM instance; no main-thread inspection or shared playback
  resolver queue. Worker kind/result schemas and error/generation tagging are fixed
  in the master. Inject workerFactory for deterministic tests; production loads worker
  and WASM URLs relative to import.meta.url.
- Dispatch serially with hash/catalog/inspect priority. Read source size then bytes
  only when dispatching, respecting 128 MiB source/64 MiB extracted limits. Worker
  resolves archive inspect jobs with existing path extraction, not an alternate name
  matcher. Inspect returns real metadata plus extracted-module SHA; catalog never
  extracts module members. Hash-only jobs use bytes already read for playback.
- Retain locators rather than queued buffers. Copy borrowed hash bytes once; never
  detach caller bytes. Supersede old hash work before retaining its replacement and
  gate replies. Recycle after a >=32 MiB input or every 100 jobs as specified. Dispose
  releases queues/copies and terminates workers.
- Implement pauseCatalog/resumeCatalog, per-job cancel and cancelMetadata (inspect
  jobs only). Stop/requeue active catalog work without losing its result promise;
  terminating a worker must preserve unrelated queued jobs. User cancellation rejects
  the affected job as AbortError; stale results cannot settle twice. Dispose rejects
  pending jobs, and callers must be able to attach handlers before asynchronous errors.
- Use the master's exact `{name,code,message}` failures. Only worker-failed suspends
  the queue; catalog/inspect/hash errors reject one job and continue. Treat binding
  throws/invalid JSON as job-failed, source limits as source-too-large and file I/O
  errors as read-failed; aborted/disposed use AbortError and do not imply track faults.
- Initialization/crash failures reject the active job and suspend queued work. D2
  recreates on retry and resubmits unfinished locators; expose no silent fallback to
  synchronous UI-thread parsing. Keep library controller/model/status out of this task.

## Research points

Inspect structured-clone/transfer ownership and abort checks around uncancellable File
reads. Ensure recycle runs between jobs, not before result delivery. Existing WASM
packaging copies flat www files; keep worker modules in that directory. Confirm every
crash/dispose path clears buffered payloads and settles promises.

## Verification

Add Node runtime tests with controlled transport/source/clock, covering priority,
backpressure, size rejection before reading, never buffering queued archives, hash
supersession, worker init/crash, recycle thresholds, cancellation mid-read/active WASM,
pause/requeue catalog while inspect proceeds, cancel inspect while archives remain,
stale generation/job replies, exact error codes, queue suspension only on worker-failed,
and exactly-once promise settlement. Run these plus
`node --test apps/starplayer-web/test/track-resolver.test.mjs` and `cargo xtask wasm`.
Hand D2 protocol tests and concrete fake transport examples. G owns actual Chromium
worker/Pages integration and performance acceptance.

## Out of scope

Source enumeration/identity, library-controls.js, discovery/model/UI, app/playlist
wiring, Rust binding changes without coordinator handoff, shared headless harness.
