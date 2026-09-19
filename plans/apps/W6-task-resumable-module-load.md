# W6 — A resumable module load, so the audio thread never blocks

| Field | Value |
|---|---|
| Milestone | Web player maintenance (follows [W4](complete/W4-task-enhancement-checkboxes.md) and [W5](W5-task-speed-adjust-and-options-disclosure.md)) |
| Status | Specified 2026-09-19; not started |
| Depends on | [W4](complete/W4-task-enhancement-checkboxes.md) (the enhancement checkboxes and `enhancements_json()`) |
| Blocks | — |
| Recommended model | Claude Opus (a new resumable API in the RT realm and a page↔worklet protocol; the granularity question in Research point 1 has to be measured, not guessed) |
| Verified by | agent (`cargo test -p starplayer-host-wasm -p starplayer-model`, `cargo xtask wasm && node apps/starplayer-web/test/headless.mjs`), then the owner in a browser on a sample-heavy XM with every enhancement ticked |

## Context for a fresh agent

Read `CLAUDE.md`, `apps/starplayer-web/README.md` and
[W4](complete/W4-task-enhancement-checkboxes.md).

### The defect

Loading a module in the web player runs the whole decode-and-enhance pipeline **inside the
worklet's `loadModule` message task**, which is on the audio rendering thread. The
`AudioWorkletGlobalScope` event loop *is* the render thread, so `process()` cannot run
while that handler works. The output is starved for the whole load; Chrome then catches
the render loop up faster than real time, and the opening of the newly loaded song is
consumed by that catch-up instead of being played.

The owner's report: switching songs sometimes drops the first seconds of the new song,
which starts already underway.

### What was measured, 2026-09-19

Nothing else is at fault. Each of these was measured, not inferred:

* **The engine always starts a new module at the top.** Fourteen browser song switches at
  random offsets, discriminated on the new module's own `song_length_frames` so stale
  telemetry could not be mistaken for a skip: every one began at song frame 0, order 0,
  row 0. A 256-order, 256-pattern, 64-channel XM loads in **832 µs** and a
  play → Stop → reload sequence restarts it at `order=0 row=0 song_frame=0`. The ZIP
  route, the Stop button and reloading the same module are all clean.
* **The load renders nothing.** A deliberate 1.5 s block inside the worklet's `loadModule`
  handler, with `currentFrame` read either side: `52224 -> 52224`. Zero frames.
* **A stalled load reproduces the symptom exactly.** With that block in place, the first
  sight of the new module is `0:00 / row 03:2` — already three rows in — and it then
  advances one row per ~20 ms, i.e. normally. Identical whether the previous song was
  playing or stopped, which is what the owner reported.
* **The cost is the enhancement rebuild, and only that.** One load of a 2 MiB
  sample-heavy MOD, native release build (wasm is several times slower again):

  | Rendering option | load |
  |---|---|
  | nothing ticked | **3.1 ms** |
  | Smooth loop seams | 3.7 ms |
  | Reduce decay noise | 9.3 ms |
  | Extend sample bandwidth | **280 ms** |
  | Upsample samples (4× sinc) | **518 ms** |
  | all four together | **1.61 s** |

  All four cost more than the sum of the parts because `CATALOGUE` order is signal-chain
  order — `denoise → sinc4x → sbr → loop` — so once the upsampler has quadrupled the frame
  count, `sbr` and the loop smoother each work on 4× the data.

  The owner confirmed the shape from the other end: every box ticked lost about four
  seconds, unticking the upsampler alone reduced it to about two, and unticking everything
  removed it entirely.

So decode (~3 ms) and the song scan (<1 ms even for a 256-pattern XM) are not worth
chunking. **The enhancement rebuild is the whole problem.**

### Code map

