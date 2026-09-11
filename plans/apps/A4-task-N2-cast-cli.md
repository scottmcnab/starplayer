# A4 — N2: `starplayer-cast` and `starplayer cast`

| Field | Value |
|---|---|
| Milestone | A4 ([master plan](A4-master-plan.md)) |
| Status | Ready 2026-09-11 |
| Depends on | M3 (`starplayer-host::Player`, the CLI) and M7-H7 (inserts through `Player`) — both landed |
| Blocks | A4-N5 (`--receiver starplayer`); nothing else |
| Parallel with | [N1](A4-task-N1-cast-probe.md) — no shared code; both touch `plans/README.md`, and N1 also touches `xtask/src/main.rs` and `apps/starplayer-web/README.md`, which this task does not |
| Recommended model | Claude Opus (a new crate with a network protocol, a codec and a second `AudioBackend`) |
| Verified by | agent (the Verification section below, including the offline fake receiver), then reviewer, then the owner on a real speaker |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine; the apps and the host crates
are `std`. Read `AGENTS.md` first — in particular design goal 5 (no allocation, no locks,
no panics inside `render()`), the working agreements on formatting and full variable
names, and the rule that you **do not commit**.

`plans/apps/A4-master-plan.md` designs "Cast to Google Home" as five tasks. This is **N2**,
the half that needs no Google registration at all: the CLI has a machine behind it, so it
renders the module itself, serves the audio over HTTP on the LAN, and tells the speaker's
**Default Media Receiver** (Google-hosted, `CC1AD845`) to play that URL. N1 (the receiver
capability probe) and N3/N4 (the custom Web Receiver and the web player's Cast button) are
out of scope here and have no code in common with this task.

Nothing in the root workspace speaks any network protocol today: there is no TLS crate, no
async runtime, no mDNS and no codec. `starplayer-cast` is the first, and it stays
**synchronous** — `rust_cast` is a blocking crate and threads are enough for one session,
one HTTP server and one encoder. **Do not add tokio, smol, async-std or any executor.**

### Code you must read before changing anything

- `apps/starplayer-cli/src/main.rs` — the usage doc block (lines 1–82), `enum Command`
  (lines 118–128) and the `Result<(), String>` error convention `main` prints as
  `starplayer: {message}`.
- `apps/starplayer-cli/src/play.rs` — `run` → `play_on(backend: &mut dyn AudioBackend, …,
  interrupted: &AtomicBool)`, the `follow` poll loop (`POLL_INTERVAL` = 50 ms), the
  once-a-second status line, `player.collect_garbage()` on every pass, `ctrlc` installed
  once from `run`, and `Outcome`. This is the shape `cast.rs` copies.
- `apps/starplayer-cli/src/render.rs` — `MixPathArg` / `InterpArg` / `DepthArg` /
  `AtEndArg` (`clap::ValueEnum`s, already crate-visible), `RenderTarget` and
  `render_dispatch` (the runtime-to-compile-time mixer dispatch), `parse_inserts`,
  `seconds_to_frames`.
- `apps/starplayer-cli/src/{insert_arg,enhance_arg,archive}.rs` —
  `parse_insert_args`, `parse_enhance_arg`, `list_effects`, `list_enhancers`,
  `archive::load_module_bytes(path, entry)`.
- `crates/starplayer-host/src/backend.rs` — `AudioBackend` (`devices`, `negotiate`,
  `open`), `RenderCallback = Box<dyn FnMut(&mut [f32]) + Send>`, `AudioSpec`, `DeviceInfo`,
  `HostError`, `Stream`, `StreamControl`, `StreamHealth`. Note what `open`'s doc says: it
  promises only to call the callback, **not** to start a thread, and the stream comes back
  paused.
- `crates/starplayer-host/src/manual.rs` — `ManualBackend` / `ManualDriver`: the push model
  `CastStreamBackend` copies (the backend stores the callback; something outside pulls).
- `crates/starplayer-host/src/player.rs` — `Player::open`, `load`, `load_module`, `play`,
  `stop`, `seek_frame`, `set_at_end`, `set_fade_frames`, `set_master_volume`,
  `install_insert`, `telemetry`, `song_length`, `collect_garbage`, `warnings`.
- `crates/starplayer-host-cpal/src/lib.rs` — `Int16Writer` (lines 181–213): the
  allocation-free f32 → i16 conversion with dither **off**, walking a block in
  `CONVERSION_SCRATCH_FRAMES` chunks. `CastStreamBackend` needs the same conversion; do not
  copy the type, reproduce the idea in the cast crate (or, if it comes out identical,
  propose moving it into `starplayer-host` — but that is a widening of scope, so ask in the
  research resolution rather than doing it silently).
- `crates/starplayer-offline/src/lib.rs` — `render_song_with_options`, `RenderOptions`,
  `RenderLength` (`default_for`, `repeat_count`, `fade_frames`, `at_end`, `max_frames`),
  `song_timeline`, `GOLDEN_HOST_BLOCK_FRAMES`.
- `crates/starplayer-offline/src/wav.rs` — `WavSample`, `WavHeader::for_format`, the
  **private** `WavHeader::write` (line 175), `write_wav`, `read_wav`, `WavError`.
- `Cargo.toml` `[workspace.dependencies]` — every external version is pinned once, with a
  comment saying why the crate is here and which of its features are off. Follow that
  house style exactly for the four new entries.
- `xtask/src/main.rs` — `NO_STD_CRATES` (lines 91–108) and `JOBS` (line 133).
  `starplayer-cast` is a `std` crate: `cargo test --workspace` and the `clippy` job pick it
  up automatically and it **must not** be added to `NO_STD_CRATES`.
- `plans/apps/A4-master-plan.md` — decisions 1–5 and 8, and research points 3, 4 and 5,
  which are this task's research points 1–3 below.

## Deliverables

### 1. `crates/starplayer-cast` — the crate

A `std` crate, `#![forbid(unsafe_code)]`, added to `[workspace.dependencies]` as a path
dependency like its siblings (`crates/*` is already a workspace member glob, so no
`members` edit). Crate-level docs explain in prose what the Default Media Receiver is, why
the audio has to leave this machine over HTTP, and why the crate is synchronous.

Pin these in `[workspace.dependencies]`, each with a comment in the existing voice:

```toml
rust_cast = { version = "0.21", features = ["thread_safe"] }
mdns-sd = "0.21"
tiny_http = "0.12"
flacenc = { version = "0.5", default-features = false }
```

`rust_cast` 0.21 is `rustls`-based and synchronous; `thread_safe` is what lets the
heartbeat thread and the control thread share one connection. `flacenc`'s default features
(`par`, `serde`) pull in rayon and serde and are off. Dev-dependencies for the fake
receiver: `rustls` 0.23 and `rcgen` (pick the current release and say which in the research
resolution). **No tokio or other async runtime enters this workspace.**

Modules:

#### `discover`

```rust
pub struct CastDevice { pub friendly_name: String, pub model: String, pub id: String, pub address: IpAddr, pub port: u16 }
pub fn discover(timeout: Duration) -> Result<Vec<CastDevice>, CastError>;
```

Browses `_googlecast._tcp.local.` with `mdns-sd`, collecting responses until `timeout`
elapses, de-duplicating on `id` (the TXT `id` key) and sorting by `friendly_name`.
`friendly_name` comes from the TXT `fn` key and `model` from `md`; a record missing `fn`
falls back to the service instance name rather than being dropped. **A speaker group
announces itself exactly like a single device** (master plan decision 8) — do not filter
it out; `md` is how the listing tells the reader which is which.

`pub fn find(devices: &[CastDevice], name: &str) -> Option<&CastDevice>` matches
case-insensitively on a **prefix** of `friendly_name`, so `--device kit` finds "Kitchen
speaker"; an ambiguous prefix is an error naming every match, not a silent first-wins.

#### `session`

Our own `CastSession` type wrapping `rust_cast`, so that `cast-sender` could replace it
without touching the CLI (research point 2):

```rust
pub struct CastSession { /* … */ }
impl CastSession {
    pub fn connect(address: IpAddr, port: u16) -> Result<CastSession, CastError>;
    pub fn launch_default_media_receiver(&mut self) -> Result<(), CastError>;
    pub fn load(&mut self, media: &MediaRequest) -> Result<(), CastError>;
    pub fn play(&mut self) -> Result<(), CastError>;
    pub fn pause(&mut self) -> Result<(), CastError>;
    pub fn seek(&mut self, seconds: f32) -> Result<(), CastError>;
    pub fn stop_media(&mut self) -> Result<(), CastError>;
    pub fn stop_app(&mut self) -> Result<(), CastError>;
    pub fn set_volume(&mut self, level: f32) -> Result<(), CastError>;
    pub fn set_mute(&mut self, muted: bool) -> Result<(), CastError>;
    pub fn status(&mut self) -> Result<MediaStatus, CastError>;
}
```

- Connect over TLS **without host verification**: a Chromecast serves a certificate for its
  own device name signed by a Google device CA, and there is no name to verify against an
  IP address. Say so in a comment; it is the one place this crate deliberately trusts the
  LAN, and the reviewer will look for the justification.
- A **heartbeat thread** pings on `urn:x-cast:com.google.cast.tp.heartbeat` every 5 s and
  answers `PING` with `PONG`; the receiver drops a connection that misses two. It must exit
  cleanly when the session is dropped (an `Arc<AtomicBool>` and a `join` in `Drop`), never
  by being leaked.
- `MediaStatus` is ours, not `rust_cast`'s: `{ player_state, current_time_seconds,
  idle_reason, media_session_id }`, so the CLI never names a `rust_cast` type.
- One `pub enum CastError` with `Display` — `Discovery`, `Connect`, `Protocol`, `Launch`,
  `Load`, `Timeout`, `Io` — each carrying a `String` the CLI can print as one line.

#### `serve`

```rust
pub struct MediaServer { /* … */ }
impl MediaServer {
    pub fn bind_for(device_address: IpAddr) -> Result<MediaServer, CastError>;
    pub fn serve_bytes(&mut self, path: &str, content_type: &str, bytes: Vec<u8>);
    pub fn serve_stream(&mut self, path: &str, content_type: &str, chunks: Receiver<Vec<u8>>);
    pub fn url(&self, path: &str) -> String;
    pub fn shutdown(self);
}
```

`tiny_http` on a thread, bound to `(lan_address, 0)` — a random free port. The LAN address
is found with the **UDP-connect trick** and no extra dependency: bind a `UdpSocket` to
`0.0.0.0:0`, `connect(device_address:port)` (which sends nothing), and read
`local_addr().ip()` — the routing table's answer for "which interface reaches that
speaker". Falling back to `0.0.0.0` and printing which address went into the URL is
acceptable only if that fails; the URL must never contain `127.0.0.1`, which the speaker
cannot reach.

Pre-rendered assets obey **RFC 7233** for a single byte range, because that is what gives
the speaker seek and a clean end:

- Every response to a known path carries `Accept-Ranges: bytes` and `Content-Type`.
- No `Range` header → `200` with `Content-Length` = the whole asset.
- `Range: bytes=first-last`, `bytes=first-` or `bytes=-suffix` → `206 Partial Content`,
  `Content-Range: bytes first-last/total`, `Content-Length` = `last - first + 1`, body =
  exactly those bytes. `last` past the end clamps to `total - 1`.
- A syntactically valid range that is **entirely** past the end → `416 Range Not
  Satisfiable` with `Content-Range: bytes */total` and no body.
- A malformed `Range`, or a multi-range request (`bytes=0-1,4-5`), is **ignored** — answer
  `200` with the whole asset, as RFC 7233 §3.1 permits. Do not implement multipart ranges.
- `HEAD` answers with the identical headers and no body, for both the `200` and `206`
  cases.
- Any other path → `404`, one line of plain text.

`serve_stream` is the `--live` path: no `Content-Length`, `Transfer-Encoding: chunked`,
each `Vec<u8>` from the channel written as one chunk, the response ending when the channel
closes. A `Range` request against a streaming path is answered `200` from the current
position (a live stream has no addressable past).

#### `encode`

```rust
pub trait CastEncoder {
    fn content_type(&self) -> &'static str;
    fn extension(&self) -> &'static str;
    fn encode_all(&mut self, samples: &[i16], sample_rate_hz: u32, channels: u16) -> Result<Vec<u8>, CastError>;
    fn push(&mut self, samples: &[i16]) -> Result<Vec<u8>, CastError>;
    fn finish(&mut self) -> Result<Vec<u8>, CastError>;
}
pub struct FlacEncoder { /* … */ }   // audio/flac
pub struct WavEncoder { /* … */ }    // audio/wav
```

- `FlacEncoder` uses `flacenc` at 16 bits. `encode_all` encodes the whole interleaved
  buffer in one call (the pre-render path, where the total sample count is known and the
  STREAMINFO can be honest). `push` uses `flacenc`'s fixed-size-frame path — encode whole
  frames of a fixed block size out of an internal remainder buffer, emitting a STREAMINFO
  with **unknown total samples** on the first call, and hold back whatever does not fill a
  frame until `finish`.
- `WavEncoder` needs a streaming header, so make `starplayer-offline`'s existing
  `WavHeader::write` public as
  `pub fn write_to<W: Write>(&self, writer: &mut W, sample_count: usize) -> Result<u32, WavError>`
  (keep the private `write` as a thin caller, or rename it and fix `write_wav`'s call
  site). For `--live` the data size is written as the 32-bit maximum a RIFF header can
  hold, since the length is not known and a chunked reader never sees the end of the
  stream; leave a comment saying so, next to `wav.rs`'s existing "4 GiB" doc block, which
  explains why `write_wav` refuses to wrap a size rather than lying about one — the live
  case is the one deliberate exception and must read as such.
- `encode` unit tests: FLAC output starts with `fLaC` and its STREAMINFO reports the
  sample rate, channel count and (for `encode_all`) the exact frame count that went in;
  WAV output round-trips through `starplayer_offline::read_wav` after being written to a
  temporary file. Decoding the FLAC back is better if `flacenc`'s own decoder is reachable
  as a dev-dependency feature — check, and say in the research resolution which check you
  ended up with and why.

#### `live`

```rust
pub struct CastStreamBackend { /* … */ }
impl CastStreamBackend {
    pub fn new(spec: AudioSpec, encoder: Box<dyn CastEncoder + Send>, chunks: SyncSender<Vec<u8>>) -> CastStreamBackend;
}
impl AudioBackend for CastStreamBackend { /* devices / negotiate / open */ }
```

A **push** backend in the shape of `ManualBackend`: `open` stores the `RenderCallback` and
returns a paused `Stream`; `Stream::play` starts a **driver thread** which from then on
pulls blocks itself. The driver:

1. paces against the wall clock, keeping about **2 s of audio ahead** of real time (Cast's
   own buffer is 2–5 s on top of that — research point 3). It renders one block, advances
   a frame counter, and sleeps until the counter is that far ahead; it never spins.
2. converts the interleaved `f32` the callback fills into `i16` with a clamping,
   dither-off conversion (`Int16Writer`'s, above) into a scratch buffer sized when the
   stream opened.
3. feeds the encoder and sends each non-empty `Vec<u8>` to `chunks`. A **bounded**
   `sync_channel` is deliberate: if the HTTP side stalls, the driver blocks rather than
   growing without limit. The `StreamHealth` records a non-fatal error each time it has to
   wait, so the CLI can report "the speaker is not draining the stream".
4. exits when `Stream` is dropped, flushing `finish()` into the channel and closing it.

`Player` is opened on this backend exactly as it is on cpal — `Player::open(&mut backend,
None, spec, MixerMode::DEFAULT)` — and everything the render callback reaches still obeys
design goal 5. **Encoding happens on the driver thread, after the callback has returned,
never inside it**: the callback fills a pre-sized scratch and nothing else. Say this in the
module doc, because it is the invariant a reviewer will check first.

`collect_garbage` and every other `Player` call are made from the **control thread only** —
the thread running the CLI's follow loop, which is the sole producer on the command ring.
Do not call into `Player` from the driver thread or the heartbeat thread.

### 2. `apps/starplayer-cli/src/cast.rs` — the subcommand

```text
starplayer cast --list [--timeout SECS]
starplayer cast --device NAME <file> [--entry N] [--format flac|wav] [--rate HZ]
                [--insert TARGET:EFFECT[:PARAM=VALUE,...]] [--enhance SPEC]
                [--repeat] [--fade S] [--live] [--volume 0..1] [--timeout SECS]
