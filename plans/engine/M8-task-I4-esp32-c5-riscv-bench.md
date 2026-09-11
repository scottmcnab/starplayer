# M8 — I4: ESP32-C5 — the RISC-V bench build and the second bare-metal CI target

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md), deliverable 6) |
| Status | Planned 2026-09-11; pulled |
| Depends on | I3 (the `embedded/` workspace, `firmware-common`'s bench runner, the budget document) |
| Blocks | — |
| Parallel with | I5, I6 |
| Recommended model | Claude Sonnet (a second board crate over an existing bench runner, a CI matrix entry and a workflow; the one unknown is the HAL's C5 feature name) |
| Verified by | agent (`cargo xtask build --board c5` and the new CI jobs green), then owner flashes and reads the UART transcript |

## Context for a fresh agent

M8's original goal was a RISC-V esp32; the owner's audio hardware is Xtensa, so the
RISC-V half of the milestone became this task: prove that the same firmware-common bench
builds and runs on a RISC-V core with the plain LLVM target
`riscv32imac-unknown-none-elf`, that its golden hashes match, and what a single 240 MHz
RISC-V core costs per voice. The ESP32-C5 has no audio device; nothing here makes sound.

Two CI facts matter. The main workspace's bare-metal check uses `riscv32imc-unknown-none-elf`
(`xtask/src/main.rs:18`, `BARE_METAL_TARGET`) precisely because that target has **no**
atomics and forces `portable-atomic`'s `critical-section` path (architecture §10). The
C5 is `imac` — it has native CAS — so it is a second, different target, not a replacement:
keep `imc` as the no-CAS canary and add `imac` as the "a real chip's target" check. Both
are cheap `cargo check`s.

ESP32-C5 support in esp-hal is recent: tracked in esp-rs/esp-hal issues #4733 and #4734
against the 1.1.0 milestone, and reported working under Embassy in August 2026. The
feature name and which of esp-rtos / esp-println / esp-backtrace already carry `esp32c5`
is research point 1; the fallback keeps the task deliverable without them.

### Code you must read before changing anything

- `embedded/boards/starplayer-a1s/` (I3) — `Cargo.toml`'s target table, `.cargo/config.toml`,
  `bench.rs`, `main.rs`'s heap set-up. The C5 crate is this minus the codec, display and
  web, with a RISC-V cycle counter.
- `embedded/firmware-common/` — the bench runner it calls.
- `embedded/xtask/src/main.rs` — the `--board` resolution table; add `c5`.
- `xtask/src/main.rs` — `BARE_METAL_TARGET`, `NO_STD_CRATES`, `job_no_std_check`,
  `job_no_std_purity`, `JOBS`.
- `rust-toolchain.toml` (main workspace) — targets list.
- `.github/workflows/ci.yml` (`no-std-check`, `no-std-purity` jobs) and
  `../ampkeeper/.github/workflows/esp32.yml` (`esp-rs/xtensa-toolchain@v1.7.0` with
  `version: "1.95.0.0"`, `Swatinem/rust-cache`, the partition-size gate).
- `plans/reference/embedded-budget.md` — the C5 rows to fill.

## Deliverables

### 1. `embedded/boards/starplayer-c5`

`[target.riscv32imac-unknown-none-elf.dependencies]` with esp-hal 1.1.x `["esp32c5",
"unstable"]`, esp-rtos `["esp32c5", "embassy"]`, esp-println, esp-backtrace, esp-alloc,
esp-bootloader-esp-idf — whichever carry `esp32c5` (research point 1). `.cargo/config.toml`:
`target = "riscv32imac-unknown-none-elf"`, `runner = "espflash flash --monitor"`,
`-Tlinkall.x`, `build-std`. Only the `bench` behaviour: boot, print chip and clock, run
the bench runner over the six fixture images with the PCM in flash (and in DRAM for the
ones that fit; the C5 has no PSRAM), print `name sha256 frames cycles cycles_per_frame`
per interpolator using `riscv::register::mcycle`, print heap high-water and
`size_of::<Voice>()`, then idle.

### 2. CI

- `rust-toolchain.toml`: add `riscv32imac-unknown-none-elf`.
- `xtask/src/main.rs`: make the bare-metal target a list —
  `BARE_METAL_TARGETS = ["riscv32imc-unknown-none-elf", "riscv32imac-unknown-none-elf"]`
  — and run `job_no_std_check` and `job_no_std_purity` over both (the `simd` job's
  bare-metal pass stays on `imc`; note why in a comment). Keep the doc comment that
  explains `imc` is the no-CAS canary and `imac` is the C5's real target.
- `.github/workflows/embedded.yml`: on push and PR, one job per board, using
  `esp-rs/xtensa-toolchain@v1.7.0` with `version: "1.97.0.0"` and `buildtargets: esp32`
  for the A1S (the same toolchain builds the C5, or use `dtolnay/rust-toolchain` with the
  `riscv32imac` target if research point 2 finds that simpler); `cargo xtask assets` needs
  the main workspace's stable toolchain first; then `cargo xtask build --board <b>` for
  the default and `bench` feature sets and `cargo xtask size` with a 90 % warn / 100 %
  fail gate against `partitions.csv`, as ampkeeper's workflow does. `Swatinem/rust-cache`
  keyed on `embedded/`.
- `embedded/xtask`: `--board c5`.

### 3. The budget document

Fill the C5 rows: flash size of the bench build, cycles per frame per interpolator with
the PCM in flash, frames/s per voice, and a one-paragraph comparison with the LX6.

## Research points

1. **esp-hal C5 support at 1.1.x**: the feature name, whether esp-rtos 0.3 has an `esp32c5`
   feature, whether esp-println/esp-backtrace do. If the HAL cannot be pinned, the
   fallback is a HAL-less build on `riscv32imac-unknown-none-elf` with the `riscv-rt` crate
   and the ROM UART for output — the build target and the hashes are still proven; only
   the cycle count loses precision. Record which path was taken.
2. **Toolchain for RISC-V in CI**: the `esp` toolchain carries the riscv targets, but a
   plain `rustup` 1.97 with `riscv32imac-unknown-none-elf` may be simpler for the C5 job
   and proves the "no fork needed" claim for RISC-V. Prefer plain rustup if the HAL
   builds on stable + `build-std`-free; otherwise the `esp` toolchain.
3. **`opt-level`**: whether `"s"` or `3` changes cycles per frame materially on RISC-V;
   record both if the difference exceeds 10 %.

## Verification

```text
cargo xtask ci --job no-std-check      # now two targets
cargo xtask ci --job no-std-purity
cd embedded && cargo xtask build --board c5 --features bench
cd embedded && cargo xtask size --board c5
# owner
cd embedded && cargo xtask flash --board c5 --features bench && cargo xtask monitor
#   → sha256 lines equal goldens/ (Linear row); cycles/frame recorded in the budget doc
```

## Out of scope

Any audio output on the C5 (no DAC on the chip; an external I2S DAC is a future
follow-up). WiFi on the C5. Replacing the `imc` canary.
