# NO-03 — Sequencer-only pause and clear-and-hold Stop

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Integrated on feature/note-overlap-audition; automated NO-03 checks passed; no main landing |
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

## Implementation handoff (2026-10-28, review-ready worktree)

Implementation is in `task/no-03-transport`, based on `3efc758`; keep this task at its
original path until main landing. The worker leaves an uncommitted diff for coordinator
review. No NO-04/05 custom overlap execution, NO-06 native audition/Preview rack, web wire
opcodes, main merge or deployment is included.

### Interfaces and behavior

- `Command::Interactive(InteractiveCommand)` validates session/build and explicit runtime
  preparation. `Player::{pause,resume,clear_and_hold,all_sound_off,interactive,
  submit_interactive}` use the host fade owner. Legacy `Command::Stop` and `Player::stop`
  retain their existing semantics. Overlap-only preparation is distinct from interactive
  transport: identity presence alone never opts a player into the new transport.
- Interactive engines start Stopped with audio/live time running. Tracker deadlines and
  song origin translate on resume, retaining FrameClock's Q32 remainder; held song time
  does not advance. Retained cadence uses exact whole-frame arithmetic, including zero
  whole frames at low rates; the existing engine zero-advance guard remains the owner.
  Initial/no-tracker cadence is configured by `articulation_interval_micros` (20 ms by
  default). Natural tracker exhaustion retains the exhaustion boundary itself.
- Provided `EventSource::{set_transport,dispatch_interactive,post_source_articulation,
  clear_scope,describe_interactive}` hooks forward through SourceMux in slot order.
  Slot 0 performs held tracker control before live events; the post-source hook performs
  live-only articulation once after events. Interactive source publication is coalesced
  into the engine's post-source telemetry; immediate transition snapshots remain allowed.
- `interactive_sources` fixes slot 0 for sequencing and slot 1 for live input. Player
  installs a SilentSource placeholder in slot 1 now; NO-06 replaces it with its prepared
  rack. Interactive `jam` changes routing state without rebuilding song playback;
  `midi_only` is rejected in this mode. Actual button Preview/selected audition remains
  NO-06 work, so the NO-03 regression uses scripted live sources.
- `ChannelTable::{retire_voice,clear_scope,prepare_admission}` retains generation/domain
  ownership for 64-frame mixer retirement while invalidating bindings immediately.
  Clears scan slots linearly without allocation. Admission enforces active quota and the
  domain retirement reserve; only that domain's oldest already-retiring slot may be
  hard-released. Retiring writes/dirty flags cannot revive voices. Scoped source cleanup
  is a provided tracker hook; MOD/MTM/S3M/XM suppress stale effect-trigger bookkeeping,
  while a legitimate future authored delayed onset remains eligible. IT drops retired
  runtime bindings through its generation-safe cleanup. Global clears reset DSP and add
  a 64-frame continuity correction; fresh input never passes through a persistent mute.
- Valid interactive seeks are dispatched while held, validate actual row/order/frame
  bounds before clearing Song, then reset the processor. They preserve live voices and
  transport, rebase the hold baseline to the seek frame and leave a zero pending tick
  remainder. Legacy row seek retains its previous song-origin behavior. The pre-existing
  legacy IT reset/sounding-ownership hazard remains separate follow-up work.
- Host Pause freezes fade gain/elapsed and settlement; Resume continues it. Natural end
  clears into Stopped with a bounded return to unity and an armed rewind for next Play.
  Explicit Stop cancels that rewind/fade. `LiveSummary` reports active/retiring/volume
  peak separately from the unchanged 64 tracker lanes; direct engine telemetry carries
  explicit interactive mode, held progress and logical cursor. Player adds host-owned
  natural completion without treating user Pause/Stop as end events.
- One coherent `PlayerSnapshot` pairs engine cursor, publication source frame, host
  transport/fade and rewind state. Rebuild uses this record only; playing extrapolation
  stops at an unpublished pending boundary. `SequencerCursor` retains RowClock tick,
  repeat/delay budgets, tempo/requested speed, pending jump, arrival, sounding position,
  deadline/remainder, song time, end state and detector scalars. Restore invokes no
  musical callback. Same-rate restoration is exact; rate changes convert whole-plus-
  fractional pending time and fade durations. New loads advance session/build; rebuilds
  retain session and selections, advance build and reject stale envelopes.
