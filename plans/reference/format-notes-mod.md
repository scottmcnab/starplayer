# MOD implementation notes

Decisions and primary-source findings for the native `starplayer-mod` implementation.
This file records format-specific choices which do not belong in the format-neutral
architecture.

## Sources researched

- 8bitbubsy's ProTracker 2 clone, official repository
  `https://github.com/8bitbubsy/pt2-clone`, commit
  `411eb2c126230d74ac56a3907c28c9d6d11a78ba`: `src/pt2_replayer.c` and
  `src/pt2_tables.c` provide a readable C port of the PT 2.3D replayer and its exact
  16 × 37 period, vibrato, and funk-speed tables.
- libxmp, official repository `https://github.com/libxmp/libxmp`, commit
  `6ec0ba21b1b28f91e22b68a51d59207c6bbf6139`: relevant focused cases include
  `test_effect_{1_slide_up,2_slide_down,4_vibrato,9_offset,e9_retrig,ef_invert_loop}.c`,
  `test_player_period_mod_range.c`, and the OpenMPT MOD fixtures for finetune, Amiga
  limits, vibrato reset, portamento target/sample change, delay/break, and jump order.

## 31 versus 15 samples

Only tagged, 31-sample MODs are accepted in M2 C3. A tagless 15-sample file is not merely
the same header with fewer entries: Soundtracker-family dialects differ in pattern/data
offsets, finetune availability, loop-start units, and effect/tempo semantics. Guessing
from plausibility fields also produces false positives on arbitrary input. `probe` thus
requires a supported signature at offset 1080 and `load` returns `BadMagic` for tagless
files. A future Soundtracker loader can add this as an explicit dialect rather than
silently applying ProTracker semantics.

Pattern storage uses the highest value across all 128 order bytes, plus one. Scanning
every entry rather than only the live ones is ProTracker's rule and is load-bearing: only
the declared `song_length` prefix is played, but an unused tail entry can still name a
pattern physically present before the sample payload, and scanning only live orders would
place the sample-data offset inside that stored pattern. `FLT8` applies the same all-entry
scan after translating its stored half-pattern order numbers.

Which entries count is a separate question, and C3 gets it wrong. ProTracker's `mt_init`
scans the table with a **signed** byte compare (`cmp.b` / `bgt`), so any entry from `0x80`
to `0xFF` is negative there and can never raise the maximum; libxmp reaches the same result
by breaking out of the scan at the first byte above `0x7f` (its "dragnet.mod" fix). As of
C3 the loader excludes only `255`, so a file with `0x80` in its unused tail is either
rejected as truncated or has every sample offset shifted.
`plans/engine/M2-task-C3b-protracker-fidelity-repairs.md` changes the filter to
`order < 0x80`; the paragraph above then describes both states, but the attribution to
ProTracker only becomes true with that fix.

`FLT8` is the one tagged layout exception: Startrekker writes one logical eight-channel
pattern as all 64 rows of channels 0–3 followed by all 64 rows of channels 4–7, and its
order values name those stored four-channel halves (`0, 2, 4, ...`). The loader halves
those live order values and interleaves each pair into native row-major MOD cells. `8CHN`
remains the ordinary eight-cells-per-row layout.

The accepted signature set, as of C3, is exactly `M.K.` and `FLT4` (4 channels), `6CHN`,
`8CHN`, `FLT8` (paired halves), and two-digit `nnCHN` for 1–32 channels. Two gaps remain:
`M!K!` is written by ProTracker itself once a module exceeds 64 patterns — it is common,
and libxmp lists it first — and single-digit `dCHN` tags are used by several trackers.
Both are rejected today; `plans/engine/M2-task-C3b-protracker-fidelity-repairs.md` adds
`M!K!` as a 4-channel tag and widens the `CHN` form to one or two digits. `CD61`
(Octalyser) and `FA04` / `FA06` (Digital Tracker) are deliberately outside this set for a
different reason — they are tracker dialects with their own pattern-loop semantics, owned
by `plans/engine/M2-task-C5-quirks-and-tempo-models.md`, not signatures to be waved
through with ProTracker semantics.

## Sample loop gate

