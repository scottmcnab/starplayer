# NO-W1 — Web transport, Instruments panel and command bridge

| Field | Value |
|---|---|
| Feature | [NO master plan](../engine/NO-master-plan.md) |
| Status | Integrated on feature/note-overlap-audition by reviewed single-change squash; task commit c20041f; NO-07 integration verification complete; owner listening/device acceptance pending |
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


## Implementation and review handoff (2026-10-28)

Base: `b427bac67e70246128a063759460befaefa0efe8`, branch
`task/no-w1-web-controls`, sibling checkout `../starplayer-no-w1-web-controls`.
The implementation remains unstaged and uncommitted for coordinator review. No main
merge, deployment, golden regeneration, historical-source edits or cargo fmt was done.

The existing app now provides Play/Pause/Stop, explicit transport and natural-end
telemetry, instrument Preview/selection/Let notes ring, global overlap bypass,
Stop preview and global all-sound-off. Selected-instrument keyboard/MIDI and jam programs
share the fixed live lanes without duplicate routing. Loop/mixed-map/reference-preview
metadata comes from the inspection WASM. Sample one-shot/looping eligibility covers all
mapped nonempty PCM independently of live-adapter pitch viability; native low-map samples
remain eligible even when the live target cannot sound them. The XM viability preflight is
shared with native onset tuning for live/reference-preview eligibility. Input is
ready-gated, and native release/one-shot completion, focus/pointer/disconnect/destination
cleanup and recoverable outer/native queue pressure retain scoped ownership.

Version2 uses a five-word ring header and seven-word atomic records containing the whole
u64 session/build identity. Fallback uses the same logical envelope. Existing opcode
values remain; opcodes2 (legacy Stop) and7 (mixer replacement) are v1-only, while renderer changes use the replacement-message
path. Incompatible/missing capabilities disable interactive controls; ordinary legacy
fallback commands remain flat triples. Rust validates scalar arguments before RT dispatch.
The fixed10-word applied-status block contains serial, identity, capabilities, acceptance
and rejection baseline; UI state changes only from the matching audio-applied result.
A prepared load is not ready until source installation and full bounded settings replay
have reached audio, including64 mute states and insert parameters.

The off-render, versioned field-wise u64 replacement codec carries the complete NO-03
cursor plus exact integer transport ramp/fade/rewind, overlap selections and live routing.
It validates bounds and command capacity before installation, converts sample-rate durations
and fractional deadlines, and transfers no voices/held keys or processor working state.
Fresh explicit sessions instead install Stopped with unity live gain, clean fade/end/rewind
state and reset overrides/routing before readiness; configured fade duration is retained.
The paired engine Load/Stop reserves two queue entries. Same-session replacements retain
state. A capture is guarded for the entire load-option transaction, retries if user transport
changes during capture, and rolls back on capture failure. Pending loads remember the latest
full-identity transport opcode; deferred installations apply it before replay/readiness, and
the page retries it against the acknowledged build before accepting input. Transport still
has the normal quantum-boundary command latency; no synchronous between-frame guarantee is
introduced. Stop/cancel suppresses a fresh load's automatic Play.

Version2 rejects legacy unversioned midiInput preparation. Insert install/remove messages
validate version, installed full identity, action/target/slot/kind, and available transaction
capacity before mutation. Earlier controls flush first; busy transactions reject before
allocation, pending slot controls are disabled, and replies wait for an applied barrier.
Stale replies cannot update the current UI. Tracker records remain a fixed64-lane wire block,
but visible tracker/scopes counts use the actual module width; live meters stay separate.

### Verification evidence

All Cargo commands use `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`; Rust tests also use
`RUST_TEST_THREADS=2`. Node tests use `--test-concurrency=1`. One Cargo/browser stream
runs at a time. Final evidence below supersedes saved earlier worker logs where named.

- `cargo test -p starplayer-host-wasm`:59 tests pass, including new fresh-session resets
  from Playing, Paused, a real active fade and actual natural end; first Preview remains
  audible, cursor stays held, settings reset, and a4-channel MOD has4 tracker/scope rows.
  An actual64-channel XM keeps64 tracker/scope lanes with a separate audible live summary.
  The same suite covers full-u64 identities, stale/wrong scalar controls, applied barriers,
  queue/retirement pressure, malformed replacement codecs, exact fractional conversion,
  natural-end/fade/repeated-row handoff and cancellation of future live events.
  Log: `/tmp/no-w1-finish-rust.log`.
- `cargo test -p starplayer-host --features note-overlap,interactive-audition`: all8 groups
  pass (27 unit,10 allocation,15 interactive transport,13 live input,6 MIDI-only,
  22 Player,2 timing and1 doctest). This includes exact six-size output checks and the
  RT allocation/deallocation guard. Log: `/tmp/no-w1-finish-host.log`.
