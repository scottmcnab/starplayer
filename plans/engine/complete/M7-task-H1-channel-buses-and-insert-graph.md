# M7 — H1: Channel buses, the `Insert` trait, and the install/parameter plumbing

| Field | Value |
|---|---|
| Milestone | M7 ([master plan](M7-master-plan.md), "The task graph" section) |
| Status | Landed 2026-09-05 |
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

## Research resolution

### 1. Does slot-order → channel-major summation move any fixed golden?

**No. Nothing moved.**

`cargo xtask goldens --check` passes with all nine hashes unchanged, before and after the
buses were switched on:

```
mod/synthetic   c5adc0cc55f95e9147a24530e9c4d0803f9c9930a59ad8107ac386ff7c3eb82a
mtm/synthetic   2ca02ee1d5ec67fb6b603ba28950cccc0a6044d44ce9f4836b3e83a5345dcabb
xm/synthetic    20beedc6f28ef53716dd63563789d70325ec50cf2350da907e236dd22fd48039
it/synthetic    d32490e9248c4edc2ea01945f4bb9a84f6aab38c8bb36ec5031c20a853b69c74
s3m/armani      59977a266576827a8fd5614b75688ee3391ebe640c0a97d43f15b4a5fde8687f
s3m/movement    cc1974494ca57ac555348007dbffd8c292823171e38a46c8fcf82e46b3b9ca5e
s3m/nicetune    89f26304021c7466a37798a5efd02bbd5e233fa1aab545161355992cbd127bc1
s3m/petri       eaa4842ed00bfc48bcc35948dcfe635ec0d7f472cb59eae05cd3632d1f73412b
s3m/reflex      bab4413c73d6d4da3681e088af0e761ec50ce7e2ec6fbd9885d28681f4a5e625
```

The dense-IT check the task asks for was run through a throwaway
`crates/starplayer-offline/examples/dense_it_hash.rs` (deleted afterwards, not committed)
that renders `starplayer_offline::canonical_sha256(GoldenFormat::It, …, 128)` — the exact
fixed-path, mono `i16`, 44.1 kHz, 10-second golden contract — over the three
`target/conformance/corpora/libxmp-…/test-dev/data/m/*.it` modules. Identical before and
after:

```
941351df7dbeb40149f080e8a1fbd88eef6b17ad876aa7301fdfe078c5095e0a  4th_Symmetriad.it
c7623119c81e86720e6d4c4d5d604e486fdba36c46e842fb071327822225943a  Fight2.it
c15eb6c840eecdc295a053d99bd80af511789c606bc8f7613b7b33ad621b427f  another life.it
```

Why it holds, restated so it can be relied on rather than re-measured: within one bus the
voices are still walked in slot order, and across buses `i32` saturating addition is
ordinary two's-complement addition — which is associative and commutative — unless an
intermediate sum reaches ±2³¹. The accumulator is on the raw `i16` scale, so that is
65,536× full scale; a module would need tens of thousands of simultaneous voices at full
volume to reach it. The float path's last bits may move, and nothing pins them.

### 2. Where does the MIDI lane's channel land?

`MIDI_CHANNEL_BASE = 48` and `MIDI_CHANNEL_COUNT = 16`, so the MIDI lanes are 48–63 and
need a 64-lane engine to have buses of their own. Every path that plays MIDI already sizes
itself that way, and none of them was changed:

* `crates/starplayer-offline/src/lib.rs` `render_smf_song` passes
  `channel_count: ChannelTable::MAX_CHANNELS` with the comment already explaining why (the
  MIDI lanes sit above a module's own). `tests/smf_block_size_determinism.rs` goes through
  that entry point, so its MIDI voices each land on their own bus, lanes 48–63.
* `crates/starplayer-offline/tests/jam_determinism.rs` also uses
  `ChannelTable::MAX_CHANNELS`.
* `crates/starplayer-host/src/engine.rs` `HostEngine::build` — which is what
  `Player::load_smf` renders through — takes `MAX_VOICE_CAPACITY` and
  `ChannelTable::MAX_CHANNELS` because a persistent host builds its engine before it has
  seen a module.

So no MIDI voice is in the spill lane on any shipping path, and each of the sixteen MIDI
lanes can carry its own insert chain from this task onwards. The spill lane exists for the
*narrow* engine case — an offline tracker render sized to a four-channel MOD, given a voice
tagged for a lane it does not have — and it is a sum into the mix, not a discard:
`a_voice_beyond_the_bus_count_is_still_heard` in `tests/insert_graph.rs` pins that. What a
spilled voice loses is the chance to have an insert on it, nothing else.

### 3. `Insert<Sample>` vs `Insert<Frame>`

The trait is written over the **mono** sample type and processes `&mut [Stereo<Sample>]`
blocks, as the task specifies. Object safety and `Send` are both proved by compilation
rather than by argument: `crates/starplayer-dsp/src/effects/mod.rs` returns
`Box<dyn Insert<Sample>>` from `build_insert`, `crates/starplayer-engine/src/insert.rs`
stores `[Option<Box<dyn Insert<Sample>>>; 4]`, and
`effects::gain::tests::a_boxed_insert_is_send` asserts `Box<dyn Insert<f32>>: Send` through
a `fn assert_send<T: Send>` bound. `Send` comes from the supertrait, so every trait object
has it without each effect restating it.

Writing the trait over the frame type instead would have made `Insert<Stereo<f32>>` the
name a host says, and would have put the stereo-ness into the type parameter rather than
into the signature — which H3's mid/side EQ and H4's stereo reverb both need to see. The
mono parameter also makes `MixPath::Mono` the single seam: it is already the type the voice
filter is written in.

The one thing it needed was a way to get from `Path::Accumulator` to `Stereo<Path::Mono>`.
They are the same type on both paths, but the trait cannot say so without an equality bound
(`MixPath<Accumulator = Stereo<Self::Mono>>`) that every caller of `MixPath` — the engine,
both hosts, the offline crate, every test — would then have to carry in its own where
clause. `MixPath::as_frames` hands the slice straight back instead: one line per
implementation, no bound, and it compiles to nothing.

### 4. The muted scratch

Kept exactly as it was. `muted_scratch` is one `RENDER_QUANTUM` buffer that muted voices
render into so their position, loops, ramps and filter state advance as if they were
audible; it is never read and it is not a bus. `accumulate_masked` still checks `is_muted`
**before** it looks for a bus, so a muted voice never reaches one — which is what keeps
`resonant_filter.rs`'s `a_muted_voices_filter_keeps_its_delay_line_moving` true and what
makes unmuting resume mid-note without a click.

## Done differently from the task file, and why

1. **`accumulate_masked` takes a `BusSegment` view, not a `buses: &mut [Path::Accumulator]`
   laid out as `bus_count × segment_len`.** The two sentences in deliverable 5 pull in
   different directions: the engine's buses are `channel_count × RENDER_QUANTUM` and a
   segment "accumulat[es] into bus windows at `offset..end`", which is a strided view, not a
   `bus_count × segment_len` contiguous block. Making the literal signature work would have
   meant a `channel_count × span` scratch per segment plus a copy back into the quantum-wide
   buses — real work per segment, for nothing. `BusSegment<'_, Accumulator>`
   (`crates/starplayer-mixer/src/voice.rs`) carries the slice, the stride, the offset and
   the frame count and hands out one window at a time; `BusSegment::none()` is the "no buses"
   view that `VoicePool::accumulate` and the mixer's own tests use. It allocates nothing,
   which a `&mut [&mut [_]]` built per segment would not have managed.

2. **The spill lane has its own `RENDER_QUANTUM` buffer.** Deliverable 5 renames
   `destination` to `spill` and then says the buses are summed into the mix first and "then
   the spill lane is added". Those cannot both be the same buffer, so `Engine` has
   `accumulator` (the pre-master mix), `buses` and `spill`. The extra buffer is 128 frames.

3. **`build` is exported as `build_insert`.** A bare `build` in `starplayer_dsp`'s root
   re-export is too generic a name for a crate whose root is `use`d wholesale;
   `effects::build` is still there under its own module path.

4. **`db_to_gain_q15` lives in `effects/gain.rs` for now**, as deliverable 3 permits, with
   a 73-entry `i32` table of whole decibels from −60 to +12 and linear interpolation of the
   centi-decibel remainder. Two conventions were fixed here and should survive the H2 merge
   because the rest of this task depends on them:
   * **Q15 unity is 32768, not 32767.** A power of two makes `DspSample::scale_q15` at unity
     the *identity* on both paths, which is what lets an insert sitting at its default leave
     a bus bit-identical — the property that makes "buses always on with a chain installed"
     safe to reason about.
   * **`GAIN_MIN_CENTI_DB` (−6000) is −∞, not −60 dB.** The bottom of the fader is silence,
     so a host has somewhere to put the slider and the routing tests have a value that
     really mutes a bus.

5. **The block-size determinism scenario stops on its phase boundaries.** An insert command
   has no scheduled frame the way a `ScriptedAction` has — it takes effect when the audio
   thread next drains the ring — so pushing it "after N host calls" makes the *input*
   differ between block sizes and the test fails for the wrong reason (it did, first time:
   at block size 3 the `SetParam` landed at frame 5248 instead of 5120). The render loop
   therefore clamps to each phase boundary, which is a whole number of quanta and hence a
   point where the output ring is empty and the engine has rendered exactly that many
   frames whatever the host asked in. The block size still varies everywhere else, and the
   256-frame smoothing ramp the `SetParam` starts runs across two quanta of ordinary
   block-split rendering after it. The invariant is not weakened: it is still "the same
   control timeline produces byte-identical output at every block size".

6. **`EngineWarnings::retired_insert_dropped` is not published through telemetry.**
   `starplayer_telemetry::Warnings` is a separate type in a separate crate with its own
   `any()` and a bit-packing in the wasm host; widening it is host-surface work and belongs
   with the rest of H7. `crates/starplayer-host/src/player.rs` now fills the field from
   `EngineWarnings::default()` with a comment saying so. The flag is set, is in
   `EngineWarnings::any()`, and is asserted on directly in
   `tests/insert_graph.rs::a_full_garbage_channel_raises_a_warning_rather_than_leaking`.

7. **`GAIN_Q15_TABLE` is indexed directly rather than through `get`.** `<[T]>::get` is not
   a `const fn`, and `db_to_gain_q15` is a `const fn` so an effect can be built at a value
   in a `const` context. Both indices are proved in range by the clamps immediately above
   them, and a `const _: () = assert!(…)` pins the 0 dB entry to `Q15_UNITY`. Every other
   index in the task's code goes through `get`/`get_mut`, and `clippy::indexing_slicing`
   stays denied in `starplayer-mixer` and `starplayer-engine`.

## Verification results (2026-09-05)

All nine commands pass on branch `h1`:

```
cargo test --workspace                     ok (76 test binaries, 0 failures)
cargo test -p starplayer-engine --features telemetry   ok
cargo xtask goldens --check                ok (9/9 byte-identical)
cargo xtask ci --job host-tests            ok
cargo xtask ci --job rt-safety             ok
cargo xtask ci --job clippy                ok
cargo xtask ci --job no-std-purity         ok
cargo xtask ci --job wasm-build            ok
cargo xtask ci --job fma-check             ok
```
