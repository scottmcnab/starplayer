# M10 — K5a: The sample-enhancer trait, the module rebuild, the playback scale, and `starplayer-enhance`

| Field | Value |
|---|---|
| Milestone | M10 ([master plan](../M10-master-plan.md), decision 6); supersedes the engine half of [K5](../M10-task-K5-sample-enhancement.md) — see its "Plan amendment" |
| Status | Landed 2026-09-09; owner listening via K5b outstanding |
| Depends on | — (M7-H5's `sinc_table.rs` is the pattern to copy, not a dependency) |
| Blocks | [K5b](../M10-task-K5b-enhance-offline-and-cli.md) (CLI/offline), [W4](../../apps/W4-task-enhancement-checkboxes.md) (web checkboxes) |
| Parallel with | K1, K2 |
| Recommended model | Claude Opus (polyphase resampling with a periodic extension, an exact-power-of-two playback scale threaded through four format processors, and a cross-target determinism contract) |
| Verified by | agent (goldens untouched, identity rebuild equal, trace positions scale exactly, pinned hash), then owner listening via K5b |

## Context for a fresh agent

Read `CLAUDE.md` first, then `plans/engine/M10-master-plan.md` (decision 6 and the K5 row)
and the original [K5 task](M10-task-K5-sample-enhancement.md) — its deliverables 1, 2, 4
and 5 are what this task builds, reshaped by the four findings below. This task is the
**engine half**: the trait, the rebuild, the playback scale, and the two enhancers in their
crate. K5b adds the CLI and offline plumbing; W4 adds the web checkboxes. Neither can
start before this lands, so the public surface named here is a contract.

Tracker samples are mostly 8-bit and often 8–16 kHz. A **sample enhancer** is a load-time
transform: it takes a sample's decoded PCM and returns decoded PCM at a possibly higher
rate. The RT path is untouched, the result is deterministic and hashable, and the goldens
never see an enhancer (`plans/product/03-accuracy-policy.md` §5 item 5).

### Four findings that reshape the original K5 text

1. **The hook is a module rebuild, not a builder field.** All five loaders create their
   own `ModuleBuilder::new()` (`crates/starplayer-mod/src/loader.rs:84`, `s3m:171`,
   `mtm:85`, `xm:200`, `it:202`) and only MOD has a `LoadOptions`. A `&dyn` field on the
   builder would put a lifetime on it and on every S3M/XM/IT helper that takes
   `&mut ModuleBuilder`. So the API is `Module::enhanced(&self, &dyn SampleEnhancer)`,
   which rebuilds through the existing `ModuleBuilder::add_sample`
   (`crates/starplayer-model/src/builder.rs:83`). This is sound because `Module` derives
   `PartialEq` over all seven fields (`module.rs:49-58`), sample and pattern ids are push
   order (`builder.rs:84`, `:174`), `sample_pcm(id)[..length_frames()]` is exactly the
   stored body (`module.rs:106-111`; forward/ping-pong loops without a sustain loop store
   exactly `loop_end` frames, `module.rs:244-253`), and every header field goes through
   `set_header` (`from_parts` is `pub(crate)`). Hence an identity enhancer must rebuild an
   **equal** module — the first test.
2. **A new `reference_rate_hz` does not change pitch on MOD, MTM or XM.** MOD derives the
   step from the Amiga period and uses the rate only for `finetune_from_rate`
   (`crates/starplayer-mod/src/processor.rs:307`, `:815`, `:1072`); MTM reuses
   `ModProcessor`; XM uses the constant 8363 (`crates/starplayer-xm/src/processor.rs:81`,
   `:1690-1708`); S3M's `S2x` overwrites the channel rate with a table (`s3m/src/processor.rs:423`).
   And every sample-offset command addresses **source** frames (`9xx`/`Oxx`:
   `mod:549`, `s3m:346`, `xm:687`, `it:1153`). So the module **carries the factor**:
   `SampleSpec.rate_scale_log2: u8` (0 = identity), `reference_rate_hz` keeps the file's
   value, and playback scales the step and the offset by the factor. A shift by 0 is
   bit-identical, so the committed goldens prove the retrofit on every format.
3. **The sinc kernel is a committed table.** Runtime `sin`/`exp` differ across libm
   implementations, so the coefficients are generated once by an `#[ignore]` test with a
   regeneration gate — copy `crates/starplayer-dsp/src/sinc_table.rs:303-423` (`generate`,
   `kaiser`, `bessel_i0`, `the_committed_table_is_what_the_generator_produces`,
   `emit_table`) — and stored as `u64` bit patterns (`f64::to_bits`/`from_bits`). With the
   table committed, the runtime needs only `+ − × ÷` and casts, so **`starplayer-enhance`
   is `no_std + alloc`**, checked on the bare-metal target like every core crate.
4. **Ping-pong loop points do not scale as `× F`.** The mixer turns at `start` and `end − 1`
   with period `2(len − 1)` (`crates/starplayer-mixer/src/sample.rs:101-124`;
   `crates/starplayer-model/src/builder.rs:259` `ping_pong_reflect` mirrors it). The exact
   scaling is `start' = F·start`, `end' = F·(end − 1) + 1`. A sample with a sustain loop
   cannot be periodic in two loops at once; it is resampled as a one-shot (zero-extended)
   with both loops' points scaled; its guard is already silence (`builder.rs:65-72`).

### Code you must read before changing anything

- `crates/starplayer-model/src/{builder,sample,module}.rs` — `add_sample`, `SampleSpec`
  (11 fields, all with `SampleIndex` accessors at `sample.rs:256-308`), `validate`.
- `crates/starplayer-core/src/sample.rs` and `crates/starplayer-mixer/src/sample.rs` —
  `PRE_ROLL_FRAMES`, `GUARD_FRAMES`, `SampleRegion`, `LoopSpan::ping_pong_frame`.
- `crates/starplayer-engine/src/{source.rs:133, sequencer.rs:381 and :451, channel.rs:124, instrument.rs:205}`
  — the two places a step enters a voice, and the engine's own `sample_region`.
- The four format processors' `sample_region` helpers (`mod:1064`, `s3m:905`, `xm:1980`,
  `it:709`) and offset sites listed above.
