# M1-task-B3 — Engine render loop, RowClock and sources

| Field | Value |
|---|---|
| Milestone | M1 ([master plan](M1-master-plan.md)) |
| Depends on | M0-A3 (minimal mixer + determinism harness) |
| Blocks | B4, B6 |
| Parallel with | B2, B5 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests + determinism test still green) |

## Context for a fresh agent

This task turns M0's quantised render skeleton into the real engine: the event-splitting
render loop, the `EventSource` trait and the pattern sequencer's timing spine (but not
its effect processing — that is B4).

Read `plans/product/01-technical-architecture.md` §§1–5. The three rules in §3.1 are the
ones that matter most and the ones easiest to get subtly wrong.

The design is validated by the original: `SB_IRQ_Handler` filled each DMA buffer in
slices bounded by the remaining tick gap, so a tick boundary always landed on the correct
output sample regardless of buffer size
(`plans/reference/original-s3mlib-analysis.md` §3).

## Deliverables

1. **`EventSource`**, exactly as in architecture §3:
   ```rust
   pub trait EventSource {
       fn next_event_frame(&self) -> Option<u64>;
       fn advance_to(&mut self, frame: u64);
       fn dispatch(&mut self, frame: u64, ctx: &mut EngineContext<'_>);
   }
   ```
   **Absolute frames, not deltas** — one engine-owned `u64` clock, so no source keeps its
   own elapsed-time bookkeeping to desync. `dispatch` takes a concrete `EngineContext`,
   not `&mut dyn EventSink`; only `EventSource` needs to be `dyn`.

2. **The render loop** (architecture §1.2), splitting for voice mixing within each
   128-frame quantum and calling the DSP hooks on whole quanta only.

3. **Rule 1 — never cache the next-event frame across a dispatch.** `Txx`, `Axx`, `SEx`,
   `SDx`, `Bxx` and `Cxx` all change *when the next tick is* from inside the tick just
   processed. Compute the next boundary at the **end** of processing the current tick.
   Structure the API so caching it is awkward rather than merely discouraged.

4. **Rule 2 — the zero-advance guard.** S3M speed 0, `A00`, pattern-break-to-self and
   SMF zero-delta events can all make a source return the same frame forever, spinning
   the render loop *inside the audio callback*, permanently.
   ```rust
   const MAX_ZERO_ADVANCE: u32 = 64;
   const MAX_EVENTS_PER_BLOCK: u32 = 4096;
   ```
   On breach: force-advance one frame, set a telemetry warning flag, keep rendering.
   **Never panic** — a panic in an AudioWorklet kills audio for the page permanently.

5. **Rule 3 — deterministic tie-breaking.** Sort by
   `(frame, source_slot, sequence_within_source)`, with `source_slot` a stable
   generational slot rather than a `Vec` position that shifts on removal. Two sources
   coinciding must dispatch identically offline and in real time.

6. **`RowClock`** (architecture §4):
   ```rust
   pub struct RowClock { pub speed: u8, pub pattern_delay: u8, pub tick_in_row: u16, pub repeat_index: u8 }
   ```
   with `is_first_tick_of_row()`, `is_first_tick_of_repeat()` and `total_ticks()`.
   `tick_in_row` is **absolute across pattern-delay repeats**, because that is what `Qxy`
   retrigger, `Ixy` tremor, `SDx` note delay and `SCx` note cut key off. Deliberately do
   **not** provide one shared `if tick == 0 { .. } else { .. }` helper across formats —
   that shared branch is where every cross-format bug would live.

7. **`PatternSequencer` timing spine** — order list → pattern → row → tick, order
   advance, pattern break, position jump, end-of-song and loop callback. It calls into a
   format-supplied effect processor through a concrete interface for now (the
   `Instrument` trait is extracted at M4, per architecture §10.1). B4 supplies the S3M
   processor.

8. **Channel binding and the command queue.** `Channel { foreground: Option<VoiceId>, .. }`
   (architecture §5.1 — background voices arrive with IT at M6 and cost the S3M path four
   bytes and one branch). The SPSC command ring, and the **garbage channel** that returns
   retired `Arc<Module>` values to the control thread so `free()` never runs on the audio
   thread (architecture §8).

9. **The control clock** (architecture §5.4): when a tracker sequencer is driving, its
   tick *is* the control clock; with no tracker, the engine synthesises one at a
   configured rate. One uniform rule — envelopes advance on control ticks.

## Research points

1. How to express "compute the next boundary at the end of the tick" in the type system
   rather than in a comment. A `TickOutcome` returned from the effect processor that the
   sequencer must consume before it can answer `next_event_frame()` is one option.
2. Whether `SourceMux` belongs here or at M4. It is needed for MIDI-over-module jamming;
   if it is cheap now, the tie-break rule is easier to test with two real sources.

## Verification

- The M0 block-size determinism test still passes, now with a sequencer running.
- A tick that changes tempo takes effect for the *next* tick, not the one after —
  assert the exact frame of tick N+1 after a `Txx` on tick N.
- The zero-advance guard: a source that always returns the current frame causes the loop
  to exit with the warning flag set, and audio continues. Assert the loop terminates and
  the output is silence-or-continued rather than a hang.
- Tie-breaking: two sources emitting at the same frame dispatch in the same order across
  100 runs and across two different host block sizes.
- `RowClock` with speed 6 and `pattern_delay` 2 reports `total_ticks() == 18`,
  `is_first_tick_of_row()` true only at `tick_in_row == 0`, and
  `is_first_tick_of_repeat()` true at 0, 6 and 12.
- No allocation inside `render()` (assert by inspection; the CI hook lands in M2-C7).

## Out of scope

Effect interpretation (B4). Telemetry publishing (B6). MIDI or SMF sources (M4).
Background voices and NNA (M6).
