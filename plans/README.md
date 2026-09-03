# StarPlayer — Plans

Design and implementation plans for the whole project: the reusable Rust engine
(`crates/`), the applications built on it (`apps/`), and the archaeology of the original
1990s DOS sources it revives (`STARPLAY/`, read-only).

```
plans/
├── README.md                  # this file — unified status index
├── 00-scoping-overview.md     # the approved scoping plan (2026-08-28) — historical record
├── product/                   # foundation docs: vision, architecture, roadmap, accuracy
├── reference/                 # archaeology of the original DOS player
├── engine/                    # M<n>-* engine plans          (+ complete/)
└── apps/                      # A<n>-* application plans     (+ complete/)
```

- **`plans/product/`** — the foundation documents and the source of truth for direction.
  [`01-technical-architecture.md`](product/01-technical-architecture.md) is the
  authoritative engine design; a change that contradicts it needs an amendment there in
  the same commit. [`03-accuracy-policy.md`](product/03-accuracy-policy.md) is the
  authority on what "accurate" means per format and records **every** deliberate
  deviation from the original — a deviation that is not written down there is a defect,
  not a decision.
- **`plans/reference/`** — what the original actually did.
  [`original-s3mlib-analysis.md`](reference/original-s3mlib-analysis.md) is the
  *specification* for MOD/S3M/MTM effect semantics;
  [`original-star-ui.md`](reference/original-star-ui.md) is the design reference for the
  TUI homage. Both were reconstructed from sources that are **gutted** — most of the
  engine and front-end sit inside TASM `comment %` blocks, so the tree does not assemble
  and the shipped `STAR.EXE` is itself a crippled build — but the text is complete.
- **`plans/engine/`** — a master plan per milestone, `M<n>-master-plan.md`, plus per-task
  work plans, `M<n>-task-<ID>-<name>.md`. Written so an agent with no access to prior
  conversation history can pick up a master plan and dispatch its task files as written;
  each task file states its dependencies, what it can run concurrently with, and the
  model tier it is intended for.
- **`plans/apps/`** — application milestone plans, `A<n>-master-plan.md`.
- **`complete/`** under `engine/` and `apps/` — a plan moves there once its deliverable
  has landed on `main` and only owner-acceptance work remains. **Files directly under an
  area are, at a glance, the outstanding work.**

Task files record *how*; the `product/` documents record *what and why*.

## Status

Nothing is implemented yet. The repository currently holds the original DOS sources, this
plan set, and `AGENTS.md`.

### Engine

