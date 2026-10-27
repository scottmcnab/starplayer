# W15 remediation R10 — Bound playlist membership work during metadata updates

| Field | Value |
|---|---|
| Status | **Integrated — original Claude and Astra reviews closed; final automated gates passed** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | R2/R6/R7/R9; reviewed integration baseline `72489c0` |
| Ownership | playlist-transfer.js; playlist-transfer/control regression tests; narrow headless source-preflight list extension |
| Branch / sibling worktree | `w15-remediation-membership` / `../starplayer-w15-remediation-membership` |
| Authorization | Worker implements/verifies only; no staging, commits, merges, pushes or deployment |

## Context for a fresh agent

Read AGENTS.md, [master](W15-remediation-master-plan.md), archived [F](complete/W15-task-F-playlist-playback-integration.md), and the producer/consumer code/tests. The owner requests fixing the medium full-feature review finding, followed by the original Claude CLI session and Astra-medium reviewing the fix. The full review report and diagnostic are `/tmp/starplayer-w15-astra-review/review.md` and membership-repro.mjs/json. The diagnostic asserts the bad behavior and is not an acceptance test.

At the reviewed baseline, `keysForLibrary` invalidated on any library.revision. Once discovery completed, playlistControls.libraryUpdated -> updateRebind -> rebindCandidates rebuilt all locator keys even for an empty playlist. The 100k-row diagnostic counts two million row visits for twenty one-row title updates; instrumented Node mean 28.63 ms / max 36.67 ms. Existing view-only performance counters omit this consumer. Model rows/IDs/locators are append-only; metadata/sidecar revisions change no membership. This narrow fix must preserve all prior playback/digest/identity/Rebind contracts.

## Deliverables

- Replace general-revision invalidation with bounded membership maintenance. Preserve `keysForLibrary` as a Set-based `.has` lookup. A cached Set is now explicitly a live membership view refreshed on keysForLibrary access: reuse it across metadata/comments/status updates and extend only newly appended locators; do not copy/rebuild all old keys on append. Keep caches isolated by library; preserve legacy indexed/rows-only fixture semantics. Defensively reset if the rows-array reference is replaced or shrinks; unsupported in-place locator edits are not a new API requirement.
- This explicitly supersedes archived F's revision-aware membership clause and the test that demands a different Set identity after append. Replace that implementation-specific assertion with stronger live-membership behavior: held lookup sees arrivals after helper refresh, metadata/sidecars preserve correct membership, and appended rows become available/rebindable. Do not weaken any behavioral, identity, complete-discovery, import/export or playback assertion.
- Add meaningful integration regressions through actual createPlaylistControls/libraryUpdated, including empty and populated playlists, completed discovery, metadata-only and sidecar changes, status-only updates, later row arrivals, source/library replacement and legacy indexed behavior. Use deterministic counts of row/locator access robust to replacing `.map` with another traversal; do not rely on wall-clock thresholds. The 100k/20 batch workload must show zero full-library membership traversals and bounded changed-row work; appends process only new keys. Preserve partial-discovery Rebind gating and newly arrived availability.
- Extend the permanent headless source/dist preflight with playlist-transfer.js and playlist-controls.js so the actual consumer fix cannot be masked by stale packaged assets. No browser production hooks or unrelated harness rewrite.
- Keep production scope small: no new model revision counter where append-only row count suffices, no queues/caps/scheduler/view rewrites. Report diff, baseline failing regressions, positive tests/counts and limitations. Coordinator owns plan/index/status, commits and final review loops.

## Research points

Audit every keysForLibrary caller and whether it retains/iterates a lookup; today production consumers use `.has` immediately. Distinguish mutable metadata revisions from immutable locator identity. Prefer stable live Set behavior over copying an N-entry Set on append. The legacy plain rows-only library fixture lacks revision; parseLibraryCsv real models already have locatorIds. Ensure cache behavior does not conflate row ID0 with missing membership or expose one library's members in another.

