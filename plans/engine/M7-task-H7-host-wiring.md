# M7 — H7: Host wiring — `Player`, the CLI, the wasm host and the web page

| Field | Value |
|---|---|
| Milestone | M7 ([master plan](M7-master-plan.md), "The task graph" section) |
| Status | Ready once H3 and H4 have landed |
| Depends on | H1 (the insert control ring), H3, H4 (the effects to expose) |
| Blocks | M7 exit ("reverb on channel 1 alone, audible" — the owner needs a way to switch it on) |
| Parallel with | H6 |
| Recommended model | Claude Sonnet (plumbing through three hosts, each with an established pattern to copy) |
| Verified by | agent (host tests, wasm build, headless web harness, CLI render with an insert), then owner listening |

## Context for a fresh agent

The engine can now run per-channel and master insert chains (H1), and `starplayer-dsp` has six
effects behind `InsertKind` and `build` (H1 gain, H3 EQ/delay/chorus, H4 reverb/compressor).
Nothing outside the engine can reach them yet: `InsertHandle` is handed out by
`Engine::take_insert_control` and nobody takes it. This task threads it through every host so
the owner can hear a reverb on one channel — from the command line, from a rendered WAV, and
from the browser.

The patterns to copy already exist for MIDI input (M4-E6): `Player::send_event` and the event
clock in `crates/starplayer-host/src/player.rs`, `HostCommand`, the arms in
`crates/starplayer-host/src/engine.rs`; opcode 10 and `exports::set_midi_input` in
`crates/starplayer-host-wasm/src/lib.rs` (an install that allocates goes through a worklet
message task, a parameter change through the command ring); the `play --midi` flags in
`apps/starplayer-cli/`; the mixer panel in `apps/starplayer-web/www/{index.html,app.js}`.

### Code you must read before changing anything

- `crates/starplayer-engine/src/engine.rs` — `InsertCommand`, `InsertHandle`,
  `take_insert_control`; `crates/starplayer-dsp/src/effects/mod.rs` — `InsertKind`, `build`,
  descriptors.
- `crates/starplayer-host/src/{player,engine,lib}.rs` and `tests/player.rs`.
- `crates/starplayer-host-wasm/src/{lib,backend}.rs`, `apps/starplayer-web/www/*`,
  `apps/starplayer-web/test/headless.mjs`.
- `apps/starplayer-cli/src/*` (`play`, `render`), `crates/starplayer-offline/src/lib.rs`
  (`render_song` and the WAV writer in `wav.rs`).
- `plans/product/01-technical-architecture.md` §9.2 (the wire protocol), §11; `CLAUDE.md`.

## Deliverables

### 1. `starplayer-host::Player`

```rust
pub fn install_insert(&mut self, target: InsertTarget, slot: u8, kind: InsertKind) -> Result<(), HostError>;
pub fn remove_insert(&mut self, target: InsertTarget, slot: u8) -> Result<(), HostError>;
pub fn set_insert_param(&mut self, target: InsertTarget, slot: u8, param: ParamId, value: i32) -> Result<(), HostError>;
pub fn bypass_insert(&mut self, target: InsertTarget, slot: u8, bypassed: bool) -> Result<(), HostError>;
pub fn inserts(&self) -> &InsertLayout;          // what the host believes is installed, per target/slot
```

`install_insert` builds the box on the caller's thread for the engine's path (the arm knows
its `Path::Mono`), queues `InsertCommand::Install`; retired boxes are collected in
`collect_garbage` alongside modules. `set_mixer_mode` (a rebuild) re-installs the layout into
the new engine, and `seek_*` sends `ResetAll`. A `player.rs` test installs a reverb on channel
1, renders, and asserts the other channels' output is identical to a render without it (the
same assertion as H4's offline exit test, through the host).

### 2. The CLI

`starplayer play` and `starplayer render` accept, repeatably:

```
--insert <target>:<effect>[:<param>=<value>,...]     e.g. --insert 1:reverb:room=60,mix=40
                                                          --insert master:compressor:threshold=-1800,ratio=400
--list-effects                                        prints every effect, its parameters, units, ranges, defaults
```

`<target>` is a 1-based channel number or `master`; unknown names and out-of-range values are
errors naming the descriptor's range. `render --golden` refuses any `--insert` (the goldens
are DSP-bypassed by policy §5.5). `render_song` gains an `inserts: &[InsertSpec]` parameter
(or a builder) so the CLI's render and the offline tests share the code path.

### 3. The wasm host and the web page

