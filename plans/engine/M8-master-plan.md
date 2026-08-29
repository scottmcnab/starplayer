# M8 — Embedded proof

| Field | Value |
|---|---|
| Goal | An esp32 RISC-V target plays a module from flash via I2S under embassy |
| Estimate | 1u |
| Depends on | M2 (fixed-point mixer) |
| Trigger | **Pull-driven.** Start when the owner wants StarPlayer in an embedded project, or before any API is stabilised for public release |

## Why it matters even though CI already checks `no_std`

The `riscv32imc-unknown-none-elf` CI check has proven since M0 that the core crates
**compile** without `std`. This milestone proves they **play** — which is a different
claim, and the one that finds the problems: RAM budget, flash-resident sample data,
fixed-point audio quality, and whether the async story actually holds.

Doing it before the API stabilises means the portability claims are validated rather than
merely asserted. Doing it after would mean discovering the constraints once they are
expensive to satisfy.

The dev machine already has `riscv32imac`, `riscv32imafc` and `riscv32imc` targets
installed, and the owner works in esp-rs and embassy day to day — so the toolchain
question is already answered.

## Deliverables

1. **An esp32 example** (C3 or C6 class, RISC-V) rendering a module through I2S under
   embassy.
2. **Borrowed sample data** — the module plays straight out of memory-mapped flash rather
   than being copied to RAM. This is what the offsets-not-references decision in
   architecture §6 was for, and this milestone is where it pays off.
3. **The fixed-point mixer path** as the only mixer, with a RAM and CPU budget measured
   and recorded.
4. **`portable-atomic` + `critical-section`** wiring verified on a target that needs it.
5. **An answer to architecture open question Q2**: is 128 the right `RENDER_QUANTUM` for
   embedded, or does this path want a compile-time override? Record it in the
   architecture document.
6. **A written budget** — RAM, flash and CPU per voice at a given rate — in
   `plans/reference/embedded-budget.md`, so a future project can size a target before
   trying.

## Exit criteria

An esp32 devkit plays a real module cleanly through I2S, with the measured budget written
down.

## Out of scope

Xtensa targets, unless the owner's hardware needs them. Any UI on the device.
