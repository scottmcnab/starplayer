# M9 — J3: Hosting CLAP effects in the insert chains

| Field | Value |
|---|---|
| Milestone | M9 ([master plan](M9-master-plan.md), "The task graph" section) |
| Status | Planned; pull-driven (not scheduled) |
| Depends on | M7-H7 (`Player::install_insert`, `InsertTarget`, the CLI `--insert` grammar) |
| Blocks | The M9 exit criterion ("hosts at least one third-party effect on a channel") |
| Parallel with | J1 |
| Recommended model | Claude Opus (a plugin host on the audio thread: activation, the process call, parameter flushing, and the fixed-path adapter) |
| Verified by | agent (a bundled test effect processed byte-identically at every block size, RT-safety, latency reported), then owner with a real plugin |

## Context for a fresh agent

M7 landed the DSP graph: per-channel and master insert chains of `Box<dyn Insert<Sample>>`
(`crates/starplayer-dsp/src/insert.rs`), installed off the audio thread through
`InsertCommand::Install` and retired over a garbage channel
(`crates/starplayer-engine/src/insert.rs`), with `Player::install_insert` and the CLI's
`--insert <target>:<effect>` on top (H7). An `Insert` is "process one 128-frame block in
place, take integer parameters, reset" — and a CLAP audio-effect plugin is exactly a thing
that processes a block in place and takes parameters. This task wraps one as an `Insert`.

Master-plan decision 5 says hosting is **float-first**: a hosted plugin runs on `f32`, and
on the fixed path an adapter converts `i32 ↔ f32` around it, which is the one place the
fixed path's cross-target bit-exactness stops — it never held for third-party code. No
plugin-delay compensation: the plugin's reported latency is summed and *reported*, not
aligned. Q5 chose `clack`; `clack-host` is its host half.

### Code you must read before changing anything

- `crates/starplayer-dsp/src/insert.rs`, `effects/mod.rs`; `crates/starplayer-engine/src/insert.rs`
  and the chain loop in `engine.rs`; `crates/starplayer-host/src/player.rs` (H7's insert
  API and `InsertLayout`); `apps/starplayer-cli` (`--insert` parsing, `--list-effects`).
- `crates/starplayer-clap/src/*` if J1 has landed (shared `clack` pin, shared FFI module
  conventions); otherwise J1's task file's research point 1.
- `clack-host`'s examples (a minimal host: bundle load, factory, instance, activation,
  `process`, param flush) and CLAP's `audio-ports`, `params`, `latency`, `state` extensions.
- `plans/product/01-technical-architecture.md` §7.2 (as amended by H1/H3/H4), §8;
  `CLAUDE.md` (the `unsafe` rule; confine FFI to one module).

## Deliverables

### 1. `crates/starplayer-clap-host` (std)

- `ClapEffect::load(bundle_path, plugin_id: Option<&str>, sample_rate_hz) -> Result<ClapEffect, HostError>`:
  loads the bundle, picks the plugin by id (or the only audio-effect in it), activates it
  at the engine rate with `min_frames = max_frames = DSP_BLOCK_FRAMES`, and configures one
  stereo in / one stereo out. Instantiation and activation happen on the caller's thread
  (main); the returned object is `Send` and is installed like any insert. Refuses
  instruments and plugins without a stereo pair, with the reason.
