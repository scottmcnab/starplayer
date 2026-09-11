# M8 — I4: ESP32-C5 — the RISC-V bench build and the second bare-metal CI target

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md), deliverable 6) |
| Status | Implemented 2026-09-11; builds link; awaiting owner hardware run |
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

## Research resolution

Implemented 2026-09-11 on branch `m8-i4`. Both feature sets of the C5 board link; nothing
has been flashed. Every figure that can be taken without the board is in
[`plans/reference/embedded-budget.md`](../reference/embedded-budget.md) (§ configuration
note, §1 flash, §2 RAM, §3 CPU table shapes, §8 toolchain); every figure that needs the
board says `TBD (owner: run the bench build)` there, with the exact command.

### 1. esp-hal C5 support at 1.1.x

**Full support, at the exact versions the A1S already pins — no fallback needed.**
`cargo info` against the live registry (not guessed, not read off a changelog) confirmed
an `esp32c5` feature on every crate the task names, all resolving to the versions the A1S's
`Cargo.toml` already requests and `embedded/Cargo.lock` already locked:

| Crate | Version requested (`^`) | Locked | `esp32c5` feature |
|---|---|---|---|
| `esp-hal` | `1.1.1` | **1.1.2** | yes |
| `esp-rtos` | `0.3.0` | 0.3.0 | yes (not used — see research point 2) |
| `esp-println` | `0.17.0` | **0.17.0** | yes |
| `esp-backtrace` | `0.19.0` | **0.19.0** | yes |
| `esp-bootloader-esp-idf` | `0.5.0` | **0.5.0** | yes |
| `esp-alloc` | `0.10.0` | **0.10.0** | yes |

So `boards/starplayer-c5/Cargo.toml` names the same version strings as the A1S's manifest
for every one of these crates, and one `embedded/Cargo.lock` resolves both boards to the
same locked versions — there is exactly one dependency story for this workspace, not two.
`esp32c5` PAC crate versions (`esp32c5 0.2.2` / `0.3.0` — esp-hal 1.1.2 depends on both,
one via `esp-riscv-rt`) are pulled in automatically and are not hand-pinned anywhere.

The only crate in the task's list this board does **not** depend on is `esp-rtos`: see
research point 2.

### 2. Toolchain for RISC-V in CI — plain `rustup` won outright, not as a fallback

**Plain stable `rustup 1.97` was tried first, and the HAL built clean on the first
attempt** — no build-std, no nightly feature, no fallback needed. The task's "prefer plain
rustup if … otherwise the `esp` toolchain" framing turned out to have only one live branch:

* `riscv32imac-unknown-none-elf` has a prebuilt `core`/`alloc` component under plain
  `rustup` — confirmed by `rustup target add riscv32imac-unknown-none-elf --toolchain
  1.97` succeeding (`rust-std` downloaded, no source build) and then `cargo +1.97 build
  --target riscv32imac-unknown-none-elf` linking a real ESP32-C5 firmware with no
  `[unstable] build-std` section anywhere in its `.cargo/config.toml`.
* The named `esp-1.97` toolchain does **not** carry a prebuilt component for this target:
  `ls ~/.rustup/toolchains/esp-1.97/lib/rustlib` has only `x86_64-unknown-linux-gnu` (its
  host triple) and `src`. Building the C5 there would have needed `-Z build-std` for the
  C5 too — for no benefit, since the target triple `riscv32imac-unknown-none-elf` is
  identical either way and esp-hal does not care which toolchain compiles it.
* **No external cross-linker is needed at all.** `riscv32imac-unknown-none-elf` links with
  rustc's self-contained `rust-lld`; confirmed by building the C5's entire dependency graph
  standalone with nothing but the target component installed and no toolchain-provided
  linker on `PATH`. `embedded/xtask`'s `Board::linker` is `None` for the C5 for exactly
  this reason, and `require_linker` skips the check for it.
