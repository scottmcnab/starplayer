# NO-02 — Format-owned per-voice articulation

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Instrument-only revision reviewed; staged/uncommitted for owner review; do not implement until requested |
| Depends on | NO-01 |
| Blocks | NO-03, NO-05, NO-06 |
| Parallel with | None during shared engine/format runtime edits |
| Least-capable safe worker | `gpt-6-astra` (high) |
| Coordinator | `gpt-6.1-sol` (xhigh) |
| Verified by | Worker runs Verification; coordinator reviews diff/evidence; owner listening where specified |

## Context for a fresh agent

Read `AGENTS.md`, the linked master (the behavioral contract), and product architecture
and accuracy policy before editing. No historical source edits, cargo fmt, broad staging,
commit/PR attribution, render allocations/locks/panics or weakened determinism tests.
Use a task branch and sibling worktree. This file plus the master is the handoff; no
conversation context is required. Resolve material contract changes with the coordinator
and amend the master/foundation documents before implementing them.

Read XM channel articulation/envelope update and IT per-voice update, note lifecycle and
filter code; inspect Instrument/MappedInstrument and ControlClock call sites. Existing
M11 language is aspirational: mapped live samples currently lack native articulation.
This is the highest canonical-regression risk and must land before enhanced behavior.

## Deliverables

- Extract concrete XM and IT articulation runtimes owned by their format crates, usable
  from tracker processors and loaded-module live instruments. Do not implement XM using
  IT semantics. Share real format computations, not parallel approximate copies.
- XM needs generational per-slot voice state for envelopes, key-off/fadeout, auto-vibrato,
  current gain/pan/pitch and sample identity; channel effect memory stays on the channel.
  Snapshot foreground articulation for detachment without losing carry/retrigger rules.
- IT runtime must address the entire prepared voice pool, including slots above 255,
  with checked allocation at construction. Keep the native eligible voice count/stealing
  policy unchanged; extra capacity is not permission to expand canonical IT polyphony.
- Expose explicit start/release/cut/control-step operations through concrete types.
  Original tracker ticks perform exactly the same ordered updates as before, once.
  Control-only steps advance articulation without effects, rows, flow or trace row events.
- Retain fixed-point/tables-only operations, existing random-state determinism and filter
  initialization. Live and song instances own separate runtime/random state so audition
  does not consume randomness that changes future tracker notes.
- Ensure reset, stealing and stale generations discard articulation safely and without
  callback-time allocation/drop. Wire no optional background creation yet.
- Correct M11's current-state claims and architecture descriptions narrowly to identify
  what this task lands and what NO-06 still owes; do not broaden M11 scope.

## Interfaces provided and source scope

Own XM/IT concrete runtimes and the provided `TrackerProcessor` control-step hook in
`engine/src/sequencer.rs`, with real XM and IT implementations and a no-op default for
formats without articulation. Preserve existing tracker-driven update ordering; NO-03
consumes the hook for held sequencing, NO-06 consumes the concrete runtime operations.
No per-program pool-sized runtime and no speculative live trait in this task: NO-06
lands that rack-level interface with both live implementations.

Own actual capacity plumbing while preserving legacy `MAX_VOICE_CAPACITY`, through `HostEngine::build`, facade
`NativeSequencer::new`, `host/src/source.rs` builders and format constructors. Follow
N+C+L+R, including the existing jam reserve in L, and size every indexed array from the
prepared pool. Validate failure off render. Native quota membership remains distinct
from storage size; NO-05 wires custom exclusions. Foreground XM ordering stays native;
NO-05 supplies the specified per-origin-channel background ordering.

Physical arrays include bounded per-domain retirement headroom (interactive web 339);
this is not extra active/native quota. Constructors accept actual configured capacity
without changing legacy defaults when features unify. Provide retirement state that is
owned until completion but cannot receive effects or further articulation updates.

## Research points

- Establish ordering of envelope carry, key-off, fadeout, auto-vibrato and filter writes
  from existing format code/tests; record exact preserved ordering in runtime docs.
- Audit IT state arrays and loops assuming VIRTUAL_CHANNELS, including duplicate-match
  scratch arrays, voice stealing, trace and capacities. All need consistent bounds.
- Distinguish mutable foreground tracker effects from persistent voice articulation;
  keep native XM effects dependent on envelope state working after extraction.

## Verification

- Existing XM/IT tests and pinned conformance results must not regress; compare trace
  and PCM before/after extraction with all instrument checkboxes unchecked.
- `cargo test -p starplayer-xm`, `cargo test -p starplayer-it`,
  `cargo test -p starplayer-engine`, `cargo xtask goldens --check`.
- Test capacities above 256, slot reuse/generation, envelope carry/sustain/release,
  IT pitch/filter envelopes and random variation. Verify allocation-free control steps.
- Run canonical determinism matrix and feature-disabled builds. Record existing known
  conformance failures rather than treating them as permission for new failures.

## Out of scope

No custom overlap policy, independent new envelope implementation, generic instrument
library, new expression controls, pause UI or broad compatibility repairs.