```

`CastArgs` + `pub fn run(args: CastArgs) -> Result<(), String>`, registered as
`Command::Cast` in `main.rs`, exactly like `play`.

- `--list` prints one line per device — friendly name, model, address:port — and exits 0
  even when nothing answers, with a line saying so (mDNS finding nothing is a normal
  result, not an error; see research point 4).
- **Pre-render path (the default).** Load the module through `archive::load_module_bytes`,
  apply `--enhance`/`--insert` the way `render` does, build a `RenderLength` from
  `--repeat`/`--fade`, and render to interleaved 16-bit stereo at `--rate` (default
  44 100). Add `pub(crate) fn render_pcm_i16_stereo(...) -> Result<Vec<i16>, String>` to
  `render.rs` beside `render_dispatch` — the same match, pinned to `FloatOut<i16, 2>` /
  `FixedOut<i16, 2>`, eight arms over mix path × interpolator — rather than duplicating the
  offline call in `cast.rs` or making `render_dispatch` write to a temporary file. Encode,
  hand the bytes to `MediaServer::serve_bytes`, LOAD, then run a control loop that prints
  the receiver's player state and position once a second (the `play.rs` status-line shape).
- **Live path (`--live`).** `Player` on `CastStreamBackend`, `streamType: LIVE`, no
  duration. This is the "endless jam-mode session on the kitchen speaker" case and nothing
  else — the master plan's decision 3.
- **The LOAD payload**:
  - `contentId`: `http://<lan-ip>:<port>/<title>.flac` (or `.wav`) — the `MediaServer` URL,
    with the module's title slugified to ASCII `[A-Za-z0-9_-]` and falling back to the
    file stem, then to `module`. The extension matters: some receivers sniff it.
  - `contentType`: `audio/flac` or `audio/wav`.
  - `streamType`: `BUFFERED` for a pre-render, `LIVE` for `--live`.
  - `metadata`: `MusicTrackMediaMetadata` with `title` = the module's own title
    (`module.header().title`, as `play.rs` reads it), falling back to the file name when it
    is empty. This is what the Google Home app and the speaker's own controls show.
  - `duration`: the rendered length in seconds (`frames / rate`) for a buffered stream;
    absent for `LIVE`.
