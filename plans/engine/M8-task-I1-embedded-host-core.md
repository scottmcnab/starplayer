# M8 — I1: The embedded host core — `starplayer-host-embedded`

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md), decisions 1, 2 and 6) |
| Status | Planned 2026-09-11; pulled |
| Depends on | M3 (landed): `starplayer-host`'s `RenderState` cadence, `starplayer-telemetry`; M7 (landed): interpolators |
| Blocks | I3, I4 |
| Parallel with | I2 |
| Recommended model | Claude Opus (a second host over the engine whose output must match the first byte for byte, on a `no_std` crate that CI checks bare-metal) |
| Verified by | agent (host tests: block-size determinism through the new player, byte equality with the std `Player`, command latency; `cargo xtask ci --job no-std-check` and `--job no-std-purity` with the crate added) |

## Context for a fresh agent

Every host so far — cpal, the AudioWorklet, `ManualBackend` — drives the engine through
`starplayer_host::Player` (`crates/starplayer-host/src/player.rs`), which is a `std`
crate whose backend contract is `RenderCallback = Box<dyn FnMut(&mut [f32]) + Send>`.
On the fixed path `HostEngine::render_chunk` (`crates/starplayer-host/src/engine.rs`)
still converts the engine's `i16` output to `f32` for that callback. An ESP32 wants
neither `std` nor a float output stage: it wants
`Engine::<FixedPath, Linear, FixedOut<i16, 2>, Arc<Module>>::render(&mut [i16])`
(`crates/starplayer-engine/src/engine.rs:509`) called directly from an I2S DMA refill.

What the std host adds on top of `Engine::render`, and what this crate must reproduce, is
the **control cadence** in `RenderState::render` (`player.rs:190`): commands are drained
and end-of-song is armed only at `RENDER_QUANTUM` boundaries **of frames emitted**, not
of device blocks, so a seek or a stop lands on the same frame whatever the block size; the
transport is settled after each whole quantum; retired `Arc<Module>`s are moved out of the
engine's garbage channel into a ring the control side drops from; telemetry is published
once per host block. That cadence is what makes the block-size invariant (design goal 3)
hold through a host, and `crates/starplayer-host/tests/player.rs` proves it for `Player`.
This task ports it — it does not refactor `starplayer-host` (master-plan "Out of scope").

The crate is `no_std` + `alloc`, `#![forbid(unsafe_code)]`, and joins the bare-metal CI
matrix. Nothing in it may name a board, a HAL or an async runtime: the firmware (I3) owns
the DMA callback and the Embassy tasks and calls into this crate from them.

### Code you must read before changing anything

- `crates/starplayer-host/src/player.rs` — the module doc's thread-boundary table, the
  constants `HOST_COMMAND_CAPACITY`, `RETIRED_CAPACITY`, `TELEMETRY_DEPTH`,
  `CONTROL_CADENCE_FRAMES`; `RenderState::render`, `drain_commands`, `apply`,
  `arm_end_of_song`, `settle_transport`, `retire_engine_garbage`, `publish`;
  `Player::collect_garbage` and `Drop for Player`.
- `crates/starplayer-host/src/{transport,source}.rs` — `Transport` (fade, `TRANSPORT_RAMP_FRAMES`),
  `SeekMailbox`, `AtEndSlot`, `build_source`, `scan_module`. These are `std`-free in
  shape; check what they actually pull in and lift what you can (research point 1).
- `crates/starplayer-offline/src/lib.rs::render_with_kernel` — the minimal engine
  construction sequence (`EngineSettings` sized from the module's header channel count and
  `recommended_voice_capacity`, `take_control`, `load_module`, `scanned_song(..).quirks`,
  `NativeSequencer`, `set_limiter(Limiter::Clamp)`), and `canonical_sha256_with` — the
  little-endian `i16` digest rule the goldens use.
