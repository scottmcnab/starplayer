# NO-01 — Contracts, feature gates and ownership

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Integrated on feature/note-overlap-audition; automated NO-01 checks passed; no main landing |
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

- Define per-instrument `let_notes_ring` and global `note_overlap_enabled` booleans,
  both default false, capability bits and interactive transport state/commands. No mode
  enum, channel settings, inheritance, explicit Cut or unrestricted Continue. Keep global
  enable separate from remembered session selections; use validated existing IDs.
- Define immutable trigger identity: instrument, original note, originating channel and
  exact unmodulated base volume after all channel processing on the actual onset tick,
  including applicable fine/fast slides, before tremolo/tremor/envelopes or separate gains.
  Live input uses resolved event-velocity base volume before CC7; preserve raw velocity
  separately from gain-scaled input and document existing format-resolution collisions.
  Later volume/pitch effects cannot mutate identity; no new quantization or tolerance.
  Identity persists through key-off/fadeout/NNA release until the slot is freed or reused;
  retiring voices are excluded from duplicate scans.
- Preallocate instrument boolean tables only with note-overlap; compile ownership/age
  metadata with either optional feature. Expose bounded boolean setters with session/build
  generation validation, no channel-setting commands/tables or Module/schema changes.

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

Record immutable identity for every new voice when the host configures note-overlap
capability, regardless of instrument checkbox, global bypass or sample eligibility. No
canonical PCM change and no allocation on toggles. Provide a separate custom-detachment
flag alongside ownership domain, so custom live tails remain in L while song custom tails
use C, and bypass can clear both without clearing native-created backgrounds. Test a
note begun unchecked/bypassed, enabling its checkbox/global switch, then identical-note
replacement; the global switch must never gate identity capture.


## Research points

- Inspect every feature forwarding edge, including optional dependencies (`?/` forwarding)
  to avoid enabling unused formats/std accidentally.
- Audit all command matches, snapshot flags and constructor call sites. Keep current
  command wire values stable; allocate new opcodes in the host task.
- Enumerate fixed MIDI-base consumers and maximum admitted tracker width before defining
  the layout; record exact limits in API docs rather than assuming 48 is safe.

## Verification

- Unit tests for boolean defaults/reset, global bypass preserving selections, onset identity, invalid IDs/generations, lane capacity
  boundaries and independent feature combinations.
- `cargo test -p starplayer-engine` and `cargo test -p starplayer`; feature-specific tests
  with neither, each individually, and both flags once declared.
- `cargo xtask ci --job no-std-check` and `cargo xtask ci --job no-std-purity`; inspect
  embedded cargo feature trees using its documented toolchain, reporting availability.
- Canonical output unchanged; no feature actually alters playback in this task.

## Out of scope

No overlap execution, transport behavior changes, UI, format lowering, new sample
storage, default feature std, embedded firmware changes, or broad MIDI refactor.


## NO-01 implementation evidence — 2026-10-27

The working diff is ready for coordinator review and is deliberately uncommitted.
No main merge, archive move, deployment, historical-source edit, cargo fmt, golden
regeneration or unrelated staging occurred. Keep this task at its current path while the
feature is developed; dependent task links remain authoritative.

### Interfaces now provided

- `starplayer-core::interactive`: separate `SessionId`/`BuildGeneration`, atomic logical
  `InteractiveCommand` envelope, `InteractiveAction`, `InteractiveTransport`, capability
  bits, `TriggerKind` and `TriggerIdentity`. The latter uses existing zero-based
  `InstrumentId`, exact original `Note` or raw MOD period, channel and an unscaled native
  base-volume `u16`. No overlap mode enum, channel policy or Module/schema changes.
- `LaneLayout`: legacy 64-lane/48-based helpers remain; strict interactive mappings admit
  tracker widths 0–64 and live lanes 64–80. Invalid widths/MIDI channel numbers do not
  wrap or truncate. `ChannelTable::try_with_layout`, `EngineSettings::with_playback` and
  `Engine::try_with_settings` provide explicit pre-playback validation. Tracker insert
  buses/scope taps/telemetry stay at most 64 and live lanes go through spill/master DSP.
