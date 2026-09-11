# M8 — I1: The embedded host core — `starplayer-host-embedded`

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](../M8-master-plan.md), decisions 1, 2 and 6) |
| Status | Implemented 2026-09-11; awaiting review |
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

## Research resolution

Implemented 2026-09-11 on branch `m8-i1`.

### 1. What in `starplayer-host` is already `std`-free in shape

**Copied, not lifted — and the task's escape clause is the right one.** Three findings:

- `transport.rs` really is `std`-free in shape *and* in text: `AtEnd`, a `GainRamp` and
  `core::mem::take`. It came across unchanged, tests and all.
- `source.rs` is **not**. It names `std::sync::Arc` and
  `core::sync::atomic::{AtomicU8, AtomicU64}`, and on `riscv32imc-unknown-none-elf` the
  first does not exist (no compare-and-swap) and the second does not exist either — that
  target implements no atomics at all. `SeekMailbox` therefore had to be rewritten onto
  `starplayer_rt::Arc` and `starplayer_rt::atomic`, and its two `AtomicU64`s onto the
  seqlock described in point 1a below. So even a `std` feature split of `starplayer-host`
  would not have let the two hosts share this file as it stands: the shared version would
  have to be the embedded one, and `Player` would then be the crate that changed.
- Sharing at all is blocked by the dependency direction, not just by the text.
  `starplayer-host` is `std` by construction (`RenderCallback = Box<dyn FnMut(&mut [f32])
  + Send>`, `Mutex` in `ManualBackend`, `String` errors, device enumeration), and a crate
  in `xtask`'s `NO_STD_CRATES` may not depend on one. Lifting would take the `std` feature
  split the task file says to avoid.

**Recorded as a follow-up**, in the crate's module documentation and in architecture §11:
three things would move into a shared `no_std` core if it is ever done — the transport,
the seek mailbox and the twenty lines of `render` that walk a block quantum by quantum —
and the mailbox would have to take the seqlock with it.

#### 1a. An unplanned finding: there is no 64-bit atomic on either M8 target

The std host's lossy taps and its seek mailbox are built on `AtomicU64`. Neither the CI
bare-metal target nor the ESP32's LX6 has a 64-bit atomic instruction, and
`portable-atomic`'s 64-bit fallback — which the workspace does not enable anyway — is a
**critical section**, i.e. a lock, taken inside `render()`, which design goal 5 forbids.
Truncating the two frame clocks to `u32` was the other option and was rejected: 2^32 frames
is 27 hours at 44.1 kHz, and what breaks at the wrap is the clock a freshly loaded
sequencer is stamped against.

`src/seqlock.rs` is the answer, and it is exactly the seqlock architecture §9(a) already
names, narrowed to one value: the writer bumps a version odd, writes two `AtomicU32`
halves and bumps it even — four stores, no branch, no retry, no interrupt mask — and a
reader that catches a write in progress reads again (bounded at eight attempts, because one
of the two readers is the DMA refill). It carries `Taps::{source_frame, output_frame}` and
`SeekMailbox::{argument, frame}`. `blocks_rendered` and `commands_rejected` stayed 32-bit
counters: they are diagnostics, and one that wrapped after four billion DMA blocks — five
months — would still be answering the question it is asked.

### 2. Does `sha2` build `no_std` on `riscv32imc` with the workspace's pinned features?

**Yes, unchanged.** The workspace already pins `sha2 = { version = "0.10",
default-features = false }`, and `cargo xtask ci --job no-std-check` and `--job
no-std-purity` both pass with `starplayer-host-embedded` in `NO_STD_CRATES` and `sha2` a
non-optional dependency of it. No minimal SHA-256 was needed.

### 3. The per-voice size

**184 bytes per voice**, measured by differencing two engines in
`tests/render_allocation.rs` rather than read off a table. `size_of::<Voice>()` is 176; the
pool's `VoiceSlot` adds the free-list link, and 184 is what the heap actually moves by per
voice.

