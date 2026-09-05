# M7 — DSP graph and effects

| Field | Value |
|---|---|
| Goal | Per-channel inserts, master bus, and the effect set |
| Estimate | 1.5u |
| Depends on | M6 |
| Trigger | **Pull-driven.** Start when either the owner wants per-channel effects for real use, or a UI milestone needs them |

## Context

The architecture has reserved space for this from M0: the render loop's DSP call sites
exist and take **whole 128-frame quanta**, which is what makes output independent of the
host's buffer size (architecture §1.4). This milestone fills them in.

The determinism constraint is the thing to hold onto: every effect must be a pure
function of its input block and its own state, with no dependence on where a host block
boundary fell. The M0 block-size determinism test is the gate, and it must still pass
with the full graph active.

## Deliverables

1. **The graph** — per-channel insert chains and a master bus, with a fixed topology
   (not a general node graph; that is more than this needs).
2. **Effects** — reverb, chorus, delay, compressor, EQ. Table-driven throughout; no
   transcendentals in the RT path.
3. **SIMD kernels** for the mixer and the heavier effects, via `core::simd` behind a
   feature, with a **scalar-equivalence test** gating them: SIMD is an optimisation, never
   a semantic change.
4. **Higher-order interpolation** — cubic (Hermite) for quality real-time, windowed sinc
   for offline rendering. Note that adding an interpolator produces new golden hashes by
   design, because the configuration is encoded in the golden filename (M2-C6).
5. **Parameter smoothing** so automation does not click, using the same
   frames-since-change discipline as the mixer's volume ramping.

## Exit criteria

Reverb on channel 1 alone, audible and correct; the block-size determinism test still
byte-identical with the full graph active; SIMD and scalar paths agree.

## Out of scope

Hosting third-party effect plugins — M9.

## The task graph (planned 2026-09-05)

Pulled by the owner on 2026-09-05, after M3–M6 landed. Task letter **H**. Six decisions
were taken while planning, each recorded here so the task files can cite them rather
than re-argue them:

1. **Per-channel buses are always on.** `VoicePool::accumulate_masked` stops summing
   every voice into one accumulator in slot order and sums each voice into the bus of
   `tag.channel` instead (voices beyond the bus count go to a spill lane). The buses are
   summed channel-major into the pre-master mix. On the fixed path `i32` saturating
   addition only differs from slot order if an intermediate sum saturates, which no
   real module reaches, so the nine goldens must stay byte-identical (H1 proves it); the
   float path's last bits move, which no golden pins and the perceptual nightly
   tolerates. The scope taps keep sampling voice state (§9.3) — reading the bus would be
   a follow-up, not part of this milestone.
2. **The `Insert` trait lives in `starplayer-dsp`** and is generic over a `DspSample`
   arithmetic trait implemented for `f32` and `i32`, so every effect exists on both mix
   paths from the day it lands and the fixed path stays cross-target bit-identical. The
   trait has its two implementations from the day it is committed — the H1 gain insert
   and the H3/H4 effects — which is what design goal 8 requires. `Stereo<T>` moves down
   from `starplayer-mixer` to `starplayer-dsp` and is re-exported from where it was.
3. **Topology is fixed**: `MAX_INSERTS_PER_CHAIN = 4` ordered slots per channel bus and
   four more on the master bus, ahead of the existing master volume and limiter. Slot
   order is processing order. Not a node graph.
4. **Inserts are built off the audio thread and installed by command.** Building an
   effect allocates its delay lines, so it arrives boxed over an engine-side control ring
   and leaves through a garbage channel, exactly as `Arc<Module>` does. Parameter changes
   are small copies on the same ring, drained at the top of a quantum with the other
   commands, and every audible parameter is smoothed inside the effect with the mixer's
   frames-since-change discipline — that is deliverable 5, folded into H1 because the
   first effect needs it.