## Verification

First demonstrate permanent regressions fail on baseline. Run `node --test --test-isolation=none apps/starplayer-web/test/playlist-transfer.test.mjs apps/starplayer-web/test/playlist-controls.test.mjs apps/starplayer-web/test/playlist-model.test.mjs apps/starplayer-web/test/playback-controller.test.mjs` and complete web Node suite (localhost may require authorized escalation). Run syntax/diff checks. Save logs and a positive before/after operation-count reproduction under `/tmp/starplayer-w15-membership-fix/`; diagnostics must not assert bad counts after the fix.

Coordinator independently reruns focused/full Node tests, builds correct integration development/Pages packages, runs W15 Chromium cases in SAB/fallback/plain and local Pages worker smoke, verifies packaged consumer identities, and obtains read-only reviews from Claude session e80ca7f5-7ec4-43b0-9549-fcc7c76304f5 and Astra at medium. Fix new substantive findings and repeat applicable checks before closure. Engine/Rust/audio implementation is unchanged; do not claim prior CI/performance/hidden runs as fresh executions. No cargo fmt or historical sources.

## Out of scope

Main merge, push/deployment, owner collection scans, changing locator/identity/metadata semantics or discovery-completion gating, digest profiling implementation, broad performance refactors, attribution. Remove the temporary task worktree/branch after reviewed integration, preserving main w15-remediation.

## Execution and review closure (2026-10-27)

Integrated as `f3f2171` after coordinator diff review and independent 40-test focused
verification. The worker never staged, committed or merged; the coordinator committed
the four-file task change and squashed it from the integration checkout. The original
six failing baseline regressions (34/40 passed) now pass; full integration Node suite
passes 314 tests with zero skipped. Positive actual-controls operation bounds fail on
the baseline and pass on the integrated source. For 100k rows and twenty one-row
metadata updates, row reads fell from 2,000,020 to 20 and locator-field reads from
6,000,120 to 120. Status-only reads remain zero. A one-row append fell from 100,002
to two row reads and 300,003 to three locator-field reads. Empty and populated
playlists, sidecar attach/replace/removal, arrivals, actual Rebind, identity isolation,
current/order, replacement/shrink and legacy indexed behavior are covered.

The original Claude CLI session e80ca7f5-7ec4-43b0-9549-fcc7c76304f5 and original
Astra-medium reviewer independently closed the P2 with no new substantive findings.
Astra additionally reran 40 focused tests, the positive 100k operation count and 500
oracle-checked boundary transitions. Both accepted the explicit refreshed-on-access
Set contract and stronger replacement of the former implementation-specific Set
identity assertion. Unsupported in-place locator edits/sparse arrays are not produced
by the existing dense, append-only model; no new support is claimed.

Coordinator final gates passed: 314 Node tests, 22 web Rust source-contract tests,
correct development/Pages builds, W15 browser cases in SAB/fallback/plain, local
Pages worker smoke, twelve critical source/dist identities and all 11 CI jobs.
Browser verification here used `--w15-only`; the unfiltered 60-second audio run and
hidden/native/view timing measurements were not repeated and remain prior evidence.
No new latency claim is derived from Node operation counts. Initial/replacement
membership builds, producer sidecar attachment and explicit import/Rebind full passes
remain permitted. No engine/Rust implementation, hashing, worker, scheduler, model,
identity or scan-gating policy changed.

Artifacts and reproducible commands are under `/tmp/starplayer-w15-membership-fix/`:
`final-evidence.json`, `coordinator-*.log`, `membership-counts.mjs` (pass integration
checkout as its argument), `coordinator-after.json`, baseline/fixed-counts files,
`claude-review-result.txt` and `astra-review.md`. All listed artifact hashes are
verified by `validate-final.py`. The clean temporary task worktree/branch was removed
after verifying its source/test patch exactly matches the integrated commit.
See the [master closure](W15-remediation-master-plan.md#r10-review-closure-and-final-verification).