- Rebuild deliberately creates fresh format processor working state and clears sound;
  it does not transfer suspended effect-memory objects. Visited loop-detector marks are
  reconstructed from the scanned timeline through saved song time, not copied from an
  arbitrarily modified runtime bitset. Pure Pause/Stop retains the existing processor
  and its effect memories exactly. This reviewed boundary is recorded in the master and
  foundation architecture.
- Interactive SMF rebuild retains a parsed `Smf` on the control thread and restores
  `SmfCursor` (consumed event index, song/pending time, sample rate), plus actual rack
  program/volume/pan/bend/sustain via `MidiControllerState`/`SmfPlaybackState`.
  `SmfSequencer::interactive_at` prepares frames off-thread; `EventFeed::smf_cursor` has
  a provided live-feed no-op. `InstrumentRack::{controller_state,restore_controllers}`
  restores controllers with empty held-note sets, never replaying consumed events.
  `InstrumentRack::for_sequenced_module` uses song lanes 0–15/N; generic racks retain
  legacy48/configured-live64 mapping. This preserves permanent live lanes64–80/L.
  Future SMF custom song tails use C; custom live tails remain L. Song/live scoped clears
  clear only the corresponding rack's held notes/sustain. The existing SMF duration-return
  and caller-owned completion policy remains; no new SMF playlist/seek API is introduced.

### Verification evidence

All Cargo commands use `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`, serially, with the shared
prepared target directory. No corpus/golden regeneration, cargo fmt or historical-source
edits occurred. Tests preserve the existing output-ring/render-ahead contract: commands
apply at engine quantum boundaries and cannot change audio already buffered for the host.

- Full affected format/facade/host suite with both flags passed:
  `cargo test -p starplayer-host -p starplayer-engine -p starplayer-mod -p starplayer-mtm
  -p starplayer-s3m -p starplayer-xm -p starplayer-it -p starplayer-midi -p starplayer
  --features note-overlap,interactive-audition` (`/tmp/no03-final-suite.log`).
- After SMF/cursor additions, host/engine/MIDI/facade suite with both flags passed
  (`/tmp/no03-updated-suite.log`). Host transport regressions: 13 passed; engine scripted
  transport regressions: 8 passed. Callback allocator hooks include clear/held seek,
  tracker rebuild replay and SMF rebuild/resume, watching allocation and deallocation.
- Engine regressions cover exact same-frame source order, continued held audio/control,
  initial and post-Stop live input, natural-exhaustion cadence, 10,000 deadline translations
  for Exact/ST3/IT clocks, bounded saturated retirement, coherent direct telemetry, DSP
  reset continuity and byte-identical PCM/event traces at block sizes 1,3,64,128,4096,8191.
- Host regressions cover MOD effect retrigger suppression, invalid and repeated held seek,
  audition-preserving song seek, pause mid fade, armed rewind/cancellation, overlap-only
  legacy Play/Stop/jam, stale build rejection, and paused/stopped mid-row rebuild at
  44.1/96 kHz with the exact subsequent tick deadline. SMF tests cover both held modes
  and rates, controller retention, cleared old sound, and future onset without replay.
- XM clear tests distinguish stale E91 retrigger from preserved ED2 future onset; IT
  clear tests prevent Q01 from reviving cleared foreground/background runtime state.
- `cargo test -p starplayer-engine -p starplayer-host --no-default-features` passed
  (`/tmp/no03-legacy.log`). Separate `--features note-overlap` and
  `--features interactive-audition` runs passed (`/tmp/no03-overlap-only.log`,
  `/tmp/no03-audition-only.log`). Later feature-off host check also passed.
- `cargo run -p starplayer-offline --bin starplayer-goldens -- --check`: all 8 existing
  hashes passed (`/tmp/no03-goldens.log`), unchanged.
- `cargo test -p starplayer-offline --test render_allocation --features telemetry`:
  11 passed (`/tmp/no03-offline-rt.log`), including dense IT and tracker/MIDI mux.
