# M8 — Embedded proof

| Field | Value |
|---|---|
| Goal | The ESP32-A1S Audio Kit plays a module from flash through its ES8388 codec under Embassy — with buttons, an optional ST7789 now-playing screen and web control — and the ESP32-C5 proves the RISC-V build by rendering the goldens bit for bit |
| Estimate | 2.5u (was 1u before the hardware, controls, display and web control were added) |
| Depends on | M2 (fixed-point mixer), M3 (host abstraction, telemetry snapshot), M7 (interpolators) — all landed |
| Trigger | **Pulled 2026-09-11.** The owner has an ESP32-A1S Audio Kit and an ESP32-C5 devkit on the bench |

## Why it matters even though CI already checks `no_std`

The `riscv32imc-unknown-none-elf` CI check has proven since M0 that the core crates
**compile** without `std`. This milestone proves they **play** — which is a different
claim, and the one that finds the problems: RAM budget, flash-resident sample data,
fixed-point audio quality, and whether the async story actually holds.

Doing it before the API stabilises means the portability claims are validated rather than
merely asserted. Doing it after would mean discovering the constraints once they are
expensive to satisfy.

## The hardware (re-planned 2026-09-11)

The original plan targeted a RISC-V C3/C6 devkit and ruled Xtensa out. The owner's
hardware reverses that:

- **AI-Thinker ESP32-Audio-Kit, ESP32-A1S module** — classic ESP32 (Xtensa LX6, two cores
  at 240 MHz), 4 MB flash, PSRAM (4 MB on most modules; confirm at boot), **ES8388**
  codec on I2S + I2C (owner confirmed the v2.2+ revision; the older AC101 revision is out
  of scope), headphone jack and two speaker amplifier outputs, six push-buttons
  KEY1–KEY6, two user LEDs, a micro-SD slot whose pins we repurpose for the display. The
  Rust target is `xtensa-esp32-none-elf`, which needs the `esp` toolchain from `espup`.
- **ESP32-C5 devkit** — RISC-V (`riscv32imac-unknown-none-elf`), no audio hardware. It
  proves the second architecture, the plain-`rustup` toolchain path, and the CPU budget on
  a single 240 MHz RISC-V core, by rendering the golden fixtures and printing their hashes
  over UART.
- A **1.69" 240×280 ST7789 SPI IPS display**, wired to the SD-card pin group.

`../ampkeeper/esp32` is the proven reference for the Rust/Embassy stack on this family:
esp-hal 1.1.1, esp-rtos 0.3, esp-radio 0.18, embassy-executor 0.10 / -time 0.5 / -sync 0.8
/ -net 0.9, picoserve 0.18, esp-alloc 0.10, espflash only, everything driven by an xtask.
It targets the ESP32-S3 today; its classic-ESP32 era (every commit before `aeb9039`,
"Collapse the chip feature axis") holds the vanilla-ESP32 dependency table and cargo
config that worked, and its `provisioning.rs` is the captive portal I6 copies in shape.

## Decisions (taken while planning; do not re-litigate in the task files)

1. **The firmware is its own workspace at `embedded/`**, like `fuzz/` — it needs the `esp`
   toolchain (`espup install --toolchain-version 1.97.0.0`, matching the workspace's
   `rust-version = "1.97"`), `build-std`, and per-target dependency tables that must not
   be resolved by the main workspace's `cargo metadata`. Board-independent host logic lives
   in the main workspace as a `no_std` crate so the existing bare-metal CI covers it.
2. **Fixed path, `Linear`, i16 stereo, 44 100 Hz.** That is the goldens' configuration, so
   an on-device render can be hashed and compared against `goldens/` with no listening
   required. The host boundary is `&mut [i16]` — the std host's `f32` callback is not used.
3. **Flash-resident modules are host-built module images** (I2): the loader runs on the
   development machine, the built `Module` is serialised with its i16 PCM 4-byte aligned,
   and the device borrows the PCM straight out of the memory-mapped image. Modules
   uploaded at run time (I6) are loaded on the device into the PSRAM heap instead.
4. **The display takes the SD-card pin group** (HSPI: GPIO 14/13/15/2/4), so all six keys
   stay available for controls and the SD slot is unsupported. The display is a
   **build-time feature** (`lcd`), off by default, so the audio path is testable without it.
5. **WiFi credentials come from a captive portal**, modelled on ampkeeper's — not from
   build-time environment variables.
