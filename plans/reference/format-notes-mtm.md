# MTM implementation notes

Decisions and primary-source findings for the native `starplayer-mtm` implementation.
MTM remains a distinct format from both MOD and S3M even where its effect vocabulary is
shared.

## Sources and corpus

- StarPlayer 2.25s `ConvertMTM`, `ConvertMTMPats`, `ConvertNoteMTM` and
  `ConvertMTMSamps` in `STARPLAY-2.25s/S3MLIB.ASM` are the repository's primary replay
  reference.
- MultiTracker's format document, pinned in libxmp commit
  `6ec0ba21b1b28f91e22b68a51d59207c6bbf6139` as
  `docs/formats/Mtm-form.txt`, defines the header, shared-track representation, unsigned
  PCM and the ProTracker effect vocabulary.
- The same pinned libxmp revision provides `src/loaders/mtm_load.c`, loader reference
  `test_loader_mtm.c`, fuzzer case `test_fuzzer_mtm_channels_bound.c`, and three executed
  mixer-state cases: `test_player_mtm_tempo.c` (two modules) and
  `test_effect_pattern_jump_mtm_break.c`.

The pinned corpus contains five MTM binaries: three with frame-state oracles, one real
loader reference (`fall1.mtm`), and one malformed channel-bound seed. None of their 124
sample headers sets the 16-bit attribute. The original also marks 16-bit MTM samples as
not supported by its player. C4 therefore rejects them explicitly with
`Unsupported("16-bit MTM samples")`; it does not silently decode half-width PCM.

## Native layout and samples

The loader bounds-checks the fixed 1.0 layout, retains each three-byte MTM cell, and
resolves pattern track references when building the immutable module. Reference zero and
out-of-range references select the empty track, matching the primary layout and libxmp's
safe recovery. Two patterns referencing the same stored track consequently expand to
identical native rows without any MOD or S3M serialization.

All declared sample spans are preflighted cumulatively against the remaining source
before any sample bytes are read or allocated, so a tiny file with a maximal 32-bit
length reports truncation instead of attempting a multi-gigabyte allocation.

MTM PCM is unsigned: byte `0x80` becomes signed zero internally. Sample volume is
clamped to 64, the finetune nibble indexes MOD's signed-nibble `C2SPD_Table`, and a loop
is enabled only when the declared end-minus-start is greater than four bytes and remains
non-empty after bounds clamping. Header pan values are native 0..15 positions and are
mapped directly to StarPlayer's bipolar pan domain; no synthetic S3M valid bit is
invented.

Amiga limits are deliberately disabled. `ConvertMTM` achieved this accidentally by
passing dummy period 1712 through `ConvertValues`; the native loader records the intended
result directly because MTM is a PC tracker format.

`fall1.mtm` is checked during the conformance run against libxmp's `format_mtm.data`:
title `- One Must Fall! 1 -`, 5 channels, 51 stored non-empty tracks (libxmp reports 52
including track zero), and 12 patterns.

## Effect reuse boundary

`MtmProcessor` owns native three-byte row decoding and supplies a linear MTM note to an
explicit semantic effect boundary in `starplayer-mod`. The shared core owns only the
ProTracker-compatible commands 0..F and their tick state. No serialized MOD period or
S3M command enters the MTM path.

The MultiTracker profile differs from ProTracker at these named points:

- `Dxx` is a hexadecimal row number, not BCD.
- `Fxx` takes effect immediately. Native MultiTracker use resets BPM to 125 on a speed
  command and speed to 6 on a tempo command. When low and high `Fxx` coexist on one row,
  the loader selects the widespread Dual Module Player interpretation and does not reset
  the counterpart, following libxmp's source-backed compatibility detection.
- `9xx` selects one absolute `xx * 256` offset for a note. It does not reproduce
  ProTracker 1/2's retained-pointer double advance or one-word fallback. A past-end
  position is retained with the normal sample region: the mixer ends a one-shot and
  wraps a looping sample into its loop, matching libxmp and the original MTM path.
- MTM always runs without the MOD Amiga-period clamp and does not apply MOD's delayed
  pattern-break postprocessing.

All remaining commands, including extended `E8x` pan, intentionally use the common
ProTracker implementation specified by the format document. The public reuse types are
an execution vocabulary only; native pattern identity remains in `MtmCell`,
`MtmPatternData`, and `MtmProcessor`.

That boundary also makes two already documented ProTracker deviations apply to MTM:
`EFx` records its speed but cannot mutate immutable shared PCM (accuracy D10), and random
LFO waveform 3 uses StarPlayer's deterministic fixed-seed stream rather than an
unspecified process seed (accuracy D11).

## Conformance projection and exclusions

The C2 adapter compares MTM on its native axes: libxmp mixer notes subtract 12 to reach
StarPlayer's note axis, Q12 period divides by 4096 into an Amiga period, four-bit pan is
expanded to the common 0..255 display axis, and StarPlayer's Q32.32 position is floored
because libxmp exposes integer `pos0`. The inherited one-integer-source-frame tolerance
is unchanged.

`pattern_jump_mtm_break.mtm` passes exactly. The two tempo modules exercise the intended
control-flow profile and agree on row, tick, speed, BPM, note, instrument, period,
volume, and pan, but retain two deliberately visible oracle-boundary differences:

- `TEMPO.MTM` has a one-shot sample which is active at StarPlayer's row 12/tick 4 event
  snapshot and ends while the following interval is rendered. libxmp's
  `compare_mixer_data` calls `xmp_play_frame` first and then omits a voice marked
  `NOTE_SAMPLE_END`, so its next record is the row 16 retrigger. C1's event-boundary
  trace contract is not changed to fabricate that post-mix view (accuracy D18).
- `TEMPO2.MTM` reaches a two-source-frame position difference by tick 53 at high BPM.
  libxmp's `libxmp_mixer_get_ticksize` casts every `freq * 2.5 / bpm` interval to `int`;
  StarPlayer's required `ExactFixedPoint` clock carries the rational remainder. The
  position mismatch is kept visible rather than weakening the global bound or adopting
  buffer-duration drift (accuracy D19).

Both cases remain executed exclusions, so any later adapter or trace improvement makes
the exclusion fail as stale. That is the expected outcome for `TEMPO.MTM`:
`plans/engine/M2-task-C2a-conformance-harness-repairs.md` turns D18 into an adapter
projection — when StarPlayer has an active one-shot voice, libxmp has no record, and the
voice reaches its region end inside the tick interval, the pair is treated as matching —
after which this case should pass and its exclusion be dropped. `TEMPO2.MTM` (D19) stays.
