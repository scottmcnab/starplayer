# StarPlayer web player

Build and serve the player from the repository root:

```text
cargo xtask wasm
cargo xtask serve
```

Open `http://localhost:8080/`. The development server sets COOP, COEP and CORP so the
default `SharedArrayBuffer` transports are available. The page still works without those
headers — `node apps/starplayer-web/dev-server.mjs --no-isolation` serves it that way
deliberately — and reports its batched `postMessage` fallback in the Engine panel.

`ARMANI`, `MOVEMENT`, `NICETUNE`, `PETRI` and `REFLEX` are packaged into `dist/modules/`
from the S3M crate's fixture corpus and appear in the **Bundled fixture** menu. Anything
else arrives by file picker, drag-and-drop onto the drop zone, or a URL the remote server
allows CORS on.

## Architecture

The A4 worklet bundle architecture is preserved: `wasm-bindgen --target no-modules`
glue, `ring.js` and `worklet-processor.js` are concatenated into the one classic script
an `AudioWorkletGlobalScope` can load without fetching or importing anything itself. The
compiled wasm module is prepared on the page and structured-cloned to the worklet.
The packaging step wraps wasm-bindgen's generated no-modules IIFE in a binding factory:
each `AudioWorkletProcessor` gets its own WASM instance and memory while reusing that
compiled module. This matters during output-channel rebuilds, when the old and candidate
nodes intentionally overlap in one `AudioWorkletGlobalScope` until activation succeeds.

`worklet-prelude.js` is concatenated **ahead** of the glue. The worklet realm has no
`TextDecoder` and the glue builds one the moment the bundle is evaluated, so without it
the bundle throws before `registerProcessor` runs and the page reports only the downstream
symptom — `AudioWorkletNode cannot be created: the node name is not defined`.

B7 uses two wasm instances:

- `starplayer_web_bg.wasm` runs on the page thread. It validates the S3M through
  `ModuleReader`'s borrowing `&[u8]` path, retains metadata, decodes windowed
  display-only `PatternCell` rows, and hands over the English effect-name table.
- `starplayer_host_wasm_bg.wasm` runs in the worklet. Once validation succeeds, the
  original file `ArrayBuffer` is transferred to it. Activation constructs the real
  `Arc<Module>` and S3M sequencer outside `process()`, restarts the sequencer's tick clock
  at the engine's current musical frame, then hands the Arc through the engine command
  ring. Replaced engine Arcs return through the garbage channel and are collected from a
  later worklet message task; `retired_modules_collected()` is the running total the page
  and the tests assert on.

ZIP archives are opened only in the page-thread `starplayer-web` instance, through the
reusable `starplayer-archive` crate. The page lists and, after any required picker choice,
extracts an S3M before the existing validation and activation path begins. The worklet is
therefore still given only module bytes and never receives or inflates an archive.

Commands are typed fixed-size records in an SPSC `SharedArrayBuffer` ring. Without SAB,
the page batches every control change made during one animation frame into one message.
Coherent B6 snapshots use an odd/even seqlock over shared memory; the fallback posts a
whole decoded snapshot every eight render quanta. The wire layout is described in
`plans/product/01-technical-architecture.md` §9.2.

The English effect names are not transcribed into JavaScript. `EffectDisplay::name` is a
`&'static str`, which cannot ride a packed snapshot across realms, so the page-side
instance serialises `EffectNames::S3M` once at start-up and the page resolves
`(code, param)` against it — one table, still owned by the model crate.

The page deliberately uses no framework or package manager. Its state is one audio node,
one snapshot and two reused tables; a framework would add a build step without reducing
the code that must understand worklet ownership. Pattern rows are a fixed 13-row DOM
window. Cells are reused and rewritten only when the sounding row changes, avoiding a
table rebuild at tracker-tick rate.

At phone width the channel and pattern tables scroll horizontally. Collapsing a channel
row or paginating channels would hide the relationship between instrument, note, VU and
the English effect name; a deliberate horizontal swipe preserves it.

The load panel's **Headphone-friendly MOD panning** option narrows MOD's authentic hard
L-R-R-L defaults to the same symmetric 60% positions used by ordinary stereo S3Ms. It
does not affect S3M, MTM, or later MOD panning effects. Changing it while a MOD is active
reloads the retained bytes at the sounding order and restores transport, volume, and
channel mutes; the old module remains live if decoding fails.

## Output and mixer options

Two panels beside the Engine panel expose what the browser is actually doing and what the
mixer is actually doing, and let both be changed while a module plays.

**Output** reports the live `AudioContext`: requested versus actual `sampleRate`,
`state`, `baseLatency`, `outputLatency`, the node's channel count against
`destination.maxChannelCount`, the sink, and the worklet's own `sampleRate` — which must
agree with the context, and is shown separately so a disagreement is visible rather than
inferred.

- **Requested sample rate** — `device default` or one of 8000 … 96000. There is no way to
  retune a running `AudioContext`, so applying one **rebuilds the whole graph**: a new
  context with `{ sampleRate, latencyHint }`, the worklet module re-added, a new node with
  the same `processorOptions` shape, the current module reloaded from the bytes the page
  retains for exactly this purpose, a seek back to the order that was sounding, and play if
  it was playing. The old context is kept until the new one has a node, so a browser that
  refuses the rate (Firefox throws `NotSupportedError` for rates the device cannot do)
  leaves the music running and the panel reports the refusal. Chromium 151 honours every
  rate in the menu exactly — see the table below.
