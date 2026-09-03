# M3 — D1: Song timeline, loop detection, seek-to-time and render-to-end

| Field | Value |
|---|---|
| Milestone | M3 (native surfaces / offline) — pulled forward for the web player's progress slider |
| Status | Landed 2026-09-03; owner listening check via W2 outstanding |
| Depends on | M2 complete (MOD/S3M/MTM processors, quirks, fixed-point mixer) |
| Blocks | W2 (web progress slider), M3 `render` command length defaults |
| Recommended model | Claude Opus (touches the sequencer and the RT path) |
| Verified by | agent (`cargo test --workspace`, `cargo xtask ci`), then owner listening check via W2 |

## Superseded in part by D2 (2026-09-03)

One decision below has been reversed. D1 said to "keep the `EndOfSongPolicy::Loop` wrap
exactly as it is — the detector's `Wrapped` visit is what turns it into the loop point",
so a song whose order list simply ran out was reported as `EndReason::Looped` and, with
Repeat off, played five seconds of a second pass under a fade. The owner tested
`NICETUNE.S3M` and decided otherwise: **running out of order list is the end of the song,
not a loop.** Task `M3-task-D2-natural-end-versus-loop.md` adds `Visit::Wrapped` and
`EndReason::Ended`, stops such a song on its end frame under `AtEnd::FadeOut` as well as
`AtEnd::Stop`, and maps it to `SongEnd::Stops` so the page adds no fade to the displayed
length. `end_frame` itself is unchanged — it is still the frame of the restart row's first
tick — so every length D1 measured is the length D2 measures. Everything else in this file
still stands; only `EndReason::Looped` now means a `Bxx`/`Cxx`/`Dxx` loop.

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there (sample-exact ticks, whole-quantum DSP, buffer-size-independent
output, no allocation/locks/panics in `render()`, format identity) are non-negotiable.

Nothing in the repo knows how long a song is or when it ends. Modules do not declare a
length: order lists loop forever, `Bxx` can jump back mid-song, `E6x`/`SBx` pattern loops
revisit rows, and `Txx`/`Axx` change tick length mid-song. The owner wants:

1. a **total track time** and an **elapsed** position, so the web player (task W2) can show a
   media-player progress slider with `m:ss` / `m:ss` and seek by dragging it;
2. an **end-of-song policy** so the player can either wrap at the loop point (Repeat on) or
   fade out over a few seconds and stop (Repeat off);
3. an **offline render length**: play once, then fade over 10 s into the second pass, with
   repeat count and fade length as parameters; songs that end naturally get no fade.

### How other players do it (prior art — replicate the idea, not the code)

- OpenMPT `CSoundFile::GetLength` walks the order list running a subset of the real player
  with a `RowVisitor` set of visited (order,row) pairs; the song ends at the first revisited
  row. Rows reached while an `E6x` pattern loop is active are exempt; a complexity counter
  bails out on malicious loops. libopenmpt then exposes `set_repeat_count` and
  `play.at_end = fadeout | continue | stop`.
- libxmp `scan_module` does the same with `scan_cnt[ord][row]` counters and an
  `inside_loop` flag, and stores per-order `time`/`speed`/`bpm` so seeking to an order
  restores timing.

**Our strategy is the same but stronger:** the scan runs the *real* `PatternSequencer` with
the *real* format processor (no mixing), so the timeline is correct by construction under
every quirk, dialect and tempo model, and the *live* sequencer carries the *same* loop
detector, so "song ended" fires on exactly the scanned frame.

### Code you must read before changing anything

- `crates/starplayer-engine/src/sequencer.rs` — `PatternSequencer` (state machine
  `Ready/Processing/Stopped`, `run_tick`, `commit`, `begin_next_row`, `move_to_order`,
  `resolve_order`, `seek_order`, `seek_row`, `restart_clock_at`, `publish_telemetry`,
  `EventSource` impl), `TrackerProcessor` trait, `TickContext` / `TickOutcome`, `Jump`
  (`within_pattern` marks an `SBx`/`E6x` loop jump), `EndOfSongPolicy`, `SequencerSettings`,
  and the synthetic `PatternData`/processor in its `#[cfg(test)]` module.
