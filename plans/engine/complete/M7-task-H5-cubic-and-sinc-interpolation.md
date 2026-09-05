# M7 — H5: Cubic Hermite and windowed-sinc interpolation, and the sample pre-roll

| Field | Value |
|---|---|
| Milestone | M7 ([master plan](M7-master-plan.md), "The task graph" section) |
| Status | Landed 2026-09-05 |
| Depends on | — |
| Blocks | H6 (the SIMD pass vectorises whatever kernels exist), H7 (the web page's interpolator select) |
| Parallel with | H1, H2 |
| Recommended model | Claude Opus (the PCM blob layout is a contract between three crates; the goldens are the gate) |
| Verified by | agent (linear goldens byte-identical, new cubic/sinc goldens committed, block-size determinism on all four kernels, cross-target hash on wasm32), then reviewer |

## Context for a fresh agent

The mixer resamples through `starplayer_dsp::Interpolate` (`crates/starplayer-dsp/src/interpolate.rs`):
`Nearest` and `Linear` exist, and `MixerMode`/`Interpolator` already name `Cubic` and `Sinc`
(`crates/starplayer-core/src/event.rs`, `crates/starplayer-engine/src/mixer_mode.rs`), which
hosts refuse today (`crates/starplayer-host/tests/player.rs`,
`an_arm_the_host_cannot_build_is_refused_rather_than_guessed`). Architecture §7.1 wants cubic
for quality real-time and windowed sinc for offline rendering; golden filenames encode the
kernel (`goldens/<format>/<stem>__i16_mono_44100_<kernel>.sha256`, see
`starplayer_offline::golden_filename_for_interpolator`), so a new kernel is a new golden, never
a moved one.

Every sample in the PCM blob carries **eight trailing guard frames**
(`starplayer_core::sample::GUARD_FRAMES`, written by `ModuleBuilder::add_sample` in
`crates/starplayer-model/src/builder.rs` and by `append_guarded_sample` in
`crates/starplayer-mixer/src/sample.rs`, read by `SampleData::resolve`). A symmetric kernel also
reads **before** the current frame — `index − 1` for Hermite, `index − 3` for an 8-tap sinc —
and `sample.rs`'s doc comment says that pre-roll "belongs to M7 along with the kernels that
need them". This task adds it.

### Code you must read before changing anything

- `crates/starplayer-core/src/sample.rs` (the whole file — it is the contract).
- `crates/starplayer-model/src/builder.rs` `add_sample`; `crates/starplayer-model/src/module.rs`
  `validate` (the "stored ≥ …" rule); every loader's use of `add_sample` (`grep -rn add_sample crates`).
- `crates/starplayer-mixer/src/sample.rs` — `SampleRegion`, `SampleData::resolve`,
  `append_guarded_sample`, `folded_frame`; `crates/starplayer-mixer/src/kernel.rs` — `mix_run`,
  `cross_boundary`, `run_limit`, `frames_before_limit`, and the tests
  `linear_interpolation_reads_through_the_loop_point_without_a_branch`,
  `splitting_a_run_anywhere_produces_the_same_frames`.
- `crates/starplayer-dsp/src/interpolate.rs`, `crates/starplayer-mixer/src/path.rs`
  (`MixPath::interpolate`).
- `crates/starplayer-offline/src/lib.rs` — `render_with_kernel`, `GOLDEN_INTERPOLATOR`,
  `RenderError::UnimplementedInterpolator`; `xtask/src/main.rs` `run_goldens`.
- `crates/starplayer-host/src/engine.rs` `define_arms!`; `crates/starplayer-host-wasm/src/lib.rs`
  (its arm table) and `apps/starplayer-web/www/{index.html,app.js}` (`#mixer-interpolator`).
- `crates/starplayer-engine/src/scope.rs` — the tap reads PCM by position; it must keep
  reading the same frames after the layout change.
- `fuzz/` — the structured targets build modules; nothing there hard-codes the layout, but check.

## Deliverables

### 1. The pre-roll (`starplayer-core`, `starplayer-model`, `starplayer-mixer`)

`PRE_ROLL_FRAMES: usize = 8` beside `GUARD_FRAMES`. Every sample's stored run becomes
`pre-roll ‖ frames ‖ guard`; `SampleRegion::pcm_offset` keeps pointing at frame 0 (so every
existing offset arithmetic, the scope tap and `folded_frame` are unchanged) and
`SampleData::resolve` checks that `pcm_offset ≥ PRE_ROLL_FRAMES` and that the eight frames
before it are addressable. Contents, mirroring the trailing guard:

- **Before frame 0**: silence, for every loop mode — a note starts from nothing.
- **Reads that cross `loop_start` backwards** (a forward loop after its first pass, when the
  position is within three frames of `loop_start`) must see the frames before `loop_end`.
  That needs either a second copy of those frames or a branch. Research point 1 decides;
  the expected answer is run-splitting: `Interpolate` gains `LEADING_FRAMES` alongside
  `GUARD_FRAMES_REQUIRED`, `run_limit`/`frames_before_limit` stop a run that many frames
  before a wrap for the wide kernels, and the wrap frames are rendered through per-frame
  `folded_frame`-style reads exactly as the ping-pong turn is today. Whichever you choose,
  `Linear` and `Nearest` must execute the code they execute today (`LEADING_FRAMES = 0`).
- one-shot, ping-pong: silence before frame 0.

`ModuleBuilder::add_sample` and `append_guarded_sample` write it; `Module::validate` accounts
for it in the stored-length rule; `SampleRegion::stored_frames` includes it.

### 2. `Cubic` (4-tap Hermite) and `Sinc` (8-tap windowed sinc) in `starplayer-dsp`

- `Cubic`: Catmull-Rom Hermite over `x[-1], x[0], x[1], x[2]`; `f32` twin straightforward; the
  fixed twin evaluates the cubic in `i64` with the fraction in Q15 and one `round_shift_nearest`
  at the end. `GUARD_FRAMES_REQUIRED = 2`, `LEADING_FRAMES = 1`.
- `Sinc`: 8 taps, 256 phases, Blackman-Harris (or Kaiser β=8 — say which) windowed sinc with
  cutoff at 0.9 Nyquist, coefficients normalised per phase so DC is exactly unity on the
  fixed path (Q1.15 `i16` coefficients summing to 32768). The table is **generated source**:
  `crates/starplayer-dsp/src/sinc_table.rs` committed, with a `std`-only test that regenerates
  it in `f64` and asserts equality, and the generator kept as `#[cfg(test)]` code in the same
  file. `GUARD_FRAMES_REQUIRED = 4`, `LEADING_FRAMES = 3`. No `libm` at run time; the table
  is data.
- `Interpolate` grows `const LEADING_FRAMES: usize` (0 for the existing kernels).

### 3. Hosts and offline

- `render_with_kernel` arms for `Cubic` and `Sinc`; `RenderError::UnimplementedInterpolator`
  keeps existing for a build with kernels featured out, if any.
- `define_arms!` in `starplayer-host` and the wasm host's table get the eight new arms
  (2 paths × 2 kernels × mono/stereo). The web page's `#mixer-interpolator` select lists
  `cubic` and `sinc`; the `player.rs` refusal test moves to a mode that is still unbuildable
  (e.g. an invalid channel count only).
- `cargo xtask goldens` renders **one** fixture per new kernel — `reflex.s3m` at cubic and at
  sinc — into `goldens/s3m/reflex__i16_mono_44100_{cubic,sinc}.sha256`, committed; the linear
  goldens are untouched and `--check` proves it.

### 4. Proof

- `cargo xtask goldens --check` byte-identical on every existing golden.
- `crates/starplayer-engine/tests/block_size_determinism.rs` and
  `crates/starplayer-mod/tests/render_determinism.rs` run the scenario on `Cubic` and `Sinc`
  as well as the existing kernels (extend the type-parameter loops).
- A kernel test: a looped sample rendered through the loop point with `Sinc` has no
  discontinuity larger than the kernel's ripple (compare against rendering the loop unrolled
  into a long one-shot); the same for `Cubic`.
