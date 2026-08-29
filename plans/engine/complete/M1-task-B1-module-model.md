# M1-task-B1 — The Module data model

| Field | Value |
|---|---|
| Milestone | M1 ([master plan](M1-master-plan.md)) |
| Depends on | M0-A2 (core types) |
| Blocks | B2, B4 |
| Parallel with | B5 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests) |

## Context for a fresh agent

`starplayer-model` holds the format-neutral representation a loaded module takes in
memory. The layout decision is deliberate and load-bearing — read
`plans/product/01-technical-architecture.md` §6 before writing anything.

The short version: **offsets, not references.**

```rust
pub struct Module {
    blob: Box<[u8]>,               // decoded pattern data and everything non-sample
    pcm: Box<[i16]>,               // all samples decoded + delta-decoded, concatenated,
                                   //   each with guard frames appended
    samples: Box<[SampleIndex]>,
    patterns: Box<[PatternIndex]>,
    orders: Box<[u16]>,
    instruments: Box<[InstrumentDef]>,
    header: ModuleHeader,
}
```

This buys four things at once: `Send + Sync` so `Arc<Module>` hands to the audio thread
with no ceremony; hashability for golden tests; fuzzability (a loader either produces a
valid index set or an `Err`); and mmap/flash friendliness on embedded, where sample data
may be borrowed rather than owned.

The other half of the decision that is equally important: **format crates keep their
native pattern bytes in `blob`.** The shared model covers samples, envelopes, instrument
definitions and a *display-only* `PatternCell` view for UIs. It is deliberately **not** a
shared pattern-cell model. The original lowered MOD and MTM into S3M before the player
saw them, and that is exactly why its MOD playback was inaccurate
(`plans/reference/original-s3mlib-analysis.md` §1).

## Deliverables

1. **`Module`** as above, with accessors that return `Option` or `Result` rather than
   panicking on a bad index — the mixer runs in the audio path and must never panic.

2. **`SampleIndex`** — `{ pcm_offset: u32, len: u32, loop_start: u32, loop_end: u32,
   loop_mode: LoopMode, default_volume: U0F16, reference_rate_hz: u32, .. }`.
   `LoopMode` covers `None`, `Forward` and `PingPong` (XM/IT need ping-pong; declare it
   now, implement the mixer side when a format uses it). `reference_rate_hz` is S3M's
   C2SPD / MOD's finetune-derived rate — **the full 32 bits**, per accuracy policy D7.

3. **`PatternIndex`** — `{ blob_offset: u32, rows: u16, channels: u8 }`.

4. **`InstrumentDef`** — enough for S3M now (a sample reference plus default volume),
   with the shape anticipating XM/IT: envelopes, a note→sample map, NNA settings. Declare
   the fields; leave them unused and documented as "M5/M6" rather than inventing
   behaviour.

5. **Guard frames.** Every sample's PCM gets N appended frames — loop-wrapped for looping
   samples, zeroed otherwise — so the interpolator reads past the loop point with no
   branch in the inner loop. Use the count agreed in M0-A3; if that task chose a
   placeholder, settle it here and update A3's comment.

6. **`PatternCell`** — a display-only view: `{ note, instrument, volume, effect,
   effect_param }` with the effect rendered both as a raw code and as a human-readable
   name. The name table comes from `plans/reference/original-star-ui.md` §2.3 — the
   original spelled effects out in English per channel, and both UIs want that.

7. **`ModuleReader`** — the minimal loader IO trait:
   ```rust
   pub trait ModuleReader { fn len(&self) -> usize; fn read_at(&self, offset: usize, buf: &mut [u8]) -> Result<(), Error>; }
   ```
   with a zero-copy `&[u8]` implementation as the fast path. Loading is **synchronous**
   over an already-obtained byte source; acquiring the bytes is async and lives in the
   platform crates, so the core never needs an async runtime (architecture §10).

8. **A builder** that loaders use, which enforces the invariants: every offset in range,
   every loop point inside its sample, guard frames present. The builder is where
   fuzz-resistance is concentrated — a loader should find it hard to produce an invalid
   `Module`.

## Research points

1. Whether samples should be normalised to `i16` at load time (simple, uniform mixer) or
   kept in their source width with the mixer generic over it (less memory on embedded).
   Recommendation: normalise to `i16` now and revisit at M8 when the embedded memory
   budget is real; record the decision either way.
2. Whether `blob` and `pcm` should be one allocation or two. Two is simpler; one may
   matter for mmap. Note the trade-off, pick the simple one.

## Verification

- A hand-built `Module` round-trips through the builder with every invariant satisfied.
- The builder **rejects**: an out-of-range `pcm_offset`, a `loop_end` past the sample
  end, a `loop_start` after `loop_end`, and a `PatternIndex` whose `blob_offset` plus
  implied size overruns `blob`.
- Guard frames for a forward-looping sample contain the loop-start data; for a non-
  looping sample they are zero.
- `Module` is `Send + Sync` (assert with a compile-time `fn assert_send_sync<T: Send + Sync>()`).
- Hashing the same module twice gives the same hash — the precondition for golden tests
  in M2.

## Out of scope

Any format parsing (that is B2). Envelope *behaviour*. Sample decompression (IT, M6).