- **One player at a time, and stop the app on exit.** Before LOAD, if the receiver is
  already running something, stop its media. On exit — natural end, `q`, or `Ctrl-C` —
  stop the media, then stop the **application** if and only if this process launched it
  (a session that attached to an already-running receiver leaves it alone), then shut the
  server down and join its thread. A `Ctrl-C` that arrives mid-song must still do all
  three; install the handler once from `run`, as `play.rs` does, and have the control loop
  poll an `AtomicBool`.
- **Transport commands arrive as newline-terminated single letters on stdin**: `p`
  pause/play, `s` stop, `q` quit, `+`/`-` volume by 0.05, `<`/`>` seek ∓10 s, and a bare
  newline prints the status line immediately. Read them on a reader thread that forwards
  `char`s down a channel the control loop selects on, so the loop never blocks on stdin.
  **This is a deliberate deviation** from the A4 master plan's "with transport keys while
  it runs": raw-mode key input needs a terminal crate (crossterm or similar) and this
  workspace has none — the TUI (A1) is where raw mode belongs, and adding it here would
  pre-empt that task's own choice. Record the deviation in a `## Deviations from the task
  file` section and say so in one line of `--help`.
- `--volume` sets the **receiver's** volume through `CastSession::set_volume`, not
  `Player::set_master_volume`: the speaker's own volume is what the user will reach for,
  and two volume controls that disagree is a bug report.
- State the measured latency in `--help` (research point 3), and say there that Cast's
  buffering is why jam mode over Cast is not offered.

### 3. Tests (offline; no speaker, no network hardware)

- `crates/starplayer-cast/tests/fake_receiver.rs` — an in-process CASTv2 receiver on
  `rustls` with an `rcgen` self-signed certificate, on `127.0.0.1` with an ephemeral port.
  Decode the `CastMessage` protobuf **by hand** — it is seven fields and no more, and a
  protobuf crate for this would be a dependency whose only user is one test:

  | # | Field | Wire type |
  |---|---|---|
  | 1 | `protocol_version` | varint (0 = `CASTV2_1_0`) |
  | 2 | `source_id` | length-delimited UTF-8 |
  | 3 | `destination_id` | length-delimited UTF-8 |
  | 4 | `namespace` | length-delimited UTF-8 |
  | 5 | `payload_type` | varint (0 = `STRING`, 1 = `BINARY`) |
  | 6 | `payload_utf8` | length-delimited UTF-8 |
  | 7 | `payload_binary` | length-delimited bytes |

  Each message on the socket is preceded by a **4-byte big-endian length**. The namespaces
  to assert are `urn:x-cast:com.google.cast.tp.connection`,
  `urn:x-cast:com.google.cast.tp.heartbeat`, `urn:x-cast:com.google.cast.receiver` and
  `urn:x-cast:com.google.cast.media`. Script the exchange
  `CONNECT → GET_STATUS → LAUNCH → CONNECT(transport) → LOAD → PLAY → PAUSE → STOP`,
  answering each with the JSON a real receiver sends, and assert: the namespace and
  `destination_id` of every request, that `requestId`s increase and each response is
  matched to its own request, that `PING` is answered with `PONG`, and the whole LOAD
  payload — `contentId` is the server's own URL, `contentType`, `streamType`, the
  `MusicTrackMediaMetadata` title, and `duration` present for `BUFFERED` and absent for
  `LIVE`.
- `crates/starplayer-cast/tests/range.rs` — the `MediaServer`'s `Range` behaviour: the six
  cases listed in deliverable 1 (`200`, three `206` forms, `416`, ignored-malformed), `HEAD`
  for `200` and `206`, `404` on an unknown path, `Accept-Ranges` on every hit, and that the
  bytes of a `206` equal the same slice of the whole asset.
- `encode` unit tests as described above, and a `live` test that drives a
  `CastStreamBackend` from a `ManualBackend`-style fixed callback and asserts the chunks
  that come out decode to the frame count that went in.
- **Goldens are untouched.** Nothing in this task may change a rendered byte; if
  `cargo xtask ci --job goldens` moves, stop and report it.

### 4. Documentation

- `apps/starplayer-cli/src/main.rs` usage block (lines 1–82): a `starplayer cast` section in
  the voice of the `play` section — what it does, the two paths, the stdin transport
  letters, the latency figure, and the note that `--list` finding nothing is a normal
  result on a NATed network.
- `README.md` (the repository root): the **Layout** block's `crates/` line mentions the Cast
  sender, and **Building** gains a `starplayer cast --list` / `cast --device` line beside
  the existing `cargo xtask wasm` one. Add a short paragraph there noting that Chrome's own
  **"Cast this tab"** streams the web player to a Chromecast today at tab-mirroring quality
  and latency and stops when the tab closes — the no-code option, listed honestly, and not
  what N2 is.
- `AGENTS.md` "Layout": add `starplayer-cast` to the `std` crate line.
- `plans/product/01-technical-architecture.md` §11 (the crate list around line 1580), in the
  `std` block beside `starplayer-host-cpal`:
  `starplayer-cast  Google Cast sender: mDNS discovery, a CASTv2 session, an HTTP media
  server and a FLAC/WAV encoder  → starplayer, starplayer-host, starplayer-offline,
  rust_cast, mdns-sd, tiny_http, flacenc`.
- `plans/README.md`: the A4 row's status, once this lands.

## Research points

Answer each in a `## Research resolution` section you append to this file, in the shape
`plans/engine/complete/M3-task-D8-cli-play.md` uses: a bolded one-line verdict, then the
evidence.

