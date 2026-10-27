# NO-06 — Faithful loaded-module audition

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Integrated on feature/note-overlap-audition; task commit fd14c4d; no main landing |
| Depends on | NO-03 and NO-05 |
| Blocks | NO-W1 |
| Parallel with | None in shared live-input/runtime code |
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

Read current InstrumentRack, SampleInstrument, MappedInstrument, live event queue and
MIDI routing. Reuse NO-02 native runtimes and NO-03 audio clock. Existing mapped sample
input is not sufficient for faithful XM/IT audition; no independent preview engine.

## Deliverables

- Implement real XM/IT loaded-module live instruments using concrete format runtime
  operations; keep format identity and avoid engine→format dependency cycles. The facade
  constructs format-specific instruments and passes existing engine abstractions.
- Preserve mapping/tuning, envelopes, release/fadeout, auto-vibrato, IT filter/native NNA,
  and the selected instrument’s checkbox when globally enabled. Duplicate identity includes
  exact base onset gain resolved from event velocity before CC7/separate gains; different
  onset gains overlap. Preserve event velocity before the rack’s current CC7 folding;
  use existing format volume resolution and document/test mapping collisions.
  Velocity scales base note gain before envelope application
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

Live checkbox behavior prevents replacement by a later note, not explicit release.
Key-up and pedal-up locate detached held instances as well as foregrounds and apply native
release/simple-sample ramps. One-shot button preview remains completion-based. Jam uses
each program's own checkbox. Custom live tails remain in L with custom-detachment flags;
global bypass clears them without disabling input or clearing native-created live voices,
and Stop preview clears the audition scope. Test held C→G, detached C key-up, sustain,
same/different velocity, unchanged event velocity with CC7 changes, toggle after an
unchecked/bypassed start and bypass with mixed native/custom live backgrounds.


## Research points

- Land the decided rack-level mutable interface with both XM/IT implementations; no
  per-program runtime, interior-mutability shortcut or engine-to-format dependency.
- Audit retriggered key ownership, sustain pedal and MIDI channel/instrument numbering;
  queue overflow must have a recoverable all-notes-off path and diagnostics.
- Prove live random state cannot perturb song runtime randomness, and wide modules do
  not collide with MIDI lanes or overflow VoiceTag representation.

## Verification

- Unit tests for keyboard-equivalent/MIDI note lifecycle, pedal, repeated notes at equal
  and different velocities, immutable onset identity, global bypass, program
  changes, held-loop release, mapped instruments, feature combinations and overflow.
- Compare isolated live XM/IT articulation to equivalent controlled tracker note fixtures,
  with timing/velocity conventions made explicit; verify pitch and filter envelopes.
- Paused/stopped live input must render without moving row/tick. Audition over playback
  must not steal foreground or alter future tracker random choices.
- Run engine/facade/format tests, goldens, RT allocation guard and six-size determinism.

## Out of scope

M11 instrument extraction/library, synthetic ADSR for legacy loops, all MIDI CCs,
separate renderer, raw-preview DSP switch, native app UI or audio-file export controls.

### NO-06 prepared SMF cadence gate correction

Source review found NO-03 gated the shared cadence/post-source hook solely on
interactive-audition, although an explicitly prepared overlap-only SMF rack also needs
native articulation. The shared cadence and prepared Song rack are available under
either optional feature, activated only by a prepared source. Song uses lanes0–15,
N native quota and C custom tails. Live uses lanes64–80 and L, and remains gated by
interactive-audition together with Pause/held transport, routing and Preview commands.
Unconfigured SMF/racks retain the original1ms sample-only behavior, even under feature
unification; neither-feature builds construct none of this optional machinery. No new
product switch or default behavior is introduced. This bounded gate correction was
approved by the coordinator from the existing master contract.

## Implementation and review handoff (2026-10-28)

The new engine `instrument::live::RackRuntime` lands with both real native adapters:
`XmLiveRuntime` owns `XmArticulationRuntime`; `ItLiveRuntime` owns
`ItArticulationRuntime` and its own random state. The facade prepares one runtime/table
per rack, with explicit `RackScope::Song` and interactive-only `RackScope::Live`.
Legacy `InstrumentRack::for_module`, sample/mapped helpers and unconfigured SMF retain
their original behavior. Host load_module/load_smf and rebuild use the prepared factory
when explicitly configured; interactive jam changes routing without replacing sources.
The permanent live source1 exists before Play and survives paused/stopped/natural-end
states. Source0 prepared SMF keeps song0–15/N, native IT background victims and custom C.

Common admission reuses the authoritative PlaybackState ownership and duplicate
predicate; it does not introduce a provenance table. Song foreground preservation uses
N→C; live custom/native backgrounds stay L. Every L onset uses oldest-active pressure,
with oldest retiring hard reclamation at physical exhaustion, never N/C victims. Exact
one-based sounding instrument tags now use u16, including pending MOD adoption, trace
and legacy telemetry conversion. VoiceSlot stays168 bytes; instrument257 is exercised.

Native onset output is initialized at the event frame, including between ticks. The
same-frame hook skips that onset. Module initial tempo/model/fractional interval seeds
prepared cadence; tracker tempo subsequently remains authoritative. Prepared racks do
not poll/step native state on MidiSource's legacy1ms clock. Pitch cents and held bend
retain current envelope/auto-vibrato modifiers, and prepared PCM rate scaling occurs
exactly once. Public native step/bend/release/write reject retired/stale generations
before touching native state, including IT random auto-vibrato.