- `crates/starplayer-dsp/src/{sinc_table.rs, tables.rs}` — the table pattern and
  `equal_power_q15` (`tables.rs:466`).
- `crates/starplayer-engine/tests/mixer_determinism.rs:367` — the repo's definition of a
  click-free loop wrap; the smoother's skip test reuses it.
- `xtask/src/main.rs:69-105` — `NO_STD_CRATES`, `FEATURE_ENABLED_NO_STD_CHECKS`.
- `crates/starplayer/Cargo.toml` — how optional crates are wired (`dep:` features).

## Deliverables

### 1. The trait and the rebuild, in `starplayer-model` (no_std + alloc)

```rust
// crates/starplayer-model/src/enhance.rs
pub struct SamplePcm<'a> {
    pub frames: &'a [i16],            // the stored body, guard excluded
    pub rate_hz: u32,                 // the sample's reference rate as loaded
    pub loop_mode: LoopMode,
    pub loop_start: u32,
    pub loop_end: u32,
    pub sustain_loop: Option<SustainLoop>,
}
pub struct EnhancedPcm {
    pub frames: Vec<i16>,
    pub rate_hz: u32,                 // must be rate_hz × 2^k, k in 0..=3
    pub loop_mode: LoopMode,
    pub loop_start: u32,
    pub loop_end: u32,
    pub sustain_loop: Option<SustainLoop>,
}
pub trait SampleEnhancer {
    /// Goes into the configuration name (`stem__i16_mono_44100_linear_enh-<name>.sha256`),
    /// so it must encode every parameter that changes the output (`sinc4x`, `loop=64`).
    fn name(&self) -> String;
    fn enhance(&self, sample: SamplePcm<'_>) -> EnhancedPcm;
}
```

- `SampleSpec` gains `rate_scale_log2: u8` (default 0) with a `SampleIndex` accessor; the
  builder stores it verbatim and `validate` rejects values above 3. `SampleIndex::to_spec()`
  reconstructs a `SampleSpec` from the accessors (precedent:
  `crates/starplayer-testkit/src/conformance.rs:591-596`).
