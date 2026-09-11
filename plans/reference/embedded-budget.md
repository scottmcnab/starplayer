# Embedded budget — RAM, flash and CPU per voice

Written by M8-I3 (ESP32-A1S) and extended by M8-I4 (ESP32-C5).

The point of this document is that a future project can size a target **before** trying:
given a module's channel count, sample rate and interpolator, what does it cost?

**Read the marks.** Every number below is one of three things, and they must not be
confused:

| Mark | Means |
|---|---|
| a plain number | measured, by the command named beside it, and reproducible today |
| `TBD (owner: run the bench build)` | needs the hardware; the exact command and where its output goes are in §6 |
| `—` | not applicable to this row |

No number here is estimated. A figure that could not be measured says `TBD`; nothing is
inferred from a host build and then quoted as if it came off the device. Do not quote a
`TBD` anywhere else.

## Configuration measured

| Item | Value |
|---|---|
| Engine | `Engine<FixedPath, Interp, FixedOut<i16, 2>>` via `starplayer-host-embedded` |
| Sample rate | 44 100 Hz |
| `RENDER_QUANTUM` | 128 frames (architecture Q2 — see §5) |
| Board | AI-Thinker ESP32-Audio-Kit, ESP32-A1S (classic ESP32, Xtensa LX6, 2 × 240 MHz, 4 MB flash, PSRAM) |
| Toolchain | `esp-1.97` (rustc 1.97.0-nightly, Xtensa); `rustup` 1.97 for the host |
| Profile | `release`: `opt-level = 3`, `codegen-units = 1`, `lto = "thin"`, `debug = true`, `panic = "abort"` |
| Rustflags | `-C link-arg=-Tlinkall.x`. **No** `-C force-frame-pointers` — see §7 |

The profile is `opt-level = 3`, not the `"s"` a size-constrained firmware would use. The
CPU figures below are a measurement of the mixer, and a figure taken at `"s"` would be a
measurement of the size/speed tradeoff instead. Flash is not the constraint here: the
`factory` partition is 2.5 MB and the audio build uses 17.7 % of it.

## 1. Flash

Section sizes from `xtensa-esp32-elf-size -A`; image bytes from
`cargo xtask size --board a1s [--features bench]`, which is the app-partition image
espflash writes.

| Build | `.text` | `.rodata` | `.data` | `.rwtext` | App image | Of the 2.5 MB partition |
|---|---|---|---|---|---|---|
| A1S, audio, `Linear` only | 268 233 | 146 924 | 9 296 | 15 820 | **464 944** | 17.7 % |
| A1S, `bench`, all four kernels | 355 841 | 169 708 | 4 400 | 15 732 | **552 560** | 21.0 % |
| A1S, `lcd` (M8-I5) | TBD | TBD | TBD | TBD | TBD | |
| A1S, `web` (M8-I6) | TBD | TBD | TBD | TBD | TBD | |
| C5 bench (M8-I4) | TBD | TBD | TBD | TBD | TBD | |

`.rodata` includes the linked module images, which are the bulk of it: 88 036 bytes in the
audio build (`PETRI.S3M` alone) and 117 040 in the bench build (all six fixtures). Net of
those, non-image `.rodata` is **58 888** (audio) and **52 668** (bench).

Three things those figures do and do not say, and it is worth being explicit:

* **The bench build is not "the audio build plus three kernels".** It contains no codec
  driver, no I2S, no DMA and no board module; the audio build contains no SHA-256 and no
  mono `FixedOut<i16, 1>` instantiation. The two `.rodata` figures therefore cannot be
  differenced to price the sinc table, and the bench build's *smaller* non-image `.rodata`
  is esp-hal's I2C/I2S/GPIO tables being absent rather than anything about kernels.
* **`.text` can be differenced, and the answer is large: +87 608 bytes.** Every
  interpolator monomorphises the whole voice-accumulation path, so three extra kernels plus
  the mono output stage plus `sha2` plus the bench's own formatting cost a third as much
  again as the entire audio firmware. That is a concrete answer to I1 research point 4's
  follow-up — whether the `linear-interp` / `float-mix` features are worth making real.
  **They are not**: the audio build instantiates `Linear` alone, and the other three
  kernels and both float paths are simply not in the binary, with no feature flag involved.
  Monomorphisation and linker GC already do the job the features were imagined for.
