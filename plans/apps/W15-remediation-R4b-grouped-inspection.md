# W15 remediation R4b — Bounded grouped archive metadata pipeline

| Field | Value |
|---|---|
| Status | **Integrated — reviewed; focused and final combined automated gates passed** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | R1, R2, R3 (the #1–#3 gate) and R4a |
| Ownership | runtime/jobs and discovery controller after R1/R2 handoff; their unit tests |
| Branch / sibling worktree | `w15-remediation-r4b` / `../starplayer-w15-remediation-r4b` |
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

Finding #4: every `inspect` job re-reads and transfers the whole outer ZIP for one
member. Sources of 32 MiB or more also recycle the worker after every member, and
nested members re-extract their inner ZIP each time. This task owns the second
serialized edits to runtime/jobs/controller. Preserve R1's fatal trap behavior and R2's
version guards and counters.

**Amendment (2026-10-26):** playback hashing now runs on the page (master A), so the
worker never interleaves a foreground job with an open group. The first draft's
retained-group and foreground-hash states, per-member ACK protocol, recovery purpose and
preemption, hash fairness and fatal consumer-callback route are withdrawn. Master D,
"Runtime group jobs", is the contract.

## Deliverables

- Replace the per-locator `inspect` job kind with master D's `inspect-group`: at most 64
  members, grouped by sourcePath, then containerMember, in deterministic locator order.
  A loose module is a one-member group. Delete the unused `hash` job kind and its tests.
- Runtime: keep one executing job, with catalog priority over groups. Read the source
  once per group, check size limits and transfer it. Deliver `onMember` at most once per
  token from streamed `member-result` frames, with instance/id/generation/attempt
  checks and duplicate suppression. Settle with `completedTokens`, tracked by the
  runtime, marking delivered before callback. Add master onIssue callback to enqueue
  options; onMember throws are logged and reported through it, nonfatally. Controller
  owns accepted-generation-guarded path issue insertion; runtime never owns the library.
  A reporting-hook throw is logged once, not a fatal/retry route.
- Worker: open one `ArchiveInspectionSession` (R4a) per archive group. For each member,
  extract, `inspect_s3m`, `module_metadata`, hash the extracted bytes and post the
  result. Then free the session and post `group-result`. Keep one source snapshot, one
  inner cache and the current module buffer only.
- Controller: form groups only from eligible outstanding work (R2's scanActionable plus
  explicit retries), capturing each member's version. Guard every result against the
  current version and context, so a newer playback always wins over an older group
  result. Playback of one locator invalidates/ignores only that member's old version;
  remove an unsubmitted descriptor if possible, but NEVER cancel a submitted shared
  group for one locator. B/C must still settle when A is played. Frozen groups may
  process an invalidated member; mark delivered and clean its old descriptor without
  applying or touching newer work. New arrivals join later groups. Map group failures as master D states:
  read-failed or source-too-large become errors on each undelivered eligible locator,
  and worker-failed keeps only undelivered descriptors for explicit Retry.
- Controller's onMember handler cleans matching group/token/version descriptors in
  finally. Terminal completedTokens reconciles cleanup idempotently for success/error/
  stale/callback-throw deliveries; no pending map or promise hangs. A throw before
  mutation leaves an actionable row and issue; partial application stays as applied.
  Neither case marks media corrupt or auto-requeues that token in the same scan.
  Subsequent members continue; later explicit Scan may request a fresh token.
- Cancellation: `cancelMetadata` and `group.cancel()` terminate an executing group's
  worker and reject `aborted` with progress. Do not requeue user-cancelled scans. Stop
  discovery pauses catalogs only. Source replacement and dispose reject `disposed`,
  and late frames are inert. None of these worker operations clear or suppress page
  digest promises/counters; their lifetime is R2's accepted-context/version contract.
- Recycle after a group or catalog per master D, never mid-group. Add test-only
  counters for reads, bytes, inner extractions, sessions, instances and recycles.
  Counters must not retain buffers or alter ordering.

## Research points

Inspect the controller's epoch and the inspected-map retry paths before replacing
jobs with groups. Check that cancellation and worker failure settle every pending
group promise exactly once. R4a extract_member returns Uint8Array on success and exposes ordinary Rust Result
errors as JS throws. Catch those per member and continue; RuntimeError uses R1's fatal
classification, never the ordinary member-error path.

## Verification

Run worker/runtime/discovery focused Node tests with `--test-isolation=none`, then
`cargo test -p starplayer-archive -p starplayer-web` and `cargo xtask wasm`.
Assert that 3, 64 and 130 uninterrupted members need 1, 1 and 3 outer reads, counted
separately from catalog and playback reads. Test both small and ≥32 MiB outer ZIPs,
and assert one inner decode per same-inner group and at most one recycle per group for
large inputs. Test direct plus one nested container, mixed member errors, mutation
between groups, newer playback during a group, cancel mid-member and between groups,
Stop discovery during a group, source replacement, and duplicate or stale frames.
Test group A/B/C with playback of A mid-scan: A's stale result cannot replace page
metadata, group is not cancelled, B/C settle once and all old descriptors clean up.
Test callback throw before mutation and after partial application: delivered token
included in terminal progress, controller-owned issue, no hanging maps, no corrupt
metadata flag, later members delivered; reporting-hook throw also stays nonfatal.
Test a trap after partial success followed by explicit Retry (undelivered only), a
page digest that spans that failure AND Retry and still publishes, normal worker
recycle/Cancel metadata independent of page digests, and dispose with frames in flight. R7 repeats the core cases in the real browser.

## Out of scope

New source selection semantics, changing R1/R2 guards, shared headless edits,
unbounded or cross-group archive caches, changed ZIP duplicate interpretation.

## Execution evidence (2026-10-27)

Integrated as `d23bb02`. Coordinator independently passed 99 focused worker/controller/group tests after full diff review. Worker full Node 262 passed; archive31/web22 tests and WASM packaging passed. Actual generated-WASM smoke passed all 12 ordinary/nested x small/large x 3/64/130 matrices with 1/1/3 reads. Inner-decode adapter counts are modeled; native Rust tests establish cache reuse.
`git diff --check` passed. Logs: `/tmp/starplayer-w15-remediation-evidence/`.

Final combined Node/Rust/WASM/CI, Chromium and Pages gates passed; see
[R7 final evidence](W15-remediation-R7-integration-evidence.md#final-execution-evidence-2026-10-27).
Native-picker, real-collection and device acceptance remain owner checks.

## Post-integration review follow-up (2026-10-27)

[R8](W15-remediation-R8-trap-cleanup.md) subsequently prevented poisoned-session cleanup from masking fatal traps; the post-integration reviewer closed the finding and all final gates passed.
