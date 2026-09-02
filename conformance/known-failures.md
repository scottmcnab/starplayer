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
