# NO-05 — XM and IT overlap integration

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Planned; external task review pending; do not implement until requested |
| Depends on | NO-02 and NO-04 |
| Blocks | NO-06, NO-W1 |
| Parallel with | NO-03 after shared runtime interface is landed; coordinate engine changes |
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

Use the landed per-voice runtimes and bounded overlap helper. XM now has the storage
needed for background articulation; IT already has native NNA, duplicate checks and
background-control effects. Extend each format at its real note lifecycle seam.

## Deliverables

- XM detaches eligible voices with their current format state; background articulation
  continues independently of foreground channel effects. Preserve carry, key-off,
  fadeout, auto-vibrato, panning and note maps for actual new notes and delayed starts.
- IT explicit policy substitutes for native NNA/DCT/DCA for the affected candidate;
  Original retains runtime NNA changes. Resolve outgoing survival separately from
  incoming duplicate action. Do not apply native and override duplicate scans twice.
- Mixed policies: apply candidate's effective mode to duplicate scan; Original candidates
  use native matching/action, Continue skips native duplicate checks, Different-notes
  cuts matching raw trigger identity, Cut does not scan background duplicates. Keep
  native background control commands and native eligibility/stealing semantics.
- Resolve eligibility per actual mapped sample, including sustain/normal loops. Ineligible
  custom overlap falls back to native behavior, preserving IT looping NNA. Cut remains
  an explicit option for loops. Sample-mode IT must also support eligible custom tails.
- Preserve original trigger note for duplicate identity even through transpose maps,
  pitch slides or different mapped sample selections. Tag custom-detachment provenance.
- Update trace to expose detached voices without duplicate channel events; ensure
  native and extra pools share safe addressable articulation storage and cleanup.
- Update accuracy-policy implementation notes and doc explanations of IT override effects.

## Research points

- Inspect IT instrument-mode versus sample-mode paths, S7x runtime actions, old effects,
  compatible portamento and duplicate check ordering before altering the trigger seam.
- Validate instrument-only changes and XM carry use the sounding voice identity, not
  merely the newest selected instrument number, when resolving outgoing policy.
- Check mixed-map UI metadata requirements with NO-W1; no all-instrument loop assumption.

## Verification

- Full XM/IT unit tests, pinned conformance comparisons and golden check; no new native
  failure or regenerated golden accepted.
- Cross product of Original/Cut/Continue/Different-notes with native IT NNA/DCT/DCA;
  mixed incoming/outgoing instruments, runtime NNA effects and sample-mode IT.
- XM envelope carry/sustain/key-off/fadeout, background auto-vibrato/filter, mixed maps,
  looping fallback, custom/native bypass cleanup and slots beyond 255.
- Same-channel C→G→C versus cross-channel notes, bounded exhaustion and determinism at
  all six block sizes; allocation guard around actual note/control processing.

## Out of scope

New custom overlap for loops, IT lowering, changing native background commands,
rewriting unrelated format compatibility or implementing an instrument catalogue.