* `crates/starplayer-model/src/enhance.rs:147` — `Module::enhanced(&self, &dyn SampleEnhancer)`.
  It copies the header, orders, instruments and patterns (all cheap), then walks
  `self.samples()` calling `enhancer.enhance(SamplePcm { .. })` once per sample and
  `builder.add_sample(..)` with the result. **That per-sample loop is the seam.**
* `crates/starplayer-host-wasm/src/lib.rs:204` — `decode(bytes, headphone, flags, ceiling)`:
  probe → per-format loader → `build_enhancement` → `module.enhanced(&chain)`.
* `crates/starplayer-host-wasm/src/lib.rs:244` — `build_enhancement`, which walks
  `CATALOGUE` and applies `ENHANCE_FRAME_BUDGET`'s 4x → 2x → identity fallback. Leave its
  policy exactly as it is; it only has to be reachable from the new stepped path.
* `crates/starplayer-host-wasm/src/lib.rs:401` — `Host::load_module_with_options`, which
  calls `decode`, then `player.set_speed_adjust`, then `player.load_module`.
* `crates/starplayer-host-wasm/src/lib.rs:731` — the `wasm_bindgen` export of the same name.
* `apps/starplayer-web/www/worklet-processor.js:120` — the `loadModule` message task.
* `apps/starplayer-web/www/app.js:814` — `activateModuleOnNode`, which posts `loadModule`
  and resolves on `moduleLoaded`; `:843`/`:881` — `onWorkletMessage` and its
  `moduleLoaded` / `moduleError` arm; `:834` — `updateModPanningAvailability`, the
  load-option availability lock; `:1291` — `applyLoadOptions`, the in-place reload.

### Why the page has to drive the steps

`AudioWorkletGlobalScope` has no `setTimeout` and no task source of its own, and a
microtask does not yield to the render loop. The only way to return to the render thread
between slices is to finish the message task and be re-entered by another message. So the
step loop is: worklet posts progress to the page → the page posts the next step → the
worklet does one slice. Each round trip is a task on both sides, and `process()` runs in
between. `app.js` already owns the load sequence, so it is the right driver.

## Deliverables

### 1. A resumable rebuild in `starplayer-model`

Add a resumable counterpart to `Module::enhanced` — for example an `EnhanceInProgress`
holding the source `Module`, the `ModuleBuilder`, the chain and a sample cursor:

* `begin(module, enhancer)` — everything `enhanced` does before the sample loop.
* `step(budget)` — enhance samples until `budget` source frames have been consumed,
  returning progress (samples done / total, or a `Done`/`More` verdict).
* `finish()` — `builder.build()`.

`Module::enhanced` must keep working and must stay **byte-identical** to the stepped path:
implement `enhanced` in terms of the new type with an unbounded budget, so there is one
implementation and no second version to drift.

### 2. A stepped load in the wasm host

Split `Host::load_module_with_options` into three, keeping the existing one-shot method as
a wrapper (the tests and `load_module` use it):

* `begin_load(bytes, headphone, flags, speed_adjust)` — probe, decode, `build_enhancement`.
  Returns the total work (source frames, or sample count) so the page can show progress,
  and `0` when no enhancer was asked for, meaning the caller may go straight to finish.
* `step_load(budget)` — one slice. Returns whether more remains.
* `finish_load()` — `player.set_speed_adjust`, `player.load_module`, the module generation,
  and the applied-flags/factor pair `applied_enhancement_flags` and
  `applied_enhancement_factor` already report.

Export all three through `wasm_bindgen`. A load already in progress when another
`begin_load` arrives is **abandoned**, not queued: the page serialises loads through its
own availability lock, and a half-built module must never reach the engine.

`finish_load` allocates and is a message task like the current load, exactly as
`set_midi_input` and `install_insert` are — no part of this may move into `process()`.

### 3. The worklet protocol

`worklet-processor.js` handles `loadModule` by calling `begin_load` and then posting
`loadProgress { requestId, done, total }` instead of finishing. A new `loadStep` message
runs one `step_load` and posts the next `loadProgress`; when nothing remains it calls
`finish_load`, rebinds the views (module activation may have consumed pre-reserved heap,
exactly as today) and posts the existing `moduleLoaded` message unchanged.

