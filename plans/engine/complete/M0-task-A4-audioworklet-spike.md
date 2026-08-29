# M0-task-A4 — AudioWorklet spike: a Rust sine wave in a browser

| Field | Value |
|---|---|
| Milestone | M0 ([master plan](M0-master-plan.md)) |
| Depends on | A1 (workspace) |
| Blocks | M1-task-B7 (web player) |
| Parallel with | A2, A3 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (headless browser) + **owner (audible check in a real browser)** |
| Settles | Architecture open question Q1 |

## Context for a fresh agent

The owner's first chosen deliverable is a WASM web player, and this path carries the
project's highest uncertainty. This task proves the whole plumbing **with a sine wave,
before any engine exists**, so that when M1's tracker code arrives, an audio bug can be
isolated from a plumbing bug.

The known hazards, all of which this spike must actually exercise rather than assume:

1. **`SharedArrayBuffer` requires COOP/COEP headers.** Without
   `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy:
   require-corp`, `SharedArrayBuffer` is unavailable. The dev server must set them, and
   the design must have a `postMessage` fallback for hosts that cannot.
2. **No `fetch` or dynamic `import` inside a worklet** in some browsers. The wasm module
   generally has to be transferred into the worklet as bytes via `postMessage` and
   instantiated there.
3. **A hard 128-frame render quantum.** `AudioWorkletProcessor.process()` is always
   called with 128 frames. This is why `RENDER_QUANTUM` is 128 (architecture §1.4) — the
   match is free, and this spike confirms it.
4. **Growing wasm memory on the audio thread causes an audible glitch.** Memory must be
   pre-reserved at instantiation.
5. **`wasm-bindgen` output needs manual massaging for worklet scope** — the generated
   glue assumes a window or worker global that a worklet does not have.

Read `plans/product/01-technical-architecture.md` §§1.4, 8 and 9. Note §9's telemetry
split: scope data wants `SharedArrayBuffer`, coherent scalar state does not.

## Deliverables

1. **`crates/starplayer-host-wasm`** — the AudioWorklet glue: wasm-side entry points for
   `init(sample_rate, channels)` and `process(output_ptr, frames)`, plus the command and
   telemetry ring wiring (rings may carry dummy payloads at this stage).

2. **`apps/starplayer-web`** — a minimal page: a start button (browsers require a user
   gesture before audio), a frequency slider, and a readout of the telemetry round-trip.
   No framework. This page grows into the real player in M1-task-B7.

3. **The worklet processor** in JavaScript: receives the wasm bytes via `postMessage`,
   instantiates in worklet scope, and calls into Rust for each 128-frame quantum. Pre-
   reserve wasm memory at init; never grow it in `process()`.

4. **A command path**: the frequency slider changes the sine's pitch via a lock-free
   SPSC ring from the main thread into the worklet — proving the real control plane
   shape, not a `postMessage` per parameter change.

5. **A telemetry path**: a peak level published back out and rendered on the page.
   Implement it with `SharedArrayBuffer` **and** with a `postMessage` fallback, and make
   the page report which one it is using.

6. **`xtask wasm`** — builds, runs `wasm-bindgen`, applies whatever worklet-scope
   massaging is needed, and emits a servable directory. Plus a dev server (Node 24 is
   installed) that sets the COOP/COEP headers.

7. **A written answer to Q1** appended to
   `plans/product/01-technical-architecture.md` §9: does `SharedArrayBuffer` work well
   enough in practice for scope telemetry, or is `postMessage` the practical default?
   State what was tested and in which browsers.

## Research points

1. Current best practice for instantiating wasm inside an AudioWorklet — this has
   changed across browser versions; verify against real browsers rather than an old
   blog post.
2. Whether `wasm-bindgen`'s `--target web` output can be used in worklet scope with a
   small shim, or whether `--target no-modules` is the cleaner base.
3. Whether `wasm-pack` adds anything over `cargo build` + `wasm-bindgen-cli` here. Prefer
   the smaller toolchain if it is sufficient.
4. Whether Safari's COOP/COEP and worklet behaviour differs enough to need a separate
   path. Record the answer even if the fallback covers it.

## Verification

- `cargo xtask wasm && <dev server>` serves a page that, on clicking start, **plays an
  audible sine wave** in Chromium and in Firefox.
- Moving the slider changes the pitch with no audible glitch and no `postMessage` per
  change.
- The telemetry readout updates, and the page correctly reports which transport it chose.
- Audio runs for at least 60 seconds with no dropouts, no console errors, and no wasm
  memory growth (assert by logging `memory.buffer.byteLength` at start and end).
- **Owner check**: the sine is audible and clean in the owner's own browser.

## Out of scope

Any module loading, any engine, any real UI. The command ring may carry a single
`SetFrequency` variant and the telemetry a single peak value; the real vocabularies come
in M1.
