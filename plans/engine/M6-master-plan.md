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
  waiving `position` under accuracy-policy D67) and 82 were recorded as `G3-IT-001` …
  `G3-IT-008` in `conformance/known-failures.md` for **G6**, grouped by the first field
  that diverges so each group is one piece of work. G6 has since settled them and
  renumbered the remainder `G6-IT-001` … `G6-IT-007`.
* Two new `QuirkSet` fields, each named by a corpus case: `it_pattern_loop`
  (`ItLoopDialect`, four `Cwt/v`-gated `SBx` profiles — accuracy policy D65) and the IT
  dialects' `tempo_model`.

## G6 — the conformance repair pass

* **55 of 121 IT cases pass** (14 with every field enforced, 41 waiving `position` under
  accuracy-policy D67), up from G3's 39. The 66 that remain are regrouped as
  `G6-IT-001` … `G6-IT-007` in `conformance/known-failures.md`, one group per first
  diverging field, and `--strict` names exactly those plus `C2-S3M-009` and F5's four XM
  records. MOD 16/27, S3M 16/17, MTM 1/3 and XM 87/93 are unchanged.
* Seven new accuracy-policy entries, **D80**–**D86**: the volume chain's one-step
  projection tolerance, the row-delay tick counter, ModPlug Tracker 1.16's IT pattern-loop
  profile, the ping-pong cycle with `S9E`/`S9F`, libxmp's ambiguous zero cutoff, the
  two-step cutoff tolerance and the four-unit pan tolerance.
* One new `QuirkSet` value named by a corpus case: `ItLoopDialect::ModPlug116`, selected
  by `FormatDialect::ModPlugIt` (D82, `data/pattern_loop_mpt.it`) — the same flow profile
  C5 already carries for S3M, so one detected tracker now has one profile in both formats.
* The IT golden hash moved. `goldens/it/synthetic__i16_mono_44100_linear.sha256` is a
  render of the synthetic IT fixture, and the fixture carries a `Zxx` filter macro and an
  `SCx`; both of G6's largest repairs — the filter staying engaged when a fully open
  cutoff arrives without a note trigger, and `SCx` silencing a voice without taking it off
  the channel — change what that render contains. Regenerated with `cargo xtask goldens`.
* `TempoModelId::ItModern` was left alone, as the task file required. **Four** cases
  depend on it: the `G6-IT-006` group is entirely `frame`-column divergence, and
  `libxmp-it-storlek-22`'s 7100-against-3445 at tick zero is the tick length itself.

## Exit criteria

The IT conformance corpus passes with exclusions justified; dense real-world `.it` files
sound right, including their voice-stealing behaviour.

G6 leaves 66 cases recorded rather than passing, so the corpus half of this criterion is
met only in the sense M2 met it: every remaining case is executed on every run, grouped,
and carries the evidence that would settle it. The owner's listening check on the three
dense `data/m/*.it` modules is the other half.

## Out of scope

Anything not in the IT format. Resist the temptation to generalise the filter into the
DSP graph — that is M7's job and a different abstraction.
