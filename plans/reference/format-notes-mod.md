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

Which entries count is a separate question. ProTracker's `mt_init` scans the table with a
**signed** byte compare (`cmp.b` / `bgt`), so any entry from `0x80` to `0xFF` is negative
there and can never raise the maximum; libxmp reaches the same result by breaking out of
the scan at the first byte above `0x7f` (its "dragnet.mod" fix). C3b moved the loader's
filter from `order != 255` to `order < 0x80`, so a file with `0x80` in its unused tail
loads with the same pattern count and the same sample offsets as one with `0x00` there.
A song length of zero is likewise not a structural error: libxmp loads such a module and
plays nothing, and so does the loader.

`FLT8` is the one tagged layout exception: Startrekker writes one logical eight-channel
pattern as all 64 rows of channels 0–3 followed by all 64 rows of channels 4–7, and its
order values name those stored four-channel halves (`0, 2, 4, ...`). The loader halves
those live order values and interleaves each pair into native row-major MOD cells. `8CHN`
remains the ordinary eight-cells-per-row layout.

The accepted signature set mirrors libxmp's `mod_magic[]` table
(`src/loaders/mod_load.c:76-95`) and each entry carries a `FormatDialect` as well as a
channel count:

| Tag | Channels | Dialect |
|---|---|---|
| `M.K.`, `M!K!` | 4 | `ProTracker` |
| `M&K!`, `N.T.` | 4 | `Noisetracker` |
| `LARD`, `NSMS` | 4 | `ProTracker3` |
| `FLT4` | 4 | `Startrekker` |
| `FLT8` | 8, paired halves | `Startrekker` |
| `CD61`, `CD81` | 6, 8 | `Octalyser` |
| `FA04`, `FA06`, `FA08` | 4, 6, 8 | `DigitalTracker` |
| `6CHN`, `8CHN`, `dCHN` (1–9), `nnCH` (1–32), `TDZ1`–`TDZ4` | as written | `FastTracker` |

`M!K!` is what ProTracker itself writes once a module exceeds 64 patterns — it is common,
and libxmp lists it first — and the single-digit `dCHN` form is used by several trackers;
C3b added both. C5 added the rest. The `ProTracker3` and `FastTracker` dialects play
exactly as ProTracker does; they are recorded because libxmp's own timing heuristics key
off them and M5 may need to. `Noisetracker` is the one that does not: C10 split `M&K!`
and `N.T.` out of `ProTracker3` because libxmp calls exactly those two
`TRACKER_NOISETRACKER`, a tracker with no CIA timer, so the dialect carries
`mod_timing: VBlank` — see *VBlank timing* below. `LARD` and `NSMS` are an *unknown*
tracker to libxmp and stayed where they were.

`CD61` and `FA04` / `FA06` were held back by C3 for a reason, and C5 accepted them **in
the same change** that gave them their pattern-loop dialect: accepting the tag alone would
have turned five honest exclusions into silent wrong playback. `.M.K` — Software Visions
DMF — is still outside the set: its first 2108 bytes are word-flipped to little endian, so
reading it needs a byte-order pass rather than a tag entry.

Digital Tracker keeps **four more header bytes** — always `00 40 00 00` — between the tag
and the pattern data, so its pattern data starts at 1088 and its sample data four bytes
later than a ProTracker file's would. libxmp skips them with a bare `hio_read32b`
(`mod_load.c:533-539`); the loader carries the same four bytes as
`ModLayout::extra_header_bytes`.

## Sample loop gate

A MOD sample loops when its repeat length is more than one word. ProTracker enables the
loop for `n_replen > 1` word, that is a loop length of **4 bytes or more**, and libxmp
uses the same `loop_size > 1` test. C3 used `loop_length > 4` bytes, which played a
two-word loop as a one-shot; that rule came from the DOS original's `ConvertSamps`
converter, not from the format. C3b moved the gate to `loop_length >= 4` and accuracy
policy §1 records the corrected attribution.

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
zero, matching PT's break handling. C5 moved the choice into the `QuirkSet` field
`mod_break_parameter`, because MultiTracker reads the same byte as hexadecimal; the
ProTracker dialect's value is unchanged. `F00` ending the song moved to
`mod_f00_stops_song` for the same reason, and the queued sample swap below to
`protracker_sample_swap_at_boundary`. The Paula clock the step derivation divides by is
`mod_paula_clock`, PAL under `canonical()` — see accuracy policy D14 for why NTSC is
offered but never selected by a loader.

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

### VBlank timing

