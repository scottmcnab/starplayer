# W15-G — End-to-end verification, packaging and documentation

| Field | Value |
|---|---|
| Status | Specified — not started |
| Recommended model | gpt-6.1-sol |
| Recommended effort | **high** |
| Depends on | E and F integrated |
| Branch / sibling worktree | `w15-g` / `../starplayer-w15-g` |
| Ownership | headless harness, packaging fixes, library user docs, verification records |
| Coordinator | gpt-6.1-sol, xhigh; [master](W15-master-plan.md) |

## Context for a fresh agent

Read `AGENTS.md`, [W15 master](W15-master-plan.md), and the complete
[behavioral specification](W15-task-folder-library-discovery.md). The master defines
shared APIs; the behavioral specification defines edge cases and compatibility. Work
from this task file without relying on conversation history. Do not commit or start
another task. Report changes, verification results and handoff contracts to coordinator.
All paths below are repository-relative; web JS paths are under
`apps/starplayer-web/www/` unless stated otherwise.

All production components should now be integrated. This task proves the combined
feature with real workers and browser handles, not only producer mocks. Earlier
workers retain responsibility for fixes in their owned production files; report
failures to coordinator and receive explicit ownership before changing those files.

## Deliverables

- Extend headless tests using OPFS directory handles returned by a stubbed picker;
  exercise real IndexedDB/isSameEntry and page reload, A→B→A identity and CSV gain/bypass.
  Native picker UI/permission dialogs remain manual acceptance, never fake evidence.
- Add real worker ZIP/nested ZIP/metadata tests using synthetic and approved fixtures.
  Test playback and playlist building before scan completes, saved missing-yet rows,
  completion/rebind, source replacement/cancel, pause/resume, worker retry, sidecar
  before/after hash, and metadata inspection without disrupting playing audio.
- Implement all named E DOM scenarios from its Verification section in real Chromium;
  pure helper tests alone do not establish focus/scroll correctness.
- Add a 40,000-row fake local enumeration test and delayed archive test. Observe first
  usable rows before completion, interleaved input/heartbeat progress, bounded visible
  DOM, retained scroll/focus and zero module reads for loose discovery/selection/add.
  Avoid brittle absolute wall-time thresholds; coordinate progress gates to prove UI
  responds before releasing delayed jobs. Exercise directory and FileList paths.
- Verify worker JS/WASM assets load under dev serving and Pages subpath with current
  COOP/COEP shim. Fix packaging in xtask only if necessary; never deploy. Keep source
  and emitted/generated artifacts separate, staging only intended source files.
- Update LIBRARY-CSV.md (E owns in-page help; route final text corrections to E) for CSV fallback/bypass, partial filename and
  metadata results, on-demand reads, archive limits, retry controls, reconnect/rebind,
  FileList initial wait, and song messages available only via sidecars. Supply exact
  automated evidence and remaining owner acceptance to the coordinator for the master; leave task status
  honest until its deliverable is reviewed/landed under coordinator control.

## Research points

Inspect harness setup/cleanup and worker network paths under non-root Pages URLs.
Provide deterministic fixture cleanup, no network requests to owner libraries, and no
new production test-only global unless the existing injection route truly cannot
support OPFS picker substitution. Track discovered vs indexed regression coverage.

## Verification

Run `node --test apps/starplayer-web/test/*.test.mjs`,
`cargo test -p starplayer-archive`, `cargo test -p starplayer-web`, `cargo xtask wasm`,
`STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`,
`cargo xtask wasm --pages` with a local Pages-path worker smoke check, and
`cargo xtask ci`. No cargo fmt. Record commands, results and material limits. If an
integration fix changes behavior rerun affected tests, then the final relevant gate.
Coordinator reviews results and final diff; owner separately tests native permission
prompts, a real large folder and loose/nested playlist reconnect after reload.

## Out of scope

Deployment, main-branch merge without authorization, actual owner collection scans,
new product scope, marking unperformed manual tests passed, implementation changes
outside assigned ownership without coordinator handoff.