1. **The Default Media Receiver and live streams** (A4 point 3). Does a Nest speaker accept
   a WAV or FLAC stream with no `Content-Length` and `Transfer-Encoding: chunked` under
   `streamType: LIVE`? Reports say WAV-on-the-fly works on Chromecast Audio; this needs
   confirming on Nest hardware, which is an owner step. Say what the code does in the
   meantime, what the fake receiver proves offline, and what exactly the owner should try.
2. **`rust_cast` maintenance** (A4 point 4). The crate has changed hands before. Record the
   exact version resolved, what its `thread_safe` feature turns on, which of its types leak
   into our API (the answer should be *none*), and what replacing it with `cast-sender`
   would cost — the point of `CastSession` is that the answer is "one module".
3. **Latency** (A4 point 5). Measure the wall-clock delay between the LOAD returning and
   sound leaving the speaker, and between a `p` and the speaker reacting, on the owner's
   devices if reachable and from the fake receiver plus the documented 2–5 s buffer if not.
   State the figure in `--help` and say why it rules out jam mode over Cast.
4. **The LAN bind address, and WSL2.** Show that the UDP-connect trick picks the interface
   that reaches the device, what it returns when there is no route, and what the code does
   then. Then run `starplayer cast --list` on this machine and record what happens:
   WSL2's NAT does not forward multicast, so mDNS is expected to find nothing and the
   speaker could not reach a server bound inside the VM either. The honest result is a
   clear message naming `--timeout` and saying that mirrored networking or a native
   Linux/Windows build is what this needs — not a hang and not a stack trace.

## Verification