- `crates/starplayer-engine/src/flow.rs` — `PatternFlowState` (libxmp `flow.c` port).
- `crates/starplayer-engine/src/source.rs` — `EventSource`, `EngineContext::new`.
- `crates/starplayer-engine/src/engine.rs` — the render loop; note `Command::SeekOrder` /
  `SeekRow` are flagged `unsupported_command` (around line 505) because the source is
  `Box<dyn EventSource>`; hosts route seeks themselves. `Engine::frame()` is the output clock
  (never stops); `Engine::source_frame()` is the musical clock (stops when not playing).
- `crates/starplayer-core/src/event.rs` — `Command` enum. `crates/starplayer-core/src/clock.rs`
  (`FrameClock::advance_tick`, `reset`), `row_clock.rs`, `tempo.rs`.
- `crates/starplayer-telemetry/src/snapshot.rs` (`TransportState`, `Snapshot`) and
  `publisher.rs` (`set_position`, `set_timing`).
- `crates/starplayer-host-wasm/src/lib.rs` — `SeekKind`/`SeekRequest` mailbox,
  `NativeSequencer`, `SeekableModuleSource`, `source_for`, `Host::{load_module_with_options,
  set_mixer_mode, decode, drain_commands, request_seek, process, pack_telemetry}`,
  `TELEMETRY_HEADER_WORDS = 19`, `GainRamp` / `transport_gain` / `pending_engine_stop`.
- `apps/starplayer-web/www/ring.js` — opcodes, `SNAPSHOT_HEADER_WORDS`, `decodeSnapshot`;
  `apps/starplayer-web/test/ring-harness.mjs` (word indices).
- `crates/starplayer-offline/src/lib.rs` — `render_with_kernel`, `golden_source`,
  `trace_loaded` (the only code that plays a module to completion today:
  `EndOfSongPolicy::Stop` + `while next_event_frame().is_some()` + `MAX_CAPTURE_TICKS`).
- Format builders: `sequencer_for` / `sequencer_with_quirks` in
  `crates/starplayer-{mod,s3m,mtm}/src/processor.rs` (all hard-code `restart_order: 0`,
  `EndOfSongPolicy::Loop`).
