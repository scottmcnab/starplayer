# A4 — N1: the Cast receiver capability probe

| Field | Value |
|---|---|
| Milestone | A4 ([master plan](A4-master-plan.md)) |
| Status | Ready 2026-09-11; owner: registration and hosting |
| Depends on | The web player and `cargo xtask wasm` as landed by [M1-B7](../engine/complete/M1-task-B7-web-player.md) and [W3](complete/W3-task-github-pages.md) |
| Blocks | A4-N3 (the StarPlayer Web Receiver) and, through it, A4-N4 (the web player's Cast button) |
| Parallel with | [N2](A4-task-N2-cast-cli.md) — no shared code; both touch `plans/README.md` |
| Recommended model | Claude Sonnet (two hand-written pages, one xtask command and a Node harness; no engine work) |
| Verified by | agent (the Verification section below), then reviewer, then the **owner** on real hardware — registration, hosting and the findings are his steps |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine that already runs in the browser:
`apps/starplayer-web` is a framework-free page driving `starplayer-host-wasm` inside an
`AudioWorklet`. Read `AGENTS.md` first, then `apps/starplayer-web/README.md`.

`plans/apps/A4-master-plan.md` wants to play modules on Google Home / Nest speakers. Its
most interesting shape — decision 1 and task N3 — is a **custom Web Receiver**: an HTML
page we host and register, which the speaker loads and runs, so that the speaker itself
renders the module through our wasm engine and the browser is only a remote. Whether that
is possible at all is unknown. Google's "Cast for audio" guidance lists ES6, Fetch,
WebSocket and MSE for third-party audio hardware and **does not list WebAssembly or Web
Audio**; Nest speakers run a fuller Chromium than that profile, but nothing official says
how much fuller.

This task builds the instrument that answers the question: a development Web Receiver whose
only job is to report what the speaker's runtime can do, plus a Chrome sender page to drive
it. It renders no music the owner would want to hear, and **it is not N3** — do not start
building the real receiver here.

The owner does the parts that cost money and involve his own devices: a $5 Cast Developer
Console account, registering this page as a Custom Receiver, registering each speaker's
serial number, publishing the page over HTTPS, and pasting the resulting JSON into the A4
master plan.

### Code you must read before changing anything

- `apps/starplayer-web/README.md` — how the page is built and served, and what `--pages`
  changes.
- `xtask/src/main.rs` — `main`'s subcommand match (~line 268), `print_usage` (~line 298),
  `run_wasm` (~line 2003) and its `--pages` handling, `build_worklet_bundle` (~line 2167)
  and specifically the `std::fs::remove_file(&glue_path)` at its end that **deletes**
  `starplayer_host_wasm.js`, `copy_web_sources` (~line 2252), `copy_fixture_modules`
  (~line 2319), `report_output`, `check_bindgen_version` (~line 2478), and the constants
  `WASM_CRATE`, `WASM_ARTIFACT`, `BINDGEN_GLUE`, `WEB_OUTPUT_DIRECTORY`, `WASM_TARGET`.
- `apps/starplayer-web/test/worklet-harness.mjs` — `syntheticMod()` (lines 13–27): a
  four-channel M.K. MOD built byte by byte in JS, with a 256-frame square-wave sample. This
  is the probe's default module, and the reason the probe needs no fixture file.
- `apps/starplayer-web/www/app.js` — how the page instantiates the worklet, compiles the
  wasm module and talks to it; the probe's bench path is a much smaller version of the same
  moves on the page thread.
- `crates/starplayer-host-wasm/src/lib.rs` — the exported surface the bench calls:
  `init(sample_rate, mixer_mode_wire)`, `load_module_with_options(bytes,
  headphone_friendly_mod_panning, enhancement_flags)`, `process(frames) -> f32`,
  `output_ptr()`, `output_len()`, `render_quantum()`, `telemetry_ptr()`.
- `.github/workflows/pages.yml` — it uploads exactly `apps/starplayer-web/dist` as the Pages
  artefact. **Anything staged inside that directory is published with no workflow change**,
  which is how the probe reaches the web without a second workflow.
- `plans/apps/A4-master-plan.md` — decisions 1, 6 and 7, and research points 1 and 2, which
  are this task's research points.

## Deliverables

### 1. `apps/starplayer-cast-probe/www/receiver.html` + `receiver.js`

The development Web Receiver. It loads the CAF receiver SDK from
`https://www.gstatic.com/cast/sdk/libs/caf_receiver/v3/cast_receiver_framework.js` and
starts it with `CastReceiverContext.getInstance().start({ disableIdleTimeout: true })`, so
a speaker with no media playing does not tear the app down mid-probe.

DOM is **one `<pre>` status line and nothing else** — no images, no fonts, no CSS framework.
The Cast audio-device guidance is explicit about CPU and memory, and an audio speaker has
no screen to show any of it on; the `<pre>` exists for the times the page is opened in a
desktop Chrome tab for debugging.

Custom namespace: `urn:x-cast:com.starplayer.probe`. It listens with
`addCustomMessageListener` and replies with `sendCustomMessage` on the same namespace, to
the sender that asked. Three messages in, four out:

| In | Payload | Out |
|---|---|---|
| `probe` | — | `report` |
| `chunk` | `{ index, total, size, data }` — `data` is base64 | `chunk-ack`, then one `report` when the last chunk lands |
| `bench` | `{ quanta }` — how many `process(128)` calls to time | `bench-report` |

Anything that throws is answered with `error` — `{ type: "error", stage, message }` — and
never left to a silent failure; an unanswered probe is indistinguishable from a speaker
that cannot run the page at all, which is the one outcome this task must not produce.

`report` carries exactly these fields (absent capability → `false` or `null`, never a
thrown exception):

```text
type             "report"
userAgent        navigator.userAgent
wasm             { supported, instantiated, error }   — instantiate a 1-function module from
                 a hand-written byte array, call it, and report the value it returned
audioContext     { supported, sampleRate, state, baseLatency }
audioWorklet     { supported, registered, toneHeard, error }  — register a processor from a
                 Blob URL, run a 440 Hz tone for 2 s, report whether process() was called
scriptProcessor  { supported }                        — the AudioWorklet-less fallback N3 would need
sharedArrayBuffer { supported }
crossOriginIsolated  window.crossOriginIsolated
memory           performance.memory ? { jsHeapSizeLimit, totalJSHeapSize, usedJSHeapSize } : null
hardwareConcurrency  navigator.hardwareConcurrency ?? null
serviceWorker    { supported, controller }
maxMessageBytes  the largest `chunk` accepted so far, or null before any chunk arrives
```

`chunk` reassembles a module into one `Uint8Array`. Report `{ type: "chunk-ack", index,
received, elapsedMs }` per chunk, and when `index === total - 1` a `report` extended with
`transfer: { bytes, elapsedMs, bytesPerSecond, chunkSize }`. This is how research point 2
gets its answer: the sender walks the chunk size up until the speaker stops acknowledging,
and the last size that worked is the message limit.

`bench` runs the wasm host **on the page thread** — `init(sampleRate, mixerModeWire)`,
`load_module_with_options(bytes, false, 0)`, then `quanta` timed `process(128)` calls with
`performance.now()` around each — and answers `bench-report` with `{ quanta, frames,
microsecondsPerQuantum: { min, average, max, p95 }, budgetMicroseconds, realTimeRatio,
module: "synthetic" | "uploaded" }`. The budget is `render_quantum() / sampleRate` in
microseconds — about **2.9 ms** at 44.1 kHz — and `realTimeRatio` is average ÷ budget, the
number that decides whether N3 is possible. The module is the uploaded one if a `chunk`
transfer has completed, otherwise the synthetic default (deliverable 4). If the
`AudioWorklet` registered, run the same bench through it as well and report
`worklet: { quanta, underruns, microsecondsPerQuantum }`.

### 2. `apps/starplayer-cast-probe/www/sender.html` + `sender.js`

Chrome only — the Cast Web Sender SDK exists nowhere else. Loads
`https://www.gstatic.com/cv/js/sender/v1/cast_sender.js?loadCastFramework=1` and
initialises `cast.framework.CastContext` with the application id from an `<input>`, so the
owner can paste the id the Developer Console gives him without a rebuild (remember it in
`localStorage`). The page has:

- the `<input>` for the application id and a **Connect** control — the SDK's
  `<google-cast-launcher>` element;
- buttons **Probe**, **Bench** (with a `quanta` field, default 512) and **Send file** (a
  file picker plus a chunk-size field, default 65 536 bytes, chunked and sent in order with
  the next chunk waiting on the previous `chunk-ack`);
- a `<pre>` that appends every reply as pretty-printed JSON, and a **Copy** button that
  puts the whole transcript on the clipboard — the owner pastes that straight into the A4
  master plan.

It needs no COOP/COEP: it is an ordinary page that neither instantiates our wasm nor uses
`SharedArrayBuffer`. **Do not include `coi-serviceworker.js` in the probe** — its service
worker would claim the Pages subpath scope and is exactly the kind of surprise a
diagnostic page must not carry. (The web player's own build is unaffected; this is a
separate directory.)

### 3. `cargo xtask cast-probe`, and `cargo xtask wasm --pages` staging

- A new `cast-probe` subcommand in `xtask/src/main.rs`: `check_bindgen_version`,
  `cargo build --release --target wasm32-unknown-unknown -p starplayer-host-wasm`, then
  `wasm-bindgen --target no-modules --no-typescript --out-dir apps/starplayer-cast-probe/dist`,
  then a flat copy of `apps/starplayer-cast-probe/www/`. Clear the output directory first,
  the way `run_wasm` does, and print a listing at the end.
  **The probe keeps the standalone glue.** `build_worklet_bundle` deletes
  `starplayer_host_wasm.js` from the web player's `dist/` because the page there never
  instantiates wasm itself — it hands the compiled module to the worklet. The probe is the
  opposite case: `receiver.js` loads that glue directly with a `<script>` tag and calls
  `wasm_bindgen({ module_or_path: 'starplayer_host_wasm_bg.wasm' })`. So this command must
  **not** call `build_worklet_bundle`, and the probe's own `dist/` is a second, independent
  output directory — do not reuse `apps/starplayer-web/dist`.
- Register it in `main`'s match, in `print_usage`, and in the usage line style already
  there.
- `run_wasm`, when `--pages` is given, additionally stages the probe under
  `apps/starplayer-web/dist/cast-probe/` (build it the same way; factor the probe build into
  a function both callers use). Because `pages.yml` uploads `apps/starplayer-web/dist` and
  nothing else, this publishes the probe at
  `https://scottmcnab.github.io/starplayer/cast-probe/receiver.html` and
  `…/cast-probe/sender.html` with **no change to `.github/workflows/pages.yml`** — say so in
  a comment, so a later reader does not go looking for a second workflow. A plain
  `cargo xtask wasm` leaves the web `dist/` exactly as it is today.
- `report_output` walks `dist/` with `read_dir`; check that a subdirectory in there does not
  break its listing, and fix it if it does.

### 4. The synthetic module

The probe must never depend on a fixture file. Port `syntheticMod()` from
`apps/starplayer-web/test/worklet-harness.mjs` into a small ES module the receiver and the
harness both import — the same four-channel M.K. MOD, built byte by byte, with the same
1084-byte header, one pattern and a 256-frame square wave. Keep the comment explaining what
each magic offset is.

**Fixtures are never published to Pages.** `crates/starplayer-s3m/tests/fixtures/` is
licensed for testing only, `copy_fixture_modules` is skipped under `--pages` for that
reason, and the probe must not reintroduce them by the back door. The only real module that
ever reaches a speaker in this task is one the owner picks himself through **Send file**.

### 5. `apps/starplayer-cast-probe/test/probe-harness.mjs`

A Node harness in the shape of `apps/starplayer-web/test/worklet-harness.mjs`: no browser.
Stub the CAF receiver SDK (`cast.framework.CastReceiverContext` with
`addCustomMessageListener` / `sendCustomMessage` / `start`), evaluate `receiver.js`, and:

- dispatch `probe` and assert the `report` has every field named above, with the right
  types, on a runtime where several of them are genuinely missing (Node has no
  `AudioContext`) — that is the point: the report must degrade rather than throw;
- dispatch a chunked transfer of the synthetic module in 4 chunks and assert the acks, the
  reassembled byte-for-byte equality, and the `transfer` block;
- dispatch `bench` with a small `quanta` against the **real** wasm from
  `apps/starplayer-cast-probe/dist/`, and assert the timings are finite, positive and
  ordered `min ≤ average ≤ max`;
- dispatch a deliberately malformed `chunk` and assert an `error` reply rather than a throw.

### 6. `apps/starplayer-cast-probe/README.md`

What the probe is, what it is not (N3), and then **the owner's step list**, numbered and
literal:

1. Register at the Cast SDK Developer Console (<https://cast.google.com/publish/>) — a
   one-off **$5** fee.
2. Add a new application, type **Custom Receiver**, with the receiver URL
   `https://scottmcnab.github.io/starplayer/cast-probe/receiver.html`. Note the
   **application id** it issues.
3. Register each speaker by **serial number** under Cast Receiver Devices (a development
   app only loads on registered devices until it is published).
4. Wait — registration takes up to about 15 minutes to propagate — then **reboot the
   speaker**.
5. Open `https://scottmcnab.github.io/starplayer/cast-probe/sender.html` in **Chrome**,
   paste the application id, and cast to the speaker.
6. Press **Probe**, then **Bench**, then optionally **Send file** with a real module.
7. Press **Copy** and paste the JSON into a `## N1 findings` section of
   `plans/apps/A4-master-plan.md`, one block per device model tested.

Also document running it locally against desktop Chrome, and note that the sender page and
the receiver page can both be opened in ordinary tabs for debugging the message protocol
without any speaker at all.

Finally, add the probe to `plans/README.md`'s apps table and to `apps/starplayer-web/README.md`'s
**Deploying** section — one sentence saying that `--pages` also stages `cast-probe/` and
that the probe deliberately has no isolation shim.

## Research points

Answer each in a `## Research resolution` section you append to this file.

1. **The speaker's platform** (A4 point 1). What the probe is built to find out, and what it
   found where you could run it: which Chromium version Nest Audio / Nest Mini / Home Max
   report, whether `WebAssembly.instantiate`, `AudioContext`, `AudioWorklet` and
   `SharedArrayBuffer` exist, whether the receiver page is cross-origin isolated (expect
   **not** — which means N3's worklet would take the `postMessage` fallback path, and the
   probe should say so in its own report rather than leaving it to be inferred), and how a
   64-channel IT scores against the ~2.9 ms quantum budget. On this machine you can only
   report desktop-Chrome and Node numbers; say clearly which figures are owner-pending.
2. **Custom message size and throughput** (A4 point 2). What the largest accepted `chunk`
   turned out to be against the runtimes you could reach, how the sender walks up to it, and
   what the measured bytes/s was. Also state the mixed-content rule that makes chunking the
   default: an HTTPS receiver page cannot `fetch()` a plain-HTTP LAN origin, so
   fetch-by-URL only works when the module's host is itself HTTPS and sends CORS, and the
   message channel is the path that always works.

## Verification

```sh
cargo xtask cast-probe
ls apps/starplayer-cast-probe/dist                              # glue, .wasm, receiver.html, receiver.js, sender.html, sender.js
node apps/starplayer-cast-probe/test/probe-harness.mjs
cargo xtask wasm --pages && ls apps/starplayer-web/dist/cast-probe
grep -c coi-serviceworker apps/starplayer-web/dist/cast-probe/receiver.html   # 0
cargo xtask ci --job wasm-build
cargo xtask ci --job clippy
cargo test --workspace
cargo xtask wasm                                                # leave dist/ in its development shape
node apps/starplayer-web/test/worklet-harness.mjs               # the web player is unchanged
```

Report the exact commands and results. **Do not commit** — the reviewer commits.

## Out of scope

N3 (the real StarPlayer Web Receiver) and N4 (the web player's Cast button) — this task
produces the evidence they are gated on, and no more. The registration itself, the $5
account, the device serials and the publishing are the **owner's**, not the agent's.
`starplayer-cast` and the CLI ([N2](A4-task-N2-cast-cli.md)). Any change to the engine, the
worklet bundle, the web player's own pages, or `.github/workflows/pages.yml`.

## Research resolution

### 1. The speaker's platform (A4 point 1)

**What the probe is built to find out.** `probe`'s `report` carries `userAgent` (to read
off the Chromium version a speaker's own user-agent string reports), `wasm.instantiated`
and `wasm.returnedValue` (does `WebAssembly.instantiate` exist and actually run — not just
exist as a global), `audioContext.{supported,sampleRate,state,baseLatency}`,
`audioWorklet.{supported,registered,toneHeard}` (registered from a `Blob` URL and proven by
whether `process()` was ever called, not merely whether the constructor ran),
`sharedArrayBuffer.supported`, and `crossOriginIsolated` — reported directly rather than
left to be inferred, exactly as the task asks, because a receiver page is never served with
COOP/COEP by the Cast Developer Console and the worklet's `postMessage` fallback path is
the one that will matter for N3 if it is ever built. `bench`'s `realTimeRatio` (page-thread
average ÷ `render_quantum() / sampleRate`) is the number that answers "does a module render
in real time here" for whatever module is loaded — the synthetic default, or a real module
the owner sends with **Send file** — including a 64-channel IT, once the owner has one to
send; the probe cannot manufacture 64 real channels itself without a fixture, and fixtures
must never reach a published page (see "Never publish or bundle the S3M fixtures" below).

**What it found where it could run.**

*Node (no browser at all)* — `apps/starplayer-cast-probe/test/probe-harness.mjs`, this run:
`WebAssembly` present and the 1-function probe module returns 42; `AudioContext`,
`AudioWorklet` and `ScriptProcessorNode` all absent and reported as `supported: false` with
no throw; `SharedArrayBuffer` present (Node has it; a page does not get it without COOP/COEP
regardless); `crossOriginIsolated` false (no `window`); `performance.memory` absent, reported
`null`; `hardwareConcurrency` 32 (this host's core count — not a stand-in for a speaker's);
a real 8-quantum bench against the real `starplayer-host-wasm` build: average **593.8 µs**
against a **2902.5 µs** budget at 44.1 kHz (`realTimeRatio` ≈ 0.20) for the synthetic
4-channel module — see the exact figures pasted under "Verification results" below (they
vary a little run to run, as wall-clock timings do).

*Desktop Chrome — not assumed, actually run.* This sandbox turned out to have a real
Chromium build at hand (`Google Chrome for Testing 151.0.7922.34`, headless, the same
version already documented in `apps/starplayer-web/README.md`'s output-panel table), so
rather than stop at the Node numbers this task file expected, the agent drove the real
`receiver.js` against it: served the packaged `dist/` over a plain local HTTP origin (no
COOP/COEP, matching an unregistered Cast receiver's own hosting), stubbed
`cast.framework.CastReceiverContext` the same way the Node harness does, and dispatched
`probe` and `bench` by hand through the real listener. Full report:

```json
{
  "wasm": { "supported": true, "instantiated": true, "error": null, "returnedValue": 42 },
  "audioContext": { "supported": true, "sampleRate": 44100, "state": "running", "baseLatency": 0.01 },
  "audioWorklet": { "supported": true, "registered": true, "toneHeard": true, "error": null },
  "scriptProcessor": { "supported": true },
  "sharedArrayBuffer": { "supported": false },
  "crossOriginIsolated": false,
  "memory": { "jsHeapSizeLimit": 4395630592, "totalJSHeapSize": 2157467, "usedJSHeapSize": 1015723 },
  "hardwareConcurrency": 32,
  "serviceWorker": { "supported": true, "controller": false }
}
```

and a 256-quantum bench of the synthetic module: page-thread average **42.6 µs**
(`realTimeRatio` ≈ 0.015), worklet-thread average **39.1 µs**, both against the same
2902.5 µs budget, `underruns: 0`. Two things worth reading precisely:

- **`sharedArrayBuffer.supported: false`, `crossOriginIsolated: false`.** Confirmed, not
  assumed: an ordinary HTTP origin with no COOP/COEP genuinely takes away
  `SharedArrayBuffer` in a real, current Chromium, even though the object exists as a
  global in Node with no such restriction. This is exactly the "expect not [isolated]"
  the task predicted, now backed by a real browser rather than an inference from Node.
- **The worklet half of `bench` only works with the compiled `WebAssembly.Module` passed
  through `processorOptions` at `AudioWorkletNode` construction, never through a later
  `node.port.postMessage()`.** The first working draft of `runWorkletBench` built the node
  bare and then `postMessage`d `{ hostModule, moduleBytes, … }` to it — the same shape
  `worklet-processor.js`'s `ready`/`loadModule` protocol uses for ordinary messages — and
  it hung forever against the real browser: no exception on either side, the worklet's
  `onmessage` simply never fired. Isolated by hand with a minimal ping processor (see
  "Deviations" below): a `WebAssembly.Module` structured-clones correctly as part of
  `AudioWorkletNodeOptions` but silently fails to arrive over the same node's message port
  afterwards, in this Chromium build. `receiver.js` now passes everything the bench needs
  through `processorOptions` and runs the whole bench synchronously in the constructor, and
  a large doc comment on `BENCH_PROCESSOR_SOURCE` records the finding so nobody "fixes" it
  back the obvious way later. This is exactly the kind of platform surprise N1 exists to
  catch before N3 is built on top of it.
- **`AudioWorkletGlobalScope` has no `performance` at all** in this Chromium build — only
  `Date` (millisecond-grained) and the audio clock `currentTime`/`currentFrame`, which only
  advances once per real render quantum pulled into the graph and therefore cannot time a
  synchronous loop inside a constructor. The worklet bench now times batches of 32 calls
  with `Date.now()` and reports the batch average against every call in the batch — coarser
  than the page-thread bench's per-call `performance.now()` figure, documented as such in
  `runWorkletBench`'s own doc comment.

*Owner-pending, and only the owner's step, per this task file:* which Chromium version
Nest Audio, Nest Mini and Home Max actually report; whether any of the desktop findings
above hold on that much more constrained runtime (memory pressure, CPU class, and the third-
party "Cast for audio" profile's stated exclusion of WebAssembly/Web Audio — Nest speakers
are expected to exceed that profile, per the master plan, but "expected" is exactly what
this probe exists to confirm or deny); the real bench numbers for a module the owner
actually cares about, including a 64-channel IT, via **Send file**.

### 2. Custom message size and throughput (A4 point 2)

**What the probe is built to find out.** `sender.js`'s **Send file** walks a real module up
through `chunk` messages at a configurable size (default 65 536 bytes); its separate **Find
message limit** control (an addition beyond the task's three named buttons, cheap to build
from the same chunk-sending code and directly answering this research point) sends a series
of disposable one-chunk transfers doubling from 4 KiB until the receiver stops
acknowledging within a 5-second window or answers `error`, and reports the last size that
worked. The receiver's own `report.maxMessageBytes` independently tracks the largest chunk
any sender has successfully delivered, so the two numbers should agree.

**What it found where it could run.** The custom-message channel only exists inside a real
Cast session — a receiver page opened in an ordinary tab, or driven directly the way this
run's desktop-Chrome check above did, never goes through the actual CASTv2 transport at
all, so there is no real message-size ceiling to discover without a live Cast session. What
*was* verified, against the real receiver logic (Node harness, and confirmed byte-for-byte
by hand): a 4-chunk transfer of the synthetic module (2364 bytes total, 591-byte chunks)
reassembles exactly, each `chunk-ack` reports a monotonically increasing `received` count
and a non-negative `elapsedMs`, and the final chunk's `report.transfer` carries the correct
`bytes`, `chunkSize` and a finite `bytesPerSecond`. A deliberately malformed `chunk` (invalid
base64) answers `{ type: "error", stage: "chunk", message }` rather than hanging or
throwing into the void — the one failure mode this task must not produce.

**The mixed-content rule.** `receiver.html` is served only over HTTPS once registered (the
Cast Developer Console requires it). A page served over HTTPS cannot `fetch()` a plain-HTTP
origin — Chromium blocks the request as mixed content before it ever leaves the page — so
fetch-by-URL for a module only works when the module's own host is itself HTTPS and sends
CORS headers permitting the receiver's origin (an S3-style bucket, GitHub Pages, or the
CLI's `starplayer cast` server placed behind TLS). A bare LAN dev server serving plain HTTP,
which is the common case for "the file is on my laptop", is exactly what fetch-by-URL
cannot reach from a registered receiver — which is why chunking over the custom message
channel is the default and not merely a fallback: it is the one path that always works,
regardless of what is or is not HTTPS on the sender's own network.

**Owner-pending:** the actual largest accepted `chunk` size and measured bytes/s against a
real Cast runtime, via **Find message limit** and **Send file** against a registered
receiver and a real speaker.

## Deviations from the task file

- **`Cargo.toml`'s workspace `exclude` gained `apps/starplayer-cast-probe`.** Not in the
  deliverable list, but necessary: `members = [..., "apps/*", ...]` requires every matched
  directory to carry a `Cargo.toml`, and this app is deliberately not a Rust crate (reuses
  `starplayer-host-wasm` directly, like the task describes). Without the exclusion,
  `cargo build` anywhere in the workspace fails immediately with "failed to load manifest
  for workspace member". Documented in place with the same reasoning `fuzz`/`embedded`
  already carry.
- **`report_output`'s subdirectory handling was a real bug, not a hypothetical one.**
  Before the fix, a `cast-probe/` entry under `dist/` would have reported its own on-disk
  directory size (a filesystem block count, not a byte total — typically 4096 on this
  machine) labelled just `cast-probe`, with none of its eight real files listed at all.
  `report_output` and its new `collect_output_listing` helper now recurse and label each
  file with its path relative to the output directory; confirmed against the real
  `--pages` build (see the listing under "Verification results" below, where
  `cast-probe/receiver.html` etc. appear with their real sizes).
- **A `Find message limit` button beyond the task's three named sender controls
  (Probe/Bench/Send file).** Cheap to build from the same chunk-send/ack code `Send file`
  already needs, and it is the concrete mechanism research point 2 asks the sender to
  have ("walks the chunk size up until the speaker stops acknowledging"). Everything the
  task named is present and unchanged; this is additive.
- **The worklet half of `bench` does not use `performance.now()`, and does not pass the
  compiled module or bytes through a post-construction `postMessage`.** Both were the
  first working draft's design and both turned out to be wrong against a real
  `AudioWorkletNode` — see "Research resolution" §1 above for what was actually observed
  and why. The task's deliverable shape (`worklet: { quanta, underruns,
  microsecondsPerQuantum }`) is unchanged; only the internal timing mechanism and the
  parameter-passing path changed, both documented in place in `receiver.js`.
- **`report.wasm` carries a fourth field, `returnedValue`,** alongside the three the task
  names (`supported`, `instantiated`, `error`) — the task's own prose asks the probe to
  "report the value it returned" but does not name a field for it, so one was added rather
  than silently dropping that half of the instruction.
- **Desktop-Chrome findings were gathered from a real browser, not inferred.** The task
  says "on this machine you can only report desktop-Chrome and Node numbers", which reads
  as expecting a real Chrome to be reachable; a Playwright-managed `Google Chrome for
  Testing 151.0.7922.34` (the same build `apps/starplayer-web`'s own headless checks use)
  was already cached on this machine, so it was used, over the DevTools protocol, the same
  way `apps/starplayer-web/test/headless.mjs` drives Chromium. This is exploratory use for
  this task's own research section — no new repo dependency, no new checked-in test, and
  no change to `apps/starplayer-cast-probe/test/probe-harness.mjs`'s Node-only contract in
  the Verification section.
- **`www/worklet-prelude-source.mjs` duplicates `apps/starplayer-web/www/worklet-prelude.js`'s
  polyfill body** rather than importing it, exactly as the task's own instruction for
  `coi-serviceworker.js` establishes the pattern of: the two apps are independent Pages
  outputs and this file is small, hand-written, and ours. Exported as a string constant
  rather than run directly, since its only use is to be concatenated into a runtime-built
  worklet bundle (see "Research resolution" §1) rather than loaded as a `<script>` on the
  probe's own main thread, which already has a native `TextDecoder`.

## Verification results (this run)

All commands run from the workspace root, in order, after the worklet-bench fix described
under "Deviations" above. Output trimmed to the parts that matter; nothing was skipped or
reordered.

```
$ cargo xtask cast-probe
     wasm-bindgen CLI 0.2.127 matches the pinned crate version
     cargo build --release --target wasm32-unknown-unknown -p starplayer-host-wasm
    Finished `release` profile [optimized] target(s) in 0.05s
     wasm-bindgen --target no-modules .../target/wasm32-unknown-unknown/release/starplayer_host_wasm.wasm

          1398  receiver.html
         26099  receiver.js
          3189  sender.html
          9777  sender.js
         19085  starplayer_host_wasm.js
        745725  starplayer_host_wasm_bg.wasm
          3300  synthetic-mod.mjs
          4375  worklet-prelude-source.mjs

xtask cast-probe: packaged into apps/starplayer-cast-probe/dist
PASS
```

```
$ ls apps/starplayer-cast-probe/dist
receiver.html  receiver.js  sender.html  sender.js  starplayer_host_wasm.js
starplayer_host_wasm_bg.wasm  synthetic-mod.mjs  worklet-prelude-source.mjs
PASS — glue, .wasm, receiver.html, receiver.js, sender.html, sender.js all present
```

```
$ node apps/starplayer-cast-probe/test/probe-harness.mjs
ok: `probe` reports every field, fully degraded, on a runtime with no audio stack
ok: a 4-chunk transfer of the synthetic module (2364 bytes) reassembles and reports
ok: bench ran 8 real quanta — average 593.8us against a 2902.5us budget
ok: a malformed `chunk` message answers `error` rather than throwing
probe-harness: all checks passed
PASS
```

```
$ cargo xtask wasm --pages && ls apps/starplayer-web/dist/cast-probe
     ... (full build log; unchanged web player steps, then:)
     staging the A4-N1 cast probe under cast-probe/ (no change to .github/workflows/pages.yml)
     wasm-bindgen CLI 0.2.127 matches the pinned crate version
     cargo build --release --target wasm32-unknown-unknown -p starplayer-host-wasm
     wasm-bindgen --target no-modules ...

          1398  receiver.html
         26099  receiver.js
          3189  sender.html
          9777  sender.js
         19085  starplayer_host_wasm.js
        745725  starplayer_host_wasm_bg.wasm
          3300  synthetic-mod.mjs
          4375  worklet-prelude-source.mjs

        108265  app.js
          1398  cast-probe/receiver.html
         26099  cast-probe/receiver.js
          3189  cast-probe/sender.html
          9777  cast-probe/sender.js
         19085  cast-probe/starplayer_host_wasm.js
        745725  cast-probe/starplayer_host_wasm_bg.wasm
          3300  cast-probe/synthetic-mod.mjs
          4375  cast-probe/worklet-prelude-source.mjs
          4678  coi-serviceworker.js
         16078  index.html
         16695  ring.js
         56577  starplayer-worklet.js
        745725  starplayer_host_wasm_bg.wasm
         10612  starplayer_web.js
        270090  starplayer_web_bg.wasm
         13957  style.css

xtask wasm: packaged into apps/starplayer-web/dist

$ ls apps/starplayer-web/dist/cast-probe
receiver.html  receiver.js  sender.html  sender.js  starplayer_host_wasm.js
starplayer_host_wasm_bg.wasm  synthetic-mod.mjs  worklet-prelude-source.mjs
PASS — the recursive `report_output` fix shows real per-file sizes under `cast-probe/`
rather than one misleading directory-size line (see "Deviations")
```

```
$ grep -c coi-serviceworker apps/starplayer-web/dist/cast-probe/receiver.html
0
PASS
```

```
$ cargo xtask ci --job wasm-build
     RUSTFLAGS="-C llvm-args=-fp-contract=off -C target-feature=+simd128" cargo build --target wasm32-unknown-unknown -p starplayer-host-wasm --features simd
    Finished `dev` profile [unoptimized + debuginfo] target(s)
xtask ci: 1 job(s) passed
PASS
```

```
$ cargo xtask ci --job clippy
=== xtask ci: clippy ===
     cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s)
xtask ci: 1 job(s) passed
PASS — no new warnings from the xtask/Cargo.toml changes
```

```
$ cargo test --workspace
94 "test result: ok. ... 0 failed" lines, 0 lines containing FAILED, across the whole
workspace (unit tests, integration tests and merged doctests for every crate). ALSA
"snd_seq_hw_open" warnings appear from a couple of MIDI-adjacent crates' tests — pre-existing
environment noise (no `/dev/snd` in this sandbox), not a new failure.
PASS
```

```
$ cargo xtask wasm
     ... (full build log)
        108265  app.js
          4678  coi-serviceworker.js
         16052  index.html
         35966  modules/PETRI.S3M
          9634  modules/REFLEX.S3M
            28  modules/index.json
         16695  ring.js
         56577  starplayer-worklet.js
        745725  starplayer_host_wasm_bg.wasm
         10612  starplayer_web.js
        270090  starplayer_web_bg.wasm
         13957  style.css

xtask wasm: packaged into apps/starplayer-web/dist
PASS — dist/ left in its development shape: fixtures present, no cast-probe/ subdirectory
```

```
$ node apps/starplayer-web/test/worklet-harness.mjs
worklet harness: 10 s of real S3M render (peak 0.731), independent overlapping processors,
MOD panning, transport, seek, memory, garbage, scope taps on both transports, live MIDI
input on both transports and bad-file rejection passed
PASS — the web player is unchanged
```

All nine verification steps pass. Additionally, and beyond what this section asked for: the
real desktop-Chrome run described under "Research resolution" §1 exercised the packaged
`dist/` end to end (`probe` and `bench`, including the worklet half) against a real
`AudioWorkletNode`, which is what caught and fixed the `postMessage`/`WebAssembly.Module`
bug recorded under "Deviations".