```sh
cargo test -p starplayer-cast
cargo test -p starplayer-cli
cargo test --workspace
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
cargo xtask ci --job no-std-purity
cargo xtask ci --job goldens
cargo run -p starplayer-cli -- cast --list                      # WSL2: expected to find nothing; record the exact output
cargo run -p starplayer-cli -- cast --help
cargo build --release -p starplayer-cli                         # record the binary size before and after this task
```

Report the exact commands and results. **Do not commit** — the reviewer commits.

## Out of scope

N3 (the StarPlayer Web Receiver), N4 (the web player's Cast button), N5 (`--receiver
starplayer`). Raw-mode transport keys and any terminal-input crate — those are A1's. Jam
mode over Cast. Creating or editing speaker groups. Any change to the engine, the mixer,
the goldens or the `no_std` crates. Adding an async runtime.

---

## Research resolution

### 1. The Default Media Receiver and live streams (A4 point 3)

**Verdict: the code emits a chunked stream with no `Content-Length` under `streamType:
LIVE` and the offline suite proves the wire format is well-formed, but whether *Nest
hardware* accepts it is an owner step that cannot be settled from here.**

What the code does now:

- `MediaServer::serve_stream` answers a live path with `200`, `Transfer-Encoding: chunked`,
  `Content-Type: audio/flac` (or `audio/wav`), no `Content-Length`, and
  `Accept-Ranges: none`. A `Range` request against a live path is ignored and answered from
  the current position — a live stream has no addressable past.
- `FlacEncoder`'s streaming path writes a STREAMINFO whose `total_samples` is **0**, the
  format's "unknown", and whose frame-size minimum/maximum are also zeroed. (`flacenc`'s
  `StreamInfo::new` defaults the minimum to `u32::MAX` and the maximum to `0` so the first
  frame narrows them; written into a header no frame will ever update, that produces a
  STREAMINFO saying the smallest frame is larger than the largest, which its own parser
  rejects. Zeroing both is the fix and is what the format intends.)
- `WavEncoder`'s streaming path writes one 44-byte header declaring `data` = 4 294 967 258
  bytes — the largest size that still leaves the outer `RIFF` size inside a 32-bit field —
  and then raw little-endian `i16`. This is the single deliberate exception to `wav.rs`'s
  "4 GiB" rule, and it is commented as such in `encode.rs`: the length is unknown when the
  header goes out, the response is chunked so no reader ever reaches the end, and a
  receiver that trusts the declared length simply never gets there.
- The LOAD payload for `--live` carries `streamType: LIVE` and **no** `duration` field at
  all (`rust_cast` skips a `None`).

What the offline suite proves, with no device and no network beyond loopback:

- `tests/range.rs::a_live_path_is_answered_chunked_and_ends_when_the_channel_closes`
  decodes the chunked body by hand and asserts `Transfer-Encoding: chunked`, the absence of
  `Content-Length`, that a `Range` header is ignored, and that every byte fed into the
  channel comes out of the response in order.
- `tests/fake_receiver.rs::a_live_load_payload_says_live_and_carries_no_duration` asserts
  the LOAD payload's `streamType` is `LIVE` and that `duration` is absent, against a LOAD
  whose protobuf and JSON the fake receiver decoded itself.
- `live.rs::the_chunks_of_a_live_flac_stream_decode_to_the_frame_count_that_went_in` runs
  the real `CastStreamBackend` driver and decodes the chunks back through `flacenc`'s own
  parser: the frame count and the sample count equal what the render callback filled.
- `encode.rs::a_live_flac_stream_declares_an_unknown_length_and_still_round_trips` pushes
  ragged blocks through `FlacEncoder` and decodes the result losslessly.

**What the owner should try, on the real hardware:**

1. `starplayer cast --device "<speaker>" song.s3m --live` with the default `--format flac`.
   Expect audio within a few seconds. If the speaker goes `IDLE` with `idleReason` `ERROR`
   in the status line, the receiver refused the stream.
2. If FLAC fails, repeat with `--live --format wav`. Reports of WAV-on-the-fly working are
   specifically about Chromecast Audio; Nest hardware is the open question, and WAV is the
   likelier of the two to be accepted because it needs no container-level length at all.
3. Note whether the Google Home app shows a running clock or a stalled one, and whether the
   speaker's own next/previous controls do anything — a `LIVE` stream should present as a
   live tap with no seek bar.
4. Report which of the two worked. If neither does, the fallback is not a code change but a
   scope one: `--live` is only for an endless session, and the pre-render path (which is the
   default and does not depend on this answer) covers playing a song.

### 2. `rust_cast` maintenance (A4 point 4)

**Verdict: `rust_cast` 0.21.0 resolved; `thread_safe` swaps its internal `Rc`/`RefCell` for
`Arc`/`Mutex`; **no** `rust_cast` type appears anywhere in this crate's public API; and
replacing it with `cast-sender` would be a rewrite of `session.rs` and nothing else.**

Evidence:

- **Version resolved:** `rust_cast 0.21.0` (the manifest pins `"0.21"`). Its own pins are
  `protobuf =3.7.2` (exact, with `protobuf-codegen` as a build dependency generating
  `cast_channel.rs` from the Chromium Open Screen `.proto`), `rustls 0.23`,
  `rustls-native-certs 0.8`, `serde`/`serde_json 1`, `thiserror 2`, `byteorder 1.5`,
  `log 0.4`. It is `edition 2024` and MIT.
- **What `thread_safe` turns on:** two things, both in the crate's own source.
  `src/lib.rs` defines `type Lrc<T> = std::sync::Arc<T>` with the feature and
  `std::rc::Rc<T>` without it; `src/message_manager.rs` defines its internal `Lock<T>` as
  `std::sync::Mutex<T>` with the feature and `std::cell::RefCell<T>` without. That is the
  whole difference, and it is exactly what lets this crate's heartbeat thread and control
  thread share one `MessageManager`. The crate's own `test_thread_safe` asserts
  `CastDevice: Send + Sync` under the feature. Its only other feature, `cast`, merely
  re-exports the generated protobuf module and stays off.
- **Types that leak into our API: none.** `grep rust_cast crates/starplayer-cast/src`
  finds uses only inside `session.rs`'s private body and in prose. The public surface is
  `CastSession`, `MediaRequest`, `MediaStatus`, `StreamKind`, `CastError`, `CastDevice`,
  `MediaServer`, `CastEncoder`/`FlacEncoder`/`WavEncoder` and `CastStreamBackend` — all
  ours, made of `String`, `f32`, `i32` and `IpAddr`. `StreamKind::to_rust_cast` is private.
  Player state and idle reason cross the boundary as the protocol's own strings
  (`"PLAYING"`, `"CANCELLED"`) rather than as `rust_cast::channels::media::PlayerState`.
- **Cost of replacing it with `cast-sender`:** one module. `session.rs` is 470 lines, of
  which the `rust_cast`-specific parts are the four channel constructors, the nine
  request calls and the `NoCertificateVerification` reference. Nothing above it —
  `serve.rs`, `encode.rs`, `live.rs`, `apps/starplayer-cli/src/cast.rs`, both integration
  tests — names `rust_cast` at all, and `tests/fake_receiver.rs` asserts on the *wire*
  (protobuf frames and JSON payloads), not on library types, so it would keep working
  unchanged against a different client. The one real obstacle is that `cast-sender` 0.3 is
  built on `smol`, which would mean an async runtime in this workspace; that is a reason to
  stay on `rust_cast` rather than a reason the wrapper does not work.
- **One thing `rust_cast` could not give us**, worth recording for whoever revisits this:
  `CastDevice::connect_without_host_verification` builds and hides its own `TcpStream`, so
  there is no way to set a read timeout on it, and a speaker that stops answering would
  hang the CLI for ever. `session.rs` therefore assembles the same stack itself —
  `rustls` client → `MessageManager::new` → the four public channel constructors — over a
  socket it holds a `try_clone` of. This is all public API of `rust_cast` (its own test
  module documents exactly this construction), not a fork.

### 3. Latency (A4 point 5)

**Verdict: the CASTv2 control round trip is ~0.2 ms in-process and about a millisecond on
a LAN; everything else a listener perceives is the receiver's own 2–5 s playout buffer,
which a sender cannot see or shorten. `--help` states 2–5 s, and that is what rules out jam
mode over Cast.**

Measured here, against the fake receiver over loopback TLS
(`cargo test -p starplayer-cast --test fake_receiver -- --ignored --nocapture
measure_round_trip_latency`, which is kept in the suite as `#[ignore]`d evidence):

```
connect  1.207206ms      TCP + TLS handshake + CONNECT
launch   440.805µs       GET_STATUS + LAUNCH + CONNECT(transport)
load     207.951µs       LOAD, request to matching MEDIA_STATUS
pause    188.054µs       mean of 50 PAUSE round trips
```

A note on how that number was obtained, because the first attempt was wrong: the same
measurement read 44 ms per round trip until the fake receiver's accepted socket got
`set_nodelay(true)`. Forty milliseconds is the delayed-ACK timer, not the protocol —
`rustls` writes a record's header and payload separately and Nagle held the second. The
session's own client socket has had `set_nodelay(true)` from the start for the same reason.

On a real LAN, add the network RTT: a wired or 5 GHz link to a speaker is 1–3 ms, so a
transport command reaches the receiver in single-digit milliseconds.

**None of that is what a listener hears.** Between LOAD returning and sound leaving the
speaker, and between a `p` and the speaker reacting, sits the receiver's own buffer, which
Google documents as several seconds and which senders have no control over: the Cast
audio-device guidance caps a stream buffer at 2 MB and the receiver decides its own
pre-roll. Two to five seconds is the figure every Cast sender reports, and the `--live`
path adds this crate's own `STREAM_LEAD` (2 s) on top, because the driver thread
deliberately renders ahead of the wall clock so a network hiccup does not starve the
receiver.

**Why that rules out jam mode over Cast:** playing along with what you hear requires the
round trip from your hands to your ears to stay under roughly 10–20 ms. Cast is two to
three orders of magnitude away from that, the latency is in a component we do not control,
and no amount of reducing `STREAM_LEAD` touches it — at lead zero the speaker's own buffer
still stands between the render and the room. `--live` exists so an endless session can be
*listened to* on the kitchen speaker, not so it can be played into. `--help` says exactly
this, in the "Latency" paragraph.

**Owner measurement, when a speaker is reachable:** with a watch, time (a) `starplayer cast
--device … song.s3m` from the `loaded` line to the first sound, and (b) typing `p` + Enter
to the speaker going quiet. Both should land in the 2–5 s band; anything much larger points
at the network rather than at the receiver.

### 4. The LAN bind address, and WSL2

**Verdict: the UDP-connect trick returns the kernel's own routing answer, returns nothing
at all when there is no route, and finds no Cast device on this machine — which is the
correct and expected result under WSL2's NAT, reported as a clear message rather than a
hang or a stack trace.**

**The trick picks the interface that reaches the device.** `serve::local_address_reaching`
binds a `UdpSocket` to `0.0.0.0:0`, calls `connect(device_address:9)` — which sends no
packet; UDP `connect` only fixes the peer — and reads `local_addr().ip()`. Demonstrated on
this machine, whose only routable interface is WSL2's `eth0` at `172.31.34.119/20` with a
default route via `172.31.32.1`:

```
192.168.1.40     -> 172.31.34.119     a LAN address off-subnet: the default route's interface
127.0.0.1        -> 127.0.0.1         loopback picks loopback
8.8.8.8          -> 172.31.34.119     the internet: same default route
172.31.34.200    -> 172.31.34.119     on-subnet: the link's own source address
2001:db8::1      -> no route (Network is unreachable)
```

`crates/starplayer-cast/src/serve.rs::the_route_to_loopback_is_loopback` pins the loopback
case in the suite.

**What it returns when there is no route:** the `connect` fails with `ENETUNREACH` (the
IPv6 line above), so `local_address_reaching` returns `None`. `MediaServer::bind_for` then
falls back to binding `0.0.0.0`, the URL names whatever the listener reports, and
`apps/starplayer-cli/src/cast.rs::warn_about_an_unroutable_url` prints a warning naming
WSL2 and mirrored networking. The URL can never contain `127.0.0.1` by accident: the
unspecified-address check in `local_address_reaching` rejects the kernel's "I have not
picked an interface" answer, and loopback is only ever returned when the device address is
itself loopback, which only the tests do.

**`starplayer cast --list` on this machine**, verbatim:

```
$ cargo run -p starplayer-cli -- cast --list
no cast device answered on this network.
That is a normal result, not necessarily a fault: mDNS is multicast, and multicast does not
cross a NAT. Inside WSL2's default networking, inside most containers, and across a VLAN
boundary, nothing will ever answer — and a speaker could not reach a media server bound in
there either. Try a longer --timeout, or run this from a native Linux or Windows build on
the same subnet as the speaker (WSL2 needs mirrored networking).
$ echo $?
0
```

That is the expected result and the honest one. Exit status is 0, because mDNS finding
nothing is a normal outcome and a non-zero exit would make it look like a fault. The message
names `--timeout` and says what actually fixes it. There is no hang: the browse runs for
exactly `--timeout` seconds and stops, and `cast --device …` on the same machine fails
immediately afterwards with the same text rather than proceeding to bind a server the
speaker could never reach.

### 5. Not asked, but the reviewer will: `Int16Writer`

The task said to reproduce `starplayer-host-cpal`'s allocation-free `f32` → `i16`
conversion in the cast crate rather than copying the type, and to *ask* rather than move it
into `starplayer-host`. **It turned out that neither was needed.** `Int16Writer` is a thin
wrapper around `starplayer::mixer::HostSample`, which is already a shared, public,
`no_std` trait: `live.rs`'s driver calls `<i16 as HostSample>::from_unit_f32(sample, &mut
dither)` with `Dither::OFF` directly, into a scratch sized when the stream opened, exactly
as `Int16Writer::write` does. So there is one conversion rule in the tree, not two, and no
widening of scope to propose. **The question for the reviewer is therefore only whether
that is the right call** — the alternative would be hoisting `Int16Writer` itself into
`starplayer-host`, which buys a shared *buffer-walking loop* on top of a shared conversion,
and the cast driver does not need one because it renders a fixed block size it chose.

---

## Deviations from the task file

1. **`discover::find` returns `Result<&CastDevice, CastError>`, not
   `Option<&CastDevice>`.** The task file's signature block says `Option`, and the sentence
   immediately after it says "an ambiguous prefix is an error naming every match, not a
   silent first-wins". `Option` cannot carry that error. `Result` was the only way to obey
   the requirement, and the error text names every match.

2. **`rustls` 0.23 is a normal dependency of `starplayer-cast`, not only a
   dev-dependency.** The task listed four crates to pin plus `rustls`/`rcgen` "for the fake
   receiver". `session.rs` needs rustls on the *client* side too, because
   `rust_cast::CastDevice::connect_without_host_verification` builds and hides its own
   `TcpStream` and there is no way to set a read timeout on a socket you cannot reach — so
   a speaker that stopped answering would hang the CLI indefinitely, which research point 4
   explicitly rules out ("not a hang and not a stack trace"). The same stack is therefore
   assembled here from `rust_cast`'s own public pieces (`MessageManager::new` plus the four
   channel constructors — the construction its own test module documents) over a socket
   this crate holds a `try_clone` of. It is `rust_cast`'s own rustls version, so exactly
   one rustls and one crypto provider (`aws-lc-rs`) are linked. `rust_cast`'s public
   `NoCertificateVerification` is reused rather than re-implemented.

3. **`rcgen` is pinned with `default-features = false, features = ["aws_lc_rs"]`.** Its
   default feature set pulls `ring`, which would put a second C crypto library into the
   dev-dependency tree beside the `aws-lc-rs` rustls already links. The task said to "pick
   the current release and say which": **0.14.10**.

4. **Transport keys are newline-terminated letters on stdin.** This is the deviation the
   task file itself asked for and asked to have recorded: raw single-keypress input needs a
   terminal crate this workspace does not have, and choosing one here would pre-empt A1's
   own choice. `starplayer cast --help` says so in its own words.

5. **`--repeat` takes a count, not a bare flag.** The task file's usage line shows
   `[--repeat]`. `render`'s `--repeat` is a `u32` count of extra passes and `play`'s is a
   bare flag; `cast`'s pre-render path goes through `RenderLength::repeat_count`, so it
   follows `render`. `--live` ignores it and says so in the flag's own help: a live stream
   loops forever by construction (`AtEnd::Continue`) and has no fade to describe.

6. **`CastSession` grew three methods the task file's block did not list**, all of them
   forced by requirements elsewhere in the same task: `launched_here()` (so the exit path
   can stop the application "if and only if this process launched it"), `pump()` (so
   incoming `PING`s get a `PONG` — the control thread is the only reader, see below), and
   a `Debug` impl. `MediaServer` likewise grew `address()`, which the `Range` tests and the
   unroutable-URL warning both need.

7. **Reads happen only on the control thread; the heartbeat thread only writes.** The task
   file says the heartbeat thread "pings every 5 s and answers `PING` with `PONG`". Doing
   both on that thread deadlocks: `rust_cast`'s `MessageManager` holds one lock around the
   stream for the whole of a blocking read, so a heartbeat thread parked in `read` would
   block the control thread's next `send` indefinitely. The split implemented instead is
   heartbeat-thread-sends-`PING`, control-thread-answers-`PING`-in-`pump()`, which the CLI
   calls once a second. The thread still exits on an `Arc<AtomicBool>` and is joined in
   `Drop`, as the task requires. `tests/fake_receiver.rs` sends a `PING` from the receiver
   and asserts the `PONG` comes back.

8. **The `--live` driver's blocking send is a retry loop rather than `SyncSender::send`.**
   The task says "if the HTTP side stalls, the driver blocks rather than growing without
   limit", and it does — but a plain blocking `send` deadlocks against `Drop` when the
   consumer has already stopped reading, which is exactly what happens when the `Stream` is
   dropped. `send_chunk` retries on a tick and checks the stop flag between attempts, and
   still records one non-fatal `StreamHealth` error the first time a chunk has to wait.

9. **`WavHeader::write` was renamed to `write_to` rather than kept as a private caller.**
   The task offered either; renaming leaves one function instead of two and `write_wav`'s
   single call site was updated.

10. **`flacenc`'s `decode` feature is used as a dev-dependency feature.** The task asked
    which check the encode tests ended up with: the real one. `flacenc = { workspace =
    true, features = ["decode"] }` under `[dev-dependencies]` turns on its own
    `component::parser` and `Decode`, so the FLAC tests parse the STREAMINFO, walk every
    frame with CRC checking, and assert the decoded samples equal the input — rather than
    asserting on byte patterns. Nothing the crate ships ever decodes FLAC.

11. **`tests/fake_receiver.rs` carries one `#[ignore]`d test**,
    `measure_round_trip_latency`. It is the evidence behind research point 3 and is
    reproducible; being `#[ignore]`d it costs CI nothing.