* **What made this possible on the code side, and the thing the task file did not
  anticipate**: the C5 board crate takes **no dependency on `embassy-executor` or
  `esp-rtos` at all**. `#[esp_hal::main]` (esp-hal's `blocking_main` procedural macro) is a
  synchronous, non-async entry point that needs no nightly language feature — unlike the
  A1S, whose `#[embassy_executor::task]`/`#[esp_rtos::main]` pair needs
  `#![feature(impl_trait_in_assoc_type)]`, which is what actually forces a nightly compiler
  on that board, independently of the target's own prebuilt-component story. The C5 has no
  DMA refill task and no control task running concurrently with anything — it renders six
  fixtures once at boot and idles — so there was never a reason to pull in an executor, and
  removing that dependency removed the one thing that would have forced nightly regardless
  of the target.
* `embedded/xtask`'s `Board` struct grew a `toolchain: &'static str` field as a result
  (`"esp-1.97"` for the A1S, `"1.97"` for the C5), and `board_command` now names it
  explicitly (`cargo +<toolchain>`) rather than hard-coding `+esp-1.97`. A `+toolchain`
  argument overrides whatever `embedded/rust-toolchain.toml` would otherwise resolve to, so
  the workspace-root file staying pinned at `esp-1.97` costs the C5 nothing.
* `.github/workflows/embedded.yml`'s `c5` job therefore installs **only** the main
  workspace's own pinned toolchain (`rustup show` against the repository root's
  `rust-toolchain.toml`, which now lists `riscv32imac-unknown-none-elf` under `targets`
  alongside the existing `riscv32imc-unknown-none-elf`) — no `esp-rs/xtensa-toolchain`
  action, no second toolchain-install step. That one step is also what generates the module
  images (`cargo xtask module-images`, main workspace) before either board's build runs.

### 3. `opt-level`: `"s"` vs `"3"` on RISC-V

**Cycles cannot be measured without the board, and none are invented here** — the whole
comparison the task asks for needs a device's `mcycle` reading, and there is no device.
What *could* be measured without one is code size, as a cheap proxy for "does the compiler
make a materially different choice at the two settings" — and it does:

| Setting | `.text` | `.rodata` |
|---|---|---|
| `opt-level = 3` (shipped) | 300 150 | 163 336 |
| `opt-level = "s"` | 202 410 (−32.6 %) | 161 368 (−1.2 %) |

(`cargo +1.97 build --release --features bench --config
profile.release.opt-level='"s"'`, C5 `bench` build, otherwise identical to the shipped
profile — a one-off override, not a change to the committed profile, which stays
`opt-level = 3` for the same reason the A1S's does: this firmware's whole purpose is a
cycles-per-frame measurement of the mixer, and a figure taken at `"s"` would measure the
size/speed tradeoff instead of the mixer.) A 32.6 % `.text` reduction at `"s"` says the
compiler *is* making substantially different inlining and codegen choices between the two
settings on this target — consistent with x86-64/Xtensa experience and exactly why `"s"`
would be the wrong setting to publish a cycle count from — but it is not itself a cycles
figure and must not be read as one. Whether the **cycles** differ by more than 10 % between
the two settings is `TBD (owner)`; the two commands are identical to the ones above with
`--config profile.release.opt-level='"s"'` appended, and the resulting `cycles_per_frame=`
fields are what would go in `plans/reference/embedded-budget.md` §3 as a footnote if the
owner runs the comparison.

## What was done differently from the task file, and why

* **The C5 board crate depends on neither `esp-rtos` nor any `embassy-*` crate**, though
  the task's deliverable section lists `esp-rtos ["esp32c5", "embassy"]` as one of the
  per-target dependencies to add. Research point 2 above is the reason: this board has
  nothing to schedule, and not depending on an async executor is exactly what let it build
  under plain stable `rustup` in the first place. `#[esp_hal::main]` does the whole job of
  `main`.
* **Two feature sets, not one.** The task's deliverable describes "only the bench
  behaviour". The board ships a `default` (no features) smoke build — boot, print the chip
  and clock, idle, with no module image linked at all — alongside `bench`, mirroring the
  A1S's `default`/`bench` split in shape (though the C5's `default` build has no audio
  path to be a *smoke build of*, unlike the A1S's, where `default` **is** the audio
  firmware). This is what lets `.github/workflows/embedded.yml` build "the default and
  `bench` feature sets" for the C5 the same way it does for the A1S, per the task's own CI
  section, and it is a fast link-only check that costs nothing extra to keep green.
