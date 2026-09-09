# M10 — Alternative synths and enhancement plugins

| Field | Value |
|---|---|
| Goal | Non-sample instruments, and the sample-enhancement plugin API |
| Estimate | open-ended |
| Depends on | M4 (the `Instrument` trait) |
| Trigger | **Pull-driven, and deliberately open-ended.** Each item below is independently pullable; none blocks anything else |

## Context

The engine was designed so this milestone is *additive*: an instrument is a thing that
consumes events and drives voices (architecture §5.3), and nothing in the sequencer,
mixer or DSP graph knows whether a voice is playing a sample or synthesising one.

Architecture open question **Q4** is settled here: does the `Instrument` trait survive
contact with a non-sample instrument, or does it need a second tier? Answer it with the
first one built, not by speculation — the trait discipline rule (architecture §10.1)
applies here as much as anywhere.

## Candidate items

Each is a separate task file when pulled.

| Item | Notes |
|---|---|
| **Wavetable** | The simplest non-sample instrument; the right one to build first, precisely because it answers Q4 cheaply |
| **FM** | Operator-based; the natural second, and the one that will stress the control-rate model |
| **SoundFont (SF2)** | Bridges the sample and MIDI worlds; makes SMF playback genuinely useful. Should produce the same `InstrumentBank` as [M11](M11-master-plan.md), so the MIDI path has one instrument-source concept |
| **SID emulation** | 6581/8580; a self-contained and well-documented target with an enthusiastic audience |
| **Physical modelling** | Karplus-Strong upward; open-ended |
| **Granular / spectral** | Open-ended; likely wants a different control-rate story, so treat any finding as input to Q4 |
| **Sample enhancement API** | A plugin point for upscaling 8-bit sample data — classical interpolation, spectral band replication, or a learned model. The interesting constraint is that it must run **at load time**, off the audio thread, producing a normal `Module`; nothing in the RT path changes |

## The enhancement API's shape

Deliberately a *load-time* transform, not a runtime one. A `SampleEnhancer` takes decoded
PCM and returns decoded PCM at a possibly higher rate, and the `Module` builder applies
it. Consequences worth stating: the RT path is untouched, the result is deterministic and
hashable, an expensive or non-deterministic model is fine because it never runs in the
audio callback, and an enhancer can be a separate crate that does not have to be `no_std`.

Better still, run it during an [M11](M11-master-plan.md) library scan rather than per
module load: after content-hash dedup the same sample is enhanced once and reused across
every module that contains it, which in a real MOD library is a large multiplier.

## Exit criteria

None — this milestone is a backlog. Each pulled item has its own.

## The task graph (planned 2026-09-05)

Planned at the owner's request while M7 was landing. Task letter **K**. Each item is still
independently pullable; the order below is the one that answers Q4 cheapest first.
Decisions taken while planning:

1. **Q4 is answered by K1, and the expected answer is "the trait survives with one
   change".** `Instrument` today is `&self` because both sample instruments are immutable
   knowledge about a module; a wavetable morph, an ADSR and an FM operator stack all need
   per-voice state that outlives `note_on`. The format crates solve this with a parallel
   array indexed by `VoiceId::index()` and validated by the generation (§5.3), and that
   array needs `&mut self` on `control_tick`, `note_on`, `note_off` and `set_bend`. The
   rack owns each instrument as a `Box<dyn Instrument>`, so `&mut self` costs nothing.
   K1 makes that change with the first instrument that needs it and records it in §5.3;
   if the wavetable turns out not to need it, K1 says so and K2 makes it instead.
2. **Two ways to be a non-sample instrument, and both are used.** *Sample-backed*
   synthesis renders its waveforms into a `Module`'s PCM blob at build time (wavetables
   are single-cycle looped samples; SoundFonts are samples) and drives ordinary voices —
   nothing in the mixer changes and the goldens cannot move. *Generator* synthesis (FM,
   SID, Karplus-Strong) needs a per-frame oscillator, so K2 adds a `VoiceSource` to the
   mixer's `Voice`: `Sample(SampleRegion)` or `Generator(GeneratorState)`, a fixed-size
   enum of built-in generators with no `dyn` and no allocation, rendered by a second arm of
   `accumulate_voice` while the sample arm stays textually what it is. A generator voice
   still goes through the gain ramps, the IT resonant filter and the pan law, so an FM
   voice gets a resonant filter for free.
