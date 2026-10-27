# NO-05 — XM and IT overlap integration

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Integrated on feature/note-overlap-audition; task commit b460c9b; no main landing |
| Depends on | NO-02, NO-03 and NO-04 |
| Blocks | NO-06, NO-W1 |
| Parallel with | None during shared runtime/cleanup integration |
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

Use the landed per-voice runtimes and bounded overlap helper. XM now has the storage
needed for background articulation; IT already has native NNA, duplicate checks and
background-control effects. Extend each format at its real note lifecycle seam.

## Deliverables

- XM detaches eligible voices with their current format state; background articulation
  continues independently of foreground channel effects. Preserve carry, key-off,
  fadeout, auto-vibrato, panning and note maps for actual new notes and delayed starts.
- For IT, checked/eligible instruments under global enable substitute continuation and
  the instrument/note/onset-volume/origin-channel duplicate rule for native NNA/DCT/DCA.
  Unchecked/bypassed/ineligible candidates keep native actions and runtime NNA changes.
  Resolve outgoing survival separately from incoming duplicate identity; never apply
  native and custom duplicate scans twice to a candidate. Preserve native background
  commands, eligibility and stealing semantics.
- Check actual mapped sample loop eligibility. Ineligible notes keep native behavior,
  including IT looping NNA; there is no custom Cut option. Support sample-mode IT too.
- Finalize base onset volume after all channel processing on its actual start tick,
  including fine slides. IT uses `note_volume`; name the XM base field in runtime docs.
  Exclude tremolo/tremor/envelopes/random/separate gains. Keep native IT NNA/DCT at its
  current early seam; preserve/detach custom candidates without native duplicate action,
  then scan after onset effects and before mixing at that frame, skipping the new VoiceId.
  Test IT same-row vxx and DxF, XM and IT volume-column fine slides, XM EAx/EBx on
  the onset tick, XM EDx and IT SDx delayed notes carrying volume-column values, and
  capacity pressure. Never scan with stale volume or re-run native effects.

- Preserve original trigger note for duplicate identity even through transpose maps,
  pitch slides or different mapped sample selections. Tag custom-detachment provenance.
- Update trace to expose detached voices without duplicate channel events; ensure
  native and extra pools share safe addressable articulation storage and cleanup.
- Update accuracy-policy implementation notes and doc explanations of IT override effects.

## Interfaces consumed and cleanup integration

Consume NO-02 concrete runtimes, NO-04 helper and NO-01 single ownership/provenance record.
Own XM/IT note lifecycles, quota/victim filters and per-origin-channel background stepping.
Exclude custom-provenance tails from native IT quota and victim selection; test a saturated
256-native + 64-custom + 16-live active pool with retirement headroom; keep native-domain voices (all tracker foregrounds and native-created backgrounds,
regardless of current checkbox) protected from other domains.
Custom cap can reduce polyphony relative to authored NNA; test/document that consequence.

Preserve foreground XM ordering. Step custom backgrounds after their origin channel's
foreground processing, before the next channel, once per tick (including detach ticks).
Test same-row Gxx, detached voices and global-volume changes on different channels.
Use scoped cleanup hooks for Stop/global all-sound-off/interactive seek/bypass, removing
native and custom ownership correctly without affecting unrelated live voices. Tests
cover active native/custom backgrounds, retiring ramps, slot reuse and bypass provenance.

Test retirement ownership independently of active quota and native victim eligibility;
custom saturation admits notes using the bounded retired-slot fallback, while native (unchecked)
IT allocation remains unchanged. Legacy seek/restart retains ownership for any
still-sounding custom tails rather than resetting their budget/age records.

Candidate decisions use current global/checkbox state and the candidate's actual sample;
provenance alone controls cleanup/budget, never whether its duplicate rule is native.
Test native background→checkbox on→custom matching→bypass preserves that native-created
background; custom tail→checkbox off→native DCT/DCA; looping incoming mapped note with
DCT=Instrument leaves eligible one-shot tails of different notes alone. Capture identity
for every new voice, including unchecked/bypassed/ineligible starts. Test subsequent
repeat after enable. Include the three from-start canonical configurations in the master,
with passive recording active and native IT looping NNA intact.


## Research points

- Inspect IT instrument-mode versus sample-mode paths, S7x runtime actions, old effects,
  compatible portamento and duplicate check ordering before altering the trigger seam.
- Validate instrument-only changes and XM carry use the sounding voice identity, not
  merely the newest selected instrument number, when resolving outgoing policy.
- Check mixed-map UI metadata requirements with NO-W1; no all-instrument loop assumption.

## Verification

- Full XM/IT unit tests, pinned conformance comparisons and golden check; no new native
  failure or regenerated golden accepted.
- Cross product of checkbox checked/unchecked, global enabled/bypassed and loop eligibility
  with native IT NNA/DCT/DCA;
  mixed incoming/outgoing instruments, runtime NNA effects and sample-mode IT.
- XM envelope carry/sustain/key-off/fadeout, background auto-vibrato/filter, mixed maps,
  looping fallback, custom/native bypass cleanup and slots beyond 255.