- The fixed sinc render of `reflex.s3m` hashes identically on `wasm32-wasip1` (the toolchain
  has the target; `cargo test -p starplayer-offline --target wasm32-wasip1` needs a runner —
  if none is installed, say so and rely on the `no-std-purity` build plus the const table's
  target-independence argument).
- `cargo xtask ci --job fma-check` (the sinc dot product must not fuse).

### 5. Documentation

Architecture §7.1 table: mark cubic and sinc landed, state the tap counts, the pre-roll and
where the leading reads come from. `sample.rs` doc comment updated. Accuracy policy §5 item 5:
the goldens are still linear; the extra cubic/sinc goldens are cross-target pins, not a new
canonical. Append `## Research resolution` here.

## Research points

1. **Leading reads across a loop start** — second copy vs. run-splitting (deliverable 1).
   Measure: the run-splitting approach costs three per-frame reads per wrap; the copy costs
   8 frames per sample and a rule in `validate`. Prefer the one that keeps `mix_run` free of a
   new branch for linear.
2. **Sinc cutoff and window**: 0.9 vs 0.95 Nyquist; 8 taps at 256 phases is the M2-C6
   assumption (`GUARD_FRAMES` was sized for it). Pick by measuring alias rejection on a swept
   sine at 2× and 0.5× rate; record the numbers.
