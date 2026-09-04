# M7 — H2: DSP primitives — `DspSample`, the tables, biquads, delay lines, an LFO

| Field | Value |
|---|---|
| Milestone | M7 ([master plan](M7-master-plan.md), "The task graph" section) |
| Status | Ready |
| Depends on | — |
| Blocks | H3, H4 |
| Parallel with | H1, H5 |
| Recommended model | Claude Sonnet (self-contained numerics with exact tests; no engine changes) |
| Verified by | agent (unit tests against `f64` references under `std`, `no-std-purity`, `fma-check`), then reviewer |

## Context for a fresh agent

`starplayer-dsp` (`crates/starplayer-dsp/`) is `#![no_std]`, `#![forbid(unsafe_code)]`, and
transcendental-free: architecture §7.3 bans `sin`, `exp`, `powf` and friends from the real-time
path because different targets' `libm` disagree, and the fixed-point path must be bit-identical
on x86, ARM and WASM. The crate today holds the interpolators, `GainRamp`, and IT's resonant
filter (`filter.rs`), which shows every convention this task follows: coefficients in Q8.24
`i32`, tables in `starplayer-core` (`LINEAR_FREQUENCY_TABLE` is `2^(n/768)` in Q8.24;
`IT_RESONANCE_TABLE_Q24`), a `const fn` per lookup, `round_shift_nearest` for every narrowing,
`f32` twin of every fixed function, and a test that pins the fixed result against the float one
to a stated tolerance.

M7's effects (H3 EQ/delay/chorus, H4 reverb/compressor) need a small shared toolkit. This task
builds it with no knowledge of the `Insert` trait H1 is writing concurrently — pure functions
and small structs only. The one shared name is `DspSample`, which H1 also references; use
**exactly** the surface below so the merge is a union.

### Code you must read before changing anything

- `crates/starplayer-dsp/src/{filter,interpolate,ramp}.rs` and `Cargo.toml`.
- `crates/starplayer-core/src/tables.rs` (or wherever `LINEAR_FREQUENCY_TABLE` lives — find it
  with `grep -rn LINEAR_FREQUENCY_TABLE crates/starplayer-core`), and the `gain.rs` precedent
  for const-evaluated tables in `crates/starplayer-mixer/src/gain.rs`.
- `xtask/src/main.rs` `fma_findings` / `FmaPass` — `starplayer-dsp` is audited for fused
  multiply-add; the audit must keep passing.
- `CLAUDE.md` working agreements (full names, compact formatting, no `cargo fmt`).

## Deliverables

### 1. `DspSample` (`crates/starplayer-dsp/src/sample.rs`)

```rust
pub trait DspSample: Copy + Default + Send + PartialEq + core::fmt::Debug {
    const ZERO: Self;
    fn from_i16(value: i16) -> Self;
    fn add(self, other: Self) -> Self;
    fn sub(self, other: Self) -> Self;
    /// Multiply by a Q8.24 coefficient (float: `coefficient as f32 / 2^24`).
    fn mul_q24(self, coefficient: i32) -> Self;
    /// Multiply by a Q1.15 gain (0..=32768 is 0..=1.0).
    fn scale_q15(self, gain: i32) -> Self;
    /// Bound to the path's full scale (fixed: `i32` saturating; float: identity).
    fn saturate(self) -> Self;
}
```

`impl` for `f32` (plain IEEE ops, no FMA — the audit checks) and `i32` (widen to `i64`,
`round_shift_nearest`, saturate on narrowing). The fixed sample scale is the mixer's: an `i16`
sample widened, as `FixedPath::Mono` is. Document the headroom: the fixed accumulator carries
`i16 × gain` sums in `i32`, so an effect may temporarily exceed `i16` range and must `saturate`
only where the mixer would.

### 2. Tables (`crates/starplayer-dsp/src/tables.rs`)

All `const`, all with an `f32` twin and a `std`-only test against `f64`:

- `pow2_q24(x_q16: i32) -> i32`: `2^x` for `x` in Q16.16, split into integer shift and fraction
  through `LINEAR_FREQUENCY_TABLE` (768 steps per octave, interpolate between neighbours).
  Also `exp_neg_q24(x_q16)` as `pow2(−x·log2 e)` with the constant in Q16.