3. **Generators are integer-only.** A generator produces `i16`-scale `i32` samples from
   tables (`sin_q15`, `pow2_q24` from M7-H2) on both mix paths; the float path converts
   the integer sample. That keeps every generator bit-identical across targets and both
   paths, at the cost of the float path hearing 16-bit-quantised synthesis, which is what
   the hardware these emulate did anyway.
4. **A shared `Adsr` runner lives in `starplayer-synth`**, not in the engine: the engine
   still runs no envelope (§5.3). Format-driven MIDI instruments (M11) keep their formats'
   envelopes; synths use the ADSR. Advanced on `Instrument::control_tick`, so it runs at
   the MIDI source's ~1 ms control rate (§5.4).
5. **`InstrumentBank` is a `Module` with no patterns.** M11 and K4 (SoundFont) should share
   one "where instruments come from" concept; whichever lands first introduces
   `starplayer_model::InstrumentBank` as a newtype over `Module` (samples, instruments,
   `format_data`, no orders) plus a manifest, and the rack's `for_bank` beside
   `for_module`. K4 does not wait for M11.
6. **Enhancement is a load-time transform with two real implementations** (goal 8): a
   windowed-sinc upsampler and a loop-seam smoother, both deterministic. Goldens never see
   an enhancer; an enhanced render is a different configuration and gets a different name.
   *Amended by K5a, 2026-09-09*: the hook is `Module::enhanced(&dyn SampleEnhancer)` — a
   rebuild through the existing `ModuleBuilder`, not a builder field, because all five
   loaders make their own builder and a `&dyn` field would put a lifetime on every
   `&mut ModuleBuilder` helper. And `starplayer-enhance` is **`no_std + alloc`**, not
   std-capable: the sinc coefficients are a committed `f64` table with a regeneration
   gate, so the runtime needs only arithmetic and the crate is checked on the bare-metal
   target like every other core crate.

| ID | Task | Depends on | Parallel with | Model |
|---|---|---|---|---|
| K1 | [Wavetable instrument, the `Adsr` runner, and Q4](M10-task-K1-wavetable-and-q4.md) | M4 (landed), M7-H2 (tables) | K5 | Opus |
| K2 | [Generator voices in the mixer, and the FM instrument](M10-task-K2-generator-voices-and-fm.md) | K1 | K5 | Opus |
| K3 | [SID voice emulation as an instrument](M10-task-K3-sid-instrument.md) | K2 | K4, K6 | Opus |
| K4 | [SoundFont 2 → `InstrumentBank`](M10-task-K4-soundfont.md) | K1 | K3, K6 | Opus |
| K5 | The sample-enhancement API — pulled 2026-09-09 as [K5a](M10-task-K5a-enhancer-core.md) (trait, rebuild, playback scale, `starplayer-enhance`; Opus; **landed**), [K5b](M10-task-K5b-enhance-offline-and-cli.md) (CLI/offline; Sonnet) and [W4](../apps/W4-task-enhancement-checkboxes.md) (web checkboxes; Sonnet); [original text](M10-task-K5-sample-enhancement.md) | — | K1, K2 | Opus / Sonnet |
| K6 | [Karplus-Strong plucked string](M10-task-K6-karplus-strong.md) | K2 | K3, K4 | Sonnet |

```
K5 ──────────────────────────────────┐
K1 ──→ K2 ──→ K3 ∥ K6                │
   └──→ K4                            ┘
```

Granular and spectral synthesis stay a backlog row: their control-rate story (grain
scheduling at sub-millisecond rates, spectral frames at hop size) is the thing Q4 cannot
settle from these six, and a task file written now would be speculation.