3. **Fixed-point Hermite rounding**: single final rounding vs. per-term; the golden for cubic
   pins whichever you pick, so pick the one that is closest to the float twin and say the
   measured maximum difference.

## Verification

```
cargo test --workspace
cargo xtask goldens --check
cargo xtask goldens            # writes the two new files only; re-run --check
cargo test -p starplayer-engine --test block_size_determinism
cargo xtask ci --job host-tests
cargo xtask ci --job clippy
cargo xtask ci --job no-std-purity
cargo xtask ci --job wasm-build
cargo xtask ci --job fma-check
cargo xtask ci --job fuzz-smoke
```

## Out of scope

16-tap sinc; oversampling; changing the default kernel (linear stays the canonical golden);
SIMD (H6); anything in the DSP graph (H1–H4).

## Research resolution

Landed on branch `h5`, 2026-09-05.

### 1. Leading reads across a loop start — neither a copy nor a branch: **defer the wrap**

The task file expected run-splitting. There is a third option, and it is better than both
the ones the task weighed, so it is what landed.

A forward-looping sample's trailing guard **already holds the loop's continuation**
(`GUARD_FRAMES` frames of it, wrapping as often as a short loop needs). So instead of
wrapping the position at `loop_end`, the kernel wraps it at `loop_end + LEADING_FRAMES`:

```
run_limit(forward loop)  = loop_end + Interp::LEADING_FRAMES     (in Q32.32)
normalise_position wraps at the same point, through the existing rem_euclid
```

Positions in `[loop_end, loop_end + LEADING_FRAMES)` read guard frames, which are by
construction the frames `loop_start …` — so the *content* is right — while the taps
*behind* the interpolation point are the real frames just before `loop_end` — so the
leading reads are right too. The wrap then lands at `loop_start + LEADING_FRAMES` rather
than `loop_start`, and the three frames it skips are exactly the three it has just played
out of the guard. Nothing is skipped and nothing is repeated.

Measured against the alternatives:

| | cost in `mix_run` | cost per wrap | cost per sample | exactness |
|---|---|---|---|---|
| Second copy of the pre-`loop_end` frames | none | none | 8 frames + a `validate` rule | exact, but needs two different things at one address for `loop_start > 0` — it does not actually work |
| Run splitting, per-frame folded reads | none (a new run bound) | 3 per-frame gathers + a "just wrapped" flag the fold has no way to know | none | exact |
| **Deferred wrap** | **none** | **none** | **none** | **exact** |

"Exact" is not a claim here, it is a test:
`kernel::tests::a_wide_kernel_renders_a_loop_exactly_as_the_unrolled_loop_renders` renders
a loop whose run-in looks nothing like the frames before `loop_end`, renders the same loop
unrolled into a long one-shot, and asserts the two are **byte-identical** on cubic, on sinc
and on linear. A silent pre-roll alone fails it; so would taking the leading taps from
before `loop_start`.

The second copy was rejected on correctness, not cost. The frames just below `loop_start`
are the sample's run-in during the first pass and would have to be the pre-`loop_end`
frames afterwards; one address cannot hold both, so a copy only ever works for
`loop_start == 0`.

`Linear` and `Nearest` have `LEADING_FRAMES = 0`, so `forward_wrap_bits` returns `loop_end`
and they execute the arithmetic they always did. `cargo xtask goldens --check` is the proof
(all nine unchanged).

**Two cases the deferral does not cover**, both recorded here rather than fixed:

* **A ping-pong loop's bottom turn.** Travelling backwards through `loop_start`, the taps
  behind the interpolation point are the frames below `loop_start` — the run-in, or the
  silent pre-roll when `loop_start == 0` — rather than the mirrored continuation. The
  deferral cannot help: deferring the turn would need mirrored frames stored *below*
  `loop_start`, and those addresses hold the run-in. Filling the pre-roll with a mirror
  instead of silence would fix the `loop_start == 0` case and put a pre-echo in front of
  every note's attack, which is the worse trade — the task file's "silence for every loop
  mode" is right. The residual is a few frames of small-weight error per turn on XM/IT
  ping-pong samples under the wide kernels only.
* **A sample with a sustain loop.** Its stored run is the whole body with a *silent* guard,
  because neither loop's end sits at the stored length, so the guard is not a loop
  continuation. Deferring the wrap there plays up to `LEADING_FRAMES` frames of the body's
  tail per wrap. This is the same class of deviation `starplayer_model::SampleIndex`
  already documents for `Linear` (which reads one such frame at low weight), three frames
  wider, on the wide kernels only. Fixing it properly needs the region to carry whether its
  guard is a continuation — a `SampleRegion` field and a constructor change through IT and
  XM — which is more than this task should spend.

### 2. Sinc window and cutoff — **Kaiser β = 8 at 0.9 Nyquist**

Measured on swept sines through the actual Q1.15 table, up-sampling ×2 (step 0.5) and
down-sampling ×2 (step 2.0), with an exact-bin FFT so there is no leakage floor. "Worst
image" is the strongest non-signal bin relative to the fundamental.

| Window, cutoff | worst image, f ≤ 0.40 | image at 0.19 fs | image at 0.25 fs | \|H\| at 0.30 fs | \|H\| at 0.40 fs | down-sample spur floor |
|---|---|---|---|---|---|---|
| Blackman-Harris, 0.90 | −17.0 dB | −53.4 dB | −38.5 dB | −1.28 dB | −3.87 dB | −91.8 dB |
| Blackman-Harris, 0.95 | −14.9 dB | −47.2 dB | −34.0 dB | −0.92 dB | −3.03 dB | −97.9 dB |
| **Kaiser β=8, 0.90** | **−22.6 dB** | **−72.2 dB** | **−61.8 dB** | **−0.80 dB** | **−3.49 dB** | **−91.9 dB** |
| Kaiser β=8, 0.95 | −19.3 dB | −73.4 dB | −50.5 dB | −0.50 dB | −2.55 dB | −97.8 dB |
| Kaiser β=6, 0.90 | −29.5 dB | −60.1 dB | −65.0 dB | −0.51 dB | −3.19 dB | −90.8 dB |

**Window.** Kaiser β=8 beats Blackman-Harris everywhere that matters at this length: 5.6 dB
on the worst image and 19 dB in the middle of the band, with *less* passband droop at
0.30 fs. Blackman-Harris spends its −92 dB sidelobe budget on a mainlobe eight taps cannot
afford — truncation, not the window's own sidelobes, is what sets the floor here. β=6 has a
better worst case at 0.375 fs but is 12–20 dB worse over the low half of the band, where
the energy in tracker samples actually is; β=8 is the better average.

**Cutoff.** 0.95 buys 0.3–0.9 dB less droop above 0.30 fs and costs 3.3 dB of image
rejection at the worst point and 11.3 dB at 0.25 fs. Image rejection is what a sinc kernel
is *for*, so 0.9 stays, as the task file assumed.

**Phases.** 256, as M2-C6 sized `GUARD_FRAMES` for; the phase is truncated from the Q0.32
fraction rather than rounded, so it can never wrap into the next source frame. That
quantisation sets the spur floor at ≈ −92 dB, measured while down-sampling by two — well
below the 16-bit noise floor, so nothing is gained by interpolating between phases.

**Normalisation.** Each row is scaled so its Q1.15 coefficients sum to exactly 32768, with
the residual pushed into the row's largest tap. DC gain is therefore exactly unity on the
fixed path — `sinc_table::tests::every_phase_has_unit_dc_gain` and
`interpolate::tests::sinc_holds_a_constant_signal_exactly` — which is what keeps a long
loop from drifting in level. The largest coefficient is 29482 at phase 0 (≈ the 0.9
cutoff), comfortably inside `i16`.

### 3. Fixed-point Hermite rounding — **Q24 fraction, staged Horner, one output rounding**