- Facade with `--no-default-features --features mod,s3m,mtm,xm,it,midi,smf,telemetry,note-overlap,interactive-audition`
  passed `cargo check` for `riscv32imc-unknown-none-elf`,
  `riscv32imac-unknown-none-elf` and `wasm32-unknown-unknown`
  (`/tmp/no03-bare-noatomics.log`, `/tmp/no03-bare-atomics.log`, `/tmp/no03-wasm.log`).
- `cargo clippy -p starplayer-host -p starplayer-engine -p starplayer-midi
  -p starplayer-telemetry -p starplayer --all-targets -- -D warnings` passed both with
  `--no-default-features` and with `--features note-overlap,interactive-audition`
  (`/tmp/no03-legacy-clippy.log`, `/tmp/no03-final-clippy.log`).
- `cargo xtask conformance --offline`: 261 total, 179 pass, 0 unexpected failures,
  16 documented deviations, 66 known failures, 0 gated: exact baseline
  (`/tmp/no03-conformance.log`). The pinned shared corpus was only read.
- Host feature-unification fixture passes with engine capabilities enabled directly and
  host package flags disabled; legacy open/load/play still succeeds. HostEngine, Player
  (before backend negotiation/open), and independent source builders reject missing
  host hooks. Reproduce with `cp Cargo.lock crates/starplayer-host/tests/feature_unification/Cargo.lock`
  then `CARGO_TARGET_DIR=/tmp/starplayer-no-target cargo run --offline --manifest-path
  crates/starplayer-host/tests/feature_unification/Cargo.toml` (the common Cargo env above
  applies). The fixture's generated lock/target are ignored; the lock is seeded from
  workspace versions (`/tmp/no03-unification.log`).
- Telemetry's no_std `interactive-audition` feature gates the heavy tracker/SMF rebuild
  fields; engine forwards it weakly so enabling transport never forces telemetry on.
  Unconditional `TransportState::{sequencer_cursor,smf_state}` getters return None when
  the feature is absent. Both isolated telemetry test configurations passed 16 unit +
  4 integration tests. Measured x86_64 sizes: TransportState 40 / Snapshot 2632 bytes off,
  versus 360 / 2952 on: all 320 optional bytes are absent off
  (`/tmp/no03-size-off.log`, `/tmp/no03-size-on.log`).
- Isolated `cargo check -p starplayer-telemetry -p starplayer-engine
  -p starplayer-host-embedded --no-default-features --target riscv32imc-unknown-none-elf`
  passed (`/tmp/no03-isolated-default.log`). Separate embedded-workspace
  firmware-common check passed on `riscv32imac-unknown-none-elf`
  (`/tmp/no03-firmware.log`). Its one full test transport literal now uses `..IDLE`
  for the added small transport scalars; production firmware behavior is unchanged.
  `cargo test --offline --manifest-path embedded/Cargo.toml -p starplayer-firmware-common
  --target x86_64-unknown-linux-gnu` passed all 96 tests (`/tmp/no03-firmware-tests.log`).
- Final host regression rerun passed 13 interactive tests and 6 callback allocator tests
  (`/tmp/no03-final-host.log`). The scoped MIDI holds regression passed
  (`/tmp/no03-midi-scopes.log`). `git diff --check` passed.
- No web UI/wire code changed; web test rerun is deferred to coordinator integration.
  Owner listening/hardware checks remain acceptance work, not a claimed worker result.

### Coordinator review

Reviewed source ordering and fractional timing, quantum DSP clearing, retirement, seek
validation, independent runtime opt-in, tracker/SMF cursor and fade restoration, scoped
MIDI mapping, generation/shadow state and telemetry exclusion. Resolved draft findings
before acceptance; documented rebuild reconstruction boundaries in master/foundation.
Final `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo test -p starplayer-host
-p starplayer-engine -p starplayer-mod -p starplayer-mtm -p starplayer-s3m
-p starplayer-xm -p starplayer-it -p starplayer-midi -p starplayer
--features note-overlap,interactive-audition` passed (42 test result groups, including
the 13 host and 8 engine interactive regressions). `git diff --check` passed.
Actual prepared live rack and audition remain NO-06; custom overlap remains NO-04/05.
