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

`PETRI` and `REFLEX` are packaged into `dist/modules/`
from the S3M crate's fixture corpus, alongside a `dist/modules/index.json` listing them.
The **Bundled fixture** menu is built from that manifest at start-up: no entry is written
into `index.html`, and a build that ships no fixtures — `--pages`, below — serves no
manifest, so the menu and its **Load fixture** button hide themselves rather than offering
names that would 404. Anything else arrives by file picker, drag-and-drop onto the drop
zone, or a URL the remote server allows CORS on. The URL box starts out pointing at
Purple Motion's *Unreal ][* on Modland, which sends `Access-Control-Allow-Origin: *`, so
one click on **Load URL** fetches a real song even on the public site; scene.org's mirror
of the same file only allows its own origin, which is why it is not the default.

## Deploying

```text
cargo xtask wasm --pages
```

packages the same build for <https://scottmcnab.github.io/starplayer/>, differing from the
development build in exactly two ways.

* **No bundled fixtures.** `dist/modules/` is not created at all. The corpus in
  `crates/starplayer-s3m/tests/fixtures/` is licensed for testing only, so it must not be
  republished; the page copes with its absence as described above.
* **The isolation shim runs.** GitHub Pages cannot send COOP/COEP, so `crossOriginIsolated`
  would be false and the page would quietly fall back to batched `postMessage` for commands
  and telemetry. `www/coi-serviceworker.js` (v0.1.7, MIT, vendored verbatim) registers a
  service worker that adds the headers to every response and reloads the page once — a
  single extra reload on a first visit, and nothing thereafter. It self-skips when the page
  is already isolated, so it costs a properly isolated host nothing. `www/index.html` marks
  its place with a `<!-- xtask:coi -->` line in `<head>`; `--pages` replaces that line with
  the script tag, and a missing marker fails the build rather than silently producing a
  non-isolated site. Without `--pages` the marker is copied through untouched, which is why
  the dev server's `--no-isolation` mode still exercises the genuine non-isolated path.

`--pages` also stages a second, independent build of the A4-N1 cast probe under
`dist/cast-probe/` — see `apps/starplayer-cast-probe/README.md` — which deliberately
carries no isolation shim of its own.

`.github/workflows/pages.yml` does this in CI. It is triggered by the **CI** workflow
completing on `main` and builds only when that run's conclusion was `success`, so nothing
reaches the public site on a red build; `workflow_dispatch` runs it by hand. It checks out
the commit CI passed on, installs the `wasm-bindgen` CLI at the workspace's exact pin,
runs `cargo xtask wasm --pages`, and uploads `apps/starplayer-web/dist` as the Pages
artefact for a second job to deploy.

The one-time repository setting it depends on is **Settings → Pages → Source: GitHub
Actions**. Without it the deploy job fails; there is no branch to configure and no
`gh-pages` branch involved.

## Controls

The progress slider mirrors the engine's own song timeline rather than a page-side
estimate: dragging it (an `input` event) only moves the elapsed readout, and releasing it
(a `change` event) sends one `OPCODE_SEEK_FRAME` command carrying the frame under the
thumb — continuous drag events are never queued, since the command ring holds only 64
entries and is drained once per render quantum. The two readouts are `m:ss`, `h:mm:ss`
once the song runs past an hour, and `--:--` while the song's length is not yet known.
Elapsed is the engine's own reported position, not compensated for the `AudioContext`'s
`outputLatency`, so it can read a little ahead of what is actually heard. Clicking either
readout switches the left one between elapsed and the time remaining as `-m:ss`, as VLC
does; the choice is kept in `localStorage`.

**Repeat**, checked by default, sends `OPCODE_AT_END`. Checked, the song comes round again
— at its loop point, or from the restart order when the order list runs out — and the
slider wraps with it.

Unchecked, what happens depends on how the song ends, which the scan decides and reports in
the snapshot's `songFlags`:

* A song that **loops**, because a `Bxx`/`Cxx`/`Dxx` jumps back into music already played,
  fades out over `SONG_FADE_SECONDS` (5 seconds) into what would otherwise be its second
  pass, then stops. The slider's length includes those five seconds, so it still represents
  one complete playback.
* A song that **ends**, because its order list simply runs out or a stop marker fires,
  stops on its end frame: no second pass, no fade, and its displayed length is exactly one
  pass.

Either way the host rewinds the transport to song frame 0 once it stops, and the slider
follows it to `0:00`. The same command is resent whenever a module is (re)activated and
whenever a playback-restoring graph rebuild happens (a sample-rate or channel-count change,
or the MOD panning reload), so the checkbox's choice survives all of them.

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

