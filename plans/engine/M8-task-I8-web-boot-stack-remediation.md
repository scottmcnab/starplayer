# M8 — I8: A1S web boot stack remediation

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md)) |
| Status | **Implementation complete; hardware acceptance pending.** Hardware acceptance found a core-0 stack-guard panic on the first unprovisioned `web` boot |
| Depends on | [I6](complete/M8-task-I6-web-control.md), [I3a](complete/M8-task-I3a-dma-refill-remediation.md), [I7](complete/M8-task-I7-rfc2217-flash-script.md) |
| Recommended model | GPT-5.6-sol |
| Verified by | Agent host/build checks, reviewer diff and build checks, then hardware boot and captive-portal acceptance over RFC2217 |

## Context for a fresh agent

The classic ESP32 A1S `web` image builds and flashes, starts the 48 kHz audio path, and plays the
compiled-in `REFLEX.S3M`, but an unprovisioned board then trips esp-hal's ProCpu stack guard before
the first captive-portal log line. The captured backtrace enters the Embassy main future at
`boards/starplayer-a1s/src/main.rs`'s `play(...).await`; the last application log is
`PLAY master_volume=1/4 ...`. The config partition reports `no network stored`, so the next web
path is `provisioning::run(...).await`.

The exact linked image reports `_stack_end = 0x3ffd9234` and `_stack_start = 0x3ffe0000`, only
28,108 bytes (27.45 KiB). `ld/stack-floor.x` currently permits 24 KiB. The `web` build also links
mutually exclusive static Embassy task pools into `.bss`: the normal station web-worker pool is
68,560 bytes and the captive-portal worker pool is 10,088 bytes. Every `.bss` byte removes one
byte from the core-0 main stack even when that boot personality never spawns the pool.

The sibling `../ampkeeper` firmware solved the same classic-ESP32 failure by keeping Embassy task
headers and atomics in internal DRAM while constructing large picoserve future bodies directly in
PSRAM (`esp32/firmware/src/psram_task.rs`). StarPlayer deliberately does not register PSRAM with
the global allocator: ordinary engine allocations contain atomics, and classic ESP32 atomics do
not work correctly in PSRAM. StarPlayer instead owns PSRAM through `src/psram.rs`'s monotonic
`Arena`; its three module buffers leave about 2.5 MiB unused on the installed board. Any solution
must preserve that allocator boundary and must not let ordinary `Box`/`Arc` allocations spill to
PSRAM.

The immediate failure is provisioning startup, but the normal web server has not yet had hardware
acceptance either. Fix both large picoserve task pools so one successful portal boot does not
leave the same latent stack squeeze in station mode.

## Deliverables

1. Move the captive-portal worker future and both normal web-worker futures out of static internal
   DRAM task pools and into PSRAM claimed from `psram::Arena`. Embassy task headers, scheduler
   state, atomics and shared synchronization objects must remain in internal DRAM. Construct each
   large future directly in its final PSRAM location so it is never materialised on the already
   constrained core-0 stack.
2. Keep PSRAM unregistered from `esp_alloc`'s global allocator. Extend the claim-only arena with
   the minimum aligned raw-storage operation needed by a small `psram_task` helper. Audit and
   document each new `unsafe` operation: allocation bounds/alignment, initialization, pinning,
   lifetime, exclusive polling, and why the selected future bodies contain no cross-core atomics.
   Do not create a general-purpose external allocator.
3. Thread the remaining arena through `web::Boot` and the selected network personality. Claim the
   existing upload/image buffers as before, then claim task storage only for the personality that
   actually starts. A missing or undersized PSRAM allocation must produce a clear boot error,
   never undefined behavior or silent task loss.
4. Remove the `#[embassy_executor::task]` static pools for the migrated workers while retaining
   their behavior, worker counts, TCP/HTTP buffer sizes, server loops and error reporting. Other
   radio, DHCP, DNS, mDNS, key, control and audio tasks remain ordinary internal task pools.
5. Raise the A1S main-stack linker floor from 24 KiB to 32 KiB, update its comments, and log the
   linked core-0 stack size once at boot. The `web`, `web,lcd`, default and `lcd` images must all
   link over the new floor.
6. Update `embedded/README.md`'s DRAM budget and `plans/reference/embedded-budget.md` with the new
   measured stack sizes and explain the PSRAM-backed picoserve task layout. Record hardware proof:
   the unprovisioned web image reaches `PORTAL open network`, remains alive for at least 30 seconds,
   and continues clean audio without DMA recovery/underrun logs.

