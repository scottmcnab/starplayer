# M10 — K6: Karplus-Strong plucked string

| Field | Value |
|---|---|
| Milestone | M10 ([master plan](M10-master-plan.md), "The task graph" section) |
| Status | Planned; pull-driven (not scheduled) |
| Depends on | K2 (generator voices), M7-H2 (`DelayLine`, one-pole tables) |
| Blocks | — (the first physical model; anything "physical modelling upward" starts here) |
| Parallel with | K3, K4 |
| Recommended model | Claude Sonnet (one well-documented algorithm on infrastructure K2 built) |
| Verified by | agent (pitch accuracy, decay time, block-size determinism, goldens), then owner |

## Context for a fresh agent

Karplus-Strong is a delay line of `L` frames fed back through a one-pole low-pass: fill it
with noise, and the loop rings at `sample_rate / L` with the high harmonics decaying
fastest — a plucked string. It is the smallest physical model there is, and on K2's
generator-voice infrastructure it is a `GeneratorKind` with a delay line and two
coefficients. The one thing it needs that FM and SID did not is **memory per voice**: a
low E (41 Hz) at 48 kHz is a 1170-frame line, which does not fit in a fixed-size
`GeneratorState`. So this task also settles how a generator owns a buffer without
allocating on the audio thread.

### Code you must read before changing anything

- `crates/starplayer-mixer/src/{voice,kernel}.rs` — `GeneratorState`, `GeneratorKind`,
  the generator arm, `VoicePool::allocate_generator` (K2).
- `crates/starplayer-synth/src/{fm,adsr}.rs` (K2, K1) — the instrument pattern.
- `crates/starplayer-dsp/src/{delay_line,tables}.rs` — `DelayLine::read_fractional`,
  `one_pole_cutoff_q24` (H2/H3).
- `crates/starplayer-core`'s `Xorshift32` (the excitation noise; seeded per note so a
  note is deterministic).
- `plans/product/01-technical-architecture.md` §7.1 (as amended by K2), §8.

## Deliverables

### 1. Generator memory

`VoicePool::new` (or a `with_generator_memory(frames_per_voice)` variant used by the
persistent hosts and `render_song`) allocates one arena `Box<[i32]>` of
`capacity × GENERATOR_FRAMES` (default 2048 frames — down to 23 Hz at 48 kHz — 8 KB per
voice, 2.1 MB for 272 voices, allocated once); `GeneratorState` carries an arena *index*,
never a pointer, and the kernel's generator arm borrows the voice's window by index. A
pool built without generator memory refuses `allocate_generator` for kinds that need it
(`None`, like a full pool) — so MOD/S3M/MTM hosts and the offline goldens pay nothing.

### 2. `GeneratorKind::Pluck`

State: line length `L` (Q16.16 for fractional tuning through `read_fractional`),
write index, the loop filter's one-pole coefficient (brightness), a decay gain (Q15),
the pick-position comb (an optional second read at `L × position`), and the excitation:
`Xorshift32` noise for `L` frames at trigger, low-passed by a "pick hardness" one-pole,
written into the line by the **instrument at trigger time**? — no: the excitation must
run in the generator (the instrument cannot touch arena memory from `note_on` without a
context call) — so the state carries `excite_remaining: u32` and the kernel fills the line
during the first `L` frames while already reading it, which is how the original
algorithm runs anyway. Extended KS (Jaffe–Smith): the fractional-delay allpass for exact
tuning and the dynamic-level low-pass are research point 1.

Integer only, per master-plan decision 3.

### 3. `PluckInstrument: Instrument` (`crates/starplayer-synth/src/pluck.rs`, feature `pluck`)

Patch: brightness (loop cutoff), decay (seconds → the per-frame gain via `pow2_q24`),
pick position, pick hardness, a `damp` on note-off (release: raise the loop loss so the
string stops in ~50 ms rather than ringing — the "palm mute"). `note_on` computes `L`
from the note (`sample_rate / f`, fractional), seeds the noise from the note and a
per-instrument counter, triggers; `note_off` writes the damp gain into the state through
`write_generator_levels`; the instrument stops the voice when the generator reports the
line's energy below a floor (a per-block RMS flag in the state, read on `control_tick`).
CC 74 → brightness, CC 71 → pick position, CC 1 → decay.

### 4. Hosts

`SynthKind::Pluck(patch)`, `play --synth pluck`, web synth select.

### 5. Proof

- Pitch: notes 40, 52, 64, 76 produce fundamentals within 2 cents (with the fractional
  read) — the integer-length version is off by up to 30 cents at 76, which is the
  reason for the fractional read; show both numbers.
- Decay: the −60 dB time at decay 1.0 s is 1.0 s ± 10 %; brightness 0 decays the 5th
  harmonic at least 3× faster than the fundamental.
- Determinism: same note script, both paths, six block sizes, byte-identical; cross-path
  bit-identity per K2's pattern; the seed makes two renders of the same note identical.
- Goldens byte-identical; RT-safety with a 32-voice strum; `no-std-purity`;
  `wasm-build`; `fma-check`; `clippy`; the `Voice` size assertion still holds (the arena
  index is a `u16`).

### 6. Documentation

Architecture §7.1 (generator memory as an arena; the Pluck row), §8 (the arena is
allocated once); append `## Research resolution`.

## Research points

1. **Extended KS**: the allpass fractional delay vs. `read_fractional`'s linear
   interpolation (which adds loss at high pitch — measure the extra damping at note 88);
   the Jaffe–Smith dynamic-level filter for velocity → brightness.
2. **Arena size**: 2048 frames caps the lowest note at 23 Hz at 48 kHz; at 96 kHz it is
   47 Hz. Decide whether the arena size scales with the sample rate at pool construction
   (the persistent hosts know the rate) and what a note below the floor does (play an
   octave up, or refuse — say which).
3. **Stereo**: two slightly detuned lines per note (a 12-string) is two voices; note it as
   a patch option through the pair mechanism K1 built for the wavetable morph.

## Verification

```
cargo test -p starplayer-mixer
cargo test -p starplayer-synth --features pluck
cargo test --workspace
cargo xtask goldens --check
cargo xtask ci --job host-tests
cargo xtask ci --job rt-safety
cargo xtask ci --job no-std-purity
cargo xtask ci --job wasm-build
cargo xtask ci --job fma-check
cargo xtask ci --job clippy
```

## Out of scope

Digital waveguide bodies, bowed strings, wind models (the "upward" of the master plan —
each is a further `GeneratorKind` once the arena exists); SIMD; sympathetic resonance.