| Milestone | Status | Master plan | Notes |
|---|---|---|---|
| M0 Foundations + WASM spike | **Landed** — owner audible check pending | [M0](engine/M0-master-plan.md) | All four tasks in `engine/complete/`: [A1](engine/complete/M0-task-A1-workspace-skeleton.md) workspace/CI, [A2](engine/complete/M0-task-A2-core-types.md) core types, [A3](engine/complete/M0-task-A3-minimal-mixer-and-determinism.md) mixer + determinism test, [A4](engine/complete/M0-task-A4-audioworklet-spike.md) AudioWorklet sine wave. Exit: **sound from a browser tab** — `cargo xtask wasm && cargo xtask serve` |
| M1 S3M in the browser | **Complete** — owner listening check passed 2026-08-30; follow-ups B8 (zip) and B9 (output/mixer options) landed | [M1](engine/M1-master-plan.md) | 7 task files ready, B1–B7. [B4](engine/complete/M1-task-B4-s3m-effects.md) is the accuracy core and the largest single task. Exit: **a real `.s3m` plays in a browser** |
| M2 MOD + MTM native, accuracy machinery | **Complete** — owner accepted 2026-09-03. Every task file is in `engine/complete/`: [C1](engine/complete/M2-task-C1-trace-format.md) trace format/differ, [C2](engine/complete/M2-task-C2-conformance-harness.md) pinned conformance harness, [C3](engine/complete/M2-task-C3-mod-loader-and-effects.md) native MOD, [C3a](engine/complete/M2-task-C3a-mod-headphone-panning.md) MOD headphone panning, [C4](engine/complete/M2-task-C4-mtm-loader-and-effects.md) native MTM, [C6](engine/complete/M2-task-C6-fixed-point-mixer.md) canonical fixed-point goldens, [C10](engine/complete/M2-task-C10-mod-vblank-timing.md) VBlank timing, and the 2026-09-02 review follow-ups. [C8](engine/M2-task-C8-dos-reference-harness.md) stays deferred | [M2](engine/M2-master-plan.md) | Conformance stands at **33 of 47** (MOD 16/27, S3M 16/17, MTM 1/3) after [C2a](engine/complete/M2-task-C2a-conformance-harness-repairs.md) harness repairs, [C3b](engine/complete/M2-task-C3b-protracker-fidelity-repairs.md) ProTracker/MTM fidelity, [C9](engine/complete/M2-task-C9-s3m-conformance-repairs.md) S3M repairs and [C5](engine/complete/M2-task-C5-quirks-and-tempo-models.md) tracker dialects and [C10](engine/complete/M2-task-C10-mod-vblank-timing.md) VBlank timing detection; [C6a](engine/complete/M2-task-C6a-golden-and-build-hygiene.md) golden and build hygiene and [C7](engine/complete/M2-task-C7-fuzzing-and-rt-safety.md) fuzzing and the allocator hook also landed; all M2 tasks are complete pending the owner listening check; [C8](engine/M2-task-C8-dos-reference-harness.md) is **deferred** with an explicit trigger |
| M3 Native surfaces | **Landed** 2026-09-03 — [D1](engine/complete/M3-task-D1-song-timeline-and-loop-detection.md) timeline, [D2](engine/complete/M3-task-D2-natural-end-versus-loop.md) natural end, [D3](engine/complete/M3-task-D3-facade-format-dispatch.md) facade dispatch, [D4](engine/complete/M3-task-D4-cpal-host.md) host abstraction and cpal, [D5](engine/complete/M3-task-D5-cli-and-wav.md) CLI and WAV, [D6](engine/complete/M3-task-D6-scope-taps.md) scope taps, [D7](engine/complete/M3-task-D7-perceptual-comparison.md) libopenmpt perceptual nightly, [D8](engine/complete/M3-task-D8-cli-play.md) `starplayer play`; owner listening and scope checks outstanding | [M3](engine/M3-master-plan.md) | Exit criteria met: `starplayer play` on Linux, `render` reproduces the browser's golden byte for byte, scopes reach the page without blocking the audio thread. The wasm retrofit behind `AudioBackend` is a deferred follow-up (owner, 2026-09-03) |
| M4 Generalise to a synthesis engine | **Split** — M4-lite **landed** 2026-09-03: [E1](engine/complete/M4-task-E1-instrument-and-sample-model.md) instrument/sample model, [E2](engine/complete/M4-task-E2-linear-frequency-and-format-dialects.md) linear-frequency table and dialects, [E3](engine/complete/M4-task-E3-voice-lifecycle-and-trace-v2.md) voice lifecycle and trace v2; M4-full (MIDI, `Instrument` trait) deferred | [M4](engine/M4-master-plan.md) | Shared machinery for XM/IT, no `Instrument` trait until the MIDI sample player exists (owner, 2026-09-03) |
| M5 XM support | **In progress** — [F1](engine/complete/M5-task-F1-xm-loader.md) landed the loader and [F2](engine/complete/M5-task-F2-xm-playback.md) landed playback: FastTracker 2's effect processor, envelopes, key-off and fadeout, auto-vibrato, linear and Amiga periods, the volume column, and the whole XM half of the conformance corpus — 72 of 93 cases pass, with accuracy-policy D42–D47 and twelve `F2-XM-*` records. F2 absorbed F4's wiring, so F3 and F5 are what remain per the [concurrency plan](engine/M3-M6-concurrency-plan.md) | [M5](engine/M5-master-plan.md) | Envelopes, key-off/fadeout, linear frequency |
| M6 IT support | **In progress** — [G1](engine/complete/M6-task-G1-it-loader.md) the IT loader landed; [G3](engine/complete/M6-task-G3-it-playback.md) landed IT playback: `ItProcessor` with per-voice articulation, New Note Actions, the duplicate check and OpenMPT's voice-stealing rule (architecture **Q3**, now settled), the effect set and the volume column, `TempoModelId::ItModern` filled in, `ItLoopDialect`, and 121 pinned IT corpus cases wired — 39 pass, 82 recorded as `G3-IT-001`…`008` for G6. G2 (filter) is concurrent; G4–G6 follow per the [concurrency plan](engine/M3-M6-concurrency-plan.md) | [M6](engine/M6-master-plan.md) | NNA/DCT/DCA, voice stealing, resonant filter. The hardest format |
| M7 DSP graph | **Pull-driven** | [M7](engine/M7-master-plan.md) | Per-channel inserts, master bus, SIMD |
| M8 Embedded proof | **Pull-driven** | [M8](engine/M8-master-plan.md) | esp32 RISC-V from flash under embassy |
| M9 Plugin surfaces | **Pull-driven** | [M9](engine/M9-master-plan.md) | CLAP instrument, then effect hosting |
| M10 Alternative synths | **Pull-driven, open-ended** | [M10](engine/M10-master-plan.md) | Wavetable, FM, SoundFont, SID; sample-enhancement API |
| M11 Instrument library | **Pull-driven, open-ended** | [M11](engine/M11-master-plan.md) | A library of tracker modules' instruments, playable from MIDI. Shares `InstrumentBank` with M10's SoundFont support |

