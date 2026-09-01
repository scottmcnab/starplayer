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

Pattern storage uses the highest non-255 value across all 128 order bytes, plus one, as
ProTracker's loader does. Only the declared `song_length` prefix is played, but an unused
tail entry can still name a pattern physically present before the sample payload. Scanning
only live orders would place the sample-data offset inside that stored pattern. `FLT8`
applies the same all-entry scan after translating its stored half-pattern order numbers.

`FLT8` is the one tagged layout exception: Startrekker writes one logical eight-channel
pattern as all 64 rows of channels 0–3 followed by all 64 rows of channels 4–7, and its
order values name those stored four-channel halves (`0, 2, 4, ...`). The loader halves
those live order values and interleaves each pair into native row-major MOD cells. `8CHN`
remains the ordinary eight-cells-per-row layout.

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

The C2 corpus contains dialect and compatibility cases wider than the tagged ProTracker
scope of C3. They must remain visible exclusions when C2 registers `trace_mod`; they are
not evidence that MOD was lowered through another format:

- `CD61` Octalyser and `FA04` / `FA06` Digital Tracker files use signatures and pattern-
  loop dialects outside C3's accepted tag set, so the loader rejects them rather than
  guessing ProTracker semantics.
- Startrekker AM-synth tests require a separate sibling `.NT` / `.AS` parameter file and
  a synthesizer/envelope path. The slice-based facade intentionally receives only the
  `.mod` bytes; ordinary `FLT4` PCM modules remain supported, but the five `flt_am_*`
  cases cannot be reconstructed from those bytes alone.
- ProTracker's instrument-only and tone-portamento sample swap changes the DMA sample
  pointer when the currently playing sample reaches its end or loop boundary. The current
  mixer API has no queued region replacement at a voice boundary, so StarPlayer applies
  the new volume/finetune state immediately but keeps the old sample until an explicit
  note or retrigger. This affects `PortaSmpChange_PT`, `PortaSwapPT`, `PTInstrSwap`,
  `PTStoppedSwap`, `PTSwapEmpty`, and `PTSwapNoLoop`; accuracy policy D12 records it.
- The corpus's default-mode `PortaSmpChange.data` deliberately expects the non-ProTracker
  interpretation (keep the old sample indefinitely). It is an oracle for a different
  compatibility profile, while C3's reference is ProTracker 1/2.

OpenMPT's `NoteDelay-NextRow.mod` is documented-only in the pinned libxmp suite. PT lets
an `EDx` whose delay exceeds the current speed leak into the next row under narrow
conditions; StarPlayer currently clears a pending delayed note when the next row is
latched. This is recorded as D13 and needs an explicit cross-row deferred-trigger state
before that non-oracle case can be claimed.
