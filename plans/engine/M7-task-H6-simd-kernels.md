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

## Research resolution

### 1. `wide` on bare metal and on wasm without simd128 — does the scalar fallback compile clean under `-D warnings`?

**Yes, on both, with nothing needed.** No `compile_error!` was required anywhere: the
`simd` feature compiles for every target this workspace builds for, and `wide` is a real
dependency rather than a conditional one.

`wide 1.7.0` (with `safe_arch 1.2.0` and `bytemuck 1.25.2`) was checked with the feature on
for `starplayer-{dsp,mixer,engine}` and the facade, on both fallback targets, with
`RUSTFLAGS="-C llvm-args=-fp-contract=off -D warnings"`:

```
riscv32imc-unknown-none-elf   4 crates, no output
wasm32-unknown-unknown        4 crates, no output
```

`safe_arch` is not even compiled on those two: `wide` selects it by `cfg(target_feature)`,
so on a target with no vector unit the vector types are plain arrays and the arithmetic is
the loop the scalar body would have written. That is why `riscv32imc` — which has no `A`
extension, let alone a `V` one — needs no special case at all.

`wasm32-unknown-unknown` **with** `-C target-feature=+simd128` builds too, and
`cargo xtask ci --job wasm-build` now does it as a second pass over `starplayer` and
`starplayer-host-wasm` (which gained a `simd` feature for exactly that). `RUSTFLAGS`
*replaces* `[build] rustflags` rather than merging with it, so that pass restates
`-C llvm-args=-fp-contract=off`; dropping it silently is what
`assert_rustflags_keep_the_fp_contract_policy` exists to prevent, and it would have been
dropped here.

The version is **`wide = "1.7"`, not the `0.7` the task file guessed.** 0.7 is long
superseded; 1.7.0 is the current release and is the one that has `i32x4::saturating_add`
and `i32x4::widening_mul`, both of which this task's answers depend on.

### 2. Is `wide`'s `i32x4` multiply exact on SSE2, which lacks `pmulld`?

**Yes — it emulates, and the emulation is exact.** `simd_equivalence.rs`'s
`the_vector_integer_multiply_is_exact` asserts it over 10,000 random lane quadruples:
`i32x4 * i32x4` is `i32::wrapping_mul` per lane, and `i32x4::widening_mul` is the full
`i64` product. Both hold.

It does not matter in the end, because **no shipped kernel relies on it**, and the reason
is the more interesting half of this research point:

`wide` can widen (`i32x4::widening_mul` → `i64x4`) but has **no narrowing conversion back**
— there is no `i32x4::from(i64x4)`, and `i64x4`'s only exit is `to_array`. Every fixed-path
primitive in this engine except addition is a widening multiply followed by
`round_shift_nearest` and a clamp back into `i32` (`DspSample::mul_q24`,
`DspSample::scale_q15`, the master volume, the fixed sinc dot product's `i64`
accumulator). An exact vector form of any of them therefore has to leave the vector through
memory once per multiply, which is strictly worse than the scalar body it would replace.

So the fixed path stays scalar, deliberately, with **one exception**:
`i32x4::saturating_add` *is* `i32::saturating_add` per lane, and that is the whole of
`FixedPath::add_frame`, so the fixed bus summation is vectorised. This is recorded in
`starplayer_dsp::simd`'s module documentation and in architecture §7.1, and the equivalence
tests still run the fixed kernels — where the two bodies are literally the same function —
so that the day one of them acquires a vector body it is compared too.

A future move to `core::simd` would change the answer: `simd_cast` narrows, and the whole
fixed path would become reachable.

### 3. Does the FMA audit's assembly scan recognise vector `vfmadd` forms?

**It already did; no mnemonic was added.** FMA3 spells the scalar and the packed form with
the same stem — `vfmadd213ss` against `vfmadd213ps` — so `assembly.contains("vfmadd")`
matches both, and the same holds for `vfmsub`, `vfnmadd` and `vfnmsub`. The
`saw_assembly_float_multiply` probe is likewise fine: `mulss`/`mulps` are substrings of
their VEX-encoded `vmulss`/`vmulps` forms, which is what a `+fma` build actually emits.

What was missing was not a mnemonic but a **pass**. `FmaPass` gained a `features` field and
two passes were added — `starplayer-mixer` and `starplayer-dsp`, each with `simd` on —
because the `wide_*` bodies are non-generic and codegen in the crate's own rlib, so a
contracted vector multiply-add there would be exactly as much of an architecture §7.3
violation as a scalar one. Both report clean:

```
starplayer-mixer +simd: separate multiply, no contraction marker, intrinsic or fused mnemonic
starplayer-dsp +simd:   separate multiply, no contraction marker, intrinsic or fused mnemonic
```

A side effect worth recording: the **plain** `starplayer-dsp` pass is conclusive for the
first time. Its comment used to say the crate had no float codegen of its own — every float
expression sat in a generic or trait-impl method — and that the pass ran anyway so the
first non-generic helper would be covered from the commit that added it. This is that
commit: `simd::scalar_sinc_dot_f32` is non-generic, the rlib now contains `mulss`, and
`requires_float_multiply` is `true` for that pass.