- Tests that must stay green unchanged: `crates/starplayer-engine/tests/block_size_determinism.rs`
  (the invariant — never weaken), `mixer_determinism.rs`, `sequencer_and_control_plane.rs`,
  `crates/starplayer-mod/tests/render_determinism.rs` (including the "seek then render equals
  a fresh render of the same order" contract from task C3b), `crates/starplayer-s3m/tests/effects.rs`,
  and the goldens (`cargo xtask goldens --check`).
- Docs to amend: `plans/product/01-technical-architecture.md` (sequencer section, `Command`
  list near line 182, §9.2 telemetry header words near lines 757–785),
  `plans/engine/M3-master-plan.md`.

## Deliverables

### 1. `crates/starplayer-engine/src/timeline.rs` (new; `no_std + alloc`)

**`RowArrival`** — `Start | Sequential | NextOrder | Jump | PatternLoop | Wrapped`. The
sequencer records how it reached the current row: `PatternLoop` for a `Jump.within_pattern`
(`SBx`/`E6x`), `Wrapped` for the end-of-list restart in `move_to_order`, `Jump` for
`Bxx`/`Cxx`/`Dxx`, `NextOrder` for falling off the end of a pattern, `Sequential` for the
next row, `Start` after `new` and after any `seek_*`. Expose `PatternSequencer::last_arrival()`.

**`LoopDetector`** — allocated once at construction (never in `render`):
- a bitset over (order,row) with per-order row offsets computed from
  `PatternData::rows_in_pattern` (cap rows per pattern at 256 and orders at
  `order_count`; a position outside the map is treated as unvisited and never marked);
- `inside_loop: bool`, `loop_arrivals: u32`, `armed: bool`;
- `pub fn visit(&mut self, position: SongPosition, arrival: RowArrival) -> Visit` with
  `Visit { New, Repeat, Looped, Budget }`, called by the sequencer once at the **first tick
  of every row, before the tick runs**:
  - `inside_loop = true` on `PatternLoop`; `inside_loop = false` on `Start | NextOrder | Jump | Wrapped`;
    unchanged on `Sequential`;
  - **always mark** the row; **check** only when `!inside_loop`: a row already marked ⇒
    `Looped`. (So `Bxx` to itself, `Wrapped` back to a played order, `Bxx` back to an earlier
    order all fire; `E6x` bodies do not; a `Dxx` out of an `E6x` body lands on the next order
    and is checked normally.)
  - `loop_arrivals` counts `PatternLoop` arrivals since the last non-loop arrival; over
    `MAX_PATTERN_LOOP_ARRIVALS = 4096` ⇒ `Budget` (infinite pattern-loop constructions, stuck
    ProTracker counters);
  - after `Looped` / `Budget` the detector disarms (`armed = false`, every later visit is
    `Repeat`) until `reset`.
- `reset()`; `reset_marking_before(&mut self, timeline: &SongTimeline, frame: u64)` — reset,
  then mark every `RowMark` whose `frame < frame` (bit ops only, no allocation). Used on seek
  so the loop point after a seek is the canonical one.

**`SongTimeline`** — plain data, `Vec`s filled by the scan off the audio thread:
```rust
pub struct RowMark { pub order: u16, pub pattern: u16, pub row: u16, pub frame: u64, pub speed: u8, pub tempo_bpm: u16 }
pub enum EndReason { Looped { target: SongPosition }, Stopped, Budget }
pub struct SongTimeline { sample_rate_hz, marks: Vec<RowMark> /* first visits, ascending frame */,
                          order_marks: Vec<Option<u32>> /* index of the first mark in each order */,
                          end_frame: u64, end: EndReason }
```
with `frame_at(order, row) -> Option<u64>`, `mark_at_frame(frame) -> Option<&RowMark>`
(binary search; the last mark with `frame <= target`), `order_mark(order) -> Option<&RowMark>`,
`loop_length_frames() -> Option<u64>` (`end_frame - frame_at(target)` for `Looped`,
`end_frame` for `Budget`, `None` for `Stopped`), `end_frame()`, `end()`, `sample_rate_hz()`,
`duration_seconds() -> f64` (host convenience; not used in the RT path).
`end_frame` is song-relative (the scan starts at `Frame::ZERO`): the frame on which the first
repeated row would start, i.e. the length of one pass.

**`scan_timeline`**:
```rust
pub struct ScanLimits { pub max_frames: u64, pub max_ticks: u32 }   // Default: 1 h at the sequencer's rate, 1_000_000
pub fn scan_timeline<T, P, D>(sequencer: &mut PatternSequencer<T, P, D>, limits: ScanLimits) -> SongTimeline
```
Drives a **throwaway** sequencer tick by tick (`next_event_frame` → `advance_to` →
`dispatch`) with its own `VoicePool::new(channels)`, `ChannelTable::new(channels)` and
`ControlClock::new(rate, Frame::ZERO)` through `EngineContext::new` — no mixing, no
telemetry. Records a `RowMark` (with the speed and tempo in effect at that row's first tick)
whenever the detector returns `New`; ends with `Looped`/`Budget` when the detector says so,
`Stopped` when `next_event_frame()` becomes `None`, and `Budget` at either limit. The
sequencer must be freshly built (first tick at `Frame::ZERO`, no timeline installed); assert
or document that. Callers build a **second** sequencer for playback — the module is an
`Arc` — rather than reusing the scanned one, so nothing depends on `TrackerProcessor::reset`
restoring every bit of processor state.

### 2. `PatternSequencer` — song clock and end policy

New fields: `detector: LoopDetector`, `timeline: Option<SongTimeline>`, `song_origin: Frame`,
`at_end: AtEnd`, `end_reached: bool`, `last_arrival: RowArrival`.

- **`starplayer-core`**: add `AtEnd { Continue, Stop, FadeOut }` (`Copy`, `Default = Continue`)
  in `event.rs`, plus `Command::SeekFrame(u64)` and `Command::SetAtEnd(AtEnd)`. `Engine`
  flags both `unsupported_command` exactly as it does `SeekOrder` — hosts route them.
- Accessors: `set_timeline(SongTimeline)`, `timeline() -> Option<&SongTimeline>`,
  `set_at_end(AtEnd)`, `at_end()`, `song_frame(now: Frame) -> u64` (`now - song_origin`,
  saturating), `song_length_frames() -> Option<u64>`, `end_reached()`, `last_arrival()`.
- In `dispatch`, before `run_tick`, when `row_clock.is_first_tick_of_row()`: call
  `detector.visit(position, last_arrival)`. On `Looped` or `Budget`:
  - set `end_reached = true` and act on `at_end`:
    - `Continue`: rebase `song_origin = frame - timeline.frame_at(target)` (or `frame` if no
      timeline / no mark), `detector.reset_marking_before(timeline, frame_at(target))`, then
      run the tick normally; `end_reached` clears at the next row's first tick.
    - `Stop`: `state = Stopped`, return without running the tick (voices ring out, as
      `Stopped` already behaves).
    - `FadeOut`: run on normally, **do not** rebase the origin (elapsed runs past the total;
      the UI clamps); `end_reached` stays sticky until the next seek. The host does the fade.
- When `commit` would set `Stopped` because the order list ran out under
  `EndOfSongPolicy::Stop` or because `outcome.stop` fired (`F00`, missing row): that is a
  natural end. Under `at_end == Continue` restart instead (`seek_order(restart_order)` +
  rebase origin to `frame`, detector reset); under `FadeOut` set `end_reached` and stop as
  today (there is nothing to fade into); under `Stop` stop as today. Keep the
  `EndOfSongPolicy::Loop` wrap exactly as it is — the detector's `Wrapped` visit is what
  turns it into the loop point.
- `seek_frame(&mut self, song_frame: u64) -> Option<RowMark>`: needs a timeline;
  `mark = timeline.mark_at_frame(song_frame)`; seek to `(mark.order, mark.row)` (share code
  with `seek_order` + `seek_row`; add a private `seek_order_row`), restore `tempo_bpm` and the
  row clock's speed from the mark, `detector.reset_marking_before(timeline, mark.frame)`,
  `song_origin = <caller's frame>` minus `mark.frame` — since the sequencer does not know
  "now", take it as a parameter: `seek_frame(song_frame, now: Frame)`; the host already
  calls `restart_clock_at(now)` right after. `last_arrival = Start`, `end_reached = false`.
- `seek_order(order)`: unchanged when no timeline is installed. With a timeline: also restore
  the order's mark speed/tempo, `reset_marking_before(mark.frame)`, and rebase the origin —
  add `seek_order_at(order, now: Frame)` for that and keep the old signature delegating with
  no origin rebase, so existing callers/tests are untouched. An order the scan never
  reached (unreachable "hidden" order) seeks fine and sets `song_origin = now`.
- `EndOfSongPolicy` stays (what an order-list end *means*); `AtEnd` is about the detected
  loop point. Say so in both types' docs.
- Nothing above allocates or panics in `dispatch`. `reset_marking_before` is a bounded bit
  loop over `marks` (≤ orders × rows).

### 3. Telemetry

`TransportState` gains `song_frame: u64`, `song_length_frames: u64`,
`song_end: SongEnd { Unknown, Loops, Stops }`, `end_reached: bool`. `TelemetryPublisher` gets
`set_song_clock(song_frame, song_length_frames, song_end, end_reached)`; the sequencer calls
it from `publish_telemetry` every tick (`song_frame` for the tick's frame). Keep `Snapshot`
`Copy`/plain data.

### 4. `crates/starplayer-offline`

- `pub fn song_timeline(module: &Arc<Module>, sample_rate_hz: u32) -> Result<SongTimeline, RenderError>`
  — builds the throwaway sequencer through each format's `sequencer_with_quirks(module, rate,
  QuirkSelection::FromDialect)` (the builder the web host uses) so offline and browser agree.
  Also a `_for(tempo_model)` variant or parameter if the goldens' `ExactFixedPoint` path needs it.
- ```rust
  pub struct RenderLength { pub repeat_count: u32 /* 0 = once */, pub at_end: AtEnd, pub fade_frames: u64, pub max_frames: u64 }
  impl RenderLength { pub fn default_for(sample_rate_hz) -> Self }   // once, FadeOut, 10 s, max 1 h
  ```
- `pub fn render_song<Path, Interp, Out>(format, bytes, sample_rate_hz, host_block_frames, length) -> Result<Vec<Out::Sample>, RenderError>`
  (plus the `i16` mono convenience mirroring `render_fixed_mono`): scan; build a fresh
  sequencer with the timeline installed and `AtEnd::Continue`; total frames =
  `end_frame + repeat_count × loop_length + fade` where for `Stopped` ends the fade is 0 and
  repeats restart from frame 0 (`loop_length = end_frame`), for `Budget` treat the cap as the
  loop point; `at_end == Stop` renders exactly `end_frame + repeat_count × loop_length` (cut);
  clamp to `max_frames`. Render in `host_block_frames` chunks through the same engine path
  as `render_with_kernel`, then apply a **linear fixed-point fade** to the last `fade` frames:
  `gain_q16 = remaining × 65_536 / fade`, `sample = (sample as i64 × gain_q16) >> 16` for
  integer output, and `sample × (gain_q16 as f32 / 65_536.0)` for float output. The last
  frame must be exactly zero. No transcendental functions.
- **Do not touch** `GOLDEN_RENDER_FRAMES`, `render_fixed_mono`, `canonical_sha256`, or the
  committed hashes under `goldens/`.
- Remove nothing from `trace_loaded`; it may optionally use the timeline later.

### 5. `crates/starplayer-host-wasm` + `ring.js` wire

- Opcodes `OPCODE_SEEK_FRAME = 8` (argument = song frame as `u32`) and `OPCODE_AT_END = 9`
  (argument `0` = `FadeOut`, `1` = `Continue`, `2` = `Stop`; `extra` = fade length in frames).
  Mirror in `apps/starplayer-web/www/ring.js` and its `test/ring-harness.mjs`.
- `decode` maps them to `Command::SeekFrame` / `Command::SetAtEnd`; `drain_commands` routes
  both to the mailbox like `SeekOrder`, never to the engine ring.
- `source_for` scans on a throwaway sequencer (`sequencer_with_quirks` twice), installs the
  timeline on the playback one, and returns it; `Host` caches `Arc<SongTimeline>` (or the
  data needed) beside `current_module` so `set_mixer_mode`'s rebuild reuses the scan
  (timeline depends on rate and dialect, not mixer mode). The rebuild should restore the
  sounding **song frame**, not just the order, via `seek_frame`.
- Mailbox: `SeekKind` gains `Frame(u64)`; add a shared `Rc<Cell<AtEnd>>` (plus fade frames)
  read by `SeekableModuleSource` and applied to the sequencer at the next event boundary.
  `NativeSequencer` forwards `seek_frame`, `seek_order_at`, `set_at_end`, `set_timeline`.
- Fade: after `render_native` in `process`, if the snapshot shows `end_reached`, the host's
  at-end mode is `FadeOut`, and no fade is in progress: `transport_gain.glide_to(0,
  fade_frames)` and `pending_engine_stop = true` (the existing ramp-then-`Command::Stop`
  machinery). When that stop is sent, also request `SeekKind::Frame(0)` so the next Play
  starts from the top. Starting the ramp up to one host block late (≤ 1024 frames) is
  acceptable for a multi-second fade — say so in a comment. Setting `Continue` while a fade
  is running glides the gain back to unity and clears `pending_engine_stop`.
- Telemetry header 19 → 22 words appended **after** word 18: `song_frame` (i32, saturating),
  `song_length_frames` (i32, saturating), `song_flags` (bit0 length known, bit1 ends by loop,
  bit2 end reached, bit3 fading). Update `TELEMETRY_HEADER_WORDS`, `pack_telemetry`,
  `ring.js` (`SNAPSHOT_HEADER_WORDS`, `decodeSnapshot` → `songFrame`, `songLengthFrames`,
  `songFlags`), `test/ring-harness.mjs` indices, and architecture §9.2.
- Leave `apps/starplayer-web/www/app.js` and `index.html` alone apart from what the header
  change forces (nothing should); W2 builds the UI.

### 6. Docs

- `plans/product/01-technical-architecture.md`: sequencer section — loop detector, timeline,
  `AtEnd` vs `EndOfSongPolicy`, seek-to-frame semantics and the stated limitation (seek
  restores speed/tempo only; no global-volume or sample-position sync, unlike OpenMPT's
  `eAdjust`); the `Command` list; §9.2 header words.
- `plans/engine/M3-master-plan.md`: the `render` command inherits
  `RenderLength::default_for(rate)` and exposes `--repeat N`, `--fade S`, `--at-end cut|fade`.
- Module-level doc comment in `timeline.rs` citing the prior art above.

## Research points

- Confirm no format processor keeps a shadow copy of speed/tempo that survives
  `TrackerProcessor::reset` and would override the mark's values on the first tick after a
  seek (MOD's deferred CIA `pending_tempo` is the suspect). `TickContext` feeds `tempo_bpm`
  and `row_clock` in each tick and `TickContext::outcome()` echoes them, so sequencer-owned
  values should win; verify with a test on the MOD processor.
- Pattern-delay repeats (`SEx`): `is_first_tick_of_row()` is true only for the first tick of
  the row, not each repeat — the detector must be consulted once per row, not per repeat.
- `rows_in_pattern` can be `None`/0 for fuzzed data; the bitset must cope, and a scan of a
  module with no playable order returns `Stopped` at frame 0.
- Scan cost: measure ticks per second on `REFLEX.S3M` in a native test and note it; in the
  browser it runs in the worklet's message task (not `process()`), like module decoding.
- `S3M` `Vxx` global volume and other processor state are **not** restored on seek; note the
  limitation in docs and tests rather than trying to fix it here.

## Verification

Engine (`cargo test -p starplayer-engine`), new tests in `timeline.rs` using the synthetic
data/processor already in `sequencer.rs` tests (extend it with a scripted jump/loop/stop
table if needed):
- straight order list → `Looped { target: (0,0) }`, `end_frame == orders × rows × speed × frames_per_tick`;
- `Bxx` back to an earlier order → correct target and `loop_length_frames`;
- `E6x` ×3 counts the body four times with no false end; `Bxx` to its own order ends there;
- `stop` outcome → `Stopped`; a pathological loop → `Budget`;
- `seek_frame` lands on the right row, restores speed/tempo, `song_frame` is continuous
  across the seek, and the loop point after a seek equals the canonical one;
- **live equals scan**: play each golden fixture (`crates/starplayer-offline` corpus) through
  a real `Engine` with the timeline installed and `AtEnd::Stop`, and assert the engine stops
  with `source_frame == timeline.end_frame`; repeat after a `seek_frame` to mid-song;
- `AtEnd::Continue` wraps: `song_frame` drops to `frame_at(target)` on the end tick.

Offline (`cargo test -p starplayer-offline`): `render_song` byte-identical at host block
sizes `1, 3, 64, 128, 4096, 8191` for the MOD, S3M and MTM fixtures; the fade's last frame is
zero; `RenderLength { at_end: Stop, repeat_count: 0 }` yields exactly `end_frame` frames;
`cargo xtask goldens --check` unchanged.

Host: `cargo test -p starplayer-host-wasm` (mailbox tests extended for `SeekFrame`,
`SetAtEnd`, the fade arming); `node apps/starplayer-web/test/ring-harness.mjs`.

Whole tree: `cargo test --workspace`, then `cargo xtask ci` (all jobs: includes the
`no-std` bare-metal check and `wasm-build`). Report the exact commands run and their
results. **Do not commit** — the reviewer commits.

## Out of scope

- The web UI (W2). Subsong discovery (unreachable orders). MOD Noisetracker restart
  position (`restart_order` stays 0). Output-latency compensation. The CLI `render`
  command and WAV writer (M3). An engine-level master fade. Full processor-state sync on
  seek. Changing golden length.
