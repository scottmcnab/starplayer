# M8 — I6: Web control — captive-portal provisioning, an HTTP/WebSocket API and module upload

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](../M8-master-plan.md), decision 5; deliverable 8) |
| Status | Implemented 2026-09-11; builds link; awaiting owner hardware run |
| Depends on | I3 (board crate, `EmbeddedPlayer` on the device, PSRAM heap region, the `modules` and `config` partitions) |
| Blocks | — |
| Parallel with | I4, I5 |
| Recommended model | Claude Opus (a WiFi stack and two boot personalities beside a real-time audio task, with flash writes that stall the cache) |
| Verified by | agent (host tests for the API encoders and the upload state machine; a `web` build; a scripted `curl`/`websocat` session against the device is the owner's step), then owner in a browser |

## Context for a fresh agent

The device plays whatever image was compiled in. This task lets a phone or laptop on the
same network control it and give it new modules: WiFi station mode with credentials from
a **captive portal** (master-plan decision 5 — not build-time environment variables), an
mDNS name `starplayer.local`, a small page served from flash, a JSON API, a WebSocket
that pushes the telemetry snapshot, and an upload endpoint that loads a module into the
PSRAM heap and swaps it in.

Every piece has a proven shape in `../ampkeeper/esp32/firmware`:

- **Provisioning as a boot personality**, `provisioning.rs` (1 120 lines): on an empty
  stored SSID or a button hold, `main` runs the portal instead of the normal task set —
  a station scan *before* flipping to an open SoftAP (`<Name>-XXXX` from the MAC),
  a hand-rolled DHCP server on 192.168.4.1/24 and a DNS catch-all so the phone's
  captive-portal detection pops the page, a picoserve portal with `GET /` and
  `POST /save`, credentials persisted with `sequential-storage`, then a soft reset.
  Copy the shape, not the code: StarPlayer's portal has no auth, no presence gate, no
  recovery personality.
- **picoserve 0.18** (`alloc, embassy, json, ws`) with a flat path router and
  pre-serialised responses (`web.rs`, `web_api.rs`); the WebSocket callback pattern in
  `web.rs:368`; worker tasks placed in PSRAM (`psram_task.rs`).
- **Static assets** compiled in gzipped with `include_bytes!` and served with
  `Content-Encoding: gzip` (`web_assets.rs`).
- esp-radio 0.18 `["esp32", "wifi"]`, embassy-net 0.9 (`tcp, udp, dhcpv4, dns,
  proto-ipv4, medium-ip`), edge-mdns 0.8, edge-nal-embassy 0.9.

The engine side is already there: `starplayer_host_wasm`'s `WireCommand { opcode: u8,
argument: u32, extra: u32 }` (`crates/starplayer-host-wasm/src/command.rs`) and its flat
telemetry word layout (`lib.rs`, `TELEMETRY_HEADER_WORDS = 22` plus per-channel words)
are a compact wire vocabulary for the WebSocket; `apps/starplayer-web/www/index.html` is
the visual reference for a page a tenth its size.

**The one real hazard**: on the classic ESP32, writing flash disables the instruction and
data caches for the duration of each erase/write, so anything reading flash — the mixer
reading a borrowed module image, the audio task's own code if it is not in IRAM — stalls.
An upload into the `modules` partition therefore **pauses playback** (or plays a module
that lives in PSRAM) for the write, and the audio task's DMA refill must be in IRAM or
accept the gap. State this in the module doc and measure it.

### Code you must read before changing anything

- `embedded/boards/starplayer-a1s/src/{main,board,audio}.rs` (I3) — the heap regions
  (the PSRAM region is where uploaded modules go), task layout, core pinning, the
  `partitions.csv` `modules` and `config` entries.
- `crates/starplayer-host-embedded/src/lib.rs` (I1) — `ControlHalf::load(Arc<Module>)`,
  `collect_garbage`, `telemetry()`; the retired-module ring (an upload retires the
  previous module through it — drop it on the control task, never in the refill).
- `crates/starplayer/src/lib.rs` — `load`, `probe`, `ModuleFormat`.
- `crates/starplayer-model/src/image.rs` (I2) — `from_image` / `from_image_or_copy` for
  images stored in the `modules` partition.
- `crates/starplayer-host-wasm/src/{command,lib}.rs` — `WireCommand`, the opcode table,
  the telemetry word layout.
- `../ampkeeper/esp32/firmware/src/{provisioning,web,web_assets,psram_task,store}.rs`
  and `esp32_main.rs` (`net_task`, `connection`, the SoftAP/STA boot decision).
- `../ampkeeper/esp32/firmware/Cargo.toml` — the exact esp-radio / embassy-net /
  picoserve / edge-* versions and feature lists that resolve together.
- `apps/starplayer-web/www/{index.html,style.css}` — visual reference only.

## Deliverables

### 1. Networking and provisioning (`boards/starplayer-a1s/src/{net,provisioning,store}.rs`, feature `web`)

- `store.rs`: SSID/PSK in the `config` partition via `sequential-storage`; `load_wifi`,
  `save_wifi`, `erase`.
- Boot decision in `main.rs`: no stored SSID, or KEY2 held ≥ 5 s at boot (I5's `Keys`),
  or a "re-provision" RTC word → `provisioning::run` (the portal personality: SoftAP
  `StarPlayer-XXXX`, DHCP, DNS catch-all, `GET /` portal page with a scanned-network
  list, `POST /save`, soft reset); otherwise station mode with DHCP, then mDNS
  `starplayer.local`, then the web server. Audio starts in both personalities (the portal
  is not silent — it plays the embedded module so the owner knows the board is alive).
- WiFi and the web workers run on core 0; the audio task stays on core 1 (I3). If I3's
  research point 6 found single-core only, the audio task outranks everything and the
  budget records the underrun behaviour under WiFi load.

### 2. The API (`boards/starplayer-a1s/src/web.rs`)

picoserve, port 80, two workers with 4 KB buffers placed in PSRAM:

| Method, path | Body / result |
|---|---|
| `GET /` | the page (gzipped, `include_bytes!`) |
| `GET /api/status` | JSON: title, format, order/pattern/row, speed, bpm, playing, volume, channels (from `Snapshot`) |
| `POST /api/play`, `/api/stop`, `/api/next`, `/api/previous` | 204 |
| `POST /api/seek` `{order}` / `/api/volume` `{level}` / `/api/mute` `{channel, muted}` | 204 |
| `GET /api/modules` | JSON list: the compiled-in image plus every image in the `modules` partition, with sizes |
| `POST /api/modules/select` `{id}` | 204; loads from the partition (borrowed, `from_image`) and swaps |
| `POST /api/modules` | raw module bytes, `Content-Length` ≤ free PSRAM; `probe` → `load` into PSRAM → `ControlHalf::load`; 201 with the new status; the previous module retired via `collect_garbage` on the control task |
| `POST /api/modules/store` `{id}` | writes the *current* PSRAM module as an image into the `modules` partition, **pausing playback for the write** (the cache hazard) |
| `GET /ws` | WebSocket pushing the flat telemetry words at 10 Hz; accepts `WireCommand` frames (9 bytes) |

Responses are serialised with `serde-json-core` into fixed buffers, as ampkeeper does,
to keep worker stacks small. The upload handler streams the body into a PSRAM `Vec<u8>`
reserved from `Content-Length`; a second upload while one is in flight returns 409.

### 3. The page (`embedded/www/index.html` → gzipped by `cargo xtask assets`)

One file, no framework, ≤ 12 KB gzipped: title and transport row, a range for volume,
order/pattern/row/speed/BPM, a channel table with VU bars and effect names driven by the
WebSocket, a module list with select, a file input for upload with progress, and a
"store to flash" button. Styled after `apps/starplayer-web/www/style.css` (copy the
palette, not the file).

### 4. Proof

- Host tests in `firmware-common`: the status JSON encoder against a synthetic
  `Snapshot`; the telemetry word packer equals the wasm host's layout for the same
  snapshot (import the constants); the upload state machine (reserve, append, complete,
  abort, 409).
- The `web` build compiles with and without `lcd`.
- Owner session: provision from a phone, `curl http://starplayer.local/api/status`,
  upload `REFLEX.S3M`, hear it, store it, reboot, select it.