## What was vectorised, and what was left scalar

| Body | Float path | Fixed path |
|---|---|---|
| Bus summation (`MixPath::add_block`) | `f32x4`, two frames per vector | `i32x4::saturating_add`, two frames per vector |
| Master volume | `f32x4` | **scalar** — widening multiply, no narrowing conversion |
| Master limiter | **scalar** — a table lookup, not arithmetic | **scalar**, same reason |
| Reverb comb bank (8 combs) | `f32x8` | **scalar** — `mul_q24` per lane |
| Reverb comb *sum* | **scalar, in comb order** — float addition is not associative | n/a (integer addition is, but one body serves both) |
| Reverb allpasses | **scalar** — four in series | **scalar** |
| EQ stereo biquad | `f32x4`, two lanes live | **scalar** |
| EQ bands | **scalar** — three in series | **scalar** |
| Fractional delay taps (delay, chorus, reverb pre-delay) | `f32x4` | **scalar** |
| Windowed sinc dot product | `f32x4` × 2, fixed-order pairwise reduce | **scalar** — `i64` accumulator |
| Steady unfiltered `mix_run` | **not landed** — see below | **not landed** |

Three shapes recur in the "left scalar" column, and each is a rule rather than a case:

* **A series chain cannot be a lane.** The reverb's four allpasses and the equaliser's three
  bands each feed the next; only the things *beside* them parallelise (eight combs, two
  channels).
* **A gather is not arithmetic.** Eight comb lines with eight lengths and eight cursors, a
  fractional tap at an arbitrary distance in a ring, the limiter's 257-entry curve — the
  loads stay scalar on every backend `wide` has, and the kernels take the gathered values as
  arrays rather than pretending otherwise.
* **The fixed path narrows.** Research point 2 above.

And one rule governs what a vector body may *reorder*: **nothing the scalar body's float
addition would notice.** The reverb's comb outputs are still summed in comb order. The sinc
dot product reduces in a fixed binary tree, `((l0+l1)+(l2+l3))+((l4+l5)+(l6+l7))`, in both
bodies — the scalar body was changed to that order in the same commit that added the vector
one, with the feature off everywhere, and all eleven goldens were unmoved, as deliverable 2
requires. (They could not have moved: the sinc *fixed* body, which is what the
`reflex__i16_mono_44100_sinc` golden hashes, was not touched. What the reordering does move
is the float path's last bits, which nothing pins and which the perceptual nightly
tolerates — the same class of change M7 decision 1 accepted for the bus summation.)

`wide` has a `reduce_add`, and it is deliberately not used: its association order is
unspecified, and an unspecified order is exactly what a bit-identity gate may not contain.

## Deliverable 4: the numbers

`cargo run -p starplayer-offline --release --example bench_render [--features simd] [-- --no-inserts]`,
x86-64 (WSL2, `--release`, `lto = "thin"`), 44.1 kHz, 128-frame host blocks. Each figure is
the median of three runs; each run times a one-second and a sixty-one-second render and
reports the difference, so the song scan and the engine's allocation cancel. Frames per
second, higher is better.

**With a reverb on channel 0, an equaliser on channel 1 and a compressor on the master bus** —
the graph M7's exit criterion describes:

| Case | scalar | `simd` | change |
|---|---:|---:|---:|
| `reflex.s3m`, float | 15.98 M | 17.51 M | **+9.6 %** |
| `reflex.s3m`, fixed | 6.93 M | 6.87 M | −0.9 % |
| `Fight2.it`, float | 3.61 M | 3.69 M | **+2.2 %** |
| `Fight2.it`, fixed | 1.507 M | 1.485 M | −1.5 % |

**With no inserts at all** (`--no-inserts`), which leaves only the voice path, the bus
summation and the master bus:

| Case | scalar | `simd` | change |
|---|---:|---:|---:|
| `reflex.s3m`, float | 110.0 M | 106.0 M | −3.6 % |
| `reflex.s3m`, fixed | 70.5 M | 69.9 M | −0.9 % |
| `Fight2.it`, float | 4.51 M | 4.45 M | −1.3 % |
| `Fight2.it`, fixed | 2.03 M | 1.97 M | −3.0 % |

Read together, the two tables say something more useful than either alone:

* **The effect kernels are the win.** `reflex.s3m` spends about 85 % of an insert-heavy
  render inside the DSP graph (0.024 s of the 0.166 s is everything else), and that is where
  the +9.6 % comes from — the reverb's comb bank, which is eight `f32x8` lanes of real work
  per frame per channel.
* **`Fight2.it` is voice-bound**, not effect-bound: the graph is only about 19 % of its
  render (0.594 s of 0.725 s is the voice path). A +2.2 % there is the same kernels doing
  the same work against a much larger denominator.
