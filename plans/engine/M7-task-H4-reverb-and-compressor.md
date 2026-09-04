# M7 — H4: Reverb and compressor inserts

| Field | Value |
|---|---|
| Milestone | M7 ([master plan](M7-master-plan.md), "The task graph" section) |
| Status | Ready once H1 and H2 have landed |
| Depends on | H1 (the `Insert` trait, `SmoothedParam`, `InsertKind`/`build`), H2 (`DspSample`, tables, `DelayLine`) |
| Blocks | H6, H7; the M7 exit criterion ("reverb on channel 1 alone, audible and correct") |
| Parallel with | H3 |
| Recommended model | Claude Opus (a fixed-point reverb and a table-driven gain computer, each proven on both paths) |
| Verified by | agent (block-size determinism with both active, allocation hook, RT60 and gain-curve tests, `fma-check`), then reviewer |

## Context for a fresh agent

H1 landed per-channel buses and the `Insert` trait in `starplayer-dsp`
(`crates/starplayer-dsp/src/insert.rs`), the gain insert (`effects/gain.rs`), `SmoothedParam`
(`smooth.rs`) and the engine's install/parameter ring. H2 landed `DspSample` for `f32` and
`i32`, the `pow2`/`log2`/`exp` and dB tables, `time_constant_q24`, and `DelayLine`. Read both
task files' `## Research resolution` sections in `plans/engine/complete/` first — they record
what actually landed.

This task adds the two effects the milestone's exit criterion and the master bus most want. The
reverb is the one the owner will listen to; the compressor is the one whose gain computer is
the hardest thing in M7 to keep transcendental-free. Both are generic over `S: DspSample`,
smooth every audible parameter, never allocate after `build`, and never put a float on the
fixed path. H3 (EQ, delay, chorus) runs concurrently and shares `effects/mod.rs`; keep your
`InsertKind` and `build` additions to one line each.

### Code you must read before changing anything

- `crates/starplayer-dsp/src/{insert,smooth,sample,tables,delay_line}.rs`, `effects/{mod,gain}.rs`.
- `crates/starplayer-engine/src/engine.rs` (how a chain runs; `InsertCommand`) and
  `crates/starplayer-engine/tests/block_size_determinism.rs` (the insert scenario H1 added).
- `crates/starplayer-mixer/src/master.rs` — the soft limiter the master chain feeds; the
  compressor sits before it and must not fight it.
- `crates/starplayer-offline/tests/render_allocation.rs`.
- `plans/product/01-technical-architecture.md` §7.2, §7.3; `CLAUDE.md`.

## Deliverables

### 1. `Reverb` (`effects/reverb.rs`)