- `log2_q16(value: u32) -> i32`: leading-zero count for the integer part, a 256-entry
  mantissa table with linear interpolation for the fraction. Error under `2^-12`.
- `db_to_gain_q15(centi_db: i32) -> i32` and `gain_to_centi_db(gain_q15: i32) -> i32`, built on
  the two above; clamp to `[-9600, +2400]` centi-dB.
- `sin_q15(phase: u32) -> i32` / `cos_q15` over a `u32` turn (`0..2^32` is one cycle), a
  1024-entry quarter-wave table with symmetry and linear interpolation; the const table is
  built by a `const fn` Taylor series (enough terms for `< 1` LSB at Q15) — no `libm` even at
  const time is not required, but the test must show the table equals `f64::sin` rounded.
- `time_constant_q24(milliseconds: i32, sample_rate_hz: u32) -> i32`: the one-pole coefficient
  `exp(−1/(t·sr))` in Q8.24 for envelope followers, via `exp_neg_q24`.

### 3. Biquad (`crates/starplayer-dsp/src/biquad.rs`)

`BiquadCoefficients { b0, b1, b2, a1, a2 }` in Q8.24 with the RBJ cookbook cookers
`low_shelf(frequency_hz, gain_centi_db, slope_q15, sample_rate_hz)`, `high_shelf(...)`,
`peaking(frequency_hz, gain_centi_db, q_q15, sample_rate_hz)`, `low_pass`, `high_pass` — using
`sin_q15`/`cos_q15` for `ω` and `db_to_gain_q15` for `A`, with a float twin of each cooker.
Transposed direct form II step `fn step<S: DspSample>(&self, input: S, state: &mut [S; 2]) -> S`.
Test: the fixed and float cookers agree within `2^-16` relative on every coefficient across a
grid of frequencies/gains; a peaking filter at 1 kHz +6 dB passes a 1 kHz sine at +6 dB ± 0.1
dB and 100 Hz at 0 dB ± 0.1 dB on both paths.

### 4. Delay line (`crates/starplayer-dsp/src/delay_line.rs`)

`DelayLine<S: DspSample>`: `new(capacity_frames)` (the **only** allocating call; documented as
control-side), `write(S)`, `read(delay_frames: u32) -> S`, `read_fractional(delay_q16: u32) -> S`
(linear between neighbours), `reset()`, `capacity()`. Power-of-two capacity with a mask, no
`%`, every index through `get`. A `StereoDelayLine` is two of them.

### 5. LFO (`crates/starplayer-dsp/src/lfo.rs`)

`Lfo { phase: u32, increment: u32 }` with `sine() -> i32` (Q15) and `triangle()`, `advance()`,
`set_rate(centi_hz, sample_rate_hz)`. Integer phase, so it never drifts between block sizes.

### 6. Wiring

`lib.rs` declares the new modules and re-exports the public names, **one `pub use` item per
line** (H1 is editing the same file). Crate docs updated. No new dependencies.

## Research points

1. Which of `LINEAR_FREQUENCY_TABLE`'s indexing helpers already exist in core (the IT filter
   interpolates fifths of a step); reuse rather than re-derive.
2. Q8.24 coefficient range: a low shelf at 20 Hz on 44.1 kHz gives `a2` near −1 and `b0`
   possibly above 8.0 at +24 dB; check whether Q8.24 clips and, if it does, clamp the gain
   range or use Q4.28 for the `b` terms — state which and why.
3. The compressor (H4) will want `log2` of an `i32` envelope and `pow2` of a negative
   Q16 exponent; make the signatures cover that without a second API.

## Verification

```
cargo test -p starplayer-dsp
cargo test -p starplayer-dsp --features std
cargo test --workspace
cargo xtask ci --job clippy
cargo xtask ci --job no-std-purity
cargo xtask ci --job fma-check
```

## Out of scope

The `Insert` trait and any effect (H1, H3, H4); SIMD (H6); touching the mixer or engine.
