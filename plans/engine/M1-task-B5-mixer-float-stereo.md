# M1-task-B5 — Float stereo mixing, interpolation and output conversion

| Field | Value |
|---|---|
| Milestone | M1 ([master plan](M1-master-plan.md)) |
| Depends on | M0-A3 (voice pool, quantised skeleton) |
| Blocks | M1 exit |
| Parallel with | B1, B2, B3, B4 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests + determinism test still green) |

## Context for a fresh agent

M0-A3 built enough mixer to render one voice and prove block-size determinism. This task
makes it a real mixer: stereo, panning, loop handling, linear interpolation, volume
ramping, and the full set of output formats.

Read `plans/product/01-technical-architecture.md` §7. Two constraints shape everything:

- **The interpolator is a monomorphised type parameter of the inner loop**, never a
  `dyn` call per sample.
- **No transcendental functions in the RT path** (§7.3). `sin`, `exp` and `powf` differ
  between libm implementations, so using them would break cross-target determinism.
  Tables only — which is what trackers do anyway.

## Deliverables

1. **Stereo mixing with panning.** Per-channel accumulation buses, then summing. Panning
   from `VoiceParams::pan` (`I1F15`). Use a **table-driven** pan law, not `sin`/`cos`;
   document which law (constant-power vs linear) and why.

2. **Loop handling** in the voice render kernel: `None`, `Forward` and `PingPong`.
   Ping-pong is declared in the model at B1 and not used by S3M — implement it now if it
   is cheap, otherwise leave a documented `todo!()`-free stub that returns an error at
   load time and cite M5.

   The loop-wrap arithmetic is the classic source of off-by-one clicks. The original
   handled it by recomputing the number of output samples available before the loop end
   and mixing in bounded runs (`plans/reference/original-s3mlib-analysis.md` §7,
   `Mixer_8bitMono`) — that structure is worth copying even though the mixer itself is
   not.

3. **Linear interpolation**, plus the existing nearest. Guard frames (B1) mean the inner
   loop needs no branch at the loop point — verify that is actually true and that the
   guard count is sufficient.

4. **Volume and pan ramping.** The original's GUS driver used hardware volume ramping to
   make every retrigger click-free, and that click-free reputation is worth preserving.
   Ramp volume and pan over a short fixed number of frames on a change, and ramp a voice
   out rather than cutting it on a hard stop.

   Ramping must not break block-size determinism: the ramp is a function of frames
   elapsed since the change, not of where a block boundary fell.

5. **Output conversion**: 8-, 16-, 24- and 32-bit signed integer, plus f32; mono and
   stereo. Dithering (TPDF) for the reduced-depth cases, off by default and deterministic
   when on — a seeded generator, not a system RNG, or offline renders stop being
   reproducible.

6. **Master volume and clipping.** A soft limiter on the master bus, table-driven. Note
   for context: the original realised "amplification" as a clipping *curve* rather than a
   multiply (`PostTable`), which is a nice idea, but its index was unbounded — accuracy
   policy **D4**. Clamp explicitly here.

## Research points

1. Whether the per-channel buses should be `f32` interleaved or planar. Planar is
   friendlier to SIMD in M7; interleaved is simpler now. Pick one and note the cost of
   changing later.
2. The right ramp length. Too short and clicks return; too long and fast retriggers
   (`Qxy`) lose attack. The original's GUS ramps were hardware-timed; find an equivalent
   in frames and record the reasoning.
3. Whether the fixed-point path should land here or wait for M2. It is the canonical
   bit-exact golden reference (§7.3), and M2's goldens need it — but M1 has no goldens.
   Recommendation: leave it for M2-task-C6 and keep the generic structure that admits it.

## Verification

- The M0 block-size determinism test still passes, at every block size, with stereo,
  panning, interpolation and ramping all active.
- Ramping determinism: a volume change at a frame that is not a multiple of 128 produces
  identical output at block sizes 1, 3, 64, 128, 4096 and 8191.
- A forward-looping sample plays across the loop point with no discontinuity — assert the
  sample-to-sample delta at the wrap never exceeds the maximum delta elsewhere in the
  loop.
- Pan hard left produces silence in the right channel and full amplitude in the left.
- Output conversion round-trips: f32 → i16 → f32 stays within one LSB.
- No NaN or infinity in the output for any input, including a zero-length sample, a
  zero step, and a step larger than the sample length.
- No allocation in the render path.

## Out of scope

Cubic and windowed-sinc interpolation, SIMD, and any DSP effects — all M7. The
fixed-point mixer path — M2-task-C6.