- `exports::set_inserts(layout_json)` or a typed equivalent, called from a worklet message
  (`{ type: 'inserts', ... }`) exactly as `set_midi_input` is, because building allocates;
  `OPCODE_INSERT_PARAM = 11` with `argument` packing target (bits 0–7), slot (8–11) and
  param (12–19), `extra` the `i32` value; `OPCODE_INSERT_BYPASS = 12`. `ring.js` mirrors the
  constants.
- The page gets an **Effects** panel beside the Mixer panel: a target select (channels 1–N from
  the loaded module, plus Master), four slot rows each with an effect select (`none` + the
  six) and, for the chosen effect, one range input per parameter generated from the
  descriptor exported by the wasm host (name, unit, min, max, default). Changes send
  `INSERT_PARAM`; choosing an effect sends the `inserts` message. The compressor's
  gain-reduction read-back is shown as a bar from telemetry if H4 exposed it through the
  snapshot, otherwise omitted.
- `headless.mjs` gains a case: load the fixture, install a reverb on channel 1, play two
  seconds, assert the page reports it installed and the audio peak did not drop to zero.

### 4. Telemetry

`Snapshot` gains nothing in this task unless H4's gain reduction needs a home; if it does, add
a `master_gain_reduction_centi_db: i16` and bump `TELEMETRY_HEADER_WORDS` with `ring.js`.

### 5. Documentation

Architecture §11 (host crates) and §9.2 (opcodes 11–12); `apps/starplayer-web/README.md` and
the CLI's `--help`; `plans/README.md`'s web-player row. Append `## Research resolution`.

## Research points

1. Where the insert layout lives across a `set_mixer_mode` rebuild — in `Player` (re-applied)
   or rebuilt by the caller; the answer is `Player`, but confirm the arm's `Path::Mono` is
   reachable from the `define_arms!` macro without a second macro.
2. The web page's CSP and bundle size after adding descriptors — keep them in the wasm host,
   not duplicated in JavaScript.

## Verification

```
cargo test --workspace
cargo test -p starplayer-host
cargo run -p starplayer-cli -- --list-effects
cargo run -p starplayer-cli -- render <fixture.s3m> --insert 1:reverb:room=60,mix=50 -o /tmp/reverb.wav
cargo xtask ci --job host-tests
cargo xtask ci --job wasm-build
cargo xtask ci --job clippy
cargo xtask wasm && node apps/starplayer-web/test/headless.mjs
```

## Out of scope

SIMD (H6); presets or saving layouts; the TUI (A1); MIDI-controlled parameters (a later
milestone can map CC to `set_insert_param`).

## Research resolution

### 1. Where the insert layout lives across a `set_mixer_mode` rebuild

**`Player`, as the task file expected**, and the arm's `Path::Mono` is reachable from the
existing `define_arms!`/`render_arm!` machinery without widening `define_arms!` itself.
`crates/starplayer-host/src/engine.rs`'s `EngineArm::build` already matches on
`($path_kind, $interpolator_kind, $channels)` per arm and — inside that same match arm —
now also calls `engine.take_insert_control()` and wraps it through one small new macro,
`insert_control_variant!`, that dispatches on the same `$buffer` token (`float` or `fixed`)
`render_arm!` already uses to tell the two mix paths apart:

```rust
macro_rules! insert_control_variant {
    (float, $handle:expr) => { HostInsertControl::Float($handle) };
    (fixed, $handle:expr) => { HostInsertControl::Fixed($handle) };
}
```

So this *is* a second macro, in the literal sense the research point's phrasing raised as
a worry — but not a second **dispatch**: it does not add a parameter to `define_arms!`'s
own signature, does not touch any of the sixteen arm definitions, and reuses a token that
was already being threaded through the macro for an unrelated reason (`render_arm!`'s own
float/fixed split). `HostInsertControl` (`crates/starplayer-host/src/insert.rs`) is the
runtime enum a `Player` holds without itself being generic over the sample type — the same
device `EngineArm` already is for the engine itself, for the same reason: which mix path a
`MixerMode` resolves to is a runtime choice, not a compile-time one a host's public type
can carry.

`Player` holds this handle directly, **not** through `HostCommand` — unlike
`Command::LoadModule`, an insert change has no transport ramp to sequence around, so there
is nothing for `RenderState`'s translation step to add. `install_insert` builds the effect
on the caller's thread (`HostInsertControl::install`) and pushes it onto the engine's own
insert ring from there. `InsertLayout` (also `insert.rs`) is `Player`'s own record of what
it asked for — install, every parameter set away from the default, the bypass bit — and
`Player::rebuild` (the common path behind both `reopen` and `set_mixer_mode`) replays it
into the freshly opened `Player` after the module is reinstalled, before the caller's
requested resume. `crates/starplayer-host/tests/player.rs`'s
`the_insert_layout_survives_a_mixer_mode_rebuild` proves it: install a reverb, set a
parameter, switch float → fixed, and both the layout's own readback and a subsequent
render still produce audio.

