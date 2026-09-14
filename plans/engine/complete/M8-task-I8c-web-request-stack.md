# M8 — I8c: Bound the web request stack

| Field | Value |
|---|---|
| Milestone | M8 |
| Status | **Landed 2026-09-15; hardware regression passed.** Owner browser acceptance pending |
| Recommended model | GPT-5.6-sol, medium effort |
| Depends on | I8b portal controller lifetime |

## Context for a fresh agent

Owner confirmed I8b: phone joins SoftAP, credentials save, reboot succeeds. Loading
starplayer.local in station mode now crashes with esp-rtos task stack overflow:
SP 0x3ffcf6f0 below stack bottom 0x3ffd1f24. Exact flashed ELF preserved at
/tmp/starplayer-i8b-crash.elf. Backtrace PCs: 400d58c1 400841c1 40082286 40084892
4003fffe 4011bdac 4013a6de 4010cf0f 401533a2 40084666. addr2line maps 4011bdac to
web.rs:890 (dispatch await), then picoserve select/server, web_task/ExternalFuture poll.
The two web futures already live in claim-only PSRAM, but executing poll still uses
internal core-0 stack. Body is a by-value heapless Vec of 1408 bytes passed through
async responses; investigate generated frames rather than assuming PSRAM relocation
eliminates request-stack temporaries. Ampkeeper web_api.rs documents stack mitigation
and boxes handler futures, but our 144 KiB internal heap cannot blindly absorb large
per-request boxes. PSRAM must remain outside the global allocator due atomic erratum.

## Deliverables

1. Identify the large request-path frames in the exact failing ELF with objdump and
   source comparison; establish the mechanism and document measurements.
2. Make a focused safe fix bounding request stack for initial page/API/WebSocket requests.
   Prefer compact borrowed/per-worker response storage or measured out-of-line boundaries
   if appropriate; preserve two workers, endpoint behavior and response capacities.
   No arbitrary heap/stack enlargement, PSRAM global allocator, unsafe shortcuts, or
   dependency churn. If broader changes are necessary, report evidence to root first.
3. Inspect the rebuilt request frames; report before/after stack and PSRAM worker costs.
   Preserve linker 32 KiB floor and existing RT and cache-off safety guarantees.
4. Document verification and any meaningful regression checks; no source-text tests.

## Research points

- Inspect call_path_router_service, dispatch, response writer and worker poll frames.
- Distinguish future storage size from transient poll/constructor execution stack.
- Inspect ampkeeper references read-only for proven response patterns.
- Keep save/reboot, station reconnect, upload and HTTP/WebSocket wire behavior compatible.

## Verification

- Source ~/export-esp-1.97.sh. Direct A1S release builds web and web,lcd with
  cargo +esp-1.97 build --target xtensa-esp32-none-elf --release --features <features>
  from board directory. Use ESP_LOG=info to match flashing script.
- Measure linked stack, relevant generated function frames and PSRAM worker size.
- Run relevant host tests if changing shared API code. git diff --check; no cargo fmt.
- Root reviews before worker commits, then merges --no-ff and flashes via main script.
- Root hardware: preserve credentials; exercise GET /, /api/status, /api/modules,
  WebSocket telemetry and repeated/concurrent requests while capturing UART for at
  least one minute; check no panic, new underruns or DMA recovery. Owner browser test
  remains acceptance if browser access is unavailable.

## Out of scope

Audio tuning, new endpoints, portal redesign, credentials changes, historical sources.

## Implementation record

The exact I8b ELF showed that moving the stored worker futures to PSRAM did not move
their generated poll frames. The picoserve select/handler poll reserved 38,912 bytes
(`0x9800`) on core 0, `FlatRoutes::call_path_router_service` reserved 27,824 bytes
(`0x6cb0`), and the shared `(StatusCode, Body)::write_to` reserved 3,168 bytes
(`0xc60`). The `ExternalFuture` task wrapper itself used only 480 bytes. The large
frames came from carrying the 1,408-byte `heapless::Vec` response body by value through
nested async state and generated branch temporaries.

Each worker now owns one 1,408-byte response buffer inside its PSRAM-resident future.
Responses borrow that buffer under a worker-local mutex guard through the socket write,
so the response value passed through picoserve contains only the guard, length and
content type. Capacity, two-worker concurrency and endpoint wire behavior are unchanged;
the fix adds no request allocation and does not register PSRAM with the global allocator.

In the rebuilt `web` ELF, the select/handler poll frame is 8,496 bytes (`0x2130`),
`FlatRoutes` is 2,992 bytes (`0xbb0`), the body writer is 144 bytes, and the task wrapper
remains 480 bytes. Each web worker future costs 12,792 PSRAM bytes (`0x31f8`), down from
34,240 (`0x85c0`): 25,584 bytes for two workers instead of 68,480. The `web,lcd` link
leaves 56,116 bytes between `_bss_end` and `_stack_start`, above the enforced 32 KiB
floor.

Both direct `ESP_LOG=info` esp-1.97 release builds passed for `web` and `web,lcd`.
`git diff --check` passed. Hardware acceptance uses the standard-library-only regression
harness:

```text
python3 embedded/tests/web_smoke.py <board-ip>
```

It checks the gzip page, status and module JSON, twenty requests across both workers,
404/malformed/empty-upload response paths, then holds a telemetry WebSocket open for
sixty seconds while sending HTTP requests through the second worker.

## Reviewer hardware result

The root reviewer independently confirmed the smaller frames in disassembly, merged
with --no-ff, and rebuilt/flashed main through embedded/flash_image.sh. The image write
was 1 330 768 bytes and hash verified; app size was 1 265 232 / 2 621 440 bytes (48.26%).
Stored Wi-Fi credentials survived and station connection resumed. Boot measured 57 548
stack bytes and 25 584 PSRAM bytes for the two web workers.

Before the fix, GET / returned its 6 625-byte gzip body but the following API request
reproduced the stack overflow (SP 0x3ffcf690). After the fix, the committed smoke test
passed HTML decompression, status/modules JSON, twenty concurrent requests, 404,
malformed JSON and empty-upload responses, and 60 seconds of WebSocket telemetry plus
parallel HTTP: 547 binary telemetry frames. It recorded 38 connection-refused retries.
The harness retries only connection refusal for up to two seconds per operation;
timeouts, invalid responses and broken WebSockets still fail. This is not a claim of
refusal-free TCP acceptance on the two-worker server.

UART covered the test with no panic, fatal error, audio underrun or DMA recovery;
dma_errors=1 remained constant. Logs: /tmp/starplayer-web-request-reproduced.log (before),
/tmp/starplayer-web-request-acceptance.log (after). Owner browser acceptance remains.
