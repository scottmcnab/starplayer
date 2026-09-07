# StarPlayer — Vision

## What this is

StarPlayer is a **reusable Rust tracked-music engine**, and a revival of a 1990s DOS
tracker player of the same name.

The original (`STARPLAY/`, ~7,700 lines of 80386 TASM assembly, built against Tran's
PMODE/W DOS extender) was known for two things: unusually accurate Scream Tracker 3
interpretation — better than Mikmod at the time — and click-free Gravis Ultrasound
output using hardware volume ramping with a mixing rate that adapted as the active
voice count rose. Its replay core (`S3MLIB.ASM`) was deliberately written as a reusable
library and was reused across several projects.

The revival keeps that intent and widens it. This is not a port of the assembly; it is a
new engine that inherits the original's *semantics* where they are right, and
generalises well past tracker patterns into a real-time, event-driven synthesis engine.

## What it must be good at

1. **Accurate tracker playback.** MOD, S3M and MTM first, interpreted per-tick the way
   the original did. Then XM and IT to their specifications.
2. **Being embedded in other things.** The engine is a library, not an application. The
   applications in `apps/` are consumers of it and carry no engine logic.
3. **Running everywhere.** WASM in the browser (the first target), native Linux/Windows/
   macOS, and `no_std` embedded — esp-rs with embassy is the concrete embedded target.
4. **Both real-time and offline.** The same engine, same code path, deterministic
   either way: low-latency interactive playback, and exact offline rendering.
5. **Being more than a module player.** Live MIDI input, MIDI file playback, keyboard
   triggering, non-sample instruments (FM, wavetable, SID, SoundFont, physical
   modelling), post-effects per channel and on the master bus, and eventually a VST/CLAP
   surface — so the same engine can back a tracker workstation or ship as a plugin.
6. **Being observable.** Live channel/row/effect state and per-channel oscilloscopes are
   a first-class API, not a debug hook. They are what every UI renders from, including a
   future tracker editor.

## Audiences

| Audience | What they get |
|---|---|
| The owner | A working StarPlayer again — in a browser, a terminal, and a CLI |
| Other projects of the owner's | A crate to drop in for music playback, native or embedded |
| The demoscene / chiptune community | An accurate, permissively-reusable player, eventually open source |
| Plugin users | StarPlayer as a CLAP/VST instrument |

## Reuse goals

All code lives in one workspace for now. The crate boundaries are drawn so that
extracting the engine for crates.io later is a manifest change, not a refactor:

- Each crate has a single responsibility and a one-directional dependency edge.
- `starplayer` is the facade and the intended public crate.
- Nothing in the `no_std` crates knows about files, threads, devices or UIs.
- Applications depend only on the facade.

## Decisions taken at scoping time (2026-08-28)

Made by the project owner; do not re-litigate in derived plans.

1. **First audible deliverable is the WASM AudioWorklet web player**, not a CLI. The
   browser is the hardest host, and proving it first de-risks everything after it.
2. **Semantic fidelity, modern mixing.** Replicate the original's per-tick
   effect/period/volume behaviour; render with a modern high-quality mixer. Emulating
   the original's 8-bit mono SoundBlaster mixer as a "retro mode" is explicitly *not*
   scoped.
3. **Canonical behaviour is the fidelity reference, deviations documented.** The
   assembly is the primary specification, but where it deviates from ST3/ProTracker
   through an outright defect, implement the canonical behaviour and record it in
   `03-accuracy-policy.md`.
4. **MOD and MTM get native effect processors.** The original converted them to S3M in
   memory; that conversion is why its MOD playback was inaccurate. Its conversion
   tables are replicated as MOD/MTM *semantics*, not as a lowering step.
5. **DOS reference capture is deferred.** Read the assembly as the spec; reconstruct a
   buildable DOS reference only if the port hits an ambiguity the source cannot settle
   (`plans/engine/M2-task-C8-dos-reference-harness.md`).
6. **`no_std` + `alloc` from day one**, CI-enforced on a bare-metal target. Retrofitting
   `no_std` later forces a rewrite of the IO and error layers.
7. **Code licence: MIT OR Apache-2.0** (decided 2026-09-07). `LICENSE-MIT` and
   `LICENSE-APACHE` sit at the repository root and every workspace member inherits
   `license` from `[workspace.package]`. Permissive because the engine is built to be
   embedded — WASM in other pages, `no_std` firmware, plugin hosts — where copyleft would
   bar most uses, and because it continues the owner's own 1996 terms:
   `STARPLAY-2.25s/SP-CODE.DOC` granted free use of the original code with a credit
   request, which MIT's attribution clause makes enforceable. The DOS sources in
   `STARPLAY/` and `STARPLAY-2.25s/` remain under those original terms. The **music
   licence** for the S3M fixtures is still deferred and is a separate decision from the
   code licence; until it is made the fixtures stay "owner's own work, for testing only"
   (see `crates/starplayer-s3m/tests/fixtures/README.md`).

## What StarPlayer is not

- Not a tracker *editor*. The telemetry surface is designed so one could be built on
  top, but composing is out of scope.
- Not a DOS emulator or a preservation project. `STARPLAY/` is kept as a reference and
  a historical record; reviving `STAR.EXE` itself is a contingency (see decision 5),
  not a goal.
- Not a general audio framework. It is a music engine with a device layer, not a DAW.
