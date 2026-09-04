# M7 — H3: EQ, delay and chorus inserts

| Field | Value |
|---|---|
| Milestone | M7 ([master plan](M7-master-plan.md), "The task graph" section) |
| Status | Ready once H1 and H2 have landed |
| Depends on | H1 (the `Insert` trait, `SmoothedParam`, `InsertKind`/`build`), H2 (`DspSample`, tables, `BiquadCoefficients`, `DelayLine`, `Lfo`) |
| Blocks | H6, H7 |
| Parallel with | H4 |
| Recommended model | Claude Opus (three effects on two arithmetic paths, each with a determinism and an audibility proof) |
| Verified by | agent (block-size determinism with every effect active, allocation hook, spectral/impulse tests, `fma-check`), then reviewer |

## Context for a fresh agent

H1 landed per-channel buses and the `Insert` trait in `starplayer-dsp`
(`crates/starplayer-dsp/src/insert.rs`), with a gain insert as its first implementation
(`effects/gain.rs`), `SmoothedParam` (`smooth.rs`), and the engine-side install/parameter ring.
H2 landed the arithmetic and the tables: `DspSample` for `f32` and `i32`, `pow2`/`log2`/`sin`
lookups, RBJ biquad cookers in Q8.24, `DelayLine` with fractional reads, and an integer-phase
`Lfo`. Read both task files' `## Research resolution` sections in `plans/engine/complete/`
before starting — they record what actually landed where the task files above guessed.

This task adds the first three real effects. Each one is generic over `S: DspSample`, so it
exists on both mix paths; each smooths every audible parameter; none allocates after
construction; none uses a float on the fixed path or a transcendental anywhere. The reverb and
compressor are H4, concurrently — you share `effects/mod.rs` (`InsertKind`, `build`); keep
your additions one line each so the merge is a union.

### Code you must read before changing anything

- `crates/starplayer-dsp/src/{insert,smooth,sample,tables,biquad,delay_line,lfo}.rs`,
  `effects/{mod,gain}.rs`.
- `crates/starplayer-engine/src/engine.rs` (how a chain is run; `InsertCommand`) and
  `crates/starplayer-engine/tests/block_size_determinism.rs` (the insert scenario H1 added —
  you extend it).
- `crates/starplayer-offline/tests/render_allocation.rs`.
- `plans/product/01-technical-architecture.md` §7.2, §7.3; `CLAUDE.md`.

## Deliverables

### 1. `Eq` (`effects/eq.rs`)

Three bands: low shelf, peaking, high shelf, each with frequency (Hz, 20–20000), gain
(centi-dB, −2400..+2400) and Q/slope (×100), plus a master `enabled`. Coefficients are
re-cooked from the tables when a parameter's smoothed value moves, once per block (not per
frame): cooking per block is the compromise between a click and per-frame `sin` lookups —
document it and make the block-rate step small enough that a full sweep over `SMOOTH_FRAMES`
is inaudible (test: no step in the output larger than the per-block coefficient delta implies).
State per band `[S; 2]` for each of left and right.

### 2. `Delay` (`effects/delay.rs`)

Stereo delay: time (ms, 1–2000; capacity chosen at `build` from the sample rate and the
maximum), feedback (percent, 0–100 — clamp below unity with a margin so the fixed path cannot
run away), mix (percent), `ping_pong` switch, and a one-pole low-pass in the feedback path
(cutoff Hz). Time changes read fractionally through `DelayLine::read_fractional` at the
smoothed delay, so a time sweep pitch-shifts like tape rather than crackling. Wet/dry is
equal-power via `sin_q15`/`cos_q15`.

### 3. `Chorus` (`effects/chorus.rs`)

Two or three modulated taps per channel (`voices` switch 2/3), rate (centi-Hz, 5–500), depth
(ms ×100), base delay (ms ×100), mix (percent), stereo spread (the right channel's LFO phase
offset). LFO from H2's `Lfo` (sine); the delay read is fractional. Because the LFO phase is an
integer that advances once per frame, the output is a pure function of frames rendered — the
determinism scenario proves it at every block size.

### 4. Registry

`InsertKind::{Eq, Delay, Chorus}` and their `build` arms; each effect's `InsertDescriptor`
lists its parameters with units, ranges and defaults in the order their `ParamId`s are numbered.
A `descriptor_roundtrip` test per effect: every `ParamSpec::default` is what `param()` returns
after `build`, and `set_param` of each bound is accepted and clamped.

### 5. Proof

- **Block-size determinism**: the H1 scenario gains one instance of each effect (EQ on channel
  0, delay on channel 1, chorus on the master), with a parameter sweep queued mid-song, on
  both paths, at every block size. Byte-identical.
- **Allocation hook**: `render_allocation.rs` renders with all three installed. Zero
  allocations.
- **Audibility, per effect, both paths**:
  - EQ: a peaking band +12 dB at 1 kHz on white noise (the core `Xorshift32`) raises the 1 kHz
    bin by 12 ± 0.5 dB in a 4096-point DFT computed in the test (a plain `f64` DFT under `std`
    is fine) and leaves 100 Hz and 10 kHz within ±0.5 dB.
  - Delay: an impulse at 300 ms / 50 % feedback / 100 % mix produces echoes at 300, 600, 900 ms
    with amplitudes 1, 0.5, 0.25 (±1 LSB on the fixed path); ping-pong alternates channels.
  - Chorus: a 1 kHz sine through the chorus has energy within ±depth-implied Hz of 1 kHz and
    the output never exceeds the input peak by more than the summed tap gains.
- **Fixed vs float**: for each effect, render the same input on both paths and assert segmental
  SNR ≥ 60 dB (`starplayer_offline::segmental_snr_db` — or a local copy in dsp's tests).
- **Bit-exactness of the fixed path across targets** is by construction (tables and integer
  arithmetic); note in the resolution that no float enters the fixed path.
- `cargo xtask ci --job fma-check` (dsp is audited; the float biquad must not fuse).

### 6. Documentation

Architecture §7.2: a short table of the effects, their parameters and the cooking rate.
Append `## Research resolution` here.

## Research points

1. **Cooking rate**: per block vs. per N frames for the EQ; measure the largest sample-to-sample
   step during a full-range gain sweep and pick.
2. **Delay feedback stability on the fixed path**: with the feedback low-pass and saturation,
   show that feedback 100 % settles rather than grows (the margin you clamp to).
3. **Chorus tap count and spread** — what sounds like a chorus rather than a flanger; record
   the defaults you chose and why.

## Verification

```
cargo test -p starplayer-dsp
cargo test -p starplayer-dsp --features std
cargo test -p starplayer-engine --test block_size_determinism
cargo test --workspace
cargo xtask goldens --check
cargo xtask ci --job host-tests
cargo xtask ci --job rt-safety
cargo xtask ci --job clippy
cargo xtask ci --job no-std-purity
cargo xtask ci --job fma-check
```

## Out of scope

Reverb and compressor (H4); host, CLI and web surfaces (H7); SIMD (H6); parametric EQ with
more than three bands; tempo-synced delay times (the engine's tempo is a source-side notion).