## Research points

1. Confirm each migrated future's linked static-pool size before removal and the resulting
   `_stack_start - _stack_end` after removal for `web` and `web,lcd`.
2. Confirm the task future can be initialized in place without a large temporary on core 0. Use
   the known-working ampkeeper helper as the reference, adapting it to StarPlayer's unregistered
   monotonic arena rather than registering an external heap region.
3. Audit picoserve/embassy-net values held directly by the migrated future. Interior `Cell`/waker
   state private to a core-0 future may reside in PSRAM; task headers, shared channels/signals,
   atomics and anything polled across cores may not.
4. Measure the extra PSRAM claimed by one portal worker and two station workers and ensure both
   personalities still leave the three 512 KiB module/upload buffers intact.
5. Check whether 32 KiB is sufficient for both Wi-Fi initialization/scan and the deepest web
   request path. If moving both picoserve pools creates materially more stack headroom, report the
   real linked value rather than relying only on the floor.

## Verification

```sh
cd embedded && cargo test -p starplayer-firmware-common
cd embedded && cargo test -p starplayer-embedded-xtask
cd embedded && cargo xtask build --board a1s
cd embedded && cargo xtask build --board a1s --features lcd
cd embedded && cargo xtask build --board a1s --features web
cd embedded && cargo xtask build --board a1s --features web,lcd
cd embedded && cargo xtask size --board a1s --features web,lcd
git diff --check

# reviewer/hardware, from the target checkout after a clean exact rebuild
./embedded/flash_image.sh
# reset and capture UART over rfc2217://192.168.0.151:8086?ign_set_control
#   -> linked main-stack size is logged
#   -> STORE reports no network stored
#   -> PORTAL scan and `PORTAL open network "StarPlayer-D3E4"` appear
#   -> no stack-guard panic, DMA recovery, underrun, or audio corruption for >=30 s
```

Use `~/export-esp-1.97.sh` for A1S firmware commands. Do not run `cargo fmt`. The root reviewer will
perform the authorized hardware flash and UART capture; the implementation worker stops after the
non-hardware verification.

## Implementation result

The fixed-web build now keeps each Embassy task header and its pointer-sized future proxy in the
internal `dram2_seg` heap while `psram_task.rs` constructs and pins the large picoserve future
directly in an aligned monotonic-arena claim. PSRAM remains absent from the global allocator. The
worker futures own no shared/cross-core atomics: their stack/configuration values point to internal
statics, shared channels/signals/mutexes remain internal, and private picoserve `Cell`/waker state
is polled only by core 0. A worker yields to the core-0 control task before a synchronous flash
write, so the executor cannot poll the PSRAM body while the shared flash/PSRAM cache is disabled.

`web::Boot` carries the arena remainder after the existing three 512 KiB upload/image buffers.
Only the selected personality consumes it. Compiler type-size output reports 34 240 bytes per
station worker (68 480 bytes for two) and 10 048 bytes for the portal worker. Each internal
`TaskStorage<ExternalFuture<_>>` is 48 bytes. An arena allocation or fresh-header failure returns a
specific boot error. The old linked static pools measured 68 560 bytes for station and 10 088
bytes for the portal; neither pool symbol remains after the change.

The new exact linked core-0 stack sizes are 35 608 bytes for default, 34 200 for `lcd`, 106 716
for `web`, and 105 284 for `web,lcd`. The previous failing `web` image had 28 108 bytes. The linker
floor is now 32 KiB and every boot logs its exact linked stack size. All 73 firmware-common tests
and all 10 embedded-xtask tests pass, as do the default, `lcd`, `web` and `web,lcd` A1S release
links. The `web,lcd` app image is 1 299 392 of 2 621 440 bytes (49.5%), and `git diff --check`
passes. Hardware proof remains pending: this implementation did not flash or execute the image on
the board.

## Out of scope

- Changing audio sample rate, DMA refill logic, codec volume or the compiled-in boot module.
- Registering PSRAM with the global allocator or moving engine `Arc`/seqlock/telemetry allocations
  into external RAM.
- Redesigning the portal, web API, network task counts, module-buffer sizes or flash partitions.
- Adding authentication, OTA, HTTPS, or desktop/web-player behavior.
