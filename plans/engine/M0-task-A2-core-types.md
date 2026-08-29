# M0-task-A2 — Core types: fixed point, clock, TempoModel

| Field | Value |
|---|---|
| Milestone | M0 ([master plan](M0-master-plan.md)) |
| Depends on | A1 (workspace) |
| Blocks | A3, and all of M1 |
| Parallel with | A4 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests) |

## Context for a fresh agent

`starplayer-core` is the bottom of the dependency graph: fixed-point arithmetic, time,
pitch, and the event and voice-parameter types. It has **no IO, no side effects and no
dependencies on other StarPlayer crates**. Everything above it is built on these types,
so getting the units right here is worth care.

Read `plans/product/01-technical-architecture.md` §§1–2 for the type design and its
rationale, and `plans/reference/original-s3mlib-analysis.md` §6 for the period and
frequency arithmetic these types have to express exactly.

The key unit decision, and the reason it is not the obvious one: **pitch is carried as
`Step` — a Q32.32 sample-position increment per output frame — not as a frequency.** The
mixer wants an increment; the division from frequency belongs in format code, which
knows both the sample's reference rate and the output rate, and runs it at tick rate.
The original does exactly this (`SB_ProcessTracks`, a 32.32 step split across
`_Mix_HighSpeed` / `_Mix_LowSpeed`). A Q16.16 Hz type would also cap at 65535 Hz for no
reason.

## Deliverables

1. **Fixed-point types** in `starplayer-core::fixed`. Either wrap the `fixed` crate or
   define thin newtypes over it — decide and document which, but do not hand-roll the
   arithmetic:
   - `Step(u64)` — Q32.32 sample-position increment per output frame.
   - `U0F16` — unsigned 0.0..=1.0, for velocity, volume, controller values.
   - `I1F15` — signed −1.0..=1.0, for panning and pitch bend.
   - `Q32_32` for the tick accumulator.
   Each needs saturating arithmetic and `no_std`-safe conversions. No transcendental
   functions anywhere (architecture §7.3).

2. **`Frame`** — a `u64` newtype for absolute output-frame position, with the
   documented invariant that it is monotonic and engine-owned.

3. **`Note { semitone: u8, cents: i16 }`** with conversions to and from MIDI note
   numbers, plus a `Period` helper type for the tracker formats. Note that the period
   arithmetic itself lives in the format crates; core only provides the representation
   and the shared `Period_Table` / waveform tables.

4. **Tables** — the S3M `Period_Table` (1712, 1616, 1524, 1440, 1356, 1280, 1208, 1140,
   1076, 1016, 960, 907) and the four vibrato/tremolo waveform tables. Transcribe from
   `plans/reference/original-s3mlib-analysis.md` §4, applying accuracy-policy deviations
   **D1** (pulse table gets a full 64 entries: 32 × 0 then 32 × 255) and **D5** (regular
   ramp, keeping the ±255 amplitude). Add a comment at each deviation citing
   `plans/product/03-accuracy-policy.md`.

5. **`TempoModel`** trait with all three implementations:
   ```rust
   pub trait TempoModel {
       /// Frames per tick, Q32.32.
       fn frames_per_tick(&self, sample_rate_hz: u32, tempo_bpm: u16, speed: u8) -> u64;
   }
   pub struct ExactFixedPoint;   // default: rate * 2.5 / bpm, exact in Q32.32
   pub struct St3Truncating;     // (rate * 10 / bpm) >> 2 — the original's truncation
   pub struct ItModern;          // stub for now; M6 fills it in
   ```
   This is one of only two traits committed before a second implementation exists
   (architecture §10.1) — it has three from the start.

6. **`FrameClock`** — owns the current `Frame`, the Q32.32 tick accumulator and the
   `TempoModel`. `next_tick_frame()` must be computed at the *end* of a tick, never
   cached across one (architecture §3.1 rule 1); encode that in the API shape so it is
   hard to misuse.

7. **Event and parameter types** — `TimedEvent`, `Target`, `Event`, `VoiceParams`,
   `DirtyBits` (a `bitflags` set mirroring the original's `_CHN_New*`), and `Command`,
   exactly as in architecture §2.1. These are declarations plus trivial constructors;
   the behaviour lives in M1.

## Research points

1. Whether the `fixed` crate's `no_std` story and API are a good fit, or whether thin
   hand-written newtypes over `u64`/`i32` with explicit saturating ops are simpler to
   audit. Prefer whichever makes the mixer inner loop obviously correct.
2. Confirm the exact ST3 constants against the assembly before hard-coding:
   `8363 * 16 = 133808` and `14317056 = 8363 * 1712`.

## Verification

Unit tests, `assert` calls on one line per the working agreements:

- `ExactFixedPoint::frames_per_tick(44100, 130, 6)` accumulates to exactly
  `44100 * 2.5 / 130` over many ticks with no drift; assert the accumulated frame count
  after 10,000 ticks matches the closed-form value.
- `St3Truncating::frames_per_tick(44100, 130, 6)` yields 848, reproducing the original's
  double truncation.
- Round-trip `Note` ↔ MIDI note number over the full range.
- `Period_Table[0] == 1712`, and `1712 * 133808 >> 4 / 8363 == 1712` (octave 4 identity).
- Every waveform table has exactly 64 entries — the regression test for deviation D1.
- Fixed-point saturating arithmetic does not wrap at the extremes.

## Out of scope

The voice pool, the render loop, any sequencing, any mixing. Those are A3 and M1.
