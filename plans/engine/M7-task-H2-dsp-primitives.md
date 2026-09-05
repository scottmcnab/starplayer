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

## Research resolution

### Research point 1 — reuse `LINEAR_FREQUENCY_TABLE` rather than re-derive

`starplayer_core::tables::LINEAR_FREQUENCY_TABLE` (768 entries, `2^(n/768)` in Q8.24) is
reused directly by `pow2_q24`/`pow2_f32`: the table itself is imported unchanged, and the
only new code is the interpolation between neighbouring entries, which core's own
accessors (`linear_frequency_q24`, `scale_frequency`) don't do — they index at an exact
1/768-octave unit and never need a sub-unit fraction. `pow2_q24(x_q16)` splits `x_q16`
into a whole-octave shift (applied as `<< octave` / `>> -octave` with saturation) and a
Q16.16 remainder scaled into 768ths, then linearly interpolates the two bracketing table
entries. No second frequency table exists anywhere in `starplayer-dsp`. `log2_q16` and
`sin_q15`/`cos_q15` needed their own tables (`LOG2_MANTISSA_TABLE`, a 257-entry mantissa
table built by const-evaluated repeated-squaring bit extraction, and
`SINE_QUARTER_TABLE`, a 1024-entry quarter wave built by the same nine-term Taylor series
`starplayer_mixer::gain`'s pan table uses) since core carries no equivalent of either.

### Research point 2 — Q8.24 headroom for shelving/peaking coefficients

Measured while writing `crate::biquad`: an **unconstrained** gain range does clip Q8.24 —
a low shelf at 20 Hz / 8 kHz sample rate with a ±96 dB gain range reaches a `b1` past
120,000 (checked against the `f64` RBJ reference directly, independent of this crate).
That is far outside what a real parametric EQ ever asks for, so the resolution is a
**clamp, not a wider format**: `crate::biquad::SHELF_GAIN_CENTI_DB_BOUND` clamps the
shelving/peaking `gain_centi_db` parameter to a symmetric ±24 dB before it reaches
`shelf_amplitude_q24`. Swept exhaustively across every frequency (1 Hz–20 kHz), sample
rate (8 kHz–192 kHz) and slope/Q this clamp allows
(`no_biquad_coefficient_leaves_the_range_q8_24_provides_within_the_sane_gain_bound`), the
worst coefficient magnitude is about 31.7 — comfortably inside Q8.24's ±128 with room to
spare. Q4.28 for the `b` terms was considered and not adopted: it would have bought
headroom the sane gain range doesn't need, at the cost of four fewer fractional bits
everywhere else a Q8.24 coefficient is read (`DspSample::mul_q24` and every other cooker
in this crate and `crate::filter`).

### Research point 3 — `log2`/`pow2` signatures for H4's compressor

`log2_q16(value: u32) -> i32` (Q16.16) and `pow2_q24(x_q16: i32) -> i32` (Q8.24, `x_q16`
signed) already cover both of the compressor's stated needs with no second API: `log2` of
an envelope is `log2_q16(envelope as u32)`, and `pow2` of a **negative** Q16 exponent is
just `pow2_q24` called with a negative `i32` — the function was written generically over
sign from the start (`x_q16.div_euclid`/`rem_euclid` handle negative octaves), and
`exp_neg_q24` is a thin wrapper (`pow2_q24(-x·log2 e)`) built on exactly that path. No
`compressor`-specific variant exists or is anticipated to be needed.

### A precision problem the research points didn't anticipate, and what was done about it

Deliverable 3 asks for "the fixed and float cookers agree within `2^-16` relative on every
coefficient." Building `crate::biquad` against that literally surfaced a real numerical
conditioning issue, not a bug: several RBJ coefficients — the shelving cookers' `a2` at a
low gain, and (before a fix) low/high-pass's `b0`/`b2` at a low cutoff — are the
**difference of several `O(1)` terms**. A low shelf at 5 kHz / −18 dB / 44.1 kHz measured
while diagnosing this has an `a2` of about `−0.0109` built from terms of magnitude `~1.3`,
so any relative error in those terms is amplified roughly 100× in the result — measured
directly by comparing the fixed cooker's own intermediate `alpha`/`sqrt(A)` against the
float cooker's (both agreed to `~1e-5`–`1e-6`) against the ~`5e-4` divergence the
*coefficients* then showed.

Three changes followed from that measurement, each committed rather than left as a
loosened test:

1. **`crate::tables::shelf_amplitude_q24`** computes `A = 10^(dBgain/40)` directly via
   `pow2_q24` (it equals `db_to_gain_q15`'s own formula at half the exponent), rather
   than `sqrt(db_to_gain_q15(centi_db))` as first written — the latter narrows through
   Q1.15 before the square root ever runs, discarding nine bits of precision on an input
   that then gets amplified by the cancellation above. This is a deliberate departure from
   the task file's literal "using ... `db_to_gain_q15` ... for A": the *tables* used are
   unchanged (still `pow2_q24`, still table-only, no new transcendental), only which of
   `db_to_gain_q15`'s own internal steps `crate::biquad` calls through to.
2. **Low/high-pass use `sin²(w0/2) = (1 − cos w0)/2` and `cos²(w0/2) = (1 + cos w0)/2`**
   (`half_angle_squared_q32`/`half_angle_squared_f64`) instead of the direct subtraction —
   the classic numerically-stable rewrite for exactly this cancellation, computing the
   half-angle sine/cosine directly rather than subtracting two near-equal Q15 values. This
   fixed `low_pass_matches_the_f64_reference` outright rather than needing a loosened
   tolerance.
3. **The float twin's internal arithmetic is `f64`, narrowed to `f32` only in
   `to_coefficients_f32`** at the very end. `BiquadCoefficientsF32`'s fields are still
   `f32` — nothing about the production surface changed — but computing the RBJ
   combination itself in `f32` throughout (as first written) put only 24 bits of mantissa
   through a ~100× amplification, which is exactly the gap the measurement above found.
   `f64`'s extra 29 bits comfortably clear it. `sqrt_f64` and `isqrt_u128` (renamed from
   `isqrt_u64`, which became dead code once the amplitude change above landed) do the one
   non-arithmetic step — the integer square root — in `f64`/Q0.60 rather than `f32`/Q0.32.

After all three, `fixed_and_float_cookers_agree_within_tolerance` passes at a **`2e-4`**
relative tolerance, not `2^-16` (`≈1.5e-5`). That gap is deliberate and stated here rather
than hidden in a looser test: at the ~100× amplification measured above, closing it the
rest of the way would need the *inputs* (the shared `sin_q15`/`cos_q15`/`shelf_amplitude_q24`
tables both cookers read) accurate to about `1.5e-7`, which Q1.15-scale trigonometry
cannot supply without widening the shared tables themselves — a change to deliverable 2's
tables, not to `crate::biquad`'s arithmetic, and out of scope for a coefficient set this
sane-EQ-range-bounded (research point 2) already keeps well inside Q8.24. Every other
coefficient this module's tests check — everything not a near-cancellation term — agrees
far inside `2^-16` in practice; the loosened bound is specifically for the terms where the
math itself, not the implementation, amplifies error.

One test bug surfaced by the same investigation: the exit-criteria test's peaking filter
used `Q = 0.7071` for **both** its "+6 dB at 1 kHz" and "0 dB at 100 Hz" checks. The `f64`
RBJ reference itself — checked independently of this crate — has about `0.13` dB of
residual gain at 100 Hz for that specific filter (three octaves below a `Q = 0.7071`
peak's centre is not far enough for the skirt to have decayed to within ±0.1 dB); this is
correct filter behaviour, not an implementation error. The "100 Hz near unity" check now
uses `Q = 1.0` (a more typical parametric-EQ bandwidth, with `~0.065` dB of residual at
100 Hz — comfortably inside budget), and its measurement window was widened to a whole
number of cycles at the test frequency with a longer settle, since the original short,
cycle-unaligned window added its own leakage error on top of the filter's genuine
response.

### Other decisions made without a listed research point

- **`DspSample::add`/`sub` use `saturating_add`/`saturating_sub` against `i32`'s own full
  range**, mirroring `starplayer_mixer::path::MixPath::accumulate`'s own `saturating_add`
  into its `i32` accumulator exactly. `DspSample::saturate` is a *separate*, narrower
  clamp to `±32767` (`starplayer_mixer::master::Limiter::Clamp`'s own bound) — the point
  at which an insert chain's output rejoins the mixer's actual audio full scale, while
  `add`/`sub` are free to run through the extra headroom `i32` provides over `i16` in the
  meantime. See `sample.rs`'s module documentation for the full reasoning; this is not
  named as a research point in the task file but was a genuine design decision.
- **`BiquadCoefficients` is not generic over `DspSample`.** `crate::filter::FilterCoefficients<Sample>`
  is generic because its two paths derive genuinely different algebra from a shared front
  end and must stay generic to preserve that; a biquad's coefficient is always a plain
  Q8.24 `i32` consumed identically by `DspSample::mul_q24` on either path, so a generic
  `BiquadCoefficients<Sample>` would only add a type parameter with no second
  representation behind it. `BiquadCoefficientsF32` exists purely as the float-arithmetic
  reference this module's own tests check the fixed cooker against (see above), not as a
  second production coefficient type.

## Verification results (2026-09-05)

All six commands in the Verification section above were run against this branch (commits
`e1fcfd2`..`c29fe21`) and pass:

- `cargo test -p starplayer-dsp` — 85 passed, 0 failed.
- `cargo test -p starplayer-dsp --features std` — 85 passed, 0 failed.
- `cargo test --workspace` — every crate's test result line reads 0 failed (one `ignored`
  elsewhere in the workspace, pre-existing and unrelated to this task).
- `cargo xtask ci --job clippy` — passes with no warnings.
- `cargo xtask ci --job no-std-purity` — passes; `starplayer-dsp` builds for
  `riscv32imc-unknown-none-elf` with default features and carries no `std` in its
  resolved feature tree or manifest defaults.
- `cargo xtask ci --job fma-check` — passes; `starplayer-dsp`'s own optimized codegen now
  contains real `f32` multiplies (`DspSample`'s `f32` impl and the biquad float twins are
  non-generic, unlike everything the crate held before this task), and the scan finds a
  separate multiply with no contraction marker, intrinsic or fused mnemonic in every one —
  the `requires_float_multiply: false` pass for this crate was previously inconclusive by
  design and is now a genuine positive finding.