About twenty of those bytes are the `PathFilter<f32>` the task file names — two `f32` of
delay line plus a `FilterCoefficients<f32>` — which a fixed-path build carries and never
reads, because a `Voice` is not generic over the mix path. Fixing it is not trivial (it
means making `Voice` generic over the path, or splitting the filter state out of the pool),
so it stayed out of scope as the task allows. At 64 voices it is 1.3 kB.

### 3a. The heap formula, and the surprise in it

```text
heap bytes = 27_800 + 184 × voice_capacity + 5_288 × channel_count
```

Measured, pinned by `the_heap_cost_of_an_engine_is_linear_in_the_voice_and_channel_counts`
and by `a_real_module_reports_what_it_costs`, and quoted in `settings_for`'s documentation
so I3 can lift it. `REFLEX.S3M` (3 channels, 3 voices) is 44 216 bytes; `PETRI.S3M` (8 and
8) is 71 576; a 32-channel module with a 64-voice pool is 209 kB. All before the module's
own PCM.

Two things a budget has to know that the task file did not anticipate:

- **The scope tap rings exist whether or not the readers are taken.** The task file says
  they "only exist with `telemetry` **and** `scope_readers` taken". They do not:
  `Engine::with_settings` calls `ScopeTaps::new(channel_count)` unconditionally under
  `feature = "telemetry"`, and this host needs `telemetry` because its cadence reads
  `Snapshot::transport.end_reached`. That is ~2.1 kB of the 5 288 bytes per channel — 1024
  `i16` buckets plus two `Arc` headers — and it is 40 % of the per-channel cost.
  **Follow-up:** giving the taps their own feature in `starplayer-engine` would save
  `channel_count × 2.1 kB` on a device that draws no scope.
- **27.8 kB is fixed, and 15.7 kB of it is two rings of `Snapshot`** — the engine's own
  telemetry channel and this host's forwarding one, three deep each. A `Snapshot` is 2 616
  bytes because it carries 64 channels whatever the module has
  (`starplayer_telemetry::MAX_CHANNELS`, fixed at 64 by M1-B6). On a 4-channel MOD the
  telemetry plumbing costs more than the audio does.

And one figure that is not heap at all: **`size_of::<RenderHalf<Linear>>()` is 11 744
bytes**, because the engine's telemetry publisher holds a working `Snapshot` inline and
this host holds another. A firmware must put it in a `static` or a `Box`, not pass it down
a call chain.

### 4. The `float-mix` / `fixed-mix` / `linear-interp` features

**Confirmed: they gate no code.** `grep -rn 'feature = "float-mix"' crates/ apps/
--include=*.rs` and its two siblings find nothing; the features exist only as forwarding
entries in the manifests. That is why this crate takes `starplayer` at its *default*
feature set plus `telemetry`: cargo refuses to let a member set `default-features = false`
on a workspace dependency that does not, and turning them off would have bought nothing.

**The flash figures are I3's to collect, and the reason is worth recording.** With
`lto = "thin"` the bare-metal rlibs carry bitcode rather than sections, so `size` on
`target/riscv32imc-unknown-none-elf/release/*.rlib` reports zeroes; a real figure needs a
linked firmware binary, which is what I3 builds. What can be said now:

- `SINC_TABLE_Q15` is exactly **4 096 bytes** (256 phases × 8 taps × `i16`,
  `starplayer-dsp/src/sinc_table.rs`), and it is referenced only from `Sinc`'s
  `Interpolate` impl. A firmware that instantiates only `Linear` never references it, so
  linker GC should drop it — the 4 kB is not automatically paid.
- The float arms are the same story: `FloatPath`, `FloatOut` and the float half of the
  voice kernels are **generic**, so a firmware that instantiates only
  `Engine<FixedPath, Linear, FixedOut<i16, 2>, Arc<Module>>` never monomorphises them.

So making the features real is worth doing only if I3's `size` output shows float or sinc
code actually surviving into the binary — which is precisely the "follow-up for after the
budget exists" the task file anticipated.

## What was done differently, and why

