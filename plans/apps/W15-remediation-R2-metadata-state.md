# W15 remediation R2 — Page-side playback digests, eligible counters and model deltas

| Field | Value |
|---|---|
| Status | **Integrated — reviewed; focused and final combined automated gates passed** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | Baseline; parallel R1/R3/R4a/R5 |
| Ownership | library-discovery.js, library-index.js and controller/model unit tests |
| Branch / sibling worktree | `w15-remediation-r2` / `../starplayer-w15-remediation-r2` |
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

Finding #1: `acceptPlaybackMetadata` patches `sha256:''`, then enqueues a worker `hash`
job, and each new hash cancels the previous one. Rapid playback of A then B therefore
leaves A with no SHA, and its sidecar comments detach. Finding #2: unplayable rows are
counted as unknown and missing-SHA work, in both `countRow` and `prepareComments`, so
the metadata hint and awaitingHashes never clear. This task also produces
rowChanges/issuesRevision for R6. R4b later owns the controller.

**Amendment (2026-10-26):** the first draft built an automatic recovery FIFO, consumed
`hash-deferred` and `onFatal` from R1, and added a visibility-flush listener. Master A
and B withdraw all three. The playback digest moves to the page.

## Deliverables

- Implement master A page digest with copied locator/scalar metadata, captured accepted
  context/generation and incremented locator version at entry. Inject
  digest(BufferSource)->Promise<ArrayBuffer> with call-time snapshot semantics; default
  WebCrypto SHA-256. Invoke before await; normalize exactly 32 digest bytes to 64 lowercase
  hex digits. Capture metadata independently of later caller mutation.
- Publish metadata+SHA once on current-version success; retain old coherent snapshot
  while pending, or unknown/empty SHA for first play. Sync throw, async rejection or
  invalid digest shape use the same current-version metadata+empty-SHA/path-issue path.
  Stale/disposed results clean records only. Never manufacture a checksum.
- Page digest registry/validity is independent of runtimeEpoch. Worker failRuntime,
  Retry/recycle and cancelMetadata must not clear it or suppress publication. Track ALL
  outstanding promises (including superseded versions) until their own finally cleanup;
  retain only byteLength and snapshots, not caller bytes. Native WebCrypto still owns
  an input copy. Disposal suppresses updates but cannot cancel native work. No queue,
  concurrency cap, recovery FIFO or worker-hash job is introduced.
- Prior to R4b independent scan jobs may cancel by locator. Hand off explicitly that
  R4b must replace that with member-version invalidation only: no cancellation of a
  shared group containing other locators. Before-row results cache only complete patches.
- Add test-only digest start/settle metrics using scalar IDs/byte lengths/timestamps for
  R7's count/input-byte estimate/start-cost/latency report. Do not retain bytes for metrics
  or infer a memory bound from playback rate. Existing 128 MiB loose and 64 MiB extracted
  input limits remain distinct; do not introduce a 64 MiB blanket module limit.
- Implement master B eligible-only counters in the controller AND in `prepareComments`
  previous/next-state accounting. Add `excluded`, `scanActionable`, `failedHashes` and
  `excludedHashes`; preserve indexed comment semantics and valid supplied-SHA joins.
  Add refreshPending for current-version pending eligible/before-row refreshes; keep
  hashesPending as all physically outstanding promises. Old ready+SHA retained during
  refresh still means metadataIncomplete with a refresh hint, not awaiting missing SHA.
  Stale-only promises do not block a verified newest row or Scan eligibility. Error-only
  rows need explicit Retry metadata, not Scan. Do not double-count when a row arrives.
- Add model `lastRowChanges` and controller-coalesced `rowChanges`/`libraryRevision`/
  `issuesRevision`. Report the fields that actually changed, including a resolved comments
  change. Preserve the existing changedRowIds/metadataUpdates consumers. Separate
  added, patched and status-only updates. Issue IDs are stable and append-only; no
  per-status serialization.
- Hand R6 schema examples plus counter/delta tests. Hand R4b the controller work
  descriptors and version-guard sites.

## Research points

Inspect cached patches and the changes emitted by attachComments; cached playback may
precede catalog rows later found unplayable. Scan completion must not imply an
error-free library. Confirm `crypto.subtle` is present wherever the page already runs
(secure contexts: localhost and Pages); the worker already depends on it.

## Verification

Run `node --test --test-isolation=none apps/starplayer-web/test/library-discovery.test.mjs apps/starplayer-web/test/library-index.test.mjs apps/starplayer-web/test/library-discovered-model.test.mjs`.
Use a deterministic injected digest that snapshots the passed byte range immediately
and resolves a 32-byte ArrayBuffer later. Seed A, B AND C with coherent metadata+SHA
(and sidecar) first; replay mutable A followed by rapid B/C without explicit scan.
Only these previously hashed, successful replay cases assert no empty-SHA interval:
keep prior coherent tuple until atomic update, eventually matching latest played bytes.
Separately assert first-play/unknown rows stay empty until success and current-version
sync throw/async rejection clears SHA with the same issue path. Include subarray input,
caller mutation/detachment after invocation, metadata/locator object mutation, hex zero
padding and invalid digest shape. Out-of-order latest version wins; stale success/failure
still decrements physical counters exactly once. A stale-only outstanding promise must
not keep metadataIncomplete true after the latest digest succeeded.
A page digest must span worker trap AND explicit Retry, scan cancellation and normal
worker recycling, then publish normally; do not guard it with runtimeEpoch. Cover
before-row versions and accepted-source disposal while pending (no post-disposal
publication but final cleanup). Use bounded delayed-digest tests to prove pending counts
and byte estimates, never to claim native speed/memory results.
Cover only-unplayable, mixed eligible/encrypted,
all-error, error+ready and sidecar before/after/replacement. A repeated Scan queues no
ineligible or error jobs, and actionable counts match queued jobs. Test exact deltas for
title-only, comments-only, hash-without-text, addition and status-only paths, plus
revision changes and indexed CSV regressions.

## Out of scope

DOM work, source acceptance, archive grouping, runtime edits, visibility listeners,
changing playback availability for metadata failures.

## Execution evidence (2026-10-27)

Integrated as `cc68dac` after coordinator diff review and independent focused Node verification.
54 controller/model/index tests passed; native input-view mutation/detachment, physical vs current pending counts, falsy failures, eligible sidecars and delta regressions included.
`git diff --check` passed. Browser verification remains R7.

Final combined Node/Rust/WASM/CI, Chromium and Pages gates passed; see
[R7 final evidence](W15-remediation-R7-integration-evidence.md#final-execution-evidence-2026-10-27).
Native-picker, real-collection and device acceptance remain owner checks.

## Post-integration review follow-up (2026-10-27)

[R9](W15-remediation-R9-playback-scan-status.md) subsequently corrected page-only refresh scan-phase transitions; the post-integration reviewer closed the finding and all final gates passed.