5. **Coefficients are cooked on the audio thread from tables, never from `libm`.** A
   parameter change is a command, not a rebuild, so cooking has to be RT-safe, and §7.3
   bans transcendentals. `2^x` comes from `LINEAR_FREQUENCY_TABLE`, `log2` from a mantissa
   table, `sin`/`cos` from a quarter-wave table (H2). Parameters are integers in fixed
   units (centi-dB, frames, cents, percent) so the fixed path never touches a float.
6. **SIMD via the `wide` crate, not `core::simd`.** `core::simd` is nightly-only and the
   toolchain is pinned to stable 1.97. `wide` is `no_std`, safe code, falls back to scalar
   on `riscv32imc`, and covers SSE2/NEON/simd128. The scalar bodies stay compiled in every
   build; the `simd` feature selects the wide bodies, and `cargo xtask goldens --check`
   with the feature on is the equivalence gate alongside a property test.

Interpolators get an 8-frame **pre-roll** ahead of every sample (the leading guard
`starplayer_core::sample` deferred to M7) so a symmetric kernel can read `index − 1..3`
without a branch; the linear kernel never reads it, so the linear goldens do not move.

| ID | Task | Depends on | Parallel with | Model |
|---|---|---|---|---|
| H1 (landed 2026-09-05) | [Channel buses, the `Insert` trait, install/param plumbing, smoothing](complete/M7-task-H1-channel-buses-and-insert-graph.md) | — | H2, H5 | Opus |
| H2 (landed 2026-09-05) | [DSP primitives: `DspSample`, tables, biquad, delay line, LFO](complete/M7-task-H2-dsp-primitives.md) | — | H1, H5 | Sonnet |
| H3 | [EQ, delay, chorus](M7-task-H3-eq-delay-chorus.md) | H1, H2 | H4 | Opus |
| H4 | [Reverb, compressor](M7-task-H4-reverb-and-compressor.md) | H1, H2 | H3 | Opus |
| H5 (landed 2026-09-05) | [Cubic and windowed-sinc interpolation, sample pre-roll](complete/M7-task-H5-cubic-and-sinc-interpolation.md) | — | H1, H2 | Opus |
| H6 | [SIMD kernels with the scalar-equivalence gate](M7-task-H6-simd-kernels.md) | H3, H4, H5 | — | Opus |
| H7 | [Host wiring: `Player`, CLI, wasm host, web page](M7-task-H7-host-wiring.md) | H3, H4 | H6 | Sonnet |

```
main ── H1 ∥ H2 ∥ H5 ──→ H3 ∥ H4 ──→ H6 ∥ H7
```

Shared files (merge order H1, H2, H5, then H3, H4, then H7, H6):
`crates/starplayer-dsp/src/lib.rs` and `effects/mod.rs` (everyone in dsp — keep the
`pub mod` / `pub use` lines one per line), `crates/starplayer-mixer/src/{path,voice}.rs`
(H1) against `{kernel,sample}.rs` (H5), `crates/starplayer-engine/src/engine.rs` (H1)
against the arms in `crates/starplayer-host/src/engine.rs` and the wasm host (H5, H7),
`plans/product/01-technical-architecture.md` §7 (everyone — append, do not rewrite).

## Follow-ups surfaced while landing (2026-09-05)

- **CLI interpolator choice**: `apps/starplayer-cli`'s `InterpArg` still offers only nearest
  and linear; cubic and sinc reach the hosts and the web page in H5 but the CLI's arms are
  H7's.
- **Deferred loop wrap residuals** (H5 research resolution): a ping-pong loop's bottom turn
  still reads below `loop_start`, and a sustain-loop sample's guard is real tail PCM rather
  than a loop copy, so a wide kernel plays up to three frames of tail per wrap. Fixing the
  second needs `SampleRegion` to say whether its guard is a continuation. Not scheduled.
- **IT loader pattern-blob amplification**: a fuzz-smoke run found a 37 KB input expanding to
  a 23.6 MB pattern blob (4100 patterns, zero samples) that trips the harness memory cap. The
  IT loader lacks the aggregate decoded-pattern budget the S3M loader has. Deserves its own
  task under M6 acceptance; not part of M7.