### Applications

| Milestone | Status | Master plan | Notes |
|---|---|---|---|
| Web player | Not started | delivered by [M0-A4](engine/complete/M0-task-A4-audioworklet-spike.md) + [M1-B7](engine/complete/M1-task-B7-web-player.md) | The primary UI and the first deliverable |
| A1 TUI STAR.EXE homage | **Pull-driven** | [A1](apps/A1-master-plan.md) | The nostalgic one. Second consumer of the telemetry API |
| A2 Desktop shells | **Pull-driven** | [A2](apps/A2-master-plan.md) | Ask whether it is needed at all first |
| A3 Mobile shells | **Pull-driven** | [A3](apps/A3-master-plan.md) | Lowest priority; the responsive web player covers most of it |

**Critical path**: M0 → M1. Everything else is either proving what those two built (M2,
M3) or extending it (M4 onward). From 2026-09-03 the rest of M3, M4-lite, M5 and M6 run
concurrently; [`engine/M3-M6-concurrency-plan.md`](engine/M3-M6-concurrency-plan.md) is
the dependency graph and merge order.

## Locked-in decisions

Made by the project owner during scoping (2026-08-28); do not re-litigate in derived
plans. Full statements in [`product/00-vision.md`](product/00-vision.md).

1. **First audible deliverable is the WASM AudioWorklet web player**, not a CLI. The
   browser is the hardest host; proving it first de-risks everything after it.
2. **Semantic fidelity, modern mixing.** Replicate the original's per-tick effect
   behaviour; render with a modern mixer. Emulating the 8-bit mono SoundBlaster mixer as
   a "retro mode" is explicitly not scoped.
3. **Canonical behaviour is the fidelity reference, deviations documented.** The assembly
   is the primary specification, but where it deviates from ST3/ProTracker through an
   outright defect, implement the canonical behaviour and record it in
   [`product/03-accuracy-policy.md`](product/03-accuracy-policy.md) §3 (seven such
   deviations, D1–D7, are already identified).
4. **MOD and MTM get native effect processors.** The original converted them to S3M in
   memory; that is why its MOD playback was inaccurate. Its conversion tables are
   replicated as MOD/MTM *semantics*, not as a lowering step.
5. **DOS reference capture is deferred** — read the assembly as the spec; reconstruct a
   buildable DOS reference only if the port hits an ambiguity the source cannot settle.
6. **`no_std` + `alloc` from day one**, CI-enforced on a bare-metal target.
7. **Licence deferred.** The repo stays private; no licence files or SPDX headers yet.

## How to use these documents

1. Read [`product/00-vision.md`](product/00-vision.md) for what this is, then
   [`product/01-technical-architecture.md`](product/01-technical-architecture.md) for how
   it works. Those two answer most questions.
2. Execute milestones in order via their master plans and task files. Start at
   [M0](engine/M0-master-plan.md).
3. Before writing any MOD/S3M/MTM effect code, read
   [`reference/original-s3mlib-analysis.md`](reference/original-s3mlib-analysis.md) §4
   and [`product/03-accuracy-policy.md`](product/03-accuracy-policy.md). The first is the
   specification; the second lists the seven places we deliberately do not follow it.
4. When a design decision changes, update the foundation document in `product/` — it is
   the source of truth. When implementation discovers a new deviation from the original,
   add it to the accuracy policy **in the same commit as the code**.
5. `STARPLAY/` is a read-only historical reference and must never be modified. Derived
   artefacts (such as the deferred DOS reference reconstruction) live elsewhere.
