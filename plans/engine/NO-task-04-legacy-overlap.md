# NO-04 — Legacy-format overlap and bounded tails

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Integrated on feature/note-overlap-audition; task commit 61138bb; no main landing |
| Depends on | NO-01 |
| Blocks | NO-05, NO-06 |
| Parallel with | NO-02/03 only with isolated worktrees and explicit shared-seam ownership |
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

Read MOD/S3M/MTM note handling and flush/retrigger paths plus ChannelTable trigger/detach,
VoicePool release/iteration and SampleRegion queued swaps. These formats currently cut
on replacement and reserve one voice per channel. Implement only opt-in behavior.

## Deliverables

- Implement instrument/global boolean resolution and duplicate matching in a reusable concrete engine
  helper; format processors explicitly identify new-note starts. Incoming identity
  chooses duplicate matching; outgoing actual voice identity chooses survival.
- Track custom-tail generation, age and originating channel. Keep original voice sample,
  parameters and routing when detaching. Replace all candidates matching instrument,
  original note, exact captured onset volume and originating channel; no cross-channel match.
- Enforce prepared custom budget: oldest custom tail first, stable slot tie-break,
  never unrelated foreground. When no eligible capacity exists, replace foreground
  normally and report diagnostic. Release/ramp behavior uses mixer primitives.
- Reject custom detachment for looping/sustain/non-PCM regions. Discard queued sample
  swap only on a successfully detached eligible one-shot, so it cannot loop later.
- Wire MOD/S3M/MTM delayed musical notes; preserve native portamento, effect retrigger,
  sample-only, offset-past-end and invalid-note behavior. Blank/no-op rows create no tail.
- Checkbox edits affect subsequent note starts; bypass clears custom-provenance tails.
  Module replacement clears voices and ownership together. Legacy seek/restart retains
  custom ownership for any still-sounding tails; never discard bookkeeping alone.
  Interactive seek uses scoped clearing. Preserve mute/channel insert routing.
- Add event-frame tests and update accuracy/architecture implementation status.

## Interfaces consumed and cleanup integration

Consume NO-01 ownership/provenance and layout contracts; do not create another age/
provenance table. Own MOD/S3M/MTM trigger seams and the concrete engine overlap helper
consumed by NO-05. Integrate NO-03 scoped clears after its landing: Stop and global
all-sound-off clear tails/bindings/metadata, interactive seek clears song tails only,
and bypass clears custom tails only. Retiring ramps keep ownership but not active quota; use the master’s per-domain retirement reserve/fallback. If this
task is prepared before NO-03, coordinator must integrate and test the clear hooks before
landing it. Add Stop/seek/bypass tests with active tails and unrelated live voices.
Player shadow state retains instrument checkbox/global selections on rebuild, resets on explicit user load.

## Research points

- Inspect dirty-bit SAMPLE triggers that are not actual notes, especially MOD queued
  swaps/revival and S3M retrigger. Carry cause through delayed processing explicitly.
- Inspect original-note storage for raw MOD periods and unusual pitch values; use stable
  original trigger identity, not current slide pitch or a lossy nearest-note conversion.
- Capture `current_volume` after all channel processing on the actual onset tick,
  including fine/fast slides; never `actual_volume` (tremolo/tremor modulation). Finalize
  before replacement/mixing at that frame. Test MOD Cxx and EAx onset rows separately,
  a MOD delayed EDx note inheriting an earlier Cxx without a sample-default reset, and
  S3M tick-zero tremor/fast slides. MOD has one effect column: do not invent EDx+Cxx in
  one cell. Later slides/envelopes/gains never rewrite identity. Test unchecked/bypassed
  starts followed by enable and repeat; equal-volume echoes intentionally replace.
- Check release ramp, pending swaps and natural sample completion do not leak ownership.

## Verification

- `cargo test -p starplayer-mod`, `cargo test -p starplayer-s3m`,
  `cargo test -p starplayer-mtm`, `cargo test -p starplayer-engine` and goldens.
- Snare→kick, same-volume C→G→C and repeated snare, quieter same-note echoes, same
  instrument across channels, checkbox/global changes, invalid sample, all loop types, pending MOD swap, pool saturation/reuse.