* **The bus summation and the master volume are a small net loss** — 1 to 3.6 % on an
  insert-free render — because `scalar_add_block_*` and `scalar_master_volume_f32` are plain
  elementwise loops over a slice, which LLVM handles at least as well as a hand-written
  four-lane body that has to build its vectors field by field and write them back through
  `to_array`. They are kept because deliverable 2 asks for them and because they are the
  only fixed-path kernel there is; if that 1–3 % ever matters, the honest fix is to delete
  the two `wide_*` bodies rather than to tune them.

Making `Stereo<T>` `#[repr(C)]` was worth about 5 % of the insert-free float render on its
own and is why that loss is 3.6 % rather than 9 %: without it the compiler may reorder
`left` and `right`, and the four loads a two-frame vector needs stay four loads.

aarch64 and wasm were **not** measured: this worktree has no ARM host and no browser
harness, and a wasm number from `wasmtime` would measure the runtime rather than the
kernels. Both are open.

## Deliverable 6: the steady unfiltered `mix_run` — prototyped, measured, not landed

It was written and measured, in a throwaway crate outside the repository (a 128-frame
destination, a 70,000-frame source, a non-integer step, one steady gain pair, forward,
unfiltered, `Linear`, float):

```
bit-identical: true
scalar:  727,661,716 frames/s
wide:  1,200,782,037 frames/s      (1.65x)
```

So it does win, and it does win **without changing a bit**: four frames' source pairs and
fractions are gathered scalar, the interpolation is one `f32x4` multiply-add, and the
accumulate is one more against `[left, right, left, right]`. Every lane operation is the
scalar body's operation on the scalar body's operands.

It is not landed anyway, for two reasons that are about design rather than about speed:

1. **The seam it needs is architectural.** `mix_run` is generic over `Path: MixPath` *and*
   `Interp: Interpolate`, and the vector body is valid only for `FloatPath` **and**
   `Linear`. Reaching it needs two new trait seams — a four-at-a-time `Interpolate` method
   and a four-at-a-time `MixPath::accumulate` — or the whole run loop moved into `MixPath`.
   That is a change to two of the engine's load-bearing traits, and design goal 8 and the
   architecture's own §10.1 both say a trait boundary is a decision with a plan behind it,
   not a bolt-on inside an optimisation task.
2. **The end-to-end share is modest.** At 728 M frames/s per voice, a thirty-voice module
   spends on the order of 15 % of its render inside `mix_run`; 1.65× of that is about 6 %
   of a voice-bound render such as `Fight2.it`, and near nothing on `reflex.s3m`, which is
   already 110 M frames/s with the graph switched off.

Recommended as its own task under M8 or later, with the trait change stated up front. The
prototype's numbers above are the case for it.

## Done differently from the task file

* **`wide = "1.7"`, not `"0.7"`** — see research point 1.
* **`crates/starplayer-mixer/tests/simd_equivalence.rs` was added as well.** The task file
  names only the dsp one, but the bus summation and the master volume live in the mixer and
  an integration test cannot reach across a crate boundary. Same rules, same shape.
* **`MixPath::master` was split into a volume pass and a limiter pass** rather than the
  volume being vectorised in place. `process_float`/`process_fixed` compute both on one
  frame, which is not a shape a vector body can take; two passes over the quantum are
  bit-identical (an `f32` store is exact, and a Q0.16 volume is below unity so the fixed
  path's `i64` product always narrows back into `i32` losslessly — asserted by
  `the_fixed_master_volume_never_leaves_i32`).
* **`DelayLine::read_fractional_parts` is new.** The tap kernel takes gathered values, so
  the gather needed a name; `read_fractional` is now that call plus the scalar kernel.
* **`attenuate_q24` moved** from `effects::reverb` to `simd`, because the comb kernel is its
  other caller and the two have to be the same three lines.
* **`Stereo<T>` became `#[repr(C)]`** — see deliverable 4.
* **`Reverb`'s comb damping state moved from `Comb` to `Tank`**, as one `[Sample; 8]`, so the
  bank can be handed to the kernel as an array. The eight *lines* stay separate.
* **`starplayer-host-wasm` gained a `simd` feature**, because the wasm build job's second
  pass has to have something to turn on. The packaged web player is **not** built with it;
  that stays H7's build, and this task's "out of scope" says so.
* **`cargo xtask ci --job simd` also runs clippy** over the two crates with the feature on.
  The workspace clippy job runs with default features, so without this the vector bodies and
  the equivalence tests would be the only code in the tree clippy never lints. It found four
  inconsistently grouped hex literals on its first run.

## Verification

Run on x86-64 (WSL2), pinned toolchain 1.97.1, at `f98b3ee`:

```
cargo test -p starplayer-dsp --features simd                                   ok (193 + 8 + …)
cargo test -p starplayer-engine --features simd --test block_size_determinism  ok (20)
cargo xtask goldens --check                                                    ok (11/11)
cargo xtask goldens --check --simd                                             ok (11/11, identical)
cargo xtask ci --job simd                                                      passed
cargo xtask ci --job no-std-purity                                             passed
cargo xtask ci --job wasm-build                                                passed
cargo xtask ci --job fma-check                                                 passed
cargo xtask ci --job clippy                                                    passed
cargo test --workspace                                                         ok
```
