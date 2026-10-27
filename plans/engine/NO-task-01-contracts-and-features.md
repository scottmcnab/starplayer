# NO-01 — Contracts, feature gates and ownership

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Reviewed; ready for coordinator handoff; do not implement until requested |
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
- Preallocate policy tables only with note-overlap; compile generational ownership/age
  metadata with either optional feature and gate custom policy fields separately. Expose bounded setters, all-original defaults and module-generation
  invalidation; no Module/image schema changes and no serialized preset format.
- Add independently selectable feature forwarding through engine, facade, formats and
  host. Web defaults enable both; core defaults and embedded dependency graph enable
  neither. Gate optional state, reservations and dispatch, not native IT functionality.
- Define actual-new-note versus effect-retrigger/sample-replacement classification at
  the trigger seam. Keep existing trigger behavior until NO-04/05 wire policies.
- Specify capacity preparation: existing native budget + 64 custom tails + 16 audition
  voices plus per-domain retirement reserve R for web with corresponding features and
  runtime opt-in. Hosts can choose budgets before playback.
  Define source-domain ownership so native IT and preview/custom stealing cannot reach
  unrelated foregrounds. NO-02 sizes articulation storage for total slot capacity.
- Define interactive live lanes 64–80 beyond all tracker channels, bounded by VoiceTag channel
  representation. Preserve MIDI channel numbers separately from engine lane IDs. Reject
  unsupported layouts before playback with an explicit error; no wrapping/truncation.
- Update API docs and planned foundation sections with actual contracts. No new trait
  until the second concrete implementation exists.

## Interfaces provided and source scope

Own `core` command/capability types, `engine/src/channel.rs` and ownership metadata,
feature manifests, and the layout contract consumed by NO-02–06/W1. Implement the
master's 17-live-lane layout (maximum 81), sharing 16 lanes between jam/selected input
and reserving one Preview-button lane. Audit/update layout-aware consumers in
`engine/src/instrument.rs`, `sequencer.rs`, `telemetry.rs`, host-wasm tests and web routing
in coordination with W1; preserve public legacy MIDI-base helpers when interactive mode
is absent. Mixer still has 64 tracker buses plus spill, not 81 insert buses. The required
bounded routing refactor is in scope; an unrelated MIDI redesign is not.

Provide one authoritative engine per-generation ownership/provenance/age record, scoped
clear contracts, and capacity configuration N+C+L+R. NO-02 owns constructor plumbing and
storage sizing; NO-03 owns clear execution; NO-04/05/06 integrate their domains. IDs
separate user session from build generation. Do not call session selections persistent
presets: they are retained only within the current user load.

Feature proof requires separate per-package `--no-default-features` checks for neither,
each and both flags, plus `cargo tree -e features`; a whole-workspace build unifies
web features and cannot prove exclusion. Run the embedded workspace's host test
`cargo test -p starplayer-firmware-common` with its documented host toolchain.

Runtime settings must opt into the capability even when features are compiled through
workspace unification. Preserve legacy constants 64/48/272 and constructor behavior;
add interactive constants/layout constructors and explicit web settings. Own decoupling
engine.rs insert bus_count from table length (maximum 64); test a 4-channel module and
64-channel module both route live lanes 64–80 through spill with no tracker inserts.

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
