# M1-task-B9 — Output device and mixer options in the web player

| Field | Value |
|---|---|
| Milestone | M1 follow-up ([master plan](M1-master-plan.md)) |
| Depends on | B7 (web player), B5 (mixer output formats) |
| Blocks | — (M3's CLI/offline renderer reuse the mode enum) |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests + headless browser) + **owner (listens at 8-bit / 11 kHz and grins)** |

## Context for a fresh agent

Read `AGENTS.md` first (working agreements: full variable names, compact formatting,
one-line asserts, no hand-written `unsafe`, no `cargo fmt`, never `git add -A`). Then
`apps/starplayer-web/README.md`, `apps/starplayer-web/src/lib.rs`,
`apps/starplayer-web/www/{app.js,ring.js,worklet-processor.js,index.html,style.css}`,
`apps/starplayer-web/test/headless.mjs`, `crates/starplayer-host-wasm/src/lib.rs`
(`Host`, `WebEngine`, `load_module`, `process`, the wire protocol),
`crates/starplayer-mixer/src/{output.rs,path.rs}` (`OutputFormat`, `FloatOut`/`FixedOut`,
`HostSample` for `f32`/`i8`/`i16`/`I24`/`i32`, `Dither`, `MixPath`, `FloatPath`/`FixedPath`),
`crates/starplayer-dsp/src/interpolate.rs` (`Nearest`, `Linear`),
`crates/starplayer-engine/src/engine.rs` (`Engine<Path, Interp, Out, Module>`,
`EngineSettings`, `Command::SetInterpolator` currently flagged as unsupported — see the
comment there: switching path or interpolator is a re-instantiation of the type
parameters, not a field write) and `crates/starplayer-core/src/event.rs`
(`Interpolator`). `plans/product/01-technical-architecture.md` §7 and §9.2 for the
mixer design and the wire protocol.

The owner asked for two things:

1. **See and choose the output device mode.** What the browser is actually running the
   `AudioContext` at, what the device supports, and a way to request a different sample
   rate / output device / channel count.
2. **Test the mixer at different rates and bit depths.** The engine has a float and a
   fixed-point path, nearest and linear interpolation, and output conversion to
   8/16/24/32-bit integer or f32 with optional deterministic dither (M1-B5). None of that is
   reachable from the player. The owner wants to *hear* them — a 1994 module through an
   8-bit, 11 kHz, nearest-neighbour path is part of the point of this project.

## Deliverables

### 1. An `OutputMode` in the engine crate, and a mode-switchable host engine

In `starplayer-engine` (no_std), a plain data description of a mixer configuration:

```rust
pub struct MixerMode { pub path: MixPathKind /*Float, Fixed*/, pub interpolator: Interpolator /*Nearest, Linear*/,
                       pub depth: OutputDepth /*F32, I32, I24, I16, I8*/, pub dither: bool, pub channels: u8 /*1 or 2*/ }
impl MixerMode { pub const DEFAULT: MixerMode; pub fn describe(&self) -> impl Display; pub fn from_wire(u32) / to_wire() }
```

`Command::SetInterpolator` stays flagged (the engine cannot re-instantiate itself);
the **host** owns the switch. In `starplayer-host-wasm`, replace the single `WebEngine`
alias with an enum over the engine instantiations the mode can select, built by a macro
so each arm is one line — `Engine<FloatPath, Linear, FloatOut<f32, 2>>`,
`Engine<FixedPath, Nearest, FixedOut<i16, 2>>`, and so on. Keep the set finite and
documented: 2 paths × 2 interpolators × {stereo, mono} = 8 arms, with **depth and dither
applied as a post-quantisation stage in the host** on the f32 output using the mixer's
own `HostSample` conversions (f32 → `i8`/`i16`/`I24`/`i32` → f32) and `Dither`. That
hears exactly what the reduced-depth output would be, without 40 engine arms. The fixed
path's native i16 output is used as-is when depth is `I16` (bit-exact with the golden
path M2 will hash), and quantised further for `I8`.

Switching mode rebuilds the engine (`Engine::with_settings`) at the *same* sample rate,
reloads the current `Arc<Module>` (already held by the host — no bytes cross again),
rebuilds the sequencer with `sequencer_for` and **seeks to the order that was sounding**,
restarts the musical clock at the engine's frame, and resumes playing if it was. Off the
render path: in the worklet message handler, like `load_module`. The retired engine's
module `Arc` goes down the garbage channel as usual; a mode switch must not leak or
allocate in `process()`.

### 2. Wire protocol