A MOD sample loops when its repeat length is more than one word. ProTracker enables the
loop for `n_replen > 1` word, that is a loop length of **4 bytes or more**, and libxmp
uses the same `loop_size > 1` test. As of C3 the loader uses `loop_length > 4` bytes, so a
two-word loop that is a real, audible loop in ProTracker is played here as a one-shot. The
`> 4` rule comes from the DOS original's `ConvertSamps` converter, not from the format;
accuracy policy §1 records the corrected attribution and
`plans/engine/M2-task-C3b-protracker-fidelity-repairs.md` moves the gate to `>= 4`.

## Period and finetune lookup

Pattern periods are decoded as PT's `setPeriod` does: scan the zero-finetune row to find
the note slot, then read the same slot from the instrument's selected finetune row. For
example, raw period 428 remains C-4 on StarPlayer's C-0 note axis when instrument
finetune +7 is latched, and sounds at period 407. Pitch, arpeggio, portamento targets and
glissando all stay in this table domain; no S3M C2SPD period lowering is used.

Arpeggio searches the selected finetune row from the channel's live period on each
non-base phase, so a preceding period slide or completed tone portamento changes its base
even though the last packed note does not. The lookup treats PT's period table as
physically flat: every finetune row has a zero sentinel, overflow can enter the next row,
and the final `-1` row uses PT's pinned 15-word overflow padding. Tone-portamento target
lookup also scans the selected row directly and retains PT's one-slot adjustment for
negative finetunes.

`Dxx` pattern break is BCD (`10 * high + low`). Values decoding past row 63 go to row
zero, matching PT's break handling.

ProTracker 1/2's `9xx` pointer bug is retained: the current note starts at `xx * 256`,
then the retained channel pointer advances by the same amount again. A following note
without a new instrument therefore starts at twice the original offset. A new instrument
resets that retained pointer, and `900` recalls the last nonzero parameter. Each offset is
tested against the channel's remaining sample length. On failure PT forces a one-word
length but does not change the last successfully advanced pointer.

`9xx` advances the retained pointer even on a row without a note. `E9x` restarts from
that pointer rather than resetting it. Both `E9x` and `EDx` use the packed-note presence
latched for the current row (and retained across `EEx` repeats), never the historical
channel note. PT also preserves both vibrato and tremolo phases for every waveform across
an `EDx`: `setPeriod` skips the ordinary-note reset, then `noteDelay` calls `doRetrg`,
which has no phase writes. libxmp's delayed-event path instead resets retrigger-enabled
selectors 0–3; C3 follows its named primary PT source and treats that oracle difference
as an explicit conformance exclusion.

`Fxx` values from 32 through 255 use PT's CIA boundary: tick zero is followed by one
interval at the old BPM, and the new BPM is committed by the next tracker event for the
interval after it. Speed values below 32 remain immediate row-clock changes.

When `EEx` shares a row with `Dxx`, PT spends the delayed repeats and then skips the
break target row. The processor advances the target once more, including the row-63 case
which continues at row zero of the following order. A later-channel `Bxx` still cancels
an earlier `Dxx`; a later `Dxx` combines its BCD row with the selected order.

## Vibrato output and the Paula period floor

ProTracker's `mt_Vibrato3` writes `n_period ± delta` straight to Paula: the vibrato result
is never clamped back into the Amiga table range, so a `4FF` on a C-1 (period 856) keeps
its whole downward half even with Amiga limits on. The depth itself is an unsigned
magnitude — PT multiplies with `mulu` and shifts with `lsr`, then adds or subtracts by the
sign of `n_vibratopos`, so it never rounds a signed product toward −∞. As of C3 the
processor computes `(waveform * depth) >> 7` on a signed product and then re-clamps
`actual_period` with the Amiga limits; both are corrected by
`plans/engine/M2-task-C3b-protracker-fidelity-repairs.md`, which also keeps the clamp where
it belongs, on slides and tone portamento.

The range limit that does exist is in the hardware, not the replayer. Paula, as modelled by
pt2-clone's `paulaSetPeriod`, clamps any period below 113 to 113 and treats period 0 as
65536 — an audible but near-silent ~54 Hz crawl. As of C3 the step derivation floors the
period at 1 when Amiga limits are off and maps period 0 to a zero step, which holds the
voice's last sample value as DC; C3b applies the 113 floor and the 65536 rule at step
derivation instead. Accuracy policy D16 records both.

## Pattern-loop mark, tremolo ramp, and processor reset

