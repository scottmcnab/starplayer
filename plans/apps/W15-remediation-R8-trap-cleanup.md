# W15 remediation R8 — Preserve fatal traps through archive cleanup

| Field | Value |
|---|---|
| Status | **Integrated — independently re-reviewed/closed; all final automated gates passed** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | Integrated R4b and R7 |
| Ownership | library-worker-jobs.js, focused worker/group tests and w15-remediation-headless.mjs; narrow Rust binding comment if needed |
| Branch / sibling worktree | `w15-remediation-review-fixes` / `../starplayer-w15-remediation-review-fixes` |
| Authorization | Worker implements and verifies; no staging, commit, merge, push or deployment |

## Context for a fresh agent

Read AGENTS.md, [master](W15-remediation-master-plan.md), [R4b](W15-remediation-R4b-grouped-inspection.md), and actual worker/group tests. All initial remediation tasks are integrated and verified at `1d40613`. The owner has requested iterative review with the original external reviewer and fixes until substantive findings are resolved. No cargo fmt, historical assembly edits, attribution, weakened tests or unrelated changes.

The coordinator reproduced a fatal-boundary defect in `executeLibraryJob`: its unconditional `finally { session?.free(); }` can replace an extraction RuntimeError with a secondary ordinary cleanup exception. In wasm-bindgen a trap during an exported mutable method can leave the borrow flag poisoned; freeing then throws an aliasing/borrow error. The endpoint sees only that error and reports job-failed, so the runtime keeps using a trapped WASM instance. Reproduction: `/tmp/starplayer-w15-remediation-review/trap-cleanup-repro.mjs` invokes an actual tiny-WASM unreachable trap from extract_member and throws an ordinary borrow/aliasing error from free; the baseline emits job-failed. This script asserts the bad behavior and is diagnostic only.

## Deliverables

- Preserve the original RuntimeError at the worker endpoint. Once a WASM trap occurs, do not reenter that poisoned WASM instance to free the archive session; runtime retirement/worker termination releases the entire instance. Keep the original worker-failed/wasm-trap classification and explicit Retry behavior.
- Healthy success and ordinary per-member errors still free the session exactly once. A RuntimeError raised during otherwise-normal cleanup remains fatal. Constructor, extraction, inspection and metadata-access traps remain fatal, including after partial member delivery. No queues, recovery, protocol changes or new policy machinery.
- Add focused endpoint/runtime regressions combining a real RuntimeError with a secondary cleanup exception, proving the poisoned free is skipped, undelivered work stays suspended, progress is retained, a queued next job cannot use the instance, and explicit controller Retry creates a healthy instance. Preserve ordinary-data-error continuation and healthy freeing assertions.
- Add a browser regression in w15-remediation-headless.mjs using a real worker/endpoint and real archive session wrapper: deliver A, throw a tiny-WASM RuntimeError extracting B, and make poisoned free throw an ordinary aliasing error if entered. Assert fatal classification, no poisoned cleanup entry, partial progress, queued work suspension and healthy explicit Retry. Report a new evidence flag and retain existing cases. Coordinator runs the combined browser harness after review.
- If a Rust doc comment promises unconditional free, clarify the healthy-cleanup / fatal-worker-retirement distinction only; no Rust implementation change is expected.
- Report changed paths, verification commands/results, and limitations. Do not edit master/status/index; coordinator owns final evidence and integration.

## Research points

Inspect generated wasm-bindgen session glue and its borrow ownership behavior rather than assuming cleanup cannot throw. Distinguish a fake transport from an actual RuntimeError at startLibraryWorker. Existing constructor/extraction/free trap tests cover each separately but do not cover a cleanup error masking an earlier trap. Keep callback exceptions nonfatal as reviewed; they are unrelated to poisoned WASM.

## Verification

Run `node --test --test-isolation=none apps/starplayer-web/test/library-inspection-groups.test.mjs apps/starplayer-web/test/library-worker-runtime.test.mjs apps/starplayer-web/test/library-discovery.test.mjs` and the complete web Node suite. Supply a positive combined-trap regression with baseline failure verified. Coordinator rebuilds WASM and runs real Chromium remediation cases in all three modes, Pages packaging/smoke and applicable final checks after integration.

## Out of scope

Changing page hashing, source acceptance, archive grouping/limits/duplicates, incremental view architecture or any reviewed withdrawn queue/ACK/fairness design; cargo fmt, owner collection scans, deployment/main merge. Temporary review worktree/branch will be removed after reviewed integration, retaining the main remediation branch.

## Execution evidence (2026-10-27)

Integrated as `6a89c0f`. Coordinator reviewed five production/test/comment paths,
independently passed 102 focused tests, rebuilt the integration package and passed
Chromium remediation SAB/fallback/plain with poisonedSessionTrapCleanup:true. Worker
full Node suite passed 303 tests. Baseline regression suite produced five expected
failures. An isolated actual wasm-bindgen 0.2.127 mutable-borrow panic probe also
reproduced ordinary ownership error masking on baseline and fatal RuntimeError
classification after the fix; this minimal ABI probe is not a malformed StarPlayer
archive reproduction. Healthy ordinary-error/session cleanup assertions remain.
Artifacts under `/tmp/starplayer-w15-remediation-review/`: r8-worker-baseline.log,
r8-coordinator-focused.log, r8-worker-full-node.log, r8-wasm.log, r8-browser.log,
borrow-probe-result.log; probe source/build inputs are in borrow-probe/.

Final re-review at `a18d224` closed this finding. Full 309 Node tests, Rust/WASM, all
three Chromium modes, Pages smoke, 11-job CI and repeated hidden/visibility checks
passed; see the [master final review record](W15-remediation-master-plan.md#review-closure-and-final-post-review-gates).
The temporary task branch/worktree was removed after clean integrated-patch checks.