- **`ControlHalf::telemetry` takes `&mut self`**, not `&self` as the deliverable lists.
  `SnapshotReader::read` consumes from a ring to reach the newest snapshot and so needs
  `&mut`; `starplayer_host::Player::telemetry` has the same signature for the same reason.
- **`EmbeddedPlayer` is a constructor, not a live value.** The deliverable describes it as
  "wrapping" the engine and the transport, but `open` hands back both halves and there is
  nothing left for a wrapper to own — the halves are meant to be moved apart immediately.
  It is a `PhantomData` namespace carrying the interpolator choice. Note that
  `EmbeddedPlayer::open` must be spelled `EmbeddedPlayer::<Linear>::open`: a default type
  parameter is used when a type is written out, never to infer one at a call site.
- **A `Load` retires *two* things, so `collect_garbage` returns 2, not 1.** The deliverable
  says 1. The engine hands back the retired module over its garbage channel and
  `replace_source` hands back the retired sequencer, and both go down the same ring —
  exactly what `starplayer-host`'s own `a_retired_module_is_dropped_on_the_control_thread`
  asserts. The test asserts 2 and says why.
- **The block-size determinism test cuts the stream at each scripted command's frame.** A
  command issued "whenever the driver next gets round to it" lands on a different quantum
  at every block size, so there would be nothing to compare. Cutting at a fixed *emitted
  frame* — 1 037, 12 345 and 16 501, none a multiple of 128 — issues each command at the
  same musical instant in every run and leaves the cadence to carry it to the next quantum
  boundary, which is the thing being tested. The ragged short block at each cut is a bonus:
  the invariant is about any sequence of block lengths.
- **The byte-equality test needed no escape hatch.** `RenderHalf::render`'s output is
  byte-identical to `starplayer_host::Player` over `ManualBackend` at
  `MixerMode { path: Fixed, depth: I16, dither: false, interpolator: Linear, channels: 2 }`
  for the first ten seconds of `REFLEX.S3M`, including through the 64-frame play ramp —
  `quantize_fixed_sample` at `OutputDepth::I16` reduces to `<i16 as
  HostSample>::from_i16_scale`, which is the clamp this crate applies, and the `f32` the
  std host emits is `code / 32_767.0` exactly. The differing voice capacity and channel
  count between the two engines change no rendered sample, as the engine's documentation
  claims.
- **The render loop counts samples, not frames.** The std host's walks `frames_emitted` and
  truncates when a block is not a whole number of frames. Counting interleaved samples and
  carrying the in-force gain across calls makes a ragged block correct rather than merely
  tolerated, and `a_render_that_is_not_a_whole_number_of_frames_keeps_the_stream_aligned`
  pins it at 2 001 samples a block.
- **`bench::render_frames` builds its engine inside the timed call**, which is documented
  on it: a firmware should ask for a long render so the construction amortises, or bracket
  `RenderHalf::render` instead. Keeping an engine alive across calls would have meant a
  handle type the deliverable does not ask for.
- **Two extra golden tests** beyond the deliverable: `reflex`'s cubic and sinc cross-target
  pins (M7-H5), because those exist to catch a kernel that is not bit-identical across
  architectures and that is exactly the question M8 asks of Xtensa and RISC-V; and the
  digest at six block sizes, because a device's figure must not depend on its DMA buffer.
- The crate-layout entry in architecture §11 was already added by the M8 planning commit;
  this task added the prose section after the host paragraphs.

## Verification run

| Command | Result |
|---|---|
| `cargo test -p starplayer-host-embedded` | pass — 9 unit, 11 player, 4 golden, 4 allocation, 1 doc |
| `cargo xtask ci --job no-std-check` | pass |
| `cargo xtask ci --job no-std-purity` | pass |
| `cargo xtask ci --job rt-safety` | pass (needed `cargo xtask conformance --fetch-only` first) |
| `cargo xtask ci --job goldens` | pass, all eight hashes unchanged |
| `cargo clippy -p starplayer-host-embedded --all-targets -- -D warnings` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo xtask ci --job host-tests` | pass |
