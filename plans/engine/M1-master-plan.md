# M1 — S3M in the browser

| Field | Value |
|---|---|
| Goal | **A real `.s3m` plays correctly in a browser tab** |
| Estimate | 2.5u |
| Depends on | M0 |
| Blocks | M2 (MOD/MTM + accuracy machinery) |
| Foundation docs | [01-technical-architecture](../product/01-technical-architecture.md), [03-accuracy-policy](../product/03-accuracy-policy.md), [original-s3mlib-analysis](../reference/original-s3mlib-analysis.md) |

## Why this milestone exists

M1 is the first musically useful deliverable and the one that proves the architecture.
It builds the whole vertical slice — module model, loader, sequencer, effect processor,
mixer, telemetry, UI — for exactly one format.

S3M is the right first format for three reasons: it is what the original was famous for
getting right; it is the format the original's replay core natively understood, so the
assembly is a direct specification rather than a conversion; and its effect set is a
superset of MOD's, so M2's MOD work is mostly a matter of *different* semantics rather
than new machinery.

The accuracy machinery deliberately does **not** land here. M1 gets S3M audibly right;
M2 adds MOD and MTM together with the trace diffing, conformance corpus and golden
hashes that prove all three. Building the harness before there is anything to test is
how a project spends a month and produces silence.

## Tasks

| Task | Summary | Depends on | Parallel with |
|---|---|---|---|
| [B1](complete/M1-task-B1-module-model.md) ✅ | `Module` blob-and-offsets layout, samples, instruments | M0-A2 | B5 |
| [B2](M1-task-B2-s3m-loader.md) | S3M parsing: header, parapointers, patterns, samples, panning | B1 | B3, B5 |
| [B3](complete/M1-task-B3-engine-render-loop.md) ✅ | Render loop, `RowClock`, sources, command queue, channel binding | M0-A3 | B2, B5 |
| [B4](M1-task-B4-s3m-effects.md) | The ST3 effect processor — the accuracy core | B2, B3 | B5 |
| [B5](M1-task-B5-mixer-float-stereo.md) | Linear interpolation, stereo, output conversion | M0-A3 | B1, B2, B3 |
| [B6](M1-task-B6-telemetry-v1.md) | Coherent scalar telemetry snapshot | B3 | B4 |
| [B7](M1-task-B7-web-player.md) | The web player UI | M0-A4, B6 | — |

B4 is the largest and most delicate task. It should be handed over on its own branch and
reviewed effect by effect.

## Exit criteria

1. A real `.s3m` — not a synthetic test file — loads and plays in a browser, sounding
   correct to the owner's ear.
2. Transport controls work: play, stop, seek by order position.
3. The UI shows order position, pattern, row, speed, BPM, and per-channel note,
   instrument, volume, pan and effect.
4. VU meters move, with the original's peak-hold-and-decay behaviour.
5. The M0 block-size determinism test still passes with a real module playing.
6. `cargo xtask ci` is green, including the `no_std` and wasm builds.

## Deliberate non-goals

No MOD, no MTM, no trace diffing, no golden hashes, no fuzzing, no native host, no CLI.
All of those are M2 and M3. Resisting them here is what keeps M1 to 2.5u.

## Note for whoever picks this up

Every task file in M1 cites `plans/reference/original-s3mlib-analysis.md` by section.
That document is the specification; `plans/product/03-accuracy-policy.md` §3 lists the
seven places (D1–D7) where we deliberately do **not** follow it. Read both before
starting B2 or B4.
