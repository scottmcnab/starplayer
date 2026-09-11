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
