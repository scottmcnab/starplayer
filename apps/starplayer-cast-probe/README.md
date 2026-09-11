# StarPlayer cast probe (A4-N1)

A development Google Cast Web Receiver, and a Chrome sender to drive it, whose only job is
to answer whether a Google Home / Nest speaker's runtime can run StarPlayer at all:
`WebAssembly`, `AudioContext`, `AudioWorklet`, `SharedArrayBuffer`, `performance.memory`,
and how fast the real `starplayer-host-wasm` renders a bench module there.

**This is not the real StarPlayer Cast receiver.** It plays no music the owner would want
to hear, has almost no UI, and reports rather than performs. The real receiver is task N3
in [`plans/apps/A4-master-plan.md`](../../plans/apps/A4-master-plan.md), and only gets
built once this probe's findings say the speaker's runtime can support it.

## What is here

- `www/receiver.html` + `www/receiver.js` — the Web Receiver: one `<pre>` status line,
  no CSS, no images. Registers a custom message listener on
  `urn:x-cast:com.starplayer.probe` and answers `probe`, `chunk` (a chunked module
  transfer) and `bench` (timed `process(128)` calls through the real wasm host).
- `www/sender.html` + `www/sender.js` — a Chrome-only page with the Cast Web Sender SDK,
  **Probe**, **Bench** and **Send file** controls, a **Find message limit** diagnostic, a
  transcript, and a **Copy** button for pasting the JSON straight into the A4 master plan.
- `www/synthetic-mod.mjs` — the default bench module: the same four-channel M.K. MOD
  `apps/starplayer-web/test/worklet-harness.mjs` builds byte by byte, so the probe never
  needs a fixture file on disk. **Nothing under `crates/starplayer-s3m/tests/fixtures/`
  is ever published by this probe** — that corpus is licensed for testing only.
- `www/worklet-prelude-source.mjs` — the `TextDecoder` polyfill `receiver.js` concatenates
  ahead of a freshly fetched copy of the wasm-bindgen glue when it builds a second wasm
  host instance inside a real `AudioWorklet`, for the `worklet` half of a `bench` reply.
- `test/probe-harness.mjs` — a Node harness (no browser) that stubs the CAF receiver SDK,
  loads the real glue and wasm binary from `dist/`, and exercises `probe`, a chunked
  transfer, `bench` against the real wasm host, and a malformed `chunk`.

## Building

```sh
cargo xtask cast-probe
```

packages `www/` plus the compiled `starplayer-host-wasm` (standalone wasm-bindgen glue,
**not** the worklet bundle — see the doc comment on `build_cast_probe` in
`xtask/src/main.rs`) into `apps/starplayer-cast-probe/dist/`, independent of
`apps/starplayer-web/dist`.

```sh
cargo xtask wasm --pages
```

additionally stages a second copy of the probe under `apps/starplayer-web/dist/cast-probe/`.
`.github/workflows/pages.yml` uploads the whole of `apps/starplayer-web/dist` and nothing
else, so this publishes the probe at
`https://scottmcnab.github.io/starplayer/cast-probe/receiver.html` and
`…/cast-probe/sender.html` **with no workflow change** — do not go looking for a second
`.github/workflows/*.yml` file for this; there isn't one. A plain `cargo xtask wasm` (no
`--pages`) never stages the probe and leaves the web player's own `dist/` exactly as it
was.

## Running the checks

```sh
cargo xtask cast-probe
node apps/starplayer-cast-probe/test/probe-harness.mjs
```

The harness needs no browser and no network access; it stubs the CAF receiver SDK and
supplies the two files the glue's own `fetch()` calls ask for directly off `dist/`.

## Running it locally, with no speaker

Both pages work as ordinary tabs:

```sh
cargo xtask cast-probe
python3 -m http.server --directory apps/starplayer-cast-probe/dist 8090
```

then open `http://localhost:8090/sender.html` in Chrome — the Probe/Bench/Send controls
stay disabled until a session actually connects, but the page loads, the Cast SDK
initialises, and `http://localhost:8090/receiver.html` can be opened directly in another
tab to read its own status line and watch what it does without ever reaching a real Cast
session. This is enough to debug the message protocol end to end before spending any of
the owner's time on real hardware. (`python3 -m http.server` is any static file server —
`cargo xtask serve` is not used here, since it serves `apps/starplayer-web/dist`, not this
directory.)

## Owner: registration and hosting

The parts that cost money and touch the owner's own devices are the owner's, not the agent's.

1. Register at the Cast SDK Developer Console (<https://cast.google.com/publish/>) — a
   one-off **$5** fee.
2. Add a new application, type **Custom Receiver**, with the receiver URL
   `https://scottmcnab.github.io/starplayer/cast-probe/receiver.html`. Note the
   **application id** it issues.
3. Register each speaker by **serial number** under Cast Receiver Devices — a development
   application only loads on registered devices until it is published.
4. Wait — registration takes up to about 15 minutes to propagate — then **reboot the
   speaker**.
5. Open `https://scottmcnab.github.io/starplayer/cast-probe/sender.html` in **Chrome**,
   paste the application id into the input (it is remembered in `localStorage`), and cast
   to the speaker with the Cast button next to it.
6. Press **Probe**, then **Bench**, then optionally **Send file** with a real module —
   never a fixture from `crates/starplayer-s3m/tests/fixtures/`, which must never reach a
   published page.
7. Press **Copy** and paste the transcript into a `## N1 findings` section of
   `plans/apps/A4-master-plan.md`, one block per device model tested.

## Notes

- No `coi-serviceworker.js` here, on purpose. That shim (used by the web player's own
  Pages build) registers a service worker that claims the whole Pages origin's scope for
  itself; a diagnostic receiver page is exactly the place that kind of surprise does not
  belong, and the probe needs no `SharedArrayBuffer` or `crossOriginIsolated` state of its
  own to do its job — it only *reports* what the speaker's own page saw.
- `sender.html` needs no COOP/COEP headers either: it neither instantiates wasm nor uses
  `SharedArrayBuffer` itself.
- `receiver.js`'s worklet-thread bench reassembles a second, independent copy of the
  wasm-bindgen glue at runtime (fetching `starplayer_host_wasm.js` fresh and prefixing the
  `TextDecoder` polyfill) rather than depending on `apps/starplayer-web`'s build-time
  worklet bundle — the two apps are deliberately independent Pages outputs.
