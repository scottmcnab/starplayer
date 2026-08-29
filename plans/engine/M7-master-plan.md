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
