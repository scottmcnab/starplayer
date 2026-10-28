# NO-02 — Format-owned per-voice articulation

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Landed on main via e031eda (2026-10-28); implementation and automated verification complete; owner listening/device acceptance pending |
| Depends on | NO-01 |
| Blocks | NO-03, NO-05, NO-06 |
| Parallel with | None during shared engine/format runtime edits |
| Least-capable safe worker | `gpt-6-astra` (medium; explicit owner assignment 2026-10-27) |
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

## NO-02 implementation evidence — 2026-10-27

Review-ready working diff, deliberately uncommitted pending coordinator review. No main
merge, plan archive move, golden regeneration, cargo fmt or historical-source edits.

### Provided interfaces and preserved ordering

- `XmVoiceState` holds only articulation and its instrument/sample references, current
  input/output gain, pan and period. `XmChannel` retains row columns, effect memories,
  tracker LFOs, base `real_volume` and trigger/flush bookkeeping; its existing public
  field reads continue through `Deref`. Native start/key-off/control computations are
  shared with `XmArticulationRuntime`, not copied into a second envelope implementation.
  The runtime provides fallible preparation, start, snapshot, release, cut, retire,
  discard, reset and per-generation control step. Start/snapshot validate against the
  actual `VoicePool`, including generation, before installing state.
- XM preserves **per-channel effects → articulation → flush**. Articulation itself is
  key-off fadeout → volume envelope → panning envelope → auto-vibrato → final volume
  product. Optional snapshots are taken immediately after native flush, without resetting
  envelopes or replaying an onset. Control-only steps use stable channel/slot order and
  frozen module global volume, with no row/effect/flow/trace-row processing. Foreground
  working state is synchronized back after held steps so resume continues that envelope.
- **XM sample-reference caveat:** FT2 invalid/transposed-note paths can change the
  channel's instrument/sample references while leaving an earlier mixer region sounding.
  The extracted state preserves that native articulation behavior. NO-05 must determine
  actual PCM/loop eligibility from the sounding mixer voice/tag/region, never assume
  `XmVoiceState.sample` is always the currently sounding PCM. Onset base volume remains
  the channel's unmodulated `real_volume`; NO-05 owns final identity capture/policy.
- `ItArticulationRuntime` owns native generational `ItVoiceState` storage and RNG, exposes
  fallible preparation plus start/release/cut/retire/discard/reset, native random swing
  and envelope-flag initialization, and `control_step` returning `ItArticulationOutput`.
  The tracker adapter retains mixer writes, scaling, trace and foreground/background
  reclamation. Start checks the live pool generation; stale mutation/control handles do
  nothing, including no RNG consumption. Retiring state is retained but does not advance.
  `ItArticulationOutput.params.step` is the native module-rate step; the consumer applies
  normal engine region scaling when writing it. Native filter extended-range setup stays
  at the note admission seam; the live adapter must likewise initialize the mixer filter
  on admission and pass `trigger_note` only for the onset step.
- IT preserves **all channel effects/synchronization → ascending slot articulation →
  trace**. Each slot advances/evaluates volume, pan and pitch/filter envelopes, applies
  fadeout and the volume product, pitch envelope, sample auto-vibrato, pan and filter,
  then writes changed parameters. The existing one-tick-late sustain release and filter
  disengagement-on-trigger rules remain. Song effects and articulation still share their
  original RNG stream in the original order; a separate live runtime owns a separate
  stream and consumes none of the song's random values.
- `TrackerProcessor::control_step(&mut TickContext)` is a provided no-op for other
  formats, with real XM/IT implementations. It does not schedule itself: NO-03 owns
  non-sequencing dispatch and cadence. Both adapters consult engine-owned retirement
  records before articulation writes; internal runtime suppression is not a competing
  domain/provenance/age registry. No custom backgrounds or live runtime trait were added.

### Capacity, preparation and resource exclusion

- IT's fallible capacity constructor no longer clamps at legacy 272. Its state and shared
  DCT/S7 target scratch cover the entire requested physical pool, with checked reservation
  before dispatch. Native admission/stealing stays capped at 256; tests fill low slots
  with unrelated voices and force native high-slot stealing at both 272 and 339 capacity.
- XM processor slot storage/snapshot/control dispatch is compiled only with either optional
  feature and allocated only by explicit `try_with_articulation`/`prepare_articulation`.
  Ordinary/default and capacity-only constructors leave it absent even when features are
  unified. Shared `XmVoiceState` arithmetic remains unconditional. This avoids adding a
  pool-sized XM table to legacy/embedded playback. No per-program runtime is allocated.
- Facade `NativeSequencer::new_with_voice_capacity` and existing fallible capacity APIs
  prepare the full native IT pool but retain legacy XM behavior. Explicit
  `new_with_articulation` additionally prepares XM snapshots. `new` preserves legacy 272
  default physical storage; per-module fallible construction can still use native size.
