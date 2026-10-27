# W15 remediation R7 — Combined regression, browser/performance gates and docs

| Field | Value |
|---|---|
| Status | **Integrated — reviewed; all automated gates passed, manual owner acceptance pending** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | R4b/R5/R5a/R6/R6a; measured corrections must integrate before final acceptance |
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
Execution results are recorded below and in the master. Coordinator reviewed all
gates; native picker/real collection owner acceptance stays pending.

## Out of scope

Deployment/main merge, scans of owner collection, weakening tests to meet timings,
identity-record deletion, changing duplicate policy, claiming frozen-page throughput,
changes in other owners' production files without explicit serialized handoff.

## Scheduling gate correction (2026-10-27)

R7 reproduced native timer starvation in natural 40k walks; this was a production
failure. R5a corrected the helper and its integration passed the final gates. Preserve
existing natural40k assertions and rerun full browser/hidden/performance gates after
reviewed integration. Pre-correction results remain labeled as diagnostic evidence.

## Retained keyboard gate (2026-10-27)

The original indexed-library Ctrl+A count failed after R6. R6a corrects the virtualized
keyboard recount; preserve existing assertions and add End/Shift+Home/Escape counts.
R6a was reviewed/integrated before the passing final full-browser run.

## Final execution evidence (2026-10-27)

Integrated as `beafb22` after coordinator diff/evidence review. All required automated
gates passed on production baseline `8519ff2` plus the six owned harness/help/docs
files. The integration app/archive sources match the tested R7 branch. The coordinator
then independently rebuilt the integration package and passed the new SAB remediation
and local Pages smoke cases. The complete final Node suite passed all 300 tests.

Reproducible commands (from the integration checkout, Chromium required):

```sh
node --test --test-isolation=none apps/starplayer-web/test/*.test.mjs
cargo test -p starplayer-archive -p starplayer-web
cargo xtask wasm
STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs
STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --w15-case remediation
STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --w15-case enumeration --mode sab
STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --w15-case background --mode sab
STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --w15-case visibility --mode sab
STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --w15-case performance --mode sab
STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --w15-case digest --mode sab
cargo xtask wasm --pages
STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --pages-smoke
cargo xtask ci
```

Full Chromium runs passed SAB/fallback/plain, existing indexed 37,674-row and natural
40k regressions, actual WASM trap/Retry, all 12 ordinary/nested archive matrices per
mode and callback cleanup. Native inner-cache/owned-byte counters are Rust cfg(test)
evidence only; JS adapter calls are not reported as native decompression counts.
31 archive tests and 22 web tests plus doc tests passed; one pre-existing manual archive
generator remains ignored. All 11 CI jobs passed. Pages smoke used the shipped
service worker under a local `/starplayer/` path; nothing was deployed.

The dedicated background run was genuinely hidden for 390,195 ms, silent, with normal
throttling. Early and after-five-minute walks completed, and the same frozen iterator
resumed safely. A separate quick nonterminal hidden/visible test measured actual
row-change notification about 122 ms after return while another digest stayed pending.
The six-minute terminal probe is not claimed as pending debounce proof. No forced
flush, simulated visibility or background-throttling disable flags were used.

The isolated 100k/10k workload passed operation/DOM/input/anchor bounds. User full
re-sort was 81.910 ms synchronous / 88 ms overlapping long task, below the 100 ms
follow-up trigger. Its overall longest task, including initial construction, was
654 ms; no universal latency claim is made. Native loose 1/32/64/128 MiB and extracted
64 MiB fixtures all passed correctness/cleanup. Large-input cost and all distributions
are preserved in the [separate profiling task](W15-followup-large-digest-start-cost.md).
No queue/cap/recovery design was introduced.

Evidence directory: `/tmp/starplayer-w15-remediation-evidence/`.
`r7-evidence.json` records exact commands/flags, per-artifact SHA-256 values, source/dist
identity, distributions, derived baseline counts, manual checks and rejected/stale
runs. All listed artifact hashes were independently checked. The permanent package
preflight checks ten source/dist JS files before launching. The corrected yield helper
SHA-256 is `c4eb5884fe93719d933f8b9cf55702d4d1a82759e3ab535e0958836d2784d297`;
the final view is `0308e7f6bc460040d4abbcfeb2b05e40e7ebdcb73aee7b9693d19d25274908d8`.
Build xtask in the intended checkout: its compiled workspace root can make a reused
foreign-worktree target package the wrong tree. Final coordinator builds used their
own integration target and confirmed the output destination.

Environment: Chromium 156.0.8078.4, Node 24.14.0, Rust 1.97.1, WSL2 Linux 6.6.87.2
x86_64, AMD Ryzen 9 5950X, 32 logical CPUs, approximately 25.4 GiB RAM. Hidden launch
flags were `--headless=new`, `--remote-debugging-port=<ephemeral>`,
`--remote-allow-origins=*`, `--no-sandbox`, `--disable-gpu`, `--disable-dev-shm-usage`,
`--autoplay-policy=no-user-gesture-required`, `--mute-audio`,
`--user-data-dir=<temporary-profile>`, `about:blank`. Actual generated ports/profile
paths for each run are in the manifest; no throttling-disabling flag was present.

Native-picker permissions, real collection/device responsiveness and real saved-playlist
reload/reconnect/FileList/storage-denied/rebind checks remain manual owner acceptance.
The [master's final record](W15-remediation-master-plan.md#final-reviewed-delivery-and-verification-2026-10-27)
is authoritative. No main merge, push or deployment occurred.

## Post-integration review follow-up (2026-10-27)

The owner-requested post-integration review produced [R8](W15-remediation-R8-trap-cleanup.md) and [R9](W15-remediation-R9-playback-scan-status.md), both closed on re-review at `a18d224`. The complete repeated Node suite is 309 tests; full three-mode Chromium, Rust/WASM/Pages, 11-job CI and repeated hidden/visibility checks passed. Initial R7 evidence above remains historical; see the [master final review record](W15-remediation-master-plan.md#review-closure-and-final-post-review-gates).


## R10 actual-consumer coverage (2026-10-27)

The full-feature Astra review found membership work outside the isolated view
benchmark. [R10](W15-remediation-R10-playlist-membership.md) fixes it and adds bounded
100k actual playlist-controls regressions. Both original Claude and Astra-medium
reviews closed the finding. Fresh 314 Node, W15 SAB/fallback/plain, development/Pages,
22 web source-contract tests and all 11 CI jobs passed; twelve package preflight
identities now include both playlist consumers. Earlier audio/hidden/performance
measurements remain historical, not new R10 timing claims. See the
[master closure](W15-remediation-master-plan.md#r10-review-closure-and-final-verification).
