# NO-06 — Faithful loaded-module audition

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Planned; external task review pending; do not implement until requested |
| Depends on | NO-03 and NO-05 |
| Blocks | NO-W1 |
| Parallel with | None in shared live-input/runtime code |
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

Read current InstrumentRack, SampleInstrument, MappedInstrument, live event queue and
MIDI routing. Reuse NO-02 native runtimes and NO-03 audio clock. Existing mapped sample
input is not sufficient for faithful XM/IT audition; no independent preview engine.

## Deliverables

- Implement real XM/IT loaded-module live instruments using concrete format runtime
  operations; keep format identity and avoid engine→format dependency cycles. The facade
  constructs format-specific instruments and passes existing engine abstractions.
- Preserve mapping/tuning, envelopes, release/fadeout, auto-vibrato, IT filter/native NNA,
  and selected custom policy. Velocity scales base note gain before envelope application
  using existing live-input convention; do not invent broad controller features.
- Allocate live lanes using NO-01 layout after module channels. Give preview input its
  own ownership domain and 16-voice cap, separate from tracker/native tail reservations.
  Oldest-preview stealing is deterministic; format states cover the actual pool slots.
- Track live note instances for key-up/sustain and NNA backgrounds, with original input
  note and generation. Release held instances on program/destination changes; keep
  preview isolated from tracker duplicate scans even for matching instrument/note.
- Preview uses prepared samples and master DSP, no inherited tracker-channel inserts.
  Reference note is 60 using existing convention; mapped instruments use native maps.
  One-shot Preview finishes; looping Preview releases on button-up. Keyboard/MIDI
  note-off uses native release or existing simple-sample ramp, never hangs on stale IDs.
- Implement bounded preview/start/release/stop commands and capability/eligibility query;
  prepare racks off render, retire Arc ownership through existing garbage paths.
- Input cleanup on Stop, Stop preview, focus loss, disconnect, source/instrument/module
  switch. Stop preview must not clear song voices/DSP. Global all-sound-off clears both.
- Before first tracker tick use module initial articulation cadence; after Pause/Stop use
  retained cadence. Native loops without terminal release require explicit Stop preview.

## Research points

- Inspect trait object/context limitations; adapt existing real implementations rather
  than introducing a speculative new abstraction or circular format dependency.
- Audit retriggered key ownership, sustain pedal and MIDI channel/instrument numbering;
  queue overflow must have a recoverable all-notes-off path and diagnostics.
- Prove live random state cannot perturb song runtime randomness, and wide modules do
  not collide with MIDI lanes or overflow VoiceTag representation.

## Verification

- Unit tests for keyboard-equivalent/MIDI note lifecycle, pedal, repeated notes, program
  changes, held-loop release, mapped instruments, feature combinations and overflow.
- Compare isolated live XM/IT articulation to equivalent controlled tracker note fixtures,
  with timing/velocity conventions made explicit; verify pitch and filter envelopes.
- Paused/stopped live input must render without moving row/tick. Audition over playback
  must not steal foreground or alter future tracker random choices.
- Run engine/facade/format tests, goldens, RT allocation guard and six-size determinism.

## Out of scope

M11 instrument extraction/library, synthetic ADSR for legacy loops, all MIDI CCs,
separate renderer, raw-preview DSP switch, native app UI or audio-file export controls.
