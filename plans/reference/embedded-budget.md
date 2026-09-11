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

### C5 configuration (M8-I4)

| Item | Value |
|---|---|
| Board | ESP32-C5 devkit (RISC-V, single core, 240 MHz), no audio hardware, no PSRAM fitted |
| Target | `riscv32imac-unknown-none-elf` |
| Toolchain | plain `rustup` **`1.97`**, the main workspace's own pinned stable toolchain — **not** `esp-1.97` (research point 2; see §8) |
| Profile | the same `[profile.release]` as the A1S — one workspace, one profile table |
| Rustflags | `-C link-arg=-Tlinkall.x` only. No force-frame-pointers concern at all: that flag works around an Xtensa LLVM register-allocator bug this board's RISC-V backend does not have |
| Linker | rustc's self-contained `rust-lld` — no external cross-linker on `PATH` |

The C5 has no audio codec and no PSRAM on the devkit in hand, so it contributes no I2S/DMA
figures and no PSRAM row anywhere below — only flash, RAM and CPU, from the `bench` build
alone.

## 1. Flash

Section sizes from `xtensa-esp32-elf-size -A` (A1S) and `readelf -S` (C5 — no
`riscv32-esp-elf-size` was needed; `readelf` reads section headers independently of
architecture and the two agree with `cargo xtask size` to the byte). Image bytes from
`cargo xtask size --board <board> [--features bench]`, which is the app-partition image
espflash writes.

| Build | `.text` | `.rodata` | `.data` | `.rwtext` | App image | Of its partition |
|---|---|---|---|---|---|---|
| A1S, audio, `Linear` only | 268 233 | 146 924 | 9 296 | 15 820 | **464 944** | 17.7 % of 2.5 MB |
| A1S, `bench`, all four kernels | 355 841 | 169 708 | 4 400 | 15 732 | **552 560** | 21.0 % of 2.5 MB |
| A1S, `lcd` (M8-I5) | TBD | TBD | TBD | TBD | TBD | |
| A1S, `web` (M8-I6) | TBD | TBD | TBD | TBD | TBD | |
| C5, default (no module linked) | 25 486 | 9 492 | 788 | 1 784 | **39 904** | 2.5 % of 1.5 MB |
| C5, `bench`, all four kernels, all six fixtures | 300 150 | 163 336 | 1 356 | 1 784 | **468 976** | 29.8 % of 1.5 MB |

The C5's `factory` partition (1.5 MB, `boards/starplayer-c5/partitions.csv`) is smaller
than the A1S's (2.5 MB) because there is no OTA and no reason to match the other board's
budget; the two "of its partition" percentages are not comparable to each other for that
reason, only each board's own two rows are. The C5's `bench` image (468 976 B) is
smaller than the A1S's `bench` image (552 560 B) despite linking the **same six module
images** and the **same four kernels**, because it links no codec driver, no I2S, no DMA
and no board pin map — there is nothing on this chip for any of those to drive.

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

### Type sizes, measured **on target** — Xtensa and RISC-V32 alike

Not read off a host build. The A1S's column was obtained by compiling
`const _: [(); 0] = [(); size_of::<T>()];` into the firmware for `xtensa-esp32-none-elf`
and reading the size rustc reported back. The C5's column needed no board and no full
firmware link: a `#[used] static` holding the three `size_of::<T>()` values was compiled
to an object file for `riscv32imac-unknown-none-elf` (`cargo build --crate-type=lib`, no
link step at all) and its bytes read back with `readelf -x` — a compiler fact, not a
device reading, and reproducible with no hardware.

| Type | Xtensa (A1S) | RISC-V32 (C5) | x86-64 (host, I1) |
|---|---|---|---|
| `starplayer_mixer::Voice` | **176 B** | **176 B** | 176 B |
| `starplayer_telemetry::Snapshot` | **1 848 B** | **1 848 B** | 2 616 B |
| `RenderHalf<Linear>` | **8 344 B** | **8 344 B** | 11 744 B |

Xtensa and RISC-V32 agree on every field, exactly — both are 32-bit little-endian targets
with 4-byte pointers and the same `repr` rules apply, so nothing in these types' layout is
architecture-sensitive. `Snapshot` is 30 % smaller than the host's build on **either**
embedded target — it is 64 `ChannelState`s plus a header, and a 32-bit target packs them
tighter — and `RenderHalf` carries two of them, which is why it is 3 400 bytes smaller too.
It is still far too large to pass by value: the A1S keeps its long-lived one in a `static`
through `StaticCell`. The C5's `bench` build never keeps one *long-lived* — `digest_row`
builds and drops an engine (and its `RenderHalf`) fresh for every row — so nothing on this
board needed the same treatment; its `SIZE …` line prints the same compile-time figure
purely for the same cross-board comparison the A1S's line supports.