One consequence worth recording because it was not asked for and is easy to miss: the
**browser host gets this for free**. `starplayer-host-wasm`'s `Host::set_mixer_mode`
already just calls `self.player.set_mixer_mode(...)`, so the wasm crate needed no insert
replay logic of its own — the web page's own `insertLayout` map never has to resend
anything across a mixer-mode change either, because the Rust side already remembered.

### 2. The web page's CSP and bundle size after adding descriptors

**Kept in the wasm host, not duplicated in JavaScript, as the research point asked**, and
verified rather than assumed: `crates/starplayer-host-wasm/src/lib.rs`'s
`exports::effects_json()` is the *only* place any effect's name, parameter names, units,
ranges or defaults are spelled out. It walks `InsertKind::ALL`, builds one instance of each
effect at a throwaway sample rate purely to read its `'static` `InsertDescriptor`, and
formats a JSON array by hand with `write!` — no `serde` or `serde_json` dependency added,
since the shapes involved (strings, small integers) do not need one and pulling one in
would be exactly the kind of duplicated-elsewhere risk this research point exists to avoid.
`apps/starplayer-web/www/worklet-processor.js` carries that one string on its very first
`'ready'` message; `app.js` parses it once into `state.effects` and builds every select,
slider, label, min/max and default from it. No CSP change was needed: `effects_json`
crosses exactly the way the English effect-name table already does (architecture §9.2,
"What does **not** cross is `EffectDisplay::name`") — a string on an existing message, not
a new resource, a new origin, or a new script source, so the page's existing
`connect-src`/`script-src` policy is untouched.

Bundle size: `cargo xtask wasm`'s own report is the number that matters, since the effects
JSON is generated at runtime, not embedded as static JavaScript.
`starplayer_host_wasm_bg.wasm` (the worklet's own wasm binary, which is where
`effects_json`, `install_insert` and `remove_insert` live, alongside the six effects
themselves) is 686,958 bytes after this task, uncompressed; `app.js` — hand-written
JavaScript, including the whole Effects panel — is 97,091 bytes. Neither figure was
measured *before* this task on this branch (H1–H5 already added the six effects'
implementation code to the same wasm binary, which dwarfs anything H7 itself added), so
there is no "before/after H7" delta to report that would mean anything; the wasm binary
size is dominated by the DSP code the effects already needed, and `effects_json`'s own
compiled footprint — one function that walks a `[InsertKind; 6]` and formats strings — is
not separately measurable from that total.

## Done differently from the task file, and why

1. **`InsertKind` gained `ALL`, `name()` and `from_name()`** (`crates/starplayer-dsp/src/effects/mod.rs`),
   not asked for by name in the deliverables but needed by every host surface that has to
   enumerate or parse an effect: the CLI's `--insert`/`--list-effects`, the wasm host's
   `effects_json`/`install_insert`, and the tests for both. Putting the table in the dsp
   crate root — where `InsertKind` itself lives — means a host never transcribes the six
   names, and adding a seventh effect later is one line in one place rather than a name
   string repeated in three crates.

2. **`render_song` gained a sibling, `render_song_with_inserts`, rather than a literal new
   parameter on itself.** The task file permits this explicitly ("or a builder");
   `render_song` is called from `~10` sites across `starplayer-offline`'s own tests, its
   `render_song_fixed_mono` wrapper, and `starplayer-testkit`'s perceptual harness, none of
   which need to know about inserts. `render_song` is now four lines that call the new
   function with an empty slice, so the two cannot drift apart, and
   `render_song_with_inserts` is what the CLI's `--insert` and a new offline test
   (`a_render_with_an_insert_installed_is_still_block_size_independent`) both call.

3. **`starplayer_telemetry::WarningFlags` gained `retired_insert_dropped`.** Not asked for
   in this task's own deliverable text (deliverable 4 only mentions the compressor's gain
   reduction), but explicitly deferred here by H1's own research resolution: *"widening it
   is host-surface work and belongs with the rest of H7."* `Player::warnings()` had a
   comment saying exactly that; this task removes the comment along with the gap it
   described. The wasm host's packed warnings word gained bit 5 for it.

