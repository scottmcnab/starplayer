# W15 remediation R5 — Non-timer cooperative directory work

| Field | Value |
|---|---|
| Status | **Landed on `main` via `ece531f` (2026-10-27) — reviewed/verified; manual feature acceptance pending** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | Baseline; parallel R1/R3 |
| Ownership | library-source.js, new cooperative-yield.js, source/scheduler unit tests |
| Historical branch / sibling worktree | `w15-remediation-r5` / `../starplayer-w15-remediation-r5` |
| Authorization now | Implementation authorized; worker must not stage, commit or merge |

## Context for a fresh agent

Read AGENTS.md, [remediation master](W15-remediation-master-plan.md), and the
[archived W15 master](W15-master-plan.md) with its linked implementation
evidence. The remediation contracts explicitly supersede only listed archived clauses.
Feature baseline is b3faf9e; execution starts from the integrated planning tip.
Task status and verification are updated by the coordinator.
Do not rely on chat history or temporary review scripts. JS paths are under
`apps/starplayer-web/www/`; tests under `apps/starplayer-web/test/`. During execution,
worker reports diff/results to coordinator, does not commit or edit another task's
files. Preserve engine/RT/no_std invariants.

Finding #5 is a timer-throttling risk, not a proven files/sec number. Existing source
map/walk yields via chained setTimeout every 128 entries. Keep source APIs, validation,
no content reads and browser-owned FileList picker limitations unchanged.

## Deliverables

- Implement master E yieldCooperatively({signal}) with scheduler.yield preferred,
  MessageChannel fallback, timer only as last fallback; release ports/listeners and
  check cancellation on both sides. Export it for R6. R4b no longer needs it: the
  amended group protocol has no per-member yield.
- Apply <=128-record or 8-ms slice boundary to FileList map/candidate processing and
  recursive directory iteration. Existing awaited I/O does not guarantee a task yield;
  no queueMicrotask-only or rAF-based workaround. Inject scheduler/clock for unit tests.
- No synthetic audio/keepalive or browser flags to defeat throttling. Freeze can stop
  progress; resume continues safely. Hand R7 exact environment capability/fallback
  diagnostics for honest hidden-page tests. Do not change identity/storage behavior.

## Research points

Check APIs by feature detection in page/worker environments; fallback tests must
exercise missing scheduler and missing MessageChannel. Source.abort checks must not
leak live message ports or wait forever on a late callback after disposal.

## Verification

Run `node --test --test-isolation=none apps/starplayer-web/test/library-source.test.mjs apps/starplayer-web/test/library-source-enumeration.test.mjs`
and new cooperative-yield tests. Assert task interleaving, both budget triggers,
zero content reads, abort before/during yield and port cleanup; verify preferred/
fallback branches with controlled APIs. R7 owns actual hidden-tab test with normal
throttling enabled and documents suspended/frozen limits; no fake visibility-only
assertion counts as real background coverage. Hand R6 the helper.

## Out of scope

Notification UI throttling redesign, playback keepalive, claiming universal hidden-tab
throughput, source/identity schema change, browser harness edits.

## Execution evidence (2026-10-27)

Integrated as `f3fe09a` after coordinator diff review. Coordinator independently passed all 41 focused source/scheduler tests, including count/time budgets, deep unwind, falsy failures, cancellation and cleanup. Browser hidden-page behavior remains R7.
`git diff --check` passed. Logs: `/tmp/starplayer-w15-remediation-evidence/`.

## Browser follow-up (2026-10-27)

R7 and independent probes reproduced native continuation priority starving ordinary
publication/search timers. [R5a](W15-remediation-R5a-background-priority.md) supersedes
the direct scheduler.yield native branch with background postTask plus yield. Existing
source budgets and fallbacks stay intact. R5a is reviewed/integrated as `9580cca`;
its natural 40k gate passed. R7 is repeating final combined acceptance.

Final combined Node/Rust/WASM/CI, Chromium and Pages gates passed; see
[R7 final evidence](W15-remediation-R7-integration-evidence.md#final-execution-evidence-2026-10-27).
Native-picker, real-collection and device acceptance remain owner checks.
