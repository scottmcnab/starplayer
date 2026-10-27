# NO-01 — Contracts, feature gates and ownership

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Planned; external task review pending; do not implement until requested |
| Depends on | Reviewed master; implementation explicitly requested |
| Blocks | NO-02, NO-04 |
| Parallel with | None; shared API foundation |
| Least-capable safe worker | `gpt-6.1-sol` (high) |
| Coordinator | `gpt-6.1-sol` (xhigh) |
| Verified by | Worker runs Verification; coordinator reviews diff/evidence; owner listening where specified |

## Context for a fresh agent

Read `AGENTS.md`, the linked master (the behavioral contract), and product architecture
and accuracy policy before editing. No historical source edits, cargo fmt, broad staging,
commit/PR attribution, render allocations/locks/panics or weakened determinism tests.
Use a task branch and sibling worktree. This file plus the master is the handoff; no
conversation context is required. Resolve material contract changes with the coordinator
and amend the master/foundation documents before implementing them.

Read core commands/events, engine source/context/channel/control modules, facade settings,
format and host Cargo manifests, and embedded workspace manifests. Preserve current
`Command::Stop` for existing callers. The new interactive web command has different
clear-and-hold semantics; this task defines it, NO-03 implements it.

## Deliverables

- Define concrete `NoteOverlapMode` (Original/Cut/Continue/ContinueDifferentNotes),
  instrument override (Inherit or explicit mode), capability bits and interactive
  transport state/commands. Internal Continue maps to UI Overlap. Separate enabled
  runtime state from persisted session selections. Use existing IDs, validated at boundary.
- Preallocate instrument/channel policy tables and generational ownership metadata only
  with note-overlap. Expose bounded setters, all-original defaults and module-generation
  invalidation; no Module/image schema changes and no serialized preset format.
- Add independently selectable feature forwarding through engine, facade, formats and
  host. Web defaults enable both; core defaults and embedded dependency graph enable
  neither. Gate optional state, reservations and dispatch, not native IT functionality.
- Define actual-new-note versus effect-retrigger/sample-replacement classification at
  the trigger seam. Keep existing trigger behavior until NO-04/05 wire policies.
- Specify capacity preparation: existing native budget + 64 custom tails + 16 audition
  voices for web with corresponding features. Hosts can choose budgets before playback.
  Define source-domain ownership so native IT and preview/custom stealing cannot reach
  unrelated foregrounds. NO-02 sizes articulation storage for total slot capacity.
- Define dynamic live-lane layout after tracker channels, bounded by VoiceTag channel
  representation. Preserve MIDI channel numbers separately from engine lane IDs. Reject
  unsupported layouts before playback with an explicit error; no wrapping/truncation.
- Update API docs and planned foundation sections with actual contracts. No new trait
  until the second concrete implementation exists.

## Research points

- Inspect every feature forwarding edge, including optional dependencies (`?/` forwarding)
  to avoid enabling unused formats/std accidentally.
- Audit all command matches, snapshot flags and constructor call sites. Keep current
  command wire values stable; allocate new opcodes in the host task.
- Enumerate fixed MIDI-base consumers and maximum admitted tracker width before defining
  the layout; record exact limits in API docs rather than assuming 48 is safe.

## Verification

- Unit tests for policy precedence/default/reset, invalid IDs/generations, lane capacity
  boundaries and independent feature combinations.
- `cargo test -p starplayer-engine` and `cargo test -p starplayer`; feature-specific tests
  with neither, each individually, and both flags once declared.
- `cargo xtask ci --job no-std-check` and `cargo xtask ci --job no-std-purity`; inspect
  embedded cargo feature trees using its documented toolchain, reporting availability.
- Canonical output unchanged; no feature actually alters playback in this task.

## Out of scope

No overlap execution, transport behavior changes, UI, format lowering, new sample
storage, default feature std, embedded firmware changes, or broad MIDI refactor.