- `node --test --test-concurrency=1 apps/starplayer-web/test/*.test.mjs`:333 tests pass
  (`/tmp/no-w1-finish-js.log`). Focused tests cover bypassed instrument selection,
  native IT help, mixed/reference metadata, preview pointer/keyboard cleanup, bounded outer
  queue recovery, rejected-state rollback, full wire identities, explicit natural end,
  media Pause versus Stop, stale metadata acknowledgements, cancelled activation, guarded
  capture/failure rollback and latest Stop/Pause intent.
- `cargo xtask wasm`, `node apps/starplayer-web/test/ring-harness.mjs`, and
  `node apps/starplayer-web/test/worklet-harness.mjs`: all pass. Logs:
  `/tmp/no-w1-finish-wasm.log`, `/tmp/no-w1-finish-ring.log`,
  `/tmp/no-w1-finish-worklet.log`.
  The worklet harness executes actual packaged Rust WASM, including both shared-ring and
  fallback paths, first Preview before Play/paused/stopped/natural-end, full-width serial
  wrap, stale commands, rejected v2 legacy Stop2 without gain/sequence mutation,64-state replay and independent WASM-memory recreation. Added
  regressions cover v2 legacy-midi rejection, malformed/stale prepared inserts, FIFO
  parameter preparation, multi-slot Reset with exact previous-state rollback, audio-applied
  insert acknowledgements and pending Pause/Stop during
  an actually deferred Playing replacement. Restored song_frame equals the captured frame,
  and stale transport intent cannot restart it.
- `STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --interactive-only`:
  the existing real Chromium/CDP runner passes SAB, forced fallback and plain HTTP.
  It covers paused/stopped audible audition, selected keyboard input/key-up/blur,
  accessible Pause/Resume, held position, bypassed checkbox selections, enhancement and
  22050Hz renderer rebuild retention, explicit same-byte session reset and4-row tracker UI.
  Final log: `/tmp/no-w1-finish-browser.log`; screenshot `/tmp/no-w1-controls.png` is
  review evidence only, not a repository asset.
- `cargo test -p starplayer --features midi,note-overlap,interactive-audition --test
  live_audition`:19 tests pass (`/tmp/no-w1-finish-live.log`). Includes native low-map XM
  onset proof, both mixed-map eligibility orientations, native live pitch/envelope/release,
  queue pressure and absolute live schedule output equality at1,3,64,128,4096,8191 frames.
- `cargo clippy -p starplayer-host-wasm -p starplayer-host --all-targets --features
  note-overlap,interactive-audition -- -D warnings` passes.
  Log: `/tmp/no-w1-finish-clippy.log`.
- `cargo check -p starplayer --no-default-features --features
  mod,s3m,mtm,xm,it,midi,note-overlap,interactive-audition --target
  riscv32imc-unknown-none-elf` passes (bare metal, no CAS).
  Log: `/tmp/no-w1-finish-portable.log`.
- `cargo check -p starplayer-host-wasm -p starplayer-web --no-default-features` passes.
  Log: `/tmp/no-w1-finish-feature-off.log`.

Saved prior worker evidence was inspected, not silently treated as a final rerun:
`/tmp/no-w1-js-final.log`325 JS tests, `/tmp/no-w1-rust-final.log` and
`/tmp/no-w1-rust-edge.log`57 host-wasm tests, `/tmp/no-w1-live.log`18 facade live tests,
`/tmp/no-w1-feature-off.log`, packaged WASM/ring/worklet logs and
`/tmp/no-w1-browser-final.log` with all three Chromium modes. They predate the final
fresh-session/lifecycle/width/insert review corrections. NO-07 owns broad CI, goldens,
full feature/target integration and final determinism evidence; none is claimed newly run
here. Physical MIDI/hardware, mobile lock-screen behavior and percussion/piano/XM/dense-IT
listening remain owner acceptance. Automated keyboard and native MIDI-event injection are
not physical MIDI-device evidence. No hardware or musical listening validation is claimed.


### Coordinator review and verification — 2026-10-28

Reviewed the complete35-file change against the master/task contracts and prior review
dispositions, including full identity validation, passive eligibility metadata, exact
cursor/ramp replacement, applied acknowledgements, bounded preparation and scoped input
cleanup. Fresh-session reset, pending transport intent, legacy v2 Stop refusal, prepared
insert ordering/Reset rollback and actual tracker width were resolved before acceptance.
The corrected4-channel screenshot was visually inspected.

Final coordinator command passed with two jobs, no incremental compilation and two test
threads: `cargo test -p starplayer-host -p starplayer-host-wasm -p starplayer-web
-p starplayer-engine -p starplayer --features midi,note-overlap,interactive-audition`.
Log: `/tmp/starplayer-no-w1-coordinator.log`. The worker's final333 JS tests,59 WASM-host
tests,19 live tests, actual-WASM/ring and three-mode Chromium evidence were inspected.
Broad CI/canonical/embedded integration is still NO-07; owner hardware/listening remains
outstanding. This acceptance authorizes the task's single-change squash on the feature
branch only; no main merge or deployment.

Task commit `c20041f` is integrated by single-change squash after the coordinator
regression passed460 tests in30 result groups. Plans remain here pending main landing.