---

## Verification results (this run)

Run in `/home/scott/projects/starplayer-a4-n2-cast-cli` on the pinned toolchain
(`1.97-x86_64-unknown-linux-gnu`), WSL2.

| Command | Result |
|---|---|
| `cargo test -p starplayer-cast` | **pass** |
| `cargo test -p starplayer-cli` | **pass** |
| `cargo test --workspace` | **pass** |
| `cargo xtask ci --job clippy` | **pass** |
| `cargo xtask ci --job host-tests` | **pass** |
| `cargo xtask ci --job no-std-purity` | **pass** |
| `cargo xtask ci --job goldens` | **pass — every fingerprint unchanged** |
| `cargo run -p starplayer-cli -- cast --list` | **pass** — found nothing, exit 0, as expected on WSL2 |
| `cargo run -p starplayer-cli -- cast --help` | **pass** |
| `cargo build --release -p starplayer-cli` | **pass** — 4 606 656 → 10 438 472 bytes |

### `cargo test -p starplayer-cast`

```
running 30 tests   test result: ok. 30 passed; 0 failed; 0 ignored     (lib)
running 7 tests    test result: ok. 6 passed; 0 failed; 1 ignored      (tests/fake_receiver.rs)
running 10 tests   test result: ok. 10 passed; 0 failed; 0 ignored     (tests/range.rs)
running 0 tests    test result: ok. 0 passed; 0 failed; 0 ignored      (doc-tests)
```

