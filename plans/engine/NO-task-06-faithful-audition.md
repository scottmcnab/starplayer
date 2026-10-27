# NO-06 — Faithful loaded-module audition

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Reviewed; ready for coordinator handoff; do not implement until requested |
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
- Use NO-01’s fixed interactive live lanes 64–80 for every module width (16 MIDI-identity
  lanes shared by jam/selected input, plus lane 80 for the Preview button). All live input,
  including the Preview lane and live NNA tails, shares the single live domain L (web 16
  active slots plus one retirement slot), separate from tracker/native and custom-tail
  domains. Oldest-live stealing is deterministic; format states cover the actual pool slots.
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

## Interfaces consumed/provided and source scope

Own `engine/src/instrument.rs` rack integration and MIDI-source cadence consumption,
XM/IT live runtime implementations, facade runtime factory, and host rack-builder calls
in `player.rs` (currently `InstrumentRack::for_module` at load_smf/load_module/jam paths).
Consume NO-02 concrete runtimes, NO-03 cadence/scoped clears and NO-01 ownership/layout.

Implement one mutable pool-indexed runtime per rack for all its format programs, with
separate live RNG; immutable program definitions hold sample/maps. Land a rack-level
runtime interface together with both real XM/IT implementations, not a runtime for
every program or RefCell mutation. Do not step format articulation on MidiSource's 1 ms
clock. Test live/playing cadence equality, paused phase rejoin and no song RNG advance
from audition. The 17 lanes share 16 live voice slots including jam and live NNA tails;
all live exhaustion uses oldest-live eviction, not native tracker quota/victim selection.

Own Stop-preview orchestration over NO-03 clear hooks. Register cleanup for Stop/global
all-sound-off, destination/program/module changes and rebuild, including retiring ramps.
Hardware CC120 remains lane-scoped. Interactive seek preserves audition. Test these scopes
with native/custom song tails active, plus held-note generations during rebuild.

Own installation of the permanent live MidiSource/rack in Player's interactive mux slot 1;
consume NO-03 invariant and post-source cadence hook. All 17 fixed live lanes are available
before first Play. Jam/selected/button changes use bounded routing commands, never source
replacement/LoadModule. Implement exact-frame articulation via the hook rather than 1 ms
MidiSource polling. Test first preview while paused, before Play and after natural end.

Use 16 active live slots plus one retirement headroom slot. At domain physical exhaustion,
release the oldest already-retiring live voice before retiring an active victim/admitting
the new note. Test every key press admits at its frame under saturation and immediately
after Stop. The documented pressure fallback may truncate a ramp, never another domain.

## Research points

- Land the decided rack-level mutable interface with both XM/IT implementations; no
  per-program runtime, interior-mutability shortcut or engine-to-format dependency.
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