The task file asked for "the fraction in Q15 and one `round_shift_nearest` at the end".
That combination **cannot be written**: with `|x| ≤ 2^15` the cubic's third Horner multiply
reaches `2^64.5` at full scale, 2.8× past `i64`. Q14 fits, with 2.9× headroom. So the real
choice was between a narrow fraction with one rounding and a wide fraction with rescalings
inside the chain, and research point 3 is exactly the question of which is closer to the
float twin. Measured over 2,000,000 random four-frame windows and fractions, plus
full-scale alternating patterns, against the exact rational value and against the `f32`
twin (all figures in `i16` steps):

| Variant | max \|− exact\| | max \|− `f32` twin\| | fits `i64`? |
|---|---|---|---|
| A: Q15 fraction, per-term rounding | 3.610 | 3.609 | yes |
| B: Q15 fraction, staged shifts | 3.496 | 3.500 | yes |
| C: Q14 fraction, single final rounding | 6.496 | 6.492 | yes |
| **D: Q24 fraction, staged shifts** | **0.506** | **0.508** | yes, ≤ 2^56 |
| (the `f32` twin's own distance from exact) | 0.009 | — | — |

The rounding *rule* barely matters (A vs B differ by 0.1 of a step); the fraction's **width**
is what dominates, because truncating a Q0.32 position to Q15 moves the interpolation point
by up to `2^-15` of a frame, which at full slope is several steps of output. D is at the
theoretical floor — 0.5 is the final rounding's own quantum — so it is what landed:

```
fraction = fraction_bits >> 8                          // Q24
accumulator  = jerk * fraction                          // Q24, ≤ 2^42
accumulator  = round(accumulator, 12) + (curve << 12)   // Q12, ≤ 2^32
accumulator *= fraction                                 // Q36, ≤ 2^56
accumulator  = round(accumulator, 24) + (slope << 12)   // Q12
accumulator *= fraction                                 // Q36
result       = current + round(accumulator, 37)         // the 37th bit is the spline's ½
```

The two interior `round_shift_nearest` calls exist only to keep the intermediates inside
`i64`; each discards under `2^-12` of a coefficient unit, which reaches the output as under
`10^-4` of a step. The result is one *meaningful* rounding at the end, which is the spirit
of what the task asked for. `interpolate::tests::cubic_fixed_and_float_agree_to_within_one_step`
pins the measured bound.

### Anything else done differently

* **`SampleData` carries two slices**, `frames()` (frame 0 onward, exactly what every
  existing caller and the scope tap wanted) and `stored()` (pre-roll onward, which the
  render kernel resamples through with the index biased by `PRE_ROLL_FRAMES`). The bias is
  loop-invariant and folds into the addressing mode. `SampleRegion::stored_frames` counts
  the whole footprint as the task asked; `readable_frames` was added for the frame-0 view
  that `Module::sample_pcm` and `validate` want.
* **`SampleData::resolve` is strict** about the pre-roll, as the task asked: a region whose
  `pcm_offset` is under `PRE_ROLL_FRAMES` does not resolve, and the voice finishes rather
  than reading outside its sample. Three voice-bookkeeping tests in `starplayer-engine` and
  `starplayer-it` construct regions at offset 0; none of them renders, so none needed
  changing.
* **The `wasm32` cross-target check was run and passed** — better than the task file's
  fallback expected. No WASI runner is installed, but `cargo build -p starplayer-offline
  --bin starplayer-goldens --target wasm32-wasip1 --release` plus a six-line `node:wasi`
  runner (`node run.mjs starplayer-goldens.wasm --print`) executes the driver, and **all
  eleven hashes — the nine linear ones and both new wide-kernel ones — match the committed
  x86-64 files exactly**. Worth turning into an `xtask ci` job; not done here.
* **`RenderError::UnimplementedInterpolator` is now unreachable** (every `Interpolator` has
  an arm) and was kept, with its doc rewritten to say why: a build that features a kernel
  out needs somewhere to say so that is not a panic.
* **`cargo xtask ci --job fuzz-smoke` found a pre-existing IT-loader finding on its first
  run**, unrelated to this task: a 37 KB input expands to a 23.6 MB *pattern* blob (4100
  patterns), whose `Vec` doubling to 45 MiB trips the fuzz harness's memory cap. The module
  it produces has **zero samples**, so the pre-roll contributes nothing to it. The artifact
  is not committed; a second run of the job passed with no artifacts. The IT loader appears
  to lack the aggregate decoded-pattern budget the S3M loader has — worth a task of its own.