Any error at any step posts the existing `moduleError` and drops the partial state.

### 4. The page's step loop

`activateModuleOnNode` keeps its single-promise contract — it resolves on `moduleLoaded`
and rejects on `moduleError`, so `activateLoadedModule` and `applyLoadOptions` do not
change. Internally it answers each `loadProgress` with a `loadStep`, and may use the
progress numbers to extend the existing "Activating …" status line with a percentage.

The existing load lock (`updateModPanningAvailability`, `state.pendingLoads`) already
prevents a second load starting mid-flight; confirm it still does, since a load now spans
many tasks rather than one.

## Research points

1. **Step granularity.** The `SampleEnhancer` trait takes a whole sample
   (`enhance(SamplePcm) -> …`), so the smallest natural slice is one sample. On the 2 MiB
   fixture above, 31 × 64 KiB samples at 4× sinc came to 518 ms, i.e. **~17 ms per
   sample** — still several render quanta. Measure the worst-case single-sample step for a
   realistic module and decide:
   * if a per-sample step fits inside the output's headroom, stop there;
   * otherwise either give the upsampler a frame-range entry point so a slice can cover
     part of a sample, or move the whole chain to the page-side wasm instance and send the
     worklet finished PCM. Record the decision and the measurement in this file.
2. **Round-trip cost.** Measure how long a page↔worklet round trip actually takes under
   load. If it dominates the slice, raise the budget per step rather than adding steps.
3. **Does the catch-up burst belong to Chrome or to the engine?** The stall experiment
   showed the engine several rows in within one animation frame of the swap, which implies
   Chrome burst-renders after a starved period. Worth confirming, because it bounds how
   much a *residual* stall still costs.

## Verification

* `cargo test -p starplayer-model` — the stepped rebuild and `Module::enhanced` produce
  identical modules for every enhancer in `CATALOGUE`, at several budgets including 1 and
  `usize::MAX`. This is the test that keeps the two paths from drifting.
* `cargo test -p starplayer-host-wasm` — `begin/step/finish` reaches the same module and
  the same `applied_enhancement_flags` / `applied_enhancement_factor` as the one-shot call,
  including through the `ENHANCE_FRAME_BUDGET` 4x → 2x → identity fallback; an abandoned
  load leaves the previous module playing.
* `cargo xtask wasm && node apps/starplayer-web/test/headless.mjs` — all three modes. The
  existing enhancement-checkbox assertions must pass unchanged, because the page's
  contract has not changed.
* **A regression test for the defect itself.** In the headless harness, tick every
  enhancement, load a module, and assert the worklet kept rendering throughout — for
  example that `currentFrame` advanced across the load, which is the measurement that
  proved the bug (`52224 -> 52224` before the fix). Without this the defect can come back
  silently.
* Owner: a sample-heavy XM from a ZIP, every box ticked, switching songs repeatedly. The
  new song must start at its beginning every time.

## Out of scope

* Chunking the decode or the song scan. Measured at ~3 ms and <1 ms; there is nothing
  there to win.
* Making the enhancers faster (SIMD on the sinc kernel, say). Worth doing on its own
  merits, and it would shrink the stall, but it cannot remove it — that is what this task
  is for.
* Caching an enhanced module so switching back to a recent track skips the rebuild.
  A reasonable follow-up once the load is stepped, not part of this.
* The cosmetic lag where the page shows the incoming module's title beside the outgoing
  module's row and elapsed for a frame or two after a swap.
* `Player::install` on a multi-threaded host (the cpal CLI) reads its anchor frame *before*
  the scan runs, so a slow load there makes the sequencer itself skip the opening. Latent,
  real, and a separate task — the browser is immune because the worklet is single-threaded.
