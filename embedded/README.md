# StarPlayer firmware

The `embedded/` workspace: StarPlayer running on real hardware. Two boards:

* the AI-Thinker **ESP32-Audio-Kit** (ESP32-A1S module, classic ESP32, ES8388 codec) —
  the one that makes sound (M8-I3);
* the **ESP32-C5 devkit** (RISC-V, no audio hardware) — proves the second architecture by
  rendering the same golden fixtures and comparing their SHA-256 and cycle counts against
  the A1S's (M8-I4).

This is **its own cargo workspace**, deliberately (M8 master-plan decision 1). The A1S is
built with the Xtensa fork of rustc and needs `-Z build-std`; both board crates carry
per-target dependency tables full of `esp-*` crates that only compile for their own
triple. None of that may be visible to the main workspace's `cargo xtask ci`.

---

## 1. Toolchain

**Two toolchains, one per board — `embedded/xtask` picks the right one for you.**

* The **A1S** is Xtensa, an out-of-tree LLVM target that only exists inside the named
  espup toolchain **`esp-1.97`** (rustc 1.97.0-nightly), pinned in `rust-toolchain.toml`
  and built with `-Z build-std` (no target has a prebuilt `core`/`alloc` for it — there is
  nothing to add with `rustup target add`).
* The **C5** (`riscv32imac-unknown-none-elf`, M8-I4) is a plain LLVM target with a prebuilt
  `core`/`alloc` component, so it is built under the **main workspace's own pinned stable
  toolchain** (`1.97`) instead — the same one that already builds
  `riscv32imc-unknown-none-elf` for `cargo xtask ci --job no-std-check`. No `-Z build-std`,
  no espup, no Xtensa fork anywhere in the C5's build. This is M8-I4 research point 2:
  plain `rustup` was tried first and it just worked, so the "simpler" `esp-1.97` fallback
  the task allowed was never needed. See `plans/reference/embedded-budget.md` §8 for the
  finding in full.

`embedded/xtask` names the toolchain explicitly per board (`cargo +esp-1.97 …` for the
A1S, `cargo +1.97 …` for the C5) — you never choose it yourself.

The A1S's `esp-1.97` toolchain is already installed. If it ever has to be installed again:

```sh
espup install --name esp-1.97 --toolchain-version 1.97.0.0
```

The C5's plain `1.97` toolchain needs only its target added once, if it is not already:

```sh
rustup target add riscv32imac-unknown-none-elf --toolchain 1.97
```

(`rust-toolchain.toml` at the repository root lists it under `targets`, so a bare `rustup
show` from the repository root installs it automatically on a fresh clone.)

**Every shell that builds the A1S must first source the Xtensa toolchain's environment**,
which puts the Xtensa GCC linker on `PATH` and sets `LIBCLANG_PATH`:

```sh
. ~/export-esp-1.97.sh
```

Forget it and `cargo xtask build --board a1s` stops before compiling anything and tells you
to run exactly that. **The C5 needs none of this** — `riscv32imac-unknown-none-elf` links
with rustc's self-contained `rust-lld`, so `cargo xtask build --board c5` never checks for
an external linker at all. Flashing needs `espflash` (3.3.0) and `cargo-espflash`, both
already installed.

## 2. Build

Everything goes through `cargo xtask`, from **this** directory:

```sh
cd embedded
. ~/export-esp-1.97.sh    # needed for --board a1s; harmless (and unnecessary) for c5

cargo xtask build  --board a1s                     # the audio firmware, six keys, no display
cargo xtask build  --board a1s --features lcd      # the audio firmware, five keys, the ST7789 screen
cargo xtask build  --board a1s --features bench    # the bench firmware (no audio)
cargo xtask build  --board a1s --features web      # the audio firmware plus WiFi, a page and uploads
cargo xtask build  --board a1s --features web,lcd  # both
cargo xtask image  --board a1s [--merge]           # an espflash image under target/
cargo xtask size   --board a1s                     # the image against its partition
cargo xtask assets [--force]                       # regenerate the module images and gzip the page
cargo xtask build  --board a1s --dev               # release codegen, debug assertions on

cargo xtask build  --board c5                      # boot-and-idle smoke build, no module
cargo xtask build  --board c5 --features bench     # the RISC-V bench (M8-I4)
cargo xtask size   --board c5 --features bench     # the bench image against its partition
```