Freeverb's topology (Jezar's public-domain design): per channel eight parallel lowpass-feedback
comb filters into four series allpasses, the right channel's delay lengths offset by 23 frames
for width; lengths scaled from the 44.1 kHz tuning to the actual sample rate at `build`. It is
chosen because it is table-free, integer-friendly and universally recognised as "a reverb".
Parameters: room size (percent → comb feedback 0.7..0.98), damping (percent → the comb's
one-pole coefficient), width (percent), mix (percent), pre-delay (ms, 0–100, one `DelayLine`),
`freeze` switch. On the fixed path the comb and allpass feedback multiplies are `mul_q24`, the
comb state saturates, and the eight combs are summed with a `>> 3` headroom shift (state the
scaling so H6 can vectorise it without changing a bit). Comb and allpass delay lines are the
smallest power of two above each length (H2's `DelayLine`), which costs memory — record the
total per instance at 48 kHz stereo in the resolution; if it exceeds 256 KB, add a
`DelayLine::with_exact_capacity` that masks by a conditional subtract instead.

### 2. `Compressor` (`effects/compressor.rs`)

Feed-forward, stereo-linked peak detector with attack/release (ms, via `time_constant_q24`),
threshold (centi-dB), ratio (×100, 1.0–20.0, and ∞ as a switch = limiter), knee (centi-dB,
soft), make-up gain (centi-dB, with an `auto` switch), look-ahead 0 (no extra latency — the
master chain must not delay the mix against the scope taps and telemetry). The gain computer
runs **once per block** on the block's peak envelope, in the log domain through H2's
`log2_q16`/`db` tables, and the resulting gain is applied through a `SmoothedParam` across the
block, so per-frame cost is one multiply. Gain reduction in centi-dB is exposed through
`param(ParamId::GAIN_REDUCTION)` as a read-only parameter so H7 can show a meter.

### 3. Registry and descriptors

`InsertKind::{Reverb, Compressor}`, `build` arms, descriptors with units/ranges/defaults,
and the `descriptor_roundtrip` test pattern from H3 (if H3 has not merged, write it here — the
merge will de-duplicate).

### 4. Proof

- **Block-size determinism**: the H1 scenario gains a reverb on channel 1 and a compressor
  on the master, with parameter sweeps queued mid-song, on both paths, every block size.
  Byte-identical.
- **Allocation hook**: `render_allocation.rs` renders with both installed. Zero allocations.
- **Reverb**:
  - an impulse at room size 50 %, mix 100 % produces a tail whose RMS decays monotonically
    (per 100 ms window) and whose RT60 lies between 0.5 s and 3 s on both paths;
  - room size 0 % / damping 100 % decays below −60 dBFS within 300 ms;
  - `freeze` holds the tail's RMS within ±1 dB over 5 s;
  - fixed vs float segmental SNR ≥ 50 dB on a drum-loop fixture (the reverb's recursion
    amplifies rounding; 50 rather than 60 is the number to beat — record what you measure);
  - **the exit criterion**: render `reflex.s3m` (`crates/starplayer-offline`'s fixture set)
    with the reverb on channel 1 only, mix 50 %; channel 1's bus has a tail (its RMS in the
    200 ms after a note-off is > −40 dBFS) and every other channel's output is bit-identical
    to a render with no inserts. Write it as an offline test.
- **Compressor**:
  - static curve: sines at −40, −20, −10, 0 dBFS through threshold −20 dB / ratio 4:1 /
    hard knee come out at −40, −20, −17.5, −15 dBFS ± 0.3 dB after the attack has settled,
    on both paths;
  - attack/release: a −20 → 0 dBFS step reaches 90 % of its gain reduction within
    `attack × 2.3` and releases likewise;
  - the gain-reduction read-back matches the measured reduction ± 0.5 dB.
- `cargo xtask ci --job fma-check`.

### 5. Documentation

Architecture §7.2: add both to the effects table, note the reverb's memory and the
compressor's block-rate gain computer. Append `## Research resolution` here.

## Research points

1. **Freeverb on the fixed path**: the comb's lowpass-feedback loop `y = x + f·(y1·(1−d) + y2·d)`
   in Q8.24 with `i32` state — does it need `i64` state to avoid limit cycles at room size
   98 %? Measure the idle-noise floor after the tail decays; it must be silent (all zero) or
   below −90 dBFS.
2. **Compressor detector**: peak vs. RMS; peak is cheaper and the design says "peak" — confirm
   or argue for RMS with a table-driven square root (`pow2(log2(x)/2)` is available).
3. **Block-rate gain computing and pumping**: with a 128-frame block (2.9 ms at 44.1 kHz), is
   the smoothed per-block gain audibly different from per-frame? Test with a fast attack
   (0.1 ms) on a click train; if it is, compute per 32 frames.

## Verification

```
cargo test -p starplayer-dsp
cargo test -p starplayer-dsp --features std
cargo test -p starplayer-engine --test block_size_determinism
cargo test -p starplayer-offline
cargo test --workspace
cargo xtask goldens --check
cargo xtask ci --job host-tests
cargo xtask ci --job rt-safety
cargo xtask ci --job clippy
cargo xtask ci --job no-std-purity
cargo xtask ci --job fma-check
```

## Out of scope

EQ, delay, chorus (H3); host, CLI and web surfaces (H7); SIMD (H6); convolution or FDN
reverbs; side-chain input; look-ahead limiting.
