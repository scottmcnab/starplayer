# M10 — K3: SID voice emulation as an instrument

| Field | Value |
|---|---|
| Milestone | M10 ([master plan](M10-master-plan.md), "The task graph" section) |
| Status | Planned; pull-driven (not scheduled) |
| Depends on | K2 (generator voices, the `GeneratorKind` enum, `write_generator_levels`) |
| Blocks | — |
| Parallel with | K4, K6 |
| Recommended model | Claude Opus (a chip's documented quirks reproduced from published references, integer-only) |
| Verified by | agent (waveform tables against reSID's documented combined-waveform behaviour, envelope rate table exact, block-size determinism), then owner |

## Context for a fresh agent

The MOS 6581/8580 SID has three voices, each with four selectable waveforms (triangle,
sawtooth, pulse with 12-bit width, LFSR noise), hard sync and ring modulation between
voices, a hardware ADSR with a famous rate table, and one shared multimode filter. This
task makes **a SID voice an `Instrument`** — a keyboard plays SID voices with their
waveforms, ADSR, sync and ring mod — on the generator-voice infrastructure K2 built. It
does **not** play `.sid` files: a PSID file is a 6502 program and needs a CPU emulator,
which is a different milestone if ever.

Two scopes are deliberately separated: the **voice** (per-voice, exact to the chip's
digital logic, and integer by nature — this task) and the **filter** (analogue, chip-revision
dependent, the hard part of every SID emulator). The voice goes through the mixer's
existing IT resonant filter (a 2-pole low-pass with resonance) as the first cut; a
SID-shaped multimode filter is a research point, not a deliverable.

### Code you must read before changing anything

- `crates/starplayer-mixer/src/{voice,kernel}.rs` — `VoiceSource::Generator`,
  `GeneratorState`, `GeneratorKind`, the generator arm (K2).
- `crates/starplayer-synth/src/{fm,adsr}.rs` (K2, K1) — the instrument pattern and where
  control-rate envelopes live; note that SID's envelope is **per frame** in hardware and
  this task runs it inside the generator, not the instrument (research point 1).
- The reSID source (`sid.cc`, `wave.cc`, `envelope.cc` — Dag Lem, GPL; **read for the
  documented behaviour, do not copy code**), the "SID envelope rate table" and the
  combined-waveform sample tables' *descriptions*; the MOS 6581 datasheet.
- `plans/product/01-technical-architecture.md` §7.1 (as amended by K2), §7.3.

## Deliverables

### 1. `GeneratorKind::Sid` in the mixer

State: 24-bit phase accumulator (`u32`, chip-exact: increment = `frequency_register`
per chip clock, scaled to the output rate through a `u64` fixed-point clock ratio at
trigger), waveform select bits, 12-bit pulse width, 23-bit noise LFSR (taps 22 and 17,
clocked on accumulator bit 19 rising), sync/ring source (another voice's accumulator —
across voices in the pool: research point 2), the envelope generator (15-bit rate counter,
the exponential-decay counter at 255/93/54/26/14/6, the ADSR-delay bug reproduced as
documented), and the 8-bit envelope output. The per-frame step: advance the accumulator
by the increment scaled to one output frame (`clock_ratio = 985248 / sample_rate`, PAL,
in Q24), compute the 12-bit waveform output (triangle from the accumulator's top 12 bits
XOR'd with bit 23 (and the ring source's bit 23 when ring mod is on), saw = top 12 bits,
pulse = compare against width, noise from the LFSR bits, combined waveforms through a
table — research point 3), multiply by the envelope, and centre to `i16` scale.

Everything is integer. A step per frame rather than per chip clock (≈22 chip clocks per
output frame at 44.1 kHz) is the deliberate approximation: the accumulator advances by
the summed increment, so the waveform is sampled rather than clocked. Pulse and noise are
therefore aliased exactly as a non-oversampled SID emulator's are; record the choice, and
leave "clock-accurate with decimation" as a later `GeneratorKind::SidClocked`.

### 2. `SidInstrument: Instrument` (`crates/starplayer-synth/src/sid.rs`, feature `sid`)

Patch: waveform bits, pulse width, ADSR nibbles (0–15 each, the chip's table), ring/sync
flags and which of the instrument's two other "voices" they read, filter cutoff/resonance
mapped onto the IT filter's `FilterParams` (an explicit, documented mapping from the 11-bit
SID cutoff), chip model (6581/8580 — affects only the combined-waveform table and the
filter mapping here). Per-note state in the K1 pattern; `note_on` writes the frequency
register from the note (`f = note_hz × 16777216 / 985248`), triggers the generator with
gate on; `note_off` clears the gate (release runs in the generator); the instrument's
`control_tick` polls the generator's envelope-done flag through the voice and stops the
voice when the envelope reaches zero in release. Built-in patches: the classic bass, lead,
arp-ready pulse, snare (noise with fast decay), and a hard-sync lead. CC 74 → cutoff,
CC 71 → resonance, CC 1 → pulse-width sweep depth (an LFO in the instrument at control
rate writing the width per tick).

### 3. Hosts

`SynthKind::Sid(patch, model)`, `play --synth sid[:patch]`, web synth select.

### 4. Proof

- Envelope: the attack/decay/release times for every nibble match the datasheet table to
  the frame at 44.1 kHz (2 ms … 8 s attack; 6 ms … 24 s decay/release).
- Waveforms: saw at register value 0x1000 has fundamental at the expected Hz ± 0.01 %;
  pulse at width 0x800 is a square (odd harmonics only, evens below −50 dB); the noise
  LFSR sequence's first 64 outputs match a hand-computed reference; the combined
  waveform table's checksum matches the resolution's stated source.
- Ring modulation and sync between two instrument voices produce the documented spectra
  (sync at ratio 1.5 shows the harmonic series of the slave restart).
- Block-size determinism, both paths; cross-path bit-identity (K2's test pattern);
  goldens byte-identical; RT-safety; `no-std-purity`; `wasm-build`; `fma-check`.

### 5. Documentation

Architecture §7.1 generator table row; the sid module's doc comment stating what is exact
(accumulator, LFSR, envelope logic) and what is approximated (per-frame stepping, the
filter); append `## Research resolution`.

## Research points

1. **Envelope in the generator vs. the instrument**: SID's envelope is clocked per chip
   cycle with a rate counter; at control rate (1 ms) the fastest attack (2 ms) is two
   steps. Run it in the generator per frame (chosen above) and confirm the rate-counter
   arithmetic at frame granularity reproduces the table within one frame.
2. **Sync and ring across the pool**: the modulating voice's accumulator lives in another
   `Voice`; the generator arm processes voices in slot order and cannot read another
   slot mid-run without a borrow of the whole pool. Options: the instrument copies the
   source accumulator into the target's `GeneratorState` once per control tick (1 ms
   stale — audible for sync at high rates?), or the SID kind carries **all three voices in
   one generator state** and the instrument is 3-voice paraphonic like the chip. The
   second is faithful and simpler; recommend it and measure the state size.
3. **Combined waveforms**: reSID derives them from sampled chip data; the 8580's
   triangle+saw etc. are tables of 4096 entries each. Source them from the published
   measurements (cite), commit as generated data with the checksum, or omit combined
   waveforms in K3 with a note — say which.
4. **The filter**: the 6581's is a non-linear analogue mess and the 8580's is tamer; a
   proper emulation is Karlsson/Dag Lem's later work. Keep the IT filter for K3; note what
   a `SidFilter` multimode (LP/BP/HP with the cutoff curve per model) in `starplayer-dsp`
   would take as a follow-up.

## Verification

```
cargo test -p starplayer-mixer
cargo test -p starplayer-synth --features sid
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

`.sid`/PSID playback (a 6502 emulator); the analogue filter; digi playback tricks; SID
register-level control from MIDI (sysex); 2SID/3SID.
