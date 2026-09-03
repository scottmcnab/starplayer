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

## Research resolution

### 1. What is host-neutral in the wasm host? — **five things moved, two stayed, and one had to move**

Read end to end, the browser host divided cleanly.

**Moved to `starplayer-host`:**

| what | where it lives now | why it is not browser-specific |
|---|---|---|
| `SeekKind`, `SeekRequest` | `starplayer_host::source` | a seek is a seek |
| the seek mailbox (`Rc<Cell<SeekRequest>>` → `SeekMailbox`) | `starplayer_host::SeekMailbox` | latest-wins single-slot delivery is the answer to a question every host asks |
| the repeat cell (`Rc<Cell<AtEnd>>` → `AtEndSlot`) | `starplayer_host::AtEndSlot` | same |
| `SeekableModuleSource` | `starplayer_host::SeekableModuleSource` | it exists because `Engine` stores `dyn EventSource` and so cannot downcast to seek — a property of the engine, not of the browser |
| `dither_for`, `quantize_float_sample`, `quantize_fixed_sample`, `DITHER_SEED` | `starplayer_host::depth` | pure arithmetic over `HostSample`; the two copies were identical |
| `scan_for` / `source_for` | `starplayer_host::{scan_module, build_source}` | the recipe in the facade's crate docs, with `Rc` swapped for `Arc` |

**Stayed in the browser host:** `WebEngine` and `pending_engine_stop`/`fading`/`fade_elapsed`
— i.e. the engine-arm enum and the transport. Both live *inside* `process()`, and lifting
them is a rewrite of that function rather than a re-import; `starplayer-host` has its own
`HostEngine` and `Transport`, which are the same shapes generalised. Deleting the browser
copies is the deferred `AudioBackend` retrofit, not this task.

**One of them had to move rather than merely deserved to.** `EventSource` is now `Send`
(see below), so `SeekableModuleSource` could not keep holding `Rc<Cell<..>>`: the browser
host stopped compiling the moment the bound landed. That is why the wasm-host diff is not
zero. It is confined to deleting the five moved items, importing them, and renaming three
call sites (`request.get()` → `.peek()`, `request.set(..)` → `.request(..)`,
`request.set(default())` → `.clear()`); `AtEndSlot` kept `get`/`set` so those sites are
untouched. Nothing about the worklet's structure changed — the handles are still built per
module there, which a single-threaded realm can afford.

**Engine edits, and which survived.** Three were in the working tree; all three are kept,
and each is load-bearing:

* **`EventSource: Send`** (`source.rs`) — the reason the task exists. A native host builds
  the sequencer on the control thread and the engine that holds it renders on cpal's audio
  thread, so the whole `Engine` — and therefore every source in it — must be `Send`. It is
  spelled as a supertrait rather than on `Engine` so the failure is reported at the source
  that is not `Send`. `PatternSequencer`'s impl grew `Tempo/Processor/Data: Send` bounds and
  `scan_timeline` repeats them because it drives a sequencer through the same trait; every
  tempo model, processor and pattern decoder in the repository is plain data over an
  `Arc<Module>`, so the bound costs nothing. The `rt-safety` job confirms the render path is
  unchanged.
* **`SourceMux::take_first`** and **`Engine::replace_source`** — `Engine::set_source` *drops*
  what it replaces. That is a `free()`, which is fine in a worklet message task and
  forbidden in an audio callback (architecture §8). A native host has no choice about where
  it swaps: once the stream is open, the callback is the only place a `&mut Engine` exists.
  `replace_source` therefore hands the retired source back, and `Player` sends it down the
  same retirement ring a retired `Arc<Module>` already travels on, to be dropped on the
  control thread. `take_first` is the mux half of that — `remove` needs a `SourceSlot`
  handle, which the callback does not hold.

The one further edit was to two engine **tests** whose `Rc<RefCell<Vec<_>>>` dispatch logs
are no longer `Send`. They are now fixed-size atomic logs — deliberately not a `Mutex`,
because a test double that took a lock inside `dispatch` would model the one thing
`render()` may never do.

### 2. cpal version and Linux backends — **0.18.2, `default-features = false, features = ["pulseaudio"]`; ALSA headers needed; WSLg has a PulseAudio sink and no ALSA card**

`cpal 0.18.2` is the newest stable (`cargo search cpal`), pinned once in
`[workspace.dependencies]`. On Linux cpal **always** builds its ALSA host — it is not
behind a feature — so the build needs the ALSA *headers* and `alsa.pc` regardless of which
host is used at run time. The normal route is `sudo apt install libasound2-dev` (or
`alsa-lib-devel`), and that is what CI takes.

The `pulseaudio` feature adds cpal's second Linux host. It costs no system dependency —
the `pulseaudio` crate speaks the protocol itself rather than binding `libpulse` — and it
is the only host that finds a real device on a machine with no sound card, which is every
container and every WSL2 box. Its sibling `pipewire` feature is left off: it binds
`libpipewire`, which *is* a system dependency, and a PipeWire server answers on the
PulseAudio socket anyway. cpal 0.18's own default feature set is empty, so
`default-features = false` only says so out loud. On Linux `cpal::default_host()` therefore
resolves to PulseAudio when a server is reachable and ALSA otherwise.