The most recently opened archive is retained, so its other tracks stay one click away: a
**Track from …** dropdown and a **Load from ZIP** button appear in the load panel beside
the bundled-fixture controls, listing the same `name — size` entries the modal shows and
keeping the playing entry selected. The list survives cancelling the modal and survives
loading a plain module; opening another ZIP replaces it. Loading from it goes through the
same extract-and-activate path as the modal pick, so the label and messages are identical.
Only the one archive's bytes are held — `archive_extract` returns a fresh copy per call —
and nothing is remembered across a page reload.

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

## Load options (M10-W4)

The load panel's checkboxes are everything applied while a module is decoded, rather than
afterwards: the MOD-only panning option, and the load-time sample enhancers from
`starplayer-enhance`. All three share one persisted record, one reload dance, and one
availability rule.

**Headphone-friendly MOD panning** narrows MOD's authentic hard L-R-R-L defaults to the
same symmetric 60% positions used by ordinary stereo S3Ms. It does not affect S3M, MTM, or
later MOD panning effects.

**Reduce decay noise**, **Upsample samples (4x sinc)**, **Extend sample bandwidth** and
**Smooth loop seams** rebuild every sample through `Module::enhanced` before the module
reaches the engine. Unlike the panning option these apply to every format, not only MOD.
No checkbox is transcribed by hand: the worklet's first `ready` message carries
`enhancements_json()`, read straight from `starplayer-enhance::CATALOGUE` (the wasm host's
`starplayer` dependency enables the facade's `enhance` feature for exactly this), and the
load panel builds one checkbox per entry from its label, description and flag bit. All four
are off by default.

**The order is the catalogue's, not the order you tick them in.** The stages always run
`denoise → sinc4x → sbr → loop`, which is the order they belong in as a signal chain: the
denoiser reads its noise floor from the fact that an 8-bit sample's frames are all
multiples of 256, which is true only before anything else has touched them; the upsampler
creates the headroom the bandwidth extender needs, and with no headroom the extender is
the identity; and the loop smoother goes last so its crossfade is measured in the rebuilt
sample's own frames. The recommended combination on 8-bit tracker material is all four —
**Reduce decay noise**, **Upsample samples**, **Extend sample bandwidth** and **Smooth loop
seams** — which is what the CLI spells `--enhance denoise+sinc4x+sbr+loop`.

Note that the checkbox **bits** are not in that order: bits 0 and 1 belong to the
upsampler and the loop smoother, which shipped first and are already in browsers'
`localStorage`, and the two enhancers added in M10-K5c took bits 2 and 3. Nothing outside
the catalogue needs to know that, which is the point.

**Frame budget.** A rebuild bypasses the IT loader's own load-time PCM budget entirely, so
the wasm host enforces its own 16-million-frame ceiling on the *rebuilt* module: a
requested 4x upsample that would cross it is quietly retried at 2x, and a 2x rebuild that
still would not fit is dropped to identity. The other three stages are unaffected, since
none of them changes a sample's length. `moduleLoaded`'s `appliedEnhancementFlags` and
`appliedEnhancementFactor` report what actually ran, and the status line says so whenever
it is narrower than what the checkbox asked for.

**The instrument metadata panel shows source lengths.** The page-side metadata instance
(`apps/starplayer-web/src/lib.rs`) validates and describes a module without ever enhancing
it — only the worklet's own copy is rebuilt — so the Instruments panel's frame counts are
always the stored length before any enhancer runs, labelled "source frames" for exactly
that reason.

Changing any of the three while a module is active reloads the retained bytes at the
sounding order and restores transport, volume, and channel mutes; the old module remains
live if decoding fails, and the persisted preference is put back whether or not another
load has started since. The panning checkbox alone has a MOD-only early-out — asking for it
on a non-MOD module just saves the preference for the next load without reloading, since
authentic panning is exactly what a non-MOD format already used — while an enhancement
choice always reloads a retained module, being format-independent.

Every load-option input is disabled while any module load is in flight. The worklet port is
a FIFO, so two `loadModule` messages in the air at once are decided by posting order rather
than by which promise settles first; the reload also claims a module revision of its own,
exactly as the load path does, so whichever of the two claimed last is the one that paints
the UI.

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

## Effects (M7-H7)

A panel beside Mixer exposes the insert graph: a **Target** select (Master, plus one entry
per channel the loaded module actually has, read from the live telemetry) and four ordered
slot rows, always visible, each an effect select (`None` plus every effect the build can
install), a **Bypass** checkbox, and — once an effect is chosen — one range input per
parameter.

Nothing about a specific effect is hand-written here. `worklet-processor.js` carries the
wasm host's own `effects_json()` on its very first `ready` message — every effect's name,
its position in `InsertKind::ALL` (the `kind` `install_insert` takes), and every
parameter's name, `ParamUnit` variant, range and default — and the panel is built entirely
from that. Reordering or renaming an effect on the Rust side changes what the page shows
without a line of JavaScript changing.

Choosing an effect (or `None`) posts an `inserts` message and rebuilds the slot's sliders
from that effect's own defaults; like `midiInput`, this is a worklet message task rather
than a wire opcode, because building an effect allocates its delay lines. Moving a slider
sends `OPCODE_INSERT_PARAM`; the Bypass checkbox sends `OPCODE_INSERT_BYPASS`. Both pack
the target and slot into the same twelve bits, exactly as `plans/product/01-technical-architecture.md`
§9.2 describes, and ride the ordinary command ring — no allocation, no worklet message,
applied in `process()` like a mute or a master-volume change.

The page keeps its own record of what it believes is installed, per target and slot —
nothing reads the engine's insert chain back over the wire, so there is nowhere else that
belief could live. A rejected install or remove (`insertsError`) puts it back the way it
was rather than leaving the panel claiming an effect that never took. The compressor's
gain-reduction meter is not shown: this build's telemetry snapshot does not carry it (H4
did not widen it, and H7 did not either), which the task's own deliverable allows —
"otherwise omitted."