New opcode `SET_MIXER_MODE` (argument = `MixerMode::to_wire()`) on the existing SAB
command ring, with the postMessage fallback batching it like the others. The telemetry
header gains the **active** mode word (what the host actually built, so the page can show
"requested vs. active"). Update `ring.js`, `ring-harness.mjs`, the worklet, and §9.2 of
the architecture document (the header word list).

### 3. Output device panel (page)

A new "Output" panel, near the Engine panel, showing live:

- `AudioContext.sampleRate` (actual), `baseLatency`, `outputLatency`, `state`;
- `destination.maxChannelCount` and current `channelCount`;
- the sink: `AudioContext.sinkId` where implemented, the device label from
  `navigator.mediaDevices.enumerateDevices()` when permission allows, otherwise "default";
- what the *engine* is running at: the mode string from telemetry, and the worklet's
  sample rate (they must agree with the context).

Controls:

- **Requested sample rate**: `device default, 8000, 11025, 16000, 22050, 32000, 44100,
  48000, 88200, 96000`. Applying it **rebuilds the `AudioContext`** with
  `{ sampleRate, latencyHint }` (the only way to change it), re-adds the worklet module,
  recreates the node with the same `processorOptions` shape, reloads the current module
  from the bytes the page already holds (`state.currentModuleBytes` — retain them), seeks
  to the order that was playing and resumes. Show **requested vs. actual** — browsers may
  refuse or resample (Chromium resamples to the device rate; Firefox may throw `NotSupportedError`
  for some rates — catch it, report it, keep the old context).
- **Output device**: a select populated from `enumerateDevices()` (audio outputs), applied
  with `AudioContext.setSinkId()` where available (Chromium 110+); hidden with a note when
  the browser lacks it. Use `navigator.mediaDevices.selectAudioOutput()` where present
  (Firefox) to get labels.
- **Channels**: stereo / mono (sets the node's `outputChannelCount` and the mixer mode's
  `channels`; rebuild the node).

### 4. Mixer panel (page)

Selects for **path** (float / fixed-point), **interpolation** (nearest / linear),
**depth** (32-bit float / 32-bit int / 24-bit / 16-bit / 8-bit), **dither** (off / TPDF).
Applying sends `SET_MIXER_MODE`; the Engine panel shows the active mode from telemetry.
Persist the last choices in `localStorage` (wrapped in try/catch) so the owner's test
setup survives a reload; the module does not persist.

Everything stays framework-free; layout follows the existing panels; phone width must
not overflow.

### 5. Documentation

`apps/starplayer-web/README.md` gains an "Output and mixer options" section; the
architecture document §7 gets a short note that the host, not the engine, owns mode
switching and why (type parameters), and §9.2 the new header word and opcode.

## Research points

1. Which browsers honour `new AudioContext({ sampleRate })` for 8000 and 96000, and what
   Chromium reports in `sampleRate` when it resamples. Record what Chromium 151 headless
   does for each rate in the list.
2. Whether `setSinkId` on an `AudioContext` needs the `speaker-selection` permission
   policy in a cross-origin-isolated page; if it does, the dev server sends the header.
3. The right place for the depth post-quantiser so it does not break the fixed path's
   bit-exactness at `I16`: after the ring, before the planar copy, in the host.

## Verification

- Unit tests in `starplayer-engine` for `MixerMode` wire round-trip and `describe()`.
- Unit tests in `starplayer-host-wasm`: every enum arm builds and renders a quantum;
  switching mode mid-song keeps the order position and the module generation, retires no
  module (same `Arc`), and the `I16` depth on the fixed path is byte-identical to the
  fixed engine's own i16 output; `I8` output only takes 256 distinct values; dither on
  changes the output and is deterministic across two runs.
- `ring-harness.mjs`: the new opcode and header word.
- `headless.mjs` (already muted with `--mute-audio`): a scenario that requests 22050 Hz,
  asserts the context rate changed (or that the refusal is reported), that the song is
  still playing at the same order, and that switching to fixed/nearest/8-bit produces a
  telemetry mode string of `fixed · nearest · 8-bit · stereo`; and phone width still
  `scrollWidth <= innerWidth`.
- `cargo xtask ci` green; `cargo xtask wasm` packages; report `dist/` sizes.

## Out of scope

Real-time sample-rate *conversion* inside the engine (the context rate is the engine
rate; the browser resamples to the device). Cubic/sinc interpolation, SIMD (M7). Native
device enumeration (M3's cpal host gets its own version of the Output panel).