- `PlaybackConfiguration`/`VoiceCapacity`: configurable N+C+L+R, runtime capability opt-in
  independent of compiled flags, and the explicit 339-slot persistent web settings factory
  `EngineSettings::interactive_web`. Ordinary constructors retain 64/48/272 conventions
  and defaults even when workspace builds unify optional features.
- `ChannelTable::playback`/`playback_mut`: one optional preallocated `PlaybackState` owns
  generational domain/age/lifecycle and immutable onset identity. Ownership compiles with
  either feature; instrument tables/custom flags compile only with overlap, and the
  instrument table is empty without runtime overlap capability. Bounded setters validate
  session/build identity. New state starts all false; bypass preserves selections and
  returns `ClearScope::CustomDetached` for dependent ramp execution.
- Shared `detach_foreground` marks Background without changing native provenance;
  `detach_custom_foreground` marks actual custom detachment before unbinding. Only a
  Foreground may become custom-created, so authored song/live NNA backgrounds cannot be
  retroactively reclassified. Custom live tails stay in L. Live cleanup origin
  (Jam/SelectedInput/ButtonPreview) is distinct from domain and from custom detachment;
  its setter validates build/session and lane compatibility. It does not split L quotas.
- `TickContext::finalize_onset`: once-only post-effects identity capture for an actual
  musical onset, independent of checkbox/global state. Native key-off/fade/NNA release
  preserves identity; stale/reused and retiring voices cannot match. Duplicate iteration
  excludes the incoming VoiceId/generation and retiring slots. Effect retriggers and
  sample replacements do not produce new identities.
- `NoteParams::raw_velocity` preserves pre-CC7 event input whenever either optional
  feature is compiled; neither-feature layout and existing scaled playback gain remain
  unchanged. Rack/telemetry/sequencer routing reads the installed layout. Existing public
  legacy MIDI helpers, host event senders and SMF mapping keep their documented contract.
- Both flags forward independently through facade/formats/host; optional facade format
  edges and MIDI's optional engine edge use weak `?/` forwarding. Web packages default
  both flags on. Reusable/embedded defaults enable neither and no new edge enables std.

### Verification completed

All Cargo workloads were serialized with `CARGO_BUILD_JOBS=2`.

- `cargo test -p starplayer-engine` and `cargo test -p starplayer` passed. Each package
  also passed separate `--no-default-features` test runs for neither flag, overlap only,
  audition only, and both flags. Existing block-size invariants (1, 3, 64, 128, 4096,
  8191) were retained and passed in every engine run. A feature-off facade doctest
  initially failed because its pre-existing MOD example referenced disabled `mod_file`;
  the example body now has a narrow `cfg(feature = "mod")` guard, and the full matrix
  passed with the MOD example still compiled in default-feature docs.
- `playback_contracts` passes 10 tests with both flags. It covers defaults/reset, retained
  selections on bypass, passive identity capture while unchecked/bypassed, exact raw MOD
  period/native volume comparison, immutable lifetime through release/later parameter
  changes, invalid IDs/session/build/voice generations, source-domain/provenance guards,
  retirement exclusion/age, runtime exclusion, layout boundaries, and every lane 64–80
  routed unchanged past all tracker inserts at both 4- and 64-channel widths. The final
  foreground-only custom-provenance hardening was rerun separately with each flag and
  both. Raw-velocity unit coverage checks CC7 zero, intermediate and unity without reverse
  scaling; affected full engine/facade optional suites passed after broadening its gate.
- Separate `cargo check -p PACKAGE --no-default-features [--features FLAGS]` and
  `cargo tree -p PACKAGE --no-default-features [--features FLAGS] -e features` passed
  for all four combinations on engine, facade, MOD, S3M, MTM, XM, IT, MIDI, native host,
  WASM host and web: 44 checks and 44 trees. Inspection confirms parser-only MIDI flags
  do not activate its engine dependency, and facade flags alone activate neither unused
  formats nor std.
- `cargo xtask ci --job no-std-check` and `cargo xtask ci --job no-std-purity` passed.
  Additional engine/facade checks for all four combinations passed independently on both
  `riscv32imc-unknown-none-elf` and `riscv32imac-unknown-none-elf` (16 checks).
- `cargo check -p starplayer-host-wasm --target wasm32-unknown-unknown` passed with web
  defaults, and `cargo test -p starplayer-host-wasm` passed 49 tests, including the legacy
  48-based jam test under compiled optional web features.
