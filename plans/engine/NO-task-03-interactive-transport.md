# NO-03 — Sequencer-only pause and clear-and-hold Stop

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Planned; external task review pending; do not implement until requested |
| Depends on | NO-02 |
| Blocks | NO-06, NO-W1 |
| Parallel with | NO-04 only in isolated worktrees; serialize shared engine integration |
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
- Implement Stop preview and all-sound-off plumbing with distinct ownership scopes;
  all-sound-off does not change transport. DSP processing still uses whole 128-frame
  quanta; command timing follows existing boundary rules.
- Publish explicit transport state for hosts while preserving compatibility of existing
  playing/end flags. Pause and Stop are never natural end. Update source/context docs.
- Reconcile seek, natural-end policy and rebuild: seek clears voices and keeps selected
  transport mode; natural-end behavior remains host policy, not an accidental pause.

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

No automatic rewind, silent tracker catch-up, seek reconstruction of cleared sound,
new tempo model, sub-quantum DSP or separate preview renderer.