- `crates/starplayer/src/lib.rs` (the documented no_std recipe near line 85) and
  `crates/starplayer/src/sequencer.rs` (`NativeSequencer`, `seek_*`, `set_timeline`).
- `crates/starplayer-engine/src/engine.rs` — `RENDER_QUANTUM`, `EngineSettings`,
  `Engine::with_settings`, `render`, `take_control`, `warnings`, and the allocation table
  in `with_settings` (`buses` is `channel_count × 128 × 8` bytes; scope taps are ~2 KB per
  channel and only exist with `telemetry` **and** `scope_readers` taken).
- `crates/starplayer-rt/src/lib.rs` — `Arc` (the only `Arc` any crate may name),
  `channel`, `garbage_channel`, `snapshot_channel`.
- `crates/starplayer-telemetry/src/snapshot.rs` — `Snapshot` (Copy, no heap),
  `TransportState`, `ChannelState`, `TelemetryReader::read`.
- `crates/starplayer-engine/tests/block_size_determinism.rs` — the invariant test shape
  you will mirror.
- `xtask/src/main.rs:69` `NO_STD_CRATES` and `job_no_std_check`.

## Deliverables

### 1. `crates/starplayer-host-embedded` (no_std + alloc)

Manifest: `starplayer` with `default-features = false` and the format features plus
`telemetry`; `starplayer-rt`; `starplayer-telemetry`; `sha2` with `default-features =
false` for the bench (verify it is `no_std`; it is already a workspace dependency). No
default feature may enable `std` (`--job no-std-purity` enforces it). Add the crate to
`NO_STD_CRATES` in `xtask/src/main.rs` and to the crate-layout block in
`plans/product/01-technical-architecture.md` §11 under `no_std + alloc`.

- `pub struct EmbeddedPlayer<Interp: Interpolator = Linear>` wrapping
  `Engine<FixedPath, Interp, FixedOut<i16, 2>, Arc<Module>>`, its `EngineHandle`, a
  `Transport`, the seek mailbox and the at-end slot. Two halves, as in the std host:
  - `RenderHalf::render(&mut self, output: &mut [i16])` — the cadence port. Any length,
    including a non-multiple of a frame. Returns the block peak as `i16`.
  - `ControlHalf` with `play`, `stop`, `load(Arc<Module>) -> Result<(), Error>` (scans,
    builds the `NativeSequencer`, sends `Load`), `seek_order`, `seek_row`, `seek_frame`,
    `set_master_volume(U0F16)`, `mute(ChannelId, bool)`, `set_at_end(AtEnd)`,
    `set_fade_frames`, `telemetry(&self) -> &Snapshot`, `is_playing`, `song_frame`,
    `song_length`, `collect_garbage(&mut self) -> usize` (drops retired modules and
    sources **here**, never in `render`), `warnings`.
  - Commands cross on `starplayer::rt::channel` with capacity `HOST_COMMAND_CAPACITY`;
    retired modules on a second ring of `RETIRED_CAPACITY`; telemetry on a
    `snapshot_channel` of depth `TELEMETRY_DEPTH`. Reuse the std host's constants by
    value and say in a comment that they are the same numbers for the same reasons.
- `pub fn settings_for(module: &Module, sample_rate_hz: u32) -> EngineSettings` — the
  sizing rule from `render_with_kernel`: channel count from the header, voice capacity
  from `recommended_voice_capacity(module).max(1)`, no scope taps. Document the resulting
  heap cost formula so the budget document (I3) can quote it.
