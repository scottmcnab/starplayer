# M9 — J1: The CLAP instrument — `starplayer-clap` as a `Player` over a `ClapBackend`

| Field | Value |
|---|---|
| Milestone | M9 ([master plan](M9-master-plan.md), "The task graph" section) |
| Status | Planned; pull-driven (not scheduled) |
| Depends on | M4 (landed): `Player::midi_only`/`jam`, `ExternalEventQueue`, `InstrumentRack` |
| Blocks | J2, J4; the M9 exit criterion |
| Parallel with | J3 |
| Recommended model | Claude Opus (an FFI boundary, a plugin lifecycle on two threads, and the one place `unsafe` may be unavoidable) |
| Verified by | agent (`clap-validator` clean, an offline harness driving `process()` byte-identical to `Player`, the block-size invariant through the plugin), then owner in a DAW |

## Context for a fresh agent

A CLAP host calls `process()` on its audio thread with a block of output buffers and a
sorted, sample-accurate list of input events (notes, MIDI, parameter changes, transport).
That is exactly the shape StarPlayer's musical path was built to accept: architecture §3.2
admitted "pre-materialised buffers" at the edge for precisely this consumer, and M4 landed
the pieces — `ExternalEventQueue` (`crates/starplayer-engine/src/instrument.rs`),
`MidiSource`, `InstrumentRack`, and `starplayer_midi::MidiDecoder` for raw MIDI bytes.

The host-side player already exists too. `starplayer_host::Player`
(`crates/starplayer-host/src/player.rs`) owns an engine arm, the command ring, the seek
mailbox, mixer-mode rebuilds, live input, and telemetry, and it is built over
`AudioBackend` (`crates/starplayer-host/src/backend.rs`), a **push** model: `open` hands the
backend a `RenderCallback = Box<dyn FnMut(&mut [f32]) + Send>` and the backend calls it
with interleaved frames whenever it wants audio. `crates/starplayer-host/tests/player.rs`
drives a `ManualBackend` exactly that way, and D9 put the AudioWorklet behind the same
trait. A CLAP `process()` is one more push. Master-plan decision 2 therefore says: **the
plugin is a `Player` over a `ClapBackend`**, and no engine code changes.

Master-plan decision 1 answers Q5 with `clack`. Read it, then verify it (research point 1)
before adding the dependency.

### Code you must read before changing anything

- `crates/starplayer-host/src/{backend,player,engine,events,lib}.rs` and
  `tests/player.rs` (`ManualBackend`, `open`, the determinism test through the host).
- `crates/starplayer-host-cpal/src/lib.rs` and `crates/starplayer-host-wasm/src/backend.rs`
  — the two existing backends; yours is the third.
- `crates/starplayer-engine/src/instrument.rs` — `MidiSource`, `ExternalEventQueue`,
  `TimedEvent`, `MIDI_CHANNEL_BASE`; `crates/starplayer-midi/src/codec.rs`.
- `crates/starplayer-host/src/events.rs` — `EventSender`, `EventClock` — to see what you
  are *not* using (decision 3).
- `apps/starplayer-cli/src/*` `play` — the `--midi` path, for the mode surface.
- `plans/product/01-technical-architecture.md` §2.3, §3.2, §3.3, §11; `CLAUDE.md`
  (the `unsafe` rule: "unless absolutely necessary" — an FFI boundary is; keep it in one
  module with a `// SAFETY:` on every block, and prefer `clack`'s safe API everywhere it
  reaches).

## Deliverables

### 1. `crates/starplayer-clap` (std, `cdylib` + `rlib`)

A workspace member producing `starplayer.clap` (a `cdylib` renamed by `xtask`), with
`clack-plugin` and `clack-extensions` pinned. The `rlib` target exists so the offline
harness (deliverable 5) can drive the plugin in-process.

- **Descriptor**: id `com.starplayer.instrument` (the owner may rename before release),
  features `instrument`, `sampler`, `stereo`.
- **Ports**: one stereo `f32` audio output; one note input port accepting CLAP note events
  **and** MIDI (`CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI`). No audio input in J1.
- **Activation**: `activate(sample_rate, min_frames, max_frames)` opens the `Player` on
  the `ClapBackend` with `AudioSpec::stereo(sample_rate)` and `MixerMode::DEFAULT`;
  `deactivate` closes it. Reactivation at a new rate is a `Player::reopen`.
- **`ClapBackend: AudioBackend`**: `devices()` returns one entry ("host"), `negotiate`
  returns the host's spec, `open` stores the `RenderCallback` behind the audio-thread
  handle. `process()` de-interleaves: call the callback into a scratch interleaved
  `Vec<f32>` sized once at activation to `max_frames × 2`, then copy out to the planar
  host buffers. Zero allocation in `process`.
