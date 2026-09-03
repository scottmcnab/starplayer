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

## Research resolution

Recorded at implementation time; each answer is the decision, not a summary of the
question. The references were read at OpenMPT `master`: `soundlib/Snd_flt.cpp` (which is
where `SetupChannelFilter` and `CutOffToFrequency` actually live — not `Sndmix.cpp`),
`soundlib/IntMixer.h` and `soundlib/FloatMixer.h` (`ResonantFilter`),
`soundlib/MixerInterface.h` (`SampleLoop`, which fixes the order
`interpolate(); filter(); mix();`), `soundlib/Mixer.h`
(`static_assert(MIXING_FILTER_PRECISION == 24)`) and `soundlib/Sndmix.cpp`
(`HandleNoteChangeFilter`, `ProcessPitchFilterEnvelope`).

### 1. The `FilterParams` encoding — **`514 × value`, with cutoff 127 mapping to `U0F16::MAX`**

`FilterParams::from_it(cutoff, resonance)` and `FilterParams::to_it()` are in
`starplayer-core`'s `event.rs`, `const fn`, with the reasoning in their doc comment. Three
constraints had to hold at once and one of them is not linear:

1. `to_it(from_it(v)) == v` for all 128 values.
2. The trace's `unit_to_scale(bits, 255)` — `(bits · 255 + 32767) / 65535` — has to
   reproduce libxmp's dumped field, and libxmp stores the IT value **doubled**
   (`src/player.c:433`, `xc->filter.cutoff = val << 1`; same for resonance at 436).
3. A fully open cutoff has to be the value `FilterParams::BYPASS` already carries, or
   `is_bypass()` — which the mixer uses to skip the filter entirely for MOD/S3M/MTM —
   would stop recognising IT's own "cutoff 127 with resonance 0 is not a filter" rule.

`514` falls straight out of (2): `514 = 2 × 257` and `65535 = 255 × 257`, so
`unit_to_scale(514·v, 255)` is exactly `2·v` with no rounding anywhere, and
`(514·v · 127 + 32767) / 65535 = v` for every `v ≤ 127`, which settles (1). The task file
suggested `516`; that is the linear `65535/127` scale and it fails (2) — at cutoff 100 it
reports 201 where libxmp reports 200 — so it was not used.

(3) is the one exception: `from_it(127, ·)` returns `U0F16::MAX` (65535) rather than
`514 × 127 = 65278`. That is harmless against the oracle because libxmp's comparator, and
the conformance adapter after it (`conformance.rs`: `if cutoff == 0 || cutoff >= 254`),
already treat 254 and 255 as the same fully-open cutoff. **Resonance keeps the linear
scale at 127**, reporting libxmp's 254, because resonance has no sentinel and the adapter
compares it exactly. The asymmetry is deliberate and tested
(`the_it_filter_encoding_reproduces_libxmps_doubled_field`,
`a_fully_open_it_filter_is_the_bypass_the_mixer_already_knows`).

**No new `DirtyBits` bit.** A filter write still raises `PITCH`, and the trace still
reports no flag for one (`dirty_bit_for` maps `VoiceParam::Filter` to `None`). Adding a bit
would change the trace's flag column and every golden that carries it, and it would buy
nothing: the mixer caches the `FilterParams` its coefficients were derived from and
compares four bytes, which is cheaper than a bit test, cannot be missed by an owner who
forgets to raise the bit, and covers a sample-rate change for free.

### 2. Extended filter range — **implemented in full; it is two changes, not one multiplication**

The IT header bit (OpenMPT's `SONG_EXFILTERRANGE`, which G1 exposes as
`has_extended_filter_range`) changes *both* halves of `SetupChannelFilter`:

* `CutOffToFrequency` divides the exponent by 20 rather than 24 — a top cutoff of
  10670 Hz instead of 5124 Hz, and 10670 is the exact figure `Snd_flt.cpp`'s own header
  comment quotes for MPT 1.16, which is the strongest available confirmation that the
  divisor is right.
* The coefficient branch changes: `kITFilterBehaviour && !SONG_EXFILTERRANGE` takes
  `d = damping·r + damping − 1`, and everything else — including every extended-range IT
  module — takes `d = (2·damping − min((1 − 2·damping)/r, 2)) · r`. `e = r²` is the same
  on both branches.

Both are implemented, on both mixing paths, and are covered by
`both_paths_reproduce_the_reference_algebra` and
`the_cutoff_law_matches_the_transcendental_at_every_value` over all 128 cutoffs.

The exponent is the awkward part: `0.25 + cutoff/20` octaves is `(960 + 192·cutoff)/3840`,
and 3840 is five times `LINEAR_FREQUENCY_TABLE`'s 768, so the exact index is a *fifth* of a
table step. Rather than round the index — which would cost up to 0.045% of the cutoff
frequency — the two neighbouring entries are interpolated, which is one subtract, one
multiply and one divide, off the per-frame path. Recorded as accuracy-policy **D71**.

