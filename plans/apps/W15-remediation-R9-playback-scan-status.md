# W15 remediation R9 — Preserve scan intent during playback refreshes

| Field | Value |
|---|---|
| Status | **Integrated — independently re-reviewed/closed; all final automated gates passed** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | Integrated R2/R6/R7/R8; browser ownership after R8 handoff |
| Ownership | library-discovery.js and focused discovery tests; w15-remediation-headless.mjs for actual view assertions |
| Branch / sibling worktree | `w15-remediation-review-status` / `../starplayer-w15-remediation-review-status` |
| Authorization | Worker implements and verifies; no staging, commit, merge, push or deployment |

## Context for a fresh agent

Read AGENTS.md, [master](W15-remediation-master-plan.md), [R2](W15-remediation-R2-metadata-state.md) and [R6](W15-remediation-R6-incremental-view.md). The owner requests iterative review/fixes on w15-remediation. Review round one found the fatal cleanup defect addressed by R8 and a second status regression. No cargo fmt, historical assembly edits, attribution or unrelated changes.

In `status()`, refreshPending alone enters scan phase logic. Playback in an idle or cancelled discovered library changes metadataPhase to running, then complete on digest settlement. The view consequently hides Scan metadata, shows ineffective Cancel metadata, and later claims a completed scan although other rows remain unknown. Master B says scan complete describes job completion; page digest pending/current-refresh counts and hints already describe playback work separately. Coordinator diagnostic `/tmp/starplayer-w15-remediation-review/playback-scan-status-repro.mjs` reproduces idle/cancelled -> running -> complete without Scan (it asserts bad behavior only).

## Deliverables

- Base scan phase transitions on actual scan intent or inspection descriptors. A page-only refresh must preserve idle/cancelled/previous completed phase, while current pending-digest counters and metadataIncomplete remain accurate.
- A real active Scan still waits for its current eligible playback refreshes and discovery, and explicit Retry metadata still reports its inspection job running/completed. Stale-only physical digests must not block current completion. Cancel metadata still preserves page digests and retains cancelled phase through their settlement.
- Add positive deterministic regressions for no-scan idle playback, playback after Cancel, genuine Scan waiting for playback, and explicit row Retry if needed to protect phase semantics. Prove new regressions fail the baseline before applying the narrow fix.
- Add an actual Chromium view/controller test with another eligible unknown row: during pending and settled playback, Scan stays visible/actionable and Cancel hidden; after a cancelled scan the cancelled label remains. Use a gated WebCrypto-compatible digest, drive actual view notifications, retain existing R8 poisonedSessionTrapCleanup and all prior cases. A helper within w15-remediation-headless.mjs is acceptable; add an evidence flag. Coordinator runs browser modes/builds after integration.
- Report exact changed paths/tests/results and limitations. Coordinator owns master/index/status/evidence. No shared API or hashing policy changes.

## Research points

Study status() side effects, scanActive, inspected and refreshPending across ordinary scan, row Retry, cancellation, playback supersession and source disposal. Keep physical hashesPending distinct from current refreshPending and scan state. Avoid hiding actionable Scan because unrelated playback is hashing.

## Verification

Run `node --test --test-isolation=none apps/starplayer-web/test/library-discovery.test.mjs apps/starplayer-web/test/library-discovered-model.test.mjs apps/starplayer-web/test/library-view-state.test.mjs` and the complete Node suite; syntax/diff checks. Coordinator independently verifies focused tests and all three Chromium remediation modes after rebuilding the integration package, plus final applicable regression/CI/Pages gates.

## Out of scope

Digest queue/cap/recovery, cancellation of native promises, changes to eligible counters or source acceptance, UI redesign, archive/runtime production edits, cargo fmt, owner collection scans, deployment/main merge. Remove the temporary task branch/worktree after reviewed integration, preserving w15-remediation.

## Execution evidence (2026-10-27)

Integrated as `a18d224`. Coordinator reviewed the three-file diff and independently
passed all 59 required focused tests. Worker baseline selected six regressions, with
five expected failures and one active-Scan preservation pass; final full Node suite
passed 309 tests, also independently repeated in the integration checkout. New actual
view/controller gate playbackRefreshPreservesScanControls checks idle/cancelled labels,
Scan/Cancel visibility, current digest hints and a remaining unknown row through pending
and settled playback; all prior R8 cases remain. Final combined browser/CI/re-review passed. Evidence under `/tmp/starplayer-w15-remediation-review/`:
r9-worker-baseline.log, r9-worker-focused.log, r9-worker-full-node.log,
r9-coordinator-focused.log and final-node.log.

Final re-review at `a18d224` closed this finding. Full 309 Node tests, Rust/WASM, all
three Chromium modes, Pages smoke, 11-job CI and repeated hidden/visibility checks
passed; see the [master final review record](W15-remediation-master-plan.md#review-closure-and-final-post-review-gates).
The temporary task branch/worktree was removed after clean integrated-patch checks.