Three pieces of per-channel and per-processor state need naming because their current
handling is a fidelity gap rather than a decision:

- ProTracker's `n_pattpos` — the `E60` loop mark — is per channel and **persists across
  pattern changes**; only `E60` writes it. A pattern whose first `E6x` has no preceding
  `E60` therefore loops back to the previous pattern's mark. As of C3 the processor clears
  `pattern_loop_start` on every pattern change and so loops to row 0; C3b removes the
  reset.
- PT's `mt_Tremolo2` chooses the half of the ramp waveform by testing `n_vibratopos`, the
  vibrato phase, rather than `n_tremolopos`. This is a ProTracker bug; StarPlayer's default
  reads the ramp from the tremolo phase, and the PT behaviour becomes a `QuirkSet` field in
  `plans/engine/M2-task-C5-quirks-and-tempo-models.md`. Accuracy policy D20 records it.
  libxmp does not model the bug, so no corpus oracle can distinguish the two.
- The processor has **no reset hook on seek**. `PatternSequencer::seek_order` / `seek_row`
  move the cursor only, so a pending CIA tempo, the LFO random state, effect memories and
  loop counters all survive a seek and a seeked render differs from a fresh render of the
  same order — a stale `Fxx` can still fire. C3b adds `fn reset(&mut self)` to the
  processor trait and calls it from both seeks. (C3a's panning toggle builds a fresh
  processor and is unaffected.)

## Waveform 3 and determinism

libxmp's MOD LFO defines selector 3 as random. Its default player seed comes from wall
clock time, which cannot satisfy StarPlayer's buffer-size and cross-target deterministic
output contract. StarPlayer uses a fixed-seed, integer-only xorshift32 stream in the MOD
processor. This makes separate replays identical while retaining one new signed random
value per waveform evaluation. The exact random sequence is consequently a documented
choice, not a claim to match an unspecified hardware or process seed.

## EFx invert loop

`EFx` is recognised, named, and its speed is retained in channel state, but it is an
explicit audio no-op. PT advances through a sample loop and destructively replaces a byte
with `-1-byte`. StarPlayer stores sample PCM in an immutable `Arc<Module>` shared by the
processor, engine and mixer. Mutating it from a tick would introduce shared mutable PCM;
an exact overlay would require storage proportional to an arbitrary loop and cannot be
allocated in `render()`. Locks, RT allocation, and cross-instance sample mutation are all
less acceptable than this rare known gap. Accuracy policy D10 records the deviation.

## Known conformance exclusions

The C2 adapter maps libxmp's MOD-specific axes without passing through S3M. The common
comparison note axis is ProTracker's displayed octave: libxmp mixer notes subtract 24,
while C1's C-4-at-reference-rate identity subtracts 12. Q12 periods divide by 4096 into
native Amiga periods. libxmp exposes mixer `pos0` as an integer source position before
mixing, while StarPlayer retains a Q32.32 source position. The MOD comparison floors
StarPlayer's fraction and applies the pinned libxmp comparator's own one-integer-sample
bound. A two-frame playback-step error remains observable.

The C2 corpus contains dialect and compatibility cases wider than the tagged ProTracker
scope of C3. They must remain visible exclusions when C2 registers `trace_mod`; they are
not evidence that MOD was lowered through another format:

- `CD61` Octalyser and `FA04` / `FA06` Digital Tracker files use signatures and pattern-
  loop dialects outside C3's accepted tag set, so the loader rejects them rather than
  guessing ProTracker semantics. These are dialects StarPlayer intends to support, not
  refusals: the five cases are tracked by
  `plans/engine/M2-task-C5-quirks-and-tempo-models.md`, which detects the dialect from the
  signature and maps it to `QuirkSet` fields.
- Startrekker AM-synth tests require a separate sibling `.NT` / `.AS` parameter file and
  a synthesizer/envelope path. The slice-based facade intentionally receives only the
  `.mod` bytes; ordinary `FLT4` PCM modules remain supported, but the five `flt_am_*`
  cases cannot be reconstructed from those bytes alone.
