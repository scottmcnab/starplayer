# NO-W1 — Web transport, Instruments panel and command bridge

| Field | Value |
|---|---|
| Feature | [NO master plan](../engine/NO-master-plan.md) |
| Status | Instrument-only revision reviewed; staged/uncommitted for owner review; do not implement until requested |
| Depends on | NO-06 (and all its dependencies) |
| Blocks | NO-07 |
| Parallel with | None touching shared web transport/command state |
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

Read web app.js, index.html, style.css, ring.js, worklet-processor.js, playback-controller,
media-session, load/rebuild paths and host-wasm exports. Engine APIs exist from NO-01–06.
Do not build a site with external hosting tools; this is the existing repository app.

## Deliverables

- Add Play / pause-icon Pause / Stop in order, pressed/accessibility state, Resume action
  and explicit transport status. Stop invokes clear-and-hold, not rewind. Map media Pause
  to sequencer pause and Stop to clear-and-hold; preserve playlist end detection.
- Instruments: Preview, select, **Let notes ring** checkbox and accessible loop/mixed-map
  badges. Disable overlap for wholly ineligible instruments, not mixed maps. No dropdown,
  channel controls, inheritance or custom Cut/Continue options. Unchecked means native
  behavior, including authored IT NNA. Use master-plan help for onset-volume matching.
- Global **Enable note overlap** starts off; remember checkbox selections and permit edits
  while bypassed. Off ramp-clears only custom tails and restores native behavior, without
  disabling audition, Pause or DSP; custom live tails are cleared too. Add Stop preview and global all-sound-off separately.

- Expose selected-instrument keyboard/MIDI destination separately from existing jam mode;
  preserve focus/text-entry rules. Never duplicate events. Handle pointer cancel, key-up,
  window blur, input disconnect and module changes. Show reference note and routing help.
- Add capability/loop metadata and bounded command opcodes/acks to host/WASM/worklet/ring
  and postMessage paths. Keep existing opcode numbers, reject stale module generations,
  validate IDs/booleans and transport enums before RT dispatch. Apply message-task preparation before note events.
- Reapply session overrides after same-load renderer/sample-enhancement rebuilds and keep
  transport state; reset on new load. New load generation rejects delayed prior replies.
- Reuse current master processing and prepared PCM. Explain that audition has no automatic
  tracker-channel inserts and sustained loops can remain audible while paused.
- Update web README/help and test existing seek/playlist/background output transitions.

## Interfaces consumed and wire/source scope

Consume engine capabilities/layout and Player session/build/transport contracts. Own
`apps/starplayer-web/src/lib.rs` metadata exports plus app.js/ring.js/worklet-processor.js,
playback-controller/media-session and host-wasm command/telemetry exports. Loop/mixed-map
badges come from the metadata WASM instance, correlated with installed session/build;
worklet acks/capabilities report actual audio-side state.

Use a negotiated versioned atomic command record carrying build generation plus existing
opcode/argument/extra; preserve old opcode values and do not use a separate generation
prefix. postMessage validates the same logical envelope. Reject incompatible interactive
ring versions and disable controls gracefully. User load/reload creates a new session;
sample enhancement/renderer rebuild creates only a new build generation and replays the
Player/UI shadow state before input is re-enabled. Reject stale acknowledgements too.

Keep 64 tracker telemetry lanes plus a live summary. `is_playing` is false while paused/
stopped; explicit transport and natural-end fields replace the falling-edge playlist
heuristic (`endReachedStopped`), including telemetry header word 15 consumers. Update
hardcoded MIDI-base UI consumers to the fixed interactive 64–80 layout. Test both wire paths,
old-version rejection, paused live meters, 64-channel audition and same-session rebuild.

Enable interactive runtime settings explicitly; feature compilation alone must not change
legacy host defaults. Add first-preview tests before Play, in Pause and after natural end,
and prove destination changes do not rebuild the tracker source or clear song tails.

## Research points

- Find all uses of boolean playing and stop edge detection (including telemetry refresh,
  Media Session, playlists, background output and renderer rebuild) before introducing
  paused state. Do not fabricate end events or auto-advance from user Pause/Stop.
- Inspect command-ring versioning and fallback batching ordering; acknowledgements must
  reflect actual applied state, not optimistic controls after a rejected command.
- Inspect browser test runner/scripts in repo; extend current harness rather than adding
  a second browser test stack. Test compile-time missing capabilities gracefully.

## Verification

- Run existing web JS/unit tests and host-wasm Rust tests, plus `cargo xtask wasm`.
- Automated controls/bridge tests for instrument checkbox/global enable, no channel controls,
  native IT defaults, remembered bypass selections, loop/mixed metadata, generations,
  state rebuild/reset, input cleanup, shared-ring and fallback ordering.
- Browser smoke: audition over song and paused/stopped, keyboard/MIDI selected target,
  rapid pause/resume, Stop holding position, channel mute/tails and instant bypass.
- Playlist/media controls never interpret Pause as track end; repeated loads/rebuilds
  leave no stuck keys or stale checkbox/global state. Verify keyboard navigation/screen-reader labels.
- Report manual MIDI/hardware checks honestly when no device is available.

## Out of scope

Persistent presets, native/TUI interface changes, visual redesign, external website
publishing, new sample processing, and implementing engine behavior in JavaScript.