The one ignored test is `measure_round_trip_latency`, the research-point-3 measurement.

### `cargo test -p starplayer-cli`

```
running 34 tests   test result: ok. 34 passed; 0 failed; 0 ignored     (unit, incl. 5 new cast tests)
running 7 tests    test result: ok. 7 passed; 0 failed; 0 ignored      (tests/enhance_flags.rs)
running 2 tests    test result: ok. 2 passed; 0 failed; 0 ignored      (tests/golden_reproduction.rs)
running 5 tests    test result: ok. 5 passed; 0 failed; 0 ignored      (tests/insert_flags.rs)
running 2 tests    test result: ok. 2 passed; 0 failed; 0 ignored      (tests/robustness.rs)
```

### `cargo test --workspace`

```
exit status 0
98 test binaries reported `test result: ok`
1565 tests passed, 0 failed
```

### `cargo xtask ci --job clippy`

```
=== xtask ci: clippy ===
     cargo clippy --workspace --all-targets -- -D warnings
    Checking starplayer-cli v0.1.0 (…/apps/starplayer-cli)
    Checking starplayer-cast v0.1.0 (…/crates/starplayer-cast)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.94s

xtask ci: 1 job(s) passed
```

### `cargo xtask ci --job host-tests`

```
=== xtask ci: host-tests ===
     cargo tree --edges features --workspace
     cargo test --workspace
…
xtask ci: 1 job(s) passed
```

