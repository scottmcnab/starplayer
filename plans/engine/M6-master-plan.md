# M6 — IT support

| Field | Value |
|---|---|
| Goal | Impulse Tracker modules play correctly |
| Estimate | 2u |
| Depends on | M5 |
| Blocks | — |

## Why this is the hard one

IT is the largest format in scope and the one whose accuracy is hardest to reach. Two
areas are worth a week each on their own:

**New Note Actions.** A channel can own several sounding voices at once — a foreground
voice receiving effect updates plus detached background voices running only their own
envelopes and fadeout. The architecture has been built for this since M0 (architecture
§5.1): background voices live in the pool tagged with `VoiceTag`, not in the channel, so
MOD and S3M pay four bytes per voice and one branch and are otherwise untouched. This
milestone is where that design is actually exercised.

**Voice stealing is audible.** On dense modules, *which* note gets cut when the virtual
channel limit is reached is part of the output. Matching libopenmpt means matching its
heuristic — prefer background and fading voices, prefer the lowest amplitude, avoid
stealing a foreground voice. This is a policy trait with real time budgeted against it,
not something to guess. It is architecture open question **Q3**.

## Deliverables

1. **`starplayer-it`** loader: header, orders, instruments, samples, patterns, and both
   compression schemes (IT2.14 8-bit and 16-bit).
2. **NNA, DCT and DCA** in the voice allocator, with `VoiceTag` matching.
3. **The voice-stealing policy trait**, with an implementation matched to libopenmpt.
   Settle Q3 and record the answer in the architecture document.
4. **The resonant filter** — a per-voice low-pass with cutoff and resonance, its own
   envelope, and the `Zxx` effect. Table-driven coefficients: no transcendentals in the
   RT path (architecture §7.3), which matters more here than anywhere else.
5. **Envelopes** — volume, panning and pitch/filter — with carry, and the instrument-mode
   note map.
6. **The IT effect set**, including the extended `Sxx` family, `Zxx`, and the old-versus-
   new effect behaviour flag.
7. **`TempoModel::ItModern`** filled in, including tempo slides.
8. **Conformance**: the OpenMPT `test_*.it` cases, which are the most demanding in the
   whole corpus.

## As landed so far

**G1** delivered deliverable 1. **G3** delivered 2, 3, 5, 6, 7 and the wiring half of 8;
**G2** delivered 4, concurrently. What G3 changed against the plan above:

* Deliverable 3 is **not a trait**. Design goal 8 keeps a trait uncommitted until its
  second real implementation, and XM's allocator is the same heuristic with a different
  New Note Action set rather than a different one, so `ItProcessor::choose_victim` is a
  concrete policy in `starplayer-it`. Q3 is settled and recorded in architecture §5.2 and
  the §12 table.
* Deliverable 7 turned out to be a **format** decision rather than a tuning knob:
  Impulse Tracker truncates its tick length to a whole output frame and does not carry the
  remainder, so `TempoModelId::ItModern` truncates and the four
  `FormatDialect::ImpulseTracker*` variants select it. Accuracy policy §2 records both
  measurements; it is the owner's call to confirm.
* Deliverable 8 grew: the harness's own completeness audit means wiring `it` brings in
  **121** cases, not the 59 `openmpt/it` fixtures the task file named — the 62 `data/*.it`
  `compare_mixer_data` pairs come with them. 39 pass (13 with every field enforced, 26
  waiving `position` under accuracy-policy D67) and 82 are recorded as `G3-IT-001` …
  `G3-IT-008` in `conformance/known-failures.md` for **G6**, grouped by the first field
  that diverges so each group is one piece of work.
* Two new `QuirkSet` fields, each named by a corpus case: `it_pattern_loop`
  (`ItLoopDialect`, four `Cwt/v`-gated `SBx` profiles — accuracy policy D65) and the IT
  dialects' `tempo_model`.

## Exit criteria

The IT conformance corpus passes with exclusions justified; dense real-world `.it` files
sound right, including their voice-stealing behaviour.

## Out of scope

Anything not in the IT format. Resist the temptation to generalise the filter into the
DSP graph — that is M7's job and a different abstraction.