* `SINC_TABLE_Q15` appears as a named symbol in **neither** ELF: thin LTO merges it into an
  anonymous `.rodata` constant. Its 4 096 bytes are inside the bench build's non-image
  `.rodata` and are provably absent from the audio build's, which never instantiates `Sinc`.

### Module images

From `cargo xtask module-images` (M8-I2), which is what `embedded/assets/*.spmi` holds:

| Fixture | Image bytes | PCM bytes | PCM share | Source module |
|---|---|---|---|---|
| `petri-s3m` | 88 036 | 64 156 | 72.9 % | 35 966 |
| `reflex-s3m` | 14 984 | 4 480 | 29.9 % | 9 634 |
| `synthetic-mod` | 6 072 | 1 376 | 22.7 % | 3 324 |
| `synthetic-xm` | 3 564 | 448 | 12.6 % | 3 449 |
| `synthetic-mtm` | 2 312 | 448 | 19.4 % | 1 548 |
| `synthetic-it` | 2 072 | 448 | 21.6 % | 1 797 |

An image is larger than its source module because the loader's work is done ahead of time:
patterns are decoded into the engine's own blob, and 8-bit PCM is widened to `i16`. The
widening is where an `i8` sample storage would pay — 32 078 bytes on `PETRI.S3M`, 0.8 % of
this board's flash (M8-I2 research point 2). It is not implemented.

## 2. RAM

### Type sizes, measured **on the Xtensa target**

Not read off a host build. Each was obtained by compiling
`const _: [(); 0] = [(); size_of::<T>()];` into the firmware for `xtensa-esp32-none-elf`
and reading the size rustc reported back.

| Type | Xtensa (this board) | x86-64 (host, I1) |
|---|---|---|
| `starplayer_mixer::Voice` | **176 B** | 176 B |
| `starplayer_telemetry::Snapshot` | **1 848 B** | 2 616 B |
| `RenderHalf<Linear>` | **8 344 B** | 11 744 B |

`Snapshot` is 30 % smaller on Xtensa — it is 64 `ChannelState`s plus a header, and the
32-bit target packs them tighter — and `RenderHalf` carries two of them, which is why it
is 3 400 bytes smaller too. It is still far too large to pass by value: the firmware keeps
it in a `static` through `StaticCell`, as I1 recommends.

### Heap, per engine

The formula is I1's, measured there by differencing two engines:

```text
heap bytes = 27 800 + 184 × voice_capacity + 5 288 × channel_count
```

It was measured on the host, where `Snapshot` is 2 616 bytes, and two of its three
constants carry a `Snapshot` inside them — so on this board the real figures are **smaller**
and the formula is a safe upper bound rather than an exact answer. The device's own number
is printed by the bench build (`HEAP [...]`) and by the audio build (`HEAP after open`).

| Module | Channels | Voices | Host formula | Device measured |
|---|---|---|---|---|
| `REFLEX.S3M` | 3 | 3 | 44 216 B | TBD (owner: run the bench build) |
| `PETRI.S3M` | 8 | 8 | 71 576 B | TBD (owner: run the bench build) |
| a 32-channel module, 64-voice pool | 32 | 64 | 209 kB | — |

### Static RAM, this firmware

From `xtensa-esp32-elf-size -A` and `xtensa-esp32-elf-nm`.

| Item | Audio build | Bench build |
|---|---|---|
| `.bss` total | 139 888 B | 123 000 B |
| — of which the heap array | 122 880 B | 122 880 B |
| — everything else (`RenderHalf`, the DMA ring and descriptors, task storage) | 17 008 B | 120 B |
| `.data` | 9 296 B | 4 400 B |
| **Main stack** (`0x3ffe_0000 − _bss_end`) | **47 424 B** | **69 208 B** |

The stack is the real constraint, and it is not obvious: on the classic ESP32 the main
stack is simply whatever internal DRAM is left over, so **every heap byte is a stack byte**.
A 160 KiB heap still links and leaves 6 472 bytes of stack, which will not survive a boot.
120 KiB is the balance this firmware ships; `embedded/README.md` §6 has the one-line check
to run after any change that moves a large static.