### 5. Documentation

`embedded/README.md`: provisioning steps, the API table, the flash-write pause.
`plans/reference/embedded-budget.md`: heap high-water with the web stack up, and the
audio underrun count during an upload and during a partition write.

## Research points

1. **esp-radio 0.18 on the classic ESP32**: the `esp32` feature exists; confirm the
   init sequence (`esp_radio::init`, `esp_rtos` with `esp-radio` feature) is the same as
   on the S3, and how much DRAM the WiFi driver reserves (it is the single biggest RAM
   consumer; the budget needs the number).
2. **The flash-cache stall**: measure the audio gap during a 64 KB partition write with
   the module in PSRAM vs borrowed from flash; decide whether the DMA refill needs
   `#[ram]` placement (esp-hal's attribute) and whether "pause playback" is a fade-out
   (`set_at_end` with a short fade) or a hard stop.
3. **DHCP/DNS**: ampkeeper hand-rolled both after finding `edge-dhcp`/`edge-captive`
   awkward under embassy-net 0.9 (its research point 2). Re-check the current `edge-*`
   releases before copying the hand-rolled versions.
4. **mDNS on the classic ESP32** with edge-mdns 0.8 and embassy-net multicast: ampkeeper
   has it working on the S3; confirm the multicast join on this chip's WiFi driver.
5. **WebSocket frame budget**: the telemetry words for 32 channels are ~1 KB at 10 Hz per
   client; cap clients at two and drop frames rather than block the worker.

## Verification

```text
cd embedded && cargo test -p starplayer-firmware-common
cd embedded && cargo xtask build --board a1s --features web
cd embedded && cargo xtask build --board a1s --features web,lcd
cd embedded && cargo xtask size --board a1s --features web,lcd     # under the factory partition
# owner
cd embedded && cargo xtask flash --board a1s --features web,lcd && cargo xtask monitor
#   → portal on first boot; then http://starplayer.local/ plays, stops, seeks, uploads;
#     underrun count during upload recorded in the budget doc
```

## Out of scope

HTTPS, authentication, OTA, cloud anything. Playlists. Streaming audio *to* the browser.
Serving the wasm web player from the device (it is 100 KB+ of JS and needs a secure
context for the worklet's `SharedArrayBuffer` path).

## Research resolution

Implemented 2026-09-11 on branch `m8-i6`. Every build below links with no warnings;
nothing has been flashed — flashing is owner-only.

### 1. esp-radio 0.18 on the classic ESP32

**The `esp32` feature exists and the init sequence is not what the task file assumed.**
There is no `esp_radio::init` in 0.18 at all — `src/lib.rs` has only a private `init()`,
and the public entry point is `esp_radio::wifi::new(peripherals.WIFI, ControllerConfig)`,
which returns the controller and its two interfaces and **starts the radio implicitly**
with whatever initial configuration it was given. What the task file describes (a token
from `init`, an explicit `start_async`) is the older `esp-wifi` shape. The requirements
that remain are two, and both are in place:

* `esp_rtos::start(timer, software_interrupt0)` must have run first. This firmware already
  called it for the embassy time driver, before M8-I6 existed.
* `esp-rtos` needs its **`esp-radio` feature**, which pulls in `esp-radio-rtos-driver` and
  `alloc`. It is enabled *by the `web` feature* (`esp-rtos/esp-radio` in the feature list)
  rather than unconditionally, so a default build's `esp-rtos` is unchanged.

`esp-radio` also needs its own **`unstable`** feature on this release: `is_connected`,
`ScanConfig`'s builder and `ControllerConfig`'s queue depths are all gated by it, the same
way `esp-hal`'s `unstable` gates I2S and PSRAM.

**How much DRAM the WiFi driver reserves — 11 658 bytes, statically.** This is the figure
the budget asked for and it is much smaller than expected. Summing every `.bss` symbol in
the linked `web,lcd` image that does not belong to the firmware crate (`xtensa-esp32-elf-nm
-S`) gives 11 658 B: `g_cnxMgr` 3 880, `s_wifi_nvs` 1 308, `gWpaSm` 816, `g_ic` 700,
`gChmCxt` 592, `s_dp` 548, `g_pm` 544, `s_ni` 344, `gScanStruct` 284 and a long tail. The
driver's real appetite is the **heap** — its receive and transmit buffers are allocated at
initialisation and per frame — which is why the `web` build's heap had to move somewhere it
could be large. `ControllerConfig::with_rx_queue_size(20).with_tx_queue_size(16)` is
ampkeeper's tuning, kept: the defaults (5 and 3) are exceeded by an ordinary broadcast
burst and by smoltcp's retransmits.

**The finding that actually shaped this task is not in the task file**: the classic ESP32
has 192 KiB of `dram_seg`, and core 0's main stack is whatever `.bss` leaves of it. With
the heap in `.bss` the `web` build **does not link at all** — `.bss` alone reaches past
`0x3ffe_0000` and the stack has nowhere to begin. The fix is ampkeeper's: the `web` build's
entire heap (96 KiB) goes in **`dram2_seg`**, the 98 768 bytes above the ROM's own data and
stacks that esp-hal leaves as an uninitialised section with no other user, reached with
`esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: …)`. It is
ordinary internal DRAM — atomics work there — and it costs the stack nothing.

### 2. The flash-cache stall

**Not measured, and the task file's "measure it" is the owner's step** — but the design
question it was meant to settle was settled by reading esp-storage rather than by timing
anything, and the answer is stronger than a measurement would have been.

`esp_storage::FlashStorage` **refuses a write outright** while the other core is running
(`FlashStorageError::OtherCoreRunning`), because that core executes from flash and the ROM
write routine turns the cache off. Since M8-I5 the audio refill *is* the other core. So:

* `Store::take` builds its `FlashStorage` with **`multicore_auto_park()`**, which parks
  core 1 around each erase and each write chunk. Without it every write on this board fails.
* Core 1 being parked means the refill is not running, so the DMA ring plays whatever it
  holds for the duration. `Bridge::pause_playback` therefore **stops the transport first**
  and waits 150 ms — the 64-frame ramp and the ring's 23 ms depth several times over — so
  what the ring holds is silence. A write without that pause is not a gap; it is several
  seconds of a loudly repeated 23 ms fragment.
* **`#[ram]` placement for the refill was considered and rejected.** It would not help: the
  refill is not merely fetching instructions, it is reading the module's PCM, and on this
  chip PSRAM is reached through the same cache as flash — so "play out of PSRAM while
  writing flash" does not work either. Parking core 1 and playing silence is the honest
  answer, and it is what esp-storage's own API pushes a caller towards.
* **Reads need none of this.** esp-storage's read path drives the SPI controller from IRAM
  without disabling the cache, and its multi-core check is on the write path alone. A slot
  read is still done under the same pause, for audio quality rather than for safety: it is
  a whole module image through a 4 KiB sector buffer and it contends for the flash bus.

The audible cost of a `POST /api/modules/store` is `TBD (owner: time the silence during a
90 KB store)`; the page warns about it beside the button either way.

### 3. DHCP and DNS — re-checked, and still hand-rolled

`edge-dhcp` 0.8 and `edge-captive` 0.8 exist and are by the same author as the `edge-mdns`
this firmware does use, so they were re-checked rather than assumed. They are not adopted,
for a reason that is structural rather than a matter of taste: both are built on
`edge-nal`'s `UdpBind`/`UdpReceive` over a socket bound to a `SocketAddr`, and the captive
case needs a socket that accepts broadcast traffic to 255.255.255.255 from a client that
has no address yet. smoltcp accepts that on a **port-only** bind — its `accepts()` skips
the address check when the local endpoint address is unspecified — which
`embassy_net::udp::UdpSocket::bind(port)` gives directly and an `edge-nal-embassy` bind
wraps in an address it has to name. ampkeeper reached the same conclusion from the other
direction. Two DHCP options and one DNS answer record are a small enough surface to own.

The DNS responder is the one parser in this firmware that reads bytes a stranger put on an
open network, and it is written accordingly: every offset is bounds-checked against the
slice, compression pointers in a question are refused rather than followed, the sixteen
bytes the answer record needs are reserved **before** the question is copied, and a query
that does not parse is dropped rather than answered. (ampkeeper's own history here is a
crafted query that indexed past a buffer and remotely rebooted the device.)

### 4. mDNS on the classic ESP32

Set up, and **not verifiable without the board** — the honest answer to "confirm the
multicast join on this chip's WiFi driver" is that it compiles and that nothing in
`edge-nal-embassy`'s multicast path is chip-specific. What was confirmed is the shape:
`edge_mdns::io::bind(&udp, IPV4_DEFAULT_SOCKET, Some(Ipv4Addr::UNSPECIFIED), None)` does
the bind **and** the 224.0.0.251 join in one call, which is why both
`embassy-net/multicast` and `edge-nal-embassy/multicast` are enabled.

Two details taken from ampkeeper's scars rather than from the API:

* the **receive** buffer is 1472 bytes and the number is load-bearing. The responder
  receives every multicast mDNS packet on the network, and a datagram larger than the
  buffer handed to `recv` fails the whole responder rather than being truncated — a
  television announcing itself with a kilobyte of TXT records is enough. The **send**
  buffer is 512, because only this device's own answers go through it.
* `edge-mdns` has **no periodic re-announce**: `broadcast` sends its burst once and then
  waits on a signal for ever. So `mdns_task` tears the responder down and rebuilds it on
  every address change, and the rebuild — new socket, new join, new burst — *is* the
  re-announcement.

### 5. WebSocket frame budget

Solved by not having a per-client buffer at all. The control task packs one telemetry block
into a single `static` (`TELEMETRY`, 2 156 B) and each WebSocket sends **straight out of it
under its lock**; the publisher uses `try_lock` and skips a frame it cannot take. So a slow
client costs staleness rather than blocking the player, and two clients cost one buffer
rather than two.

The frame is variable-length — 22 header words plus eight per channel actually present, so
344 bytes for eight-channel `PETRI.S3M` rather than the 2 136 a fixed 64-channel block
would be — at 10 Hz. The client cap the task file asks for falls out of
`WEB_TASK_POOL_SIZE = 2` rather than needing a counter: a WebSocket occupies a worker for
as long as it is open, and there are two.

## The upload path, and what PSRAM can and cannot hold

M8-I3 found that **on the ESP32 the atomic instructions do not work on memory in PSRAM**,
and that esp-alloc's `GlobalAlloc::alloc` is `alloc_caps(EnumSet::empty(), …)`, which takes
the first registered region that fits — so registering PSRAM at all lets an ordinary
allocation land there once DRAM fills, and this engine puts `Arc` reference counts,
seqlocks and a telemetry sequence on the heap.

This task therefore **does not register PSRAM with the allocator either**, and does not use
`alloc_caps(External, …)` as the task brief suggested. `src/psram.rs` takes the region
esp-hal mapped and hands out whole buffers from a bump arena. The guarantee is then
structural rather than statistical: no atomic can reach PSRAM, because the global allocator
has never heard of it. Three buffers are claimed at boot — one 512 KiB upload staging
buffer and two 512 KiB image halves, used ping-pong.

`POST /api/modules` accepts **two kinds of file**, and the difference is the whole design:

* **a module image** (`.spmi`, what `cargo xtask module-image` writes) is copied PSRAM to
  PSRAM and played with `Module::from_image`, which borrows the pattern blob and the PCM
  **in place**. No decode, no DRAM, and the limit is the buffer. This is design (b) from
  the brief, and it is the path a `PETRI.S3M`-sized module takes;
* **a raw module file** is decoded by `starplayer::load`, which allocates the decoded PCM
  in DRAM because that is where the global allocator is, then `Module::to_image` serialises
  it, the image is copied into the free PSRAM half, and both DRAM allocations are dropped
  before `from_image` borrows the PSRAM copy. Steady-state DRAM is then the same as for an
  image — an `Arc`, four index vectors and the sequencer — but the **transient peak** is the
  decoded module plus its image at once. The handler refuses up front when
  `HEAP.free()` is below `3 × content-length + 24 KiB`, with a sentence telling the owner to
  upload a `.spmi` image instead. Refusing is the difference between "too big for this
  board" and a heap-exhaustion panic, which on a device is a reset.

`Arc::new(module)` is in DRAM in both paths, as required.

**The ping-pong needed one thing the brief did not mention.** A module borrows its PSRAM
half for as long as it plays, and `ControlHalf::load` does not end that borrow — the render
half swaps on its next quantum and returns the old `Arc` over the garbage channel, and
`collect_garbage` is what finally drops it. So `Bridge::wait_for_retirement` drains that
channel (ten tries, 10 ms apart) before reusing the other half. In practice the control
task's once-a-second collection has run long before a second upload arrives; "in practice"
is not a guarantee and the failure mode is the mixer reading PCM out from under itself.

## What was done differently from the task file, and why

* **The router is one flat `match` *and* one response type, and the second half of that is
  a memory result, not a style one.** The task file says to use picoserve with "a flat path
  router", which is ampkeeper's lesson about `Router` being a cons list. That alone was not
  enough. `IntoResponse::write_to` is monomorphised per **response type**, and the first
  draft called it from fourteen match arms: the two-worker task pool came out at **97 456 B
  of `.bss`**, which on this chip is 97 456 B of stack, and the firmware did not link.
  Funnelling every JSON and text answer through one `Body` type, extracting the request body
  once as `&[u8]` and decoding it synchronously with `serde_json_core` instead of through a
  per-type `Json<T>` extractor, brought the pool to **68 560 B**. `write_to` now appears
  three times in the whole file. This is written into `web.rs`'s module documentation and
  into the budget, because it is the single easiest thing for a later change to undo.
* **A linker assertion now guards the main stack.** `ld/stack-floor.x` fails the build if
  `_stack_start − _stack_end` drops under 24 KiB. On this chip the stack is whatever `.bss`
  leaves behind, so any static added anywhere silently shortens it and the failure mode is a
  boot that overwrites the WiFi driver's own state rather than a link error. ampkeeper hit
  that twice; a build error is cheaper than a field report.
* **`sequential-storage`'s `remove_all_items` is not used.** It needs `MultiwriteNorFlash`,
  which esp-storage implements for `FlashStorage` but not for the `&mut FlashStorage` the
  store hands it. `erase_wifi` writes an *empty* record instead, and an empty SSID is
  already the "never provisioned" answer everywhere that reads it.
* **`POST /api/reprovision` replaces the task file's "re-provision RTC word".** The brief
  offers a `#[ram(unstable(rtc_fast, persistent))]` word as one of three triggers. An API
  endpoint that erases the credentials and reboots does the same job, is testable with
  `curl`, and needs neither a persistent-RAM dance nor a second boot-decision path. The two
  triggers that remain are the empty stored SSID and the key held at boot.
* **The re-provision key is KEY2, except in the `lcd` build, where it is KEY1.** M8-I5
  sacrificed KEY2 to the display's MOSI, so a build with a screen has no KEY2 at all to
  hold. The gesture moves rather than disappearing; the constant is one `cfg!` in `main.rs`
  with the reasoning beside it.
* **No DHCP hostname.** `DhcpConfig::hostname` is a heapless **0.9** `String`, and this
  crate is on heapless **0.8** because picoserve 0.18 is — the two are different types.
  Carrying a second renamed copy of the crate for one cosmetic field in a router's client
  list was not worth it; mDNS is how the device is found. (This is the one place the version
  split is visible; `StaticConfigV4::dns_servers` is written as `Default::default()` for the
  same reason.)
* **The `connection` task is far simpler than ampkeeper's.** Theirs carries an RSSI poll, a
  scan-request channel and a mode-flip that works around a transmit-queue leak after a long
  outage. None of that has been *observed* on this board, and inventing a workaround for a
  fault nobody has seen here would be a worse deviation than leaving it out. `net.rs` says
  so at the function, and names itself as the place to grow if the owner's run finds the
  link does not recover from a router reboot.
* **`Bridge` exists even on a board with no PSRAM.** The first draft returned
  `Option<Bridge>` and left the control task with nothing to answer jobs with — which would
  have meant a board that could not be **provisioned**, since the portal's `POST /save` goes
  through the same job channel. It now always exists; what it loses without PSRAM is upload
  and slot playback, each answering with a sentence saying so.
* **The `web` build's heap is 96 KiB, not the default build's 120 KiB**, and lives in
  `dram2_seg`. See research point 1.

## `unsafe` inventory (M8-I6's additions)

Master-plan decision 6 tolerates `unsafe` only under `embedded/`, each site with a
`// SAFETY:` comment. M8-I6 adds one module's worth and its call sites:

| Site | Why |
|---|---|
| `src/psram.rs`, `unsafe impl Send for Buffer` | A `Buffer` is a pointer to a region the arena hands out exactly once and never again, so moving it between tasks transfers sole ownership — which is what `Send` claims. Deliberately **not** `Sync`. |
| `src/psram.rs`, `Buffer::as_mut` / `fill` / `view` (`slice::from_raw_parts[_mut]`) | The buffer is *reused*, and each use hands out a `&'static [u8]` that a `Module` borrows from; a `&'static mut` cannot express that, because reborrowing it as shared would freeze it for the rest of the program. The aliasing obligation is stated on each method and discharged by the ping-pong in `web.rs` plus `Bridge::wait_for_retirement`. |
| `src/web.rs`, six call-site blocks | Each names which half it is writing and why nothing is borrowing it. |
| `src/main.rs`, `#[unsafe(link_section = ".dram2_uninit")]` | An unsafe *attribute*, not a block: it places the `web` build's heap array in the reclaimed DRAM region. Edition 2024 requires the `unsafe(…)` spelling. |

`store.rs`, `net.rs` and `provisioning.rs` contain no `unsafe` at all, and neither does
`firmware-common`.

## Verification run

Every command was run in this worktree; `. ~/export-esp-1.97.sh` was sourced first for the
firmware ones.

| Command | Result |
|---|---|
| `cd embedded && cargo test -p starplayer-firmware-common` | pass — **55** tests (22 new: the telemetry word packer against the wasm host's layout index by index, the status and module JSON encoders, the wire-command decoder, the upload state machine) |
| `cd embedded && cargo test -p starplayer-embedded-xtask` | pass — 10 tests (4 new, on the page's staleness rule) |
| `cd embedded && cargo clippy -p starplayer-firmware-common -p starplayer-embedded-xtask --all-targets -- -D warnings` | pass, clean |
| `cd embedded && cargo xtask build --board a1s` | **links**, no warnings — 475 280 B (18.1 %) |
| `cd embedded && cargo xtask build --board a1s --features lcd` | **links**, no warnings — 499 136 B (19.0 %) |
| `cd embedded && cargo xtask build --board a1s --features bench` | **links**, no warnings — 552 560 B (21.0 %), unchanged |
| `cd embedded && cargo xtask build --board a1s --features web` | **links**, no warnings — 1 340 288 B (51.1 %) |
| `cd embedded && cargo xtask build --board a1s --features web,lcd` | **links**, no warnings — 1 367 776 B (52.1 %) |
| `cd embedded && cargo xtask size --board a1s --features web,lcd` | pass — 1 367 776 / 2 621 440 B = **52.1 %** of the factory partition |
| `xtensa-esp32-elf-nm … \| grep _bss_end` | core 0 stack: 38 712 B (default), 37 312 B (`lcd`), 31 188 B (`web`), **29 756 B** (`web,lcd`) — all clear of the new 24 KiB linker floor |
| `cargo run -p starplayer-embedded-xtask -- assets --force` | pass — `index.html.gz` is **6 628 B**, against the 12 288 B budget the asset step enforces |

The default, `lcd` and `bench` images are unchanged but for 32 and 16 bytes on the first
two, which is one `#[cfg]` on a control-task branch and the stack-floor linker fragment.

Not run: `cargo xtask flash` / `monitor` (owner-only, never run by an agent), and nothing
that needs the board.

## Owner steps

In this order. **Flashing is owner-only; no agent has run any of it.**

1. **Provision it.** Flash the `web` build, listen for the compiled-in module, then join
   the open `StarPlayer-XXXX` network from a phone. Confirm the captive-portal page opens
   by itself; if it does not, browse to `http://192.168.4.1/` and say so — that is the DNS
   catch-all, and it is the piece most likely to behave differently per phone.

   ```sh
   cd embedded && . ~/export-esp-1.97.sh
   cargo xtask flash --board a1s --features web
   cargo xtask monitor --board a1s
   ```

2. **Find it again.** After the reset, confirm `curl http://starplayer.local/api/status`
   answers. If mDNS does not resolve, the UART log prints the DHCP address — use that and
   record that mDNS needs work on this chip (research point 4 could not be confirmed
   without the board).

3. **Drive it from the page.** `http://starplayer.local/` — play, stop, next, previous, the
   volume slider, a channel mute, and the WebSocket's channel table moving at 10 Hz.
   Confirm the connection badge recovers after the board is reset under it.

4. **Upload, both ways.** Upload `REFLEX.S3M` raw and hear it. Then
   `cargo xtask module-image` a larger module in the main workspace and upload the `.spmi`
   — that is the path with no DRAM ceiling, and the one worth proving. Record the largest
   raw module that loads before the 507 refusal, which is the real answer to "what fits".

5. **Store it, and time the silence.** Press **store to flash** for a slot and time how
   long the music stops. Reboot, `POST /api/modules/select` that slot, and confirm it plays.
   Put the figure in `plans/reference/embedded-budget.md` §2, where it says TBD.

6. **Watch the underrun counter** in the once-a-second transport line while a page is open
   and while an upload is in flight. Anything above zero with the radio busy is the
   interesting result, and it belongs in the budget beside the figure above.

7. **Re-provision.** Hold KEY2 (KEY1 in an `lcd` build) from power-on for five seconds and
   confirm the portal comes back; then do the same with
   `curl -X POST http://starplayer.local/api/reprovision`.

8. **Watch the stack.** esp-hal's stack-guard watchpoint fires on an overflow. The `web,lcd`
   build has 29 756 B of main stack against a 24 KiB linker floor, and picoserve's request
   handling is the deepest path in the firmware — a guard panic during an HTTP request is
   the signal that a worker's future needs to shrink further, not that the floor should be
   lowered.
