# Conformance known failures

These records track executed corpus failures; they are not accepted accuracy-policy
deviations. Every record blocks the M2 exit until the engine conforms or the documented
behaviour is deliberately resolved in `plans/product/03-accuracy-policy.md`. The harness
still runs every excluded case, so a fix makes its exclusion fail as stale.

**Who owns what.** `plans/engine/M2-task-C9-s3m-conformance-repairs.md` owns every S3M
record below; each exclusion row names its record id and points at that task file.
No MOD or MTM case is recorded here. The ProTracker fidelity repairs and the mixer
voice-boundary sample swap landed as
`plans/engine/M2-task-C3b-protracker-fidelity-repairs.md`; the four MOD cases that task
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

Six S3M pattern-loop cases that once sat under C2-S3M-001 are **not** failures against
StarPlayer's reference: they encode the Imago Orpheus, ModPlug 1.16 and ST3.01 flow modes
libxmp selects from the `cwtv` field. They are dialect targets of
`plans/engine/M2-task-C5-quirks-and-tempo-models.md`. One of them,
`libxmp-s3m-pattern-loop-mpt-breakjump`, used to report `trace ended before libxmp row 2
frame 0`: that string was the harness exhausting its `oracle.len() + 256` tick budget
(libxmp emits no line for a silent tick, so the oracle length is only a lower bound), not
a state divergence. C2a derives the budget from the oracle's last timestamp and reports
exhaustion as a harness error; re-run, the case first diverges at tick 3 channel 0 on
`active`.

## C2-S3M-001 — ST3.21 pattern-loop control flow

Two cases, `libxmp-s3m-pattern-loop-st321` and `libxmp-s3m-pattern-loop-st321-breakjump`,
are real ST3.21 bugs inherited from M1. Since C2a they no longer reach the loop control
flow before failing: the first diverges at tick 0 channel 0 on `volume` (49 against 64)
and the second at tick 2 channel 0 on `pan` (136 against 119), so an initial-state
difference has to be resolved before the `SBx` loop counter and start-row semantics and
the same-row `Bxx`/`Cxx` interaction for ST3.21 can be assessed. ST3.21 is StarPlayer's
S3M reference; the other trackers' flow modes are C5 dialects, not failures.

## C2-S3M-002 — Amiga period limits

`AmigaLimits.s3m` agrees at the first frame but StarPlayer then advances the samples at
a different rate after clamping (first position mismatch on tick 1, 55 against 56.08
source frames; period is also one native unit lower). With position and period
disregarded, the active set then differs at tick 192. Resolve the S3M Amiga-limit period
and step calculation.

## C2-S3M-003 — high-frequency cutoff

`FreqLimits.s3m` first diverges at tick 31 channel 0 on `period`: 65 expected against 48
actual, StarPlayer sliding past ST3's high-frequency limit instead of clamping at it. The
earlier reading — a voice left active at row 5 frame 2 — was an alignment artefact; with
period and position disregarded the rest of the trace agrees. Resolve ST3's high-frequency
period clamp and the voice-stop boundary that follows from it.

## C2-S3M-004 — effect parameter memory

`ParamMemory.s3m` first diverges at tick 2 channel 0 on `period`, 1720 expected against
1723 actual: the effect-memory difference shows up as a slide rate, and channel volume
then differs at tick 13. Resolve the ST3 effect-memory sharing and continuation rules
exercised by this module.

## C2-S3M-005 — row delay and retrigger

`PatternDelaysRetrig.s3m` first diverges at tick 0 channel 3 on `note`, 64 expected
against 48 actual, so the sounding note identity is already wrong before any row-delay
behaviour is reached. Resolve that, then first-tick processing and tick numbering across
S6x row-delay repetitions, including retrigger interaction.

## C2-S3M-006 — lower period limit

`PeriodLimit.s3m` first diverges at tick 0 channel 0 on `note`, 101 expected against 66
actual, then advances the sample at a different rate. Resolve the ST3 lower output-period
limit, note identity at the limit, and zero-cut boundary.

## C2-S3M-007 — portamento sample change

`PortaSmpChange.s3m` changes StarPlayer's sounding instrument identity from 1 to 2 on
the Gxx row while the oracle keeps voice instrument 1. Resolve ST3's instrument/sample
latching semantics during tone portamento.

## C2-S3M-008 — sample-portamento continuation

`s3m_sample_porta.s3m` first diverges at tick 114 channel 0 on `active`: StarPlayer ends
the voice at row 19 frame 0 where the oracle keeps it sounding. Resolve tone-portamento
continuation after an instrument change and its resulting voice lifetime.
