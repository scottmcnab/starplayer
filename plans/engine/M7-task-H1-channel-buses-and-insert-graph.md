# M7 — H1: Channel buses, the `Insert` trait, and the install/parameter plumbing

| Field | Value |
|---|---|
| Milestone | M7 ([master plan](M7-master-plan.md), "The task graph" section) |
| Status | Ready |
| Depends on | — (M6 landed) |
| Blocks | H3, H4, H6, H7 |
| Parallel with | H2 (dsp primitives), H5 (interpolators) |
| Recommended model | Claude Opus (the render loop, both mix paths, the golden contract, RT-safety) |
| Verified by | agent (`cargo xtask goldens --check` byte-identical, block-size determinism with an insert active, the allocation hook with an insert active), then reviewer |

## Context for a fresh agent

StarPlayer's render loop already has the shape architecture §1.4 asks for: `Engine::render_quantum`
(`crates/starplayer-engine/src/engine.rs`) accumulates voices in event-split segments inside a
128-frame quantum, then calls `process_channel_inserts` (a no-op with the right signature) and
`process_master_bus` (real since M1-B5: master volume, then the limiter) on the **whole quantum**.
This task makes the no-op real.

Two things stand in the way, and both are architectural rather than DSP:

1. **There are no per-channel buses.** `VoicePool::accumulate_masked`
   (`crates/starplayer-mixer/src/voice.rs`) sums every voice into one accumulator in slot order.
   A per-channel insert needs a per-channel signal, so the pool has to sum each voice into the bus
   of its `tag.channel`, and the engine then sums the buses. Master-plan decision 1 says the buses
   are **always on** — there is no "no inserts, old path" branch to keep in sync — and that the
   nine fixed-path goldens must not move. Read that decision and its reasoning before touching
   the pool; then prove it with `cargo xtask goldens --check`.
2. **Nothing can be built on the audio thread.** An effect owns delay lines; allocating them in
   `render()` is banned (design goal 5, proven by `crates/starplayer-offline/tests/render_allocation.rs`).
   So an insert is built by the host, arrives boxed over a control ring, and is retired through a
   garbage channel — the same discipline `Command::LoadModule` and `GarbageChannel<Module>` use
   (`crates/starplayer-engine/src/command.rs`, `crates/starplayer-rt/src/garbage.rs`).

The `Insert` trait is committed in this task with its first implementation (a gain insert used
by the tests) and its second arriving in H3/H4; design goal 8 is satisfied because the trait is
shaped from the effects' known needs (whole-quantum block, integer parameters, reset), not
speculatively. It lives in `starplayer-dsp`, generic over a `DspSample` arithmetic trait
that H2 is writing concurrently — coordinate by using exactly the surface named below and
nothing more, so the merge is a `pub use` line.

### Code you must read before changing anything

- `crates/starplayer-engine/src/engine.rs` — `render_quantum`, `drain_commands`, `apply_command`,
  `process_channel_inserts`, `process_master_bus`, `EngineSettings`, `with_settings`.
- `crates/starplayer-mixer/src/voice.rs` — `accumulate_masked`, `VoiceTag`, the slot/free-list
  bookkeeping. `crates/starplayer-mixer/src/path.rs` — `MixPath`, `Stereo`, `FloatPath`,
  `FixedPath::accumulate` (saturating), `master`.
- `crates/starplayer-mixer/src/master.rs` — the master bus as it stands.
- `crates/starplayer-dsp/src/ramp.rs` — `GainRamp`, and its module comment on why a ramp is a
  function of frames since the change. Smoothing in this task follows it exactly.
- `crates/starplayer-engine/src/command.rs`, `crates/starplayer-rt/src/{spsc,garbage}.rs` —
  the control plane you are extending.
- `crates/starplayer-engine/tests/block_size_determinism.rs` — the invariant you are adding a
  scenario to. Read its module comment; it says why it must never be weakened.
- `crates/starplayer-offline/tests/render_allocation.rs` — the allocation hook.
- `crates/starplayer-engine/src/scope.rs` module comment — it says "there are no per-channel
  buses"; after this task that sentence is history and must be rewritten (deliverable 6).
- `plans/product/01-technical-architecture.md` §1.4, §7.2, §8; `CLAUDE.md`.

## Deliverables

### 1. `Stereo<T>` moves to `starplayer-dsp`

`crates/starplayer-mixer/src/path.rs` defines `Stereo<T>`; effects need it and dsp cannot depend
on mixer. Move the type (unchanged) to `crates/starplayer-dsp/src/frame.rs`, re-export it from
`starplayer_dsp` and keep `pub use starplayer_dsp::Stereo` in `starplayer_mixer::path` so every
existing path (`starplayer_mixer::Stereo`, `starplayer_mixer::path::Stereo`) still resolves.

### 2. The `Insert` trait (`crates/starplayer-dsp/src/insert.rs`)