- `EmbeddedPlayer::open(module: Arc<Module>, sample_rate_hz: u32) -> Result<(RenderHalf,
  ControlHalf), Error>`; `open_empty(sample_rate_hz, settings)` for a host that loads
  later (I6's upload path).

### 2. The bench routine

`pub mod bench`: `pub fn render_digest<Interp>(module: &Arc<Module>, sample_rate_hz: u32,
frames: usize, block_frames: usize) -> Result<[u8; 32], Error>` rendering **mono** i16
through `FixedOut<i16, 1>` at the goldens' rate and hashing little-endian bytes exactly as
`canonical_sha256_with` does, so the digest for the six fixtures under
`crates/starplayer-offline/src/bin/starplayer-goldens.rs` equals the committed
`goldens/` hashes when run on the host. The mono arm is a second instantiation, not a
second code path. Also `pub fn render_frames<Interp>(..) -> Result<usize, Error>` that
renders stereo into a caller-supplied scratch and returns frames produced, for the
cycles-per-frame measurement (the firmware supplies the clock).

### 3. Proof (host tests, in `crates/starplayer-host-embedded/tests/`)

- **Block-size determinism through the player**: `REFLEX.S3M` rendered through
  `RenderHalf::render` at block sizes 1, 3, 64, 128, 4096 and 8191 frames is byte-identical,
  with a `Stop` command queued at a frame that is not a multiple of 128 and a seek at
  another, so the cadence is exercised, not just the ring.
- **Byte equality with the std host**: the same module through `starplayer_host::Player`
  over `ManualBackend` at `OutputDepth::I16` with dither off, converted back to `i16`,
  equals `RenderHalf::render`'s stream for the first 10 seconds. If the std path's
  `quantize_fixed_sample` makes exact equality impossible, prove equality against
  `starplayer_offline::render_song_with_options` on the fixed path instead and record why
  in the Research resolution.
- **Command latency**: a `Stop` sent while the render half is mid-block takes effect at
  the next quantum boundary and not before; a `Load` retires the previous module through
  the ring and `collect_garbage` returns 1.
- **The golden digests**: `bench::render_digest` for the six fixtures equals `goldens/`.
- **No allocation in `render`**: reuse `crates/starplayer-offline/tests/render_allocation.rs`'s
  recording allocator pattern over `RenderHalf::render`.

### 4. Documentation

Crate-level doc: what it is (the `no_std` host), what it is not (a port of `Player`),
the cadence rule in one paragraph, and the heap formula. A short section in
`plans/product/01-technical-architecture.md` §11 after the `starplayer-host-wasm` line.

## Research points

1. **What in `starplayer-host` is already `std`-free in shape** — `Transport`,
   `SeekMailbox`, `AtEndSlot`, `scan_module`. If they can be lifted into a `no_std` module
   of `starplayer-host` re-exported to both hosts without touching `Player`'s surface, do
   that rather than copying; if it takes a `std` feature split of `starplayer-host`,
   copy and record the duplication as a follow-up. Do not start a refactor of `Player`.
2. **Does `sha2` build `no_std` on `riscv32imc` with the workspace's pinned features?**
   If not, a minimal SHA-256 in the bench module is acceptable — it is not on the RT path.
3. **`Voice` carries a `PathFilter<f32>` on the fixed path**
   (`crates/starplayer-mixer/src/voice.rs:106-136`, ~20 bytes per voice of dead state).
   Measure the per-voice size and report it; fixing it is out of scope unless trivial.
4. **The `float-mix` / `fixed-mix` / `linear-interp` features gate no code.** Confirm, and
   record the flash cost of the float arms and the 4 KB sinc table in the bench build's
   `size` output (I3 collects it); making the features real is a follow-up for after the
   budget exists.

## Verification

```text
cargo test -p starplayer-host-embedded
cargo xtask ci --job no-std-check       # crate added to NO_STD_CRATES
cargo xtask ci --job no-std-purity
cargo xtask ci --job rt-safety
cargo xtask ci --job goldens            # untouched
cargo clippy -p starplayer-host-embedded --all-targets -- -D warnings
```

## Out of scope

Any board, HAL or async code. Scope taps on the device. Live MIDI input, jam mode, inserts
(the embedded player has no insert control; a follow-up can add `HostInsertControl` once
the budget says there is room). Refactoring `starplayer-host`.
