# M3 — D9: Retrofit the wasm host behind `AudioBackend`

| Field | Value |
|---|---|
| Milestone | M3 ([master plan](M3-master-plan.md)); the follow-up the owner deferred from D4 on 2026-09-03 |
| Status | Ready — not yet dispatched; lower priority than the M5/M6 format work |
| Depends on | D4, D6, D8 landed |
| Blocks | Nothing hard; A1 (TUI) benefits from one host lifecycle |
| Parallel with | F2, G3 and their repair tasks |
| Recommended model | Claude Opus (the live browser audio path; both transports must keep working) |
| Verified by | agent (the Node worklet and ring harnesses, `cargo xtask wasm`, the wasm-build job), then reviewer, then the owner's browser check |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine; hosts are `std`. Read
`AGENTS.md` first.

Task D4 shaped `starplayer-host` from the wasm host's lifecycle and implemented it for
cpal. The M3 master plan called the wasm retrofit "the test of whether the abstraction is
honest"; the owner deferred it so cpal and the CLI were not blocked on it. D4 already
moved the host-neutral pieces (`SeekKind`, `SeekRequest`, `SeekableModuleSource`, the
transport ramp, dither and depth quantisation) into `starplayer-host` and the wasm host
imports them. What remains is the other half: the wasm host still builds its own
`WebEngine` arm enum, its own load/scan/set-source recipe and its own telemetry and
scope publication, in parallel with `starplayer_host::Player`.

An AudioWorklet is not a backend that *pulls* a callback the way cpal does: the browser
calls `process()` with exactly 128 frames, on a thread the host does not own, with no
device enumeration and a sample rate fixed by the `AudioContext`. The honest question is
whether `AudioBackend`'s `negotiate`/`open`/`Stream` shape survives that, or whether the
worklet is a second kind of backend the trait must admit — a "push" backend whose
`open` returns the callback for the host to drive. Answer it in code, not in prose.

### Code you must read before changing anything

- `crates/starplayer-host/src/{lib,backend,player,engine,manual,source,transport,depth}.rs`
  and its tests; `crates/starplayer-host-cpal/src/lib.rs`.
- `crates/starplayer-host-wasm/src/{lib,command}.rs` — all of it; `apps/starplayer-web/www/{worklet-processor.js,ring.js,app.js}`,
  `apps/starplayer-web/test/{worklet-harness,ring-harness,headless}.mjs`.
- `plans/engine/complete/M3-task-D4-cpal-host.md` research resolution 1 (what was
  moved and what was left).
- `plans/product/01-technical-architecture.md` §9.1–§9.3, §11.

## Deliverables

1. `starplayer-host-wasm` implements `AudioBackend` (or the push-shaped sibling this task
   introduces, with a paragraph in the crate docs justifying it) and drives
   `starplayer_host::Player` for everything that is not wasm-specific: engine construction
   at the maxima, module activation, seeks, transport, telemetry snapshot access, garbage
   collection. What stays in the wasm crate: the wire command decoding, the
   `SharedArrayBuffer`/`postMessage` transports, the scope window copy, the planar output
   buffer, and memory pre-reservation.
2. Behaviour is identical: the same opcodes, the same 22-word telemetry header, the same
   scope protocol, the same fallback behaviour. The Node harnesses are the proof, and the
   worklet must still never allocate in `process()` on the shared-memory path.
3. `starplayer-host`'s docs describe both backends; architecture §11 says the wasm host
   depends on `starplayer-host`.

## Research points

