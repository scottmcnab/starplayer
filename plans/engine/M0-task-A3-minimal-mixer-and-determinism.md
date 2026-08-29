# M0-task-A3 — Minimal mixer and the block-size determinism test

| Field | Value |
|---|---|
| Milestone | M0 ([master plan](M0-master-plan.md)) |
| Depends on | A2 (core types) |
| Blocks | M1-task-B3 (engine render loop) |
| Parallel with | A4 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (determinism test at six block sizes) |

## Context for a fresh agent

This task creates just enough of `starplayer-mixer` and `starplayer-engine` to render
one voice, and — the real point — **establishes the block-size determinism invariant
before any format code exists**.

Read `plans/product/01-technical-architecture.md` §1.4. The rule is:

> Split for voice mixing; quantise for DSP. The engine renders on an internal fixed
> `RENDER_QUANTUM = 128` frames. Events split *within* a quantum for voice accumulation.
> The DSP graph and master bus only ever see whole quanta. Arbitrary host block sizes are
> adapted by a small output ring.

If per-channel DSP ever consumes a ragged 3-, 17- or 411-frame segment, output starts
depending on the host's buffer size: offline stops matching real-time, and neither
matches across hosts. It is the highest-probability silent failure in the design, and it
is cheap to make impossible — but only if the test exists from the beginning. That is why
this task lands in M0 rather than alongside the first real mixer work.

## Deliverables

1. **`Voice` and `VoicePool`** in `starplayer-mixer`:
   - `VoiceId { index: u16, generation: u16 }` — generational, because voices can be
     stolen out from under their owner (architecture §5.2).
   - `pool.get_mut(id) -> Option<&mut Voice>` so a stale handle is impossible to misuse.
   - Fixed capacity, no allocation after construction.
   - `VoiceTag { channel, instrument, sample, note }` — four bytes, present from the
     start so IT's Duplicate Check has somewhere to look in M6, and early-outed by every
     other format.

2. **Voice rendering**, generic over the accumulator type, with `f32` and fixed-point
   (`i16` sample, `i32`/`i64` accumulator) paths. Nearest and linear interpolation only
   for now; the interpolator is a monomorphised type parameter of the inner loop, never
   a `dyn` call per sample.

3. **Guard frames.** Sample data carries N appended frames (loop-wrapped for looping
   samples, zeroed otherwise) so the interpolator reads past the loop point without a
   branch in the inner loop. Define N and document why.

4. **The quantised render skeleton** in `starplayer-engine`:
   ```
   for each whole RENDER_QUANTUM:
       voice accumulation, split at event boundaries within the quantum
       (DSP hooks: no-ops for now, but the call sites exist and take whole quanta)
       output conversion → ring
   host block served from the ring
   ```
   The output ring adapts arbitrary host block sizes. `RENDER_QUANTUM = 128` is a
   constant now; architecture open question Q2 asks whether embedded wants a
   compile-time override — do not add the knob yet.

5. **A stub event source** that emits a parameter change at a chosen frame, so the test
   can prove events land at exact frames rather than at buffer boundaries.

6. **Output conversion** to f32 and i16, mono and stereo. The remaining depths (8/24/32
   integer, dithering) come in M1.

## Research points

1. Whether the output ring should hold converted output or pre-conversion accumulator
   frames. Converted is simpler; pre-conversion keeps the door open for a host that
   wants a different format per call. Pick one and record the reason.
2. The right guard-frame count for the interpolators planned in M7 (cubic needs more
   than linear). Choosing the larger number now costs a few bytes per sample and avoids
   a format-wide change later.

## Verification

**The determinism test is the deliverable.** Render an identical scenario — a looping
sample, a parameter change at a frame that is deliberately *not* a multiple of 128, and
at least 20,000 frames of output — at host block sizes **1, 3, 64, 128, 4096 and 8191**,
and assert every result is byte-identical.

Additionally:

- Assert the parameter change takes effect at exactly the requested frame, at every
  block size — the point of splitting within the quantum.
- A voice that ends returns to the pool; `voices_active()` returns to 0.
- A stale `VoiceId` (whose slot has been reused) yields `None` from `get_mut`.
- No allocation occurs after pool construction. If `assert_no_alloc` is not yet wired up
  (that is M2-task-C7's job), assert it by inspection and leave a `TODO` citing the task.

## Out of scope

Any real DSP. Cubic and sinc interpolation. SIMD. Any sequencer or format code. The DSP
call sites exist and take whole quanta, but they do nothing.