The I2S DMA ring is `DMA_RING_QUANTA × 128 × 4` bytes = **4 096 B** at the shipped depth of
8 quanta, plus one descriptor per 512-byte chunk.

| Measurement | Value |
|---|---|
| Peak stack of the audio task | TBD (owner: run the audio build — esp-hal's stack-guard watchpoint fires on an overflow, and a clean run through a whole song is the evidence that 47 424 B is enough) |
| Heap high-water with the web stack up | TBD (M8-I6) |

## 3. CPU

Cycles per output frame, `release`, stereo `FixedOut<i16, 2>`, from the bench transcript's
`cycles_per_frame=` field. One row per PCM location, one column per interpolator.

`PETRI.S3M` (8 channels):

| PCM location | `Nearest` | `Linear` | `Cubic` | `Sinc` |
|---|---|---|---|---|
| Flash (memory-mapped image) | TBD (owner: run the bench build) | TBD | TBD | TBD |
| PSRAM | TBD | TBD | TBD | TBD |
| DRAM | — see below | — | — | — |

`REFLEX.S3M` (4 channels):

| PCM location | `Nearest` | `Linear` | `Cubic` | `Sinc` |
|---|---|---|---|---|
| Flash | TBD | TBD | TBD | TBD |
| PSRAM | TBD | TBD | TBD | TBD |
| DRAM | TBD | TBD | TBD | TBD |

**`PETRI.S3M` has no DRAM row, and that is a result rather than a gap.** Its image is
88 036 bytes; internal DRAM on this chip is one segment of about 176 KB, of which this
firmware's heap is already 120 KiB. The exit criterion's own module cannot be held in DRAM
while an engine capable of playing it also exists. That is precisely why flash-resident,
borrowed PCM (M8-I2) was worth building, and the bench prints
`BENCH petri-s3m <kernel> dram skipped=no staging buffer large enough` rather than quietly
measuring something smaller.

Each line also carries `core_load=`, the share of one 240 MHz core at 44 100 Hz:
`cycles_per_frame × 44 100 ÷ 240 000 000`. Frames per second per voice follows from the
same figure. TBD until the run.

ESP32-C5, single RISC-V core, PCM in flash (M8-I4): TBD.

### How the cycles are counted

**Not from `CCOUNT`.** The Xtensa cycle-count special register is 32 bits and wraps every
17.9 seconds at 240 MHz, which is shorter than a ten-second render could plausibly take for
the sinc kernel over a sample-heavy module — so a raw difference could be wrong by a
multiple of 2³² with nothing on the line to show it. The bench reads esp-hal's 64-bit
**microsecond** timer and multiplies by the configured CPU frequency
(`esp_hal::clock::cpu_clock().as_mhz()`), which at 240 MHz is exactly 240 cycles per
microsecond and cannot wrap. Over a multi-second render the quantisation error is under a
millionth.

## 4. Latency

| Item | Frames | Milliseconds |
|---|---|---|
| `RENDER_QUANTUM` | 128 | 2.90 |
| I2S DMA ring, as shipped (`DMA_RING_QUANTA = 8`) | 1 024 | 23.2 |
| Worst-case command-to-audio (one quantum + the ring) | 1 152 | 26.1 |

The depth that was **chosen** is 8. The depth the board actually *needs* — research point
4's measurement, the smallest that never underruns with the radio off — is TBD (owner: run
the audio build at `DMA_RING_QUANTA` = 2, 4 and 8 and watch the `underruns=` field on the
once-a-second transport line). The intent is to take the smallest clean depth and double it
for M8-I6's headroom; 8 is a deliberately generous starting point, not a result.

## 5. Q2 — is 128 the right `RENDER_QUANTUM`?

**Yes, and this path does not want a compile-time override.** Recorded in
`plans/product/01-technical-architecture.md` §12.

Three reasons, all visible in this firmware:

1. **The quantum is not the latency knob; the DMA ring is.** 128 frames is 2.9 ms, and the
   ring is eight times that. Anything a host wants to tune about responsiveness it tunes by
   changing the ring depth, which it owns outright and which costs 512 bytes of DRAM per
   quantum of depth.
2. **128 frames × 4 bytes = 512 bytes is a natural DMA chunk on this chip** — well inside
   the 4 095-byte descriptor limit, and it lets each descriptor be exactly one quantum, so
   the DMA's available-space figure is a whole number of quanta and the control cadence and
   the DMA boundary coincide. A smaller quantum would multiply descriptors; a larger one
   would coarsen the boundary a seek or a stop can land on.
3. **The RAM a smaller quantum would save is not where the RAM goes.** The engine's
   quantum-sized buffers are `channels × 128 × 8` bytes — 8 kB on `PETRI.S3M`'s eight
   channels — against a fixed 27.8 kB of telemetry and pool overhead and 88 kB of module
   image. Halving the quantum would save 4 kB and double the per-block control overhead.

The one result that would reopen the question is a cycles-per-frame figure showing the
per-quantum fixed cost dominating the per-sample cost, which the §3 table will settle.

## 6. What the owner runs, and where the output goes

Two flashes, in this order. **Agents do not run these.**

```sh
cd embedded
. ~/export-esp-1.97.sh

# 1. the bench — this transcript is this document's data
cargo xtask flash --board a1s --features bench
cargo xtask monitor --board a1s | tee /tmp/starplayer-bench.log
```

Then, from that log:

| Log line | Goes to |
|---|---|
| `SIZE voice=… render_half=…` | §2, "Type sizes" — confirms the compile-time probe |
| `HEAP [boot] …` / `HEAP [<fixture>] …` | §2, "Heap, per engine" — the "Device measured" column |
| `BENCH <fixture> <kernel> <location> … cycles_per_frame=… core_load=…` | §3, the CPU tables |
| `BENCH … linear flash sha256=…` | **the exit criterion** — must equal `goldens/<format>/<stem>__i16_mono_44100_linear.sha256` |
| `BENCH … skipped=…` | §3, the DRAM note |

```sh
# 2. the audio build — the listening check
cargo xtask flash --board a1s
cargo xtask monitor --board a1s
```

| Log line | Goes to |
|---|---|
| `I2C device at 0x…` | M8-I3 research point 1 — confirms the board revision |
| `HEAP after open: …` | §2, "Heap, per engine" |
| `… underruns=N dma_errors=N` on the transport line | §4, the ring-depth measurement |

And the listening check itself: `PETRI.S3M` through the headphone jack, clean, with the
stereo image the right way round. If the channels are swapped, set `SWAP_CHANNELS` in
`embedded/boards/starplayer-a1s/src/audio.rs` and rebuild — the constant exists for exactly
that.

## 7. Things that shaped the numbers above

* **`-C force-frame-pointers` had to be dropped.** With it on, the firmware does not
  compile: `rustc-LLVM ERROR: Error while trying to spill A8 from class AR: Cannot scavenge
  register without an emergency spill slot`. Ampkeeper hit the same Xtensa LLVM bug from
  the other direction on the ESP32-S3. The cost is less reliable panic backtraces.
* **PSRAM is mapped but not added to the general heap** in the audio build. The ESP32's
  atomic instructions do not work correctly on memory in PSRAM (esp-alloc documents the
  erratum for the ESP32, S2 and S3), and this engine puts atomics on the heap — `Arc`
  reference counts, the host's seqlocks, the telemetry ring. A heap region that could hand
  one of those out of PSRAM is a heap that can silently corrupt a reference count. The
  bench build does register it, because it has no real-time path and asks for external
  memory explicitly by capability. **M8-I6 inherits this**: an uploaded module's *sample
  data* may live in PSRAM; the `Arc` that owns it may not.
* **The audio task runs on core 0.** `RenderHalf` is not `Send`, because `Engine` holds a
  `Box<dyn EventSource>` with no `+ Send` bound, so it cannot be handed to esp-rtos's
  second-core `SendSpawner`. Nothing in this milestone competes for core 0, so every figure
  above is a single-core figure with an idle second core. See the M8-I3 research
  resolution.
