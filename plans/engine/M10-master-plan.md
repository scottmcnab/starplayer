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
