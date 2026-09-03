# M3 — D4: The host abstraction and the cpal backend

| Field | Value |
|---|---|
| Milestone | M3 ([master plan](M3-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | D3 landed (`starplayer::NativeSequencer`, `recommended_voice_capacity`, `MAX_VOICE_CAPACITY`) |
| Blocks | The CLI `play` command (a small follow-up once D5 has landed the CLI skeleton), A1 (TUI) |
| Parallel with | D5, D6, D7, F1, G1 |
| Recommended model | Claude Opus (a real-time audio callback on a second thread; the first native host) |
| Verified by | agent (`cargo test -p starplayer-host-cpal`, the example plays a fixture on this machine or explains exactly why it cannot), then reviewer, then the owner listens |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine; the hosts are `std`. Read
`AGENTS.md` first — the design invariants there are non-negotiable, above all: no
allocation, locks or panics in `render()`, and the audio thread never drops the last
`Arc<Module>` (it goes back over the garbage channel).

The only real host so far runs in a browser AudioWorklet
(`crates/starplayer-host-wasm`). M3's master plan explains why native came second: the
host abstraction was to be *designed under the harder constraint* and a cpal backend
would then "fall out nearly free". This task is where that is cashed in. The owner
decided (2026-09-03) that the wasm host is **not** retrofitted behind the new trait in
this task — shape the trait from the wasm host's lifecycle, implement it for cpal, and
leave the retrofit as a follow-up — so cpal, the CLI and the TUI are not blocked on the
riskier half.

### What the wasm host does that a native host must do too

Read `crates/starplayer-host-wasm/src/lib.rs` end to end before designing anything. The
lifecycle it embodies, which the trait must capture:

1. **Build the engine once**, before any module, at the maxima (`MAX_VOICE_CAPACITY`,
   `ChannelTable::MAX_CHANNELS`), for the stream's sample rate, in a chosen `MixerMode`
   (`WebEngine` is an enum over the instantiations the host is willing to build; copy the
   idea, choosing the arms a native host wants — float stereo f32 is the default, fixed
   i16 for the golden path).
2. **Load a module off the audio thread**: `starplayer::load` → `Arc::new` →
   `starplayer::scan_song` → `NativeSequencer::new(…, QuirkSelection::Override(scanned.quirks))`
   → `set_timeline` → `set_at_end`; hand the `Arc<Module>` in with
   `EngineHandle::load_module` and the sequencer with `Engine::set_source`, and collect the
   retired `Arc` with `collect_all_garbage` on the control side.
3. **Transport and seeks**: play/stop through `Command`s over the engine's ring; order,
   row and frame seeks through a host-owned mailbox read on the audio thread before
   `render` (`SeekRequest`/`SeekKind`/`SeekableModuleSource` in the wasm host — research
   point 1 asks whether that wrapper belongs in the facade so both hosts share it).
4. **Telemetry**: the `TelemetryReader` is read on the control side; VU and position are
   what a CLI prints. Task D6 is concurrently adding scope taps to the engine and the wasm
   host; do not depend on them.
5. **Click-free stop**: the wasm host ramps the transport gain over 64 frames before
   sending `Command::Stop`. Do the same.
6. **Output depth and dither** are a post-stage applied to the engine's float output
   (`HostSample`, `Dither`), not engine arms.

### Code you must read before changing anything

- `crates/starplayer-host-wasm/src/lib.rs` — the whole thing; `crates/starplayer-host-wasm/src/command.rs`.
- `crates/starplayer/src/{lib,sequencer}.rs` — `load`, `scan_song`, `NativeSequencer`,
  `recommended_voice_capacity`, `MAX_VOICE_CAPACITY`, the host recipe in the crate docs.
- `crates/starplayer-engine/src/{engine,command,ring}.rs` — `Engine::render`,
  `render_native` if it exists, `EngineSettings`, `EngineHandle`, `MixerMode`, `OutputRing`
  (the engine adapts any host block size; the callback just asks for `frames`).
- `crates/starplayer-mixer/src/output.rs` — `HostSample`, `OutputFormat`, `Dither`.
- `crates/starplayer-offline/src/lib.rs` — `render_song` is the offline twin: same
  recipe, no thread.
- `crates/starplayer-host-cpal/{Cargo.toml,src/lib.rs}` — the 9-line stub; the workspace
  `Cargo.toml` for how versions are pinned once in `[workspace.dependencies]`.
- `plans/engine/M3-master-plan.md` deliverables 1–2; `plans/product/01-technical-architecture.md`
  §7.1 (the host owns the mixer mode), §8 (RT rules), §11 (crate layout: hosts depend only
  on the facade).

## Deliverables

### 1. `starplayer-host` (new, `std`) — the abstraction

A small crate `crates/starplayer-host` (the M3 master plan's "`starplayer-audio`"; name it
`starplayer-host` to match the two backend crates) holding what is backend-neutral:

- `AudioSpec { sample_rate_hz, channels, preferred_block_frames }` and `DeviceInfo { name, is_default, supported_rates }`;
- `trait AudioBackend { fn devices(&self) -> Vec<DeviceInfo>; fn open(&mut self, device: Option<&str>, requested: AudioSpec, callback: Box<dyn FnMut(&mut [f32]) + Send>) -> Result<Stream, HostError>; }`
  with `Stream { spec: AudioSpec, .. }` exposing `play`, `pause`, `close`, and the
  negotiated spec — the callback is asked for interleaved f32 frames of any length;
- `Player` — the backend-neutral controller built on the recipe above: `load(bytes)`,
  `play`, `stop`, `seek_order`, `seek_frame`, `set_at_end`, `set_master_volume`,
  `mute`, `telemetry() -> &Snapshot`, `collect_garbage`, `song_length`, `song_frame`. It
  owns the engine on the audio side and the control handle, mailbox and telemetry reader
  on the caller's side. Everything the wasm host's `Host` does that is not
  wasm-specific lives here, so that the retrofit later is "wasm implements
  `AudioBackend`" and nothing else.

Keep it honest: if a piece of the wasm host's logic (the seek mailbox, the transport
ramp, the mixer-mode enum builder) is host-neutral, move it here or to the facade rather
than copy it (research point 1).

### 2. `starplayer-host-cpal` — the backend

`cpal` pinned in `[workspace.dependencies]` (newest stable 0.15/0.16 — research point 2),
default host, ALSA and PulseAudio via cpal's defaults, `f32` and `i16` stream formats
(convert i16 in the callback with `HostSample`), device enumeration by name, sample-rate
and buffer-size negotiation (request the module's preferred rate, accept what the device
gives, tell `Player` the negotiated spec so the engine is built at *that* rate), error
callback wired to a flag `Player` exposes. The audio callback: drain the seek mailbox,
`engine.render(frames)`, apply transport gain and depth, write out. No allocation in the
callback — prove it with the allocator-hook pattern from
`crates/starplayer-offline/tests/render_allocation.rs` applied to the callback closure in
a test that drives it directly (no device needed).

### 3. `examples/play.rs` (in `starplayer-host-cpal`)

`cargo run -p starplayer-host-cpal --example play -- <module> [--device NAME] [--rate HZ] [--buffer FRAMES] [--list-devices]`:
plays the module to its natural end or loop point, printing order/row/BPM from telemetry
once a second, exits 0. It is the acceptance test until the CLI's `play` is wired.

### 4. Documentation

`plans/engine/M3-master-plan.md` deliverable 1–2 status; architecture §11 lists
`starplayer-host`; the crate docs explain the split and the deferred wasm retrofit.

## Research points

1. **What is host-neutral in the wasm host?** Read `SeekableModuleSource`, `SeekRequest`,
   the transport `GainRamp`, `WebEngine` and `pending_engine_stop`; decide what moves to
   `starplayer-host` (or the facade, if `no_std`) and what stays. Record the list. Do not
   edit the wasm host beyond re-importing anything you move (task D6 is editing it
   concurrently; keep the diff there minimal).
2. **cpal version and Linux backends.** Pin the newest stable `cpal`; confirm which
   features give ALSA and PulseAudio on Linux and what system packages the build needs
   (`libasound2-dev`). This machine is WSL2: check whether a PulseAudio server is reachable
   (`$PULSE_SERVER`, `/mnt/wslg/PulseServer`) and whether cpal enumerates any device. If
   no device exists, the example must fail with a clear message, and the acceptance runs
   through the direct-callback test instead — say which happened.
3. **Block-size independence on a real device.** cpal delivers ragged buffer sizes;
   the engine's ring adapts. Add a test that feeds the callback sizes 1, 3, 64, 128, 4096
   and 8191 and compares the concatenation to a single 128-frame-block render: byte-identical.
4. **Sample rate and the timeline.** The scan depends on the rate (architecture §4.1);
   confirm `Player::load` scans at the negotiated rate and rescans if a stream is reopened
   at another rate.

## Verification

```sh
cargo test -p starplayer-host -p starplayer-host-cpal
cargo run -p starplayer-host-cpal --example play -- --list-devices
cargo run -p starplayer-host-cpal --example play -- crates/starplayer-s3m/tests/fixtures/NICETUNE.S3M
cargo test --workspace
cargo xtask ci --job clippy
cargo xtask ci --job no-std-purity          # the new std crates must not be in NO_STD_CRATES and must not leak std into the facade
cargo xtask ci --job host-tests
```

Report the exact commands run and their results, including what `--list-devices` printed
on this machine. **Do not commit** — the reviewer commits.

## Out of scope

The wasm retrofit behind `AudioBackend` (follow-up task). The CLI (D5). Windows and macOS
testing (A2). MIDI input (M4-full).