### Why the host owns the mixer mode

The engine's path, interpolator and output format are **type parameters**, so changing one
is a re-instantiation, not a field write — `Command::SetInterpolator` is still flagged as
unsupported for exactly that reason. `starplayer-engine` therefore carries only
`MixerMode`, a plain `no_std` description with a stable `u32` wire encoding, and
`starplayer-host-wasm` owns a sixteen-arm enum over the engine instantiations the mode can
select (2 paths × 4 interpolators × mono/stereo), built by a macro so each arm is one line.

Depth and dither are **not** engine arms. They are a post-quantisation stage in the host,
applied to the rendered samples with the mixer's own `HostSample` conversions and `Dither`
after the engine's output ring and before the planar copy the worklet reads. That gives
all five depths on all sixteen arms without eighty engine instantiations, and it keeps the
fixed path's native `i16` output bit-exact at `I16` depth — the golden path M2 will hash.

A mode switch happens in the worklet's message handler, never in `process()`. It rebuilds
the engine at the same sample rate, hands it the `Arc<Module>` the host already holds — no
bytes cross again and nothing is retired — rebuilds the sequencer with
`sequencer_with_quirks` under `QuirkSelection::Override` of the cached scan's own quirks,
so the file's tracker dialect, tempo model and resolved MOD timing all survive the switch
(the scan, not the UI, is where that override comes from — see `starplayer::scan_song`),
seeks it to the order that was sounding,
restarts its clock at the new engine's frame, and restores master volume, channel mutes
and the play/stop state.

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
panning while checking order, transport, and mute restoration. It then races a six-channel
module load against the panning toggle — the toggle is dispatched from a MutationObserver
microtask in the same task that hands the load's bytes to the worklet, so the window does
not depend on timing — and asserts that the panning option was unavailable during the load
and that the module the page displays is the one that is actually sounding. It then plays a
fixture,
asserts the quantum is 128 frames and wasm memory does
not move, loads a second module over the top of the first and waits for the retired Arc,
drops a deliberately broken file and checks that playback survives it, rebuilds the graph
at 22050 Hz and checks the song comes back at the order that was sounding, switches the
mixer to `fixed · nearest · 8-bit · stereo` and then to mono and checks both round-trip
through the telemetry header without growing wasm memory, selects channel 1 in the Effects
panel and reverb by name in its first slot (M7-H7 — never by the wire number
`InsertKind::ALL` happens to give it), samples the master peak for two seconds and asserts
it never drops to zero, then removes it the same way and checks its sliders disappear, and
finally shrinks the viewport to 390×844 and asserts nothing overflows horizontally.

The Node worklet harness does not need a browser: it stubs `AudioWorkletGlobalScope`,
hides Node's `TextDecoder` so the bundle has to supply its own, loads a real bundled
fixture, renders ten seconds through the engine, exercises transport, confirms retired Arc
collection, inspects synthetic MOD telemetry at both panning settings, proves two
overlapping processors own independent memories and Rust hosts, and checks that a bad file
is rejected with a readable message.
