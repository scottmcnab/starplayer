# M8 — I3: ESP32-A1S bring-up — the `embedded/` workspace, ES8388 over I2S, a module from flash

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md), decisions 1–3 and 6–7; deliverables 3–5). **The milestone's exit criterion** |
| Status | Planned 2026-09-11; pulled |
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
