# W15-F — Playlist continuity and playback enrichment wiring

| Field | Value |
|---|---|
| Status | Complete — reviewed and integrated on `w15-folder-library`; final automated acceptance passed |
| Recommended model | gpt-6.1-sol |
| Recommended effort | **high** |
| Depends on | D2 integrated; parallel with E; final app check after E |
| Branch / sibling worktree | `w15-f` / `../starplayer-w15-f` |
| Ownership | playlist JS, `app.js`, `playback-controller.js`, playlist/app unit tests; source-contract tests after B |
| Coordinator | gpt-6.1-sol, xhigh; [master](W15-master-plan.md) |

> Post-review amendment (2026-10-27): [R10](../W15-remediation-R10-playlist-membership.md)
> supersedes only the revision-aware membership-cache clause: the lookup is a live
> append-aware Set, reused across metadata/sidecar revisions. Historical evidence and
> all membership/Rebind behavior remain requirements.

## Context for a fresh agent

Read `AGENTS.md`, [W15 master](W15-master-plan.md), and the complete
[behavioral specification](W15-task-folder-library-discovery.md). The master defines
shared APIs; the behavioral specification defines edge cases and compatibility. Work
from this task file without relying on conversation history. Do not commit or start
another task. Report changes, verification results and handoff contracts to coordinator.
All paths below are repository-relative; web JS paths are under
`apps/starplayer-web/www/` unless stated otherwise.

Current availability text is also the boolean play gate; growing libraries must
separate them. Existing entries persist locator and display snapshots. Consume D2
notifications and E's fixed view contract; this task is the sole app.js writer.

## Deliverables

- Add playlist.updateDisplay batch API and persist through normal subscriber saves.
  Preserve entry IDs, ordering, current ID, selection and locators. Use display.metadataState
  and all import/export/legacy rules in the specification. Update every matching entry
  by identity plus locator, including legacy entries and rows played before discovery.
- Share C locator key and make keysForLibrary cache revision-aware (legacy immutable
  CSV libraries still work). Add libraryUpdated for incremental membership/status and
  metadata updates as the master's partial `{locator,patch}` records; suppress repeated unnecessary store writes when values unchanged.
- Separate availability text from canPlay. Matching discovered contexts permit saved
  locators before/after a missing scan row; resolver errors remain authoritative. Keep
  disconnected, upload-missing and indexed-not-listed gates unchanged. Delay explicit
  rebind until discovery completes; explain paused/failed pending state. Imported
  unavailable counts distinguish pending from missing.
- Wire app startup onLibrary vs onLibraryUpdate to the view and playlist controls,
  and preserve old indexed comments flow. Use canPlay in playlist-playback gating.
  Fix reconnect wording and empty-title labels. No new source, model or worker APIs.
- Capture B's metadata synchronously immediately after Loader.inspect_s3m, beside
  readMetadata and before any await; parse JSON to an object immediately. Catch
  accessor/parse errors and skip optional enrichment without failing playback. Retain
  that snapshot locally across activation;
  only after successful current/revision/node commit checks forward it with locator/
  bytes to the matching context's acceptPlaybackMetadata. Do not read mutable loader
  metadata after awaiting worklet activation. Extend playback-controller activation
  context with the locator captured by play() so library and playlist paths use the
  correct original locator without consulting later playlist selection. Gate against stale playback and library generation;
  apply to direct library and playlist playback, not unrelated upload playback.
  This call never delays activation or shares/detaches playback-owned bytes; do not
  reparse the module for enrichment. Preserve existing playback error behavior.
- Integrate persisted folder identities/reconnect and explicit rebind with indexed
  libraries, FileList and storage-denied sessions. E owns adding metadataState from
  the library view; F owns imports and updates. Coordinate E's interface at review.

## Research points

Inspect preparePlayback's cancellation/commit lifecycle before choosing the exact
successful hook; inspecting a candidate is not proof that it became current. Confirm
playlist current and locator are captured rather than read after an async hash. Keep
upload, remote streaming, Media Session and existing playback invariants unchanged.

## Verification

Run playlist model/store/transfer/control/playback and app Rust source-contract tests
(`cargo test -p starplayer-web`) plus applicable Node tests. Cover status text without
skips on explicit play/advance, complete-but-missing row resolving, old gates, rebind
only on completion, membership arrivals, import metadata provenance, legacy updates,
normal persistence, stale activation/hash, same-locator different-library isolation,
metadata before discovery, and updateDisplay preserving current/order. After E lands,
run all Node tests and `cargo xtask wasm` before handing integrated app to G.

## Out of scope

Source/worker/control/view production files owned by A–E; engine/audio rendering;
shared headless harness; unrelated playlist/UI features. After B lands, only the
web Rust #[cfg(test)] app/HTML source-contract tests transfer to F; preserve production
bindings and assertion intent. Incorporate E-provided HTML contract updates serially.

## Verification evidence (2026-10-26)

Worker passed combined E/F full web Node suite 200/200, web Rust tests 15/15 plus
docs, `cargo xtask wasm` and `git diff --check` after updating to integrated E.
Coordinator independently passed playlist/playback/app-metadata tests 51/51 and
reviewed all production, Rust source-contract and store-helper changes. Existing
assertions remain intact. Generated view/discovery/worker modules are packaged.

Playback captures an originating locator and optional `captureContext` result
before resolution; activation snapshots structured metadata before any await and
publishes it only after commit/context/identity guards. Enrichment is optional and
borrows retained playback bytes. Partial before-row patches update all matching
legacy/unknown entries without no-op saves. Informational discovery statuses never
gate matching-folder playback; rebind waits for complete discovery. G passed the extended real IndexedDB store helper and the
explicit/advance/reconnect scenarios in all three browser modes.

Final integrated automated acceptance passed in G, including all three browser
modes and full 11-job repository CI. Remaining manual owner acceptance is recorded
in the [master](W15-master-plan.md). This task is archived with the completed
integration deliverable; `main` was not merged or deployed.