`--board a1s` is also the default, so a bare `cargo xtask build` (no `--board`) builds the
A1S — `--board c5` always has to be spelled out.

`cargo xtask assets` runs the **main** workspace's `cargo xtask module-images`, which
writes `embedded/assets/*.spmi` — the module images the firmware links with
`include_bytes!` — and gzips `www/index.html` into `embedded/assets/index.html.gz`, which
the `web` build links the same way. All of them are git-ignored build products, and
`build`, `image` and `size` generate them automatically when they are missing or stale, so
a fresh clone builds. The gzipped page has a hard 12 KiB budget and the asset step fails
the build if it is exceeded (it is 6.6 KiB today).

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

# the audio firmware: plays REFLEX.S3M out of the headphone jack
cargo xtask flash --board a1s
cargo xtask monitor --board a1s

# the C5 (M8-I4): one flash, no audio, no listening check — see the "C5" note below
cargo xtask flash --board c5 --features bench
cargo xtask monitor --board c5
```

`flash` builds a merged image (bootloader + partition table + app) with `--skip-padding`,
so a reflash leaves the `modules` and `config` partitions alone, and attaches the monitor
when it is done.

The Audio Kit's USB port is a CP2102; on Linux it appears as `/dev/ttyUSB0` and needs the
user to be in the `dialout` group. If espflash cannot get the board into the bootloader,
hold **BOOT** (KEY1 area, the button marked `IO0`) while tapping **EN/RST**.

### The C5

There is no "audio" build to flash — the chip has no codec and no DAC, so `--features
bench` is the only build worth flashing at all. `espflash` autodetects the chip
(`esp32c5`) over USB the same way it does for the A1S; hold the board's **BOOT** button
while tapping **RESET** if it does not enter the bootloader on its own. The transcript's
`BENCH … linear flash sha256=…` lines are the exit criterion (they must equal the same
`goldens/` hashes the A1S's do), and the full log is what fills
`plans/reference/embedded-budget.md`'s C5 rows — its §6 has the line-by-line mapping.

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

The **C5** bench prints the same line shape from the same shared code
(`firmware_common::bench`), so the two boards' transcripts can be compared line for line:

```text
=== StarPlayer C5 bench (RISC-V, no audio hardware) ===
BUILD profile=release kernels=nearest,linear,cubic,sinc
CPU  240 MHz  (240000000 cycles/s)
SIZE voice=176 bytes  render_half=8344 bytes
HEAP [boot] …
STAGING dram=ok psram=unavailable (this devkit has no PSRAM)
IMAGE synthetic-mod bytes=6072 (5.9 KiB)
BENCH synthetic-mod nearest flash sha256=… frames=441000 … cycles_per_frame=… core_load=…%
…
BENCH petri-s3m nearest dram skipped=no staging buffer large enough
…
idle
```

Its `sha256=` lines must equal the **same** `goldens/` hashes the A1S's do — that equality
on *both* boards is M8's exit criterion, not just the A1S's half of it. `petri-s3m`'s
`dram` rows are always `skipped=`, on this board as on the A1S: its 88 036-byte image does
not fit the 32 KiB `DRAM_STAGING_BYTES` buffer, and there is no `psram` row at all — this
devkit has none fitted.

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
CORE1 audio refill running
ord 000 pat 000 row 00/06 125 bpm   3/ 8 voices  0:01/2:47  peak=… underruns=0 …
```