- Block-size determinism and no-allocation tests with overlap active; compare all-unchecked
  feature-on output against feature-off golden output. Never update canonical goldens.

Canonical comparisons for each format start from fresh state with identity recording
active: global on/all unchecked; global off/all checked; global on/only wholly looping
instruments checked. Compare each to feature-off; mid-song bypass has separate cleanup tests.


## Out of scope

XM/IT articulation, loop release synthesis, UI, changing canonical effect semantics,
new mixer implementation or editing historical assembly.


## NO-04 implementation handoff — 2026-10-28

Review-ready working diff in `../starplayer-no-04-legacy-overlap`, based on `aaefc9f`.
No commit, main merge, archive move, deployment, historical edit, cargo fmt, unrelated
staging or golden regeneration occurred. Keep this task at its original path pending
main landing. NO-05/06 remain separate implementation tasks.

### Exact implementation interfaces

- `starplayer_engine::overlap` provides the concrete `eligible_one_shot` sample predicate,
  `OverlapDiagnostics`, and `ChannelTable::{custom_overlap_candidate,prepare_note_overlap,
  replace_note_overlap_duplicates}`. There is no new trait, mode or channel override.
  Eligibility validates the actual one-based sample tag against its region's PCM offset
  and both model loop fields; it rejects zero/unmapped/empty/stopping/normal-loop/sustain
  voices, including aliasing PCM and released-sustain one-shot regions. Current sounding
  instrument checkbox/global state decides each candidate's action; immutable trigger
  instrument/note/volume/channel decides exact duplicates. Retiring generations cannot
  participate. Native-created backgrounds retain N, custom tails C, live voices L.
- `prepare_note_overlap` owns song foreground N→C admission and atomic custom detachment,
  using the authoritative PlaybackState age/domain/generation records. It evicts the
  oldest custom tail (stable slot tie-break) through NO-03 retirement, uses that domain's
  already-retiring fallback at its physical limit, and never steals an unrelated native
  foreground. Failure leaves native replacement available and counts a refused detach;
  successful pressure eviction counts a custom eviction. Both counters are saturating
  and exposed by `PlaybackState::overlap_diagnostics`. The shared predicate/duplicate
  scan also supports NO-05 candidates; NO-06 still owns live L admission and held instances.
- `TickContext::trigger_legacy_overlap` executes preservation, trigger, passive capture
  and all-match replacement synchronously at the format's post-effects flush before any
  mixing. Only a real musical start requests detachment/duplicates. MOD/MTM retain a
  row-local original note and explicit pending cause through EDx; S3M carries a separate
  musical cause alongside SDx's native saved dirty byte. No inference from SAMPLE dirty
  bits, tremor/tremolo gain, slid pitch or a nearest MOD note is used. Native E9x/Qxy and
  sample revival copy the prior immutable identity to a replacement mixer generation via
  `PlaybackState::inherit_onset`, without being classified as a new musical onset.
- MOD captures its exact packed input period, MTM its native linear input, and S3M its
  resolved semitone. All capture native `current_volume` after defaults, Cxx/EAx/EBx,
  supported S3M fine slides and Qxy's supported tick-zero volume operations. EDx/SDx
  capture when they actually start; later effects and separate gains never rewrite it.
  Passive capture runs while unchecked, globally bypassed and sample-ineligible.
- `Voice::tag_pending_sample` records a pending instrument/sample tag only for the
  matching queued PCM. Boundary adoption and native retrigger commit both together;
  `discard_pending_region` clears both only after a successful eligible custom detach.
  The optional metadata is a packed u32 in existing slot padding; the unchanged 168-byte
  x86_64 slot-stride assertion passes, and the field/branch is absent without mixer
  `note-overlap`. Engine forwards that flag. MOD starts tag the actual mapped model sample.
  A completed MOD swap changes sounding checkbox/eligibility but not original identity.
- Native offsets and swaps remain intact. MOD 9xx past end really starts its documented
  retained-pointer one-word fallback and therefore records an onset/preserves eligible
  outgoing sound. MTM/S3M raw out-of-range one-shots do not detach or invent an onset.
  Looping incoming regions retain native wrap behavior. One-shot-to-one-shot queued swaps
  still end natively. Detached tails keep sample, parameters, pan and originating lane
  mute/insert routing; channel effects continue to target only foreground.
