# Embedded budget — RAM, flash and CPU per voice

Written by M8-I3 (ESP32-A1S) and extended by M8-I4 (ESP32-C5). Until those land, every
number here is a placeholder marked `TBD`; do not quote a `TBD` anywhere else.

The point of this document is that a future project can size a target **before**
trying: given a module's channel count, sample rate and interpolator, what does it cost?

## Configuration measured

| Item | Value |
|---|---|
| Engine | `Engine<FixedPath, Interp, FixedOut<i16, 2>>` via `starplayer-host-embedded` |
| Sample rate | 44 100 Hz |
| `RENDER_QUANTUM` | 128 frames (architecture Q2 — see §5) |
| Toolchain | `esp` 1.97.0.0 (Xtensa); `rustup` 1.97 (RISC-V) |
| Profile | `release`, `opt-level = "s"` unless stated |

## 1. Flash

| Build | Text + rodata | Of which const tables | Notes |
|---|---|---|---|
| A1S, audio only, `Linear` | TBD | TBD (sinc table 4 KB if `Sinc` instantiated) | |
| A1S, `lcd` | TBD | | |
| A1S, web | TBD | | |
| C5 bench | TBD | | |

Module images: bytes per fixture, and the PCM share of each (I2 research point 2).

| Fixture | Image bytes | PCM bytes | Source file bytes |
|---|---|---|---|
| PETRI.S3M | TBD | TBD | 35 966 |
| REFLEX.S3M | TBD | TBD | 9 634 |

## 2. RAM

Static per engine (from `Engine::with_settings`), as a formula and as measured:

| Buffer | Formula | 4 channels | 16 channels | 32 channels |
|---|---|---|---|---|
| `buses` | `channels × 128 × 8 B` | 4 KB | 16 KB | 32 KB |
| accumulator + spill + muted | 3 KB | | | |
| output ring | `128 × 2 × 2 B` | 512 B | | |
| voices | `voice_capacity × size_of::<Voice>()` (TBD B each) | TBD | | |
| I2S DMA ring | `depth × 128 × 4 B` | TBD | | |
| Total measured heap after `open` | | TBD | TBD | TBD |

Peak stack of the audio task: TBD. Heap high-water mark with the web stack up: TBD.

## 3. CPU

Cycles per output frame, `release`, `PETRI.S3M` (TBD voices average) — fixed path:

| PCM location | `Nearest` | `Linear` | `Cubic` | `Sinc` |
|---|---|---|---|---|
| Flash (memory-mapped image) | TBD | TBD | TBD | TBD |
| PSRAM | TBD | TBD | TBD | TBD |
| DRAM | TBD | TBD | TBD | TBD |

Expressed as **% of one 240 MHz core at 44.1 kHz** and as **frames/s per voice**, so a
target with a different clock can be sized: TBD.

ESP32-C5 (single RISC-V core, PCM in flash): TBD.

## 4. Latency

DMA ring depth chosen, in frames and milliseconds, and the worst-case command-to-audio
latency (one quantum plus the ring): TBD.

## 5. Q2 — is 128 the right `RENDER_QUANTUM`?

TBD. Expected answer: yes; the number the embedded path actually wants to tune is the
I2S DMA ring depth, which is the host's own and independent of the quantum.