The CIA boundary above only exists on a CIA-clocked replayer. ProTracker could also run its
interrupt off the 50 Hz vertical blank, and NoiseTracker and SoundTracker had nothing
*but* that; on the vertical blank there is no timer to program, so every non-zero `Fxx`
is ticks per row whatever its value. Nothing in the file header distinguishes the two for
an `M.K.` module.

`K-P-K.MOD` ("Klisje paa klisje", 4-channel `M.K.`, 93 orders, June 1993) is the case
that forced the issue. Its relevant cells, decoded from the file:

| Order | Pattern | Row | Channel | Cell |
|---|---|---|---|---|
| 31 | 32 | 63 | 4 | note 428, instrument 11, `F20` — with a four-note chord on the row |
| 32 | 20 | 0 | 1 | `F04` |
| 82 | 59 | 63 | — | `F30` |

No row anywhere in the file carries both an `Fxx` below `0x20` and one of `0x20` or more,
and there is one `Bxx`/`Dxx` in the whole song. Read as CIA the song is **29.1 minutes**;
read as VBlank it is **10.7**. Both high values sit on the last row of a section under a
chord with the next pattern restoring `F04` immediately: they are fermatas — a 32-tick
and a 48-tick hold — written on a tracker where `Fxx` was only ever ticks per row. The
file is third-party and is not in this repository.

The rules implemented, from libxmp `src/loaders/mod_load.c:816-950` and `src/scan.c:50`
and `:671-708` (M2-C10; the quirk field is `mod_timing`, resolved by
`starplayer::scan_song`):

* The tags `M&K!` and `N.T.` are NoiseTracker, which has no CIA mode: VBlank outright,
  from the header alone. `LARD` and `NSMS` are an *unknown* tracker to libxmp, not
  NoiseTracker, and get neither the shortcut nor the detection below.
* Detection from pattern evidence runs for the `M.K.` and `M!K!` tags only, and is turned
  off again by a sample header declaring 32768 words or more — no Amiga tracker could
  write one, so the file is an OpenMPT module and its timing is not in doubt.
* A row carrying both a low and a high `Fxx` is a CIA tracker: two different meanings for
  the same command byte on one row is only possible where the byte has two meanings.
* At least eight orders, every high `Fxx` confined to a pattern only the last two orders
  play, and the last such value not `0x7D` (125, the CIA default, which means the file was
  written or converted to play as CIA): VBlank, no comparison. This is the
  silence-at-the-end-of-a-module idiom.
* Otherwise, a high `Fxx` anywhere asks for a **length comparison**: scan the song as CIA,
  and if one pass is at least eight minutes — or the scan ran out its budget — scan it
  again as VBlank and keep the shorter, ties to CIA. A deliberately slow short song is
  therefore never sped up.

## Vibrato output and the Paula period floor

ProTracker's `mt_Vibrato3` writes `n_period ± delta` straight to Paula: the vibrato result
is never clamped back into the Amiga table range, so a `4FF` on a C-1 (period 856) keeps
its whole downward half even with Amiga limits on — it reaches period 885. The depth
itself is an unsigned magnitude — PT multiplies with `mulu` and shifts with `lsr`, then
adds or subtracts by the sign of `n_vibratopos`, so it never rounds a signed product
toward −∞; phase 132 with depth 15 on period 428 gives 426, not 425. C3 computed
`(waveform * depth) >> 7` on a signed product and then re-clamped `actual_period` with the
Amiga limits. C3b fixed both: the LFO scales the unsigned magnitude and negates, vibrato
writes its result unclamped, and the clamp stays where PT puts it, on the slides and on
tone portamento.

The range limit that does exist is in the hardware, not the replayer. Paula, as modelled by
pt2-clone's `paulaSetPeriod`, clamps any period below 113 to 113 and treats period 0 as
65536 — an audible but near-silent ~54 Hz crawl. C3b applies both at step derivation.
The zero rule is unconditional (no format wants a DC hold); the 113 floor follows the
loader's Amiga-limits flag, because it is the Amiga's limit and not the format's:
extended-range MODs and MultiTracker put their top octaves below 113 by design and libxmp
plays them unclamped. Under Amiga limits period 1 derives the same `Step` as period 113,
and period 0 always derives the step for 65536 rather than `Step::ZERO`. Accuracy policy
D16 records both.

## Pattern-loop mark, tremolo ramp, and processor reset

Three pieces of per-channel and per-processor state need naming because their current
handling is a fidelity gap rather than a decision:

- ProTracker's `n_pattpos` — the `E60` loop mark — is per channel and **persists across
  pattern changes**; only `E60` writes it. A pattern whose first `E6x` has no preceding
  `E60` therefore loops back to the previous pattern's mark. C3 cleared
  `pattern_loop_start` on every pattern change and so looped to row 0; C3b removed that
  reset. C5 moved the state itself out of `ModChannel` into
  `starplayer-engine`'s `PatternFlowState`, which is shared with S3M and parameterised by
  the module's dialect; ProTracker's `PatternFlow` has neither `global_target` nor
  `pattern_reset`, so the behaviour is unchanged.
- PT's `mt_Tremolo2` chooses the half of the ramp waveform by testing `n_vibratopos`, the
  vibrato phase, rather than `n_tremolopos`. This is a ProTracker bug; StarPlayer's default
  reads the ramp from the tremolo phase, and the PT behaviour is the `QuirkSet` field
  `protracker_tremolo_ramp_from_vibrato_phase`, added by C5 and off under `canonical()`.
  Accuracy policy D20 records it. libxmp does not model the bug, so no corpus oracle can
  distinguish the two and a unit test is the only thing that observes the field.
- The processor needed a **reset hook on seek**. `PatternSequencer::seek_order` /
  `seek_row` moved the cursor only, so a pending CIA tempo, the LFO random state, effect
  memories and loop counters all survived a seek and a seeked render differed from a fresh
  render of the same order — a stale `Fxx` could still fire. C3b added a required
  `fn reset(&mut self)` to `TrackerProcessor` and calls it from both seek entry points; it
  rewrites each channel in place and reseeds the LFO stream, so it allocates nothing and a
  host may drive a seek from the audio thread. (C3a's panning toggle builds a fresh
  processor and is unaffected.)

## The queued sample swap and ProTracker's null sample

An instrument number **without** a note, or with a note and a tone portamento, does not
restart the voice in ProTracker 1/2. The channel's volume, finetune and reported sample
number change at once; the sample itself is written into Paula's pointer and length
registers, and the DMA channel picks it up when it next reloads them — at the loop point,
or at the end of a one-shot. C3b models that with one `Option<SampleRegion>` per mixer
voice, adopted by the kernel where it already detects those two boundaries. The queue is
only created when a note has already sounded on the channel, which is libxmp's
`TEST_NOTE(NOTE_SET)` guard, and a note that is not a tone portamento clears it.

Three sub-rules come out of the hardware and are visible in the corpus:

