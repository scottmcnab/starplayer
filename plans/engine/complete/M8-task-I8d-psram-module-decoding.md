# M8 — I8d: Decode web uploads into PSRAM

| Field | Value |
|---|---|
| Milestone | M8 |
| Status | Landed; hardware verified; owner listening acceptance pending |
| Recommended model | GPT-5.6-sol; high for loader/ownership work, medium for UI/tests |
| Depends on | I8c web request stack remediation |

## Context for a fresh agent

The A1S has 4 MiB PSRAM, but raw uploads currently require internal heap space of
3 * file length + 24 KiB. Wi-Fi leaves about 22 KiB free, so a 17 KiB S3M fails.
web.rs already streams uploads to a 512 KiB PSRAM staging buffer, calls ordinary
starplayer::load (allocating samples/patterns internally), calls Module::to_image
(another internal copy), then copies into one of two 512 KiB PSRAM image buffers.
I8c keeps two web futures (25,584 bytes total) in claim-only PSRAM. Global allocator
must remain internal-only: atomics/Arc reference counts cannot live in ESP32 PSRAM.

Owner approved all five native formats (MOD/S3M/MTM/XM/IT), keeping current playback
through upload and conversion, dynamic use of available PSRAM, existing SPMI layout,
and no flash partition change. Original assembly trees are read-only. Follow AGENTS.md.

## Deliverables

1. Reserve 64 KiB PSRAM for network futures and 256 KiB reusable decoder workspace.
   Divide remaining detected PSRAM into three equal four-byte-aligned buffers (raw
   staging/current image/replacement image). At 4 MiB each is 1,288,872 bytes. Allocate
   once; reuse without bump-arena leaks. No global PSRAM allocator.
2. Add reusable no_std caller-buffer image conversion for all five formats. Preserve
   existing load APIs and SPMI version/layout. Use checked preflight then incremental
   writing of canonical metadata, native patterns and decoded PCM directly to supplied
   image storage. Use PSRAM workspace for bulk scratch/state. No whole-sample/pattern/
   image DRAM allocations. Share decode/validation semantics with owned loaders. A new
   output trait must ship with both actual implementations, not a speculative interface.
3. Conversion must yield after bounded work: <=4096 input bytes scanned or <=1024 PCM
   frames emitted per step. No single unbounded format decode hidden in a step. Internal
   state/metadata allocations must be fallible; reserve 8 KiB internal heap headroom
   independently of raw/image size. Preserve atomic/cache-off safety. Prepare fully
   before queuing playback; failed uploads leave current playback intact.
4. Replace time-based image reuse with confirmed release of all queued/render module
   references. Retain per-buffer ownership tokens as needed; no writing into an image
   until its module is definitively no longer borrowed. Timeout returns busy, never
   proceeds anyway. Keep one conversion in flight; cancellation/timeout releases the
   staging/workspace safely, with no late job borrowing reused memory.
5. GET /api/upload-limits returns JSON with max_upload_bytes, max_image_bytes and
   max_stored_image_bytes (existing 252 KiB flash slot capacity). Existing upload route
   accepts raw formats and SPMI. UI fetches limits, checks raw size, distinguishes upload
   vs preparing, and explains decoded/metadata limits and inability to save large images.
6. Store immutable playback timeline row/order tables in the unused tail of each image
   buffer. Borrowed timelines clone without allocation; owned desktop timelines retain
   their existing behavior. The embedded arena boundary initializes aligned typed slices;
   the engine stays safe Rust. Capacity exhaustion remains a recoverable preparation
   error. Reclaim the tail only after every timeline clone and source referring to that
   slot has retired. This is necessary for the owner's 12-order ARMANI.S3M: two internal
   timeline copies alone could exceed the Wi-Fi heap budget even though PCM is in PSRAM.
7. Size the web engine for eight channels/eight voices instead of the boot module's
   three. Reject wider uploads explicitly. Disable unused scope tap rings and use
   single-entry engine/host scalar telemetry queues to recover internal memory; default
   desktop engine settings stay unchanged. IT preparation uses the host's actual voice
   capacity instead of allocating 256 voice states. MOD timing comparisons scan end
   summaries without row tables, then write the selected timeline once into PSRAM.
8. Document measured PSRAM/internal RAM/stack budgets, tests and hardware acceptance.

## Shared interfaces and division

Core worker owns model, loaders, facade and their tests. Firmware worker owns embedded
Rust and any narrowly required host/RT fallible adoption or retirement API. UI worker
owns embedded/www/index.html and browser/hardware test additions. Communicate exact
core conversion API early to firmware worker; agree API before integration. Proposed
shape: caller-buffer decoder construction/preflight with source/destination/workspace,
step(work budget) returning Pending or completed image length and resource errors.
No root-worktree edits by workers; each has its own sibling task worktree based on this
spec commit. Root merges branches --no-ff into integration branch and then main.