- `impl Insert<f32> for ClapEffect`: `process` de-interleaves the `Stereo<f32>` block into
  two planar scratch buffers sized at load, calls the plugin's `process` with
  `steady_time` advanced by 128 per call, re-interleaves. Parameter changes since the last
  block are delivered as `CLAP_EVENT_PARAM_VALUE` events at offset 0 in the same call
  (which is also the flush). `reset` calls the plugin's `reset`. `set_param`/`param` map
  `ParamId` to the plugin's parameter *index* (stable within an instance) with the value
  scaled from the plugin's `[min, max]` to the `ParamSpec` range the descriptor advertises
  (percent of range ×100, so every plugin parameter is `0..=10000`; the descriptor's names
  are the plugin's).
- `impl Insert<i32> for FixedAdapter<ClapEffect>`: converts the block `i32 → f32` (the
  mixer's `i16`-scale divided by 32768), processes, converts back with rounding and
  saturation. Documented as decision 5.
- `descriptor()` is built at load from the plugin's parameter list into a leaked
  `&'static InsertDescriptor` — one leak per loaded plugin instance, off the audio thread,
  documented. (The trait wants `&'static`; a `Box::leak` of a few hundred bytes per
  plugin load is the honest cost, not a redesign of the trait.)
- **Latency**: `ClapEffect::latency_frames()` from the `latency` extension;
  `Player::insert_latency_frames()` sums every hosted effect's latency along the master
  path and the deepest channel path, and telemetry gets `dsp_latency_frames: u32` so a
  UI can show it. Nothing is compensated.
- **Main-thread callbacks**: CLAP plugins may `request_callback`; the host side polls
  `ClapEffect::main_thread_tick()` from `Player::collect_garbage` (already called
  periodically by every host).
- **Plugin state**: `ClapEffect::save_state()`/`load_state()` through the `state`
  extension, so J2's plugin state and a CLI session can persist a hosted effect's settings.

### 2. Host surfaces

- `Player::install_clap_insert(target, slot, bundle_path, plugin_id)` beside
  `install_insert`, building the boxed effect on the caller's thread for the arm's path.
- CLI: `--insert 1:clap:/path/to/effect.clap[#plugin.id][:param=value,...]` and
  `--list-clap /path/to/bundle.clap` (ids, names, parameter list). `render` refuses
  hosted plugins under `--golden`.
- `starplayer-clap` (J1), when both have landed: the instrument's insert layout may name
  hosted effects; state (J2) stores bundle path + plugin id + plugin state blob. A missing
  bundle on load is a warning, not a failure.

### 3. Proof

- **A bundled test effect**: build a tiny CLAP effect in `crates/starplayer-clap-host/tests/gain_plugin/`
  (`clack-plugin`, a gain with one parameter; `cdylib` built by the test harness via
  `cargo build` of the fixture crate, or committed as a workspace example) and host it:
  a render with it at 0 dB is byte-identical to a render with no insert; at −6 dB it is
  byte-identical to H1's gain insert at −600 centi-dB on the float path.
- **Block-size determinism** through the hosted effect at 1, 3, 64, 128, 4096 and 8191
  (the engine feeds it whole quanta regardless — the test proves the wrapper adds no
  state that depends on the host block).
- **RT-safety**: `process` under the allocation hook with a stream of parameter changes.
- **Latency**: a plugin reporting 64 frames makes `insert_latency_frames()` say 64.

### 4. Documentation

Architecture §7.2: "third-party effects" paragraph; §11: `starplayer-clap-host`; the CLI
help; append `## Research resolution`.

## Research points

1. `clack-host`'s threading contract: which calls are `[main-thread]`, which
   `[audio-thread]`, and how it enforces them in types; the `Insert` object crosses from
   main to audio exactly once at install and back once at retirement.
2. Whether `max_frames = 128` upsets plugins that want larger blocks (some allocate per
   `max_frames`; smaller is always legal). Measure with two real plugins the owner has
   (Surge XT's effects and Airwindows are open CLAP effects).
3. In-place processing: CLAP allows `in == out` buffer aliasing only if the plugin says
   so; use separate in/out scratch and copy — measure the cost.

## Verification

```
cargo test -p starplayer-clap-host
cargo test -p starplayer-host
cargo run -p starplayer-cli -- render <fixture.s3m> --insert 1:clap:target/.../gain_plugin.clap -o /tmp/hosted.wav
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
cargo xtask ci --job rt-safety
```

## Out of scope

Hosting instruments; plugin GUIs; plugin-delay compensation; sidechains; multi-bus
plugins; VST3/AU/LV2 effects.