### `cargo xtask ci --job no-std-purity`

```
     cargo tree --edges features,no-dev --target riscv32imac-unknown-none-elf -p starplayer-telemetry
     cargo tree --edges features,no-dev --target riscv32imac-unknown-none-elf -p starplayer
     cargo tree --edges features,no-dev --target riscv32imac-unknown-none-elf -p starplayer-host-embedded
     scanning manifests for `std` in a default feature set

xtask ci: 1 job(s) passed
```

`starplayer-cast` is a `std` crate and is deliberately **not** in `NO_STD_CRATES`; none of
its five external dependencies is reachable from any `no_std` crate.

### `cargo xtask ci --job goldens`

```
=== xtask ci: goldens ===
ok    c5adc0cc55f95e9147a24530e9c4d0803f9c9930a59ad8107ac386ff7c3eb82a  goldens/mod/synthetic__i16_mono_44100_linear.sha256
ok    2ca02ee1d5ec67fb6b603ba28950cccc0a6044d44ce9f4836b3e83a5345dcabb  goldens/mtm/synthetic__i16_mono_44100_linear.sha256
ok    20beedc6f28ef53716dd63563789d70325ec50cf2350da907e236dd22fd48039  goldens/xm/synthetic__i16_mono_44100_linear.sha256
ok    a5da3e4b0ac210a910d98e2eb44b28e584a999f408eea542b5070431d13bb218  goldens/it/synthetic__i16_mono_44100_linear.sha256
ok    eaa4842ed00bfc48bcc35948dcfe635ec0d7f472cb59eae05cd3632d1f73412b  goldens/s3m/petri__i16_mono_44100_linear.sha256
ok    bab4413c73d6d4da3681e088af0e761ec50ce7e2ec6fbd9885d28681f4a5e625  goldens/s3m/reflex__i16_mono_44100_linear.sha256
ok    742675f6173bfd64dd486b841f4d7f4603fee6682c77d91d31d6719d914d54a5  goldens/s3m/reflex__i16_mono_44100_cubic.sha256
ok    a1258c1e4e0acfdb440b79d748ed145c11e797d35b469723e6209d90bc109fa8  goldens/s3m/reflex__i16_mono_44100_sinc.sha256

xtask ci: 1 job(s) passed
```

**Not one golden moved.** Nothing in this task touches a rendered byte: `render_pcm_i16_stereo`
is `render_dispatch`'s own match with the sample type and channel count pinned, calling the
same `render_song_with_options`.

### `cargo run -p starplayer-cli -- cast --list`

```
no cast device answered on this network.
That is a normal result, not necessarily a fault: mDNS is multicast, and multicast does not
cross a NAT. Inside WSL2's default networking, inside most containers, and across a VLAN
boundary, nothing will ever answer — and a speaker could not reach a media server bound in
there either. Try a longer --timeout, or run this from a native Linux or Windows build on
the same subnet as the speaker (WSL2 needs mirrored networking).
```

Exit status **0**. This is the expected WSL2 result (research point 4).

### `cargo run -p starplayer-cli -- cast --help`

Prints the long description — what the command does with the audio, the two paths, the
stdin transport letters, the 2–5 s latency figure and why it rules out jam mode, and the
note that `--list` finding nothing is normal — followed by the flag list. Exit status 0.

### `cargo build --release -p starplayer-cli`

| | bytes |
|---|---|
| before this task | 4 606 656 |
| after this task | 10 438 472 |

+5.8 MB, and the shape of it is worth stating: it is `aws-lc-rs`/`aws-lc-sys` (the C crypto
library rustls links, which `rust_cast` would have pulled in whichever way the TLS stack
was assembled), `protobuf` 3.7's generated `cast_channel` module and runtime, `serde_json`,
`mdns-sd` with `mio`/`socket2`/`if-addrs`, `tiny_http`, and `flacenc`. It is the cost of
the CLI speaking a network protocol for the first time; the engine crates are unchanged and
the `no_std` graph is untouched.

### Also verified, not on the list

- `cargo test -p starplayer-cast --test fake_receiver` run eight times in a row after
  fixing one genuine race: the trailing CLOSE a dropped session sends has no reply to wait
  for, so the test now waits for the receiver to record it before snapshotting. Eight
  consecutive green runs, plus the workspace run above.