An `lcd` build's boot log has one more line between `PLAY` and `CORE1` — `LCD  ST7789
found` or `LCD  ST7789 not found — continuing headless`, from `LcdDisplay::take`'s probe.
"Not found" is not fatal: the audio still plays and the six-vs-five-key map is unaffected
either way (`lcd` always drops KEY2, whether or not a panel actually answered).

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

This section is the **A1S**'s pin map. The C5 touches no peripheral beyond the CPU clock,
the heap and, in the `bench` build, the `mcycle` CSR — there is no pin map for it because
there is nothing wired to describe.

| Function | GPIO | Note |
|---|---|---|
| I2C SDA / SCL | 33 / 32 | ES8388 control, 7-bit address `0x10` |
| I2S MCLK | 0 | `CLK_OUT1`; a strapping pin — MCLK only appears after boot |
| I2S BCLK / LRCK | 27 / 25 | |
| I2S DOUT (ESP → codec DSDIN) | 26 | |
| I2S DIN (codec ASDOUT → ESP) | 35 | unused; input-only pin |
| PA enable (speaker amplifier) | 21 | driven high once audio is flowing |
| Headphone detect | 39 | input-only |
| KEY1–KEY6 | 36, 13, 19, 23, 18, 5 | KEY1 is on an input-only pin; six keys by default, five (no KEY2) when `--features lcd` is built |
| LED4 / LED5 | 22 / 19 | LED5 shares KEY3 |
| Display (`--features lcd`, HSPI on the SD-card pin group) | 14 SCK, 13 MOSI, 15 CS, 2 DC, 4 RST | 12 and 15 are strapping pins; **13 is also KEY2**; GPIO12 is never wired to the panel and carries no pull |

The map lives in exactly one place in code: `boards/starplayer-a1s/src/board.rs`.

### Keys (M8-I5)

Six push-buttons, KEY1–KEY6, debounced (two agreeing 10 ms samples) and polled by
`boards/starplayer-a1s/src/keys.rs`; the debounce/edge/hold-repeat state machine itself is
`firmware-common`'s `keys` module — host-tested, fed `(now_ms, [bool; 6])`.

Six-key map (default build):

| Key | GPIO | Press | Hold ≥ 1 s |
|---|---|---|---|
| KEY1 | 36 | play / pause | — |
| KEY2 | 13 | stop (rewind to order 0) | — (I6: re-provision) |
| KEY3 | 19 | previous order | seek back one order every 200 ms |
| KEY4 | 23 | next order | seek forward one order every 200 ms |
| KEY5 | 18 | volume − (1/16 step) | repeat every 200 ms |
| KEY6 | 5 | volume + (1/16 step) | repeat every 200 ms |

Five-key map (`--features lcd`, GPIO13 is the display's MOSI so KEY2 is unavailable):

| Key | GPIO | Press | Hold ≥ 1 s |
|---|---|---|---|
| KEY1 | 36 | play / pause | stop (rewind to order 0) |
| KEY3 | 19 | previous order | seek back one order every 200 ms |
| KEY4 | 23 | next order | seek forward one order every 200 ms |
| KEY5 | 18 | volume − (1/16 step) | repeat every 200 ms |
| KEY6 | 5 | volume + (1/16 step) | repeat every 200 ms |

Volume is applied through `ControlHalf::set_master_volume` and lives in RAM only — it does
not persist across a reset.

### The display (M8-I5, `--features lcd`)

An ST7789 SPI IPS panel, 1.69" 240×280 (a 240×320 ST7789 RAM windowed with a 20-row
y-offset), on HSPI: SCK 14, MOSI 13, CS 15, DC 2, RST 4, 20 MHz to start (research point 3
— raise towards the panel's 40 MHz ceiling once the owner has measured a clean redraw on
the board in hand). Off by **default**; `cargo xtask build --board a1s --features lcd`
builds it in. `boards/starplayer-a1s/src/lcd.rs` is the only file that names `mipidsi` or
`embedded-graphics`'s driver types; what is actually drawn is
`firmware-common::screen::Screen`, host-tested against `embedded_graphics::mock_display`.

The screen never panics if the panel is loose or absent: `LcdDisplay::take` goes headless
on the first SPI/init error (the same fail-soft shape ampkeeper's `display.rs` uses for its
I2C LCD backpack) and every draw call after that is a no-op.

`mipidsi = "0.9.0"` and `embedded-graphics = "=0.8.1"` — **not** the newer `mipidsi 0.10.0`
/ `embedded-graphics 0.8.2` (M8-I5 research point 2). The newer pair tightens its
`fixed`/`az` version requirements to a range that cannot be satisfied alongside
`starplayer-core`'s `fixed = "1.31"` at all — `cargo` refuses to resolve a single `az`
version for the whole graph. See the long comment beside `embedded-graphics` in
`embedded/Cargo.toml`'s `[workspace.dependencies]` for the exact ranges. Neither mipidsi
release needs `display-interface-spi`: both carry their own
`mipidsi::interface::SpiInterface`.

### DIP switches

The Audio Kit carries a five-way DIP switch block beside the SD slot. It multiplexes the
SD-card pin group between the slot, the JTAG header and the key matrix, and the silkscreen
labelling differs between board revisions. For **this** firmware:

* nothing here uses the SD card or JTAG, so any switch position that does not route the
  group to the SD slot boots and plays;
* the six-key default build reads KEY1–KEY6 as plain GPIO inputs — no SD card, no display,
  the SD-card pin group's DIP position does not matter to it;
* the `--features lcd` build takes the SD-card pin group (GPIO14/13/15/2/4) for the ST7789
  display, which means the SD slot must be switched **off** — a display and an SD card
  cannot both have those pins, and GPIO13 being both KEY2 and the display's MOSI is exactly
  why the `lcd` build drops to five keys.

Leave them as the board shipped for the six-key default build; switch the SD-card group off
before flashing an `lcd` build, and record what the board in hand is actually set to when
you do.

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
  rust-toolchain.toml     channel = "esp-1.97"  (the A1S's; the C5 is built under +1.97 instead)
  .cargo/config.toml      the `xtask` alias and nothing else
  firmware-common/        board-independent: the bench runner, the now-playing view model,
                          the key debounce state machine, the screen renderer, the
                          formatting helpers — builds and tests on the host
  boards/starplayer-a1s/  the Xtensa firmware
    .cargo/config.toml    target, runner, -Tlinkall.x, build-std  (directory-scoped!)
    partitions.csv
    src/board.rs          the pin map, once
    src/es8388.rs         the codec driver
    src/audio.rs          I2S + circular DMA + the refill task (runs on core 1)
    src/keys.rs           the six push-buttons as GPIO inputs, always built
    src/lcd.rs             the ST7789 SPI driver, #[cfg(feature = "lcd")] only
    src/images.rs         the aligned include_bytes! wrappers
    src/bench.rs          the `bench` build's runner
    src/psram.rs          the PSRAM arena, #[cfg(feature = "web")] only (M8-I6)
    src/store.rs          the config and modules partitions, `web` only
    src/net.rs            station mode, embassy-net and mDNS, `web` only
    src/provisioning.rs   the captive portal personality, `web` only
    src/web.rs            picoserve, the JSON/WebSocket API and module upload, `web` only
    src/main.rs           boot, core pinning, the control/keys/display tasks
    ld/stack-floor.x      a linker ASSERT that fails the build if the main stack drops
                          under 24 KiB (see §6)
    build.rs              adds that fragment to the link
  www/index.html          the page the `web` build serves, gzipped into assets/ by xtask
  boards/starplayer-c5/   the RISC-V bench firmware (M8-I4) — no audio hardware
    .cargo/config.toml    target, runner, -Tlinkall.x — no build-std (research point 2)
    partitions.csv
    src/images.rs         the aligned include_bytes! wrappers (bench-only)
    src/bench.rs          the `mcycle`-backed board glue over firmware-common's runner
    src/main.rs           boot — synchronous `#[esp_hal::main]`, no embassy/esp-rtos
  assets/                 git-ignored build products: *.spmi module images, index.html.gz
  xtask/                  build / image / size / assets / flash / monitor