- `HostEngine::build_with_settings`, `Player::open_with_settings` and
  `build_source_with_settings` carry actual settings through installation, jam source
  rebuilding and renderer rebuilding. The configured source gets its rate from settings.
  Plain host/source constructors retain 272 and legacy lanes; configured web settings
  produce 339 physical slots and 81 lanes, without adding another jam reserve to L.
  Engine voice-pool and format/ownership preparation failures are returned off-render;
  existing unrelated engine-component allocations retain their prior infallible behavior.
- `EngineSettings::playback_configuration` and the facade's explicit prepared entry are
  unconditional APIs. The host therefore cannot silently skip optional XM preparation
  merely because a different workspace consumer enabled the dependency features. A
  facade without optional preparation support explicitly rejects that requested entry.
  Runtime capability/settings, not feature unification alone, enable the allocation.

### Verification completed

All heavy Cargo workloads were serialized with `CARGO_BUILD_JOBS=2` and
`CARGO_INCREMENTAL=0`, using the coordinator-prepared shared generated cache and immutable
pinned corpus link. No test expectation was loosened and no golden was regenerated.

- `cargo test -p starplayer-xm -p starplayer-it -p starplayer-engine -p starplayer
  -p starplayer-host --features note-overlap,interactive-audition` passed. Final follow-up
  passes reran the changed XM/IT/facade/host packages after admission and retirement fixes.
- XM/IT/engine also passed `--no-default-features` separately with neither flag, overlap
  only, and audition only. Existing canonical block-size invariants remain unchanged.
- New tests cover slot 338, stale generation start/snapshot/release/cut/control, retirement,
  unchanged random streams on rejected operations, all 339 DCT/S7 candidates, the native
  256 quota with wider storage, envelope steps without channel volume slides, native XM
  tick equivalence with optional snapshots, and engine-owned retirement suppressing both
  held and native XM voice writes. Existing tests/conformance retain sustain/release,
  carry, auto-vibrato, IT pitch/filter envelope and random-effect coverage.
- Five-format facade tests compare independently reset canonical PCM against explicit
  339-slot optional preparation with selections unchecked at host sizes 1, 3, 64, 128,
  4096 and 8191. Outputs are byte-identical. This is extraction/preparation equivalence;
  NO-04/05 still owe passive format identity capture and the full policy toggle matrix.
- Allocation-failure injection traverses every prepared facade constructor allocation,
  including XM's optional table and IT's wider state/scratch, and verifies errors return
  without leaking module owners. A direct XM allocator rejection and impossible-capacity
  case also return errors. Host tests confirm legacy272 versus explicit339/81 and reject
  inconsistent configured capacity before rendering.
- `cargo xtask conformance --offline` exactly matches the supplied baseline:
  **261 total, 179 pass, 0 unexpected fail, 16 deviations, 66 known failures, 0 gated**.
  Existing known failures/deviations remain recorded in the repository policy/exclusions.
- `cargo xtask goldens --check`: all eight committed hashes unchanged, including XM/IT.
- `cargo test -p starplayer-offline --test render_allocation`: all 11 hooks passed,
  including the added concrete XM/IT start/control/release/retire/reset test and existing
  dense-IT 30-second and corpus rendering tests. The hook counts deallocation too.
- `cargo xtask ci --job no-std-check` and `cargo xtask ci --job no-std-purity` passed.
- Affected XM/IT/facade/host all-target Clippy with both flags and `-D warnings` passed.
  `git diff --check` passed. No device/listening validation or full future NO integration
  acceptance is claimed.

Local reproducible logs: `/tmp/no02-final-tests.log`, `/tmp/no02-feature-tests.log`,
`/tmp/no02-conformance.log`, `/tmp/no02-goldens.log`, `/tmp/no02-allocation.log`,
`/tmp/no02-no-std.log`, `/tmp/no02-clippy.log`. Logs/cache are not staged.

### Dependent work remains

NO-03 schedules the held control hook and supplies clear/retirement execution. NO-05
owns custom admission/exclusions, originating-channel XM background ordering, detach
capture and exact onset identity. NO-06 constructs one live runtime per rack, not per
program, provides both real rack-level implementations in the same trait commit, and
integrates mapping/NNA/held-note release and the shared articulation cadence. No live
source/rack is installed by this task; current `MappedInstrument` remains sample-only.
M11 and architecture current-state descriptions now make these boundaries explicit.

### Coordinator review

Reviewed extraction arithmetic/order, slot generation validation, native quota, retirement,
optional XM storage exclusion, host/facade configuration and task boundaries. Final
`CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo test -p starplayer-xm -p starplayer-it
-p starplayer-engine -p starplayer -p starplayer-host --features
note-overlap,interactive-audition` passed on the final diff. `git diff --check` passed.
No overlap policy, held clock scheduling or faithful live runtime is claimed here.
