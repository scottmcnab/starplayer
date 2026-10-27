# W15 remediation R3 — Atomic source acceptance with independent pending selections

| Field | Value |
|---|---|
| Status | **Landed on `main` via `ece531f` (2026-10-27) — reviewed/verified; manual feature acceptance pending** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | Baseline; parallel R1/R5 |
| Ownership | library-controls.js, selection-status index.html markup, controls unit tests |
| Historical branch / sibling worktree | `w15-remediation-r3` / `../starplayer-w15-remediation-r3` |
| Authorization now | Implementation authorized; worker must not stage, commit or merge |

## Context for a fresh agent

Read AGENTS.md, [remediation master](W15-remediation-master-plan.md), and the
[archived W15 master](W15-master-plan.md) with its linked implementation
evidence. The remediation contracts explicitly supersede only listed archived clauses.
Feature baseline is b3faf9e; execution starts from the integrated planning tip.
Task status and verification are updated by the coordinator.
Do not rely on chat history or temporary review scripts. JS paths are under
`apps/starplayer-web/www/`; tests under `apps/starplayer-web/test/`. During execution,
worker reports diff/results to coordinator, does not commit or edit another task's
files. Preserve engine/RT/no_std invariants.

Finding #3 is more than early disposeCurrent: accepted callbacks compare a global
revision incremented by pending selection. Reproductions show rows mutate with zero
UI notifications after only deleting early disposal. Browse identity failure also
must retain the current usable context.

## Deliverables

- Implement master C: separate selectionEpoch, acceptedGeneration and per-context
  comments revision. Pending source/index/identity/chooser operations retain current
  context, controller, callbacks, resolver cache and accepted source controls.
- Prepare candidate completely; commit synchronously with token guard, one disposal,
  one replacement callback, then start discovery/sidecars. On malformed/slow index,
  rejected identity, cancelled picker, remote attach failure or overtaken attempt,
  keep prior library usable and clean only candidate resources.
- Give every chooser/read/Browse operation its pending token. Manual comments during
  a chooser attach to old accepted context; auto-comments continue until real acceptance.
  A late manual read for old context cannot attach to the accepted replacement.
- Add library-selection-status for candidate messages/errors, leaving library-summary
  to accepted progress. Reconnect last-handle persistence occurs only after acceptance.
  Keep pending manual index precedence, stream/reopen and Browse escape routes intact.
- Hand index.html ownership to R6 when reviewed. No view changes required here; accepted
  onLibraryUpdate must remain observable while selection stays pending.

## Research points

Read every revision check in controls, including deferred comments and restored remote
sidecars; do not globally substitute one counter. Keep picker user activation intact.
Construction failure before commit cannot dispose old context; callback guards install
on prepared candidate using the accepted token assigned at commit.

## Verification

Run `node --test --test-isolation=none apps/starplayer-web/test/library-controls.test.mjs`.
Add an actively emitting old discovery controller and assert BOTH changed row data
and delivered onLibraryUpdate during: multi-index chooser, delayed/invalid index,
Browse identity rejection, denied/cancelled picker, failed remote attach and newer
selection overtaking older read. Assert old Stop/Resume/enrichment/comments work,
no premature resolver.clear/dispose, last-handle unchanged until acceptance, exactly
one old disposal and no stale callbacks afterward. Manual comment attachment races
with source acceptance and deferred auto-sidecars must be explicit tests. R7 repeats
key cases with real OPFS/worker/browser. Run web Rust source-contract tests if markup
assertions are touched; arrange narrow R4a handoff rather than weaken them.

## Out of scope

Identity policy change, registry pruning, source read implementation, runtime/model
edits, other HTML/view redesign.

## Execution evidence (2026-10-27)

Integrated as `4368b49` after coordinator diff review and independent focused Node verification.
23 controls tests passed, including chooser alternatives, pending-root Browse, active callbacks and comments races; two Rust markup checks passed in the worker checkout.
`git diff --check` passed. Browser verification remains R7.

Final combined Node/Rust/WASM/CI, Chromium and Pages gates passed; see
[R7 final evidence](W15-remediation-R7-integration-evidence.md#final-execution-evidence-2026-10-27).
Native-picker, real-collection and device acceptance remain owner checks.