### Heap, per engine

The formula is I1's, measured there by differencing two engines:

```text
heap bytes = 27 800 + 184 × voice_capacity + 5 288 × channel_count
```

It was measured on the host, where `Snapshot` is 2 616 bytes, and two of its three
constants carry a `Snapshot` inside them — so on this board the real figures are **smaller**
and the formula is a safe upper bound rather than an exact answer. The device's own number
is printed by the bench build (`HEAP [...]`) and by the audio build (`HEAP after open`).
The same is true of the C5 — both embedded targets' `Snapshot` is 1 848 bytes, not the
formula's 2 616 — so the same "safe upper bound" reading applies there too, and the same
`HEAP […]` lines carry the C5's real figure once it is run.

| Module | Channels | Voices | Host formula | A1S device measured | C5 device measured |
|---|---|---|---|---|---|
| `REFLEX.S3M` | 3 | 3 | 44 216 B | TBD (owner: run the bench build) | TBD (owner: run the bench build) |
| `PETRI.S3M` | 8 | 8 | 71 576 B | TBD (owner: run the bench build) | TBD (owner: run the bench build) |
| a 32-channel module, 64-voice pool | 32 | 64 | 209 kB | — | — |

**Why the C5's `bench` heap is 176 KiB and not smaller.** The `bench` build's own
`DRAM_STAGING_BYTES` (32 KiB, `boards/starplayer-c5/src/bench.rs`) is carved out of this
*same* heap — `Staging::claim` calls `esp_alloc::HEAP.alloc_caps`, not a separate static,
exactly as the A1S's does — so the budget that has to hold at once is the staging buffer
**and** the largest fixture's engine, not either alone: `32 768 + 71 576 = 104 344` bytes
at the host formula's upper bound. 176 KiB (180 224 B) was chosen to leave close to double
that as headroom (`boards/starplayer-c5/src/main.rs`'s `HEAP_BYTES` doc comment has the
arithmetic in full) against both the formula being an upper bound and ordinary allocator
overhead — a margin chosen deliberately generous, because a heap that is merely *adequate*
here would risk an out-of-memory abort on exactly the fixture (`PETRI.S3M`) the milestone's
exit criterion is stated in terms of, on a board with no debugger attached to diagnose it
from. This is arithmetic, not a device reading, and the "C5 device measured" column above
is where the real number belongs once it exists.

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

### Static RAM, the C5's `bench` (M8-I4)

From `readelf -S` on the linked ELF. Unlike the A1S's table above, "Main stack" here is
not a hand computation against a fixed top-of-RAM address — `esp-hal`'s `linkall.x`
allocates the C5's `.stack` section as *whatever RAM is left* after every other section, so
the linker's own `.stack` size **is** the figure, read the same way as everything else in
this table.

| Item | Default build | `bench` build |
|---|---|---|
| `.bss` total | 180 636 B | 180 636 B |
| — of which the heap array | 180 224 B | 180 224 B |
| — everything else | 412 B | 412 B |
| `.data` | 788 B | 1 356 B |
| **`.stack`** (linker-allocated, the rest of the `RAM` region) | **135 724 B** | **134 644 B** |

`.bss` and its heap share are identical between the two builds because `HEAP_BYTES` is a
constant unconditional on the `bench` feature — unlike the A1S, where the audio build's
heap (122 880 B, 120 KiB) and the bench build's are the same number for the same reason,
but the two boards chose different constants (§2's heap note has the C5's arithmetic).
Two things this table says that the A1S's equivalent could not, because the A1S's stack is
computed by hand against a hard-coded top-of-RAM address while the C5's is read straight
off the linker: **the stack was never the constraint here.** Even the `bench` build, with
its 176 KiB heap and 32 KiB staging buffer both live at once, leaves over 130 KiB of stack
— more than twice the A1S's *entire* budget for its own 120 KiB heap. This is a build-time
fact (what the linker allocated), not a device reading of what the bench routine's
recursion or the golden-render call stack actually used; the peak the firmware really
reaches stays `TBD` below, exactly as the A1S's does.