- Same-volume C→G→C and same-note echoes at different onset volumes versus cross-channel notes, bounded exhaustion and determinism at
  all six block sizes; allocation guard around actual note/control processing.

## Out of scope

New custom overlap for loops, IT lowering, changing native background commands,
rewriting unrelated format compatibility or implementing an instrument catalogue.

## NO-05 implementation handoff — 2026-10-28

Review-ready, unstaged diff on `task/no-05-xm-it-overlap`, worktree
`../starplayer-no-05-xm-it-overlap`, base `3211810`. No commit, main merge, deployment,
historical edit, cargo fmt, golden regeneration or unrelated staging occurred. Keep this
plan at its current path pending coordinator acceptance and eventual main landing.

### Implemented seams and dependent contracts

- XM classifies explicit musical notes at `trigger_note`, preserving original input
  before transpose and carrying delayed starts to their real flush. The exact onset base
  is `XmChannel::real_volume` (0–64), finalized after volume setters/fine slides, before
  any mixing. Effect retriggers inherit immutable identity without custom detachment.
  All actual starts record passively, including bypassed, unchecked and looping starts.
- XM uses the shared bounded post-effects trigger helper. Its slot snapshot retains the
  outgoing articulation independently of incoming channel effects. Each origin advances
  custom backgrounds after foreground effects/articulation/flush and before the next
  channel, once on detach ticks too. Same-row Gxx order is explicit in a test. Only
  authoritative C backgrounds step here; native unbound stop ramps are excluded.
  Detached traces use `report_trace_voice`, without extra channel events.
- IT keeps native DCT/NNA early. Native candidates keep their authored actions; current
  eligible checked candidates skip native duplicate actions and preserve the outgoing
  foreground through N→C admission. `pending_onset` holds the new generational handle
  until that channel's effects finish, then captures `note_volume` (0–256) and runs the
  all-match scan, skipping the incoming generation. The shared ownership record remains
  the only domain/provenance/age/identity authority. Retirement is synchronized before
  mixing; S7 background commands also reject same-tick engine-retiring generations.
- Actual incoming map/offset viability gates custom preservation without changing native
  early DCT inputs. Pending instrument selection is resolved for that preflight. A custom
  note-only detach retains its pre-detach instrument resolution without replaying defaults.
  Sample-mode IT works too. Native IT Oxx past-end is ignored (Old Effects clamps to the
  final frame), so those remain real onsets; XM's native invalid 9xx stop makes no tail.
- Current sounding tag/sample eligibility and checkbox decide policy, independently of
  immutable matching identity and creation provenance. Native-created backgrounds stay N
  after checkbox changes and survive custom bypass; unchecked C tails receive native DCT.
  Normal/sustain loops remain native, including IT NNA. Original notes survive transpose.
- Native IT quota/victims exclude C/L/retiring ownership; native policy over eligible N
  slots is unchanged. An authoritative N foreground replacement remains net-zero even
  after a legacy reset discarded its runtime snapshot. Custom eviction and retiring-slot
  fallback use NO-03/04 helpers; no alternate ownership table or allocation was added.
- Context-free `reset` and public sequencer seeks reset channels immediately and defer
  slot cleanup until the next row/repeat/tick/control callback has engine context. That
  callback retains only valid C articulation, discarding native/stale snapshots before
  stepping them. Public seek tests retain custom generations and later duplicate matching;
  envelope tests show C continues while native reset snapshots do not advance. Scoped
  retirement suppresses further writes and leaves ownership until physical completion.
- `PlaybackState::inherit_onset` is now public for IT native sample-replacement transfers;
  its capability/generation/identity validation is unchanged. NO-06 can consume it too.
  The shared core identity documentation now names XM `real_volume` explicitly.

### Verification evidence

All Cargo streams were serialized, with `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0` and
`RUST_TEST_THREADS=2` for tests. Shared target/corpus caches were retained; no canonical
expectation or determinism assertion was weakened.

- Full affected regression passed:
  `cargo test -p starplayer-xm -p starplayer-it -p starplayer-engine -p starplayer
  -p starplayer-host --features note-overlap,interactive-audition`
  (`/tmp/no05-full-tests.log`). Final XM/IT/facade suite reran after refinements
  (`/tmp/no05-final-formats.log`); final retirement/row-repeat guards have the IT/facade
  rerun in `/tmp/no05-retirement-final.log`.
- `crates/starplayer/tests/xm_it_overlap.rs` has 24 real-processor tests with both flags
  (the overlap-only run passed 22 tests before the final offset regression was added). The tests cover exact C→G→C/echo/channel identities; passive
  enable-then-repeat and ineligible capture; original/transposed notes; XM EAx/EBx and
  both volume-column fine directions; IT vxx/DxF and volume-column fine directions;
  non-clamped inherited fine-up; EDx/SDx with volume and note-only inheritance;
  instrument-only/retrigger/portamento/no-start behavior; pending maps; sample-mode IT;
  native/current-candidate versus immutable provenance; released identity; mixed normal/
  sustain loops; envelope sustain, carry, fadeout, panning, auto-vibrato and IT filters;
  exact once-per-origin stepping/Gxx order; public context-free seeks and control reset.
