# W15 remediation — Correctness, bounded scanning and responsive libraries

| Field | Value |
|---|---|
| Status | **Implemented, independently reviewed and verified on `w15-remediation` — manual owner acceptance and future main landing pending (2026-10-27)** |
| Baseline | `48eee06`, `library-folder-plan` (feature baseline `b3faf9e`) |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Workers | GPT-6.1-sol, **high** effort |
| Historical delivery | [W15 master and evidence](complete/W15-master-plan.md), [behavioral specification](complete/W15-task-folder-library-discovery.md) |
| Integration branch / worktree | `w15-remediation` / `../starplayer-w15-remediation` |
| Authorization now | Implementation, delegated workers and integration commits authorized; no main merge, push or deployment |

## Context for a fresh agent

Read AGENTS.md, this master and the assigned task. The original W15 implementation
and its successful automated evidence remain historical facts; independent review
subsequently reproduced gaps not established by those tests. This active plan is the
source of truth for remediation. Implementation is authorized; verification results are recorded as tasks finish.
Original task branches/worktrees were deleted; the original integration checkout
`/home/scott/projects/starplayer-w15-folder-library` on `w15-folder-library` remains.
Do not reuse/delete that checkout as part of remediation.

Review input was `/tmp/starplayer-w15-review/planning-request.md`, including twelve
dispositions and independent reproductions. Temporary reproduction programs were
`reproduce.mjs` and `runtime-checks.mjs` in that directory. They assert the *bad*
behavior, so their success is not feature acceptance. They are reference material,
not a permanent test dependency: workers must recreate fixtures/regressions in-repo.

No rendering, DSP, RT allocation, no_std or sample-exact timing changes. Retain CSV,
remote, upload, locator, identity and playlist compatibility except explicit clauses
below. No source-tree writes into users' library folders, catalog persistence or
new archive formats. Do not run cargo fmt or touch historical assembly.

## Twelve-item disposition and landing gates

| # | Disposition | Required outcome / owner |
|---|---|---|
| 1 | Correctness blocker | Hash playback bytes on the page with WebCrypto; publish metadata+SHA together per current version; no worker hash queue to cancel; R2 |
| 2 | Correctness blocker | Eligible metadata and sidecar counts, useful scan/retry actions; R2/R6 |
| 3 | Correctness blocker | Pending selection cannot dispose or silence accepted context; R3 |
| 4 | Bounded performance blocker | One source read per bounded archive group; nested reuse; cancellable groups; R4a/R4b |
| 5 | Scheduling improvement + browser gate | Non-timer cooperative yields; measure hidden-page behavior without claiming frozen-page progress; R5/R7 |
| 6 | Bounded view-performance blocker | Incremental row/tree/filter/order work, bounded issue rendering, evidence beyond throttle changes; R2/R6/R7 |
| 7 | Intentional limitation / documentation | Directory Size/Date remain unknown; explain difference from FileList/CSV, no extra stat I/O; R7 |
| 8 | Intentional conservative identity behavior / documented deferral | Preserve uncertain-handle session fallback, known later matches and no eviction; explain recovery limits; R7 |
| 9 | Intentional compatibility behavior / regression | First raw duplicate wins for catalog AND legacy upload/extraction; retain compatibility tests and note; R4a/R7 |
| 10 | Fatal-instance hardening blocker | Actual WASM RuntimeError retires worker, suspends queue, explicit retry; ordinary data errors remain isolated; R1/R4b |
| 11 | Intentional incomplete-scan behavior | Preserve disabled Rebind pending and complete matching-set gating; optional explanatory help only, no inferred zero matches; R7 |
| 12 | Process / landing bookkeeping | Keep not-merged status truthful; future authorized multi-commit milestone landing uses --no-ff; coordinator/R7 |

