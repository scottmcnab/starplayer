# M2-task-C3 — MOD: loader and ProTracker effect processor

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Status | Landed — owner listening check pending |
| Depends on | M1 |
| Blocks | C5, C7 |
| Parallel with | C4 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (conformance corpus) + **owner (listening check)** |

## Context for a fresh agent

`starplayer-mod` holds the MOD loader **and** its ProTracker effect processor.

**The single most important instruction: do not route MOD through S3M.** The original
converted MOD to a synthetic S3M image at load time
(`plans/reference/original-s3mlib-analysis.md` §5, `ConvertMOD` at `S3MLIB.ASM:1072`),
and that conversion is exactly why its MOD playback had known inaccuracies — it destroyed
the format's identity before the player ever saw it. This is a locked-in decision
(`plans/product/00-vision.md` decision 4).

What we **do** take from the original is its conversion tables, read as MOD *semantics*.
Those tables encode real, correct MOD facts; only the lowering was wrong.

The reference for MOD effect behaviour is ProTracker, not the original assembly. libxmp's
`test-dev/` suite (C2) is the practical oracle — **mine it before writing effect code**.

## Deliverables

1. **Loader.** Per analysis §5 (MOD):
   - ID at offset 1080: `M.K.` / `FLT4` → 4 channels, `6CHN` → 6, `8CHN` / `FLT8` → 8,
     else a decimal `"ddCH"` up to 32. **31 samples always** — 15-sample MODs are a
     separate, older layout; decide whether to support them and say so either way.
   - Title 20 bytes @0; 31 × 30-byte sample headers @20; song length @950; 128-byte order
     list @952; patterns @1084, each `4 × channels × 64` bytes.
   - Pattern count = (highest order value, ignoring 255) + 1.

2. **Sample headers**, with the two gates that matter:
   - length = BE u16 @22 × 2; volume @25 clamped to 64; loop start = BE u16 @26 × 2
     clamped to length; loop length = BE u16 @28 × 2.
   - **Loop enabled only if loop length > 4.** A universal MOD convention; getting it
     wrong makes many modules buzz.
   - Finetune from the signed nibble @24 via the **MOD** table:
     ```
     8363,8413,8463,8529,8581,8651,8723,8757,   ; finetune  0..+7
     7895,7941,7985,8046,8107,8169,8232,8280    ; finetune -8..-1
     ```
     Note this ordering is **different** from the monotonic `FineTuneTable` that S3M's
     `S2x` uses. Do not share the two tables.
   - **MOD samples are signed** — the original XORed 0x80 to make them unsigned for its
     mixer. Convert to `i16` directly.

3. **Panning.** The classic Amiga L-R-R-L interleave:
   `0,8,9,1,2,10,11,3,4,12,13,5,6,14,15,7` repeated. Expose a "hard pan versus reduced
   separation" option — hard Amiga panning is authentic but unpleasant on headphones, and
   every modern player offers a stereo-separation control. Default to the authentic value
   and let the host reduce it.

4. **The Amiga-limits derivation.** The original scans every note in the song and clears
   the limits flag the moment one falls outside octaves 3–5, so a standard-range MOD gets
   period clamping and an extended-range MOD does not. This is genuinely clever and
   correct — reproduce it.

5. **The ProTracker effect set**, natively. `0xy` arpeggio (with `000` meaning no effect),
   `1xx`/`2xx` porta up/down, `3xx` tone porta, `4xy` vibrato, `5xy`/`6xy` combined
   slides, `7xy` tremolo, `8xx` pan, `9xx` sample offset, `Axy` volume slide, `Bxx`
   position jump, `Cxx` set volume, `Dxx` pattern break (**BCD, unlike S3M's decimal —
   verify which and note it**), `Exy` extended, `Fxx` speed/tempo split at 32.

   `Exy` sub-effects: `E0` set filter (no-op), `E1`/`E2` fine porta, `E3` glissando,
   `E4` vibrato waveform, `E5` set finetune, `E6` pattern loop, `E7` tremolo waveform,
   `E8` pan, `E9` retrigger, `EA`/`EB` fine volume slide, `EC` note cut, `ED` note delay,
   `EE` pattern delay, `EF` invert loop.

   **ProTracker quirks that differ from S3M and must be handled here, not inherited:**
   sample offset memory and its behaviour past the sample end; the `9xx` "offset beyond
   end" rule; vibrato depth and the sine table's amplitude; ProTracker's one-shot
   two-word loop convention; `E9`/`ED` behaviour on tick 0; and the period table being
   the *finetuned* one rather than a computed value.

6. **A period-table-based pitch path.** MOD is a period-domain format with a fixed
   finetuned period table, not a computed one. Use the table.

## Research points

1. Whether to support 15-sample (Soundtracker-era) MODs. Cheap to add, and a fair number
   exist. Decide, implement or explicitly reject, and record it.
2. The full set of ProTracker quirks libxmp's `test-dev/` exercises. This is the research
   that determines whether this task takes three days or two weeks — do it first.
3. `Dxx` pattern break parameter encoding in ProTracker (BCD) versus S3M (decimal).
   Confirm from the corpus rather than from memory.
4. Whether `EF` invert loop is worth implementing. Rare, weird, and cheap; note the
   decision.

## Verification

