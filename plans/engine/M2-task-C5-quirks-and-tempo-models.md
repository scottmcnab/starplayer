# M2-task-C5 — QuirkSet and TempoModel, wired end to end

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Depends on | C3 (MOD) |
| Blocks | — |
| Parallel with | C6 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests per quirk) |

## Context for a fresh agent

`TempoModel` was declared in M0-A2 with three implementations. `QuirkSet` has been
referenced by every accuracy decision so far but does not exist yet. This task makes both
real, and threads them from the host API down to the effect processors.

Read `plans/product/03-accuracy-policy.md` — §1 lists quirks that are always on, §2 the
ones gated behind `quirks-starplayer`, §3 the defects we never reproduce. This task
implements §2's gate.

The philosophy: quirks are **data, not branches scattered through the code**. A quirk
that cannot be expressed as a `QuirkSet` field probably is not a quirk — it is a format
difference and belongs in that format's processor.

## Deliverables

1. **`QuirkSet`** — a plain struct of named booleans and small enums, `Copy`, with named
   constructors:
   ```rust
   impl QuirkSet {
       pub fn canonical() -> Self;          // the default
       pub fn starplayer_classic() -> Self; // accuracy policy §2
   }
   ```
   Every field carries a doc comment naming the accuracy-policy entry it implements. A
   field without a policy entry is not allowed.

2. **Threading.** `QuirkSet` reaches the effect processors from the engine configuration,
   not from a global. It is fixed for the lifetime of a loaded module — changing it
   mid-playback is not supported, and the API should make that obvious.

3. **`TempoModel` selection**, likewise, with `ExactFixedPoint` the default and
   `St3Truncating` available. Make the drift difference observable: a test that renders
   the same module under both and asserts the frame counts diverge by the expected amount
   is worth more than a comment.

4. **The `quirks-starplayer` feature flag** gating the classic profile's *code* where it
   would otherwise cost anything at runtime. Where a quirk is a single branch on a
   `QuirkSet` field, no feature gate is needed — prefer the simpler form.

5. **Documentation** in the facade crate's docs explaining what a quirk profile is, when
   to use it, and pointing at the accuracy policy.

## Research points

1. Whether any quirk needs to vary *per format* rather than per session. If so,
   `QuirkSet` should be per-format-processor rather than per-engine; decide before
   threading it.
2. Whether `ItModern`'s tempo semantics (tempo slides) can be stubbed usefully now or
   should stay a `todo` until M6. Prefer an honest stub that returns the exact model's
   answer and is documented as incomplete.

## Verification

- `QuirkSet::canonical()` and `::starplayer_classic()` differ in exactly the fields the
  accuracy policy §2 lists — assert field by field, so adding a quirk without updating
  the policy fails the test.
- Rendering a module under both tempo models produces the expected frame-count divergence
  over a long render.
- Under `canonical()`, all M2 conformance results are unchanged from before this task.
- Every `QuirkSet` field is referenced somewhere in the codebase (a dead quirk is a bug).

## Out of scope

Any new quirk discovered during M2 that is not yet in the accuracy policy — add it to the
policy first, then here.