4. **The compressor's gain-reduction meter is not shown anywhere** — not in `Player`'s
   surface, not over the wire, not in the web page. Deliverable 3 names the fallback
   explicitly ("shown as a bar from telemetry if H4 exposed it through the snapshot,
   otherwise omitted") and H4 did not: it exposed the reduction only through
   `Insert::param(ParamId::GAIN_REDUCTION)`, which nothing on the insert control ring's
   *read* side exists to answer (the ring is one-way, control → audio, by design — see
   `starplayer-dsp/src/insert.rs`'s own module doc on why an effect arrives by command).
   Wiring a read path across that ring, or adding a `master_gain_reduction_centi_db` field
   to `Snapshot` and threading it through every tick, is exactly the scope deliverable 4
   was written to let this task skip.

5. **The CLI's `--insert` target is 1-based** (`1` is the first channel, `ChannelId(0)`),
   exactly as the deliverable states, via a subtraction in `apps/starplayer-cli/src/insert_arg.rs::parse_target`
   — the one place it happens. Worth recording because it reads differently from
   `starplayer-offline/tests/reverb_exit_criterion.rs`'s own internal use of `ChannelId(1)`
   for what its comments call "channel 1": that test is engine-level code naming a
   `ChannelId` directly and was never bound by a "1-based" convention, whereas the CLI is a
   flag a person types and "the first channel is 1" is what that person expects. The two
   are not the same channel. **The exact invocation an owner should use to hear the exit
   criterion through the CLI is `--insert 1:reverb:...`**, which lands on `ChannelId(0)` —
   `reflex.s3m`'s first channel — not the second channel H4's own fixture exercises.
   Both are audible; they are simply not the same one, and a listener comparing the CLI's
   output against H4's own measured numbers should know that going in.

6. **`--insert`'s grammar has no slot field**, because the deliverable's own two examples
   (`--insert 1:reverb:room=60,mix=40`, `--insert master:compressor:...`) never show one.
   Repeating `--insert` for the *same* target fills that chain's four slots in the order
   given (`insert_arg::parse_insert_args`'s `next_slot` map); a fifth repeat for one target
   is refused rather than silently overwriting the first, naming the four-slot limit in the
   error.

7. **Insert wire encoding, chosen but not specified by the task file**: `InsertKind`'s wire
   value is its position in `InsertKind::ALL` (gain 0 … compressor 5); a target's wire byte
   is a channel index `0..64` or the reserved value `0xFF` for the master bus
   (`INSERT_TARGET_MASTER`, `channels_count` being `ChannelTable::MAX_CHANNELS`, comfortably
   below `0xFF`). Both are shared verbatim between `OPCODE_INSERT_PARAM`/`OPCODE_INSERT_BYPASS`,
   `exports::install_insert`/`remove_insert`, and `effects_json`'s own `"wire"` field, so a
   host never has two numberings to reconcile.

8. **A top-level `starplayer --list-effects`, needing no subcommand**, was added beside the
   per-subcommand `render --list-effects` / `play --list-effects` the deliverable text
   describes, because this task's own Verification section names the literal invocation
   `cargo run -p starplayer-cli -- --list-effects` — no subcommand — which the per-subcommand
   flags alone cannot satisfy. `Cli::command` became `Option<Command>`.

## Verification results (2026-09-06)

All commands pass on branch `h7`, at commit `5af9c30`:

```
cargo test --workspace                              ok (every crate, 0 failures)
cargo test -p starplayer-host                        ok (22 + 1 doctest)
cargo run -p starplayer-cli -- --list-effects        ok (prints all six effects)
cargo run -p starplayer-cli -- render <fixture> \
    --insert 1:reverb:room=60,mix=50 -o out.wav      ok (audibly differs from a plain render)
cargo xtask ci --job host-tests                      ok
cargo xtask ci --job wasm-build                      ok
cargo xtask ci --job clippy                          ok (-D warnings, whole workspace)
cargo xtask wasm && node apps/starplayer-web/test/headless.mjs
                                                      ok (sab, fallback, plain — all 60 s)
```

Also run, beyond the required list, because they touch what this task changed:

```
cargo xtask goldens --check                          ok (11/11 byte-identical)
cargo xtask ci --job rt-safety                       ok (including two insert-specific
                                                          allocation tests already landed by H3/H4)
cargo xtask ci --job no-std-purity                   ok
cargo xtask ci --job fma-check                        ok
node apps/starplayer-web/test/ring-harness.mjs       ok
node apps/starplayer-web/test/worklet-harness.mjs    ok
```
