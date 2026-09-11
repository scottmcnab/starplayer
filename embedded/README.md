# StarPlayer firmware

The `embedded/` workspace: StarPlayer running on real hardware. Today that is one board,
the AI-Thinker **ESP32-Audio-Kit** (ESP32-A1S module, classic ESP32, ES8388 codec);
M8-I4 adds the ESP32-C5.

This is **its own cargo workspace**, deliberately (M8 master-plan decision 1). It is built
with the Xtensa fork of rustc, it needs `-Z build-std`, and its board crates carry
per-target dependency tables full of `esp-*` crates that only compile for their own
triple. None of that may be visible to the main workspace's `cargo xtask ci`.

---

## 1. Toolchain

The compiler is the named espup toolchain **`esp-1.97`** (rustc 1.97.0-nightly, Xtensa),
pinned in `rust-toolchain.toml`. It carries both targets the milestone needs —
`xtensa-esp32-none-elf` (A1S) and `riscv32imac-unknown-none-elf` (C5, M8-I4).

It is already installed. If it ever has to be installed again:

```sh
espup install --name esp-1.97 --toolchain-version 1.97.0.0
```

**Every shell that builds firmware must first source the toolchain's environment**, which
puts the Xtensa GCC linker on `PATH` and sets `LIBCLANG_PATH`:

```sh
. ~/export-esp-1.97.sh
```

Forget it and `cargo xtask` stops before compiling anything and tells you to run exactly
that. Flashing needs `espflash` (3.3.0) and `cargo-espflash`, both already installed.

## 2. Build

Everything goes through `cargo xtask`, from **this** directory:

```sh
cd embedded
. ~/export-esp-1.97.sh

cargo xtask build  --board a1s                     # the audio firmware
cargo xtask build  --board a1s --features bench    # the bench firmware (no audio)
cargo xtask image  --board a1s [--merge]           # an espflash image under target/
cargo xtask size   --board a1s                     # the image against its partition
cargo xtask assets [--force]                       # regenerate the module images
cargo xtask build  --board a1s --dev               # release codegen, debug assertions on
```

`cargo xtask assets` runs the **main** workspace's `cargo xtask module-images`, which
writes `embedded/assets/*.spmi` — the module images the firmware links with
`include_bytes!`. They are git-ignored build products, and `build`, `image` and `size` all
generate them automatically when they are missing, so a fresh clone builds.

Host-testable logic lives in `firmware-common`, which builds for the host too:

```sh
cargo test -p starplayer-firmware-common
cargo test -p starplayer-embedded-xtask
```

### Why `xtask` and not `cargo build`

Cargo reads `.cargo/config.toml` from the directory it is **invoked in**. The target
triple, `-Tlinkall.x` and `-Z build-std` live in `boards/starplayer-a1s/.cargo/config.toml`,
so a firmware build is only correct with cargo's current directory set to that crate.
`cargo xtask` does that for you. A bare `cargo build` from `embedded/` will quietly try to
build the firmware for the host and fail on the first `esp_hal` name.

There is exactly one target directory, `embedded/target`. Do not create a second — a
`build-std` tree for one Xtensa target is already 1.6 GB.

## 3. Flash and monitor — **owner only**

Agents build images and hand them over. **An agent must never run `espflash flash`,
`espflash erase` or `cargo xtask flash`.** The two commands below are for the owner:

```sh
cd embedded
. ~/export-esp-1.97.sh

# the bench firmware: renders every golden fixture, prints hashes and cycles, no audio
cargo xtask flash --board a1s --features bench
cargo xtask monitor --board a1s

# the audio firmware: plays PETRI.S3M out of the headphone jack
cargo xtask flash --board a1s
cargo xtask monitor --board a1s
```

`flash` builds a merged image (bootloader + partition table + app) with `--skip-padding`,
so a reflash leaves the `modules` and `config` partitions alone, and attaches the monitor
when it is done.

The Audio Kit's USB port is a CP2102; on Linux it appears as `/dev/ttyUSB0` and needs the
user to be in the `dialout` group. If espflash cannot get the board into the bootloader,
hold **BOOT** (KEY1 area, the button marked `IO0`) while tapping **EN/RST**.

### What a good boot looks like

The **bench** build prints, in order:

```text
=== StarPlayer A1S bench ===
BUILD profile=release kernels=nearest,linear,cubic,sinc
CPU  240 MHz  (240000000 cycles/s)
SIZE voice=176 bytes  render_half=8344 bytes
HEAP [boot] …
STAGING dram=ok psram=ok
IMAGE synthetic-mod bytes=6072 (5.9 KiB)
BENCH synthetic-mod nearest flash sha256=… frames=441000 … cycles_per_frame=… core_load=…%
…
=== bench complete ===
```

Every `BENCH … linear flash` line's `sha256=` must equal the committed hash in
`goldens/<format>/<stem>__i16_mono_44100_linear.sha256`. That equality **is** M8's exit
criterion. `grep BENCH` over a captured log is the whole extraction tool.

The **audio** build prints its boot sequence and then one transport line a second:

```text
StarPlayer 0.1.0 on the ESP32-A1S Audio Kit
CPU  240 MHz   heap 120.0 KiB
PSRAM 4194304 bytes (4096.0 KiB) mapped at 0x3f800000
I2C  device at 0x10 (ES8388)
JACK headphone_detect=inserted
CODEC ES8388 at 0x10: DAC up, 16-bit Philips slave, MCLK/LRCK 256, muted
MODULE image=88036 bytes (85.9 KiB) channels=8 samples=5
HEAP after open: …
I2S  44100 Hz stereo 16-bit, MCLK on GPIO0, DMA ring 8 quanta (1024 frames, 23 ms)
PLAY
ord 000 pat 000 row 00/06 125 bpm   3/ 8 voices  0:01/2:47  peak=… underruns=0 …
```