**The plumbing is a stub by design.** The flag lives on the voice
(`VoiceFilter::set_extended_range`, reachable through `Voice::filter_mut`) and defaults to
off, so the format processor that knows the module's header can set it in one line. G3 owns
that line; nothing in this task reads the IT header. `the_extended_filter_range_opens_the_cutoff_further`
pins the mixer's half of it.

### 3. Fixed-point format — **Q8.24 coefficients in `i32`, a 256× pre-amplified `i32` delay line**

Which is OpenMPT's own `MIXING_FILTER_PRECISION` and `MIXING_FILTER_PREAMP`, chosen here on
the worst-case gain rather than by imitation:

* `input_gain = 1/(1 + d + e)` is largest where `1 + d + e = damping·(r + 1) + r²` is
  smallest. `r = sr/(2π·frequency)` bottoms out at `1/π ≈ 0.3183` — the cutoff clamped to
  Nyquist — and `damping` at `0.0645` (resonance 127), so `1 + d + e ≥ 0.1863` and
  `input_gain ≤ 5.37`.
* `feedback_1 = (d + 2e)/(1 + d + e)` runs from `−3.83` at that same corner up towards
  `+2` as `r` grows; `feedback_2 = −e/(1 + d + e)` stays inside `(−1, 0]`.

So the coefficients need three integer bits and Q8.24 gives eight — twenty times the worst
case — while 24 fractional bits quantise them at `6 × 10⁻⁸`, below the `f32` path's own
`1.2 × 10⁻⁷`. `no_coefficient_leaves_the_range_q8_24_provides` sweeps all 128 cutoffs × 128
resonances × 8 sample rates × both ranges and confirms it empirically.

The **delay line**, not the output, is what needs the pre-amplification: at a low cutoff and
a high sample rate `input_gain` is small and `y[n]` is a fraction of an `i16` LSB for many
frames running, so an un-amplified state would quantise to zero and the filter would output
silence. Eight bits puts the state at `i16 × 256` and the feedback clamp at
`i16 × 2 × 256 = 2²⁴`; against a coefficient saturated all the way to `i32::MAX` the widened
sum is still below `2⁵⁷`, so `i64` is never close to overflowing.

**Headroom and clipping.** A resonant two-pole overshoots — that is the effect — so nothing
is clamped on the way *out*. What is clamped is the *feedback*, to twice the input range,
exactly as OpenMPT's `ClipFilter` does: that bounds the recursion without flattening the
resonant peak. The mixer's accumulator then saturates as it already did
(`FixedPath::accumulate`'s `saturating_add`) and the master bus bounds the quantum after
it, so a filter overshoot is handled by machinery that was already there. The one place the
fixed path knowingly differs from OpenMPT is its rounding rule, which follows C6 rather than
OpenMPT's shift — accuracy-policy **D72**.

### 4. Does the filter run when the voice is muted? — **yes, and it has to**

`accumulate_masked` already renders a muted voice into a discard buffer rather than
skipping it, and the filter is inside that call, so this came out right by construction —
but it is the case that most deserves a test, because the filter is the only part of the
voice path whose output depends on the *two frames before it*. Skipping it would leave the
delay line holding whatever was in it when the channel was muted, and unmuting would ring
the filter with a discontinuity that was never in the signal.
`a_muted_voices_filter_keeps_its_delay_line_moving` renders 1024 frames muted then 1024
audible and asserts the audible half is byte-identical to the second half of a render that
was never muted.

### Two things the task file asked for that were done differently

* **The reset rule.** The task asks for a reset on `set_region` as well as on trigger and
  retrigger. OpenMPT resets only under `chn.triggerNote`, and `set_region` is precisely the
  tone-portamento sample swap that is *not* a new note (`FilterPortaSmpChange.it`). The
  delay line is therefore reset in `Voice::new` and `Voice::retrigger` and kept across
  `Voice::set_region`. Recorded as accuracy-policy **D73** and pinned by
  `a_retrigger_resets_the_delay_line_but_a_sample_swap_does_not`.
* **`ITResonanceTable` is not in OpenMPT's `Tables.cpp`.** OpenMPT has no such table; it
  calls `std::pow` inline in `Snd_flt.cpp`. The literal 128-entry table the task describes
  is Schism Tracker's `resonance_table` (`player/filters.c`), which is what was
  transcribed — as Q0.24 integers, so the fixed path never touches a float — with both
  Schism's printed literals and the underlying formula checked in tests. **D70**.

### One consequence worth flagging for G3

`FilterParams` carries whole IT cutoff units, so a filter *envelope*'s half-unit resolution
(`cutoff · (envModifier + 256) / 512`) is rounded before it reaches the mixer: a sweep moves
in 128 steps rather than 256. Recorded as **D74**, with a note that the `U0F16` field has
the bits spare if a corpus case ever shows it.
