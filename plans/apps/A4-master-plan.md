# A4 — Cast to Google Home: StarPlayer on the speakers

| Field | Value |
|---|---|
| Goal | Play a module on a Google Home / Nest speaker or speaker group, chosen from the web player or the CLI, the way Spotify's Cast button does |
| Estimate | 1.5u (N1 spike + N2 + N3 + N4); N5 is +0.25u |
| Depends on | M3 (`starplayer-host::Player`, the CLI), M7-H7 (inserts through `Player`), the web player |
| Trigger | **Pull-driven.** Proposed 2026-09-06 after owner interest; N1 is cheap and settles whether N3/N4 are possible at all |
| Owner prerequisites | A Google Cast Developer account ($5, one-off) and the serial numbers of the speakers to register for development; Chrome as the browser for the web-player sender |

## What "like Spotify" actually means on Cast

Two things are easy to conflate:

- **Spotify Connect** is Spotify's own protocol: the speaker pulls the stream from Spotify's
  servers and the phone is a remote. It needs a cloud service and is not something a
  self-hosted player can copy.
- **Spotify's Cast button** is Google Cast: the phone tells the speaker to launch Spotify's
  *custom Web Receiver app*, which runs on the speaker's own Chromium-based Cast runtime
  and plays from Spotify's servers. The phone can leave; the receiver keeps playing.

Google Cast is what this milestone is about, and it offers exactly two receiver shapes:

1. **The Default Media Receiver** — Google-hosted, needs no registration, plays a media
   URL you give it. Supported audio on Google Home / Nest Audio / Chromecast Audio:
   FLAC (to 96 kHz/24-bit), WAV/LPCM, Opus, Vorbis, MP3, AAC. So *something* on the
   network must serve the audio over HTTP. From the CLI that is natural (we render, we
   serve). From the web player it is not: a browser cannot host an HTTP server, so the
   receiver would have to fetch the module and **render it itself**, which is shape 2.
2. **A custom Web Receiver** — an HTML5/JavaScript app we host over HTTPS and register
   (application id, one-off $5 developer account, each test device registered by serial
   until the app is published). It runs on the speaker. If the speaker's runtime has
   WebAssembly and Web Audio, that app can be **our own wasm host and worklet**: the
   speaker renders the module sample-exactly, the sender is a remote, and the browser
   sender only has to hand over module bytes and transport commands.

