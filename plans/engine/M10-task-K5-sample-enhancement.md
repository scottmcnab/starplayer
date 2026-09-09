# M10 — K5: The sample-enhancement API

| Field | Value |
|---|---|
| Milestone | M10 ([master plan](M10-master-plan.md), "The task graph" section); composes with [M11](M11-master-plan.md) (enhance once at library-scan time) |
| Status | Superseded 2026-09-09 by [K5a](complete/M10-task-K5a-enhancer-core.md) + [K5b](complete/M10-task-K5b-enhance-offline-and-cli.md) + [W4](../apps/complete/W4-task-enhancement-checkboxes.md) — see "Plan amendment" |
| Depends on | — (M7-H5's sinc kernel and pre-roll are useful precedent, not a dependency) |
| Blocks | — |
| Parallel with | K1, K2 |
| Recommended model | Claude Sonnet (a load-time transform with two implementations and a deterministic contract; no RT code) |
| Verified by | agent (bit-identical output across runs and targets, loop continuity, the goldens untouched, an A/B render), then owner listening |

## Context for a fresh agent

Tracker samples are mostly 8-bit and often 8–16 kHz. The M10 master plan's "enhancement
API" is a **load-time** transform: a `SampleEnhancer` takes decoded PCM and returns
decoded PCM at a possibly higher rate, and the module builder applies it before the PCM is
committed to the blob. Consequences the plan spells out and this task must keep: the RT
path is untouched, the result is deterministic and hashable, an expensive enhancer is fine
because it never runs in the audio callback, and an enhancer may live in a `std` crate.

Master-plan decision 6 requires **two real implementations** so the trait is committed
honestly: a windowed-sinc upsampler and a loop-seam smoother. Goldens never see an
enhancer; an enhanced render is a different configuration with a different name.

### Code you must read before changing anything

- `crates/starplayer-model/src/{builder,sample,module}.rs` — `ModuleBuilder::add_sample`,
  `SampleSpec` (`reference_rate_hz`, loop fields, sustain loop), `validate`.
- `crates/starplayer-mixer/src/sample.rs` and `crates/starplayer-core/src/sample.rs` —
  the guard and pre-roll contract (an enhancer changes the frames *before* guards exist).
- `crates/starplayer-dsp/src/{interpolate,sinc_table}.rs` (H5) — the sinc kernel you may
  reuse for the upsampler's design (but at higher quality: this runs offline).
- Every loader's call to `add_sample` (`grep -rn add_sample crates`) — the hook point is
  the builder, so no loader changes.
- `crates/starplayer-offline/src/lib.rs` — `render_song`, `golden_filename_for_interpolator`
  (the filename-encodes-configuration rule); `apps/starplayer-cli` `render`.
- `plans/product/03-accuracy-policy.md` §5 item 5.

## Deliverables

### 1. The trait, in `starplayer-model` (no_std)

```rust
pub struct SamplePcm<'a> { pub frames: &'a [i16], pub rate_hz: u32, pub loop_span: Option<(u32, u32, LoopMode)>, pub sustain_loop: Option<(u32, u32, LoopMode)> }
pub struct EnhancedPcm { pub frames: Vec<i16>, pub rate_hz: u32, pub loop_span: Option<(u32, u32, LoopMode)>, pub sustain_loop: Option<(u32, u32, LoopMode)> }
pub trait SampleEnhancer {
    fn name(&self) -> &'static str;          // goes into the configuration name
    fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm;
}
```

`ModuleBuilder::with_enhancer(&mut self, enhancer: &dyn SampleEnhancer)` (set once before
samples are added); `add_sample` runs it, then rescales `reference_rate_hz` and the loop
points by the rate ratio it returns (the enhancer returns the *new* loop points, so a
resampler is responsible for keeping a loop's length integral), then proceeds exactly as
today. An enhancer that returns the input unchanged is the identity, and a builder with no
enhancer must produce byte-identical blobs to today's — the eleven goldens prove it.

### 2. `crates/starplayer-enhance` (std; the implementations)

- **`SincUpsampler { factor: 2 | 4, taps: 64 }`**: polyphase windowed-sinc (Kaiser β=10)
  upsampling in `f64`, rounded to `i16` with TPDF-free plain rounding (deterministic:
  no RNG). **Loops are resampled as periodic signals**: the loop region is treated as
  circular so the seam has no transient, and the output loop length is exactly
  `factor × input length`; the pre-loop run-in is resampled linearly. Sustain loops
  likewise. One-shots get a symmetric tail. Determinism: `f64` arithmetic with the
  same operation order on every target and `-fp-contract=off` already set — assert the
  hash of an enhanced fixture in a test.
- **`LoopSmoother { crossfade_frames }`**: crossfades the last `n` frames of a forward loop
  with the frames before its start so the seam is click-free — the classic tracker
  "loop crossfade", applied at load. Integer arithmetic (`i32` with rounding). Also fixes
  the DC step of loops that end on a different level than they start, by an equal-power
  blend. No rate change.
- **`Chain(Vec<Box<dyn SampleEnhancer>>)`** so both can be applied in order; its `name`
  joins the parts with `+`.

### 3. Hosts and goldens

- `starplayer render --enhance sinc4x[+loop]` and `play --enhance …`; `info --enhance`
  prints the resulting rates and loop points. `--golden` refuses `--enhance`.
- `render_song` gains an optional enhancer (through a small `RenderOptions` if H7 did not
  already add one); the WAV writer is unchanged.
- `golden_filename` grows a configuration segment only when an enhancer is set
  (`stem__i16_mono_44100_linear_enh-sinc4x.sha256`); no such golden is committed in K5,
  but the naming rule is tested so a later pin is visibly a new file.

### 4. Proof

- The builder with no enhancer: `cargo xtask goldens --check` unchanged.
- `SincUpsampler`: a 1 kHz sine at 8 kHz → 32 kHz has its image at 7 kHz below −80 dB
  and the fundamental within 0.05 dB; a looped sample's seam after upsampling has no
  discontinuity above the interpolation ripple (compare against the periodic extension).
- `LoopSmoother`: a loop with a 10000-unit DC step at the seam has a seam step under
  `n`-frame slope after smoothing; a loop already continuous is unchanged (bit-identical).
- Determinism: the enhanced fixture's SHA-256 pinned in a test; re-run twice; if a
  `wasm32-wasip1` runner is present (H5's resolution says `node:wasi` works), compare there.
- Timing unchanged: `song_timeline` of a module before and after enhancement is identical
  (an enhancer changes samples, never the sequencer).
- `clippy`, `no-std-purity` (the trait is in the model crate: no `std` leaks).

### 5. Documentation

Architecture §6 (the builder hook), §11 (`starplayer-enhance`); accuracy policy §5 item 5
(the configuration-name rule for enhancers); M11 master plan: "enhance at scan time" now
has a concrete API. Append `## Research resolution`.

## Research points

1. **Where 8-bit dither belongs**: Amiga samples are 8-bit; upsampling does not add bits
   of resolution. A "16-bit reconstruction" (low-pass the quantisation noise) is a
   possible third enhancer; measure whether it helps on ST-01 samples and note it.
2. **Rate ceiling**: a 4× upsample of a 32 kHz sample is 128 kHz — pointless above the
   output rate. Cap the output rate at `2 × target_rate` if the builder knows it (it does
   not today; propose a `target_rate_hz` hint on `with_enhancer` or leave it to the
   caller and say so).
3. **Memory**: 4× on every sample of a 4 MB IT module is 16 MB; fine on desktop,
   not on `riscv32imc`. The trait is `no_std` but the implementations are `std`; note that
   an embedded host can run an enhancer at build time and ship the enhanced module.

## Verification

```
cargo test -p starplayer-model
cargo test -p starplayer-enhance
cargo test --workspace
cargo xtask goldens --check
cargo run -p starplayer-cli -- render <fixture.mod> --enhance sinc4x+loop -o /tmp/enhanced.wav
cargo xtask ci --job host-tests
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
```

## Out of scope

Spectral band replication or a learned model (they are further `SampleEnhancer`
implementations in their own crates once the API exists); running an enhancer at runtime;
the library scan (M11).

## Plan amendment (2026-09-09)

Pulled at the owner's request, with a new deliverable: checkboxes in the web player that
apply the enhancers at load. Planning against the code changed four things, so the work
is split into three task files and this one is kept as the record of the original shape:

1. `ModuleBuilder::with_enhancer` cannot be the hook — every loader builds its own
   `ModuleBuilder` internally. The hook is `Module::enhanced(&dyn SampleEnhancer)`, a
   rebuild through `add_sample`, and loaders stay untouched.
2. A new `reference_rate_hz` does not change pitch on MOD/MTM/XM, and sample-offset
   commands address source frames, so the module carries `rate_scale_log2` and the engine
   scales the step; processors scale offsets where they read them.
3. The sinc kernel is a committed table, so `starplayer-enhance` is `no_std + alloc`.
4. Ping-pong loop points scale as `F·start`, `F·(end − 1) + 1`; sustain-loop samples are
   resampled as one-shots.

Engine half: [K5a](complete/M10-task-K5a-enhancer-core.md). CLI/offline: [K5b](complete/M10-task-K5b-enhance-offline-and-cli.md).
Web: [W4](../apps/complete/W4-task-enhancement-checkboxes.md).
