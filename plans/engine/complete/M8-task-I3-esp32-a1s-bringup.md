# M8 — I3: ESP32-A1S bring-up — the `embedded/` workspace, ES8388 over I2S, a module from flash

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](../M8-master-plan.md), decisions 1–3 and 6–7; deliverables 3–5). **The milestone's exit criterion** |
| Status | **Implemented 2026-09-11; builds link; awaiting owner hardware run** — see [Research resolution](#research-resolution) and the owner steps at the end |
| Depends on | I1 (`starplayer-host-embedded`), I2 (module images) |
| Blocks | I4, I5, I6 |
| Parallel with | — |
| Recommended model | Claude Opus (hardware bring-up with three real unknowns — MCLK, the codec init sequence, DMA cadence — and the budget measurements the whole milestone exists to produce) |
| Verified by | agent (both boards' firmware builds in CI-equivalent commands; the bench build's UART transcript with golden hashes equal to `goldens/`), then **owner** flashes and listens |

## Context for a fresh agent

Everything below the HAL exists after I1 and I2: `starplayer-host-embedded` gives an
`i16` render entry point with the control cadence, a mailbox and a bench digest;
`Module::from_image` borrows a module out of a `'static` slice. This task puts them on
the AI-Thinker ESP32-Audio-Kit and makes sound.

The board is a classic ESP32 (Xtensa LX6, dual core, 240 MHz, 4 MB flash, PSRAM) with
an **ES8388** codec. The Rust stack is the one `../ampkeeper/esp32` proves daily on the
S3: esp-hal 1.1.1, esp-rtos 0.3 (the Embassy executor), esp-println, esp-backtrace,
esp-alloc, esp-bootloader-esp-idf, embassy-time/-sync, espflash. ampkeeper's classic-ESP32
era is the template for the vanilla-chip variant of that stack: read
`git -C ../ampkeeper show aeb9039^:esp32/firmware/Cargo.toml` (the
`[target.xtensa-esp32-none-elf.dependencies]` table at line ~228) and
`git -C ../ampkeeper show aeb9039^:esp32/firmware/.cargo/config.toml` (`-Tlinkall.x`,
`-C force-frame-pointers`, `build-std`). The toolchain is installed
(`~/.rustup/toolchains/esp`, `espflash 3.3.0`); it must be upgraded to
`espup install --toolchain-version 1.97.0.0` because the main workspace pins
`rust-version = "1.97"` and cargo refuses older compilers (research point 0 — do this
first, it is the cheapest way to lose a day).

The firmware is its own workspace (master-plan decision 1), directory-scoped to the `esp`
toolchain exactly as `../ampkeeper/esp32/firmware/rust-toolchain.toml` is, so the main
workspace stays on stable 1.97 and `cargo xtask ci` never sees a `xtensa` target.

The **pin map** below is the one every Arduino/ESP-ADF port of the v2.2 Audio Kit uses.
It is research point 1 to confirm against the schematic and an I2C scan before wiring
anything; the ES8388 answers at `0x10`, an AC101 (the older revision, out of scope) at
`0x1A`.

| Function | GPIO | Note |
|---|---|---|
| I2C SDA / SCL | 33 / 32 | codec control |
| I2S MCLK | 0 | `CLK_OUT1`; a strapping pin — MCLK output only after boot |
| I2S BCLK / LRCK | 27 / 25 | |
| I2S DOUT (ESP → codec DSDIN) | 26 | |
| I2S DIN (codec ASDOUT → ESP) | 35 | unused; input-only pin |
| PA enable (speaker amp) | 21 | drive high for the speaker outputs |
| Headphone detect | 39 | input-only |
| KEY1–KEY6 | 36, 13, 19, 23, 18, 5 | I5; KEY1 on an input-only pin |
| LED4 / LED5 | 22 / 19 | LED5 shares KEY3 |
| SD-card group (display, I5) | 14 SCK, 13 MOSI, 15 CS, 2, 4, 12 | 12 and 15 are strapping pins; **13 is also KEY2** — see I5 |

### Code you must read before changing anything

- `crates/starplayer-host-embedded/src/lib.rs` (I1) — `EmbeddedPlayer::open`,
  `RenderHalf::render`, `ControlHalf`, `settings_for`, `bench::render_digest`,
  `bench::render_frames`.
- `crates/starplayer-model/src/image.rs` (I2) — `Module::from_image`, the alignment
  requirement and the `#[repr(C, align(4))]` wrapper it needs around `include_bytes!`.
- `../ampkeeper/esp32/firmware/src/esp32_main.rs` — the heap set-up (`esp_alloc::heap_allocator!`
  regions, the PSRAM region added by hand), `esp_rtos::start`, the second-core executor
  (`CORE1_SPAWNER`), the task list; `../ampkeeper/esp32/firmware/src/panic.rs`;
  `../ampkeeper/esp32/xtask/src/main.rs` — `Resolution`, `cargo +esp` invocation with
  `current_dir(firmware/)`, `cargo espflash save-image`, the `size` command.
- `../ampkeeper/esp32/Cargo.toml` `[profile.*]` — copy the release/dev shape and the
  reasons given in its comments; drop the `btuuid`/`p384` overrides that do not apply.
- `../ampkeeper/AGENTS.md` "Working agreements" for the flashing rule: agents build images
  and hand off; the owner flashes.
- `crates/starplayer-offline/src/bin/starplayer-goldens.rs` — the fixture list and
  hashes the bench must reproduce; `goldens/`.
- `plans/product/01-technical-architecture.md` §8 (RT rules — they apply to the DMA
  refill), §10, §12 Q2; `plans/reference/embedded-budget.md` (the stub to fill).
- ESP-ADF's `es8388.c` (`es8388_init`, `es8388_config_i2s`, `es8388_set_voice_volume`) for
  the register sequence — the reference for research point 2.

## Deliverables

### 1. The `embedded/` workspace

```text
embedded/
  Cargo.toml              members: firmware-common, boards/starplayer-a1s, boards/starplayer-c5 (I4), xtask
                          all [profile.*] here; `exclude` nothing
  rust-toolchain.toml     channel = "esp"   (nightly features + Xtensa; the C5 board is
                          built with the same toolchain's riscv target, see I4)
  .cargo/config.toml      alias xtask only — no build settings, as ampkeeper's
  firmware-common/        board-independent: NowPlaying view model (I5), bench runner, log helpers
  boards/starplayer-a1s/  the Xtensa firmware: .cargo/config.toml (target, runner, -Tlinkall.x,
                          -C force-frame-pointers, build-std), partitions.csv, src/
  assets/                 .gitignored build products from `cargo xtask module-images`
  xtask/                  build / image / size / flash / monitor (stable std binary)
  README.md               toolchain install, build, flash, monitor, the pin map
```

Dependencies for `boards/starplayer-a1s` in a `[target.xtensa-esp32-none-elf.dependencies]`
table (mirror ampkeeper's, at the same versions; verify each resolves for `esp32`):
esp-hal 1.1.1 `["esp32", "unstable"]`, esp-rtos 0.3.0 `["esp32", "embassy"]`,
esp-println 0.17 `["esp32", "log-04"]`, esp-backtrace 0.19 `["esp32", "panic-handler",
"println"]`, esp-alloc 0.10, esp-bootloader-esp-idf 0.5 `["esp32"]`, embassy-executor
0.10 `["nightly"]`, embassy-time 0.5.1, embassy-sync 0.8, static_cell 2.1,
critical-section 1, log 0.4. Engine: `starplayer = { path = "../../../crates/starplayer",
default-features = false, features = ["s3m", "mod", "mtm", "xm", "it", "telemetry"] }`,
`starplayer-host-embedded`, `starplayer-model`. Features on the board crate: `bench`
(render the goldens and print, no audio), `lcd` (I5), `web` (I6); default = none.

`partitions.csv` for 4 MB: `nvs`, `phy_init`, `factory` app (~2.5 MB), a `modules` data
partition for I6 (the rest; not used here), and a `config` partition (I6). No OTA slots.

`embedded/xtask`: `build --board a1s [--features ..] [--dev]`, `image` (`cargo espflash
save-image --chip esp32`), `size` (partition-relative, warn at 90 %), `flash` and
`monitor` as thin espflash delegators, `assets` (calls the main workspace's
`cargo xtask module-images`). Builds run `cargo +esp` with `current_dir` on the board
crate so its directory-scoped config applies.

### 2. Board bring-up (`boards/starplayer-a1s/src/`)

- `board.rs` — the pin map as named constants and a `Board::take(peripherals)` that
  returns typed handles (I2C bus, I2S TX with DMA, PA-enable output, key inputs for I5,
  the SPI group for I5). One place for every GPIO number.
- `es8388.rs` — a minimal blocking driver over `embedded-hal` I2C: `init_dac_only`
  (the ADF sequence: chip power, master clock from MCLK at 256×fs, I2S Philips 16-bit,
  DAC power up, output mixer routing to both LOUT1/ROUT1 (headphone) and LOUT2/ROUT2
  (speaker), unmute), `set_volume(u8)`, `mute(bool)`. Research point 2 decides whether an
  existing Rust crate is used instead.
- `audio.rs` — esp-hal I2S master TX, Philips, 16-bit stereo, 44 100 Hz, MCLK out on
  GPIO0 (research point 3), a **circular DMA** transfer over a static buffer of
  `DMA_RING_QUANTA × 128` frames. The refill is an Embassy task awaiting the DMA
  available-space future and calling `RenderHalf::render` for exactly the free space —
  never more than the ring, never across a DMA boundary it does not own. The audio task
  is pinned to core 1 through a second `esp_rtos` executor as ampkeeper does
  (`CORE1_SPAWNER`), so WiFi (I6) and the display (I5) on core 0 cannot starve it. State
  the RT rules in the module doc: no allocation, no `log!` in the refill, no blocking I2C.
- `main.rs` — heap (`esp_alloc::heap_allocator!` for DRAM; a PSRAM region added with
  `HeapRegion::new(.., MemoryCapability::External)` as ampkeeper does, so I6 can place
  uploaded modules there), `esp_rtos::start`, board init, ES8388 init, the embedded
  `PETRI.S3M` image (`include_bytes!` in an `#[repr(C, align(4))]` wrapper) →
  `Module::from_image` → `EmbeddedPlayer::open` → play; a control task that calls
  `collect_garbage` and logs the `Snapshot` transport once a second over UART.
- `bench.rs` (feature `bench`) — no codec: for each of the six fixture images, run
  `bench::render_digest` and print `name sha256 frames cycles cycles_per_frame` using the
  Xtensa cycle counter (`xtensa_lx::timer::get_cycle_count` or esp-hal's equivalent), for
  `Nearest`/`Linear`/`Cubic`/`Sinc`, with the PCM (a) borrowed from flash, (b) copied to
  PSRAM, (c) copied to DRAM (for the fixtures that fit). Print heap high-water after
  `open` (`esp_alloc::HEAP.stats()`), `size_of::<Voice>()`, and the free DRAM at boot.
  This transcript **is** the budget document's data.

### 3. The budget document and Q2

Fill `plans/reference/embedded-budget.md` from the bench transcript: flash by build
variant (`cargo xtask size`), RAM by channel count, cycles per frame per interpolator per
PCM location, frames/s per voice, latency for the DMA depth chosen. Then answer Q2 in
`plans/product/01-technical-architecture.md` §12: whether 128 stays (expected), with the
observation that the tunable on this path is the DMA ring depth, which the host owns.

### 4. Documentation

`embedded/README.md` (toolchain, build, flash, monitor, the pin map, the DIP switches);
`CLAUDE.md` layout block gets an `embedded/` line; `plans/README.md` M8 row updated.

## Research points

0. **Toolchain**: `espup install --toolchain-version 1.97.0.0` and confirm
   `cargo +esp --version` reports 1.97; confirm `xtensa-esp32-none-elf` and
   `riscv32imac-unknown-none-elf` are both present in that toolchain (they were in
   1.95.0.0). If 1.97.0.0 is not installable, the fallback is lowering the workspace
   `rust-version` — an owner decision, not the agent's.
1. **Pin map and codec identity**: I2C scan at boot, log the result; confirm the I2S pins
   against the v2.2 schematic. Record the board revision printed on the silkscreen.
2. **ES8388 driver**: search crates.io for an `embedded-hal` 1.0 ES8388 driver; if one
   exists at a sensible version, use it and record the version; otherwise port the ADF init
   sequence (it is ~30 register writes) into `es8388.rs`.
3. **MCLK on classic ESP32**: MCLK is only available on GPIO0/1/3 via `CLK_OUT1..3`. Confirm
   esp-hal 1.1 exposes it for the ESP32 I2S peripheral (`with_mclk` or the `I2S0` clock-out
   register through the `unstable` API). Fallbacks, in order: write the `PIN_CTRL` /
   `IO_MUX` bits directly (one documented `unsafe`); run the ES8388 without MCLK — it can
   derive its clocks from BCLK in some configurations (ADF's `es8388_config_i2s` with
   `use_mclk = false`) at the cost of some quality — record which was needed.
4. **DMA ring depth and Q2**: measure underruns at 2, 4 and 8 quanta of DMA ring with the
   control task logging; pick the smallest that never underruns with WiFi off, then double
   it for I6's headroom. This is the measurement Q2's answer rests on.
5. **PSRAM on this module**: 4 MB or 8 MB; whether esp-hal's `psram` init for the classic
   ESP32 needs the `quad` mode flag; whether the mixer's random sample reads from PSRAM
   through the cache are competitive with flash (the bench's (a)/(b)/(c) rows answer it).
6. **Core pinning**: whether esp-rtos 0.3's second-core executor on the classic ESP32 works
   as on the S3 (ampkeeper's `multicore` feature). If not, single core with the audio task
   at the highest priority, and record the underrun behaviour.

## Verification

```text
# host side
cargo xtask module-images                         # embedded/assets/*.spmi
cd embedded && cargo xtask build --board a1s                    # audio firmware
cd embedded && cargo xtask build --board a1s --features bench   # bench firmware
cd embedded && cargo xtask size --board a1s
# owner, on hardware
cd embedded && cargo xtask flash --board a1s --features bench && cargo xtask monitor
#   → every fixture line's sha256 equals goldens/<format>/<fixture>.sha256 (Linear row)
cd embedded && cargo xtask flash --board a1s && cargo xtask monitor
#   → PETRI.S3M plays through the headphone jack; transport log advances; no underrun lines
# main workspace unaffected
cargo xtask ci
```

## Out of scope

Buttons and the display (I5), WiFi and the portal (I6), the C5 (I4), audio input, the
speaker amplifier's volume curve beyond "it makes sound", OTA. Any change to the engine
crates: if bring-up needs one, stop and write it up.

## Research resolution

Implemented 2026-09-11 on branch `m8-i3`. Both firmware variants link; nothing has been
flashed. Every figure that can be taken without the board is in
[`plans/reference/embedded-budget.md`](../../reference/embedded-budget.md); every figure that
cannot says `TBD (owner: …)` and names the command that produces it.

### 0. Toolchain

Already installed before this task started, as the **named** toolchain `esp-1.97`
(`espup install --name esp-1.97 --toolchain-version 1.97.0.0`), not the floating `esp`
channel. `rustc 1.97.0-nightly`, with both `xtensa-esp32-none-elf` and
`riscv32imac-unknown-none-elf` present — so I4 needs no second install.
`embedded/rust-toolchain.toml` pins `channel = "esp-1.97"`, and `embedded/xtask` passes
`+esp-1.97` explicitly on top of that so an error message names the compiler it asked for.
`espflash` and `cargo-espflash` are 3.3.0.

Every build needs `. ~/export-esp-1.97.sh` sourced first (the Xtensa GCC linker's `PATH`
and `LIBCLANG_PATH`). `cargo xtask` checks for `xtensa-esp32-elf-gcc` before it compiles
anything and prints that line when it is missing, because the failure mode otherwise is a
link error at the end of a five-minute build.

### 1. Pin map and codec identity

**Not confirmed — it cannot be from here.** The map in the task file is the one every
Arduino and ESP-ADF port of the v2.2 Audio Kit uses; it is written once, as named
constants, in `boards/starplayer-a1s/src/board.rs`, and reproduced in
`embedded/README.md` §4.

What this task added instead is the *instrument*: the audio build runs an I2C scan at boot,
before it configures anything, and prints every address that answered —
`I2C  device at 0x10 (ES8388)` on the expected board, `0x1a (AC101 — this revision is out
of scope)` on the older one, and a line naming SDA/SCL if nothing answers at all. The probe
is a zero-length write over `0x08`–`0x77`, which changes no register on any device. The
headphone-detect pin is read and logged on the same boot.

The board revision printed on the silkscreen is still for the owner to record.

### 2. ES8388 driver — **there is no crate; forty register writes were written out**

The crates.io registry API returns `total: 0` for `es8388`; `lib.rs` finds nothing; the
`esp-rs/esp-hal-community` repository carries a buzzer and a smart-LED driver and no
codecs. The nearest published relatives are `es8311` 0.1.1 and `es7210` 0.1.0 — both
`embedded-hal` 1.0, both `no_std`, both different silicon with different register maps.
One unpublished Rust driver exists (`github.com/hi-squeaky-things/driver_es8388`, last
pushed 2024-03-30) and names `esp-hal 0.16` as a hard, non-optional dependency **of the
library**, which makes it neither portable nor current.

So `boards/starplayer-a1s/src/es8388.rs` is a fresh minimal driver over `embedded-hal` 1.0.
Its sequence comes from ESP-ADF's `es8388_init()` on the **`release/v2.x`** branch —
`components/audio_hal/driver/es8388/es8388.c` — cross-checked against the same driver's new
home on `master` (`components/esp_codec_dev/device/es8388/es8388.c`, byte-identical init),
the register map in `es8388.h`, the **ES8388 datasheet Rev 5.0 (July 2018)** for the bit
fields, and the **ES8388 User Guide (2011-06-17)** for the vendor's own bring-up flow.

*The path this task file names 404s*: the driver moved out of `audio_hal` on `master`.
`release/v2.x` is where the classic file still lives.

Four places where the sources disagree, decided and recorded beside the writes:

1. **`MASTERMODE` (`0x08`) = `0x00` — codec as I2S slave.** The reset default is `0x80`,
   *master*; omitting this write leaves the codec driving BCLK and LRCK against the ESP32,
   which is also driving them.
2. **`DACCONTROL21` (`0x2b`) = `0x80`, not `0xc0`.** Bit 7 `slrck` shares one LRCK between
   ADC and DAC; bit 6 `lrck_sel` chooses which, and both the datasheet and the User Guide
   say it must be 0 (and that it only has an effect in master mode). ESP-ADF writes `0xc0`
   only in its analog line-in bypass path. Third-party ports that write `0xc0` for playback
   are "correcting" a misleading ADF comment, and are wrong.
3. **`LDACVOL`/`RDACVOL` (`0x1a`/`0x1b`) are written explicitly to `0x00`.** They **reset
   to `0xc0` = −96 dB**. A driver that leaves them alone brings the codec up perfectly and
   produces silence, with no error anywhere — the single most likely way to lose a day on
   this part.
4. **The output-enable bits are the datasheet's, not ESP-ADF's.** ADF's `es_dac_output_t`
   has `DAC_OUTPUT_LOUT1 = 0x04` and `DAC_OUTPUT_ROUT2 = 0x20` transposed with respect to
   Register 4 (`bit5 = LOUT1`, `bit4 = ROUT1`, `bit3 = LOUT2`, `bit2 = ROUT2`). Invisible
   for "all four outputs" (`0x3c` either way) and wrong the moment a build wants the
   headphone pair alone, so the `outputs` module spells the datasheet's bits.

Three further deliberate departures: the undocumented writes ADF makes at `0x35`/`0x37`/
`0x39` ("disable the internal DLL to improve 8K sample rate") are **omitted** — they lie
past the datasheet's documented map, which stops at `0x34`, and this firmware runs at
44 100 Hz. The speaker outputs are set to 0 dB where ADF leaves them at −45 dB, because
this board has a PA-enable pin that already decides whether the speakers sound. And volume
is exposed as `set_volume_attenuation(half_decibels)` straight onto `0x1a`/`0x1b` rather
than as ADF's `0..100`, whose mapping runs through a board table and subtracts a
`BOARD_PA_GAIN` the Audio Kit — not an official ADF board — has no published value for;
ADF's curve never reaches 0 dB at volume 100 and could not be explained in a budget
document.

The codec is left **muted** by `init_dac_only`, and `main` unmutes it only after the DMA
ring is running and prefilled with real audio. An unmuted codec in front of an empty ring
is a click loud enough to be remembered.

### 3. MCLK on the classic ESP32 — **esp-hal exposes it; no fallback was needed**

`esp-hal` 1.1.2 has a chip-specific `I2s::with_mclk` for the ESP32 (`src/i2s/master.rs`,
`#[cfg(esp32)]`) taking a `ClkPin`, implemented only for GPIO0, GPIO1 and GPIO3 →
`CLK_OUT1`, `CLK_OUT3`, `CLK_OUT2`. It programs `IO_MUX.PIN_CTRL` and connects the output
signal itself. `audio::start` calls `.with_mclk(peripherals.GPIO0)` and that is the whole
of it: **no register poke, no `unsafe` of ours, and neither fallback** (the documented
`PIN_CTRL` poke, or running the ES8388 without MCLK from BCLK) was reached.

`DACCONTROL2` is still written `0x02` (MCLK/LRCK = 256). In slave mode the codec
auto-detects the ratio, so the write is advisory — it costs one byte and is correct if the
roles are ever swapped.

**One thing on this path the owner must check by ear.** esp-hal sets both `tx_msb_right`
and `tx_right_first` on the classic ESP32 (the latter because the chip emits two clock
pulses before the first sample, and sending the right channel first keeps WS high across
them). The two should cancel and an interleaved `[left, right]` buffer should arrive the
right way round, but nothing here can prove it. If the stereo image is reversed, set
`SWAP_CHANNELS` in `audio.rs` and rebuild; the constant exists so that the fix is one line
rather than a redesign.

### 4. DMA ring depth and Q2

The ring is a circular DMA transfer whose **chunk size is exactly one render quantum**
(512 bytes), so the peripheral's available-space figure is always a whole number of quanta
and the refill's block size is the engine's own. `DMA_RING_QUANTA` is 8 — 1 024 frames,
23.2 ms — which is a deliberately generous starting point, not a result. The measurement
(2, 4, 8 against the `underruns=` field on the once-a-second transport line) is the
owner's, and is `TBD` in the budget document with the procedure written out.

**Q2 is settled anyway, and not by that measurement.** 128 stays, with no compile-time
override, and the reasoning is in `plans/product/01-technical-architecture.md` §12 and
`plans/reference/embedded-budget.md` §5. The short version is that the quantum is not the
knob: 512 bytes is a natural DMA chunk on this chip and makes each descriptor one quantum;
latency belongs to the ring (2.9 ms against 23.2); and the RAM a smaller quantum would save
is 4 kB against a fixed 27.8 kB of telemetry overhead and an 88 kB module image.

### 5. PSRAM — and an erratum that changes the design

PSRAM on the classic ESP32 is quad-SPI only; `esp_hal::psram::PsramConfig` for this chip
has no mode flag at all (it has `size`, `cache_speed` and `psram_vaddr_mode`), and
`PsramSize::AutoDetect` maps the 4 MB maximum and probes. So there is nothing to configure
and the task file's "does it need the `quad` flag" has no arm to take.

The finding that matters is different, and it is not in the task file:

> **On the ESP32, ESP32-S2 and ESP32-S3 the atomic instructions do not work correctly on
> memory located in PSRAM.** (esp-alloc's own documentation.)

This engine puts atomics on the heap: `starplayer_rt::Arc`'s reference count, the seqlocks
`starplayer-host-embedded` carries its two frame clocks in, the telemetry ring's sequence.
esp-alloc's plain `alloc` takes the first region that fits in **registration order**, so
registering PSRAM at all makes those reachable from PSRAM the moment internal DRAM fills —
a silent corrupted reference count rather than an honest allocation failure.

So, **against the deliverable as written** ("a PSRAM region added with `HeapRegion::new(..,
External)` as ampkeeper does"):

* the **audio** build maps PSRAM and reports its size, and does **not** register it;
* the **bench** build does register it, because it has no real-time path and no `Arc` it
  would mind finding there, and because it asks for external memory explicitly by
  capability (`alloc_caps(External)`) rather than letting a general allocation fall into
  it.

**M8-I6 inherits the constraint** and should be planned around it: an uploaded module's
*sample data* may live in PSRAM; the `Arc` that owns it may not. That is a real change to
what I6 assumed, and it is written into `main.rs`, the README and the budget document
rather than only here.

The flash/PSRAM/DRAM comparison the deliverable asks for is still produced, by copying a
module image into a staging buffer claimed with the right capability and borrowing the
module out of it. `PETRI.S3M` gets no DRAM row, and that is a result: 88 036 bytes of image
cannot coexist with a 120 KiB heap in one ~176 KB DRAM segment. The bench prints
`skipped=no staging buffer large enough` rather than quietly measuring something else, and
the budget document says why that refusal is the point of M8-I2.

### 6. Core pinning — **the audio task is on core 0, and the blocker is a type**

esp-rtos 0.3's `start_second_core` is chip-independent and there is no reason to think it
behaves differently here than on ampkeeper's S3. The blocker is upstream of the chip:

> `Engine` holds `Box<dyn EventSource>` — with no `+ Send` bound — so `Engine`, and
> therefore `RenderHalf`, is **not `Send`** and cannot be handed to a second-core
> `SendSpawner`.

Two ways out, neither taken:

1. **`Box<dyn EventSource + Send>` in `starplayer-engine`.** The task file's Out of scope
   says "any change to the engine crates: if bring-up needs one, stop and write it up" —
   this is that write-up. It is a one-word change with a wide blast radius (every
   `set_source`/`add_source` caller, `SourceMux`, the offline renderer's boxed sequencers),
   and it is the *right* long-term answer if the engine is ever to be driven from a thread
   other than the one that built it.
2. **Build the whole player inside the second core's entry closure**, so nothing crosses.
   That works without touching the engine, but it moves the control half to core 1 too and
   needs the command channel M8-I5 and M8-I6 will want anyway — and it could not be
   validated without the board.

Neither is needed for this milestone's exit criterion: there is no WiFi and no display yet,
so core 0 has nothing to be starved by. The audio and control tasks share the main
executor, and the refill's underrun counter is printed every second so the owner's run
measures exactly what single-core costs. **M8-I5 and M8-I6 should not be started without
settling this**, because both add work to the core the refill is on.

## What was done differently from the task file, and why

* **`-C force-frame-pointers` is not in the board's rustflags**, though the task file says
  to mirror ampkeeper's classic-ESP32 config. With it on the firmware does not compile:
  `rustc-LLVM ERROR: Error while trying to spill A8 from class AR: Cannot scavenge register
  without an emergency spill slot` — the Xtensa LLVM register-allocator bug ampkeeper hit
  from the other direction on the S3, reproduced here on esp-1.97 at `opt-level = 3`. The
  cost is less reliable panic backtraces; `[profile.release] debug = true` keeps the symbols.
* **The release profile is `opt-level = 3`, not ampkeeper's `"s"`.** The whole point of
  this firmware is a cycles-per-frame measurement of a DSP inner loop; a figure taken at
  `"s"` would measure the size/speed tradeoff instead. Flash is not the constraint — the
  audio build is 17.7 % of its partition.
* **The heap is 120 KiB, and the number is a balance against the stack.** On the classic
  ESP32 the main stack is whatever DRAM is left between `_bss_end` and `0x3ffe_0000`, so
  every heap byte is a stack byte. At the 160 KiB first tried, the firmware still *links* —
  and leaves 6 472 bytes of stack, which will not survive a boot. At 120 KiB the stack is
  47 424 bytes. The check is one `nm` line and is written into the README and into the
  constant's own documentation, because a link that succeeds is not evidence here.
* **`bench.rs` is split in two.** The line format, the two renders behind each row and the
  kernel/location dispatch live in `firmware-common`, a `no_std` crate that builds and
  **tests on the host** (15 tests, including one that reproduces
  `goldens/s3m/reflex__i16_mono_44100_linear.sha256` through the whole runner). The board
  crate supplies only the cycle counter, the staging buffers and the heap numbers. Two
  reasons: I4's C5 must emit byte-identical lines from the same code or the two
  architectures cannot be compared, and a bench runner that can only be exercised by
  flashing a board is a bench runner nobody will change safely.
* **The cycle counter is derived, deliberately.** `CCOUNT` is 32 bits and wraps every
  17.9 s at 240 MHz, which is shorter than a ten-second sinc render might take — a raw
  difference could be wrong by a multiple of 2³² and look plausible. The bench reads
  esp-hal's 64-bit microsecond timer and scales by the configured CPU frequency: exactly
  240 cycles per microsecond at 240 MHz, quantisation error under a millionth over a
  multi-second render, and it cannot wrap.
* **The DMA ring is prefilled with real audio before the transfer starts.** A circular
  transfer plays whatever is in its buffer the moment it begins; 23 ms of silence at the
  top of a song is a click waiting for the unmute.
* **`Board::take` takes one peripheral per argument** rather than the whole `Peripherals`
  and a leftover. Handing back "the rest" would need either a partial move it cannot return
  or `clone_unchecked` on every singleton; splitting at the call site keeps `board.rs` free
  of `unsafe` entirely.
* **The bench renders 24 rows per fixture** — four kernels × three PCM locations, plus a
  skip line where a location cannot hold the image — and hashes **mono** while timing
  **stereo**, because the goldens are mono and the player is stereo and a cycles figure
  taken in mono would understate the mixer by a factor a reader could not recover.
* **The partition table declares `modules` and `config` now**, unused, because adding a
  partition row later moves every row after it and invalidates whatever a device had
  stored.
* **`exclude = ["fuzz", "embedded"]` was added to the root manifest.** Neither is reached
  by the `members` globs today; saying so out loud means a future glob cannot quietly drag
  one in and break `cargo xtask ci`.

## `unsafe` inventory

Master-plan decision 6 says `embedded/` is the only place `unsafe` is tolerated, each site
with a `// SAFETY:` comment. There are **three**, all in the board crate, all in two files:

| Site | Why |
|---|---|
| `src/main.rs`, `esp_alloc::HEAP.add_region(...)` for PSRAM | `#[cfg(feature = "bench")]` only. This is `esp_alloc::psram_allocator!`'s own expansion; the region is the one esp-hal has just mapped and nothing else touches it. |
| `src/bench.rs`, `Staging::claim` → `HEAP.alloc_caps` | The raw allocation the capability-directed staging buffers need. `GlobalAlloc::alloc`'s contract, with a non-zero constant layout; never freed, which is intended for a firmware that runs its rows once. |
| `src/bench.rs`, `Staging::load` → `slice::from_raw_parts_mut` | Turns that allocation into the `&'static [u8]` `Module::from_image` requires. The aliasing obligation — no live `Module` borrowing the previous contents — is stated on the method and discharged at every call site by an explicit `drop(module)`. |

`firmware-common` and `embedded/xtask` contain none. `esp_alloc::heap_allocator!` and
`dma_circular_buffers_chunk_size!` expand to `unsafe` internally; neither appears in this
tree's own text.

## Verification run

Every command below was run in this worktree. `. ~/export-esp-1.97.sh` was sourced first
for the firmware ones.

| Command | Result |
|---|---|
| `cargo xtask module-images` (main workspace) | pass — six images, 117 040 bytes |
| `cd embedded && cargo xtask build --board a1s` | **links**, no warnings |
| `cd embedded && cargo xtask build --board a1s --features bench` | **links**, no warnings |
| `cd embedded && cargo xtask image --board a1s [--merge]` | pass — 464 944 B app image, 530 480 B merged |
| `cd embedded && cargo xtask size --board a1s` | pass — 464 944 / 2 621 440 B = 17.7 % |
| `cd embedded && cargo xtask size --board a1s --features bench` | pass — 552 560 / 2 621 440 B = 21.0 % |
| `cd embedded && cargo test -p starplayer-firmware-common` | pass — 15 tests (host, esp-1.97) |
| `cd embedded && cargo test -p starplayer-embedded-xtask` | pass — 4 tests |
| `cargo xtask ci` (main workspace) | pass — every job; `conformance` and `rt-safety` need `cargo xtask conformance --fetch-only` first in a fresh worktree |
| `cargo metadata --no-deps` (main workspace) | pass — no `embedded/` package is a member |

## Owner steps

In this order. **Flashing is owner-only; no agent has run any of it.**

1. **Flash the bench build and capture the transcript.**

   ```sh
   cd embedded
   . ~/export-esp-1.97.sh
   cargo xtask flash --board a1s --features bench
   cargo xtask monitor --board a1s | tee /tmp/starplayer-bench.log
   ```

   Check that every `BENCH … linear flash sha256=…` line equals the committed hash in
   `goldens/<format>/<stem>__i16_mono_44100_linear.sha256`. **That equality is M8's exit
   criterion.**

2. **Paste the numbers into `plans/reference/embedded-budget.md`.** Its §6 is a table of
   which log line fills which row: `SIZE …` into §2, `HEAP […] …` into the heap table's
   "Device measured" column, `BENCH … cycles_per_frame=… core_load=…` into the §3 CPU
   tables.

3. **Flash the audio build and listen.**

   ```sh
   cargo xtask flash --board a1s
   cargo xtask monitor --board a1s
   ```

   `PETRI.S3M` should play out of the headphone jack. Check, in the log: the I2C scan says
   `0x10 (ES8388)` (research point 1, and the board revision off the silkscreen); the
   transport line advances once a second; `underruns=0` and `dma_errors=0` stay at zero.
   And check by ear that the stereo image is the right way round — if it is reversed, set
   `SWAP_CHANNELS` in `boards/starplayer-a1s/src/audio.rs` and rebuild.

4. **Measure the ring depth** (research point 4): rebuild with `DMA_RING_QUANTA` at 2 and
   at 4, watch `underruns=`, take the smallest clean depth, double it for M8-I6, and record
   both the clean depth and the chosen one in the budget document's §4.

5. **Decide the core-pinning question before M8-I5 or M8-I6 is started** — research point
   6 above. Both of those tasks put work on the core the DMA refill is running on.