Items 1–3 must pass before R4b (archive grouping integration) starts. All blocker tasks
and automated R7 gates are required before this remediation is considered ready for a
future landing. Native-picker, actual large collection and real-device performance
checks remain explicitly named owner acceptance; do not silently replace failed
browser gates with those manual items. No merge to main, push or deployment is authorized.
Registry deletion/reconciliation UI (#8) and cosmetic Rebind redesign (#11) are deferred:
uncertain equality is not evidence that two identities differ, and incomplete discovery
cannot prove no future match. Documentation of both behaviors is in scope, not optional.

## Deliverables

| Task | Dependencies | Exclusive production ownership |
|---|---|---|
| [R1](W15-remediation-R1-worker-hash-and-traps.md) | Baseline | worker runtime/jobs JS: fatal WASM trap classification only |
| [R2](W15-remediation-R2-metadata-state.md) | Baseline | discovery controller (incl. page-side playback digest), library-index, shared row-delta/counter contract |
| [R3](W15-remediation-R3-source-acceptance.md) | Baseline | library-controls and selection-status markup |
| [R5](W15-remediation-R5-cooperative-scheduling.md) | Baseline | library-source and new cooperative-yield helper |
| [R4a](W15-remediation-R4a-archive-session.md) | Baseline | web Rust session bindings; narrow owned-buffer ZIP normalization helper and archive compatibility tests |
| [R6](W15-remediation-R6-incremental-view.md) | R2/R3/R5 | library-view, view-state helper and HTML/CSS after R3 handoff |
| [R4b](W15-remediation-R4b-grouped-inspection.md) | R1/R2/R3/R4a | runtime/jobs + discovery controller after R1/R2 handoff |
| [R5a](W15-remediation-R5a-background-priority.md) | R5 (measured integration correction) | cooperative-yield.js and its unit tests after R5 handoff |
| [R6a](W15-remediation-R6a-keyboard-selection.md) | R6 (retained browser regression) | keyboard selection recount before offscreen repaint in library-view.js |
| [R7](W15-remediation-R7-integration-evidence.md) | R4b/R5/R5a/R6/R6a (transitively all) | headless/W15 browser harness, documentation, final evidence |
| [R8](W15-remediation-R8-trap-cleanup.md) | R4b/R7 (post-integration review) | worker jobs and endpoint/group/browser trap-cleanup regressions |
| [R9](W15-remediation-R9-playback-scan-status.md) | R2/R6/R7/R8 (post-integration review) | controller scan-phase logic and focused/browser view regressions |

R1, R2, R3, R4a and R5 all start from the baseline and touch disjoint files; run up to
three concurrently (R1 and R4a are small). R6 follows R2/R3/R5 and consumes R5's
integrated yield helper. R4b follows R1/R2/R3/R4a (the #1–#3 gate) and owns
runtime/controller files only after R1/R2 finish; R4b and R6 may overlap. R7 is last in the initial implementation graph; post-integration review corrections R8 then R9 follow its reviewed baseline. Coordinator alone owns active
plan status, index and archive annotations. Workers own their module unit tests;
R7 owns shared headless files. R3 hands index.html to R6, then R7 may edit only help
text after explicit final handoff. R4a owns web Rust source-contract test maintenance;
if a later UI change requires updates, coordinator serializes that narrow handoff.

Create `w15-remediation` from the reviewed planning tip, then
`w15-remediation-r1`, `-r2`, `-r3`, `-r4a`, `-r4b`, `-r5`, `-r5a`, `-r6`, `-r6a`, `-r7` with matching
sibling names `../starplayer-w15-remediation-r1`, etc., from integrated prerequisites;
review producer APIs/tests before consumers start. Workers
report diffs, verification and limitations; coordinator stages explicit paths only and
follows AGENTS commit/merge rules. Delegate bounded implementation tasks in dependency order.

## Contracts superseding archived W15 clauses

This section explicitly replaces the archived master's **Worker runtime**, **Context/
runtime**, **Notifications** and associated D1/D2 clauses only where stated. It also
replaces archived "supersede old hash", "recycle every large job" and UI full-refresh
strategies. All unrelated behavioral requirements and original evidence remain intact.

### A. Page-side playback hashing and fatal WASM isolation (#1, #10)

**Playback hashing (amended 2026-10-26).** The archived design sent the playback
digest to the library worker as a `hash` job, and a newer hash cancelled the older one.
That cancellation is the root cause of #1. The digest itself was already
`crypto.subtle.digest`, which is asynchronous and computed off the UI thread by the
browser; routing it through the worker added a queue, a retained byte copy and the
cancellation, not parallelism. Replace it:

- At `acceptPlaybackMetadata(locator, metadata, bytes)` entry, copy the locator and
  metadata's scalar/string fields, capture accepted context/generation, and increment
  that locator's version. Call injected `digest(bytes)` synchronously before any await.
  Its contract is `BufferSource -> Promise<ArrayBuffer>` resolving to exactly 32 SHA-256
  bytes, with WebCrypto-compatible input snapshot semantics: copy the passed view's
  byte range at invocation, not at eventual resolution. Default is
  `bytes => crypto.subtle.digest('SHA-256', bytes)`. The controller neither detaches
  nor retains/copies the caller's input after invocation; native WebCrypto does copy
  it. See [WebCrypto digest steps](https://w3c.github.io/webcrypto/#SubtleCrypto-method-digest).
  Injected delayed test digests must implement the same snapshot semantics.
- Normalize the returned 32-byte ArrayBuffer to 64 lowercase hex digits (two zero-padded
  digits per byte). Publish ONE metadata+SHA patch only if the accepted context/generation
  and locator version still match. Snapshot metadata before waiting as well as bytes:
  mutating the caller's metadata object cannot change a pending digest's eventual patch.
  The app already captures module_metadata before its activation awaits.
- Page digest validity does NOT depend on runtimeEpoch. Worker traps, explicit runtime
  Retry/recreation/recycling and Cancel metadata neither clear the digest registry nor
  invalidate its results. Remove the current worker-hash cleanup dependencies from
  failRuntime/retry/cancel paths. Accepted-source replacement, context disposal and a
  newer playback version suppress publication; they cannot cancel the native promise.
- Do not publish interim metadata or `sha256:''`. Until the digest settles, retain the
  previous coherent metadata/SHA/sidecar snapshot if one exists, even if it describes
  earlier played bytes. Label it as awaiting refresh; do not present that old digest
  as verification of the newer bytes. A first-play row with no prior digest remains
  unknown with empty SHA until success. Before-row playback caches only the completed
  coherent patch. Each locator's digest completes independently of other locators.
- Synchronous injected-digest throws, async rejection, and invalid result shape use the
  SAME failure path: for a still-current version publish the captured metadata with
  `sha256:''` and add a joined locator-path issue. This deliberately detaches any older
  sidecar digest on failure. A later playback or explicit scan can supply SHA; no automatic
  retry. Stale/disposed successes OR failures only clean bookkeeping, not rows/issues.
- Track every physically outstanding returned promise in a separate record, including
  superseded same-locator versions. Retain only scalar byteLength, copied locator/metadata
  and version/generation, never bytes in completion closures. Settle/finally removes
  exactly that record even when its result is ignored. A synchronous throw creates no
  outstanding promise. Do not zero pending counters on worker Retry or on supersession;
  disposal stops publication but records clean up as their native operations settle.
- This policy deliberately has no native-digest concurrency cap. Activation rate is
  NOT a proved bound and hashing is not guaranteed faster than activation. Every pending
  WebCrypto operation may own its input snapshot, so controller "no copy" does not mean
  zero browser allocation or bounded aggregate bytes. Loose source files may be up to
  128 MiB; 64 MiB is the archive-extracted-entry limit, not a universal module limit.
  R7 measures outstanding counts, pending input-byte estimates and copy/start costs;
  results are evidence for a follow-up decision, not grounds to pre-build a queue,
  cap, recovery FIFO or worker-hash state. No speed/memory result is claimed here.
- Before grouping, an independent same-locator scan job may be cancelled on playback.
  After R4b, increment/invalidate only that member's version: ignore its older result
  and, if not yet submitted in a frozen group, remove only its queued descriptor.
  Never call shared group.cancel() merely because one member is played. Unrelated
  members must still settle exactly once. A submitted group's immutable member list
  may still process the stale member; its result is delivered/cleaned but not applied.
  Explicit Cancel metadata, source disposal and other whole-group operations retain
  their documented cancellation behavior. Scan hashing stays inside the worker.

The worker `hash` job kind becomes unused. R1 must not extend it; R4b deletes it, with
its tests, when it rewrites the runtime. The "one active plus one pending hash",
`hash-deferred`, automatic recovery queue, eight-hash fairness rule and `onFatal`
hash-snapshot hook from the first draft are withdrawn; nothing remains for them to protect.

**Fatal WASM traps.** Recognize `WebAssembly.RuntimeError` at the actual worker endpoint
(`startLibraryWorker`) before flattening, and send the existing tagged error with
`code:'worker-failed'` and optional `cause:'wasm-trap'`. The runtime must honor that
fatal code: terminate the instance immediately, settle the active request once, suspend
queued requests and expose runtimeError. Do not mark that member or unrelated rows
metadata-error. Late messages from retired instances are ignored by instance/attempt/
job/generation. Ordinary loader Result errors, unsupported data and JSON validation
errors remain job-failed and the queue continues on the same instance. Existing
startup/transport fatal behavior and explicit Retry are preserved. No automatic
crash/retry loop. Abort, dispose and stale errors never become corrupt-track errors. After a trap,
do not call back into WASM to free an archive session: wasm-bindgen may retain its
mutable borrow, and a secondary ownership error would mask the original RuntimeError.
Worker termination releases the poisoned instance. Healthy success/ordinary-error
paths still free normally; a trap during otherwise-normal cleanup remains fatal.

### B. Eligible work, sidecars and change notifications (#2, #6)

For discovered rows, `eligible = row.playable`; known encrypted/oversized/unsupported
archive rows are excluded. Metadata errors remain playable and count as errors, but
Scan never retries them. Counts `ready/unknown/error` describe eligible rows only;
add `excluded` and `scanActionable` (unknown or ready-without-SHA, excluding errors
and already pending inspection or playback digest). `metadataIncomplete` means eligible
unknown/error/missing-SHA work remains, including pending work; excluded-only library
is complete for metadata unless a current eligible playback refresh is pending.
`hashesPending` counts ALL physically outstanding playback promises, including obsolete
same-locator versions and before-row locators. Add `refreshPending` for current-version
pending refreshes of eligible rows (or before-row locators provisionally eligible),
counted once per locator. `metadataIncomplete` is also true while refreshPending>0,
even when an older ready+SHA snapshot is retained. A stale-only pending promise still
counts in hashesPending but does not make an otherwise verified latest snapshot
incomplete. Show pending refresh separately from missing/uninspected metadata; awaitingHashes
continues to count actual missing SHAs, not retained old snapshots. scanActionable excludes
current-version pending refresh, but not a stale promise alone. Counts transition when
before-row work arrives without double-counting and clean on every promise settlement. Scan enabled iff scanActionable > 0; errors
remain visible with Retry metadata, and do not make Scan appear productive.

Discovered sidecar counters: awaitingHashes includes eligible non-error rows missing
SHA; add failedHashes for eligible error rows without SHA and excludedHashes for
ineligible rows without SHA. Joined/unmatched retain their actual join semantics.
A previously supplied valid SHA may still join even for an excluded row. Do not change
indexed CSV comment semantics. Update prepareComments accounting, its previous-row
snapshot and attach/replacement paths, not only discovery's countRow. UI names pending,
failed and excluded separately; "scan complete" describes job completion, not proof
that all tracks were valid. Empty/all-unplayable/mixed/error-only states are mandatory.
Page-only playback refreshes preserve the idle/cancelled/previous completed scan phase
and do not hide an otherwise actionable Scan button. Pending digests remain visible in
their dedicated counters/hints. Genuine active scans wait for current eligible
refreshes; explicitly requested row Retry reports its own inspection job phase.

Extend existing onLibraryUpdate change with `rowChanges:[{id,added,fields}]`,
`libraryRevision` and `issuesRevision`; retain changedRowIds/metadataUpdates for
playlist compatibility. `fields` is deduplicated changed public row/index domains:
name, folder, format, title, channels, size, timestamp, tracker, playable, comments,
metadataState, error, sha256. Comments means resolved display/search text changed,
not merely a new SHA. For added rows fields may be ['*']. Model mutation helpers
return existing ID arrays and additionally expose `library.lastRowChanges` consumed
immediately by controller; sidecar attachment provides its delta similarly. Controller
coalesces fields per ID until notification. Status-only updates carry empty rowChanges.
issuesRevision increments only when the append-only issues list changes; issues have
stable insertion-index IDs within a context. Full-context replacement resets these.

**Hidden pages (amended 2026-10-26).** No visibility listener or `flushUpdates()` is
required. The 100 ms notification debounce is a single timeout, not a timer chain.
R5a runs native yields at background priority to avoid the observed boosted
continuation starvation of ordinary publication/search timers. Hidden delivery alignment and return latency are
measured by R7 for the actual browser configuration, with no universal throughput promise.
Pending deltas are coalesced, never dropped, and R3's accepted-generation guard remains
the final publication gate. If R7's hidden-page run shows deltas held for more than a
few seconds after the page becomes visible again, the coordinator adds a flush as a
follow-up; do not pre-build it.

### C. Pending selections versus accepted contexts (#3)

Use distinct `selectionEpoch` and `acceptedGeneration`. A pending attempt owns source,
index-read/candidate-choice/identity work and its own error/status; it cannot dispose,
invalidate generation callbacks, clear caches, change comments revision, reconnect
handle or replace accepted context. Opening/cancelling a picker alone never changes
acceptedGeneration. Pending candidate chooser and slow/malformed index leave old
library callbacks, Stop/Resume, playback enrichment and comments fully operational.

Prepare a candidate context off to the side, without starting discovery or attaching
sidecars. A dormant candidate controller may capture acceptedGeneration+1, without
mutating the accepted counter; selectionEpoch guards ensure only the current candidate
can commit that proposed value. Constructor failure is still pre-commit failure. `commitCandidate(attempt,context)` synchronously checks current selectionEpoch,
increments acceptedGeneration, swaps state.library, disposes old discovery/resolver
once, initializes callbacks/summary/control state, then starts the new controller and
auto-comments. No await between final current check and swap. A failed identity/read/
parse/attach/context construction rejects only the candidate and leaves old accepted
context usable. Losing candidate resources are disposed without touching old context.

Accepted callbacks test object identity + acceptedGeneration only, never selectionEpoch.
Comment request revisions belong to accepted context. Manual comments during a pending
choice apply to the accepted library. Pending index selection and Browse supersede only
pending work; Browse commit follows successful identity creation. Candidate choices
capture attempt tokens so late chooser/read/remote-index responses cannot install an
old candidate. Persist last-reconnect handle only at acceptance. Identity-registry
lookup may occur while preparing; it cannot alter the active identity or erase records.
Keep selection errors/status in a new `library-selection-status` element so current
progress cannot overwrite a chooser/error, and candidate messages cannot replace current
library-summary. Existing manual/remote index precedence remains unchanged.

### D. Bounded archive group snapshots (#4)

This supersedes whole-source reads/recycling per member without adding a cache spanning
jobs. Group scan/retry members by sourcePath, then containerMember, deterministic
locator order. At most **64 members per group**; freeze this list before dispatch,
new arrivals form later groups. Runtime queue holds descriptors only. Each group reads
one outer source (128 MiB maximum), transfers it once, retains one Rust-owned source
snapshot and at most one decoded inner ZIP (64 MiB maximum); retain one current module
and one member extraction buffer only. Never retain all extracted modules or inner ZIPs.

R4a adds a WASM `ArchiveInspectionSession` with constructor(bytes),
`extract_member(containerMember,memberPath,rawHex)->Uint8Array`, and ordinary `.free()`.
The session owns outer bytes, resolves with existing exact/raw/encoded matching, caches
only the most recent inner ZIP and replaces it when the container changes. It does not
inspect modules or hash. Existing archive functions remain unchanged. Source read and
session are immutable snapshots for that group: a file changed on disk during the group
is observed on the next read/group/playback, not midway. No mtime-as-content proof.

**Runtime group jobs (amended 2026-10-26).** With playback hashing on the page (A),
the worker never runs a foreground job while a group is open. The first draft's
retained-group/foreground-hash states, per-member ACK protocol, recovery-group
preemption and hash fairness are withdrawn. Keep the existing one-executing-job model:

- API: `enqueue({kind:'inspect-group',generation,sourcePath,members,purpose:'scan'|'retry'},{onMember,onIssue})`
  returns `{id,result,cancel}`. Members are `{token,locator,version}`, at most 64,
  tokens unique within the group. It replaces the per-locator `inspect` kind. A loose
  module is a one-member group whose source bytes are the module itself.
- Priority: catalog, then inspect groups, both FIFO. Paused catalogs behave as today.
- Execution: on dispatch the runtime checks size limits, reads the source once and
  transfers it. The worker opens one `ArchiveInspectionSession` (archives only),
  processes members in order, and posts `member-result` after each member, carrying
  `{instanceId,id,generation,attempt,token}` plus `payload:{metadata,sha256}` or an
  ordinary `error`. It then frees the session and posts `group-result`. There is no
  per-member ACK: the worker does not block the page and each result is small.
- Delivery: the runtime invokes `onMember` at most once per token, as messages arrive.
  It ignores frames whose instance, id, generation or attempt do not match the
  executing group, and duplicate tokens. It maps each token back to its own
  locator/version and does not trust worker-echoed locators.
- Settlement: the result resolves `{generation,kind:'inspect-group',payload:{completedTokens}}`.
  A rejection carries `{name,code,message,cause?,progress:{completedTokens}}`.
  `completedTokens` lists, in member order, every token whose callback ran: successes,
  ordinary per-member errors and results the controller ignored as stale. The runtime
  marks delivery before invoking the callback, tracks this set itself and never takes
  it from the worker. Controller handler removes the matching (group/token/version)
  descriptor in finally even if metadata application throws or ignores a stale result.
  Terminal success/rejection reconciles completedTokens again idempotently, including
  callback-throw tokens, without deleting any newer descriptor for the same locator.
  No delivered token is automatically resubmitted in the same scan pass.
- Callback errors: an exception thrown by `onMember` is a programming error, not a
  media error. Catch it, console.error it and continue after invoking the supplied
  onIssue({generation,id,token,locator,version,path,reason:'internal error applying metadata'}).
  Runtime builds path from its trusted joined locator; the controller owns this hook,
  checks accepted context/generation, and appends/deduplicates the library path issue
  through its existing issue routine. Runtime does not mutate the library. If onIssue
  itself throws, log it once and continue; do not recurse or block cleanup/settlement.
  This is nonfatal and never sets metadataState error. Retain any already applied
  coherent patch; a row not applied can stay actionable with the issue and be rescanned
  on a later explicit Scan. No automatic loop or hanging inspected-map entry.
- Cancellation: `cancelMetadata` and `group.cancel()` cancel queued groups. For the
  executing group they terminate the worker, as active-job cancellation already does,
  and reject `aborted` with current progress. Source replacement and dispose reject
  `disposed`. The queue continues on a fresh instance. User-cancelled scans are not
  requeued. Stop discovery pauses catalogs only; known-row groups keep running.
- Failure: a WASM trap or transport failure rejects the group `worker-failed` with
  progress and suspends the queue (A). The controller keeps only undelivered
  descriptors for explicit Retry; there is no automatic restart. Page digests and
  their counters survive this failure/retry independently of worker epochs. Ordinary per-member
  decode errors are delivered and the group continues. R4a exposes ordinary Rust
  Result errors as wasm-bindgen JS throws from extract_member; catch these around
  each member and emit its ordinary error. RuntimeError remains fatal under A. An unreadable outer source
  rejects `read-failed` without fabricated member callbacks; it and source-too-large
  follow the existing eligible-row error mapping.

Recycle after group completion if outer input >=32 MiB, or accumulated catalog/member
operations >=100; a threshold crossed inside a group waits until the group ends
(at most 64 members), not every member. One source snapshot, one inner cache; no idle
archive cache after group settles. Tests count reads, inner extractions, instances and
recycles separately from discovery catalog and playback reads. For 130 uninterrupted
same-source member inspections expect ceil(130/64)=3 outer reads, not 130, including
32 MiB sources. A same-inner run uses one inner extraction per group.

### E. Scheduling and bounded view work (#5, #6)

R5/R5a export `yieldCooperatively({signal})`: when scheduler.postTask AND
scheduler.yield exist, call `scheduler.postTask(() => scheduler.yield(), {priority:
'background'})` so the native continuation inherits background priority. Otherwise
use a MessageChannel task, finally setTimeout only where neither native path nor
MessageChannel exists. Never use a direct unprioritized native yield from the page task. Check abort before
and after yielding and close per-operation ports/listeners. Enumeration/map construction
checks a slice budget every 128 records or 8 ms, whichever first; yield is a real task,
not a microtask loop or requestAnimationFrame. No requirement for progress in frozen/
discarded pages. Browser may still throttle; UI resumes from existing iterator on return.
Do not fake activity/audio to evade power policy. Keep notification debounce separate
from work scheduling. No visibility listener is required (see B, Hidden pages).
Reference: [Chrome chained timer throttling](https://developer.chrome.com/blog/timer-throttling-in-chrome-88/).
This motivates avoiding timer chains; it is not a throughput guarantee for another API.

R6 uses rowChanges to update only affected caches. Maintain per-row tree contributions,
format/unplayable counts, search inclusion and sort-key snapshots. Append rows update
ancestor counts/insertion once; metadata without folder/playable changes does not rebuild
tree. Reevaluate changed rows against active query; reinsert into sorted results only
when membership or effective sort keys change. Sort changed rows then merge once with
existing sorted order; no full search+sort on every metadata batch. O(N) array movement
for a batch merge is allowed, O(N log N) sorting/comparison of unchanged rows is not.
User-initiated query, sort or format/scope changes and sidecar replacement keep
today's synchronous full search+sort: one pass per user action, plain string
comparison keys, already used at 40k rows. Do not build sliced or cooperative sorting
up front. R7 measures the longest task for a full re-sort at 100k rows. If it exceeds
100 ms, the coordinator decides on slicing as a follow-up task.

Preserve visible anchor/offset, focus and selection; map positions only when order changes.
Update only visible row DOM for display-only metadata. Tree DOM is lazy and updated only
for affected visible nodes; preserve expansion. Issues are paginated 100/page (accessible
Previous/Next plus total), built only when expanded/page/revision changes. No JSON.stringify
of the full issue list on status updates. Progress/labels update only changed scalar values;
no scanning all tree nodes for every status tick. Existing playlist snapshot-add behavior
and indexed-library search semantics remain unchanged. A longer debounce alone does not
meet this contract; incremental batch work must stay O(changed rows) plus at most one
O(N) merge.

## Research points

- Read the existing worker endpoint and controller guards before implementing any new
  failure code; fake transports alone do not establish real endpoint classification.
- Inspect archive ownership and extraction helper bounds; distinguish owned live bytes
  from WASM high-water capacity and avoid copying the outer snapshot per member.
- Inspect all source/index/comments revision closures, not just selectSource disposal.
- Inspect existing browser launch flags before hidden-page measurement, and record
  which cooperative scheduling branch actually ran.
- Preserve pure query/compare semantics when extracting incremental helpers; verify
  row fields that affect comment search, effective sorting and folder counts.

## Independent planning review (2026-10-26)

The requesting coordinator reviewed the complete master and all eight tasks, then
returned contract gaps to the original planning session for revision. The revised
plans define retained-group/foreground-hash state, attempt-tagged messages, fatal
callback classification, active/pending hash recovery and visibility flush ownership.
Final review clarified recovery fairness and cancelled-session cleanup. All twelve
findings have explicit dispositions and the task dependencies/ownership agree.
Local link, task structure, scope checks passed. These are planning
checks: no remediation was implemented or feature-tested. This records the planning review; implementation was subsequently authorized on 2026-10-27.

### Owner-side amendment (2026-10-26, after the review above)

A second review against the baseline code simplified the design. Contracts above
already reflect these amendments, which take precedence over the review summary:

- **#1:** the playback SHA-256 moves from the library worker to page-side
  `crypto.subtle.digest`, which is already asynchronous and off the UI thread. With
  nothing to cancel, the active/pending hash queue, `hash-deferred`, automatic recovery
  FIFO, eight-hash fairness rule and `onFatal` hash-snapshot hook are withdrawn.
  Metadata and SHA publish together in one patch, so successful replay of an already
  hashed row has no interim empty-SHA window; first play and failure are defined in A. R2 owns this; R1 shrinks to trap classification; R4b deletes the `hash` kind.
- **#4:** with no foreground worker hash, R4b keeps the existing one-executing-job model:
  streamed `member-result` messages without per-member ACK, cancellation by worker
  termination as today, and no retained-group, preemption or fairness states. A
  callback exception is caught, logged and recorded as an issue, not a fatal error.
- **#5/#6:** the visibility-return `flushUpdates` listener is withdrawn. Once R5 removes
  timer chains from the walk, the single debounce timeout is not intensively throttled.
  It is a measured follow-up if R7 shows otherwise.
- **#6:** full user-initiated query/sort recomputation stays synchronous; slicing is a
  measured follow-up only if a 100k-row re-sort exceeds 100 ms.
- **Scheduling:** R2 and R4a have no code dependency on other tasks and start at the
  baseline. R4b now waits on R1/R2/R3/R4a; it no longer needs R5's helper.

### Post-amendment clarification review (2026-10-26)

The coordinator requested five narrow clarifications after the owner-side amendments.
These drafts preserve page WebCrypto, atomic publication, a single worker group job,
streaming without ACK, nonfatal callback issues, and measured follow-up thresholds.
Playback invalidates only its shared-group member; page digests use accepted context/
version rather than worker epoch and count stale outstanding promises until settlement.
Native input snapshots are uncapped and measured, with 128 MiB loose and 64 MiB extracted
limits correctly distinguished. onIssue makes library issue ownership explicit and
terminal reconciliation cleans callback-throw descriptors. No-empty-SHA assertions are
limited to seeded, previously hashed successful replays; first-play/failure tests remain.
R3 and unrelated owner amendments are unchanged. Independent final planning review
passed: local links, task structure, twelve-item coverage, consistent acyclic dependencies,
documents-only scope and empty staging area were checked.
These are planning clarifications, not implemented fixes or measured performance results.

## Verification and evidence policy

Each task states focused tests; R7 owns combined browser/performance gates. Retain all
existing assertions unless replacing a test of explicitly superseded behavior (for
example the worker `hash` job and its cancellation); replace it with stronger
versioned guarantees.
No full CI is needed for drafting these documents. Planning validation is local links,
contract/coverage/ownership checks and preservation of owner changes, not feature proof.

Final implementation evidence must include ordinary and nested 3/64/130-member archives
below/above 32 MiB, read/recycle/cache bounds, mutations between groups and playback,
successful replay retaining prior coherent SHAs until atomic replacement (first-play/failure
separately tested), page digest count/copy/latency measurements, real worker trap classification, mixed eligibility/sidecars, pending-source acceptance,
100k-row/10k-issue browser workload and hidden-page scheduler evidence. Record environment,
flags, before/after operation counts, latency distributions and all limits. Existing 40k
stress remains, but does not alone establish these workloads. Owner collection scans
are not authorized by this plan; use synthetic/approved fixtures. No absolute real-device
performance claim from controlled headless timing alone.

## Out of scope and status maintenance

No automatic registry eviction or storage reset, no eager Size/Date stat pass, no changed
ZIP duplicate policy, no partial-discovery rebind, no content-addressed permanent catalog,
no engine/RT changes. Coordinator updates #12 only when a landing actually happens;
retain archived success evidence with a dated link to this follow-up. Implementation and integration commits were authorized by the owner on 2026-10-27.
No merge to main, push or deployment is authorized.

## Execution record (2026-10-27)

Implementation authorized by the owner. Integration: `w15-remediation`,
`/home/scott/projects/starplayer-w15-remediation`. R1 reviewed and integrated as
`1e79716`; coordinator independently passed 42 worker/runtime tests. R2 (`cc68dac`)
and R3 (`4368b49`) are reviewed/integrated with 54 and 23 focused tests. The combined
correctness integration passes all 230 web Node tests. R4a (`b0cf227`) and R5 (`f3fe09a`) are reviewed/integrated after their focused gates.
R4b (`d23bb02`) and R6 (`3b8ebe4`) are reviewed/integrated with 99 and 50 focused
tests. R7 subsequently completed the combined browser/performance/docs gate; final results below supersede these intermediate counts.

### R4a ownership clarification

Coordinator review approved a narrow `crates/starplayer-archive/src/lib.rs` helper
that normalizes duplicate directory records once in an already owned session buffer.
Repeated `find_entry`/`extract` on duplicate-bearing ZIPs otherwise copy the outer
source each time. Legacy APIs and first-entry semantics stay unchanged; R4a owns
this helper and its compatibility tests. The outward session API is unchanged.

### R2 producer handoff

The integrated model exports `createRowPredicate(library, query, options)` accepting
a row and `createRowComparator(library, sort)` comparing row IDs. `search()` reuses
these preserving existing indexed ordering. Typed deltas and pending/eligible counters
follow sections A/B. R4b must replace independent inspection cancellation with
member-only invalidation; R6 consumes these helpers without rewriting the model.

### R4a/R5 producer handoffs

`new Loader.ArchiveInspectionSession(bytes)` / `extract_member(...)` / `.free()`
passed actual generated-binding smoke. Rust `Result` errors throw ordinary strings;
traps remain RuntimeError. Native inner-decode/ownership statistics are cfg(test) only,
not browser exports. R5 exports `yieldCooperatively`, `createCooperativeYield` and
`cooperativeYieldMethod` for consumer injection/diagnostics. R4b and R6 own their
serialized production files; R7 takes shared harness/help ownership after both finish.

### R6 producer handoff

`createLibraryView` accepts optional `onOperation(name, amount)` instrumentation.
`createIncrementalViewState` exposes full/update/order/tree/nodes/includes; its private
tree uses keyed child Maps for affected-path pruning. Public model `buildTree()` is
unchanged. Issue pagination is lazy and bounded to 100 DOM items, with status-only
updates avoiding the producer's lazy full issue snapshot. R7 owns shared headless
files, Pages smoke and help text now; further production fixes need serialized handoff.

## Intermediate integrated verification (2026-10-27)

Coordinator verification on the integrated feature passed:

- `node --test --test-isolation=none apps/starplayer-web/test/*.test.mjs`: 296 passed.
- `cargo test -p starplayer-archive -p starplayer-web`: 31 archive tests passed
  (one pre-existing manual generator ignored), 22 web tests passed; doc tests passed.
- `cargo xtask wasm`: passed.
- `cargo xtask ci`: all 11 jobs passed, including conformance, render allocation/RT
  safety, block-size independence, goldens, SIMD/FMA, trace, WASM, bare-metal no_std,
  Clippy and feature purity. Existing checksum-pinned corpus cache was copied into
  the integration target for the offline gate; no tests or exclusions were changed.
- Existing Chromium SAB W15 view gate: all eight named DOM cases passed.
- Additional deterministic review cross-check: 3,200 incremental batches across 32
  query/sort/scope/comment scenarios matched full-model order and tree counts.

Evidence: `/tmp/starplayer-w15-remediation-evidence/final-{node,rust,wasm,ci}.log`,
`coordinator-view-browser.log`, `incremental-crosscheck.mjs`. These intermediate results were followed by the final correction and R7 gates recorded below.

## Measured scheduling correction (2026-10-27)

R7 found a production gap that the focused R5 unit tests did not establish: native
boosted yield continuations starved both the publication and search debounce timers.
Natural 40k walks published only completion; independent 400k walks blocked timers
for about four seconds. Background postTask plus native yield restored partial
updates and timers with the controller unchanged. [R5a](W15-remediation-R5a-background-priority.md)
owns this narrow helper correction. R7 was notified and reran full/hidden/performance acceptance after reviewed integration. Existing natural
40k assertions stay intact; no inline controller flush or visibility listener is added.
See [Chrome priority inheritance](https://developer.chrome.com/blog/use-scheduler-yield#priority_inheritance)
and the [scheduling specification](https://wicg.github.io/scheduling-apis/).

### Priority correction reviewed and integrated

R5a integrated as `9580cca` after coordinator diff review, 45 focused tests and an
independent unchanged natural40k browser pass (partial rows, timer heartbeats, focus/
scroll and zero reads). R7 task baseline advanced to this integrated version. Final
Node/Rust/WASM/CI and full/hidden/performance browser gates passed after the correction; pre-correction artifacts remain explicitly labeled diagnostic evidence.

## Retained keyboard-selection correction (2026-10-27)

R7 retained the indexed-library 37,674-row Ctrl+A assertion and found a stale
selectedPlayable cache when keyboard focus is virtualized away. [R6a](W15-remediation-R6a-keyboard-selection.md)
owns the narrow user-selection recount before repaint. Ordinary renders keep their
no-recount path; metadata/status operation bounds remain unchanged. R7 repeated final acceptance after this correction; no existing assertion was weakened.

### Keyboard correction reviewed and integrated

R6a integrated as `8519ff2` after diff review, 50 focused tests and independent
37,674-row Chromium keyboard/Add selected checks. The correction only changes the
keyboard repaint branch. R7 baseline advanced while preserving its uncommitted
harness/docs work; final full/hidden/performance acceptance passed on this version.

## R7 native-cost follow-up decision (2026-10-27)

All native fixture correctness/cleanup gates passed, but large input invocation cost
is material: 128 MiB p50 150.020 ms / p95=max 181.165 ms, with four physical promises
and 512 MiB summed input bytes in the controlled burst. This is not measured total
browser memory. [Separate profiling follow-up](W15-followup-large-digest-start-cost.md)
records all sizes/distributions and requests diagnosis plus a reviewed recommendation.
No queue/cap/recovery implementation is added here. Final isolated 100k user re-sort
was 81.910 ms with an 88 ms overlapping long task, below the 100 ms slicing trigger; no sliced
query/sort follow-up is opened on that final measurement. Earlier loaded/exploratory
timings remain labeled as such.

## Final reviewed delivery and verification (2026-10-27)

This section records initial delivery through `1d40613`. Post-integration review
corrections below supersede its source-equality statement and Node count.

All remediation tasks and the two measured integration corrections are implemented,
reviewed, committed and verified on `w15-remediation` in
`/home/scott/projects/starplayer-w15-remediation`. Workers used GPT-6.1-sol at high
effort, did not stage/commit/merge, and reported bounded diffs/results. The coordinator
reviewed each diff, independently verified focused gates, and squashed each single
change from the integration checkout. No merge to main, push or deployment occurred.
These plans remain active until the future authorized main landing, following the
archive rule in [the plan index](../README.md).

| Task / correction | Integration commit | Result |
|---|---|---|
| R1 | `1e79716` | Fatal WASM traps retire/suspend worker; explicit Retry |
| R2 | `cc68dac` | Atomic page digest/metadata, eligible counters and row deltas |
| R3 | `4368b49` | Accepted library remains live during candidate preparation |
| R4a | `b0cf227` | Bounded owned archive session and duplicate compatibility |
| R5 | `f3fe09a` | Bounded cooperative source enumeration |
| R4b | `d23bb02` | Streamed groups of at most 64; one source read per group |
| R6 | `3b8ebe4` | Incremental view updates and bounded issue pagination |
| R5a | `9580cca` | Background-priority native yields preserve timer/input progress |
| R6a | `8519ff2` | Offscreen keyboard selection recount before repaint |
| R7 | `beafb22` | Combined browser/performance gates, packaging checks and help |

All automated gates passed. The final production tree includes R5a/R6a; the
integration app/archive sources are identical to the R7 tested branch. The coordinator
also rebuilt and ran remediation/Pages smoke directly in the integration checkout
following R7 integration. No worker result is substituted for an unexecuted required gate.

| Verification | Final result / evidence artifact |
|---|---|
| Complete Node suite | 300 passed, zero failed/skipped; `coordinator-final-node.log` and `final-node.log` |
| Rust archive/web | 31 archive + 22 web passed; doc tests passed; one pre-existing manual archive-generator test ignored; `final-rust.log` |
| WASM packaging | Development and Pages builds passed; `coordinator-final-wasm.log`, `coordinator-final-pages-build.log` |
| Full CI | All 11 jobs passed, including conformance, RT allocation/safety, block-size independence, goldens, SIMD/FMA, trace, WASM, bare-metal no_std, Clippy and feature purity; `final-ci.log` |
| Full Chromium regression | Unfiltered default-duration SAB, forced postMessage fallback and plain modes passed; retained indexed/remote/upload/playlist/reconnect/legacy/40k assertions; `r7-full-headless.log` |
| Remediation regressions | All three modes plus independent final integration SAB passed; real WASM endpoint trap/Retry, snapshot/version/atomic replay, eligible sidecars, pending candidate, shared groups and callback cleanup; `r7-regressions-all-modes.log`, `coordinator-final-remediation.log` |
| Archive matrices | All 12 ordinary/nested, small/above-32-MiB, 3/64/130-member cases per mode passed; measured source reads 1/1/3, recycling between groups, latest bytes on later group/playback; `r7-full-headless.log` |
| Native session ownership/cache | Rust cfg(test) proves one-inner reuse/invalidation, exact 128 MiB source ownership and freeing; browser does not export native inner-decode counters; `final-rust.log`, `r4a-session-evidence.log` |
| Hidden/freeze-thaw | 390,195 ms genuinely hidden with normal throttling; early and after-five-minute 65,536-row walks completed; same active iterator resumed safely; chained-timer gap about 60,000 ms; `r7-background.log` |
| Nonterminal visibility return | Pending row reached DOM about 121 ms and actual debounced row-change notification about 122 ms after return with another digest still pending; `r7-visibility-return.log` |
| Incremental view workload | 100,101 final rows / 10,000 issues; operation bounds, input, anchor/focus and 100-item issue DOM passed; `r7-performance.log` |
| Native digest fixtures | Valid loose 1/32/64/128 MiB and extracted 64 MiB inputs; native snapshot, atomic publication and complete promise cleanup passed; `r7-digest.log` |
| Local Pages subpath | Shipped isolation shim and actual nested catalog/metadata worker under `/starplayer/`; required assets 200; optional missing resources specifically accounted; `r7-pages-smoke.log`, `coordinator-final-pages-smoke.log` |

Artifacts are under `/tmp/starplayer-w15-remediation-evidence/`. The exact R7 commands,
launch flags, source/dist and artifact SHA-256 values, measurements and limitations are
in `r7-evidence.json`, with a short `r7-evidence.txt`. R7 task records reproducible
commands. All manifest artifact hashes were independently checked after handoff.
A permanent headless preflight compares ten critical source/dist JS files before
launch; this rejects stale packages, including foreign-worktree xtask-cache mistakes.
Pre-correction, stale-cache and smoke-only runs remain labeled diagnostic/nonfinal.

### Performance evidence and practical limits

Environment: Chromium 156.0.8078.4, Node 24.14.0, Rust 1.97.1, WSL2 Linux
6.6.87.2 x86_64, AMD Ryzen 9 5950X, 32 logical CPUs, approximately 25.4 GiB RAM.
Hidden runs used a real second tab and normal throttling, no fake audio/activity and
no disable-background-throttling flags. The observed native scheduling branch was
`scheduler.postTask(background) + scheduler.yield`.

For 20 display-only batches of 100 rows, revised work was 2,000 row evaluations,
zero full sorts/tree builds and no issue materialization. Fifty status ticks performed
zero row evaluations/full sorts/tree builds or additional issue getter reads. Adding
100 rows evaluated/contributed those 100 rows and merged once over 100,000 existing
rows. Five sort-key batches each evaluated 100 rows and merged once over 100,100;
unchanged rows were not fully sorted. The earlier full-rebuild strategy would evaluate
2,000,000 rows, sort/rebuild 20 times and render 10,000 issues for those display batches;
these baseline counts are derived from that strategy, not timed baseline executions.

With 20 visible rows and 100 issue DOM items, update latency p50/p95/max was
33.120/33.275/33.275 ms and handled-input latency 33.140/33.235/33.235 ms (20 samples,
including two animation frames). Full user re-sort was 81.910 ms synchronous with
88 ms overlapping longest task: below the 100 ms follow-up threshold. The workload's
overall longest task, including initial full construction, was 654 ms; the claim is
bounded incremental updates, not that all page work is short. No sliced query task
is opened. Headless results are specific to this machine and fixture.

The six-minute hidden run proves this configuration's scheduling and safe resumption,
not throughput for frozen/discarded pages. Its terminal digest probe had already
flushed before visibility returned, so it is not debounce evidence. The separate
quick, genuinely hidden nonterminal test above measures actual debounce publication;
it is not a six-minute hidden pending-delta measurement. No visibility listener or
forced flush was added. No observed result triggered a visibility-flush follow-up.

Native large-input invocation cost triggered the separate
[profiling follow-up](W15-followup-large-digest-start-cost.md), committed as `9a098a2`.
For 128 MiB, invocation p50/p95=max was 150.020/181.165 ms. A deliberate four-request
controller burst had four physical promises and a 512 MiB summed input-byte estimate;
this is not measured total browser memory or an upper bound on native overhead.
Each size had three repeats of four synchronous digest calls followed by one separately
measured real app activation. This does not establish natural playback concurrency,
statistical confidence or a universal device budget. The follow-up profiles the cost
and requests a recommendation; it does not authorize queues/caps/recovery machinery.

### Remaining owner acceptance and landing

- Exercise native directory-picker grant/deny/cancel and reconnect permission prompts
  in the target browser.
- Use an approved real large index-free collection on owner devices: browse/search,
  play/add during discovery, Stop/Resume, and loose/outer/nested playlist playback.
- Reload/reconnect saved entries before rediscovery; verify FileList/storage-denied
  reselection and explicit complete-scan rebind with the owner's environment.
- Review the separate measured large-digest profiling follow-up before choosing any
  later responsiveness/memory policy change.

Directory Size/Date remain unknown without stat enrichment; uncertain identity remains
conservative/session-only; first raw ZIP duplicate wins; Rebind pending remains gated
by complete discovery. These documented decisions are preserved. Main landing requires
future authorization and the repository's multi-commit `--no-ff` rule. No owner acceptance
is represented as completed by synthetic automation.

## Post-integration independent review (2026-10-27)

The owner requested the original review session to inspect the implemented branch and
iterate on findings. Round one reviewed `1d40613` and found two defects: fatal session
cleanup could mask a trapped mutable borrow, and playback without a requested scan
incorrectly changed metadataPhase to running/complete. Other reviewed contracts had no
substantive findings. Two low-consequence cases were recorded: programming exceptions
in a synchronous view callback can lose that flushed delta until a full refresh, and
missing WebCrypto in an insecure context produces a raw path issue. These are retained
edge cases, not silently claimed fixed.

[R8](W15-remediation-R8-trap-cleanup.md) was reviewed and integrated as `6a89c0f`.
The coordinator independently passed 102 focused tests and all three Chromium
remediation modes, including the new poisonedSessionTrapCleanup gate. A separate
minimal real wasm-bindgen 0.2.127 mutable-borrow panic probe reproduced the precise
ownership error on baseline and worker-failed/wasm-trap after the fix; no repository
Rust panic was injected. [R9](W15-remediation-R9-playback-scan-status.md) was reviewed
and integrated as `a18d224`; coordinator focused 59 tests and full 309 Node tests passed.
Both fixes were returned to the same reviewer session; round two closed both findings
with no new substantive regressions. Final post-review verification passed as recorded
below; original delivery evidence above remains historical. Artifacts: `/tmp/starplayer-w15-remediation-review/`.

### Review closure and final post-review gates

The original Claude CLI session `e80ca7f5-7ec4-43b0-9549-fcc7c76304f5`
reviewed `1d40613`, then re-reviewed the fixes at `a18d224`. It explicitly closed both
findings and found no new substantive regressions. It checked the actual pinned
wasm-bindgen mutable-borrow probe and requested no further reproductions. The reviewer
closed the code findings before the final integration run completed; the coordinator
subsequently verified every result below. CLI version 2.1.311; resumed model reported
claude-opus-5-5. Review was read-only; GPT-6.1-sol high workers implemented the fixes,
and the coordinator reviewed, verified, committed and squash-integrated them.

| Correction | Integration commit | Final behavior |
|---|---|---|
| R8 | `6a89c0f` | Never reenter poisoned WASM for session cleanup; preserve original fatal trap, completed progress, suspension and explicit healthy Retry |
| R9 | `a18d224` | Playback refreshes preserve idle/cancelled/completed scan intent; actionable Scan remains visible; active Scan and explicit row Retry keep their actual job phases |

Final production sources remain those reviewed at `a18d224`. One later harness edit
only corrects a diagnostic limitation string that assumed the old playback completion
flush; it changes no test logic/assertions. Both temporary review task branches and
worktrees were removed after clean-status and integrated-patch checks, retaining
`w15-remediation`. Main, historical assembly and committed AGENTS.md remain unchanged.
No cargo fmt, attribution, push, deployment or main merge occurred.

| Final verification | Result / artifact under `/tmp/starplayer-w15-remediation-review/` |
|---|---|
| Complete Node suite | 309 passed, zero failed/skipped; `final-node.log` |
| Rust archive/web | 31 archive + 22 web passed; doc tests passed; one pre-existing manual archive generator ignored; `final-rust.log` |
| WASM and Pages | Development/Pages packaging and local shipped-shim worker smoke passed; `final-wasm.log`, `final-pages-build.log`, `final-pages-smoke.log` |
| Full Chromium | Unfiltered default 60-second SAB/fallback/plain suites passed, including both new evidence flags, retained 40k/indexed/remote/upload/reconnect/legacy assertions and archive matrices; `final-headless.log` |
| Complete CI | All 11 jobs passed, including RT/no_std/conformance/goldens/SIMD/FMA/Clippy/feature purity; `final-ci.log` |
| Real borrow-guard probe | Baseline emitted ordinary ownership error; fixed endpoint preserved worker-failed/wasm-trap after A delivered; `borrow-probe-result.log`, `borrow-probe/` |
| Nonterminal visibility return | Genuine hidden tab, another digest held pending; DOM 135 ms / actual row-change notification 135 ms after return; `final-visibility.log` |
| Repeated hidden/freeze-thaw | 390,193 ms genuinely hidden with normal throttling; early and after-five-minute 65,536-row walks and same-iterator freeze/thaw passed; timer control maximum gap about 50,797 ms; `final-background.log` |

The hidden gates were repeated because R9 changes playback-only completion-flush
behavior. They make no frozen-page or real-device throughput claim. The delta had not been published before return and reached the DOM 138 ms afterward;
this records observed timing without assuming an immediate completion flush. The dedicated nonterminal visibility test measures
actual debounce separately. The hidden run initially overlapped CI; it is not an
isolated throughput benchmark. Earlier R7 100k-view/native-digest measurements remain
historical evidence for the unchanged view/scheduler and native hashing design, not
fresh timing claims after these corrections.

The first extra visibility attempt used the Pages package, whose omitted development
fixture produced a 404. It is retained as `visibility-pages-package-diagnostic.log`,
not counted as passing evidence. Rebuilding the development package and rerunning the
unchanged assertions passed. No test assertion was removed or relaxed.

Exact commands, source/dist identities and artifact hashes are in
`final-review-evidence.json`; `round-1-result.txt` and `round-2-result.txt` preserve the
reviewer reports. Final plan/link/DAG/scope/attribution/clean-worktree/main-preservation
checks are recorded by `validate-review.py`.

Only two review edge cases remain: a programming exception in a synchronous view
callback can lose that flushed delta until a full refresh; insecure HTTP without
WebCrypto produces a raw path issue. Both were explicitly classified low-consequence
by the reviewer. The already documented large-digest profiling follow-up remains
separate. Native permission prompts, real collection/device and saved-playlist/
reconnect/FileList/storage-denied/rebind checks remain manual owner acceptance.