```rust
/// Frames in one DSP block. The engine's `RENDER_QUANTUM` must equal this; the engine
/// asserts it at compile time.
pub const DSP_BLOCK_FRAMES: usize = 128;

pub struct ParamId(pub u8);

/// Integer parameters in fixed units, so the fixed path never sees a float.
pub struct ParamSpec { pub name: &'static str, pub unit: ParamUnit, pub min: i32, pub max: i32, pub default: i32 }
pub enum ParamUnit { CentiDecibels, Frames, Cents, Percent, Milliseconds, Ratio /* x100 */, Switch }

pub struct InsertDescriptor { pub name: &'static str, pub params: &'static [ParamSpec] }

pub trait Insert<Sample: DspSample>: Send {
    /// Process one whole block in place. `block.len() == DSP_BLOCK_FRAMES` always.
    fn process(&mut self, block: &mut [Stereo<Sample>]);
    /// RT-safe: a copy and the start of a smoothing ramp, never an allocation.
    fn set_param(&mut self, id: ParamId, value: i32);
    fn param(&self, id: ParamId) -> Option<i32>;
    /// Clear every delay line and envelope; parameters keep their values.
    fn reset(&mut self);
    fn descriptor(&self) -> &'static InsertDescriptor;
}
```

`DspSample` is H2's trait (`crates/starplayer-dsp/src/sample.rs`): `Copy + Default + Send`,
with `from_i16`, `add`, `sub`, `mul_q24(coefficient: i32)`, `scale_q15(gain: i32)`, `saturate`,
and `ZERO`. If H2 has not merged when you start, write the *minimum* of that trait you need
(`ZERO`, `add`, `scale_q15`) in `sample.rs` with the exact names above and `impl` for `f32`
and `i32` — the merge will be a union. Do not invent other names.

### 3. The gain insert (`crates/starplayer-dsp/src/effects/gain.rs`, `effects/mod.rs`)

One parameter, `gain` in centi-decibels from −6000 to +1200, default 0, smoothed. It is the
trait's first implementation, the determinism scenario's subject, and the reference for how an
effect smooths: it holds a `SmoothedParam` (deliverable 4) and applies `current()` per frame.
Decibel-to-linear is H2's `db_to_gain_q15` table function; if H2 is not merged, a local
`const` table for whole decibels with linear interpolation is acceptable and gets replaced in
the merge. `effects/mod.rs` also declares `InsertKind` (`Gain` now; H3/H4 add theirs) and
`pub fn build<S: DspSample>(kind: InsertKind, sample_rate_hz: u32) -> Box<dyn Insert<S>>`, the
one place a host names an effect. `alloc` is already a dependency of the crate.

### 4. Parameter smoothing (`crates/starplayer-dsp/src/smooth.rs`)

`SmoothedParam`: `steady(value)`, `set_target(value, frames)`, `current()`, `advance()`, `is_moving()`,
`snap()`. Linear in the parameter's own units, `i32` arithmetic with an `i64` accumulator or
a `GainRamp` inside — the point is the discipline, not a new interpolator: a value is a function
of frames elapsed since the change and lands exactly on the target. Default ramp length
`SMOOTH_FRAMES = 256` (two quanta; `RAMP_FRAMES` is 64 for a voice, which is too fast for a
filter cutoff). Test it the way `ramp.rs` tests `GainRamp`, including "however it is split".

### 5. Buses in the engine

- `VoicePool::accumulate_masked` gains a `buses: &mut [Path::Accumulator]` laid out as
  `bus_count × segment_len` windows plus the existing `destination` renamed `spill`; a voice
  whose `tag.channel < bus_count` accumulates into its bus window, otherwise into `spill`.
  Muted voices still go to `discard`. Keep the free-list bookkeeping textually as it is.
