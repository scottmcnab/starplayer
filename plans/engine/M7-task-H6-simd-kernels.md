# M7 — H6: SIMD kernels behind the `simd` feature, with the scalar-equivalence gate

| Field | Value |
|---|---|
| Milestone | M7 ([master plan](M7-master-plan.md), "The task graph" section) |
| Status | Ready once H3, H4 and H5 have landed |
| Depends on | H3, H4 (the effects to vectorise), H5 (the kernels to vectorise), H1 (the bus summation) |
| Blocks | M7 exit ("SIMD and scalar paths agree") |
| Parallel with | H7 |
| Recommended model | Claude Opus (bit-exact equivalence across three ISAs; a dependency decision with `no_std` and `forbid(unsafe_code)` constraints) |
| Verified by | agent (`cargo xtask goldens --check` with `--features simd`, property tests, `no-std-purity` with the feature on, wasm build with simd128), then reviewer |

## Context for a fresh agent

Architecture §7.1 says SIMD is "an optimisation *inside* the monomorphised loop, never a
semantic change", gated by a scalar-equivalence test, and the master plan says `core::simd`.
**`core::simd` is nightly-only and the toolchain is pinned to stable 1.97**
(`rust-toolchain.toml`), so master-plan decision 6 substitutes the `wide` crate: `no_std`,
safe code only (the dsp and mixer crates are `#![forbid(unsafe_code)]`), SSE2/NEON/simd128
backends with a scalar fallback on `riscv32imc-unknown-none-elf`. The `simd` feature already
exists as an empty flag on `starplayer-dsp`, `starplayer-mixer`, `starplayer-engine` and the
facade; this task gives it a body.

What "equivalence" means here is **bit-identical output**, not "close": the fixed path is the
cross-target golden reference and the float path's block-size determinism is a byte comparison.
That constrains what may be vectorised — only computations whose per-lane operations are the
same IEEE-754 or integer operations in the same order as the scalar body. Horizontal sums that
re-associate float additions are out; per-lane biquads, parallel comb banks, elementwise bus
summation and gain application are in.

### Code you must read before changing anything

- `crates/starplayer-dsp/src/effects/{reverb,eq,chorus,delay,compressor}.rs` and
  `biquad.rs`, `delay_line.rs`; the H3/H4 resolutions on scaling.
- `crates/starplayer-mixer/src/{kernel,path,master,voice}.rs`; `MixPath::add_frame` from H1;
  the bus loop in `crates/starplayer-engine/src/engine.rs`.
- `crates/starplayer-dsp/src/interpolate.rs` — the sinc dot product from H5.
- `xtask/src/main.rs` — `run_goldens`, `job_no_std_purity`, `job_wasm_build`, `fma_findings`.
- `.github/workflows/ci.yml`; `.cargo/config.toml` (`-fp-contract=off` must also hold for the
  vector code — the FMA audit reads the assembly, so it will tell you).
- `plans/product/01-technical-architecture.md` §7.1, §7.3, §10.

## Deliverables

### 1. The dependency and the feature

`wide = { version = "0.7", default-features = false }` as an optional workspace dependency,
enabled by `starplayer-dsp/simd` (and through it the mixer's and engine's `simd`). Confirm
`wide` builds for `riscv32imc-unknown-none-elf`, `wasm32-unknown-unknown` (with and without
`-C target-feature=+simd128`) and `thumbv6m` is not required. If `wide` cannot be made to
build for the bare-metal target, the `simd` feature must be refused there by a `compile_error!`
with a message, and the resolution must say so.

### 2. The vectorised bodies

Each lives beside its scalar body in the same module, selected by `#[cfg(feature = "simd")]`,
with the scalar body **always compiled** (as `pub(crate) fn scalar_*`) so the equivalence test
can call both in one build:

1. **Bus summation and master volume** (`engine.rs` bus loop via `MixPath::add_block`, and
   `master.rs`'s volume multiply): `f32x4`/`i32x4` elementwise. The limiter's table lookup
   stays scalar.
2. **Reverb comb bank**: the eight combs per channel as two `f32x4`/`i32x4` lanes — the
   canonical SIMD-friendly structure in Freeverb; the four allpasses stay scalar (they are
   in series).
3. **EQ**: left and right biquads as a 2-lane pair; three bands stay in series.
4. **Chorus and delay**: the fractional-read interpolation of the taps as lanes.
5. **Sinc interpolation**: the 8-tap dot product as `i32x8`/`f32x8` multiply then a
   **fixed-order** lane sum (`((l0+l1)+(l2+l3))+((l4+l5)+(l6+l7))`, and the scalar body must
   sum in exactly that order — change the scalar body to match if it does not, *before* the
   feature is on, and prove with the sinc golden from H5 that the change moved nothing).
6. **The steady unfiltered `mix_run`**: optional. Four consecutive output frames need a gather
   of source frames at `position + k·step`; try it, measure, and keep it only if it wins on
   x86-64 without changing a bit. Report the numbers either way.

### 3. The scalar-equivalence gate

- `crates/starplayer-dsp/tests/simd_equivalence.rs` (feature-gated): for every vectorised body,
  random inputs (the core `Xorshift32`) × 1000 blocks, both paths, `assert_eq!` on bit patterns.
- `cargo xtask goldens --check --simd` builds `starplayer-offline` with the feature on and
  checks every golden, including H5's cubic/sinc pins. Add the flag to `run_goldens`.
- CI: a new `simd` job in `ci.yml` and `xtask ci --job simd` that runs the equivalence tests,
  the goldens with `--simd`, the block-size determinism test with the feature, and
  `cargo check --target riscv32imc-unknown-none-elf --features simd` for the `no_std` crates.
  The wasm build job gains a second build with `RUSTFLAGS=-C target-feature=+simd128` and
  the feature; the web page is **not** switched to it in this task (the page's build is H7's).
- `cargo xtask ci --job fma-check` with the feature on for `starplayer-dsp` and
  `starplayer-mixer` (extend `FmaPass` with a features field).

### 4. Measurement

A `criterion`-free benchmark: `cargo run -p starplayer-offline --release --example bench_render`
(add it) renders `reflex.s3m` and one dense `.it` for 60 s of audio with a reverb, an EQ and a
compressor installed, scalar vs `--features simd`, and prints frames per second. Record x86-64
numbers in the resolution; aarch64 and wasm if you can run them.

### 5. Documentation

Architecture §7.1: replace "via `core::simd`" with what landed and why; §7.3 unchanged. The
`simd` feature's doc line in every `Cargo.toml` that has it. Append `## Research resolution`.

## Research points

1. `wide` on bare metal and on wasm without simd128 — does the scalar fallback compile clean
   under `-D warnings`?
2. Whether `wide`'s `i32x4` multiply is exact on SSE2 (which lacks `pmulld`) — it emulates;
   the equivalence test decides, but know before you rely on it.
3. Whether the FMA audit's assembly scan recognises vector `vfmadd` forms; extend the
   mnemonic list if not.

## Verification

```
cargo test -p starplayer-dsp --features simd
cargo test -p starplayer-engine --features simd --test block_size_determinism
cargo xtask goldens --check
cargo xtask goldens --check --simd
cargo xtask ci --job simd
cargo xtask ci --job no-std-purity
cargo xtask ci --job wasm-build
cargo xtask ci --job fma-check
cargo xtask ci --job clippy
cargo test --workspace
```

## Out of scope

Enabling `simd` by default anywhere; runtime CPU dispatch; changing any scalar result;
the web page's build flags (H7).