- **Transport**: host `is_playing` edges map to `Player::play`/`stop`; a host seek to
  song position 0 while stopped maps to `seek_order(0)`. Host tempo is **not** applied to
  the module (its tempo is its own); recorded as a parameter for J2 to expose as a
  choice if the owner wants tempo-follow later.
- **Events**: each input event with sample offset `t` becomes a `TimedEvent` at
  `block_start + t` on the `ExternalEventQueue` via `Player::take_event_sender` — **not**
  through `send_event`, whose lead is for live input. CLAP note events (`note_on`,
  `note_off`, `note_choke`, `note_expression` velocity/pressure) map straight to `Event`;
  MIDI events go through `MidiDecoder`. Channel is the CLAP channel (0–15), mapped by the
  rack to `MIDI_CHANNEL_BASE + channel`. Events must reach the queue **before** the block
  renders; the plugin pushes them all, then renders.
- **Modes** (a plugin parameter in J2; a compile-time default of `Instrument` here):
  `Module` (the loaded module plays under host transport, notes ignored),
  `Instrument` (`Player::midi_only`: host notes play the module's instruments, the song
  does not), `Jam` (`Player::jam(true)`: both).
- **Loading a module**: `starplayer-clap` has no GUI; J1 loads from a path given by the
  `STARPLAYER_MODULE` environment variable at activation (a development convenience,
  documented as such) and from state in J2. `Player::load` runs on the main thread;
  the `Command::LoadModule` handoff is already RT-safe.
- **Latency**: 0. The output ring carries a partial quantum between calls but adds no
  delay to a frame's timing. Assert it in the harness (deliverable 5).
- **Garbage**: `on_main_thread` (the host's main-thread callback, requested via
  `host.request_callback()`) runs `Player::collect_garbage`; the audio thread never drops
  a module.

### 2. `xtask clap`

Builds the `cdylib` in release, renames it to `starplayer.clap` under `target/clap/`, and
on Linux writes it into `~/.clap/` when given `--install`. `xtask ci --job clap-build`
builds it (no install).

### 3. Real-time safety

`process()` is the audio callback: no allocation, no lock, no panic. Extend
`crates/starplayer-offline/tests/render_allocation.rs`'s pattern with a test in the clap
crate that drives `process()` under the allocation hook with a burst of note events.

### 4. Documentation

Architecture §11: add `starplayer-clap` to the std table; §12: answer Q5 in the table with
a pointer to the master plan decision. `plans/README.md` M9 row. A `README.md` in the crate
saying how to build, install and load it in a host, and what `STARPLAYER_MODULE` is for.

### 5. Proof

- **`clap-validator`** (free-audio's Rust tool, `cargo install clap-validator`) passes on
  the built bundle with no failures; warnings listed in the resolution.
- **Offline harness** (`crates/starplayer-clap/tests/process.rs`, in-process through the
  `rlib`): a fake host activates the plugin at 48 kHz, loads the S3M fixture in `Jam`
  mode, feeds a fixed note script at sample offsets, and calls `process()` at block sizes
  1, 3, 64, 128, 4096, 8191. All six outputs are byte-identical to each other **and** to a
  `Player`-over-`ManualBackend` render of the same script — the plugin adds nothing.
- Transport edges start and stop the song at the right frame (telemetry `song_frame`).
- `cargo xtask ci --job clap-build`, `--job clippy`, `--job host-tests`.

## Research points

1. **Pin `clack`.** Confirm the published versions of `clack-plugin`, `clack-host`,
   `clack-extensions` and `clap-sys` on crates.io, their MSRV against the pinned 1.97
   toolchain, and whether the plugin side needs any `unsafe` in *our* code. If `clack` is
   git-only or unpinnable, fall back to `clap-sys` with the FFI confined to
   `crates/starplayer-clap/src/ffi.rs` and say so in the resolution.
2. **Note dialects**: whether to prefer CLAP note events over MIDI when a host offers both
   (yes — velocity is a `f64`, which `U0F16` keeps better than 7 bits; §2.3), and how
   `note_id` maps to the rack's held notes (it does not need to: the rack keys by note).
3. **Where the module comes from in a host with no GUI**: env var now, state in J2; check
   whether CLAP's `preset-load` extension is the honest long-term home and note it for J2.
4. **Which hosts to test in**: Bitwig, REAPER and Ardour all load CLAP on Linux; the WSL2
   box has no display, so the owner's DAW check is an acceptance item — say so.

## Verification

```
cargo test -p starplayer-clap
cargo xtask clap
clap-validator validate target/clap/starplayer.clap
cargo xtask ci --job clap-build
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
cargo xtask ci --job rt-safety
```

## Out of scope

State and parameters (J2); a GUI; hosting effects (J3); VST3 (J4); tempo-follow; audio
input; note expressions beyond velocity and pressure.
