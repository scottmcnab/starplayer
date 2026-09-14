# M8 — I8b: Keep the portal radio controller alive

| Field | Value |
|---|---|
| Milestone | M8 |
| Status | **Implemented 2026-09-15.** Hardware acceptance remains with the owner |
| Recommended model | GPT-5.6-sol, low effort |
| Depends on | I8a web internal-heap remediation |

## Context for a fresh agent

The A1S web build prints `PORTAL open network "StarPlayer-D3E4"` but the phone cannot
see it. In `embedded/boards/starplayer-a1s/src/provisioning.rs`, `run()` creates a local
WifiController, starts the AP and service tasks, then returns Ok. esp-radio 0.18's
WifiController destructor deinitializes Wi-Fi. The spawned runner owns only an interface.
Ampkeeper's `../ampkeeper/esp32/firmware/src/provisioning.rs` deliberately retains the
controller for its entire portal session. Prior UART-only acceptance missed this lifetime bug.

## Deliverables

- Move the controller into the existing access_point_net_task along with its runner.
  Keep ownership for the entire runner future lifetime with a named binding; no leaks,
  unsafe code, new heap allocations, or additional task are necessary.
- Keep run() returning successfully after startup so main.rs can spawn reboot_task.
  Correct its inaccurate never-returns documentation and explain task ownership.
- Preserve SSID/configuration, pre-AP scan, DHCP/DNS, audio and reset behavior.
- Record verification in this task. Root reviewer handles merge, flashing and archive.

## Research points

- Verify pinned WifiController Drop tears down the radio and the task retains ownership
  across its await; do not use an unbound wildcard argument that drops it immediately.
- Confirm the reboot watcher is still spawned after provisioning startup returns.

## Verification

- Source ~/export-esp-1.97.sh; build A1S release web and web,lcd from the worker checkout.
  Avoid stale xtask paths by invoking cargo +esp-1.97 build directly in the board directory
  with --target xtensa-esp32-none-elf --release --features web (then web,lcd).
- Inspect linked stack symbols for each build and retain the 32 KiB stack floor.
- git diff --check. No cargo fmt or artificial unit test of source text.
- Reviewer: flash merged main via embedded/flash_image.sh; capture at least 60 seconds
  of runtime, checking for panic, fatal errors, new underruns or DMA recovery.
- Owner acceptance: SSID visible for at least a minute; phone joins and receives an IP;
  portal loads; save credentials resets the board and station mode connects.

## Implementation result

The existing SoftAP network task now owns the named `WifiController` binding alongside
the `embassy-net` runner. The pinned esp-radio 0.18 implementation confirms that dropping
the controller calls `wifi_deinit()` and marks both radio interfaces uninitialised; retaining
it in the never-returning runner future therefore keeps the radio alive for the whole portal
session without another task or allocation. `run()` still returns after spawning the portal
tasks, and `main.rs` then spawns the reboot watcher as before.

Both required direct A1S release builds pass. The linked `web` image has `_bss_end` at
`0x3ffd1f34` and `_stack_start` at `0x3ffe0000`, leaving 57 548 bytes of core-0 stack. The
`web,lcd` image has `_bss_end` at `0x3ffd24cc` and the same `_stack_start`, leaving 56 116
bytes. Both retain the enforced 32 KiB floor. `git diff --check` passes. Hardware verification
remains with the reviewer and owner as specified above.

## Out of scope

Dependencies, radio tuning, allocation changes, other web features, and historical sources.

## Reviewer hardware result

The branch was reviewed and merged with --no-ff. The merged main web image was rebuilt
by embedded/flash_image.sh and flashed through the remote RFC2217 bridge; esptool verified
1 338 224 bytes at address zero. The app occupies 1 272 688 of 2 621 440 bytes. This scripted
image reports 57 564 linked stack bytes; both the direct builds and scripted image exceed the
32 KiB floor. A 65-second UART capture reached 1:03 playback, announced
StarPlayer-D3E4, and contained no panic, fatal error, underrun or DMA recovery. The existing
dma_errors=1 counter stayed constant. Capture: /tmp/starplayer-portal-lifetime.log.

Only owner acceptance remains: confirm SSID visibility for a minute, join and obtain an
address, load http://192.168.4.1/, and save credentials to verify reboot/station connection.
The UART announcement is not evidence of over-the-air visibility or successful provisioning.