Passive capture uses original Note (including cents), input channel, exact instrument
and native pre-CC7 base volume: XM0–64, IT0–256. For example raw velocities32768 and32769
both resolve XM32 / IT128. Distinct rendered gains caused by CC7 do not change identity;
CC7/CC10 preserve the legacy next-note latch semantics. Released native backgrounds keep
identity until terminal release; retiring generations are excluded. Sample eligibility
uses current normal/sustain loops independently of original note identity. Detached
held instances are released by matching key-up/pedal-up even when unchecked/bypassed.

Preview, destination and program controls are bounded and audio dispatch returns fixed
outcomes. Route/program/preview-instrument changes clear all matching old instances,
including held=false one-shots/native backgrounds. Ordinary one-shot pointer-up permits
completion; looping preview releases natively. Stop-preview is L-only, CC120 lane-only,
user Stop/global all-sound-off clear all and DSP, and interactive seek preserves L.
Recoverable queue overflow drops bounded stale input, emits AllNotesOff and exposes a
counter. Module/rebuild ownership uses existing source retirement; rebuild restores
routing shadow and clears held generations. Pure `live_instrument_capability` metadata
distinguishes any mapped playable/one-shot/looping key from reference60 preview
playability/loop status; native all-zero maps never fall back to definition.sample.

### Worker verification evidence

All commands used CARGO_BUILD_JOBS=2 and CARGO_INCREMENTAL=0 (tests also used
RUST_TEST_THREADS=2). No cargo fmt, golden regeneration, historical-source edits,
staging or commits were performed.

- Engine/facade/XM/IT/host test suites passed with neither flag, overlap-only,
  audition-only and both (`/tmp/no06-matrix-{off,overlap,audition,both}.log`). These
  include canonical unconfigured SMF, engine six-size invariance, native format tests,
  host rebuild/transport and existing ownership/retirement tests. The final both-feature suite also passed after the public native
  lifecycle guards (`/tmp/no06-final-suite.log`).
- New facade `live_audition` suite:17 tests pass after final lifecycle guards
  (`/tmp/no06-live-final.log`). It compares actual native live pitch/volume/pan/filter
  against controlled tracker notes with explicit XM live60/native48 conversion; tests
  map selection independently; covers native/simple cents/bend, enhanced rate scaling, native random
  isolation, retired/stale random operations, passive velocity collisions/CC7 latch,
  detached sustain/key release, high IDs, one-shot/loop release, queue overflow recovery,
  mixed custom/native bypass, scoped CC120, same-frame onset, mismatched capacity and
 100 immediate admissions under L pressure with protected song lane63.
- Its absolute event schedule (including beforePlay, paused, Stop, resume and pressure
  immediately afterStop) produces audible, byte-identical PCM at1,3,64,128,4096,8191.
- Prepared SMF tests pass overlap-only and both: actual SMF events capture passive
  identity, create N/C with no L reservation, and follow native initial tempo137 at8000
  and44117Hz across100 render quanta rather than1ms polling (`/tmp/no06-smf-overlap.log`
  and feature matrix). Native release, song pause/rebuild and live isolation also use
  host interactive tests (15 with both flags), including audible Preview after natural
  fade completion without song advancement (`/tmp/no06-end.log`).
- Host callback allocation/deallocation guard includes audible actual XM/IT live runtime,
 80 pressure notes, invalid preview, Stop-preview and Stop:10 tests pass with both flags
  (matrix and `/tmp/no06-rt.log`). No hardware or listening evidence is claimed.
- All eight canonical goldens pass `cargo xtask goldens --check`
  (`/tmp/no06-goldens.log`); no files regenerated.
- Facade checks with xm,it,midi and neither/each/both optional flags pass on
  riscv32imc-unknown-none-elf (no CAS), riscv32imac-unknown-none-elf and
  wasm32-unknown-unknown (`/tmp/no06-portable-*.log`).
- Strict clippy (`--all-targets -- -D warnings`) passes affected engine/facade/XM/IT/host
  packages for overlap-only, audition-only and both (`/tmp/no06-clippy-*.log`).
- Overlap-only excludes preview RackChannel, destination state, Live scope and routing
  branches. On this64-bit host PreparedRack is72 bytes overlap-only versus136 bytes
  with audition, asserted by the feature layout test (`/tmp/no06-size-*.log`). Neither
  feature compiles no prepared-rack module. The existing168-byte VoiceSlot stride
  assertion remains unchanged and passes (`/tmp/no06-stride.log`).

W1 consumes the bounded command enums, explicit factory and pure metadata query; actual
applied identity/generation acknowledgement and browser input/focus/disconnect wiring
remain W1's work. NO07 owns final broad integration; owner listening remains outstanding.
The diff is review-ready but intentionally unstaged/uncommitted for coordinator review.

Coordinator review completed 2026-10-28. Final engine/mixer/all-format/MIDI/facade/host
regression with both flags passed52 test groups (`/tmp/starplayer-no06-coordinator.log`).
Pinned `cargo xtask conformance --offline` passed with the unchanged261/179/0/16/66/0
baseline (`/tmp/starplayer-no06-coordinator-conformance.log`). All builds used two jobs
and no incremental compilation. A stale packed-tag bit-range comment was corrected
to match the reviewed16-bit instrument tag; no behavioral changes followed verification.
