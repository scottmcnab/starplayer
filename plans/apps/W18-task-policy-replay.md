# W18-R — Desired page policies and audio-applied transport replay

| Field | Value |
|---|---|
| Status | Implemented and verified on feature branch; intentionally unmerged |
| Baseline | W18 feature branch specification commit 36ac458 |
| Least-capable safe worker | GPT-6.1-sol (high) |
| Coordinator | W18 master; worker leaves changes uncommitted for review |

## Context for a fresh agent

Read AGENTS.md, W18 master and W18-I (Instruments policies). This delegates the
page desired/applied policy and transport ordering portion of W18-I to a separate
worker. Entire final branch gets Astra medium review and MUST remain unmerged.
Owner wants Enable note overlap and new Unique notes across channels to retain values
for the page session, even through new modules/reloads/rebuilds, resetting only on page
reload. Interactive wire2 is not publicly deployed and is extended in place.

Instruments worker /root/w18_instruments owns engine uniqueness, core action/opcode26,
host policy shadows and codec uniqueness field, selected-note opcode25 and UI/metadata.
Coordinate names and APIs directly. Your scope app.js policy initialization/setters/
acks/load/rebuild hostSettings; worklet-processor replay/ordering; host replacement
transport deferral APIs; corresponding focused tests. Shared app.js, host player.rs/
replacement.rs, README/help need integration; avoid selected-note and engine ownership
code. Playlist worker is separate. Parent owns foundation docs/index.

## Deliverables

- Desired page preferences for overlap and uniqueness start false, use no browser
  storage, retain across new modules, same-byte reloads, playlist advances, enhancement,
  renderer and output rebuilds. Per-instrument Ring still resets new session and retains
  same-module rebuild. Reference-note lifetime implemented by Instruments worker.
- Distinguish page intent revision from identity-scoped acknowledged applied policy.
  Older acknowledgements (even current identity) update applied state without replacing
  newer intent. Latest rejected DIRECT edit rolls back to last applied value. Define
  request/replay bookkeeping to distinguish edits and installation reconciliation.
- Failed posted/inspection loads retain desires, prove retained-module identity under
  existing rules and reconcile settings before enabling interaction/Play. Stale messages
  never change new-module accepted state or erase page intent.
- Propagate latest desired settings through hostSettings and policy revision capture.
  Edits during candidate or current-node replay must reconcile before readiness and
  transport; do not seed once and allow obsolete settings to play. Keep disabled control
  semantics truthful. Extend persistent revision invalidation/retry logic.
- Worklet currently stageLoadTransport immediately on loadTransport and before replay
  in loadModule (including deferred). Gate pending Play/Resume until latest desired
  policies ACTUALLY audio-apply. newest Stop/Pause wins and cancellation holds.
- Host load_replacement currently queues RestoreTransport saved Playing before JS replay.
  Add a browser-specific deferred-restoration path/API as needed: preserve exact saved
  cursor/transport ramp, hold song sequencing until replay complete, then restore saved
  state without destructive Stop/Play emulation. Existing reusable native API behavior
  must stay correct. Coordinate replacement codec new uniqueness field with I worker.
- Force replay spans >one quantum (large mutes/inserts). Readiness notification alone
  is insufficient; first Playing samples and resumed sequencing require latest applied
  policy. Retain queue reserves, identity/barrier rejection proofs and no RT allocations,
  locks or panics. No mutable WASM view access/allocation inside process().
- Coordinate new global control ID/constant with I worker (it creates checkbox HTML and
  opcode26/ring export). Update policy help/README only as agreed; parent can resolve.
- Add meaningful race and actual execution-order regressions, migrate old new-module
  overlap-reset assertions to page lifetime without weakening Ring reset checks.

## Research points

app.js activateModuleOnNode/activateLoadedModule/onWorkletMessage and candidate rebuild
capture paths; queueCommand settings revisions; worklet startReplay/continueReplay,
loadModule/loadTransport/deferredLoads/pendingAcks; host Player replacement and
RestoreTransport audio action. Current settings state is accepted state, insufficient
for racing page desired changes. Existing load proof carefully compares old installed
control/audio identity after preparation failure; retain it. Existing bridge groups16
fit 32-command quantum admission including settlement reserves. Existing host/native
replacement tests pin cursor/ramp; extend for browser deferred path.

## Verification

- git diff --check; node --check; Node suite serialized, meaningful VM/worklet mocks
  asserting command EXECUTION sequence (not merely final state).
- Page rapid toggles, older accepted same-identity reply, latest direct rejection,
  stale reply, new/same-byte load, failed retained load, candidate revisions/rebuild,
  playlist advance and page-only lifetime. Instrument selection reset separate.
- Multi-quantum mute/insert replay: saved Playing, pending Play/Resume wait for policies,
  latest Stop/Pause stays, exact saved cursor/ramp restored. Host tests deferred/native
  replacement correctness and admission pressure; rejected replay cannot restore Playing.
- Serialize Cargo/browser with parent. Initial Cargo slot belongs I worker; request
  slot before Rust/build/browser checks. CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
  RUST_TEST_THREADS=2; target symlink shares /tmp/starplayer-no-target. Node concurrency1.
- Leave edits UNCOMMITTED with evidence; parent reviews/checks before commit.

## Out of scope

Engine duplicate predicate, selected-note audition, playlist, foundation docs, v3,
persisting page preferences, main merge/push/deploy, cargo fmt, broad staging,
attribution, historical edits, user scratch files or W6 checkout.

## Completion evidence

Implementation, independent verification and all resolved Astra findings are recorded
in [W18 review and verification](W18-review-and-verification.md). The owner requested
that the completed feature remain unmerged; no main landing, push or deployment occurred.
