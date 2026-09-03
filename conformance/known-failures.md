# Conformance known failures

These records track executed corpus failures; they are not accepted accuracy-policy
deviations. Every record blocks the M2 exit until the engine conforms or the documented
behaviour is deliberately resolved in `plans/product/03-accuracy-policy.md`. The harness
still runs every excluded case, so a fix makes its exclusion fail as stale.

**Who owns what.** C9 (`plans/engine/complete/M2-task-C9-s3m-conformance-repairs.md`,
landed) left the one S3M record below; its exclusion row points here. The MOD record C5
opened, `C2-MOD-001`, was a harness limitation and is resolved below.
No MTM case is recorded here. The ProTracker fidelity repairs and the mixer
voice-boundary sample swap landed as
`plans/engine/complete/M2-task-C3b-protracker-fidelity-repairs.md` (landed); the four MOD cases that task
file listed as targets but did not fix turned out to be oracle-representation
differences, and each now cites `plans/product/03-accuracy-policy.md` — D21 for
ProTracker's tick-zero `E9x` retrigger (`openmpt-mod-delay-break`), D22 for its
tick-zero instrument latch under `EDx` (`openmpt-mod-portamento-swap-pt`), and D18 for a
queued stop that falls due inside a tick interval, which makes libxmp omit the channel
for that whole tick (`openmpt-mod-swap-no-loop`,
`openmpt-mod-portamento-sample-change-pt`).
The harness repairs — trace alignment, loop-aware position comparison, the D18 adapter
projection, the per-field waiver and the tick budget — landed as
`plans/engine/complete/M2-task-C2a-conformance-harness-repairs.md`, so every first divergence
quoted below is one the repaired harness observed, not an alignment position.

The ten tracker-dialect cases — the Imago Orpheus, ModPlug 1.16 and ST3.01 flow modes
libxmp selects from the `cwtv` field, and the Octalyser and Digital Tracker MOD tags —
were never failures against StarPlayer's reference, and C5 has landed their
`FormatDialect` and `QuirkSet` fields. All ten now pass (the last needed the harness
re-anchor recorded under `C2-MOD-001` below). The MOD failures are not recorded
here either: the ProTracker fidelity repairs and the voice-boundary sample swap are
`plans/engine/M2-task-C3b-protracker-fidelity-repairs.md`. The harness repairs — trace
alignment, loop-aware position comparison, the D18 adapter projection, the per-field
waiver and the tick budget — landed as
`plans/engine/complete/M2-task-C2a-conformance-harness-repairs.md`, so every first
divergence quoted below is one the repaired harness observed, not an alignment position.

## C2-S3M-009 — the Rxy tremolo phase and its first tick

`ParamMemory.s3m` is the last S3M record. Its shared parameter memory, the effect it
exists to test, now agrees for every family through row 58 of 63 with `period` and
`position` waived under accuracy-policy D39 (libxmp truncates the S3M vibrato in its
Amiga period domain, so `H82` moves its period by 8 quarter-units where ST3 moves it by
11). The remaining first divergence is **tick 353 channel 0 on `volume`, 64 expected
against 59 actual**, on the `Rxy` rows, and it has two parts:

1. ST3's `M_FX_R` reads and advances `_VibCount`, the phase it shares with `Hxy` and
   `Uxy`, so a preceding `Kxy` vibrato leaves the tremolo mid-waveform. libxmp and
   OpenMPT both give the tremolo its own position, which starts at zero here.
2. libxmp applies the tremolo delta on the first tick of a row as well, using the phase
   un-advanced, where the original runs `Rxy` only as a minor effect.

Both would also change `Hxy`'s tick-zero behaviour if adopted, which no corpus case
currently pins, so neither was changed on speculation. Resolving this needs a decision on
whether ST3 really shares `_VibCount` between vibrato and tremolo — the tremolo depth
scale itself was resolved and is accuracy-policy D35.

## C2-MOD-001 — resolved: the pairer re-anchors a waived timeline

