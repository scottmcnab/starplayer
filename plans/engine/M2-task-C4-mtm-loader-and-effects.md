# M2-task-C4 — MTM: loader and effect processor

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Depends on | M1 |
| Blocks | C7 |
| Parallel with | C3 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests + any corpus coverage) |

## Context for a fresh agent

MultiTracker (`.mtm`) is the smallest of the three original formats and shares MOD's
effect vocabulary, so this task is mostly a loader plus a thin effect layer over the MOD
processor from C3. As with MOD, **do not route it through S3M**
(`plans/product/00-vision.md` decision 4).

Specification: `plans/reference/original-s3mlib-analysis.md` §5 (MTM). The original's
`ConvertMTM` is at `S3MLIB.ASM:1222`.

MTM's distinguishing feature is its **track** indirection: patterns are arrays of track
numbers, and tracks are the actual 64-row × 3-byte note data, shared between patterns.
This is a real compression scheme and the loader must handle it — including track 0
meaning "empty".

## Deliverables

1. **Header.** `MTM` + version (4 bytes), title @4 (20), `numtracks` u16 @24,
   `lastpattern` u8 @26, `lastorder` u8 @27, `commentlen` u16 @28, `numsamples` u8 @30,
   attribute @31, `beatspertrack` @32, `numchannels` @33 (1–32), 32 pan bytes @34.

2. **Layout.** `numsamples` × 37-byte sample headers @66 → 128-byte order list →
   `numtracks` × 192-byte tracks (64 rows × 3 bytes) → `(lastpattern+1)` × 32 u16 track
   numbers (**0 = empty**) → comment → sample data.
   Track base = `66 + 37*numsamples + 128`; pattern table = tracks + `192*numtracks`.

3. **Sample headers.** Name 22; length u32 @22; loop start u32 @26; loop end u32 @30 with
   **loop enabled only if end − start > 4**; finetune nibble @34 through the same MOD
   `C2SPD_Table` as C3 (share the table with `starplayer-mod` via `starplayer-core`, or
   duplicate it with a comment — do not accidentally use S3M's monotonic table);
   volume @35 clamped to 64; attribute @36 bit 0 = 16-bit.

   **MTM samples are already unsigned** — unlike MOD. This is the kind of detail that
   produces a module that plays but sounds wrong, so test it explicitly.

4. **Note packing.** `b0[7:2]` = pitch (0 = none); `b0[1:0]:b1[7:4]` = 6-bit instrument;
   `b1[3:0]` = effect; `b2` = data. Pitch → note: `octave = pitch/12 + 2`,
   `note = pitch%12`.

5. **Panning** from the 32 header pan bytes. Note that the original set the S3M "entry
   valid" bit (bit 5) on each when writing its synthetic header — an artefact of the
   conversion with no meaning here. Use the pan values directly.

6. **Amiga limits: off.** The original's MTM path called its period converter with a dummy
   period that always landed in octave 2, which had the side effect of *always* clearing
   the Amiga-limits flag for any MTM containing a note. Set it off **directly and
   deliberately** rather than inheriting the same result by accident, and comment why.

7. **The effect processor.** MTM uses MOD's effect vocabulary. Reuse the C3 processor
   where the semantics genuinely match; where MultiTracker differs, implement the
   difference rather than papering over it. Document the reuse boundary explicitly — a
   future reader needs to know which behaviours are shared by design and which by
   coincidence.

## Research points

1. Whether MultiTracker's effect semantics differ from ProTracker's anywhere that
   matters. Documentation is thin; the corpus and real modules are the practical source.
2. Whether any MTM in the wild actually uses the 16-bit sample attribute bit. The
   original noted it as "not supported in player yet".
3. How much MTM coverage libxmp's `test-dev/` has. If it is thin, hand-written tests
   carry more weight here than for MOD.

## Verification

- Load a real `.mtm` and assert title, channel count, track count and pattern count
  against a known-good tool.
- Track indirection: two patterns sharing a track produce identical rows; track 0 yields
  an empty row.
- Sample sign: an unsigned `0x80` byte becomes ~0 amplitude, not full scale. (The
  MOD-versus-MTM sign difference is the most likely bug in this task.)
- The loop gate: `end - start == 4` is one-shot; `== 5` loops.
- Amiga limits are off, and the code says so deliberately.

## Out of scope

MOD (C3). Any 16-bit MTM sample support unless research point 2 finds real files.