* **`HEAP_BYTES` is 176 KiB, not sized by analogy to the A1S's 120 KiB.** The two boards'
  `bench` builds share the same design — `DRAM_STAGING_BYTES` (32 KiB) is carved out of the
  **same** internal heap the engine allocates from, via `esp_alloc::HEAP.alloc_caps`, not a
  separate static — but the A1S's `bench` build also registers a 128 KiB **External**
  (PSRAM) heap region that takes the pressure off its internal one; the C5 has no PSRAM at
  all, so every byte, staging buffer and engine alike, competes for the one internal heap.
  The host formula's upper bound for `PETRI.S3M`'s engine (71 576 B) plus the staging
  buffer (32 768 B) is 104 344 B; 176 KiB (180 224 B) leaves close to double that as
  headroom rather than merely clearing it, because an inadequate heap here would abort
  partway through rendering exactly the fixture the milestone's exit criterion is stated in
  terms of, on a board with no debugger attached. `boards/starplayer-c5/src/main.rs`'s
  `HEAP_BYTES` doc comment has the arithmetic; confirmed to link with over 130 KiB of stack
  still free at that heap size (`readelf`'s `.stack` section — the C5's linker allocates the
  stack as "whatever RAM is left", so this is a linker-computed fact, not a guess).
* **Type sizes (`Voice`, `Snapshot`, `RenderHalf<Linear>`) were measured for the budget
  document without a board**, via a `#[used] static` holding the three `size_of::<T>()`
  values, compiled to an object file for `riscv32imac-unknown-none-elf` with no link step
  (`cargo build --crate-type=lib`) and read back with `readelf -x`. All three are
  byte-identical to the A1S's Xtensa figures (176 / 1 848 / 8 344), which is itself a
  finding: both targets are 32-bit little-endian with 4-byte pointers, so nothing in these
  types' layout is architecture-sensitive. This is a compiler fact, not a device reading,
  and `plans/reference/embedded-budget.md` §2 says so explicitly.
* **`readelf -S`/`readelf -x`, not a `riscv32-esp-elf-size`/`-nm`.** No such tool was
  installed or needed — `readelf` reads ELF section headers and raw section bytes
  independently of the target architecture that produced them, and its `.text`/`.rodata`/
  `.data`/`.bss` figures agree with `cargo xtask size`'s espflash-derived app-image size to
  the byte.
* **The partition table is the C5's own, not a copy of the A1S's.** `factory` is 1.5 MB
  (`0x180000`) against the A1S's 2.5 MB — there is no OTA, no `modules` and no `config` row
  on a board that stores nothing, and the C5's much smaller `bench` image (468 976 B,
  versus the A1S's `bench` image at 552 560 B despite linking the same six module images
  and the same four kernels — the difference is the codec driver, I2S, DMA and the board
  pin map, all absent here) did not need the A1S's headroom. The devkit's actual flash size
  is unconfirmed from here (no board in hand); `flash_size = "4mb"` is the common
  ESP32-C5-DevKitC-1 figure and the partition table's own comment says so, with the fix
  (`espflash board-info`) named for the owner if the board in hand differs.

## `unsafe` inventory

Master-plan decision 6 says `embedded/` is the only place `unsafe` is tolerated, each site
with a `// SAFETY:` comment. The C5 board crate has **two**, both in
`boards/starplayer-c5/src/bench.rs`, and both are the same pattern the A1S's bench module
already uses for the same reason (`Module::from_image` requires a `&'static [u8]`, and a
bench that copied each fixture into a fresh leaked allocation would exhaust the staging
buffer after two fixtures):

| Site | Why |
|---|---|
| `Staging::claim` → `esp_alloc::HEAP.alloc_caps` | The raw allocation the internal-RAM staging buffer needs. `GlobalAlloc::alloc`'s contract, with a non-zero constant layout; never freed, intended for a firmware that runs its rows once. |
| `Staging::load` → `slice::from_raw_parts_mut` | Turns that allocation into the `&'static [u8]` `Module::from_image` requires. The aliasing obligation — no live `Module` still borrowing the previous contents — is stated on the method and discharged at the one call site by an explicit `drop(module)` in [`run`]. |