`pattern_loop_dt.mod` alternates 255 BPM at speed 3 with 63 BPM at speed 1, and accuracy
policy D15 (ProTracker's CIA latch) puts StarPlayer's timeline one interval behind libxmp's
at every tempo command, flipping the residual frame offset by about 1300 frames. The
time-based pairer re-derived that offset only after a successful pairing, so three records
per flip fell back to the wrong tick and the case reported `row` 1 against 0 at tick 121
although its 488-tick `(row, tick_in_row)` sequence matched the oracle exactly.

Resolved in the harness after C5 landed: when a `frame`-waived timeline fails to pair a
record, `pair_by_time` re-anchors on the record's `(row, tick_in_row)`, searching forward
from the last pairing, and re-derives the offset there. The case now passes waiving
`frame,position` under D15 like its four Octalyser and Digital Tracker siblings.

## Resolved by M2-C9

`C2-S3M-001` through `C2-S3M-008` are closed. `libxmp-s3m-pattern-loop-st321`,
`libxmp-s3m-pattern-loop-st321-breakjump`, `openmpt-s3m-amiga-limits`,
`openmpt-s3m-frequency-limits`, `openmpt-s3m-pattern-delays-retrigger`,
`openmpt-s3m-period-limit`, `openmpt-s3m-portamento-sample-change` and
`libxmp-s3m-sample-portamento` all pass, seven of them with a per-field waiver naming an
accuracy-policy entry for a libxmp representation difference (D19, D36, D37, D38, D39)
rather than an engine gap. The engine repairs behind them are accuracy-policy D24–D35.
`libxmp-s3m-pattern-loop-mpt-breakjump`, a C5 dialect case, began passing as a side
effect of the ST3.21 flow repairs and no longer carries an exclusion.

## Resolved by M2-C5

All ten tracker-dialect cases are closed (`libxmp-mod-pattern-loop-dt` after the harness re-anchor above).
`libxmp-s3m-pattern-loop-imf-breakjump` and `libxmp-s3m-pattern-loop-st301-breakjump`
pass with **no** waiver at all and their exclusion rows are gone;
`libxmp-s3m-pattern-loop-imf`, `-mpt`, `-st301` and the four Octalyser / Digital Tracker
MOD cases pass under per-field waivers naming accuracy-policy D15, D19, D36, D37, D38 and
the new D40. Every one of those is a representation or timeline difference against the
oracle, not an engine gap.

# Impulse Tracker (task G3)

Task G3 landed IT playback: the effect processor, instrument articulation, New Note
Actions, duplicate checks and voice stealing, wired into 121 pinned corpus cases. Thirteen
pass with every field enforced and twenty-six more pass with `position` waived under
accuracy-policy entry **D67**. The eighty-two records below are the remainder, grouped by
the first field that diverges once `position` is set aside, so each group is one piece of
work rather than one case. They belong to **G6** (`plans/engine/M6-task-G6-*`), the
conformance-repair task M6 exits through; none of them is an accepted deviation, and every
excluded case is still executed on every run, so a fix makes its exclusion fail as stale.

The categories were measured at the same commit as the exclusion rows. A case appears in
exactly one group — the one its *first* divergence names — so fixing a group will move
cases into another group before it moves them into the pass column.

## G3-IT-001

**The volume chain's last bit — 24 cases.** The first difference is one or two steps of
the trace's 0..64 volume axis, usually while an envelope is attacking or a fadeout is
running. StarPlayer implements OpenMPT's chain (`Vol · VEV · NFC · CV · SV · IV · GV`
folded as `muldiv(vol14 · GV256, CV · insVol, 1 << 20)`); libxmp folds the sample and
instrument global volumes in *after* the envelope and the fadeout
(`QUIRK_INSVOL`, `src/player.c:1102`) and divides by different powers of two on the way.
The quantisation to six bits then turns a sub-percent difference into a whole step. G6
should settle which order Impulse Tracker itself uses — `it2play`'s `Music_*` volume code
is the authority — and either match it or record the difference as a deviation with a
one-step tolerance.

## G3-IT-002

**The filter envelope and the `Zxx` cutoff — 16 cases.** The first difference is two or
three steps of the 0..255 cutoff axis while a filter envelope is running. Both players
compute the same product — OpenMPT's `cutoff · (envModifier + 256) / 256` and libxmp's
`filter.cutoff · filter.envelope >> 8` are algebraically identical — so the difference is
the *envelope interpolation*: libxmp interpolates node values pre-multiplied by four
(`src/loaders/it_load.c:664`) and StarPlayer interpolates the model's −32..32 values scaled
by eight. libxmp's own `ZxxSecrets` test comment records that its filter-envelope handling
is wrong ("libxmp right shifting the cutoff by the filter envelope range instead of
deriving coefficients off of the product"), so G6 must decide this against OpenMPT rather
than against the oracle, and may have to record it as a deviation.

## G3-IT-003

**The sounding voice set — 12 cases.** The first difference is a channel or a virtual
channel that one player has sounding and the other does not: a New Note Action that should
not have allocated a background voice, a voice that should have been freed when its fadeout
reached zero, or a duplicate check that should have killed one. The virtual-channel
numbering the adapter reproduces (research point 1) is only as good as the set of voices it
is numbering, so a difference here also shifts every later background row.

## G3-IT-004

**Note and sample selection — 10 cases.** The first difference is the note or the sample a
channel is playing: the empty-note-map-slot rules (`kITEmptyNoteMapSlot`,
`kITEmptyNoteMapSlotIgnoreCell`), the lone-instrument-number rules
(`kITInstrWithoutNote`, `kITMultiSampleInstrumentNumber`), and the portamento sample-swap
rules (`kITPortamentoInstrument`, `kITPortamentoSwapResetsPos`) interact, and G3 implements
them from OpenMPT's description rather than from a per-case reading.

## G3-IT-005

**Pitch — 8 cases.** The first difference is the `period` column by more than one whole
period, so it is a real pitch error rather than D64's axis resolution: a slide that ran on
the wrong tick, a portamento target that was not consumed, or an arpeggio phase that is out
of step.

## G3-IT-006

**Row flow and the tick budget — 8 cases.** The first difference is `row`, `tick-in-row` or
`frame`: a pattern loop, break or jump that resolved differently, or a row whose tick
budget differs because `SEx`, `S6x` or a `Txx` tempo slide was counted differently. The
four `pattern_loop_it*` fixtures that exercise D65's dialects are **not** in this group —
three of them pass with the D67 waiver — so this is the `Cxx`/`Bxx`/`SBx` interaction
rather than the dialect selection.

## G3-IT-007

**Panning — 1 case.** The first difference is the pan column outside surround, so it is the
pan envelope's asymmetric scaling, pitch/pan separation, or the pan swing.

## G3-IT-008

**Sample position beyond the D67 waiver — 3 cases.** Two cases diverge on `position` by
more than the accumulated-rounding difference D67 covers, so something other than the
frequency's last bit moved the voice. The third, `openmpt-it-bidi-loops`, is the fixture
that exists to test it: Impulse Tracker's software mixer plays a ping-pong loop **one
sample short** of the file's, which OpenMPT emulates and G3 did not implement.