**This machine (WSL2 + WSLg), measured.** `$PULSE_SERVER` is `unix:/mnt/wslg/PulseServer`
and it is reachable. `cargo run -p starplayer-host-cpal --example play -- --list-devices`
prints:

```
* [PulseAudio] RDP Sink
    rates: 8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000
  [ALSA] Discard all samples (playback) or generate zero samples (capture)
    rates: 8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000
```

So: a real device exists, `cpal::default_host()` picks PulseAudio, and the acceptance run
is the real one — `play` renders `NICETUNE.S3M` to its end and exits 0. ALSA alone would
have found only `null`, which is exactly why `pulseaudio` is compiled in.

**Building without root.** The headers are not installed here and there is no root, so
`alsa-lib` 1.2.11 was built from its release tarball (it ships `configure`; no autoconf
needed) into `target/alsa-lib`, and cargo is pointed at it with

```sh
PKG_CONFIG_PATH="$PWD/target/alsa-lib/lib/pkgconfig" cargo test -p starplayer-host-cpal
```

`PKG_CONFIG_PATH` is *prepended* to pkg-config's own path, so exporting it is harmless on a
machine that has the system package. It is deliberately **not** committed to
`.cargo/config.toml` or any other file: it names a build output that exists only where
somebody built it. The crate docs carry both routes.

**A finding worth recording: the WSLg sink's default block is 96 000 frames — two seconds.**
Asking for no particular buffer size gets that, and with it two seconds of latency on every
stop, seek and telemetry reading. The rendered audio is identical either way (design goal 3
holds, and the tests prove it at 1, 3, 64, 128, 4096 and 8191), but the *latency* is not, so
`examples/play.rs` asks for 1024 frames by default and `--buffer 0` restores the device's
own. This is also why `RenderState::render` publishes telemetry once per **callback** rather
than once per quantum: a per-quantum publish was tried and is strictly worse, because the
snapshot ring keeps the oldest three when it fills, so a callback publishing a hundred
snapshots leaves the reader holding the one from the *start* of the block. The reasoning is
recorded on `RenderState::render` so it is not re-tried.

### 3. Block-size independence on a real device — **proved at 1, 3, 64, 128, 4096 and 8191, on both paths and across the end of the song**

`crates/starplayer-host/tests/player.rs` drives the callback directly through
`ManualBackend`, so no device is involved:

* `the_same_song_renders_byte_identically_at_every_block_size` — float path, 44.1 kHz,
  25 600 frames, compared bit-for-bit (`f32::to_bits`) against the 128-frame render;
* `the_fixed_path_is_block_size_independent_too` — the same on the canonical fixed path,
  where one differing bit is a real difference rather than a rounding one;
* `a_song_played_past_its_end_still_renders_identically_at_every_block_size` — the case the
  quantum alignment in `RenderState::render` exists for: the end-of-song decision has to
  land on the same frame at every block size, or a stop starts up to a whole block late.

The mechanism is that the callback walks the block aligned to **frames emitted**, not to the
block, so the control plane is drained and the end of the song armed on a multiple of
`RENDER_QUANTUM` whatever length the device asked for — the cadence the browser worklet gets
for free by always being called with 128 frames.

Allocation is proved separately, in `crates/starplayer-host/tests/callback_allocation.rs`,
with the allocator-hook pattern from `starplayer-offline`: the callback allocates nothing on
any of the eight engine arms at any of the five depths, and swapping a module inside the
callback allocates nothing **and drops nothing** there.

### 4. Sample rate and the timeline — **`Player::open` negotiates first and scans at the answer; `Player::reopen` rebuilds and rescans**

`AudioBackend::negotiate` is a separate call from `open` precisely because its answer
decides two things that cannot be corrected afterwards: the rate the engine's voice pool and
control clock are built for, and the rate every module is scanned at (architecture §4.1).
`Player::open` negotiates, builds the engine at the negotiated rate, opens the stream, and
then refuses the stream if the backend opened something other than what it agreed to.
`Player::load` scans at `self.spec.sample_rate_hz`, which is the negotiated rate and never
the requested one.

`Player::reopen` closes the stream, builds a new player at the new rate and **re-scans** the
loaded module rather than carrying the timeline over: a song timeline is measured in frames
at one output rate, and installing a 44.1 kHz timeline in a 48 kHz sequencer leaves the
progress slider, the loop point and the audio disagreeing about where the song is. Master
volume and the repeat setting are restored; per-channel mutes are not, because the caller
owns those.

Two tests: `the_song_is_scanned_at_the_rate_the_device_agreed_to_not_the_rate_that_was_asked_for`
(ask for a rate the backend does not have, and check the scanned length is the negotiated
rate's) and `reopening_at_another_rate_rebuilds_the_engine_and_rescans_the_module`.

The one place a scan is *reused* is a mixer-mode rebuild, which changes nothing the scan
depends on — the timeline is a function of the output rate and the module's dialect and of
nothing the mixer chooses — so `build_source` takes an optional cached scan.