### The I2C scan expectation

The boot log's first interesting line is the bus scan, and it is **research point 1's
instrument**:

* `I2C  device at 0x10 (ES8388)` — the expected board, the v2.2+ revision. Play on.
* `I2C  device at 0x1a (AC101 — this revision is out of scope)` — the older Audio Kit
  revision. This firmware cannot drive it; M8 declared it out of scope.
* `I2C  no device answered` — check SDA on GPIO33 and SCL on GPIO32, and check that the
  board is powered from USB rather than from a battery header.

Other addresses may appear and are harmless; the scan probes `0x08`–`0x77` with a
zero-length write, which changes no register on any device.

## 4. The board

| Function | GPIO | Note |
|---|---|---|
| I2C SDA / SCL | 33 / 32 | ES8388 control, 7-bit address `0x10` |
| I2S MCLK | 0 | `CLK_OUT1`; a strapping pin — MCLK only appears after boot |
| I2S BCLK / LRCK | 27 / 25 | |
| I2S DOUT (ESP → codec DSDIN) | 26 | |
| I2S DIN (codec ASDOUT → ESP) | 35 | unused; input-only pin |
| PA enable (speaker amplifier) | 21 | driven high once audio is flowing |
| Headphone detect | 39 | input-only |
| KEY1–KEY6 | 36, 13, 19, 23, 18, 5 | M8-I5; KEY1 is on an input-only pin |
| LED4 / LED5 | 22 / 19 | LED5 shares KEY3 |
| SD-card group (the display, M8-I5) | 14 SCK, 13 MOSI, 15 CS, 2, 4, 12 | 12 and 15 are strapping pins; **13 is also KEY2** |

The map lives in exactly one place in code: `boards/starplayer-a1s/src/board.rs`.

### DIP switches

The Audio Kit carries a five-way DIP switch block beside the SD slot. It multiplexes the
SD-card pin group between the slot, the JTAG header and the key matrix, and the silkscreen
labelling differs between board revisions. For **this** firmware:

* nothing here uses the SD card, JTAG or the keys, so **any** switch position boots and
  plays;
* M8-I5 takes the SD-card pin group for the ST7789 display, which means the SD slot must
  be switched **off** then — a display and an SD card cannot both have GPIO14/13/15.

Leave them as the board shipped until M8-I5 says otherwise, and record what the board in
hand is actually set to when you do.

### Partition table — 4 MB, no OTA

`boards/starplayer-a1s/partitions.csv`:

| Name | Type | Offset | Size | Used for |
|---|---|---|---|---|
| `nvs` | data | `0x9000` | 24 KB | — |
| `phy_init` | data | `0xf000` | 4 KB | — |
| `factory` | app | `0x10000` | 2.5 MB | the firmware |
| `modules` | data | `0x290000` | 1.375 MB | M8-I6's uploaded modules |
| `config` | data | `0x3f0000` | 64 KB | M8-I6's WiFi credentials |

No OTA slots: this is a bring-up firmware flashed over USB, and an `ota_0`/`ota_1` pair
would halve the app budget for a feature the milestone does not need. `modules` and
`config` are declared now rather than later because adding a partition row moves every row
after it and invalidates whatever a device had stored.

## 5. Layout

```text
embedded/
  Cargo.toml              the workspace and every [profile.*]
  rust-toolchain.toml     channel = "esp-1.97"
  .cargo/config.toml      the `xtask` alias and nothing else
  firmware-common/        board-independent: the bench runner, the now-playing view model,
                          the formatting helpers — builds and tests on the host
  boards/starplayer-a1s/  the Xtensa firmware
    .cargo/config.toml    target, runner, -Tlinkall.x, build-std  (directory-scoped!)
    partitions.csv
    src/board.rs          the pin map, once
    src/es8388.rs         the codec driver
    src/audio.rs          I2S + circular DMA + the refill task
    src/images.rs         the aligned include_bytes! wrappers
    src/bench.rs          the `bench` build's runner
    src/main.rs           boot
  assets/                 git-ignored *.spmi module images
  xtask/                  build / image / size / assets / flash / monitor
```

## 6. Notes for the next person

* **`-C force-frame-pointers` is deliberately absent** from the board's rustflags. With it
  on, this firmware does not compile: the Xtensa LLVM register allocator fails with
  `Cannot scavenge register without an emergency spill slot`. Ampkeeper hit the same bug
  from the other direction on the S3. The comment in `.cargo/config.toml` has the detail.
* **The heap is 120 KiB and the number is a balance against the stack.** On the classic
  ESP32 the main stack is whatever DRAM is left between `_bss_end` and `0x3ffe_0000`, so
  every heap byte is a stack byte. At 160 KiB the firmware links and leaves 6 472 bytes of
  stack, which will not survive a boot; at 120 KiB the stack is 47 424 bytes. After any
  change that moves a large static, check it:

  ```sh
  xtensa-esp32-elf-nm target/xtensa-esp32-none-elf/release/starplayer-a1s \
    | grep -E " (_bss_end|_stack_start)$"
  ```

* **PSRAM is mapped but not added to the general heap** in the audio build. The ESP32's
  atomic instructions do not work correctly on PSRAM, and this engine puts atomics on the
  heap (`Arc` refcounts, the host's seqlocks, the telemetry ring). `src/main.rs` has the
  full reasoning; M8-I6 inherits the constraint.
* **The audio task runs on core 0**, not on a second-core executor, because `RenderHalf`
  is not `Send` (the engine holds a `Box<dyn EventSource>`). See the "Core pinning" section
  of `src/main.rs` and the M8-I3 task file's research resolution.
* The measured sizes live in `plans/reference/embedded-budget.md`, which is where any new
  number belongs.