- **Output device** — populated from `navigator.mediaDevices.enumerateDevices()` and
  applied with `AudioContext.setSinkId()`. Labels stay blank until the browser grants
  output permission; the select and its Apply button hide themselves, with a note, on a
  browser without `setSinkId`. `navigator.mediaDevices.selectAudioOutput()` is offered as
  **Choose device…** where it exists (Firefox). `setSinkId` needs **no** Permissions-Policy
  header here: `speaker-selection` is not among the 82 policy-controlled features
  Chromium 151 implements, and for a top-level same-origin document the specification's
  default allowlist is `self` anyway. The dev server therefore sends no extra header.
- **Channels** — stereo or mono. Applying rebuilds the worklet node inside the same
  context with `outputChannelCount: [n]` and a mixer mode whose channel count matches.

**Mixer** selects path (float / fixed-point), interpolation (linear / nearest), depth
(32-bit float, 32-bit int, 24-bit, 16-bit, 8-bit) and dither (off / TPDF). Applying sends
one `SET_MIXER_MODE` command; the Engine panel's **Active mixer** line is read back out of
the telemetry header, so it shows what the host actually built rather than what was asked
for. A 1994 S3M through `fixed · nearest · 8-bit · mono` at 11025 Hz is the point of the
exercise.

Every one of those choices is written to `localStorage` (in a try/catch — a
storage-blocked page still works, it just forgets). The module is never persisted.

### Why the host owns the mixer mode

The engine's path, interpolator and output format are **type parameters**, so changing one
is a re-instantiation, not a field write — `Command::SetInterpolator` is still flagged as
unsupported for exactly that reason. `starplayer-engine` therefore carries only
`MixerMode`, a plain `no_std` description with a stable `u32` wire encoding, and
`starplayer-host-wasm` owns an eight-arm enum over the engine instantiations the mode can
select (2 paths × 2 interpolators × mono/stereo), built by a macro so each arm is one line.

Depth and dither are **not** engine arms. They are a post-quantisation stage in the host,
applied to the rendered samples with the mixer's own `HostSample` conversions and `Dither`
after the engine's output ring and before the planar copy the worklet reads. That gives
all five depths on all eight arms without forty engine instantiations, and it keeps the
fixed path's native `i16` output bit-exact at `I16` depth — the golden path M2 will hash.

A mode switch happens in the worklet's message handler, never in `process()`. It rebuilds
the engine at the same sample rate, hands it the `Arc<Module>` the host already holds — no
bytes cross again and nothing is retired — rebuilds the sequencer with `sequencer_for`,
seeks it to the order that was sounding, restarts its clock at the new engine's frame, and
restores master volume, channel mutes and the play/stop state.

### What Chromium 151 headless does with a requested rate

| Requested | Actual `sampleRate` |
|---|---|
| device default | 44100 |
| 8000 | 8000 |
| 11025 | 11025 |
| 16000 | 16000 |
| 22050 | 22050 |
| 32000 | 32000 |
| 44100 | 44100 |
| 48000 | 48000 |
| 88200 | 88200 |
| 96000 | 96000 |

No rate was refused and none was silently resampled to the device rate: Chromium honours
the constructor argument and resamples on its own output side. Firefox and Safari have not
been run here (neither is installed) and still need an owner check.

### Reading the memory line

The Engine panel's memory line is the check that matters for real-time safety, and it
means *growth during playback*. Loading a module allocates on the worklet instance, and
that allocation happens in a message task, outside `process()` — the panel rebases on it
and says so. Growth between two loads is a defect; growth across a load is the cost of
having a module.

## Browser notes

`Start audio` constructs and resumes `AudioContext` synchronously before the first
`await`. This is the conservative iOS Safari unlock shape. Chromium is exercised by the
headless harness; **Firefox and real iOS Safari have not been run** — neither is installed
on this machine — so they still need an owner/device check.

URL loading depends on the remote server allowing CORS. A readable error is shown when it
does not. A malformed file is rejected by the page-side loader and never reaches the live
graph.

## Checks

After `cargo xtask wasm`:

```text
node apps/starplayer-web/test/ring-harness.mjs
node apps/starplayer-web/test/worklet-harness.mjs
node apps/starplayer-web/test/headless.mjs                          # three modes, 60 s each
node apps/starplayer-web/test/headless.mjs --mode sab --seconds 5   # quick
```

The headless check skips cleanly when no Chromium/Chrome executable is installed; set
`CHROME=/path/to/chrome` to point it at one. It drives the browser over the DevTools
protocol with Node's built-in `WebSocket`, and runs the page three ways — cross-origin
isolated on shared memory, isolated with the fallback forced, and served without COOP/COEP
at all. Each run first loads a synthetic MOD and observes hard, 60%, then hard L-R-R-L
panning while checking order, transport, and mute restoration. It then plays a fixture,
asserts the quantum is 128 frames and wasm memory does
not move, loads a second module over the top of the first and waits for the retired Arc,
drops a deliberately broken file and checks that playback survives it, rebuilds the graph
at 22050 Hz and checks the song comes back at the order that was sounding, switches the
mixer to `fixed · nearest · 8-bit · stereo` and then to mono and checks both round-trip
through the telemetry header without growing wasm memory, and finally shrinks the viewport
to 390×844 and asserts nothing overflows horizontally.

The Node worklet harness does not need a browser: it stubs `AudioWorkletGlobalScope`,
hides Node's `TextDecoder` so the bundle has to supply its own, loads a real bundled
fixture, renders ten seconds through the engine, exercises transport, confirms retired Arc
collection, inspects synthetic MOD telemetry at both panning settings, proves two
overlapping processors own independent memories and Rust hosts, and checks that a bad file
is rejected with a readable message.