## Research points

- Measure raw decoder intermediates, image reader metadata allocation and host load
  preparation: fallibility must extend through adoption, not stop at bulk conversion.
- Share normalization, loop trimming/guards, signedness, delta/compressed decoding and
  native patterns with existing loaders. Byte-identical SPMI to load().to_image() is
  the oracle. Preserve malformed-file rejection and existing amplification budgets.
- Inspect timeout handling: jobs currently carry static staging slices; a timed-out
  HTTP request must not allow staging overwrite while control still processes it.
- Preserve default and non-web personalities. No audio RT allocations/locks/panics.

## Verification

- Each format: byte-identical image comparison against existing load().to_image() for
  corpus and synthetic fixtures; compressed IT, XM deltas, stereo, loop boundaries.
- Exact capacity +/-1, arithmetic overflow, truncation/malformed input, decoded expansion,
  workspace exhaustion, metadata allocation failure, cancellation and rapid replacement.
- Allocation instrumentation proves internal bulk usage does not scale with PCM/pattern
  size. Per-step work bounds tested, not merely documented.
- Root CI/golden and buffer-independent rendering at 1,3,64,128,4096,8191 unchanged.
- Embedded host/common tests and A1S ESP_LOG=info release web + web,lcd. Inspect stack
  frames and keep 32 KiB floor. No cargo fmt. git diff --check.
- Root flash through embedded/flash_image.sh after merged image is built. Preserve Wi-Fi
  credentials. Upload small S3M and each format, >512 KiB decoded image, repeat swaps,
  malformed/oversize rejects, HTTP/WS traffic and >=60 s UART without new underruns.
- If owner's exact 17 KiB file is unavailable, use representative fixture and explicitly
  leave exact-file listening acceptance with owner. Workers must not access hardware.

## Out of scope

Browser-side conversion, new tracker formats, general PSRAM allocator, flash partition
expansion, audio tuning, historical-source modifications. No attribution in commits.

## Verification results — 2026-09-15

- Reviewed implementation branches were merged with `--no-ff`. All root CI gates
  pass: workspace tests, conformance, real-time safety, goldens, SIMD/FMA/trace checks,
  WASM, both bare-metal targets, clippy and no-std purity. Conformance/RT were rerun
  after supplying the existing pinned corpus cache; two test-only lint fixes completed
  workspace clippy. No golden or conformance expectations changed.
- All five caller-buffer converters match the owned-loader images. Allocation-failure,
  work-bound, workspace, compression/stereo/delta, malformed-input and guard tests pass.
  Mutation parity checks covered 6,600 MOD/S3M/MTM, 1,800 XM and 2,400 IT cases.
- Embedded common: 78 tests. Host embedded: full suite plus 16 player regression tests
  after the final adoption timing fix; targeted clippy passes. Prepared sources rebase
  on their first render dispatch so preparation time cannot skip initial ticks. Failed
  queue commits preserve pending seeks; a seek after commit targets the new source.
- ESP_LOG=info release A1S `web`, `web,lcd` and default builds pass. Linked main stack
  is 55,948 B (`web`) and 54,500 B (`web,lcd`), above the 32 KiB linker floor. Largest
  station request frame is 8,496 B; control poll is 4,272 B. Application is 1,309,104 B
  of 2,621,440 B (49.94%). The flash script wrote and hash-verified the merged image
  through the Mac RFC2217 port without touching module/config partitions.
- Hardware `/api/upload-limits`: raw/image 1,288,872 B, stored image 258,048 B. Network
  futures use 25,584 B of their 64 KiB arena. Lowest free internal heap at logged
  upload checkpoints: 45,136 B (not a continuously sampled heap high-water mark).
- `ARMANI.S3M`: 17,554 B raw, 28,812 B canonical image, upload 0.71 s. A 300,304 B
  synthetic S3M expands to 600,856 B and uploaded in 3.02 s; direct SPMI took 5.46 s.
  All five formats passed. Invalid/oversized files, oversized store, partial disconnect,
  concurrent request and four rapid replacements passed the committed smoke test.
- `web_smoke.py`: 21 concurrent API requests, error routes, then 60 seconds of parallel
  HTTP/WebSocket traffic with 542 telemetry frames. 57 connection-refused retries were
  bounded by the existing two-worker retry policy. About 220 seconds of UART capture
  showed zero underruns, no panic, and DMA errors fixed at the startup baseline of 1.
- ARMANI was left playing; the serial connection was released. Only owner listening
  acceptance remains. The owner's module and local UART logs were not committed.
