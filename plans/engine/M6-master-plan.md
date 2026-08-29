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

## Exit criteria

The IT conformance corpus passes with exclusions justified; dense real-world `.it` files
sound right, including their voice-stealing behaviour.

## Out of scope

Anything not in the IT format. Resist the temptation to generalise the filter into the
DSP graph — that is M7's job and a different abstraction.