- The IT action table covers all 4 NNA × 4 DCT × 3 DCA × 2 checkbox × 2 global × 2 actual
  loop states. Disabled/ineligible configured paths compare voice state against a truly
  unconfigured native table, while checked eligible candidates assert C continuation.
  S74 runtime NNA and S70 background control are tested over both N and C backgrounds.
- Full pressure fixture fills **256 N + 64 C + 16 L** with 339 prepared slots. It checks
  domain protection, retirement fallback/reuse, exact incoming identity, and a non-clamped
  base20+D3F→92 repeated onset retiring the intended old match while the new generation
  survives. Native stealing excludes quiet C/L and reset-at-quota retains net-zero own
  foreground replacement. L here is scripted pool input; faithful live format behavior
  is still NO-06, not claimed by this fixture.
- Enabled synthetic XM/IT PCM is byte-identical at **1, 3, 64, 128, 4096, 8191** host
  frames, and differs from native PCM. Three independently fresh configured renders at
  every size match canonical native PCM: enabled/all unchecked; bypassed/all checked;
  enabled/only wholly looping checked, including authored IT looping NNA.
- Independent XM/IT/engine `--no-default-features` suites passed with neither optional
  flag, overlap alone and audition alone (`/tmp/no05-feature-*.log`). Real seam fixtures
  also passed facade `--no-default-features --features xm,it,note-overlap`
  (`/tmp/no05-overlap-only.log`). Trace-enabled seam suite passed
  (`/tmp/no05-trace-tests.log`); trace is a diagnostic, not an allocator-proof build.
- Existing callback allocator hooks now include actual enabled XM/IT note processing,
  articulation, custom eviction, duplicate scanning and bypass retirement. They count
  deallocations too. All 9 hooks passed with both flags (`/tmp/no05-final-callback.log`);
  all 8 applicable hooks passed overlap-only (`/tmp/no05-callback.log`).
- `cargo xtask conformance --offline` matches the exact pinned baseline:
  **261 total, 179 pass, 0 unexpected failures, 16 deviations, 66 known failures, 0 gated**
  (`/tmp/no05-conformance.log`). `cargo xtask goldens --check` passed all 8 unchanged hashes
  (`/tmp/no05-goldens.log`).
- `cargo xtask ci --job no-std-check` and `--job no-std-purity` passed. Facade xm,it,overlap
  checked on `riscv32imc-unknown-none-elf`; both flags checked on
  `riscv32imac-unknown-none-elf` and `wasm32-unknown-unknown`
  (`/tmp/no05-{bare-overlap,bare-both,wasm-both}.log`).
- Affected all-target Clippy with both flags and `-D warnings` passed
  (`/tmp/no05-clippy-final.log`); `git diff --check` passed. No browser/UI, hardware,
  firmware flashing or listening acceptance is claimed. Full NO-07 integration remains
  separate; this is the task's targeted verification, not a claim to full root CI.

### Confirmed pre-existing native follow-up

The real note-only fixture confirms an existing instrument-mode IT gap: authored NNA
Continue detaches the foreground, then missing-instrument resolution through that empty
foreground maps instrument zero, so the incoming note is lost. The separate named
`preexisting_native_it_note_only_after_nna_loses_foreground_mapping` regression pins that
baseline; NO-05 preserves resolution for its newly introduced custom detach without
silently changing canonical authored NNA. The foundation accuracy policy records this
cause separately from G6-IT-002's broader known voice-set issues. No conformance exclusion
was reclassified and no golden changed. Native accuracy repair needs its own task.

NO-06 still owns faithful live rack/velocity/held-instance behavior; NO-W1 owns UI/wire;
NO-07 owns broad integration and owner listening. Neither this diff nor these tests imply
those deliverables have landed.


### Final coordinator review refinement — retained-offset identity

Restricted IT identity inheritance to non-musical native replacements/`keep_position`.
A genuine new musical note with an invalid initial offset receives no inherited identity,
matching the legacy helper's distinction between no onset and an attempted invalid onset.
The real-format regression starts a 1024-frame sample, uses a same-instrument-only O01 row
to retain offset 256 without triggering, then selects a 64-frame sample on a new note.
It failed on the inherited identity before the fix (`/tmp/no05-offset-before.log`) and
passes afterward; tags/parameters/positions also match a fully unconfigured native table.
The full 24 seam tests passed (`/tmp/no05-offset-final.log`), and affected all-target Clippy
with both flags and `-D warnings` passed (`/tmp/no05-offset-clippy.log`). No native PCM
processing, canonical golden, ownership lifecycle or other task scope changed.

Coordinator review completed 2026-10-28. Final affected-package regression passed:
`cargo test -p starplayer-engine -p starplayer-xm -p starplayer-it -p starplayer-host
-p starplayer --features note-overlap,interactive-audition` with two build jobs and
incremental compilation disabled (`/tmp/starplayer-no05-coordinator.log`).
