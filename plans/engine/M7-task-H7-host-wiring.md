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
