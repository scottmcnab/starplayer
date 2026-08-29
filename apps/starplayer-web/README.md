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
at all. Each run plays a fixture, asserts the quantum is 128 frames and wasm memory does
not move, loads a second module over the top of the first and waits for the retired Arc,
drops a deliberately broken file and checks that playback survives it, and finally shrinks
the viewport to 390×844 and asserts nothing overflows horizontally.

The Node worklet harness does not need a browser: it stubs `AudioWorkletGlobalScope`,
hides Node's `TextDecoder` so the bundle has to supply its own, loads a real bundled
fixture, renders ten seconds through the engine, exercises transport, confirms retired Arc
collection and checks that a bad file is rejected with a readable message.
