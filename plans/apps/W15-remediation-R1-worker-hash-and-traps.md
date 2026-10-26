# W15 remediation R1 — Fatal WASM trap isolation in the library worker

| Field | Value |
|---|---|
| Status | **Integrated — reviewed; focused verification passed** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | Baseline; parallel R2/R3/R4a/R5 |
| Ownership | library-worker-runtime.js, library-worker-jobs.js and worker unit tests |
| Branch / sibling worktree | `w15-remediation-r1` / `../starplayer-w15-remediation-r1` |
| Authorization now | Implementation authorized; worker must not stage, commit or merge |

## Context for a fresh agent

Read AGENTS.md, [remediation master](W15-remediation-master-plan.md), and the
[archived W15 master](complete/W15-master-plan.md) with its linked implementation
evidence. The remediation contracts explicitly supersede only listed archived clauses.
Feature baseline is b3faf9e; execution starts from the integrated planning tip.
Task status and verification are updated by the coordinator.
Do not rely on chat history or temporary review scripts. JS paths are under
`apps/starplayer-web/www/`; tests under `apps/starplayer-web/test/`. During execution,
worker reports diff/results to coordinator, does not commit or edit another task's
files. Preserve engine/RT/no_std invariants.

Finding #10: `startLibraryWorker` flattens every thrown error, including
`WebAssembly.RuntimeError`, to `job-failed`. The runtime then reuses an instance whose
WASM state may be inconsistent after a trap. Do not claim RefCell poisoning was
reproduced; the fix is defensive.

**Amendment (2026-10-26):** the first draft of this task also rebuilt hash
scheduling (one active plus one pending copy, `hash-deferred`, purposes, fairness,
`onFatal`). Master section A moves playback hashing to the page, owned by R2, so that
work is withdrawn. Leave the `hash` job kind and its tests untouched; R4b deletes it.
The file name is kept for link stability.

## Deliverables

- In `startLibraryWorker`'s job catch, classify `error instanceof WebAssembly.RuntimeError`
  (with a structural fallback on `name === 'RuntimeError'` for realms where `instanceof`
  fails) as `{code:'worker-failed', cause:'wasm-trap'}`. All other errors keep their
  current codes.
- In the runtime's `onmessage` error path, honor `worker-failed` as fatal: route it
  through the existing `workerFailed` handling, which terminates the instance, settles
  the active job once with `worker-failed`, suspends the queue and waits for explicit
  Retry. Preserve the optional `cause` on the rejection. Today every message-level
  error maps to `job-failed`; change only this fatal branch.
- Ordinary loader `Result` errors, unsupported data and payload-validation failures
  stay `job-failed`, and the queue continues on the same instance.
- Preserve idle-failure reporting, paused catalogs, cancellation before ready and late
  message guards. No automatic restart loop.

## Research points

Inspect the real worker endpoint (`library-worker-jobs.js`), not only fake transports.
Check how wasm-bindgen surfaces a Rust panic (`unreachable` trap) in the current build
so the test's injected error matches production.

## Verification

Run `node --test --test-isolation=none apps/starplayer-web/test/library-worker-runtime.test.mjs`
and add endpoint regressions to that file or a new
`apps/starplayer-web/test/library-worker-jobs.test.mjs` (run it explicitly if added).
Inject a `RuntimeError` through `startLibraryWorker` with a controlled loader and assert
the `worker-failed`/`wasm-trap` frame. In the runtime, assert instance termination, exact-once
rejection, queue suspension, no rows touched, and success on a recreated instance after
explicit Retry. A second trap suspends again without looping. An ordinary bad-file error
continues on the same worker. R7 repeats the trap case through the real browser build.

## Out of scope

Hash scheduling (withdrawn; see master A), discovery/model behavior, archive grouping,
view/source-selection work, CI claims.

## Execution evidence (2026-10-27)

Integrated as `1e79716` after coordinator diff review and independent focused run:
42 worker/runtime tests passed, including actual WASM unreachable at the endpoint,
realm fallback, repeated traps, exact-once settlement, stale-frame suppression and
ordinary-error continuity. `git diff --check` passed. Browser verification remains R7.
