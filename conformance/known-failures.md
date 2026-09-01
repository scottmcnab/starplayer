# Conformance known failures

These records track executed corpus failures; they are not accepted accuracy-policy
deviations. Every record blocks the M2 exit until the engine conforms or the documented
behaviour is deliberately resolved in `plans/product/03-accuracy-policy.md`. The
exclusions file links each case to one of these stable anchors, and the harness still
runs every excluded case so a fix makes its exclusion fail as stale.

## C2-S3M-001 — pattern-loop compatibility

The eight libxmp pattern-loop cases take a different active control-flow path in
StarPlayer. First divergences range from tick 4 to tick 30: StarPlayer revisits row 0 or
row 2 while the applicable IMF, OpenMPT, ST3.01, or ST3.21 oracle expects another loop
or its break/jump destination. Resolve the `SBx` loop counter/start-row semantics and
the same-row `Bxx`/`Cxx` interaction for each compatibility mode.

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
