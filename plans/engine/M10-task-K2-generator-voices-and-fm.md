# M10 — K2: Generator voices in the mixer, and the FM instrument

| Field | Value |
|---|---|
| Milestone | M10 ([master plan](M10-master-plan.md), "The task graph" section) |
| Status | Planned; pull-driven (not scheduled) |
| Depends on | K1 (the `&mut self` trait, `Adsr`, `starplayer-synth`), M7-H2 (`sin_q15`, `pow2_q24`) |
| Blocks | K3, K6 |
| Parallel with | K5 |
| Recommended model | Claude Opus (the mixer's inner loop gains a second arm; the goldens are the gate) |
| Verified by | agent (goldens byte-identical, block-size determinism, cross-path bit-identity of the generator output, spectral tests of every algorithm), then owner |

## Context for a fresh agent

K1 proved a sample-backed synth needs nothing from the mixer. FM cannot be sample-backed:
an operator's phase is modulated per frame by another operator's output, so the waveform
is computed, not read. Master-plan decision 2 adds a **generator** kind of voice to the
mixer — `Voice` gains a `VoiceSource` that is either the sample region it has today or a
fixed-size `GeneratorState` — rendered by a second arm of `accumulate_voice`
(`crates/starplayer-mixer/src/kernel.rs`), after which the frame goes through the same
gain ramps, the IT resonant filter and the pan law as a sample frame. Decision 3 makes
generators **integer-only**: `i16`-scale `i32` output from tables on both paths, so the
generator is bit-identical everywhere and the float path merely converts.

The sample arm must stay textually what it is: `cargo xtask goldens --check` is the proof,
as it was for G2's filter and H1's buses.

### Code you must read before changing anything

- `crates/starplayer-mixer/src/{voice,kernel,path,sample}.rs` — `Voice`, `VoiceParams`,
  `accumulate_voice`, `mix_run` and its `const` arms, `MixPath::Mono`/`interpolate`/
  `accumulate`, `VoicePool::allocate` (a generator voice still needs a `SampleRegion`
  today — it will not after this task).
- `crates/starplayer-engine/src/channel.rs` — `ChannelTable::trigger` (the only way an
  instrument starts a voice; it takes a region).
- `crates/starplayer-synth/src/*` (K1) — `Adsr`, the parallel-array state pattern, the
  instrument's `note_on` shape.
- `crates/starplayer-dsp/src/tables.rs` — `sin_q15`, `pow2_q24`; `crates/starplayer-core`'s
  `LINEAR_FREQUENCY_TABLE` and `Step`.
- `plans/product/01-technical-architecture.md` §5.3, §7.1, §7.3 (the FMA and
  transcendental rules apply to generators too — integer only makes both moot).

## Deliverables

### 1. `VoiceSource` in the mixer

```rust
pub enum VoiceSource { Sample(SampleRegion), Generator(GeneratorState) }
pub struct GeneratorState { pub kind: GeneratorKind, pub phase: [u32; 6], pub level: [i32; 6], pub feedback: i32, pub scratch: [i32; 4] }
pub enum GeneratorKind { Fm4 { algorithm: u8, ratios: [u16; 4], feedback: u8 }, /* K3 adds Sid, K6 adds Pluck */ }
```

`Voice::region()` becomes `Voice::source()`; `SampleRegion`-taking constructors keep
working through `VoiceSource::Sample`. `VoicePool::allocate_generator(tag, state, params)`
and `ChannelTable::trigger_generator` beside the existing ones. Size: keep `Voice` under
256 bytes; `GeneratorState` at most ~96 bytes — the pool has 272 slots.

`accumulate_voice` matches on the source once per voice per segment: the sample arm is
the existing body **unchanged**; the generator arm runs `generate_run::<Path, RAMPING,
FILTERED>` producing `Path::Mono` via `Path::from_generator(i32)` (float: `as f32`; fixed:
identity) for each frame, then the same `filter`/`accumulate` calls as `mix_run`. A
generator voice's `step` is its base frequency as `Step` — the phase increment per frame is
`step.to_bits() >> k` with the sample-rate scaling folded in at trigger, so pitch bends and
the existing `VoiceParam::Step` writes work with no new parameter.

### 2. The FM engine (`crates/starplayer-synth/src/fm.rs`)

Four operators, eight algorithms (the OPL3/DX-style 4-op set: serial chain, 2+2, 3-into-1,
parallel, and the four mixed ones — list them by number in the doc comment with a diagram
each), operator 1 feedback (0–7), per-operator frequency ratio (Q8.8, 0.5–16), level
(0–127 → Q15 via `db_to_gain_q15` with the DX 96 dB range), and a per-operator `Adsr`
running in the **instrument** (control rate), whose output the instrument writes into
`GeneratorState::level` per tick (the mixer only multiplies). Phase modulation:
`out = sin_q15(phase + (modulator_out << MOD_SHIFT))`, `i32` throughout, the modulation
index calibrated so level 127 gives the DX7 maximum (~4π radians). Output clamped to
`i16` scale.

Determinism: phase is `u32`, wrapping; nothing depends on the block. The same note script
renders bit-identically on both paths and at every block size.

### 3. `FmInstrument: Instrument`

Per-voice state `{ voice, adsr: [Adsr; 4], patch: FmPatch, bend }` in the K1 pattern.
`note_on` computes the carrier step from the note through the linear-frequency table
(K1's path) and triggers a generator voice; `control_tick` advances four ADSRs and writes
the operator levels into the voice's `GeneratorState` through a new
`EngineContext::write_generator_levels(voice, &[i32; 6])` (a plain field write, no dirty
bit); `note_off` releases; `Done` stops. A `FmPatch` type with eight built-in patches
(electric piano, bass, bell, brass, organ, marimba, pad, lead — the DX7 sound of each,
not a copy of any ROM patch) and a `from_dx7_sysex`-shaped constructor left as a research
point. CC 74 → operator-2 level (brightness), CC 73/72 → carrier attack/release, CC 1 →
feedback.

### 4. Hosts

`SynthKind::Fm(patch)` in `Player::load_synth`, `play --synth fm[:patch]`, the web page's
synth select.

### 5. Proof

- `cargo xtask goldens --check` byte-identical (eleven).
- Block-size determinism with an FM note script, both paths; and **cross-path
  bit-identity** of the generator output: render the same script on `FixedPath` and
  `FloatPath` through a mono `f32`/`i16` output and assert the float samples are exactly
  the fixed samples scaled — no other float rounding enters (the pan law at centre and
  master volume at unity are exact powers of two; use `Limiter::Clamp`).
- Spectral: each algorithm with a single-operator sine carrier produces a pure tone; a
  2-op stack with index 1 has sidebands at the Bessel amplitudes ± 1 dB; feedback 7 on
  operator 1 produces a saw-like spectrum (harmonics at −6 dB/octave ± 2 dB).
- The IT resonant filter on a generator voice: an FM voice with `FilterParams` cutoff 40
  is low-passed (the filter arm is shared — prove it works on the generator arm too).
- RT-safety with a 64-voice FM burst; `no-std-purity`; `wasm-build`; `fma-check` (the
  generator is integer, but the arm is compiled into the float kernel — the audit must
  still pass).
- Voice size: a `const` assertion on `size_of::<Voice>()`.

### 6. Documentation

Architecture §7.1: "Voice rendering" gains the generator arm and decision 3; §5.3 the
`write_generator_levels` context call; §11 the synth crate's `fm` feature. Append
`## Research resolution`.

## Research points

1. **Where the ADSR runs**: instrument at control rate writing levels per tick (chosen —
   1 ms steps, smoothed by nothing but the tick rate) vs. an envelope inside the generator
   at frame rate. Measure whether a 1 ms level step on a bright patch clicks; if it does,
   add a per-frame linear ramp toward the written level inside the generator (a
   `GainRamp` per operator), which keeps the write at tick rate and the audio smooth.
2. **Modulation index calibration**: DX7's TL → modulation depth curve; derive the
   `MOD_SHIFT` and level table so that a DX7 E.PIANO-shaped patch sounds like one.
3. **DX7 sysex import**: 32-voice bulk dumps are well documented (128 bytes per voice);
   a `from_dx7_sysex` is cheap once the 6-op → 4-op question is answered — and it is not
   cheap (DX7 is 6-op, 32 algorithms). Decide whether `Fm4` should be `Fm6` from the
   start; `phase: [u32; 6]` above leaves room. Recommend and record.

## Verification

```
cargo test -p starplayer-mixer
cargo test -p starplayer-synth
cargo test -p starplayer-engine --test block_size_determinism
cargo test --workspace
cargo xtask goldens --check
cargo xtask ci --job host-tests
cargo xtask ci --job rt-safety
cargo xtask ci --job no-std-purity
cargo xtask ci --job wasm-build
cargo xtask ci --job fma-check
cargo xtask ci --job clippy
```

## Out of scope

SID (K3), Karplus-Strong (K6); a patch editor; oversampling the generator (aliasing at high
modulation is part of the sound and a later `Fm4Oversampled` kind if wanted); SIMD across
voices (an H6-style follow-up).
