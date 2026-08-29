# M2-task-C6 — The fixed-point mixer and golden hashes

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Depends on | M1-B5 (float mixer) |
| Blocks | M2 exit; M8 (embedded) |
| Parallel with | C1, C3, C4 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (cross-target hash equality in CI) |

## Context for a fresh agent

The float mixer is the default for desktop and browser. The **fixed-point mixer is the
canonical bit-exact reference** (`plans/product/01-technical-architecture.md` §7.3) and
the embedded path (M8).

Why bit-exactness needs the fixed path: x86 SSE2, ARM NEON and WASM SIMD agree on
`+ - * /` under IEEE-754 round-to-nearest, but they do **not** agree on `sin`, `exp` or
`powf` — different libm — and FMA contraction changes results. Integer arithmetic has no
such freedom. So goldens are hashed on the fixed path and floats carry a tolerance.

Note the corollary that has already shaped the design: **no transcendental functions
anywhere in the RT path**, on either mixer. Tables only.

## Deliverables

1. **The fixed-point voice kernel** — `i16` sample, `i32`/`i64` accumulator, Q32.32 step,
   nearest and linear interpolation. Structurally identical to the float kernel; the
   difference is the accumulator type, so the two should share their shape through
   generics rather than being two hand-written loops that drift apart.

2. **Fixed-point panning, ramping and output conversion**, matching the float path's
   *behaviour* (not its bits). Table-driven pan law, same ramp lengths.

3. **Disabled FMA contraction** for the float path, so the two paths' relationship is
   stable across compilers. Document how (a build flag, or `#[inline(never)]` boundaries)
   and verify it actually took effect.

4. **Golden hashes**: SHA-256 of a fixed-point **i16 mono 44100 Hz linear-interpolation**
   render with DSP bypassed. The filename encodes the configuration:
   ```
   goldens/s3m/<module>__i16_mono_44100_linear.sha256
   ```
   so changing the interpolator produces a *visibly new* golden rather than silently
   breaking every existing one. **Golden WAVs stay out of the repo** — only hashes are
   committed.

5. **`cargo xtask goldens`** to regenerate, with a `--check` mode for CI.

6. **Cross-target hash equality in CI**: render the same module on x86-64, aarch64 and
   wasm32 on the fixed path; assert identical hashes. This is the test that keeps the
   determinism claim honest.

7. **A float-path tolerance comparison** against the fixed path — not bit-exact, but
   within a stated segmental SNR — so a float regression that the goldens cannot see
   still gets caught.

## Research points

1. Whether aarch64 CI is available without hardware (qemu, or a hosted ARM runner). If
   neither is practical, degrade to x86-64 plus wasm32 and record the gap rather than
   quietly dropping the test.
2. The right rounding convention in the fixed accumulator. Truncation is cheapest;
   round-to-nearest is better sounding. Whichever is chosen becomes part of the golden
   contract, so choose deliberately and document it.
3. Whether module *loading* is deterministic enough to hash the `Module` itself as a
   cheaper first-line check. B1 made `Module` hashable for exactly this.

## Verification

- The same module renders to the same hash on two runs, at six different host block sizes.
- The same module renders to the same hash on x86-64 and wasm32 (and aarch64 if
  available).
- The float path stays within the stated SNR of the fixed path on the whole corpus.
- Changing the interpolator produces a differently-named golden, and `--check` reports a
  missing golden rather than a mismatch.
- The fixed path allocates nothing and produces no NaN/Inf (trivially, but assert it).

## Out of scope

SIMD (M7). Cubic and sinc interpolation (M7). The embedded build itself (M8).
