# Conformance known failures

These records track executed corpus failures; they are not accepted accuracy-policy
deviations. Every record blocks the M2 exit until the engine conforms or the documented
behaviour is deliberately resolved in `plans/product/03-accuracy-policy.md`. The harness
still runs every excluded case, so a fix makes its exclusion fail as stale.

**Who owns what.** `plans/engine/M2-task-C9-s3m-conformance-repairs.md` owns every S3M
record below; each exclusion row names its record id and points at that task file.
The MOD failures are not recorded here: the ProTracker fidelity repairs and the
voice-boundary sample swap are `plans/engine/M2-task-C3b-protracker-fidelity-repairs.md`,
and the harness repairs — trace alignment, loop-aware position comparison, the D18
adapter projection, the per-field waiver and the tick budget — are
`plans/engine/M2-task-C2a-conformance-harness-repairs.md`.

Six S3M pattern-loop cases that once sat under C2-S3M-001 are **not** failures against
StarPlayer's reference: they encode the Imago Orpheus, ModPlug 1.16 and ST3.01 flow modes
libxmp selects from the `cwtv` field. They are dialect targets of
`plans/engine/M2-task-C5-quirks-and-tempo-models.md`. One of them,
`libxmp-s3m-pattern-loop-mpt-breakjump`, additionally reports `trace ended before libxmp
row 2 frame 0`: that string is the harness exhausting its `oracle.len() + 256` tick budget
(libxmp emits no line for a silent tick, so the oracle length is only a lower bound), not
a state divergence. C2a derives the budget from the oracle's last timestamp and reports
exhaustion as a harness error; the case must be re-run afterwards.

## C2-S3M-001 — ST3.21 pattern-loop control flow

Two cases, `libxmp-s3m-pattern-loop-st321` and `libxmp-s3m-pattern-loop-st321-breakjump`,
are real ST3.21 bugs inherited from M1: StarPlayer restarts row 0 at tick 30 where ST3.21
continues the loop, and again at tick 24 where ST3.21 follows the break/jump. Resolve the
`SBx` loop counter and start-row semantics and the same-row `Bxx`/`Cxx` interaction for
ST3.21, which is StarPlayer's S3M reference; the other trackers' flow modes are C5
dialects, not failures.

## C2-S3M-002 — Amiga period limits

`AmigaLimits.s3m` agrees at the first frame but StarPlayer then advances the samples at
a different rate after clamping (first position mismatch on tick 1; period is also one
native unit lower). Resolve the S3M Amiga-limit period and step calculation.

## C2-S3M-003 — high-frequency cutoff

`FreqLimits.s3m` leaves a StarPlayer voice active at row 5 frame 2 where the oracle has
no mapped active voice until row 16. Resolve ST3's high-frequency voice-stop boundary.

## C2-S3M-004 — effect parameter memory

`ParamMemory.s3m` diverges in its active row/frame sequence by row 16. Resolve the ST3
effect-memory sharing and continuation rules exercised by this module.

## C2-S3M-005 — row delay and retrigger

`PatternDelaysRetrig.s3m` reports StarPlayer row 0 frame 12 when the oracle has advanced
to row 1 frame 0. Resolve first-tick processing and tick numbering across S6x row-delay
repetitions, including retrigger interaction.

## C2-S3M-006 — lower period limit

`PeriodLimit.s3m` starts at the oracle period but StarPlayer reports note F#5 where the
libxmp mixer voice reports F-8, then advances the sample at a different rate. Resolve
the ST3 lower output-period limit, note identity at the limit, and zero-cut boundary.

## C2-S3M-007 — portamento sample change

`PortaSmpChange.s3m` changes StarPlayer's sounding instrument identity from 1 to 2 on
the Gxx row while the oracle keeps voice instrument 1. Resolve ST3's instrument/sample
latching semantics during tone portamento.

## C2-S3M-008 — sample-portamento continuation

`s3m_sample_porta.s3m` leaves StarPlayer on an active row 3 frame 3 when the oracle's
next active frame is row 16 frame 0. Resolve tone-portamento continuation after an
instrument change and its resulting voice lifetime.