- The libxmp `test-dev/` MOD cases pass, with any exclusion carrying a written reason.
- The loop-length > 4 gate: a module with a 2-word loop plays as one-shot.
- Finetune: a sample with finetune −1 plays at 8280 Hz reference, not 8413.
- Amiga limits: a standard-range MOD clamps periods; an extended-range MOD does not.
- Panning: channel 0 left, 1 right, 2 right, 3 left.
- **Owner check**: at least three well-known `.mod` files sound right.

## Coordinator review gate — 2026-09-01

The first implementation and an independent source audit are complete, but C3 is **not
ready to land**. The implementation remains unstaged and uncommitted until every item
below is corrected or replaced by a source-backed documented exclusion. These findings
are the repair specification for the next implementation pass.

### Loader correctness

1. **Scan all 128 order entries when locating sample data.** The current loader derives
   the pattern count from only the declared `song_length` prefix. That contradicts the
   deliverable above and ProTracker's all-128-entry pattern scan: a higher unused order
   still occupies pattern bytes in the file. Ignoring it moves the computed sample-data
   offset into pattern data and corrupts every decoded sample. Add a regression fixture
   whose active order prefix names pattern zero and whose inactive order tail names a
   higher pattern; assert both the pattern count and the first sample bytes. Continue to
   ignore `255`, and preserve the special stored-pair accounting for `FLT8`.

### Effect-state correctness

2. **Base arpeggio on the channel's current period, not a stale note number.** After
   period slides or tone portamento, `current_note` no longer identifies the sounding
   period. Match ProTracker's period-table search from the current period and selected
   finetune row, including its wrap/sentinel behaviour. Cover arpeggio immediately after
   both a plain period slide and a completed tone portamento.

3. **Latch whether the current row actually contains a note.** `EDx` on a row without a
   note must not retrigger the historical `current_note`. Use explicit row-local delayed
   trigger state rather than inferring eligibility from persistent channel state. Cover
   `ED0`, a positive `EDx`, no-note `EDx`, and row-delay repetitions.

4. **Audit `E9x` tick-zero and row-delay behaviour with the same row-local note state.**
   The current retrigger path can restart or reset state on repeated tick zero even when
   ProTracker would retain the existing sample pointer. Add focused oracle-derived tests
   for note and no-note rows, including `EEx` repetition.

5. **Correct `9xx` memory and no-note behaviour.** A `9xx` row must update and apply the
   retained sample offset according to ProTracker even when the row has no new note, and
   an `E9x` retrigger must not erase the retained offset state. Preserve the already
   implemented PT 1/2 double-offset pointer rule and the past-end one-word rule. Test each
   transition independently so one rule cannot mask another.

6. **Apply `Fxx` BPM at ProTracker's CIA update boundary.** The current tick outcome
   commits the new tempo for the interval beginning at the command's tick zero; PT uses
   the old duration for that interval and installs the new CIA value for the following
   tick. Implement this without weakening sample-exact event boundaries, then assert the
   absolute frame of the first two tick boundaries around a tempo change.

7. **Find tone-portamento targets in the selected finetune row.** Do not first identify a
   slot in the zero-finetune row and then index that slot in another row. ProTracker scans
   the selected finetune table for the packed period. Add a non-exact/intermediate-period
   case where the two algorithms choose different targets.

8. **Preserve vibrato and tremolo phases across delayed-note triggers.** The pinned PT
   source deliberately skips the ordinary-note phase reset while latching `EDx`, and
   `noteDelay` reaches `doRetrg` without resetting either phase when the delay expires.
   Cover selectors 0–7. This differs from libxmp's delayed-event path; for C3 the named
   primary ProTracker source governs and the oracle disagreement must stay documented.

### Host and conformance completion

9. **Finish the browser's user-facing MOD dispatch.** `apps/starplayer-web/www/index.html`
   still describes and accepts only S3M/ZIP, while archive messages in `app.js` say S3M
   even when MOD entries are offered. Update the file picker accept list, load/drop/URL
   labels, archive heading, empty-archive error, and archive-choice message to say MOD or
   generic module where appropriate. Keep the already-correct native backend dispatch
   and effect-name refresh ordering, and add a regression check for the visible strings.

10. **Integrate and execute the C2 MOD corpus gate before archiving C3.** The parallel C3
    branch predates the landed C2 harness, so this is completed after bringing C3 onto the
    current `m2` integration branch. Register the native `trace_mod` capture and use
    format-specific oracle projection: libxmp MOD notes require subtracting 24 rather
    than the S3M subtract-12 mapping, and MOD period Q12 requires division by 4096 rather
    than 1024. Make a deliberate, documented projection or tolerance decision for the
    integer libxmp sample-position oracle versus StarPlayer's fractional position. Run
    all 27 pinned MOD cases; every case must pass or have a specific written exclusion,
    with no remaining format gate.

### Required re-review

- Add a focused regression for every item above; a green existing suite is insufficient
  because the first implementation passed it while these defects remained.
- Re-run the task's package, workspace, clippy, `no_std`, bare-metal facade, WASM, trace,
  block-size determinism, conformance, and `git diff --check` commands.
- Obtain a second independent source audit after the repair pass. The coordinator reviews
  that report and the complete diff before staging any implementation path.

## Out of scope

MTM (C4). Any S3M conversion path. XM (M5).