- ProTracker's instrument-only and tone-portamento sample swap changes the DMA sample
  pointer when the currently playing sample reaches its end or loop boundary. The current
  mixer API has no queued region replacement at a voice boundary, so StarPlayer applies
  the new volume/finetune state immediately but keeps the old sample until an explicit
  note or retrigger. This affects `PortaSmpChange_PT`, `PortaSwapPT`, `PTInstrSwap`,
  `PTStoppedSwap`, `PTSwapEmpty`, and `PTSwapNoLoop`; accuracy policy D12 records it as a
  tracked gap, not an accepted deviation. One fixed-size pending region per voice, applied
  by the voice kernel at the wrap or one-shot end, is RT-safe;
  `plans/engine/M2-task-C3b-protracker-fidelity-repairs.md` owns that work.
- The corpus's default-mode `PortaSmpChange.data` deliberately expects the non-ProTracker
  interpretation (keep the old sample indefinitely). It is an oracle for a different
  compatibility profile, while C3's reference is ProTracker 1/2.

The pinned libxmp mixer dump is also not a ProTracker period oracle — but the reason is
not the Amiga clock, and an earlier revision of this file was wrong to say it was. libxmp's
MOD loader keeps `C4_PAL_RATE` (8287) for files identified as ProTracker or OpenMPT and
selects NTSC only for the ScreamTracker3, FastTracker, TakeTracker and ModsGrave ids or for
more than four channels (`src/loaders/mod_load.c:1113-1123`, `src/load_helpers.c:302`).
Every fixture excluded here is a 4-channel `M.K.` file, and the dumps advance at PAL speed:
`ptoffset.data` moves 218 source frames per tick at period 325.3, where PAL predicts 218.1
and NTSC 220.1. Both sides are PAL.

What differs is the representation of pitch, and accuracy policy D14 now records it:

- libxmp derives a finetuned period continuously, `428 · 2^(-(note + finetune/128)/12)`,
  producing non-integers such as 162.65 and 453.45 where ProTracker reads one of the 16
  integer finetune tables (163, 453). Positions drift by roughly one source frame per tick
  on finetuned samples, which excludes `finetune`, `AmigaLimitsFinetune` and `PTInstrSwap`.
- libxmp's C-4 rate is the rounded integer 8287, making its clock `8287 × 428 =
  3_546_836 Hz` against the exact PAL `3_546_895 Hz` — 17 ppm. That crosses the
  one-integer-sample bound only at the final compared tick of `PTInstrVolume`.

Four cases that a previous revision filed under the same heading are not engine deviations
at all, and must not be read as accepted ones:

- `PortaTarget` — the harness compares floored positions with a linear `|a − b| <= 1`. The
  true position is 64.003 on a 64-frame loop, so StarPlayer reports 0 and libxmp 63.
  libxmp's own comparator special-cases loop start/end equivalence. Harness repair, tracked
  by `plans/engine/M2-task-C2a-conformance-harness-repairs.md`.
- `PatternJump` — the trace aligner pairs records on `(row, frame)` with no time or order,
  so order 0 row 0's silent ticks are paired against libxmp's order 1 row 0 records. The
  actual positions 0..1090 match exactly. Same task file.
- `InstrSwapRetrigger` — the one-shot ends 0.07 tick into row 1 frame 8. libxmp omits a
  voice marked `NOTE_SAMPLE_END` after mixing the interval while C1 snapshots before it;
  that is the D18 mechanism, and C2a turns it into an adapter projection rather than a
  per-case exclusion.
- `ptoffset` — StarPlayer's channel 0 voice stays active about 15 ticks longer than
  libxmp's after the `9xx`-without-note sequence. Both sides are PAL, so the clock explains
  nothing; the cause is unexplained (candidates are the `9xx` retained-pointer semantics
  and the loop gate above) and is to be re-diagnosed against ProTracker by C3b/C2a, not
  accepted as a deviation.

D15 records the two dumps whose first timestamp assumes libxmp's immediate `Fxx` tempo
interval rather than PT's CIA update boundary. Excluding those cases outright hides the
behaviour they exist to test, so C2a adds a per-field waiver: only the tick-0 `frame` field
is waived, and the rest of each trace is enforced. D16 records the out-of-range arpeggio
volume disagreement.

OpenMPT's `NoteDelay-NextRow.mod` is documented-only in the pinned libxmp suite. PT lets
an `EDx` whose delay exceeds the current speed leak into the next row under narrow
conditions; StarPlayer currently clears a pending delayed note when the next row is
latched. This is recorded as D13 and needs an explicit cross-row deferred-trigger state
before that non-oracle case can be claimed.