| Measurement | Value |
|---|---|
| Peak stack of the `bench` routine | TBD (owner: run the `bench` build — a stack-guard overflow would abort partway through the transcript; a clean run through all six fixtures is the evidence the linker's 134 644 B is enough, and it plainly is) |

## 3. CPU

Cycles per output frame, `release`, stereo `FixedOut<i16, 2>`, from the bench transcript's
`cycles_per_frame=` field. One row per PCM location, one column per interpolator.

### A1S (Xtensa LX6, 240 MHz)

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

### C5 (RISC-V, single core, 240 MHz, no audio hardware) — M8-I4

Same units, same `core_load=` formula (`cycles_per_frame × 44 100 ÷ 240 000 000`), and
directly comparable row for row with the A1S's tables above — that comparability, not
sound, is this board's entire reason to exist. There is no PSRAM column: the devkit has
none fitted (research point 1).

`PETRI.S3M` (8 channels):

| PCM location | `Nearest` | `Linear` | `Cubic` | `Sinc` |
|---|---|---|---|---|
| Flash (memory-mapped image) | TBD (owner: run the bench build) | TBD | TBD | TBD |
| DRAM | — see below | — | — | — |

`REFLEX.S3M` (4 channels):

| PCM location | `Nearest` | `Linear` | `Cubic` | `Sinc` |
|---|---|---|---|---|
| Flash | TBD | TBD | TBD | TBD |
| DRAM | TBD | TBD | TBD | TBD |

`PETRI.S3M` has no DRAM row on this board either, and for the same reason as the A1S: its
88 036-byte image does not fit `DRAM_STAGING_BYTES` (32 KiB) — see §2's heap note. The
bench prints `BENCH petri-s3m <kernel> dram skipped=no staging buffer large enough` here
too.

**The comparison this table is for**, once both boards' `Flash`/`Linear` cells are filled:
one 240 MHz Xtensa LX6 core against one 240 MHz RISC-V core, same fixed-point mixer, same
compiler family, same `-O3`/thin-LTO codegen policy — the only free variable is the ISA and
its compiler backend. `TBD` until the owner runs both.

### How the cycles are counted

**A1S: not from `CCOUNT`.** The Xtensa cycle-count special register is 32 bits and wraps
every 17.9 seconds at 240 MHz, which is shorter than a ten-second render could plausibly
take for the sinc kernel over a sample-heavy module — so a raw difference could be wrong by
a multiple of 2³² with nothing on the line to show it. The bench reads esp-hal's 64-bit
**microsecond** timer and multiplies by the configured CPU frequency
(`esp_hal::clock::cpu_clock().as_mhz()`), which at 240 MHz is exactly 240 cycles per
microsecond and cannot wrap. Over a multi-second render the quantisation error is under a
millionth.

**C5: directly from `mcycle`, no derivation needed.** RISC-V's `mcycle` CSR is
architecturally 64 bits everywhere — on `riscv32imac` the low and high halves are two
separate 32-bit CSRs (`mcycle`/`mcycleh`) — and `riscv::register::mcycle::read64()` already
assembles a consistent 64-bit value across a wrap with the standard "read high, read low,
re-read high, retry if it changed" sequence. `boards/starplayer-c5/src/bench.rs`'s
`CycleClock::now` is one function call; unlike the A1S there is no microsecond-timer
derivation to get wrong, and no wraparound this board could reach in one boot's lifetime.

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

**This whole section is A1S-only.** The C5 has no I2S, no DMA ring and no audio output at
all (M8-I4, out of scope) — there is no latency path on this board to measure. Its `bench`
build's two renders both use `BLOCK_FRAMES = 128` (`boards/starplayer-c5/src/bench.rs`),
the same `RENDER_QUANTUM`, purely so the cycle figures in §3 are taken at the same block
size the A1S's are and stay comparable — not because anything on the C5 consumes blocks.

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

### The C5 (M8-I4) — one flash, no listening check

```sh
cd embedded

# no ". ~/export-esp-1.97.sh" needed — the C5 build uses the main workspace's plain
# rustup toolchain (§8), not the Xtensa one. `. ~/export-esp-1.97.sh` does no harm if it
# is already sourced from an A1S session in the same shell; it is simply not required.
cargo xtask flash --board c5 --features bench
cargo xtask monitor --board c5 | tee /tmp/starplayer-c5-bench.log
```

There is only the one flash — the C5 has no "audio build" to listen to. From that log:

| Log line | Goes to |
|---|---|
| `SIZE voice=… render_half=…` | §2, "Type sizes" — confirms the C5 column against the compile-time probe |
| `HEAP [boot] …` / `HEAP [<fixture>] …` | §2, "Heap, per engine" — the "C5 device measured" column |
| `STAGING dram=… psram=unavailable …` | confirms the no-PSRAM finding (§8 / research point 1) |
| `BENCH <fixture> <kernel> <location> … cycles_per_frame=… core_load=…` | §3, the C5 CPU tables |
| `BENCH … linear flash sha256=…` | **the exit criterion**, same as the A1S's — must equal `goldens/<format>/<stem>__i16_mono_44100_linear.sha256` |
| `BENCH … skipped=…` | §3, the C5 DRAM note |

If any `sha256=` line disagrees with its `goldens/` entry, that is the finding this whole
board exists to surface — a kernel or a mixer path that is not bit-identical between
Xtensa and RISC-V, which architecture invariant 5 (bit-identical fixed-point output across
x86/ARM/WASM) already commits to, and this milestone is where RISC-V joins that claim.

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
* **The C5's `bench` heap is 176 KiB, not the A1S's 120 KiB, and the reason is the board,
  not the engine.** The C5 has no PSRAM staging region to take the pressure off internal
  RAM the way the A1S's `External`-capability region does for its own `bench` build — every
  byte the C5's `bench` needs, `DRAM_STAGING_BYTES` and every fixture's engine alike, comes
  out of the one internal heap. §2 has the arithmetic; the chip's own RAM (≈313 KiB in the
  primary region, `ld/esp32c5/memory.x`) has room to spare either way.