```

## 6. Web control (M8-I6, `--features web`)

Off by default. A build without it links no radio, no TCP/IP stack and no HTTP server, and
is byte-for-byte the firmware M8-I5 shipped.

### Getting the player onto a network

There are no build-time credentials (M8 master-plan decision 5). The first boot with no
stored network — and any boot where the re-provision key is held — comes up as a **captive
portal** instead:

1. Flash `--features web` (or `web,lcd`) and power the board. It plays the compiled-in
   module in both personalities, so the music is how you know it is alive.
2. Join the open network **`StarPlayer-XXXX`** from a phone or laptop (the suffix is the
   last two bytes of the board's SoftAP MAC, so two boards on one desk are
   distinguishable). The phone's own captive-portal detection should open the page; if it
   does not, browse to **http://192.168.4.1/**.
3. Pick the network from the list the board scanned before it became an access point, type
   the passphrase, and press **Save and restart**. The board writes the credentials to the
   `config` partition and soft-resets two seconds later — the delay is what lets the
   confirmation page reach the phone before the radio goes.
4. It comes back in station mode and answers at **http://starplayer.local/** (mDNS). The
   UART log prints the DHCP address too, for a network where mDNS does not work.

**To provision it again**, either hold the re-provision key at boot for five seconds, or
`curl -X POST http://starplayer.local/api/reprovision` — both end with the portal. The key
is **KEY2** in the six-key build and **KEY1** in the `lcd` build, where KEY2's GPIO is the
display's MOSI and there is no KEY2 at all. A boot that nobody is touching costs one GPIO
read; the key must be held from power-on, and the music plays while you hold it.

