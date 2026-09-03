# M6 — G2: The per-voice resonant filter

| Field | Value |
|---|---|
| Milestone | M6 ([master plan](M6-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | D6 landed (the scope tap is in place, so `kernel.rs` is free), E2 (the `2^(n/768)` table) |
| Blocks | M6 exit (IT modules with `Zxx` and filter envelopes sound wrong without it) |
| Parallel with | G3, D4, F2 |
| Recommended model | Claude Opus (the mixer's inner loop; both mix paths; the golden contract) |
| Verified by | agent (`cargo xtask goldens --check` byte-identical, block-size determinism with the filter on, a spectral test), then reviewer |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there are non-negotiable, above all here: **no transcendental functions
in the real-time path — tables only** (architecture §7.3), byte-identical output at every
host block size, and the fixed-point path is the bit-exact golden reference across x86,
ARM and WASM.

Impulse Tracker's resonant low-pass filter is a **voice-level** effect, not a DSP-graph
insert (M6 master plan: "Resist the temptation to generalise the filter into the DSP
graph — that is M7's job and a different abstraction"). Every voice carries
`VoiceParams.filter: FilterParams { cutoff: U0F16, resonance: U0F16 }`
(`crates/starplayer-core/src/event.rs`, with `FilterParams::BYPASS` and `is_bypass()`),
and `VoiceParam::Filter` can already be written — the IT processor (task G3, concurrent)
will write it from the instrument's initial cutoff/resonance, the filter envelope and
`Zxx` macros. **Nothing reads it.** The kernel (`crates/starplayer-mixer/src/kernel.rs`,
`mix_run`) interpolates, applies the gain ramps and accumulates; the voice's own signal
exists only inside `MixPath::mix` (`crates/starplayer-mixer/src/path.rs`) before the add.
This task makes the filter real, on both paths, without touching a single byte of the
unfiltered output.

### The reference

OpenMPT's `soundlib/Sndmix.cpp` `CSoundFile::SetupChannelFilter` (coefficients from
cutoff, resonance and the `flt_modifier`, with the IT-compatible branch) and the
`ITResonanceTable` in `Tables.cpp`, and its `Fastmix`/`IntMixer.h` `ResonantFilter` step
(a two-pole IIR: `y = a0·x + b0·y1 + b1·y2`, applied to the interpolated mono sample
before panning). Read them with WebFetch. The cutoff→frequency law is
`110 · 2^(0.25 + cutoff/24)` Hz — a power of two, which is exactly what
`starplayer_core::tables::LINEAR_FREQUENCY_TABLE` (`2^(n/768)`, E2) provides at
`n = 192 + 32·cutoff`, so no `pow`; the resonance law is a 128-entry table OpenMPT ships
verbatim; the rest is multiplication and one division, both IEEE-exact.

### Code you must read before changing anything

- `crates/starplayer-mixer/src/{kernel,path,voice,gain,sample}.rs` — the whole kernel,
  both `MixPath` impls (`FloatPath`, `FixedPath`, `GAIN_FRACTION_BITS`), `Voice` (where the
  two delay-line values go), `mix_run`'s `const RAMPING`/`const REVERSE` monomorphisation.
- `crates/starplayer-dsp/src/{lib,interpolate,ramp}.rs` — the crate the filter's
  coefficient code belongs in (`starplayer-dsp` "interpolators, ramping, IT resonant
  filter, biquad", architecture §11).
- `crates/starplayer-core/src/{event,tables}.rs` — `FilterParams`, `VoiceParam::Filter`,
  `DirtyBits` (a filter write currently sets `PITCH`; decide whether it needs its own bit),
  `LINEAR_FREQUENCY_TABLE`, `linear_frequency_q24`.
- `crates/starplayer-engine/src/{engine,scope,trace}.rs` — where `render_quantum` calls the
  pool, and the D6 scope tap (which deliberately ignores the filter; keep it that way).
- `crates/starplayer-offline/src/lib.rs` — `canonical_sha256`, the goldens, `segmental_snr_db`;
  `crates/starplayer-testkit/src/bin/starplayer-perceptual/analysis.rs` — a radix-2 FFT
  already exists there (D7); lift it into the testkit library if the spectral test wants it.
- `crates/starplayer-engine/tests/block_size_determinism.rs`,
  `crates/starplayer-offline/tests/render_allocation.rs`.
- `plans/product/01-technical-architecture.md` §7.1–§7.3, §5.3; `03-accuracy-policy.md` §3
  (IT entries continue from **D64**; G3 is allocating in the same range — coordinate by
  taking **D70–D74** for this task).

## Deliverables

### 1. `starplayer_dsp::filter` — coefficients from tables

```rust
pub struct FilterCoefficients<Sample> { pub input_gain: Sample, pub feedback_1: Sample, pub feedback_2: Sample }
pub fn resonant_low_pass_f32(cutoff: u8 /* 0..=127 */, resonance: u8 /* 0..=127 */, sample_rate_hz: u32, extended_range: bool) -> FilterCoefficients<f32>;
pub fn resonant_low_pass_fixed(cutoff: u8, resonance: u8, sample_rate_hz: u32, extended_range: bool) -> FilterCoefficients<i32 /* Q?.? — choose and document */>;
```

Both derive from the same integer inputs; the float one may use `f32` arithmetic (no
`powf`, no `exp`, no `sin`); the fixed one integer arithmetic only. The frequency comes
from `LINEAR_FREQUENCY_TABLE`; the resonance from a transcribed `IT_RESONANCE_TABLE`
with a test that it matches OpenMPT's values. `FilterParams` → `(cutoff, resonance)`
mapping: `cutoff = bits · 127 / 65535` rounded — G3 encodes `cutoff · 516` (research point
1 fixes the encoding jointly with G3 and the trace's `unit_to_scale(bits, 255)` so the
oracle's 0..255 field round-trips). Bypass is IT's rule: cutoff 127 with resonance 0 is
no filter at all.

### 2. The kernel step

- `Voice` gains `filter_state: [Sample; 2]` per path — store as two `f32` and two `i32`
  (a `Copy` struct; the unused pair costs eight bytes) — reset on trigger and on
  `set_region`/`retrigger` as OpenMPT resets on a new note.
- `MixPath` gains `fn filter(sample: Self::Mono, state: &mut [Self::Mono; 2], coefficients: &FilterCoefficients<Self::Mono>) -> Self::Mono` (or an equivalent shape you justify), applied to the interpolated mono value **before** the pan gains.
- `mix_run` gains `const FILTERED: bool`; the `false` arm is textually the code that exists
  today so its output is byte-identical (the goldens prove it). The `true` arm computes the
  filtered sample per frame. Coefficients are recomputed **only when the voice's filter
  params change** (a dirty check at the start of the voice's run, at tick rate in
  practice), never per frame.
- Clipping: the fixed path's filter can overshoot; saturate the way `FixedPath` already
  saturates its accumulator, and document the headroom.

### 3. Proof

- `cargo xtask goldens --check` unchanged (bypass arm byte-identical).
- Block-size determinism test with a scripted voice whose filter is on (both paths).
- A spectral test: a white-noise-like sample (a fixed LCG sequence, not `rand`) rendered
  through cutoff 0 / resonance 0 has at least 24 dB less energy above 2 kHz than the
  unfiltered render at 44.1 kHz; a resonance-127 render shows a peak near the cutoff.
  Deterministic; no tolerance on the exact values, only on the bands.
- The allocator hook still passes (`--job rt-safety`); `trace-zero-cost` unaffected.
- A golden for a synthetic filtered fixture is **not** added here — G3's synthetic IT
  fixture will carry a `Zxx` once both land; add a `crates/starplayer-mixer` unit test that
  pins the first 64 filtered samples of a known input on the fixed path as the
  cross-target contract instead.

### 4. Documentation

Architecture §7.2's "IT's resonant filter, which is a *voice*-level filter rather than an
insert" becomes a paragraph on what landed: the law, the tables, the two arms, the reset
rule. Accuracy-policy entries (D70–D74 as needed) for any place the fixed path's
quantisation of the coefficients knowingly differs from OpenMPT's float.

## Research points

1. **The `FilterParams` encoding.** Fix, jointly with G3's task file (read it), the exact
   `U0F16` encoding of a 0..127 cutoff and resonance such that the trace's
   `unit_to_scale(bits, 255)` reproduces libxmp's 0..255 field. Write the two helper
   functions in `starplayer-core` (`FilterParams::from_it(cutoff, resonance)`,
   `FilterParams::to_it()`), so both tasks use one definition.
2. **Extended filter range** (`flt_modifier`, OpenMPT's `SONG_EXFILTERRANGE`): the IT
   header flag G1 exposes as `has_extended_filter_range`. Implement the modifier path if
   it is one multiplication; note it either way.
3. **Fixed-point format** for the coefficients and state: enough headroom for
   resonance 127 without overflow on a full-scale input. Justify the choice with the
   worst-case gain of the two-pole.
4. **Does the filter run when the voice is muted?** It must, for the same reason muting is
   a mixer discard: state continuity on unmute.

## Verification

```sh
cargo test -p starplayer-dsp -p starplayer-mixer -p starplayer-engine
cargo test --workspace
cargo xtask goldens --check                       # byte-identical
cargo xtask ci --job rt-safety
cargo xtask ci --job fma-check
cargo xtask ci --job trace-zero-cost
cargo xtask ci --job no-std-check
cargo xtask ci --job clippy
cargo test -p starplayer-engine --test block_size_determinism
```

Report the exact commands and results. **Do not commit** — the reviewer commits.

## Out of scope

The IT processor, envelopes, `Zxx` parsing (G3). A high-pass mode. The M7 DSP graph.