Shape 2 is the Spotify-shaped answer and the one the owner asked for ("ideally also from
the web player"). Its one open question is whether Google's speakers expose enough of the
web platform: the audio-device guidance for third-party "Cast for audio" hardware lists
JavaScript ES6, Fetch, WebSocket and MSE and **does not list WebAssembly or Web Audio**,
caps stream buffers at 2 MB, and warns about CPU and memory. Google's own Nest speakers run
a fuller Chromium than the third-party profile, and there is anecdotal evidence of
WebAssembly in receivers, but nothing official. That is why the first task is a probe.

## Decisions (proposed, to be confirmed by N1)

1. **Probe before building.** N1 registers a development receiver whose only job is to
   report what the speaker can do (`WebAssembly`, `AudioContext`, `AudioWorklet`,
   `SharedArrayBuffer`, memory, and whether a 32-voice module renders in real time) and
   play a test tone. Its findings decide whether N3/N4 are the wasm receiver or a
   streaming fallback.
2. **The native path streams; the browser path renders on the speaker.** The CLI has a
   machine behind it, so N2 renders and serves audio to the Default Media Receiver with
   no registration at all — useful on day one, and the fallback if N1 says no. The web
   player uses the custom receiver (N3, N4).
3. **Pre-render by default, live only when asked.** A rendered file with HTTP `Range`
   support gives the speaker seek, duration and a clean end; Cast devices buffer several
   seconds and give the sender no control over it, so a live stream buys nothing for a
   song and rules out jamming anyway. Live streaming (chunked FLAC or WAV) exists for
   "play this endless jam-mode session on the kitchen speaker" and nothing else.
4. **FLAC over WAV on the wire.** WAV/LPCM at 44.1 kHz stereo is 1.4 Mbit/s and works,
   but Google's audio-device guidance caps audio at 2 Mbit/s and FLAC halves it with a
   pure-Rust encoder (`flacenc`, no C). WAV stays as the zero-dependency fallback.
5. **`rust_cast` for CASTv2, `mdns-sd` for discovery.** `rust_cast` 0.21 (2025-12) is
   `rustls`-based, protobuf from the Chromium Open Screen mirror, and already exercises
   `mdns-sd` for discovery; `cast-sender` 0.3 is the alternative on `smol` +
   `native-tls` and is younger. Both are pure Rust. Pin `rust_cast`; revisit if its
   custom-namespace support (N5) proves thin.
6. **The web player switches COEP to `credentialless`.** The page is cross-origin isolated
   (`COOP: same-origin`, `COEP: require-corp`) for `SharedArrayBuffer`. The Cast Web
   Sender SDK is a cross-origin script from `www.gstatic.com` that `require-corp` blocks
   unless it carries CORP headers; `credentialless` keeps isolation (and the SAB) in
   Chrome without demanding headers of Google. Chrome is the only sender browser anyway
   — the Cast SDK is Chrome-only.
7. **Module bytes reach the receiver in chunks over the custom namespace.** Cast custom
   messages are small (research point 3 pins the limit; 64 KB is the working assumption),
   so the sender streams the module in numbered chunks and the receiver reassembles and
   loads. Fetch-by-URL is offered when the page's origin is reachable from the speaker
   (a LAN dev server is; an Internet-hosted page is if it sends CORS/CORP), and chunking
   is the path that always works.
8. **Speaker groups are just devices.** A Google Home speaker group announces itself on
   mDNS like a single device and accepts the same commands; multi-room is free with
   either shape and costs no code beyond listing groups in the picker.

## The task graph

| ID | Task | Depends on | Model | What it delivers |
|---|---|---|---|---|
| N1 | Receiver capability probe | owner registration | Sonnet | `apps/starplayer-cast-probe/`: a registered development Web Receiver that reports the speaker's platform (WebAssembly, `AudioContext`, `AudioWorklet`, SAB, `performance.memory`, user agent), plays a tone through Web Audio, and — if wasm works — runs `starplayer_host_wasm` on a bundled fixture and reports render time per quantum. Results go into this plan as a `## N1 findings` section. Owner does the $5 registration, device registration and the HTTPS hosting (GitHub Pages is enough). |
| N2 | `starplayer-cast` + `starplayer cast` | M3, H7 | Opus | A std crate: `mdns-sd` discovery of `_googlecast._tcp` (devices and groups, with friendly names), `rust_cast` session (connect, launch Default Media Receiver, LOAD, play/pause/seek/stop, volume, status), a minimal HTTP server (`tiny_http`) bound on the LAN interface serving a pre-rendered FLAC/WAV with `Range`, and a live chunked mode over an `AudioBackend` implementation that pushes rendered blocks into the encoder. CLI: `starplayer cast --list`, `starplayer cast --device "Kitchen" song.it [--insert …] [--live] [--format flac\|wav]`, with transport keys while it runs. Tests: an in-process fake Cast receiver (the CASTv2 handshake and media namespace over a local TLS socket) plus the HTTP server's `Range` behaviour; goldens untouched. |
| N3 | The StarPlayer Web Receiver | N1 says yes | Opus | `apps/starplayer-cast-receiver/`: the wasm host and worklet from `apps/starplayer-web` repackaged as a Cast Web Receiver (CAF receiver SDK): custom namespace `urn:x-cast:com.starplayer` for `load` (chunked bytes or URL), `play`/`stop`/`seek`/`mute`/`volume`/`mixer-mode`/`insert`, media-status reporting so the Google Home app and the speaker's own controls show title and position, idle timeout handling, and the audio-device guidance honoured (no images, minimal DOM, one status line). Built by `xtask cast-receiver`, published by the owner to the HTTPS host, registered under the development application id. If N1 says the speaker has Web Audio but no `AudioWorklet`, the receiver falls back to a `ScriptProcessorNode` render path; if no WebAssembly at all, N3 is replaced by N3′: the receiver plays a stream the sender fetches from a `starplayer cast` server, and the web player can only cast when the CLI server is on the LAN (recorded as the honest limit). |
| N4 | Cast button in the web player | N3 | Sonnet | Cast Web Sender SDK loaded from gstatic with `loadCastFramework=1`; COEP switched to `credentialless` in the dev server and documented for deployment; a Cast button in the transport bar (the SDK's `<google-cast-launcher>` element); on connect, the current module is chunked to the receiver, the page's transport, mute, mixer-mode and Effects panels drive the receiver through the custom namespace, and the page's local engine is stopped (one player at a time). The receiver's media status feeds the page's position slider; the scopes and telemetry panels show "casting" rather than fake data. Headless test: a fake receiver in Node asserting the chunk protocol and message sequence. |
| N5 | CLI to the custom receiver | N2, N3 | Sonnet | `starplayer cast --receiver starplayer` launches our receiver instead of the Default Media Receiver and drives it over the custom namespace through `rust_cast`, giving the CLI sample-exact remote playback with no audio on the wire. Optional. |

```
owner registration ── N1 ──→ N3 ──→ N4
M3 + H7 ────────────── N2 ──┴──→ N5
```

N2 has no dependency on N1 and is the part that can be built today.

## Research points (carried into the task files when pulled)

1. **The speaker's platform** (N1): which Chromium version Nest Audio, Nest Mini and Home
   Max run; whether `WebAssembly.instantiate`, `AudioContext`, `AudioWorklet` and
   `SharedArrayBuffer` exist; whether the page is cross-origin isolated (it will not be,
   so the worklet's `postMessage` fallback path is the one that matters); how much CPU a
   64-channel IT costs against the ~2.9 ms quantum budget.
2. **Custom message size limit and throughput** on the Cast custom namespace, and whether
   the receiver may `fetch()` the sender page's origin over HTTP on the LAN (mixed-content
   rules on an HTTPS receiver page block plain-HTTP fetches — which is exactly why chunking
   over the message channel is the default).
3. **Default Media Receiver and live streams**: `streamType: LIVE`, chunked
   transfer-encoding, and whether the speaker accepts a WAV/FLAC stream with no
   `Content-Length` (reports say WAV-on-the-fly works on Chromecast Audio; confirm on Nest).
4. **`rust_cast` maintenance**: 0.21 is recent, but the crate has changed hands before;
   pin it, wrap it behind our own `CastSession` type so `cast-sender` could replace it.
5. **Latency figures**: Cast buffers 2–5 s; measure on the owner's devices and state it in
   the CLI help, because it rules out jam mode over Cast and the plan should say so.

## Exit criteria

From the web player in Chrome, a Cast button lists the owner's speakers and groups; picking
one moves playback of the current module to the speaker with the page acting as the remote
(transport, mute, effects), and closing the tab does not stop the music. From the CLI,
`starplayer cast --device <name> <module>` plays on that speaker or group. Both survive the
speaker's own volume and stop controls.

## Alternatives considered and rejected

- **DLNA/UPnP** — Nest speakers do not implement it.
- **Bluetooth sink** — Nest Audio accepts Bluetooth from a phone; it is not "from the web
  player" and needs nothing from this project.
- **A Spotify-Connect-style cloud** — needs a hosted service and accounts; nothing here
  wants one.
- **Tab casting** — Chrome's "cast this tab" streams the web player's audio to a
  Chromecast today with zero work, at tab-mirroring quality and latency, and stops when
  the tab closes. Worth noting in the README as the no-code option; not the deliverable.

## Sources

- Supported media for Google Cast: https://developers.google.com/cast/docs/media
- Cast for audio devices (receiver constraints): https://developers.google.com/cast/docs/audio
- Web Receiver overview: https://developers.google.com/cast/docs/web_receiver
- Registration (fee, application id, device registration): https://developers.google.com/cast/docs/registration
- Web Sender integration: https://developers.google.com/cast/docs/web_sender/integrate
- COEP `credentialless`: https://developer.chrome.com/blog/coep-credentialless-origin-trial
- `rust_cast` 0.21: https://docs.rs/crate/rust_cast/latest
- `cast-sender` 0.3: https://lib.rs/crates/cast-sender
- `flacenc` (pure-Rust FLAC encoder): https://github.com/yotarok/flacenc-rs/
- Speaker groups: https://support.google.com/googlehome/answer/7174267