- From `embedded/`, `cargo +1.97 test -p starplayer-firmware-common` passed 96 tests.
  Both installed `1.97` and `esp-1.97` toolchains produced firmware-common feature trees
  with neither optional flag using `cargo +TOOLCHAIN tree -p starplayer-firmware-common
  -e features,no-dev`. These are host/dependency checks, not firmware/device validation.
- `cargo xtask goldens --check` passed all eight committed hashes without regeneration.
  `git diff --check` passed.

Local review logs: `/tmp/starplayer-no01-verification.log`,
`/tmp/starplayer-no01-supplement.log`, `/tmp/starplayer-no01-final-contracts.log`.
The commands above are the reproducible evidence; logs/build products are not staged.

### Exact dependent integration still required

These do not relax the authoritative Deliverables above; they identify where NO-01's
provided contracts are consumed. No overlap policy or interactive transport executes in
this task, and no dependent behavior test is claimed complete.

- **NO-02:** thread prepared pool/layout/configuration through host/facade/source and
  concrete format constructors, size every indexed articulation array for physical pool
  capacity, and return allocation errors before playback. Ordinary web `Host::new`/Player
  still use legacy constructors; the explicit interactive settings factory is available
  for that plumbing. Quota admission/retirement enforcement consumes N/C/L/R, not the
  legacy 272 constant.
- **NO-03/04/05/06:** execute returned clear scopes with click-safe bounded ramps,
  invalidate bindings/held input and preserve ownership while retiring. Per-domain
  physical reserve fallback and native/custom/live victim protection still need their
  behavioral tests. Transport commands are logical vocabulary only; NO-03 executes them
  while preserving legacy `Command::Stop` and host fade/seek behavior.
- **NO-04:** classify actual MOD/S3M/MTM onsets (including delayed starts), pass original
  period/note and post-onset-tick `current_volume` to `finalize_onset`, capture regardless
  of current selection/bypass/loop eligibility, and implement replacement/detachment.
  The unchecked/bypassed note → enable → identical-note test currently proves identity
  capture/matching at the seam; actual audible replacement belongs here.
- **NO-05:** finalize XM's named exact base field and IT `note_volume` after their onset
  effects, preserve native NNA/DCT ordering, register any direct pool admissions in the
  same authoritative state, and wire post-onset duplicate action. Cover key-off/fade/NNA
  lifetime, mixed eligibility/provenance, onset/fine-slide/delay/capacity pressure and
  toggle-then-repeat behavior. Only native format workers can choose the correct seam;
  the shared allocation helper cannot derive it from modulated `VoiceParams.volume`.
- **NO-06:** wire actual live native-format velocity/default resolution before CC7 using
  `raw_velocity`, document/test collisions from existing native volume resolutions with
  no new identity quantization, set actual live origin synchronously on admission, and
  provide the 17-lane prepared live rack/held-instance lifecycle. Existing simple/mapped
  instruments are not presented as faithful XM/IT articulation. Host event-sender/SMF
  mappings remain legacy until explicit layout-aware configured host plumbing is used;
  hardware MIDI channel identity stays separate from engine lanes.
- **NO-04/05/07:** run five-format feature-on canonical-equivalence comparisons with
  passive recording wired for all notes in the three reviewed from-start configurations
  (all unchecked; bypassed/all checked; enabled/only wholly ineligible checked), then
  actual enabled-policy determinism and RT allocation hooks. NO-01's seam tests/default
  goldens do not claim these future format behavior comparisons.
- **NO-03/W1:** add separate live telemetry/control-only publication and versioned
  interactive wire capability/ack negotiation; existing wire opcodes and 64-lane tracker
  telemetry have not been repurposed. No UI transport or overlap controls are implemented.

### Coordinator review

Reviewed contract, feature graph, ownership transitions, lane routing and documentation.
`CARGO_BUILD_JOBS=2 cargo test -p starplayer-engine -p starplayer --features
note-overlap,interactive-audition` passed on the final diff, including all 10 contract
tests and existing determinism/doc tests. `git diff --check` passed. No behavior from
dependent tasks is claimed implemented. Task remains at this path until main landing;
only task/feature integration is authorised.