- An instrument number naming a **slot with no sample data** is ProTracker's null sample.
  It leaves the channel's volume, finetune and reported number alone and queues a *stop*
  at the same boundary (`PTSwapEmpty.mod`, and `PTInstrSwap.mod` row 12, where reporting
  the empty slot's number would be the visible error).
- A **one-shot queued behind a one-shot** also stops rather than swapping: there are no
  loop registers to reload (`PTStoppedSwap.mod`, `PTSwapNoLoop.mod`; libxmp
  `src/mixer.c:726`).
- A channel whose sample has **already run out** has no boundary left to wait for, so the
  queued sample starts at once (libxmp's `libxmp_virt_queuepatch` fallback). Where it
  starts depends on whether the channel still owned that voice: a voice that merely ran
  out leaves the channel owned and the replacement starts at its loop point, while a
  channel that was cut is an ordinary fresh note from frame zero.

MTM is **not** included. libxmp gates the whole mechanism on `QUIRK_PROTRACK`, which its
MTM loader does not set, and the MultiTracker format document describes no such rule, so
an MTM instrument column applies volume and finetune and leaves the sounding sample
alone — no queue, and no restart either. That is what `MtmProcessor` does.

## `8xx` and `E8x` pan

C3b's task file expected the pan mapping to be unable to reach hard right or exact
centre. It can: `pan_byte` round-trips every one of the 256 `8xx` bytes through the trace
projection exactly, and `E8x` maps `value << 4` on the same 0..255 domain, which is
precisely what libxmp does for MOD (`fx_setpan` with `fxp <<= 4`). ProTracker itself has
no `E8x` pan at all, so libxmp is the reference here and the mapping is left alone; a unit
test pins the round trip. MultiTracker is the case that genuinely needed changing — see
`format-notes-mtm.md`.

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

- `CD61` Octalyser and `FA04` / `FA06` Digital Tracker files were outside C3's accepted
  tag set because they are tracker dialects with their own pattern-loop semantics. C5
  accepted the tags and implemented the dialects together, and four of the five cases now
  pass. All five waive `frame` and `position` under accuracy-policy D15: each fixture sets
  its tempo with an `Fxx` of 32 or more on row 0, and ProTracker's CIA latch defers that to
  the next tracker event, so StarPlayer's first interval is 882 frames where libxmp's is
  already at the new tempo. The fifth, `libxmp-mod-pattern-loop-dt`, produces a row
  sequence identical to the oracle's for all 488 ticks; what fails is the harness's
  residual-offset pairing across the fixture's alternating 255 and 63 BPM sections, which
  is recorded as `C2-MOD-001` in `conformance/known-failures.md`.
- Startrekker AM-synth tests require a separate sibling `.NT` / `.AS` parameter file and
  a synthesizer/envelope path. The slice-based facade intentionally receives only the
  `.mod` bytes; ordinary `FLT4` PCM modules remain supported, but the five `flt_am_*`
  cases cannot be reconstructed from those bytes alone.
- ProTracker's instrument-only and tone-portamento sample swap changes the DMA sample
  pointer when the currently playing sample reaches its end or loop boundary. C3b
  implements it: one fixed-size `Option<SampleRegion>` per voice, adopted by the render
  kernel at the forward-loop wrap or the one-shot end, so there is no allocation, no lock
  and no branch on the hot path when the slot is empty. `PTInstrSwap`, `PTSwapEmpty` and
  `PTStoppedSwap` now pass; `PortaSwapPT`, `PortaSmpChange_PT` and `PTSwapNoLoop` remain
  excluded for reasons that are no longer about the swap (D22 and D18 respectively).
  Accuracy policy D12 records the design and the two residual representation gaps.
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
  on finetuned samples. Since C2a those cases waive `position` alone and enforce every
  other field: `finetune`, `AmigaLimitsFinetune`, `PatternJump`, `InstrSwapRetrigger`,
  `ptoffset` and `PTInstrVolume` pass that way; `PTInstrSwap` then exposes the D12 gap.
- libxmp's C-4 rate is the rounded integer 8287, making its clock `8287 × 428 =
  3_546_836 Hz` against the exact PAL `3_546_895 Hz` — 17 ppm. That crosses the
  one-integer-sample bound only at the final compared tick of `PTInstrVolume`.

Four cases that a previous revision filed under the same heading were not engine
deviations at all; C2a (now landed, `plans/engine/complete/M2-task-C2a-conformance-harness-repairs.md`)
resolved each of them:

- `PortaTarget` — the harness compares floored positions with a linear `|a − b| <= 1`. The
  true position is 64.003 on a 64-frame loop, so StarPlayer reports 0 and libxmp 63.
  libxmp's own comparator special-cases loop start/end equivalence. The harness now
  compares circularly inside the loop span and the case passes.
- `PatternJump` — the trace aligner pairs records on `(row, frame)` with no time or order,
  so order 0 row 0's silent ticks are paired against libxmp's order 1 row 0 records. The
  actual positions 0..1090 match exactly. The harness now pairs by time; the case passes
  with only D14's `position` waived.
- `InstrSwapRetrigger` — the one-shot ends 0.07 tick into row 1 frame 8. libxmp omits a
  voice marked `NOTE_SAMPLE_END` after mixing the interval while C1 snapshots before it;
  that is the D18 mechanism, now an adapter projection rather than a per-case exclusion.
  The case passes with only D14's `position` waived.
- `ptoffset` — the apparent 15-tick voice-lifetime difference was an alignment artefact.
  Re-diagnosed by C2a: the offsets applied at ticks 0 to 7 match libxmp exactly, the first
  difference is D14 drift on a finetune-14 sample at tick 8, and with `position` waived the
  whole trace, active set included, matches. Not a `9xx` or loop-gate difference.

D15 records the two dumps whose first timestamp assumes libxmp's immediate `Fxx` tempo
interval rather than PT's CIA update boundary. Excluding those cases outright hides the
behaviour they exist to test, so C2a added a per-field waiver. Both cases waive `frame`
and `position` (the CIA latch shifts libxmp's whole timeline, so every voice is also
permanently offset inside its loop), and the rest of each trace is enforced. That exposed
two real ProTracker differences, both since resolved by C3b. `VibratoReset`'s one-unit
volume disagreement at tick 136 was the signed-shift LFO rounding, and the case now
passes with only D15's waiver. `DelayBreak` keeps the row-0 voice alive into row 1 frame 0
because ProTracker's `mt_RetrigNote` retriggers on tick zero of a row that carries `E9x`
and no note while libxmp handles retrigger only from tick one; that is accuracy policy
D21 and the case stays excluded for it. D16 records the out-of-range arpeggio volume
disagreement.

OpenMPT's `NoteDelay-NextRow.mod` is documented-only in the pinned libxmp suite. PT lets
an `EDx` whose delay exceeds the current speed leak into the next row under narrow
conditions; StarPlayer currently clears a pending delayed note when the next row is
latched. This is recorded as D13 and needs an explicit cross-row deferred-trigger state
before that non-oracle case can be claimed.
