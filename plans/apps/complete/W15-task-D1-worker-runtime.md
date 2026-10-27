# W15-D1 — Isolated worker transport, scheduling and recycling

| Field | Value |
|---|---|
| Status | Complete — reviewed and integrated on `w15-folder-library`; final automated acceptance passed |
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

## Verification evidence (2026-10-26)

Worker and coordinator independently passed runtime/resolver tests 37/37. Worker
passed `cargo xtask wasm`, verified all flat worker assets are packaged, and ran
a generated-WASM metadata/SHA and corrupt-ZIP endpoint smoke check. Full diff
review and `git diff --check` passed. Real Chromium/Pages loading remains with G.

Runtime uses one active job and FIFO Maps for hash/catalog/inspect priorities,
retains locators for queued source work, preserves caller hash bytes, requeues
paused catalogs with the same promise, and suspends only on `worker-failed`.
All public request/result/error schemas match the master. Internal transport uses
ready/startup-error and tagged job/result/error messages; test fakes are in
`library-worker-runtime.test.mjs`.

Coordinator follow-up: idle transport/crash failure now rejects one queued or
next-enqueued job with `worker-failed`, including paused catalogs; other jobs stay
suspended for explicit recreation. Six new regressions cover all job kinds,
cancellation/supersession and disposal. Worker and coordinator runtime/resolver
verification passed 43/43; WASM packaging passed again. This fixes a silent hang
without changing the public API.

Final integrated automated acceptance passed in G, including all three browser
modes and full 11-job repository CI. Remaining manual owner acceptance is recorded
in the [master](W15-master-plan.md). This task is archived with the completed
integration deliverable; `main` was not merged or deployed.
