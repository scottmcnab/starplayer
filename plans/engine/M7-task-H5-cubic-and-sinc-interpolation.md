# M7 — H5: Cubic Hermite and windowed-sinc interpolation, and the sample pre-roll

| Field | Value |
|---|---|
| Milestone | M7 ([master plan](M7-master-plan.md), "The task graph" section) |
| Status | Ready |
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