- `Module::enhanced(&self, enhancer: &dyn SampleEnhancer) -> Result<Module, Error>`: a
  fresh `ModuleBuilder`; `set_header(self.header().clone())`; `set_orders`; every
  instrument cloned in id order; every pattern re-added from `pattern_bytes`, `rows`,
  `channels` in id order; every sample: `sample_pcm(id)[..length_frames]`, `enhance`, then
  `to_spec()` with the returned rate/loop/sustain fields and `rate_scale_log2 =
  log2(returned rate / source rate)` — an inexact ratio is `Error::Invalid("an enhancer
  must return the source rate times a power of two")`; `add_sample`; `build()`. A sample
  that already carries a non-zero scale is enhanced from its stored frames and the scales
  add. Empty samples round-trip.
- `ping_pong_reflect` (`builder.rs:259`) becomes `pub` so the upsampler's reflected
  extension is the mixer's arithmetic by construction.

### 2. The playback scale (mixer, engine, four processors)

- `SampleRegion` (`crates/starplayer-mixer/src/sample.rs:135`) gains `rate_scale_log2: u8`
  with a `with_rate_scale(self, log2: u8)` builder and an accessor; `Default`/`one_shot`/
  `looping` keep 0.
- The six `sample_region` helpers (`mod:1064`, `s3m:905`, `xm:1980`, `it:709`, engine
  `instrument.rs:205`; MTM shares MOD's) pass `sample.rate_scale_log2()`. MOD's one-word
  region at `mod/src/processor.rs:800` does the same.
- The engine shifts the step **at its two entry points only**: the initial `params.step`
  in `TickContext::trigger_channel` (`sequencer.rs:381` → `ChannelTable::trigger`,
  `channel.rs:124`) and `VoiceParam::Step` in `write_voice_param` (`source.rs:133`,
  `sequencer.rs:451`), by the target voice's `region.rate_scale_log2()`, with a saturating
  shift. Nothing inside `render()` changes (`rt-safety` stays green); the trace hook's
  dirty-bit path is untouched.
- Offsets are scaled where each processor **reads the file's offset command** (`mod:549`,
  `s3m:346`, `xm:687`, `it:1153`) so all later channel-state arithmetic runs in stored
  frames. Then audit every remaining comparison of a channel-state offset or position
  against `length_frames()`/`loop_start()`/`loop_end()` (`mod:572 advance_sample_pointer`,
  `mod:800`, `xm:1744`, `it:1522-1523`, `it:2135`, and anything
  `grep -n "length_frames()\|loop_end()\|loop_start()" crates/starplayer-*/src/processor.rs`
  turns up) and make each one unit-consistent. Record what you found in the Research
  resolution.
- **Proof** (offline crate, `features = ["trace"]`): for each of MOD, S3M, XM and IT,
  trace a fixture that uses a sample-offset command plain and enhanced with `sinc4x`; every
  per-tick sample position in the enhanced trace equals the plain position `<< 2`
  **exactly** (Q32.32 steps shift exactly; forward loop lengths scale exactly; ping-pong
  turns scale exactly by finding 4), and note, instrument, volume, period and pan columns
  are identical. Extend `starplayer_offline::fixtures` synthetic modules with a `9xx`/`Oxx`
  row where the existing ones lack it; REFLEX.S3M is a real-world S3M case.

### 3. `crates/starplayer-enhance` (no_std + alloc)

Cargo: `starplayer-model`, `starplayer-dsp` (for `equal_power_q15`); dev-deps
`starplayer-s3m`, `starplayer-offline`, `sha2`. Registered in the root
`[workspace.dependencies]`, in `NO_STD_CRATES`, and — via the facade feature
`enhance = ["dep:starplayer-enhance"]` (never in `default`, re-exported as
`starplayer::enhance`) — as `("starplayer", "enhance")` in `FEATURE_ENABLED_NO_STD_CHECKS`.

