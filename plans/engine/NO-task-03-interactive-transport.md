# NO-03 — Sequencer-only pause and clear-and-hold Stop

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Authorised; pending dependencies |
| Depends on | NO-02 |
| Blocks | NO-06, NO-W1 |
| Parallel with | NO-04 only in isolated worktrees; serialize shared engine integration |
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

Read engine frame/source_frame, source mux ordering, sequencer RowClock, timeline/seek,
ControlClock, telemetry publication and facade/host command interception. Today stopping
freezes mixing and dispatch together. New behavior is explicit interactive transport;
existing hosts using legacy Stop retain their contract.

## Deliverables

- Implement Playing/Paused/Stopped interactive transport. Pause freezes only tracker
  source scheduling/advance and effect progression. Audio clock, live sources, mixing
  and DSP continue. Stop clears every voice/binding/articulation/input hold and insert
  delay while preserving tracker position/effect memories and pending tick remainder.
- Separate tracker-source position from audio-time live deadlines using explicit source
  identity/mapping; do not freeze all SourceMux members or rebase queued live events.
  Stable event tie-break ordering must remain defined across pause/resume commands.
- Preserve integer/fractional tempo timing. Control-only ticks use the last active
  tracker cadence (module initial cadence before first tick), not 1 kHz envelopes.
  Resume after saved sequence remainder; rejoin articulation once at next tracker tick,
  with no double step or catch-up. Preserve tick phase for repeated toggles.
- Dispatch format articulation even without a sequence event; ticking ControlClock's
  counter alone is insufficient. Once stopped, do not advance cleared tracker voices.
- Provide scoped clear hooks and global all-sound-off plumbing; NO-06 owns Stop preview;
  all-sound-off does not change transport. DSP processing still uses whole 128-frame
  quanta; command timing follows existing boundary rules.
- Publish explicit transport state for hosts while preserving compatibility of existing
  playing/end flags. Pause and Stop are never natural end. Update source/context docs.
- Reconcile seek, natural-end policy and rebuild: seek clears voices and keeps selected
  transport mode; natural-end behavior remains host policy, not an accidental pause.

## Interfaces provided and source scope

Own `engine/src/{engine,sequencer,control}.rs`, source scheduling/clock hooks, and
`starplayer-host/src/{player,transport,events,source}.rs`. Consume NO-02's tracker
control-step hook and provide one cadence authority to NO-06: tracker-driven while
Playing, synthesised at retained/initial cadence while held, no duplicate 1 ms live
format step. Use the master's same-frame ordering and resume phase contract.

Translate both pending tick deadline and song_origin across held time without calling
FrameClock reset on resume. Freeze tracker and SMF sequenced sources, not live feeds;
keep live timestamps monotonic. Expose held song progress. Implement HostCommand pause/
clear-and-hold through Player, freezing fade elapsed time, fade settlement and end arming
on Pause without ramping output to zero. Interactive Stop cancels fade/rewind state and
leaves new live audio audible. Preserve legacy host paths.

Provide scoped ramped-clear hooks with immediate logical invalidation and bounded retiring
slots. NO-04/05/06 attach domain cleanup; NO-06 alone owns Stop-preview orchestration.
Global all-sound-off is distinct from hardware lane-scoped CC120. Interactive valid seek
clears song state before reset, preserves audition and selected transport, and dispatches
while paused/stopped. Do not change legacy seek here; record its pre-existing IT hazard.

Own Player session/build identity and shadow-state/rebuild hooks. NO-04/05/06/W1 supply
and replay their settings. Preserve pause through source replacement, jam toggles and
mixer rebuild; clear held input when voices are rebuilt. Publish held-state telemetry
and define explicit state/natural-end flags with W1. Tests must cover pause mid end-fade,
song progress/fractional remainder, SMF vs live sources, seek with audition active,
source/jam replacement and rebuild/sample-rate conversion. No synthetic end event.

## Follow-up integration requirements

- Cadence deadlines participate in engine event splitting. Add the provided post-source
  articulation hook to EventSource, forward through SourceMux in slot order, and dispatch
  once at the exact cadence frame after due source events. Held song control steps run
  in slot 0’s own dispatch, before live events; the post-source hook is for live articulation
  only. NO-06 consumes it; test with
  a scripted source now, including deadlines between 1 ms live housekeeping ticks.
- Define the interactive Player invariant: slot 0 sequenced playback, slot 1 permanently
  prepared live source. NO-06 wires the rack; routing changes cannot reconstruct playback.
  Test first-preview enablement in Pause preserves effects, voices and pending remainder.
- Interactive initial Stopped has unity gain and a running audio/live path. Natural end
  clears into interactive Stopped, restores unity and arms rewind for next Play; explicit
  user Stop cancels rewind. Resume-from-Pause cannot use Play's legacy fade-restart path.
- Held seek never dispatches a tick: rebase hold origin to the seek frame, zero pending
  held remainder, set frozen target cursor; test Paused/Stopped seek→resume timing.
- Clear/retirement hooks use N+C+L+R accounting: retiring slots retain ownership but not
  active quota; when domain physical capacity is full hard-release only its oldest
  already-retiring slot before admitting a new note. Test immediate input after Stop and
  saturated consecutive admissions without deferred note timing. Legacy constructors
  and native allocation remain unchanged without runtime opt-in.

## Research points

- Trace all paths that intercept Play/Stop/seek before Engine::apply_command; attach
  new commands at the correct owner, not only the fallback command match.
- Inspect all tempo models for fractional tick intervals and tempo changes. Tests must
  include pause before first tick, exactly on a tick, mid-tick and at natural end.
- Determine existing render-ahead/output-ring latency contract; preserve it rather than
  claiming already buffered audio can be retroactively stopped.

## Verification

- Frame-scheduled tests verify row/tick unchanged during pause, continued sample and
  envelope/DSP evolution, live input delivery, and correct next tick after resume.
- Check Stop clears tails and holds position; Play never resurrects killed foregrounds.
- Compare uninterrupted canonical renders and every host block size 1,3,64,128,4096,8191;
  repeat with identical pause/input schedules, fractional tempos and rapid toggles.
- `cargo test -p starplayer-engine`, format tests, facade tests, goldens and RT hooks.
- Feature-off/overlap-only builds preserve legacy transport semantics.

## Out of scope

No automatic rewind on user Pause/Stop (natural-end armed rewind is in scope),
silent tracker catch-up, seek reconstruction of cleared sound,
new tempo model, sub-quantum DSP or separate preview renderer.
