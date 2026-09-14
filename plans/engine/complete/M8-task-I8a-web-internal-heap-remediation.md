# M8 — I8a: A1S web internal-heap remediation

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](../M8-master-plan.md)) |
| Status | **Complete 2026-09-14.** The 144 KiB internal heap starts esp-radio and the unprovisioned captive portal with measured headroom |
| Depends on | [I8](M8-task-I8-web-boot-stack-remediation.md) |
| Recommended model | GPT-5.6-sol |
| Verified by | Agent build/budget checks and reviewer diff, then RFC2217 flash plus unprovisioned portal/audio hardware acceptance |

## Context for a fresh agent

I8 successfully moved the picoserve task futures from `.bss` into the claim-only PSRAM arena. The
exact `web` image now boots with 106,716 bytes of linked core-0 stack instead of 28,108 and no
stack-guard panic. Hardware then exposed the next latent I6 budget error. Immediately before
provisioning, the 96 KiB `dram2_seg` internal heap reports 81,076 bytes used and only 17,228 free.
`esp_radio::wifi::new` fails at `esp_wifi_init_internal` with error 257, `ESP_ERR_NO_MEM`, before
the first scan. Audio continues cleanly with zero underruns, confirming that the radio's dynamic
internal allocation is the failed resource.

The I8 relocation recovered about 78 KiB of ordinary `dram_seg`; it is currently all stack. The
web image may safely give 48 KiB of that recovered region to a second **Internal-capability** heap
region while retaining about 57 KiB of linked core-0 stack. The existing 96 KiB region in
`dram2_seg` remains the first heap region. StarPlayer PSRAM must remain unregistered with the
global allocator because engine `Arc` reference counts and seqlocks contain atomics that are not
valid in classic-ESP32 PSRAM.

`esp_alloc::heap_allocator!` is a block expression with its own scoped static, so it can register
the existing `dram2_seg` region and a second `.bss` region. Both regions carry the Internal
capability. The total web heap should be reported as 144 KiB, with comments keeping the region
split and its stack cost explicit.

## Deliverables

1. For `web` builds, retain the 96 KiB `dram2_seg` internal heap and add a 48 KiB internal heap
   region in ordinary `.bss`. Register `dram2_seg` first, then the `.bss` reserve. Do not register
   PSRAM, alter the claim-only PSRAM arena, or change the 120 KiB non-web heap.
2. Make the boot heap log report the true 144 KiB total for `web`. Update `main.rs` comments with
   the two-region arithmetic, allocation purpose, registration order, and measured post-I8 stack
   tradeoff. Keep each size named so future budget changes do not confuse total capacity with one
   linker section.
3. Retain I8's 32 KiB linker floor. Measure all four release personalities. Expected approximate
   core-0 stacks after the new `.bss` region: default 35,608 B, `lcd` 34,200 B, `web` 57,564 B,
   `web,lcd` 56,132 B. Record actual results in `embedded/README.md`,
   `plans/reference/embedded-budget.md`, and this task.
4. Hardware acceptance must show: a 144 KiB heap at boot; I8's exact stack log above 32 KiB;
   Wi-Fi initialization succeeds; the scan completes; `PORTAL open network "StarPlayer-D3E4"`
   appears; and transport/audio logs continue for at least 30 seconds without a panic, fatal
   message, new underrun, or DMA recovery.

## Research points

- Confirm error 257 maps to `ESP_ERR_NO_MEM` / `WifiError::OutOfMemory` in the pinned esp-radio
  release.
- Confirm both macro-created regions have `MemoryCapability::Internal`, so esp-radio's
  internal-only malloc can use both while no global allocation can enter PSRAM.
- Report heap usage immediately after successful radio initialization or portal startup if an
  existing log can do so cheaply; the hardware record should show remaining headroom rather than
  merely that one boot succeeded.
- Check the 48 KiB addition against `web,lcd`, the tightest web link, and keep at least the 32 KiB
  enforced floor.

## Verification

```sh
cd embedded && cargo test -p starplayer-firmware-common -p starplayer-embedded-xtask
cd embedded && cargo xtask build --board a1s
cd embedded && cargo xtask build --board a1s --features lcd
cd embedded && cargo xtask build --board a1s --features web
cd embedded && cargo xtask build --board a1s --features web,lcd
cd embedded && cargo xtask size --board a1s --features web,lcd
git diff --check

# reviewer/hardware from the merged target checkout
./embedded/flash_image.sh
# reset/capture UART for >=30 s
```

Use `~/export-esp-1.97.sh` for firmware builds. Do not run `cargo fmt`. The implementation worker
must not access hardware; the root reviewer performs the authorized flash and capture.

## Implementation result

The `web` allocator now registers its existing 98 304-byte `dram2_seg` region first and a
separately named 49 152-byte `.bss` region second. `esp_alloc::heap_allocator!` assigns both the
Internal capability, for a true total of 147 456 bytes reported as 144 KiB at boot. The non-web
heap remains 122 880 bytes and PSRAM remains an unregistered claim-only arena. Both provisioning
and station paths print `HEAP.stats()` immediately after `esp_radio::wifi::new` succeeds so the
hardware run will capture the radio's remaining headroom. The pinned ESP32 bindings define error
257 as `ESP_ERR_NO_MEM`, and esp-radio 0.18 maps that value to `WifiError::OutOfMemory`.

The exact release links leave 35 608 bytes of core-0 stack in default, 34 200 in `lcd`, 57 564 in
`web`, and 56 132 in `web,lcd`, all above the unchanged 32 KiB floor. The corresponding web
`.bss` totals are 120 276 / 121 276 bytes, including the exact 49 152-byte second heap reserve;
both retain 98 304 bytes in `.dram2_uninit`. Firmware-common's 73 tests and embedded-xtask's 10
tests pass, as do all four required A1S release links. The `web,lcd` image is 1 300 944 of
2 621 440 bytes (49.6%), and `git diff --check` passes.

Hardware acceptance flashed the exact merged `main` image and verified its hash. Boot reported
147 456 heap bytes and 57 564 linked stack bytes. After audio construction the heap had 66 380
bytes free; after successful Wi-Fi initialization it had 22 148 bytes free. The scan found one
network, the portal claimed 10 048 PSRAM bytes and announced `StarPlayer-D3E4` at
`http://192.168.4.1/`. A 43-second UART capture contained no panic or fatal message; playback
advanced continuously with zero underruns and no DMA recovery log.

## Out of scope

- Moving engine allocations, task headers, atomics, audio buffers, or the general allocator into
  PSRAM.
- Reducing radio queue depths, module/image buffer sizes, or audio engine capacity to mask the
  missing heap.
- Changing portal/API behavior, partitions, sample rate, DMA refill logic, codec settings or the
  boot module.
