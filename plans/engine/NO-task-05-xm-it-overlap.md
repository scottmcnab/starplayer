# NO-05 — XM and IT overlap integration

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | In progress on task/no-05-xm-it-overlap; NO-01–04 integrated |
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