6. **No `unsafe` in `crates/`.** The firmware crates under `embedded/` are the only place
   `unsafe` is tolerated (esp-hal's heap and static-cell macros, DMA buffers), each site
   with a `// SAFETY:` comment.
7. **`portable-atomic`'s `critical-section` path is not exercised by either board.** The
   ESP32 (LX6) and the ESP32-C5 (`imac`) both have native compare-and-swap, so
   `target_has_atomic = "ptr"` holds and `starplayer-rt`'s target-conditional dependency
   never engages. The claim stays verified by the `riscv32imc` CI check; on hardware the
   claim is only that the core crates build and play with esp-hal's `critical-section`
   implementation present. Architecture §10 is corrected in the same change (ESP32 and
   ESP32-S3 have CAS; ESP32-S2 does not).

## Deliverables

1. **`crates/starplayer-host-embedded`** — a `no_std` player over the fixed path with the
   std host's quantum-aligned control cadence, an i16 render entry point, a command mailbox
   and a bench routine (I1).
2. **Borrowed sample data** — `Module` can borrow its PCM and pattern blob from a
   `'static` slice, and a module-image format plus `cargo xtask module-image` produce what
   it borrows (I2). This is what the offsets-not-references decision in architecture §6
   was for, and this milestone is where it pays off.
3. **The ESP32-A1S plays** — the `embedded/` workspace, ES8388 + I2S DMA bring-up, a
   module image played from flash at boot, and a bench build that renders every golden
   fixture and prints its SHA-256 and cycles per frame (I3).
4. **A written budget** — RAM, flash and CPU per voice at 44.1 kHz, per interpolator,
   with the PCM in flash, PSRAM and DRAM — in `plans/reference/embedded-budget.md`, so a
   future project can size a target before trying (I3, extended by I4).
5. **An answer to architecture open question Q2**: is 128 the right `RENDER_QUANTUM` for
   embedded, or does this path want a compile-time override? Recorded in the architecture
   document (I3).
6. **The ESP32-C5 build** — the same bench on RISC-V, a second bare-metal target in CI, and
   an `embedded.yml` workflow that builds both boards (I4).
7. **Controls and display** — six keys mapped to transport and volume, and the ST7789
   now-playing screen behind the `lcd` feature (I5).
8. **Web control** — captive-portal provisioning, a JSON/WebSocket API over picoserve, a
   small embedded page, and module upload into PSRAM (I6).

## The task graph

Task letter **I**. `I1 → I2 → I3` is the critical path to the exit criterion; I4, I5 and I6
are independent of each other once I3 has landed. I1 and I2 can be built concurrently in
separate worktrees — I3 is the first consumer of both.

| Task | Deliverable | Depends on | Parallel with | Model |
|---|---|---|---|---|
| [I1](complete/M8-task-I1-embedded-host-core.md) | `starplayer-host-embedded`: `EmbeddedPlayer`, cadence, mailbox, bench | — | I2 | Opus |
| [I2](complete/M8-task-I2-flash-resident-modules.md) | `PcmStorage::Borrowed`, the module image, `xtask module-image` | — | I1 | Opus |
| [I3](complete/M8-task-I3-esp32-a1s-bringup.md) | `embedded/` workspace, ES8388/I2S, plays from flash, budget doc, Q2 | I1, I2 | — | Opus |
| [I4](complete/M8-task-I4-esp32-c5-riscv-bench.md) | C5 bench firmware, second CI target, `embedded.yml` | I3 | I5, I6 | Sonnet |
| [I5](complete/M8-task-I5-buttons-and-display.md) | Six keys, ST7789 now-playing screen (`lcd` feature) | I3 | I4, I6 | Sonnet |
| [I6](complete/M8-task-I6-web-control.md) | Captive portal, HTTP/WebSocket API, page, module upload | I3 | I4, I5 | Opus |

## Exit criteria

- The ESP32-A1S plays `REFLEX.S3M` (the compiled-in boot module) from flash cleanly through the headphone jack; `PETRI.S3M` reaches it through the bench build or a web upload.
- The bench build's on-device SHA-256 of the 10-second fixed/mono/linear render of every
  golden fixture equals the committed hash under `goldens/`, on the A1S and on the C5.
- `plans/reference/embedded-budget.md` holds measured numbers, and Q2 is settled.
- Buttons control playback; the `lcd` build shows the sounding row; the page at
  `http://starplayer.local/` plays, stops, seeks and uploads a module.

## Out of scope

The AC101 codec revision of the Audio Kit. SD-card playback. Audio *input*. OTA updates
and signed images (ampkeeper solved these; not needed to prove the engine). Any ESP32-S2
or ESP32-S3 board. A `no_std` port of `starplayer-host` itself — I1 records where its
cadence logic could later be shared, and stops there.