- `Engine` allocates `buses: Box<[Path::Accumulator]>` of `channel_count × RENDER_QUANTUM` in
  `with_settings` (persistent hosts pass `ChannelTable::MAX_CHANNELS`, so 64 × 128 frames;
  offline passes the module's count). The accumulator you have today becomes the pre-master
  mix.
- Per quantum: clear buses; segment loop as today, accumulating into bus windows at
  `offset..end`; then for each channel `c` in order: run chain `c` on the whole bus if it has
  any insert, add the bus into the mix; then the spill lane is added; then the master chain;
  then `Path::master` as today. Summation is `Path::Accumulator` addition — add a
  `MixPath::add_frame(destination, source)` (float `+=`, fixed `saturating_add`) rather than
  reaching into the fields.
- `const _: () = assert!(RENDER_QUANTUM == starplayer_dsp::DSP_BLOCK_FRAMES);`
- `MixPath` gains `type Mono: DspSample` as a bound (it already has the associated type) so
  `Box<dyn Insert<Path::Mono>>` is nameable from the engine.

### 6. The insert control plane

In `starplayer-engine`:

```rust
pub enum InsertTarget { Channel(ChannelId), Master }
pub enum InsertCommand<Sample: DspSample> {
    Install { target: InsertTarget, slot: u8, insert: Box<dyn Insert<Sample>> },
    Remove  { target: InsertTarget, slot: u8 },
    SetParam { target: InsertTarget, slot: u8, param: ParamId, value: i32 },
    Bypass  { target: InsertTarget, slot: u8, bypassed: bool },
    ResetAll,
}
pub struct InsertHandle<Sample> { /* Producer<InsertCommand<Sample>>, GarbageCollector<Box<dyn Insert<Sample>>> */ }
```

`Engine::take_insert_control()` hands the handle out once, like `take_control`. The engine
drains the insert ring in `render_quantum` right after `drain_commands`, bounded by
`MAX_COMMANDS_PER_QUANTUM`. `Install` into an occupied slot retires the old box over the
garbage channel (an `Err` from `retire` sets a new `EngineWarnings::retired_insert_dropped`
and drops in place, mirroring `retired_module_dropped`). `Remove` retires. `Command::LoadModule`
resets every insert (delay lines must not carry a previous song into the next); `ResetAll` is
for the host's seek. `MAX_INSERTS_PER_CHAIN = 4`. A chain is
`[Option<Box<dyn Insert<Path::Mono>>>; 4]` plus a bypass bit per slot; store chains in a
`Box<[Chain]>` of `channel_count + 1`. Ring and garbage capacities are two new
`EngineSettings` fields with defaults of 16 and 16.

Bypass is a bit the engine reads, not something the effect does, so bypassing is one branch and
a bypassed effect is still `reset` and still receives parameters.

### 7. Proof

- **Goldens**: `cargo xtask goldens --check` passes unchanged with the buses always on. If a
  golden moves, stop and report — do not regenerate. The likely cause is a saturation-order
  difference on the fixed path; the fix is to find the module and reason, not to accept it.
- **Block-size determinism**: `crates/starplayer-engine/tests/block_size_determinism.rs` gains
  a scenario with a gain insert on channel 1 and one on the master, a `SetParam` queued to
  land mid-song, and a `Remove` later, rendered at every block size on both paths. Byte-identical.
- **Allocation hook**: `render_allocation.rs` gets a case that renders with a gain insert
  installed on every channel and a stream of `SetParam`s. Zero allocations inside `render`.
- **Routing**: an engine test with two voices on two channels and a gain insert at −∞ (min)
  on channel 0 proves channel 0 is silent and channel 1 untouched, on both paths; a voice on a
  channel beyond the bus count still sounds (the spill lane).
- **Retirement**: installing over an occupied slot delivers the old box to the collector;
  nothing is dropped on the audio thread (the offline garbage-channel test pattern).
- `cargo test --workspace`, `cargo xtask ci --job host-tests`, `--job rt-safety`,
  `--job clippy`, `--job no-std-purity`, `--job wasm-build`, `--job fma-check`.

### 8. Documentation

- Architecture §7.2: replace the two-line "Per-channel insert chains and a master bus…"
  opening with what landed: buses always on and why the goldens hold, the `Insert` trait, the
  control ring, smoothing, and the fixed topology. Keep the IT filter subsection as it is.
- `crates/starplayer-engine/src/scope.rs` module comment: the taps still sample voice state,
  but the reason is now "the tap predates the buses and reading the bus is a follow-up", not
  "there are no buses".
- Append a `## Research resolution` section to this file.

## Research points

1. **Does slot-order → channel-major summation move any fixed golden?** Answer with
   `cargo xtask goldens --check`. Also render the three dense `data/m/*.it` modules from the
   pinned corpus (`cargo xtask conformance --fetch-only` caches it under
   `target/conformance/corpora/`) on the fixed path before and after and diff the bytes, since
   those are the mixes most likely to saturate an intermediate sum. Record what you found.
2. **Where does the MIDI lane's channel land?** `MIDI_CHANNEL_BASE = 48`
   (`crates/starplayer-engine/src/instrument.rs`); persistent hosts use 64 channels so lanes
   48–63 get buses of their own, but confirm the offline SMF path (`Player::load_smf`,
   `crates/starplayer-offline/tests/smf_block_size_determinism.rs`) sizes its engine so MIDI
   voices are not all in the spill lane — or state that they are and that it is fine.
3. **`Insert<Sample>` vs `Insert<Frame>`**: the trait is written over the mono sample type and
   processes `Stereo<Sample>` blocks. Confirm object safety with the `DspSample` bound and that
   `Box<dyn Insert<f32>>` is `Send` (the host builds it on another thread).
4. **The muted scratch**: muted voices currently render into `muted_scratch` so their state
   advances. Keep that; it is not a bus.

## Verification

```
cargo test --workspace
cargo test -p starplayer-engine --features telemetry
cargo xtask goldens --check
cargo xtask ci --job host-tests
cargo xtask ci --job rt-safety
cargo xtask ci --job clippy
cargo xtask ci --job no-std-purity
cargo xtask ci --job wasm-build
cargo xtask ci --job fma-check
```

## Out of scope

Any real effect (H3, H4); host or CLI surfaces (H7); SIMD (H6); reading the buses from the
scope taps; a general node graph; changing the master limiter.
