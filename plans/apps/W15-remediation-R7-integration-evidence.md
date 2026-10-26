# W15 remediation R7 — Combined regression, browser/performance gates and docs

| Field | Value |
|---|---|
| Status | **Specified / outstanding — reviewed plan, amended 2026-10-26** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | R4b/R5/R6; all prerequisites integrated |
| Ownership | w15-headless.mjs/headless harness, LIBRARY-CSV.md, help text after R6 handoff |
| Branch / sibling worktree | `w15-remediation-r7` / `../starplayer-w15-remediation-r7` |
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

All changes need combined verification: existing tests passed while the reported
issues remained. Temporary repro scripts assert bad behavior and must be converted
to permanent positive assertions; do not report their success as a fix.

## Deliverables

- Add real runtime/controller/browser regressions for all 1–6/10, using synthetic
  OPFS/files/archives. Cover rapid replay of mutable A then B/C without an explicit scan:
  first seed A/B/C with coherent metadata+SHA, then assert no empty-SHA interval for
  these successful replays: prior coherent tuple remains until new tuple publishes
  atomically, eventually matching latest bytes and sidecar. Separate first-play tests
  require unknown/empty until success; digest failures deliberately clear SHA with issue. Cover excluded-only/mixed/error metadata states, and the active
  library continuing while a chooser, invalid index or identity failure is pending.
  Include before-row versions and page digests spanning a worker trap AND explicit
  Retry, Cancel metadata and recycling. Verify group A/B/C continues B/C when playback
  invalidates A; no shared-group cancellation. Check sync digest throw vs async rejection,
  stale-promise cleanup and callback throws before/after application with controller
  issue insertion and terminal descriptor cleanup. Also retain explicit group cancellation. These are contract gates, not satisfied by checking only final progress
  text.
- Run real ordinary/nested group scans of 3/64/130 members, source sizes below/above 32
  MiB, and report read/recycle/inner-decode counts against master D. Catalog/playback
  overhead is separately attributed. Interleave playback/input/cancel and assert no
  active playback change. Preserve core sample-exact/audio safety tests.
- Measure page-side digest behavior under bounded rapid-replay fixtures: peak physically
  outstanding promise count (including superseded versions), peak SUM(input byteLength
  for outstanding calls), synchronous invocation/start time (including native input-copy
  cost), activation latency and digest settlement latency. The summed pending input
  bytes are a conservative input-snapshot budgeting estimate, NOT measured total browser
  memory or a proven upper bound on WebCrypto overhead/copies. Controller no-copy and
  native allocation must be distinguished. Instrument scalar events only; no extra byte
  retention. Use at most 8 rapid plays per fixture run, then await settlement and cleanup;
  include approved synthetic valid loose modules at representative 1/32/64/128 MiB source
  sizes and an archive-extracted input up to 64 MiB. Large sizes may use accepted trailing
  padding only if the existing loader accepts it; keep the actual accepted input size in
  evidence. Do not alter format validity or limits to meet a measurement. Include native
  subview snapshot correctness and deterministic delayed-digest unit tests for overlap.
  Report environment, repeat count, p50/p95/max and limitations; unexpected cost/retention
  is a coordinator follow-up trigger, not authorization for a queue/cap/recovery design.
  No activation-faster/slower-than-hash assumption is an acceptance assertion.
- Extend browser performance harness to 100k rows/10k issues with tree open, actual row
  additions and metadata-only batches, active search/sort/scroll/keyboard focus.
  Instrument full scans/sorts, tree builds, issue DOM counts, longest tasks, p50/p95
  update/input latency and baseline vs revised counts. Deterministic gates use master
  operation bounds and interleaved control acknowledgments before gated jobs release;
  include measured timing without hardcoding machine-independent speed claims. Record
  the longest task for a user-initiated full re-sort at 100k rows; above 100 ms, report
  it as a follow-up candidate (master E), not as a failure of this task.
- Add a dedicated background-browser run with normal throttling (remove any inherited
  disable-background-timer-throttling/rendering/occlusion flags). Open another tab,
  prove document.visibilityState==='hidden', silent/no playback, observe enumeration
  and a chained-timer control for at least 6 minutes. Run a finite synthetic enumeration
  both just after hiding and again after 5 minutes hidden, so an early-completing walk
  cannot stand in for the intensive-throttling case. Keep dataset memory bounded. Also
  record how long pending library deltas take to appear after the page becomes visible
  again (master B, Hidden pages); report more than a few seconds as a follow-up candidate.
  Log browser version/flags, yield
  branch, visibility transitions, records over time and resumption. Do not use simulated
  visibility alone or disable throttling then claim background coverage. If environment
  cannot genuinely hide/freeze, report the gate unexecuted and require a real-browser
  run before acceptance. Separately freeze/thaw via supported browser lifecycle tooling
  and assert safe recovery, not progress while frozen. No blanket throughput promise.
- Document #7 unknown directory Size/Date/no stat enrichment; #8 uncertain stored-handle
  comparisons cause conservative session-only behavior (known matches later in registry
  still work), reconnect/reselection/explicit rebind limitations; no automatic eviction
  or advice to clear all storage as routine fix. A registry-repair UI remains deferred.
- Document #9 first raw duplicate wins consistently for catalog and legacy upload/
  playback; retain classic/ZIP64 tests. Explain #11 Rebind pending means match set not
  known until scan completes, not a claim matches exist; preserve one-click full rebind.
- Supply coordinator exact evidence and remaining manual owner checks. Coordinator
  alone updates active plan status/index/landing statements (#12); preserve archived
  W15 historical evidence and honest not-merged/not-deployed status.

## Research points

Inspect existing headless launch args before adding hidden runs. Separate correctness
failures from environmental inability to emulate a real hidden page. A 6-minute timer
observation is evidence for this browser/configuration only. Test-only counters must
not retain module buffers or alter job ordering. No owner library scans are authorized.

## Verification

Run focused new tests then `node --test --test-isolation=none apps/starplayer-web/test/*.test.mjs`,
`cargo test -p starplayer-archive -p starplayer-web`, `cargo xtask wasm`,
`STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`, Pages packaging
and local subpath worker smoke, `cargo xtask ci`. Run dedicated background/performance
harness and record its exact command, flags and artifacts. Retain prior all-format/
remote/upload/reconnect/legacy tests, and add trap-through-actual-endpoint coverage.
No implementation tests are claimed passed by this planning document. Coordinator
reviews all gates; native picker/real collection owner acceptance stays pending.

## Out of scope

Deployment/main merge, scans of owner collection, weakening tests to meet timings,
identity-record deletion, changing duplicate policy, claiming frozen-page throughput,
changes in other owners' production files without explicit serialized handoff.
