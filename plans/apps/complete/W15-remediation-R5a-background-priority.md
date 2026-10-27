# W15 remediation R5a — Fair native cooperative yields

| Field | Value |
|---|---|
| Status | **Landed on `main` via `ece531f` (2026-10-27) — reviewed/verified; manual feature acceptance pending** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | Integrated R5; preserve integrated R2/R4b notification contracts |
| Ownership | www/cooperative-yield.js and test/cooperative-yield.test.mjs only |
| Historical branch / sibling worktree | `w15-remediation-r5a` / `../starplayer-w15-remediation-r5a` |
| Authorization | Implementation/integration authorized; worker must not stage, commit or merge |

## Context for a fresh agent

Read AGENTS.md and [remediation master](W15-remediation-master-plan.md). JS/test paths
are under apps/starplayer-web/. The reviewed implementation is on w15-remediation.
R7 found that ordinary scheduler.yield continuations starve normal timers in Chromium
156: natural 40k Directory/FileList walks lasted 350–465 ms but published only the
completion update. Independent 400k walks lasted about four seconds with zero timer
polls/notifications before completion, though animation frames ran. This affects the
80 ms search debounce as well as the controller's 100 ms publication timeout.

A real-browser experiment with the controller unchanged established that
scheduler.postTask(() => scheduler.yield(), {priority:'background'}) restores partial
notifications and timer callbacks. Directory/FileList 100k/400k walks emitted 9–38
partial updates and 96–394 timer polls; 400k duration remained about four seconds.
Evidence: /tmp/starplayer-w15-remediation-evidence/r5-priority-probe.jsonl. Temporary
probes are reference evidence, not a test dependency. The master now requires this
native priority boundary. R7 owns shared headless files and will repeat all final
browser/performance/hidden gates after this correction is integrated.

## Deliverables

- Prefer the native path only when BOTH scheduler.postTask and scheduler.yield are
  callable. Invoke yield inside a background-priority postTask callback; preserve
  receiver binding. Do not invoke an unprioritized yield directly from the page task.
- If either native method is absent, retain MessageChannel, then timer as last fallback.
  Keep yieldCooperatively({signal}) and injected factory signatures compatible. Diagnostic
  cooperativeYieldMethod returns `scheduler.postTask(background) + scheduler.yield`
  for this native branch, otherwise existing MessageChannel/setTimeout names.
- Preserve prompt abort before/during/after yielding, exact-once settlement, listener/
  port/timer cleanup and rejection of all reasons including falsy values. An abort
  before a queued native callback runs must prevent that callback invoking yield.
  Late native success/rejection must be handled silently without extra settlement.
- Extend focused tests for both native capabilities, background priority and receiver,
  missing postTask/yield fallbacks, queued-native abort and falsy postTask/yield failures.
  Preserve all applicable existing cancellation/fallback tests. Production source and
  discovery/controller/view files remain unchanged; do not add inline flushes, visibility
  listeners, a queue/fairness state machine or timer chains.

## Research points

The native yield priority is inherited from its surrounding postTask. Chrome documents
boosted continuations and background inheritance at
https://developer.chrome.com/blog/use-scheduler-yield#priority_inheritance ; current
specification: https://wicg.github.io/scheduling-apis/ . Do not use outdated proposed
priority arguments to yield(). Distinguish responsiveness from frozen-page progress.

## Verification

Run `node --test --test-isolation=none apps/starplayer-web/test/library-source.test.mjs apps/starplayer-web/test/library-source-enumeration.test.mjs apps/starplayer-web/test/cooperative-yield.test.mjs`.
Run `cargo xtask wasm` (reuse an inactive target if useful), then
`STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --w15-case stress --mode sab`.
The existing natural 40k Directory AND FileList before-completion usability, input,
heartbeat, pause/resume, scroll/focus and zero-content-read assertions must pass.
Do not replace them with a barrier or animation-frame-only observer. Record native
branch diagnostics and results under /tmp/starplayer-w15-remediation-evidence/r5a-*.
R7 repeats the full suite and six-minute normal-hidden run on integrated final code.

## Out of scope

Controller publication changes, UI/headless edits, changing 128-record/8-ms source
budgets, synthetic activity/browser throttling flags, performance claims for other
browsers, engine/RT/no_std work, cargo fmt, historical assembly or attribution.

## Execution evidence (2026-10-27)

Integrated as `9580cca`. Coordinator reviewed the two-file diff and independently
passed all 45 focused source/scheduler tests and the unchanged Chromium SAB 40k
stress gate: first usable rows 7,551 directory / 10,880 FileList, heartbeat ticks
187 / 175, 18 visible rows and zero content reads. WASM packaging passed. The
controller stayed unchanged; final combined coordinator/R7 gates passed.
Evidence: `/tmp/starplayer-w15-remediation-evidence/r5a-*.log`.

Final combined Node/Rust/WASM/CI, Chromium and Pages gates passed; see
[R7 final evidence](W15-remediation-R7-integration-evidence.md#final-execution-evidence-2026-10-27).
Native-picker, real-collection and device acceptance remain owner checks.
