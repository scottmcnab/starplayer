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