- Existing NO-03 clears execute without a second ownership table: bypass retires custom
  provenance only; Stop/all-sound-off include tails; valid interactive seek retires song
  foreground/backgrounds while preserving unrelated live; invalid seek preserves sound.
  Legacy seek/restart leaves still-sounding custom ownership visible until completion or
  bypass. Module replacement now sweeps finished ownership after releasing all voices.
- Persistent Player loads prepare selection boxes to the actual module count, including
  SMF instrument modules, regardless of count0/stale initial settings. `EngineSettings::
  set_instrument_count`, `Engine::replace_instrument_selections` and HostEngine forwarding
  update count/storage while preserving the voice ownership slots. The replaced box
  travels with the old source in one Retired installation record. Explicit loads reset
  selections/global and session; same-session install/rebuild retains/replays settings
  and updates stored bounds. Invalid/stale IDs are rejected against the installed module.
- Prepared loads in **either optional capability** reserve source/table and later module
  retirement room before any mutation and preflight the engine command ring. A bounded
  pending command retains an entire blocked Load in order; at most one prepared load
  enters a quantum. An old module's garbage is moved before the next admission, without
  popping into a full retirement queue. Burst-load tests cover overlap-only, audition-only
  and both, including callback deallocation detection. Legacy constructors remain native.
- MTM's optional features now forward to its actual `starplayer-mod` effect-core dependency,
  as well as engine. Dedicated MTM-only tests prove real overlap and Stop/clear retrigger
  suppression without facade all-format feature unification.

### Verification evidence

All heavy Cargo workloads were serialized with `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`;
test batches also used `RUST_TEST_THREADS=2`. Shared target/corpus caches were only used,
not deleted or regenerated. No canonical expectation was weakened.

- Full affected suite passed with both flags:
  `cargo test -p starplayer-mod -p starplayer-s3m -p starplayer-mtm -p starplayer-mixer
  -p starplayer-engine -p starplayer -p starplayer-host --features
  note-overlap,interactive-audition` (`/tmp/no04-final-targeted.log`). The strengthened
  tick-zero tremor assertion was rerun in the final 19-test `legacy_overlap` regression
  (`/tmp/no04-final-onsets.log`). Existing engine block-size invariants remain unchanged.
- `legacy_overlap` covers all three real native processors: snare→kick, C→G→C, repeated
  hits, quiet echoes, all exact matches, different channels, checkbox/global edits,
  passive enable-then-repeat, exact raw MOD periods, Cxx/EAx/EBx and supported S3M fine/Qxy
  onset operations, tick-zero tremor silence, inherited delayed base volume, retriggers,
  sample-only/blank/portamento/invalid starts, offsets, normal/sustain/unmapped/aliased PCM,
  pending/adopted MOD swaps, saturation/reuse, foreground protection, retained parameters,
  natural completion and origin mute routing. Dedicated MTM-only feature tests pass.
- Enabled synthetic PCM is byte-identical at host sizes **1, 3, 64, 128, 4096, 8191** and
  differs from native PCM (the fixture actually overlaps). From independently reset state,
  every format matches native PCM in all three required configurations at all six sizes:
  enabled/all unchecked; bypassed/all checked; enabled/only wholly looping checked.
  Additional loaded MOD/S3M/MTM seed fixtures verify the same matrix and assert that
  passive identity recording actually happened in every configuration.
- Host tests cover count0 opening, modules with 1/2/5 actual instrument counts, explicit
  load resets, invalid/stale IDs, same-session rebuild replay and SMF bounds. Scope tests
  use real MOD tails plus an unrelated scripted live voice for bypass, Stop/all-sound-off,
  valid/invalid held seek and legacy seek ownership. The real faithful live rack is NO-06.
- Independent `--no-default-features` test suites passed for affected packages with
  neither flag, overlap only and audition only (`/tmp/no04-feature-{off,overlap,audition}.log`).
  MTM alone passed `cargo test -p starplayer-mtm --no-default-features --features FLAG
  --test optional_playback` separately for each flag (`/tmp/no04-mtm-only-{overlap,audition}.log`).
  Final host suites rerun every host test separately in all three optional configurations
  after the prepared-load retirement review fix (`/tmp/no04-host-final-*.log`).