* **The C5 has no dual-core question at all.** It is a single RISC-V core — there is no
  "which core does the bench run on" the way the A1S's core-pinning finding (M8-I3) had to
  settle, because there is only the one.
* **`riscv32imac-unknown-none-elf` needed no `-Z build-std`**, unlike `xtensa-esp32-none-elf`.
  It is a plain LLVM target with a prebuilt `core`/`alloc` component distributed by
  `rustup target add` — confirmed by building the C5's whole dependency graph (esp-hal,
  esp-alloc, `starplayer`, …) under the **plain stable** `1.97` toolchain the main
  workspace already pins, with no build-std flag anywhere in `boards/starplayer-c5/.cargo/config.toml`.
  See §8.
* **No external cross-linker either.** `riscv32imac-unknown-none-elf` links with rustc's
  self-contained `rust-lld`; `boards/starplayer-c5/.cargo/config.toml` names only
  `-C link-arg=-Tlinkall.x`, and `embedded/xtask`'s `Board::linker` is `None` for this
  board because there is nothing to check for on `PATH`.

## 8. Toolchain (M8-I4 research point 2)

**Plain `rustup` was preferred, and it needed no fallback.** The task file's two candidate
paths were "the same `esp-1.97` toolchain (simplest)" or "plain stable `rustup` … if the
HAL builds on stable + build-std-free". The second one won outright:

* `riscv32imac-unknown-none-elf` has a prebuilt `core`/`alloc` under plain `rustup` (`rustup
  target add riscv32imac-unknown-none-elf` against the toolchain the main workspace's own
  `rust-toolchain.toml` pins — now `1.97`, with that target added to its `targets` list
  alongside the existing `riscv32imc-unknown-none-elf`).
* The named `esp-1.97` toolchain does **not** carry a prebuilt component for this target at
  all (`ls ~/.rustup/toolchains/esp-1.97/lib/rustlib` has only its own host triple and
  `rust-src` — everything else it builds from source via `-Z build-std`, which is how the
  A1S's Xtensa target works). Building the C5 under `esp-1.97` would therefore have needed
  `-Z build-std` for the C5 too, for no benefit: the target itself is unaffected by which
  toolchain builds it, since `riscv32imac-unknown-none-elf` is the same LLVM target triple
  either way.
* **What made this possible on the code side**: the C5 board crate takes no dependency on
  `embassy-executor` or `esp-rtos` at all — `#[esp_hal::main]` is a synchronous, non-async
  entry point (`esp-hal`'s `blocking_main` procedural macro), so nothing in the crate's
  dependency graph reaches for `embassy-executor`'s `nightly` feature (`impl_trait_in_assoc_type`),
  which is the language feature that would have forced a nightly compiler regardless of the
  target's own prebuilt-component story. The A1S needs `esp-rtos` for its DMA refill task
  running concurrently with a control task; the C5 renders six fixtures once at boot and
  idles — there is nothing to schedule, so there was nothing pulling in the async story at
  all.
* **`embedded/xtask` builds the two boards under two different named toolchains as a
  result** (`Board::toolchain`: `"esp-1.97"` for the A1s, `"1.97"` for the C5), both invoked
  explicitly with `cargo +<toolchain>` from the board's own directory. `embedded/rust-toolchain.toml`
  still pins `esp-1.97` at the workspace root — the C5's `+1.97` argument overrides that
  per-invocation, exactly the way a `+toolchain` argument is documented to.
* One CI consequence, spelled out in `.github/workflows/embedded.yml`: the C5 job installs
  **only** the main workspace's own pinned toolchain (`rustup show` against the repository
  root's `rust-toolchain.toml`) — no `esp-rs/xtensa-toolchain` action, no second toolchain
  install step at all. That step alone is also what the C5 board build's `+1.97` resolves
  to, and what generates the module images before either board's build runs.