`firmware-common` and `embedded/xtask` remain `#![forbid(unsafe_code)]`/unsafe-free (the
former already was; neither changed in that respect). `esp_alloc::heap_allocator!` expands
to `unsafe` internally; it does not appear in this crate's own text.

## Verification run

Every command below was run in this worktree. `. ~/export-esp-1.97.sh` was sourced (for
the A1S half only — the C5 half needs no toolchain environment at all, confirmed by
running its commands in a shell that had never sourced it).

| Command | Result |
|---|---|
| `cd embedded && cargo xtask build --board c5` | **links** — 39 904 B app image, no warnings |
| `cd embedded && cargo xtask build --board c5 --features bench` | **links** — 468 976 B app image, no warnings |
| `cd embedded && cargo xtask size --board c5 [--features bench]` | pass — 2.5 % / 29.8 % of the 1.5 MB `factory` partition |
| `cd embedded && cargo +1.97 clippy --manifest-path boards/starplayer-c5/Cargo.toml --target riscv32imac-unknown-none-elf --release [--features bench] -- -D warnings` | pass, both feature sets |
| `cd embedded && cargo test -p starplayer-firmware-common` | pass — 15 tests (unaffected by this task; re-run for confirmation) |
| `cd embedded && cargo test -p starplayer-embedded-xtask` | pass — 6 tests (2 new: the C5's partition size, its `linker`/`toolchain` fields) |
| `cd embedded && cargo xtask build --board a1s [--features bench]` | pass, unaffected — **464 944 B / 552 560 B, byte-identical to I3's figures** |
| `cd embedded && cargo xtask size --board a1s [--features bench]` | pass, unaffected — 17.7 % / 21.0 %, unchanged |
| `cargo xtask ci --job no-std-check` (main workspace) | pass — now runs both `riscv32imc-unknown-none-elf` and `riscv32imac-unknown-none-elf` |
| `cargo xtask ci --job no-std-purity` (main workspace) | pass — both targets |
| `cargo xtask ci --job simd` (main workspace) | pass — unaffected; stayed on `riscv32imc-unknown-none-elf` alone, as designed |
| `cargo xtask ci` (main workspace, full sweep) | pass — every job (`conformance`/`rt-safety` need `cargo xtask conformance --fetch-only` first in a fresh worktree, the same pre-existing caveat I1/I3 recorded) |
| `cargo clippy -p xtask --all-targets -- -D warnings` (main workspace) | pass |
| `cargo test -p xtask` (main workspace) | pass — no unit tests in this crate; nothing regressed |
| `cargo tree --target riscv32imac-unknown-none-elf -p starplayer-rt -e features` | confirms decision 7: no `critical-section` feature pulled into `portable-atomic` on `imac` (native CAS), unlike `imc` |
| `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/embedded.yml'))"` | pass — valid YAML |
| `cargo metadata --no-deps` (main workspace) | pass — no `embedded/` package is a member |

## Owner steps

**Flashing is owner-only; no agent has run any of it.**

1. **Flash the C5 bench build and capture the transcript.**

   ```sh
   cd embedded
   cargo xtask flash --board c5 --features bench
   cargo xtask monitor --board c5 | tee /tmp/starplayer-c5-bench.log
   ```

   Check that every `BENCH … linear flash sha256=…` line equals the committed hash in
   `goldens/<format>/<stem>__i16_mono_44100_linear.sha256` — **the same hashes the A1S's
   own bench build must match.** That equality on both boards is M8's exit criterion.

2. **Paste the numbers into `plans/reference/embedded-budget.md`.** Its §6 "The C5" has
   the line-by-line mapping (`SIZE …` → §2 type sizes; `HEAP […] …` → §2 heap table;
   `BENCH … cycles_per_frame=… core_load=…` → §3's C5 CPU tables; `STAGING …` → confirms
   the no-PSRAM finding).

3. **Optionally, the `opt-level` comparison** (research point 3): rebuild with
   `--config profile.release.opt-level='"s"'` appended to the `bench` build command, flash,
   and compare `cycles_per_frame=` against the shipped `opt-level = 3` run. Record both in
   §3 as a footnote only if they differ by more than 10 %, per the task file.