There is no password on the portal and none on the API. The threat model is a music player
on a desk; anyone already on the network can change the song, which is the point.

### The API

Port 80. Everything is JSON in and JSON or plain text out; every refusal carries a sentence
written for a person, not a code.

| Method, path | Body | Answer |
|---|---|---|
| `GET /` | — | the page, gzipped, `Content-Encoding: gzip` |
| `GET /api/status` | — | the transport, the module and up to 16 channel rows |
| `GET /api/modules` | — | the compiled-in module plus every stored slot |
| `POST /api/play`, `/api/stop`, `/api/next`, `/api/previous` | — | 204 |
| `POST /api/seek` | `{"order":12}` | 204 |
| `POST /api/volume` | `{"level":32768}` | 204 (0–65535) |
| `POST /api/mute` | `{"channel":3,"muted":true}` | 204 |
| `POST /api/modules` | raw module bytes, or a `.spmi` image | 201, or 409/413/507 with the reason |
| `POST /api/modules/select` | `{"id":2}` | 204 — `0` is the compiled-in module, `1..5` a flash slot |
| `POST /api/modules/store` | `{"id":3}` | 204 — **pauses playback**, see below |
| `POST /api/reprovision` | — | 204, then a reset into the portal |
| `GET /ws` | WebSocket | binary telemetry at 10 Hz; accepts 9-byte command frames |

```sh
curl http://starplayer.local/api/status
curl -X POST http://starplayer.local/api/play
curl -X POST --data-binary @PETRI.S3M http://starplayer.local/api/modules
curl -X POST -H 'content-type: application/json' -d '{"id":1}' http://starplayer.local/api/modules/store
```

The WebSocket's wire format is the **same flat word layout the browser player uses**
(`crates/starplayer-host-wasm/src/lib.rs`): 22 header words then eight words per channel,
little-endian `i32`. `firmware-common/src/api.rs` owns this end of it and its host tests
are what keep the two in step.

### Uploading a module, and what actually fits

Two kinds of file are accepted at `POST /api/modules`:

* **A module image** (`.spmi`, what `cargo xtask module-image` writes in the main
  workspace). It is streamed into PSRAM and played **borrowed in place** — no decoding, no
  heap, and the only limit is the 512 KiB PSRAM buffer. This is the way to put
  `PETRI.S3M`-sized music on the device.
* **A raw module file** (`.mod`, `.s3m`, `.mtm`, `.xm`, `.it`). The device decodes it with
  the ordinary loader, which allocates the decoded PCM **in DRAM**, serialises the result
  back out to PSRAM, and frees the DRAM. The peak is the decoded module plus its image at
  once, against a 96 KiB heap that already holds the engine — so this path is for small
  modules, and it refuses with a sentence saying so rather than running out of memory. The
  `web` build's heap lives in `dram2_seg`; `HEAP.stats()` is printed at boot.

The `Arc` that owns an uploaded module, and everything else with an atomic in it, stays in
**DRAM**: on the classic ESP32 the atomic instructions do not work on PSRAM, so this
firmware never registers PSRAM with the allocator at all (`src/psram.rs` has the full
argument).

### The flash-write pause — the one thing that is audible

On the classic ESP32 an erase or a write **turns the instruction and data caches off** for
its duration. Everything mapped through that cache becomes unreadable: code executing from
flash, the compiled-in module image, and PSRAM, which shares the cache. esp-storage refuses
a write outright while the second core is running, and this firmware runs the audio refill
on core 1 — so a write parks it.

`POST /api/modules/store` therefore **stops the transport first**, waits for the 64-frame
ramp and the 23 ms DMA ring to drain to silence, writes, and plays again. Storing a 90 KB
module is a few seconds of silence, and that is the designed behaviour, not a fault; the
page warns about it beside the button. `POST /api/modules/select` for a flash slot does the
same for a long read, which is safe but contends for the flash bus badly enough to cost
underruns. Saving WiFi credentials pauses playback the same way, and is followed by a reset
anyway.

### The DRAM budget

The `web` build is close to the chip's limit and the limit is **not** flash (52 % of the
partition) — it is the 192 KiB of internal DRAM, out of which core 0's main stack is
whatever `.bss` leaves behind. Two things keep it in bounds, and both are easy to undo by
accident:

* **The heap is in `dram2_seg`, not `.bss`** (`main.rs`'s `HEAP_BYTES`). With a `.bss` heap
  the `web` build does not link at all.
* **Routing is one flat `match`, and every JSON answer is one body type** (`web.rs`).
  picoserve monomorphises `write_to` per response type, and a `.route()` chain costs a
  stack frame per route. The first draft's two web workers were 97 KiB of `.bss`; they are
  68 KiB now.

`ld/stack-floor.x` fails the build if the main stack drops under 24 KiB. It is 29 756 B in
the `web,lcd` build. If it fires, shrink a static or move it to `dram2_seg` — do not lower
the floor.

## 7. Notes for the next person

* **`-C force-frame-pointers` is deliberately absent** from the board's rustflags. With it
  on, this firmware does not compile: the Xtensa LLVM register allocator fails with
  `Cannot scavenge register without an emergency spill slot`. Ampkeeper hit the same bug
  from the other direction on the S3. The comment in `.cargo/config.toml` has the detail.
* **The heap is 120 KiB and the number is a balance against the stack.** On the classic
  ESP32 core 0's main stack is whatever DRAM is left between `_bss_end` and `0x3ffe_0000`,
  so every heap byte (and every other `.bss`/`.data` byte — core 1's 8 KiB stack included,
  since M8-I5) is a byte core 0's stack does not get. At 160 KiB the firmware links and
  leaves 6 472 bytes of stack, which will not survive a boot; at 120 KiB the default
  (six-key, no `lcd`) build's stack is 38 712 bytes and the `lcd` build's is 37 312 bytes —
  both comfortably clear of the 6 472-byte failure point, but check after any change that
  moves a large static:

  ```sh
  xtensa-esp32-elf-nm target/xtensa-esp32-none-elf/release/starplayer-a1s \
    | grep -E " (_bss_end|_stack_start)$"
  ```

* **PSRAM is mapped but not added to the general heap** in the audio build. The ESP32's
  atomic instructions do not work correctly on PSRAM, and this engine puts atomics on the
  heap (`Arc` refcounts, the host's seqlocks, the telemetry ring). `src/main.rs` has the
  full reasoning; M8-I6 inherits the constraint.
* **The audio refill task runs on core 1** — moved there by M8-I5. M8-I3's write-up blamed
  `RenderHalf` not being `Send` on the engine's `Box<dyn EventSource>` and kept everything
  on core 0 as a result; that diagnosis did not hold once actually checked (`EventSource`
  and `Insert` both already carry a `Send` supertrait bound, so `RenderHalf` was already
  `Send` — see the compile-time assertion beside its definition in
  `crates/starplayer-host-embedded/src/player.rs`). The real, and genuinely unavoidable,
  `Send` obstacle was `esp_hal`'s own DMA transfer type (`audio::AudioTransfer`), which
  main.rs crosses with a small `unsafe impl Send` wrapper (`SendTransfer`) whose safety
  argument is a one-time ownership handoff into core 1's entry closure, the same shape
  `esp-rtos`'s own internal `SecondCoreStack` wrapper uses. Core 0 now runs the keys task,
  the control task and (under `lcd`) the display task; see the "Core pinning" section of
  `src/main.rs`.
* The measured sizes live in `plans/reference/embedded-budget.md`, which is where any new
  number belongs.
* **The C5 needs no `-Z build-std` and no Xtensa toolchain at all** — it is built under
  the main workspace's own pinned stable `1.97`, not `esp-1.97` (M8-I4 research point 2,
  `plans/reference/embedded-budget.md` §8). This is a **per-board** choice
  (`embedded/xtask`'s `Board::toolchain`), not a workspace-wide one: `esp-1.97` still pins
  `embedded/rust-toolchain.toml` at the root for the A1S, and `cargo +1.97` on the C5's
  invocation overrides that per build.
* **The C5's `bench` heap is 176 KiB, not the A1S's 120 KiB**, because this board has no
  PSRAM to take `DRAM_STAGING_BYTES` off the internal heap the way the A1S's `External`
  region does. `boards/starplayer-c5/src/main.rs`'s `HEAP_BYTES` doc comment has the
  arithmetic; `plans/reference/embedded-budget.md` §2 has the budget it is sized against.