- All affected all-target Clippy with both flags and `-D warnings` passed
  (`/tmp/no04-clippy.log`); the final host retirement change has its own final Clippy log.
- `cargo xtask goldens --check` passed **all 8** existing hashes unchanged
  (`/tmp/no04-goldens.log`). `cargo xtask conformance --offline` exactly matches baseline:
  **261 total, 179 pass, 0 unexpected failures, 16 deviations, 66 known failures, 0 gated**
  (`/tmp/no04-conformance.log`). Corpus inputs were read only.
- Callback allocator hooks watch **allocation and deallocation**, and include actual
  enabled MOD/S3M/MTM detachment, custom eviction, bypass, actual table installation and
  burst prepared loads with full retirement queues. With both flags all 8 hooks passed;
  audition-only/overlap-only runs exercise their applicable hooks. The offline render
  allocator suite also passed all **11** tests with telemetry and both dependency features,
  including dense IT and concrete articulation (`/tmp/no04-offline-allocation.log`).
- `cargo xtask ci --job no-std-check` and `--job no-std-purity` passed. Facade with
  `--no-default-features --features mod,s3m,mtm,note-overlap` checked on
  `riscv32imc-unknown-none-elf`; adding `interactive-audition` checked on
  `riscv32imac-unknown-none-elf` and `wasm32-unknown-unknown`
  (`/tmp/no04-{bare-overlap,bare-both,wasm-both}.log`). WASM host default feature check
  passed on wasm32 (`/tmp/no04-wasm-host.log`).
- `cargo +1.97 test --offline --manifest-path embedded/Cargo.toml -p
  starplayer-firmware-common --target x86_64-unknown-linux-gnu` passed **96** tests;
  its `cargo +1.97 tree -e features,no-dev` has neither optional flag. Both used the
  prepared shared target (`/tmp/no04-firmware{,-tree}.log`). These are host/default
  feature checks, not firmware flashing or hardware validation.
- Host feature-unification fixture passed after seeding its ignored lock from root:
  `CARGO_TARGET_DIR=/tmp/starplayer-no-target cargo run --offline --manifest-path
  crates/starplayer-host/tests/feature_unification/Cargo.toml` (`/tmp/no04-unification.log`).
  `git diff --check` passed. No browser/UI code changed or owner listening result is claimed.

### Source-backed follow-up and dependent obligations

The existing S3M loader/header exposes the fast-volume-slide flag, but
`S3mProcessor::static_volume_slide`/`set_minor` never reads it. A fresh base20+D10 onset
with the flag set remains20, rather than canonical fast-sliding to21. The named regression
`existing_s3m_fast_slide_flag_gap_does_not_invent_an_onset_volume_change` pins this existing
unsupported case; it is not evidence of canonical fast-flag fidelity. NO-04 preserves
existing effects and captures the base field actually applied. A separate accuracy task
must research/repair the flag and extend onset tests then. This discovery/disposition is
also recorded in the foundation accuracy policy and review log; no golden is changed.

NO-05 must use the split helper stages around XM/IT's native onset/articulation ordering,
wire exact base identity for those formats, preserve native provenance and enforce N/C
under their actual allocation paths. A helper admission failure leaves the foreground
bound for the task's specified ordinary replacement fallback; do not leave an unresolved
incoming identity or reuse a stale slot. NO-06 still owns L admission, faithful rack/native
live mapping/velocity resolution, held-instance note-off/pedal lifecycle and preview scopes.
No real live overlap/audition behavior is implemented here. NO-07 retains cross-format/web
integration, all optional matrices and owner listening acceptance. Full root CI/browser/
hardware acceptance is not replaced by this task's targeted checks.

Coordinator review completed 2026-10-28. Final affected-package regression passed:
`cargo test -p starplayer-engine -p starplayer-mod -p starplayer-s3m -p starplayer-mtm
-p starplayer-host -p starplayer --features note-overlap,interactive-audition`
with two build jobs and incremental compilation disabled
(`/tmp/starplayer-no04-coordinator.log`).
