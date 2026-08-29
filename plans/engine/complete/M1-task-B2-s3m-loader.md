# M1-task-B2 — The S3M loader

| Field | Value |
|---|---|
| Milestone | M1 ([master plan](M1-master-plan.md)) |
| Depends on | B1 (module model) |
| Blocks | B4 |
| Parallel with | B3, B5 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests on real .s3m files) |

## Context for a fresh agent

`starplayer-s3m` holds both the loader **and** the effect processor, because file
semantics and effect semantics are inseparable (architecture §11). This task is the
loader half; B4 is the effect half.

The specification is `plans/reference/original-s3mlib-analysis.md` §5 (S3M) and §6
(period arithmetic). Read those sections before starting. The original's `ParseModule`
is at `STARPLAY/S3MLIB.ASM:2163` — inside a `comment %` block, but the text is complete.

## Deliverables

1. **Header parsing.** Validate `SCRM` at offset 0x2C. Read: title (0x00, 28 bytes),
   `Ordnum` (0x20), `Insnum` (0x22), `Patnum` (0x24), `generalflags` (0x26),
   `globalvol` (0x30), `initialspd` (0x31), `initialBPM` (0x32), `mastervol` (0x33, with
   bit 7 as the stereo flag).

   `generalflags` **bit 4 is the Amiga-limits flag** and must be carried through to the
   effect processor — it gates period clamping to `[113*4 .. 856*4]` = `[452 .. 3424]`.

   Channel count is the number of bytes in the 32-byte channel-settings array at 0x40
   whose value is `<= 0Fh` (i.e. enabled).

2. **Layout walk.** Order list at 0x60 (`Ordnum` bytes) → `Insnum` u16 parapointers →
   `Patnum` u16 parapointers → an optional 32-byte default-pan block, present **iff**
   byte 0x35 == 252 → data. All parapointers are × 16.

3. **Sample headers** (80 bytes each). Fields used: `[0]` = 1 for PCM; `[0x0E]` u16 data
   parapointer; `[0x10]` length; `[0x14]` loop start; `[0x18]` loop end; `[0x1C]`
   volume; `[0x1F]` flags (bit 0 = loop, bit 2 = 16-bit); `[0x20]` C2SPD; `[0x30]` name;
   `[0x4C]` `SCRS` magic.

   **Read the full 32-bit C2SPD field** — the original read only the low 16 bits, which
   is accuracy-policy deviation **D7**. Add a comment at the site citing it.

   Note that the original repurposed `[0x2C]` at runtime to hold the GUS DRAM address.
   That is a hardware artefact with no modern equivalent; ignore the field.

4. **Sample data.** **S3M samples are unsigned 8-bit** — convert to signed `i16` on load.
   16-bit samples (flags bit 2) are signed little-endian. Append guard frames per B1.

5. **Packed pattern unpacking.** Per pattern: a u16 packed length, then a byte stream.
   Within a row, a `0` byte terminates the row; otherwise bits 0–4 are the channel and
   bit 5 ⇒ two bytes follow (note, instrument), bit 6 ⇒ one byte (volume), bit 7 ⇒ two
   bytes (command, info). 64 rows per pattern.

   Unpack into the format's own native cell representation stored in `Module::blob`. Do
   **not** lower it into a shared cell type (architecture §6).

   Note byte conventions: **255 = no note, 254 = note cut**, otherwise `(octave<<4)|note`.

6. **Default panning.** Reproduce `ClearChannels` and `LoadPanSettings` exactly
   (analysis §5, "Panning at playback"):
   - default pan **7** (centre) for every channel;
   - if the module is stereo, read channel-setting byte `[0x40 + chan]`: `>= 128` → 7;
     `< 8` → **3** (left); else → **0Ch** (right);
   - then, if header byte 0x35 == 252, override from the 32-byte default-pan array,
     taking `byte & 0x0F` **only when bit 5 of that byte is set**.

7. **Robustness.** Every read is bounds-checked and returns `Err`, never panics. A
   truncated file, a parapointer past EOF, a pattern whose packed length overruns the
   file, a loop point past the sample end: all are `Err` or clamped, and which is chosen
   is documented per case. This task is fuzzed in M2-task-C7; write it as though the
   fuzzer already exists.

## Research points

1. Real-world S3M files violate the spec in known ways (Insnum/Patnum larger than the
   file supports, orders containing 254/255 markers, samples with `loop_end` beyond
   `length`). Survey a handful of real modules and decide clamp-vs-reject per case.
   Prefer clamping to rejecting where a real tracker would have played the file.
2. Whether ST3's own writer ever emits the 32-byte pan block without the 252 marker. If
   so, note it; do not guess a heuristic.

## Verification

- Load at least three real `.s3m` files of differing channel counts and assert:
  title, channel count, order count, instrument count and pattern count match what a
  known-good tool reports (`openmpt123 --probe`, or a hex dump cross-check).
- Unpack a pattern and compare a handful of cells against a hex dump of the packed data,
  by hand, in the test.
- A file truncated at every 64-byte boundary yields `Err` rather than a panic, at every
  truncation point.
- Sample sign conversion: a byte of `0x00` becomes the minimum, `0x80` becomes ~0, `0xFF`
  becomes near maximum.
- The default-panning rules produce the documented values for a stereo module with and
  without the 252 pan block.

## Out of scope

Effect interpretation (B4). Sequencing (B3). Any other format.
