/* A build-time floor under the main stack (M8-I6).
 *
 * On the classic ESP32 the main stack is whatever DRAM is left between the end of `.bss`
 * and 0x3ffe_0000, so *every* static added anywhere in the firmware silently shortens it,
 * and the failure mode is not a link error but a boot that overwrites `.bss` — which in a
 * `web` build is the WiFi driver's own statics. A firmware that runs out of stack at run
 * time is far harder to diagnose than one that will not link, so this turns the squeeze
 * into a build failure.
 *
 * 24 KiB, against the figures M8-I6's research resolution records for each build. If this
 * fires, something added a large static — a task pool, a StaticCell, a heap region in
 * `.bss` — and it must be shrunk, or moved into `dram2_seg` the way main.rs's `web` heap
 * is. See embedded/README.md, "The DRAM budget".
 */
ASSERT(_stack_start - _stack_end >= 24K, "the main stack is under 24 KiB: see embedded/boards/starplayer-a1s/ld/stack-floor.x")
