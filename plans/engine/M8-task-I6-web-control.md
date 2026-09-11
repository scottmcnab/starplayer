# M8 — I6: Web control — captive-portal provisioning, an HTTP/WebSocket API and module upload

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md), decision 5; deliverable 8) |
| Status | Planned 2026-09-11; pulled |
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