- **`SincUpsampler { factor: UpsampleFactor (Two | Four), rate_ceiling_hz: Option<u32> }`**
  with `new(factor)`, `with_rate_ceiling(hz)`. One committed polyphase table
  `[[u64; 64]; 4]` (64 taps per phase, four phases at quarter steps, Kaiser β = 10, cutoff
  0.9 × the *input* Nyquist, each phase's taps normalised to unit DC gain in f64); 2× uses
  phases 0 and 2. Generator + regeneration gate + `#[ignore]` emitter exactly like
  `sinc_table.rs`. Source treated as an infinite sequence: zero before frame 0; the body;
  then for `n ≥ loop_end` the forward loop's periodic continuation or the ping-pong loop's
  reflection (`ping_pong_reflect`), zero for one-shots and for samples with a sustain loop.
  Output body = `F × source frames` (ping-pong: `F·(end − 1) + 1` when no sustain loop), loop
  points per finding 4, sustain loop points scaled the same way. f64 accumulation in a
  fixed order (tap 0 to tap 63, no tree reduction); round half away from zero via
  `as i64` casts; saturate to `i16`. Works one sample at a time and writes `i16` directly —
  never a whole-module `f64` buffer. **Rate ceiling**: the sample's effective rate is
  `rate_hz` for S3M/IT/MOD/MTM and `8363 · 2^((relative_note + finetune/128)/12)` for XM
  (the XM loader stores 8363 for every sample, `model/src/sample.rs:118-127`); to keep the
  trait honest, `SamplePcm` carries `relative_note: i8` and `finetune: i8` too. If
  `effective × F > ceiling`, the largest factor that fits is used (2, then identity).
- **`LoopSmoother { crossfade_frames: u32 }`** — forward loops with no sustain loop only;
  `n = min(crossfade_frames, len / 2)`. **Skip (bit-identical) when the seam already passes
  the click criterion** of `mixer_determinism.rs:367` (the wrap step `|x[start] − x[end−1]|`
  ≤ the largest adjacent step inside the loop). Otherwise, if `loop_start ≥ n`: equal-power
  crossfade of the last `n` loop frames toward the `n` frames before `loop_start`, weights
  from `equal_power_q15`, i32 with rounding, so the frame after the wrap continues what the
  tail was heading for; if `loop_start < n` (the common MOD case): add a linear correction
  ramp over the last `n` frames that removes the DC step `x[start] − x[end − 1]`. No rate
  change; `name()` is `loop=<n>`. Ping-pong loops are continuous at the turn and untouched.
- **`Chain(Vec<Box<dyn SampleEnhancer>>)`** applying in order, `name()` joined with `+`.
- **`CATALOGUE: &[EnhancerDescriptor { id: &'static str, label: &'static str, description: &'static str, flag_bit: u8 }]`**
  with entries `sinc4x` (bit 0, "Upsample samples (4× sinc)") and `loop` (bit 1, "Smooth
  loop seams"), plus `sinc2x` (no bit; CLI-only), and `fn from_flags(flags: u32, ceiling_hz: Option<u32>) -> Option<Chain>`.
  K5b's parser and W4's `enhancements_json()` both read this; JS transcribes nothing.

### 4. Proof

- `cargo xtask goldens --check` unchanged (every scale is 0).
- Identity rebuild: `module.enhanced(&Identity) == module` for the synthetic MOD/MTM/XM/IT
  fixtures and REFLEX.S3M (an `Identity` enhancer in the model's tests).
- Trace-position `<< 2` invariant per format (deliverable 2).
- `SincUpsampler`: a 1 kHz sine at 8 kHz → 32 kHz has its image at 7 kHz below −80 dB and
  the fundamental within 0.05 dB (Goertzel in f64); a looped sample's seam after
  upsampling matches the upsampled periodic extension to within rounding; a ping-pong
  loop's scaled turn matches the mixer's reflection; the rate ceiling falls back per sample.
- `LoopSmoother`: a loop with a 10000-unit DC step at the seam passes the click criterion
  after smoothing; an already-continuous loop is bit-identical; both `loop_start` branches.
- Determinism: the SHA-256 of `REFLEX.S3M` enhanced with `sinc4x+loop` pinned in a test
  (hash the rebuilt module's `pcm()` little-endian); run twice.
- `song_timeline` of a module before and after enhancement is identical.
- `cargo xtask ci --job no-std-purity`, `clippy`, `rt-safety`, `host-tests`.

### 5. Documentation

Architecture §6 (the rebuild hook and `rate_scale_log2`, next to the guard-frame
paragraph, `plans/product/01-technical-architecture.md:696-736`), §11 (`starplayer-enhance`
beside the other `no_std` crates); accuracy policy §5 item 5 (the `_enh-<name>` filename
rule). `plans/engine/M10-master-plan.md`: point the K5 row at K5a/K5b. Append
`## Research resolution` here.

## Research points

1. Whether the engine's step shift should live in `VoiceParam::apply` (mixer, one site) or
   in the engine's two entry points (as specified). The engine sites are specified because
   the region is known there and the mixer's `Voice` stays ignorant of provenance; say if
   you found a reason to prefer the other.
2. `kaiser`/`bessel_i0` exist only as `#[cfg(test)]` functions in `sinc_table.rs`; lift them
   into a small `starplayer_dsp::design` module (f64 arithmetic only, no_std) and make the
   H5 generator use it, or duplicate them in this crate's generator test — say which.
3. 8-bit dither / 16-bit reconstruction (original K5 research point 1): measure on an ST-01
   style sample whether low-passing the quantisation noise helps; note it, do not build it.
4. Memory: a 4× rebuild of a 4 MB IT is 16 MB plus the transient copy; fine on desktop.
   W4 adds a frame budget in the wasm host; note anything an embedded host would need.

## Verification

```
cargo test -p starplayer-model -p starplayer-enhance -p starplayer-mixer -p starplayer-engine
cargo test -p starplayer-offline --features trace
cargo test --workspace
cargo xtask goldens --check
cargo xtask ci --job no-std-purity
cargo xtask ci --job rt-safety
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
```

## Out of scope

The CLI `--enhance` flag, `RenderOptions` and golden naming (K5b); the web checkboxes and
wasm export (W4); spectral band replication or a learned model; running an enhancer at
runtime; the library scan (M11).

## Research resolution

*Written 2026-09-09, on branch `k5a`.*

### 1. Where the step shift lives

**Kept in the engine, as specified**, and no reason was found to prefer `VoiceParam::apply`.
Three things settled it:

* `VoiceParam::apply` (`crates/starplayer-core/src/event.rs:360`) takes only
  `&mut VoiceParams`. It has no region, so moving the shift there would mean either giving
  `VoiceParams` a copy of the scale — a field the mixer would carry and never read — or
  moving the whole `apply` into the mixer, where `Voice` would then know why its frames
  exist. Both are worse than one `match` at each of the engine's own entry points.
* The shift has to happen at a **trigger** too, not only at a parameter write, and a
  trigger never goes through `apply`. So the mixer site would not have been one site; it
  would have been `apply` *plus* `Voice::new`.
* `render()` is untouched either way, but the engine sites keep the change entirely outside
  `starplayer-mixer`, so `rt-safety` and the SIMD kernels never had to be re-reasoned about.

Two adjustments to the letter of the deliverable, both narrowing rather than widening:

* The trigger-side shift is applied in **`ChannelTable::trigger`** (`channel.rs`) rather
  than in `TickContext::trigger_channel`. `trigger` is the single function every
  `trigger_channel` reaches, and it is *also* what the engine's own instruments call
  (`instrument.rs:398`, `:477`), so one site covers both instead of two sites covering one
  each.
* There is a **third** step entry point the task did not list, found by the audit below:
  FastTracker 2 recomputes its period every tick and writes it straight into
  `Voice::params` rather than through `write_voice_param`, at
  `crates/starplayer-xm/src/processor.rs:1777`. Left unscaled it would have overwritten the
  scaled trigger step on the very next tick and silently broken XM. `scaled_step` is
  therefore `pub` and `starplayer-xm` calls it. The XM trace-position test is what would
  have caught it.

### 2. `kaiser` / `bessel_i0`

**Duplicated in this crate's generator test**, not lifted into a `starplayer_dsp::design`
module. The reason is that the lift as described is not possible: `kaiser` needs
`f64::sqrt` and the prototype needs `f64::sin`, and neither exists in `core` — they are
`std`-only inherent methods, and a `no_std` `starplayer-dsp` would need `libm` to get them.
A `design` module would therefore have had to be `#[cfg(any(test, feature = "std"))]`, which
is a feature-gated public module whose only two consumers are both `#[cfg(test)]` anyway.

The duplication is forty lines of textbook mathematics in a `#[cfg(test)] mod tests`, and
each copy is pinned by its own regeneration gate against its own committed table
(`the_committed_table_is_what_the_generator_produces` in both `sinc_table.rs` and
`polyphase.rs`), so the two cannot drift without a test failing. Sharing would have bought
nothing a reader needs and cost a `std`-shaped seam in a crate that has none.

### 3. 8-bit dither / 16-bit reconstruction

**Measured, and there is nothing worth building.** Three measurements, on a scratch harness
run against the committed S3M fixtures and a synthetic control:

* **Every frame of REFLEX.S3M's and PETRI.S3M's PCM is a multiple of 256** — that is, both
  modules are entirely widened 8-bit, which is the ST-01-style material the research point
  asks about. After a `sinc4x` rebuild they are not, so the reconstruction is already
  producing genuinely 16-bit-valued frames rather than shifted 8-bit ones.
* **A clean sine quantised to 8 bits measures 47.0 dB SNR** (0.92 full scale; the textbook
  full-scale figure is 49.9 dB). That is the floor the source carries, and no load-time
  transform can lower it — the quantisation happened in someone else's sampler, decades ago.
* **Only 1.4 % of the upsampled noise power lies above the source Nyquist.** This is the
  number that settles it: the polyphase filter's stopband keeps the source's noise inside
  the source's own band, so a low-pass at the source Nyquist could remove at most 1.4 % of
  the noise power — about 0.06 dB — and would remove signal along with it, because signal
  and noise occupy exactly the same band.

Adding dither at the 16-bit rebuild is worse than useless for the same reason: TPDF dither
would put a fresh floor roughly 48 dB *below* the one already there.

The transform that would actually help is noise **shaping at the original quantisation**,
which is not available to us, or a de-noiser / band-extender — and both of those are the
spectral work this task lists as out of scope. **Noted, not built.**

### 4. Memory

Measured on the two committed S3M fixtures:

| Module | Samples | PCM frames | `sinc4x` | Growth | With a 22 050 Hz ceiling |
|---|---|---|---|---|---|
| REFLEX.S3M | 4 | 2 240 (4 kB) | 8 768 (17 kB) | 3.91x | 4 416 frames, 1.97x |
| PETRI.S3M | 5 | 32 078 (63 kB) | 128 072 (250 kB) | 3.99x | 64 076 frames, 2.00x |

Growth is just under 4x rather than exactly 4x because each sample carries a fixed pre-roll
and guard that do not scale, and because a ping-pong loop's stored length is
`F·(end − 1) + 1` rather than `F·end`.

Both fixtures' samples sit at 8 363-8 900 Hz, so a 22 050 Hz ceiling puts **every** sample
at 2x — `8363 × 4 = 33 452` is over, `8363 × 2 = 16 726` is not — and the blob doubles
instead of quadrupling. That is the ceiling working exactly as intended: it is a per-sample
decision, and on this material every sample happens to make the same one.

Extrapolating: a 4 MB IT becomes 16 MB. Fine on desktop, fine in a browser tab.

What an **embedded** host would need, for M8's benefit:

* The rebuild is a second allocation of the whole PCM blob. On a target where the module is
  *borrowed* from memory-mapped flash (architecture §6's mmap case), enhancement gives that
  up entirely — the rebuilt blob has to live in RAM. An ESP32-class host should treat
  `sinc4x` as unavailable rather than as a slow path, and use the ceiling to opt in per
  sample if it wants anything at all.
* **`ModuleBuilder` has no `reserve`.** `add_sample` extends `self.pcm` frame by frame, so
  the growing `Vec` doubles, and during the final reallocation the old and new buffers
  coexist. The peak is therefore well above the `4 × source` the finished module costs. An
  embedded host — or a wasm host with a hard heap ceiling — would want
  `ModuleBuilder::reserve_pcm(frames)` and `Module::enhanced` to call it with the total the
  enhancer is about to produce. That is a small, self-contained follow-up, deliberately not
  taken here because it changes the builder's public surface and nothing on desktop needs
  it.
* The rebuild is not incremental and cannot be interrupted, so a host with a frame budget
  has to run it off the audio thread and swap the module in when it is done — which is
  exactly what the engine's existing `Arc<Module>` swap and garbage channel already do.
  W4's wasm budget is that pattern; nothing new is needed for it.

### 5. The offset-site audit

`grep -n "length_frames()\|loop_end()\|loop_start()" crates/starplayer-*/src/processor.rs`
turns up twenty-eight lines. Classified:

| Site | What it compares | Verdict |
|---|---|---|
| `mod:304` | `sample.length_frames() > 0` | A zero test. Unit-free. |
| `mod:573` `advance_sample_pointer` | channel `sample_offset` vs `length_frames()` | **Consistent**: the offset is scaled at `sample_offset()`, where the file's `9xx` parameter is read, so both sides are stored frames. |
| `mod:799` | `sample_offset + 2` vs `length_frames()` | **Changed.** PT's retained one-word region is two *source* frames; scaled to `2 << rate_scale_log2()` so an enhanced sample's "one word" is the same duration. The region is tagged with the scale. |
| `mod:1064` `sample_region` | loop points → `SampleRegion` | **Changed**: tagged with the scale. |
| `mod:1169`, `:1173`, `:1831` | test helpers reading `region().length_frames()` | Test-only, and already in region frames. |
| `mtm:250` | test helper | Test-only. MTM shares `ModProcessor`, so it inherits every MOD site above. |
| `s3m:346` (`Oxx`) | writes `sample_offset` | **Changed**: scaled by the sample the channel is about to trigger. |
| `s3m:905` `sample_region` | loop points | **Changed**: tagged. |
| `xm:687` (`9xx`) | writes `sample_start_frame` | **Changed**: scaled, using the sample resolved four lines above. |
| `xm:1726` | `length_frames() > 0` | A zero test. |
| `xm:1744` | `offset >= length_frames()` (`kFT2ST3OffsetOutOfRange`) | **Consistent**: the offset is scaled at `:687`. Note this is the site that would break loudly if it were not — a `9xx` past the end of the plain sample lands *inside* the quadrupled one. |
| `xm:1980` `sample_region` | loop points | **Changed**: tagged. |
| `it:709` `sample_region` | loop points, sustain and main | **Changed**: tagged. |
| `it:1522-1523` | `loop_start`/`loop_end` vs `position >> 32` | **Consistent** in units, but it *quantises*: the sustain-loop release truncates the Q32.32 cursor to a whole frame, so `4·floor(p)` and `floor(4p)` differ by up to three frames. This is IT's own semantics (`SusAfterLoop.it`), it is not made worse by the scale, and it is the one place the `<< 2` invariant is stated as a bound rather than an equality. `tests/enhanced_trace_positions.rs::an_it_sustain_loop_release_quantises_its_position_to_a_whole_frame` pins the bound. |
| `it:2135` (`Oxx`) | `offset >= length_frames()` | **Consistent**: the offset — `SAx`'s high byte included — is scaled at the read, from the *sounding* sample rather than from one a note on the same row might be about to select. That is deliberate: `end` is already read from the sounding sample, and the two sides of the comparison have to be in one sample's units. It is also moot in practice, since `Module::enhanced` gives every sample the same enhancer and only a rate ceiling can make two samples differ. |
| `it:2424` | `region().length_frames()` | Already in region frames. |
| `engine instrument.rs:205` | loop points | **Changed**: tagged. |

Nothing else in the four processors compares a channel-state offset against a sample
extent. The only remaining frame-count constant in a comparison is MOD's one word, handled
above.

### 6. Two design calls the task file left open

**`reference_rate_hz` keeps the file's value.** Deliverable 1 says `to_spec()` takes "the
returned rate", and finding 2 says "`reference_rate_hz` keeps the file's value". Those
conflict, and finding 2 wins: the returned rate is used **only** to derive
`rate_scale_log2`. Quadrupling `reference_rate_hz` would do nothing at all for MOD, MTM and
XM (which never derive pitch from it) and would break MOD's `finetune_from_rate` table
lookup outright, while for S3M and IT it would double-count against the engine's own shift.
`SamplePcm::rate_hz` is documented as *the rate the frames in the slice represent* —
`reference_rate_hz << rate_scale_log2` — so a chained or already-scaled sample's ceiling
check stays honest and the scales add.

**The trace-position invariant is stated against the `Linear` kernel.** A forward loop's
wrap is deferred by the interpolator's `LEADING_FRAMES`
(`starplayer_mixer::kernel::forward_wrap_bits`), which is a constant in *frames* and does
not scale. `Linear` and `Nearest` have zero leading frames, so the wrap is exactly
`loop_end` and scales exactly; `Cubic` and `Sinc` do not. The offline trace engine is
`Engine<FixedPath, Linear, StereoI16>`, so the proof is exact where it is made, and the
deferral is a rendering difference rather than a sequencing one. This is recorded in the
test's own module documentation.

### 7. Deviations from the task file

* The identity-rebuild test over real fixtures lives in `crates/starplayer-enhance/tests/`
  rather than in the model's own tests: `starplayer-model` cannot load a MOD or an IT
  without dev-depending on all five format crates. The model keeps an `Identity` enhancer
  and a hand-built module covering every sample shape (forward, ping-pong, one-shot, empty,
  sustain), which is the part that does belong there.
* The trace-position proof lives in `crates/starplayer-offline/tests/`, gated on that
  crate's own `trace` feature, rather than in `starplayer-enhance/tests/`. Resolver 2
  unifies dependency features across the workspace, so a `starplayer-offline/trace`
  dev-dependency in the enhance crate compiles the engine's per-tick recorder into
  `render()` for **every** `cargo test --workspace` in the repository — which is exactly
  what `starplayer-offline/Cargo.toml` already warns about, and which showed up immediately
  as three `starplayer-host` allocation tests failing.
* `xtask`'s `assert_resolved_features_exclude_std` now walks `--edges features,no-dev`.
  Without it, `starplayer-enhance`'s dev-dependency on the (necessarily `std`) offline
  harness reads as a purity violation. Dev-dependencies are never compiled into the crate a
  host links, and the `cargo check --target <bare metal>` half of the same job already
  ignored them, so the two halves now agree on what they are looking at.
* `synthetic_it_with_offset()` is a **new** fixture rather than an `Oxx` added to
  `synthetic_it()`: the committed IT golden hashes that generator's bytes. Its looped sample
  is 512 frames (so the smallest non-zero offset parameter, 256 frames, lands inside it) and
  has a forward loop with no sustain loop (so the release quantisation above does not blunt
  the exactness proof). `synthetic_it()`'s bytes are unchanged, which `cargo xtask goldens
  --check` confirms.
* The S3M offset case is **PETRI.S3M**, not REFLEX.S3M: REFLEX uses no `Oxx` at all
  (its commands are A, C, D, G, H, J, K, Q, S), while PETRI uses 146 of them against real
  samples. Both are traced; REFLEX remains the determinism hash's subject.
* `catalogue::enhancer_for_id` and `catalogue::descriptor` were added beside `from_flags`.
  Without a constructor, the `sinc2x` entry the task asks for — an enhancer with no flag bit
  — would be a catalogue row nothing could build, and K5b's parser would have to hard-code
  the mapping the catalogue exists to own.
* `starplayer-enhance`'s dev-dependency is the **facade** rather than `starplayer-s3m`: the
  identity rebuild has to be proved on all five formats, and the facade's autodetecting
  loader is the one thing that reaches them all. It re-exports `starplayer-s3m`, so REFLEX
  and PETRI still load.