1. **Pull versus push.** Decide whether the worklet fits `AudioBackend::open(callback)`
   (the worklet's `process` calls the stored callback) or needs a second trait; record the
   reasoning and the shape chosen.
2. **`HEAP_RESERVE_BYTES`.** `Player` allocates its engine and rings; confirm the reserve
   still covers the worst case with 64 channels and 256 voices.
3. **`headless.mjs`** fails before this task ("timed out waiting for the slider to grow by
   the fade with Repeat off"). Investigate whether it is the harness or the page, since this
   task touches the page's host; fix it if it is cheap and in scope, else record what you
   found.

## Verification

```sh
cargo test -p starplayer-host -p starplayer-host-wasm
cargo xtask ci --job wasm-build
cargo xtask wasm && node apps/starplayer-web/test/worklet-harness.mjs && node apps/starplayer-web/test/ring-harness.mjs && node apps/starplayer-web/test/headless.mjs
cargo test --workspace
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
```

Report the exact commands and results. **Do not commit** — the reviewer commits.

## Out of scope

Changing the wire protocol. The TUI. MIDI.

## Research resolution

### 1. Pull versus push — **one trait, unchanged; the worklet is a push backend and `AudioBackend` already admitted one**

The premise of the question is that `AudioBackend` was written for a backend that *pulls*:
cpal owns a thread and calls the render callback from it, while an AudioWorklet is *pushed* —
the browser calls `process()` on a thread nobody here owns, with exactly 128 frames, and this
crate's only entry point is a `process(frames)` export that JavaScript invokes.

Read carefully, that difference is not in the trait. **`AudioBackend::open` never promised to
start a thread.** It promised to take ownership of a `RenderCallback` and call it; which clock
does the calling is the backend's own business and appears nowhere in the signature. The proof
was already in the crate before this task: `ManualBackend` is a push backend whose clock is the
caller, and D4 shipped it as the abstraction's second implementation. `WorkletBackend` is
`ManualBackend` with a browser for a clock.

So the shape chosen is: `crates/starplayer-host-wasm/src/backend.rs` implements the existing
`AudioBackend`. `open` stores the callback in a plain field and returns a `Stream` whose
`StreamControl` shares only an `Arc<AtomicBool>`; `WorkletBackend::render(&mut [f32])` is what
`Host::process` calls, and it either invokes the stored callback or fills silence. The `Host`
holds the backend beside the `Player`, because a `Player` owns a `Stream`, not a backend.

Three findings from doing it in code rather than in prose:

* **`negotiate` is where the worklet actually differs, and it is the half a second trait would
  have had to duplicate.** An `AudioContext` is constructed at a sample rate and cannot be
  renegotiated, so `negotiate` answers with the context's rate whatever was asked for — exactly
  the "the device answers, and the answer is what the engine and the scan are built from"
  contract §4.1 needs, and exactly what the pre-D9 host was doing by hand with a
  `sample_rate_hz` field it carried for the purpose. The block size is not a preference here
  either: `RENDER_QUANTUM` is a promise the browser keeps, so it comes back named.
* **A push-shaped sibling trait whose `open` *returned* the callback buys one thing and costs
  two.** It buys not having to keep the backend alive alongside the `Player`. It costs a second
  lifecycle for every host to implement — which is the "do not leave two parallel lifecycles"
  the task forbids — and a duplicated `negotiate`, since the wasm backend's whole platform
  contribution lives there. Rejected.
* **No lock on the render path.** `ManualBackend` puts the callback behind a `Mutex`, which is
  honest for a test harness driven from an arbitrary thread. A worklet's `process()` *is* the
  audio realm, so `WorkletBackend` stores the callback in a plain `Option<RenderCallback>`
  field: no lock, no `RefCell`, no allocation. Only the play/pause flag is shared, and it is a
  relaxed atomic.

**What that deleted from `starplayer-host-wasm`** (`lib.rs` 1497 lines → 1182, of which the
tests grew; the new `backend.rs` is 176 lines including its own tests):

| gone | replaced by |
|---|---|
| `WebEngine`, `build_engine_arm`, `render_web_arm!`, `define_web_engine!` — eight typed arms | `starplayer_host::HostEngine`, inside `Player`'s callback |
| `transport_gain`, `pending_engine_stop`, `pending_end_rewind`, `fading`, `fade_elapsed`, `fade_frames`, `faded_gain`, `begin_transport_stop`, `set_at_end`'s fade arithmetic | `starplayer_host::Transport` |
| `BuiltSource`, `source_for`, `scan_for`, `seek_request`, `at_end`, `request_seek` | `Player::{load_module, seek_order, seek_row, seek_frame, set_at_end, set_fade_frames}` |
| the end-of-song arming block inside `process` | `RenderState::arm_end_of_song` |
| `Host::set_mixer_mode`'s rebuild recipe (60 lines: settings, sounding position, master volume, mutes, was-playing, scope readers) | `Player::set_mixer_mode`, plus five lines here that put the mutes back |
| `Host::decode` of `WireCommand` → `starplayer_core::Command` | one call on `Player` per opcode |
| `float_interleaved`, `fixed_interleaved`, `quantized_i16` | one `interleaved` buffer the callback writes |
| `control`, `telemetry`, `current_module`, `song_scan`, `active_mode`, `commands_rejected`(engine half) | `Player`'s own surface |

**What stayed**, exactly as the deliverable says: `command.rs` and the opcode block, the 22-word
telemetry packing, the scope window copy and its refresh cadence, the planar output buffer, the
`thread_local!` host and the `#[wasm_bindgen]` exports (every signature unchanged), and
`HEAP_RESERVE_BYTES`. The only behavioural addition is `Player`'s: `Player::open` leaves the
transport silent, so `Host::with_mode` now calls `player.play()` — one 64-frame glide up where
the old host started flat at unity, which is one click fewer and nothing else. The Node worklet
harness, which renders ten seconds of REFLEX with no explicit Play, reports the same peak
(0.731) before and after.

**Two real bugs surfaced by the retrofit, both in `starplayer-host` and both now fixed and
tested there:**

1. **A landed fade re-armed itself.** The engine applies its command ring inside its own
   render, so for one whole quantum after `settle_transport` sends `Command::Stop` the engine
   still reports itself playing. `arm_end_of_song` therefore fired a second time on a song
   that had just finished fading, re-arming the fade over silence. The pre-D9 wasm host did not
   have this because it took the arming decision *after* rendering, where the engine had
   already stopped — but that ordering is the one D4 deliberately replaced, because deciding
   after the block makes the decision land on a different frame at every block size. The fix is
   a latch (`RenderState::awaiting_engine_stop`) rather than a reordering, so block-size
   independence is untouched. Regression test:
   `a_fade_that_has_landed_does_not_re_arm_itself_over_the_silence`.
2. **`Player` published a stale snapshot.** `RenderState::publish` forwarded `last_snapshot`,
   which `arm_end_of_song` had read at the *start* of the callback — contradicting the comment
   directly above it ("One publication per callback, carrying the newest state"). On a 128-frame
   worklet quantum that is a whole quantum of lag, and it made the wire header report a song
   length of zero for the first quantum after a load. `publish` now reads the engine's telemetry
   again, so the arming cadence stays fixed while the reading a caller gets is as new as the
   engine can make it.

### 2. `HEAP_RESERVE_BYTES` — **still covers it, with ~14 MiB to spare; the retrofit costs exactly one 64 KiB wasm page**

Measured on the packaged bundle in Node, at 64 channels and 256 voices (`ChannelTable::MAX_CHANNELS`
and `MAX_VOICE_CAPACITY`, which is what `Player::open` builds whatever is loaded), by temporarily
setting the reserve to 4 KiB so the reported `WebAssembly.Memory` size *is* the host's own
footprint:

| | pre-D9 | with `Player` |
|---|---|---|
| after `init` (64 ch, 256 voices, stereo float) | 1 376 256 B (1.31 MiB) | 1 441 792 B (1.38 MiB) |
| after loading MOVEMENT.S3M (58 499 B) | 2 031 616 B | 2 097 152 B (2.00 MiB) |
| after 5 000 quanta and four mixer-mode rebuilds | 2 031 616 B | 2 097 152 B |

So the whole retrofit costs **65 536 bytes — one wasm page**, which is `HostEngine`'s
`MAX_FRAMES_PER_RENDER` (4 096-frame) scratch pair replacing the worklet's 1 024-frame buffers.
The worklet only ever asks for 128 frames, so that scratch is over-provisioned here; it is left
as it is rather than made a parameter, because 64 KiB is not worth a knob and the same
`HostEngine` then serves a cpal device that really does hand over 96 000 frames at once.

With the reserve at its shipped 16 MiB, `init` reports 17 956 864 B and **nothing grows
thereafter** — not a module load, not 5 000 quanta, not a rebuild through all four of
`0x0202`/`0x0221`/`0x0102`/`0x0261`; the worklet's own `fault` message (which fires on any growth
inside `process`) never appears. The live host uses ~2 MiB of the 16 MiB reserve, leaving about
14 MiB of headroom for the module blob itself, so a module would have to be well past 10 MB
before the reserve stopped covering it. Left at 16 MiB.

### 3. `headless.mjs` — **the harness, not the page; stale since task D2, and fixed here**

Reproduced first, on a clean checkout of `d9` at 965fa89, before any change:

```
Error: timed out waiting for the slider to grow by the fade with Repeat off
  page says: Playing Reflex; the retired module Arc returned off the audio callback.
  console: quiet
```

The page is right and the assertion is wrong. `app.js`'s `updateProgress` adds the fade to the
displayed length only when `!repeat.checked && (songFlags & SONG_FLAG_LOOPS)`, and task D2 made
`SONG_FLAG_LOOPS` mean "the song comes round through its own flow — a `Bxx`/`Cxx`/`Dxx` jump
back into music already played", not merely "the order list ran out". The fixture is REFLEX.S3M,
whose order list runs out: `EndReason::Ended`, so the flag is clear, so there is no fade to add,
so `progress.max` never grows and the wait can only time out. The assertion was correct before
D2 and has been stale since; nothing in the host or the page is broken, and D9 does not change
either side of it.

Fixed in the harness, which was cheap and in scope: the block now asserts that with Repeat off a
song that *ends* keeps its one-pass slider and displayed length, then seeks a second from the end
and keeps the existing assertions that the transport stops, `elapsed` reads `0:00` and the slider
rewinds to 0 — which is exactly D2's behaviour, and the thing worth testing. `app.js`'s own
comment already explains the rule, so the harness now quotes it rather than contradicting it.
