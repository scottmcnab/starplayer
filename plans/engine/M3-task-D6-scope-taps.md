# M3 — D6: Telemetry (b) — per-channel scope taps

| Field | Value |
|---|---|
| Milestone | M3 ([master plan](M3-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | M4-lite landed (E1–E3) |
| Blocks | G2 (IT filter — same mixer files; land this first), A1 (TUI oscilloscopes) |
| Parallel with | D3, F1, G1 |
| Recommended model | Claude Opus (touches the render loop; RT path; wasm host and web UI) |
| Verified by | agent (`cargo test --workspace`, `cargo xtask goldens --check`, `cargo xtask ci --job rt-safety`, `cargo xtask wasm` + Node harnesses), then the owner looks at the scopes in the web player |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there are non-negotiable, above all: no allocation, locks or panics in
`render()`, byte-identical output at every host block size, and no change to what the
mixer produces.

Architecture §9 splits telemetry in two. Half (a), the coherent scalar snapshot published
once per tick over an SPSC channel of whole `Snapshot`s, landed in M1-B6
(`crates/starplayer-telemetry`, `crates/starplayer-rt/src/snapshot.rs`). Half (b), the
**lossy audio taps** for per-channel oscilloscopes, is this task: "per-channel fixed
ring; the audio thread stores a `Relaxed` write index; the UI may read torn data. Tearing
is invisible on a scope. Downsample in the audio thread so it ships 32 values per quantum,
not 4096."

### The design constraint that decides where the tap goes

There are **no per-channel buses**. `VoicePool::accumulate_masked`
(`crates/starplayer-mixer/src/voice.rs`) sums every voice into one accumulator in slot
order, and that summation order is what makes the float path's output block-size
independent and what the golden hashes fingerprint. Introducing per-channel accumulation
would change float summation order and break the goldens. So **the tap does not read the
mix at all**. It samples *voice state* at the start of each render segment, which costs
the mixer nothing and cannot perturb it:

- the engine's `render_quantum` (`crates/starplayer-engine/src/engine.rs`) already splits
  each 128-frame quantum into segments at event boundaries and calls `accumulate_masked`
  per segment with a quantum-relative `offset`;
- immediately **before** each segment's accumulation, walk `voices.iter()`; for every
  active voice, for every tap bucket whose first frame `4·b` (for `b` in `0..32`) lies in
  `offset .. offset + span`, read one PCM frame at the voice's position advanced by
  `(4·b − offset)` steps, scale it by the voice's volume, and write it to the ring of
  channel `voice.tag.channel` at bucket `b` of the current quantum;
- a bucket belongs to exactly one segment (the one containing its first frame), so the
  result is a pure function of the quantum and the engine state, never of the host block.

What the tap deliberately ignores: interpolation, gain ramps, pan and the M6 filter.
§9(b) tolerates exactly this — it is a picture, not the audio.

### Code you must read before changing anything

- `crates/starplayer-engine/src/engine.rs` — `render_quantum`, the segment loop,
  `EngineSettings`, `Engine::telemetry_reader`, how `EngineWarnings` reach the snapshot.
- `crates/starplayer-mixer/src/voice.rs` — `Voice` (`position()`, `region()`, `params`,
  `tag`), `VoicePool::iter`; `crates/starplayer-mixer/src/sample.rs` — `SampleRegion`,
  `SampleData::resolve`, `LoopSpan::ping_pong_frame`; `crates/starplayer-mixer/src/kernel.rs`
  — `normalise_position`, `wrap_forward`, `fold_ping_pong` (reuse the same folding for a
  read past a loop end; do not write a second loop rule).
- `crates/starplayer-rt/src/lib.rs` (`pub use portable_atomic as atomic`),
  `crates/starplayer-rt/src/snapshot.rs` — the existing publisher/reader shape and its
  `#![forbid(unsafe_code)]` constraints (architecture §8.1, §9.1).
- `crates/starplayer-telemetry/src/{snapshot,publisher,vu}.rs` — what (a) looks like;
  `crates/starplayer-engine/src/telemetry.rs` — the engine-side glue.
- `crates/starplayer-host-wasm/src/lib.rs` — `telemetry_ptr`/`telemetry_len`,
  `pack_telemetry`, `TELEMETRY_HEADER_WORDS`, `Host::process`, both transports;
  `apps/starplayer-web/www/{worklet-processor.js,ring.js,app.js,index.html,style.css}` —
  how the page reads the snapshot on the SAB path and the `postMessage` fallback, and the
  Channels panel that draws the VU meters.
- `crates/starplayer-engine/tests/block_size_determinism.rs` (never weaken),
  `crates/starplayer-offline/tests/render_allocation.rs` (the allocator hook).
- `plans/product/01-technical-architecture.md` §9, §9.1, Q1 resolution; `plans/apps/A1-master-plan.md`
  (the TUI is the second consumer).

## Deliverables

### 1. `starplayer_rt::tap` — a lossy fixed ring

```rust
pub const TAP_BUCKET_FRAMES: usize = 4;                  // 32 buckets per RENDER_QUANTUM
pub const TAP_RING_BUCKETS: usize = 1024;                // ~93 ms at 44.1 kHz; a power of two
pub struct TapRing { values: Arc<[AtomicI16]>, write_index: Arc<AtomicU32> }   // portable-atomic types
impl TapRing {
    pub fn new() -> (TapWriter, TapReader);              // the only allocation
}
impl TapWriter { pub fn write(&self, bucket: u32, value: i16); pub fn commit(&self, next_bucket: u32); }  // Relaxed stores
impl TapReader { pub fn latest(&self, out: &mut [i16]) -> u32; }   // copies the newest `out.len()` buckets, oldest first; returns the write index it saw
```

Safe Rust only. `Relaxed` everywhere; document that a reader may see a torn window and
that this is intended. One ring per channel, allocated by the engine at construction for
`channel_count` channels, never in `render()`.

### 2. The engine-side tap

`starplayer_engine::ScopeTaps` owned by the engine behind `feature = "telemetry"` (the
same feature as half (a), so a host that wants scopes already has the snapshot). It holds
`Box<[TapWriter]>` and a per-quantum bucket cursor. The sampling rule is the one in the
context section; a muted channel is still tapped (the UI shows the channel's own signal,
which is why muting is a mixer-side discard). The value written is
`pcm_frame * volume`, folded through the voice's region exactly as the kernel would fold
that position (`normalise_position` semantics; a one-shot past its end writes 0). When
several voices share a channel (IT background voices, later), the channel's bucket takes
the **sum, saturating** — deterministic in slot order.

`Engine::scope_readers(&mut self) -> Option<Box<[TapReader]>>` hands the reader halves to
the host once, like `telemetry_reader`.

### 3. The wasm host and the web player

- Expose the rings' storage to the page on the SAB path the way the snapshot is exposed:
  `scope_ptr()`, `scope_len()`, `scope_bucket_frames()`, and the per-channel write index
  words, so the page reads directly out of wasm memory. On the `postMessage` fallback,
  include the newest 256 buckets per active channel in the same batched message the
  snapshot already rides in (the page-side copy; the audio thread allocates nothing).
- `apps/starplayer-web`: an oscilloscope per channel in the Channels panel, drawn from the
  ring at animation-frame rate on a small `<canvas>`, alongside the existing VU bar.
  Responsive; hidden below phone width if it does not fit. Report which transport the
  scopes are using in the Engine panel.
- **VU stays in the snapshot.** B6 said the VU level would move to (b) at M3; it is a
  scalar the snapshot carries at no cost and every consumer already reads it there, so
  moving it would change the 22-word wire header for no benefit. Record this in the task
  file's research resolution and in architecture §9.

### 4. Documentation

Architecture §9(b) becomes a description of what landed (the sampling rule, the bucket
size, the ring capacity, the transport on each path) and notes that the tap is not the mix.

## Research points

1. **Bucket size and ring length.** 4 frames → 32 buckets per quantum matches §9's "32
   per quantum"; 1024 buckets (~93 ms) is enough for a scope drawing at 60 Hz with
   headroom. Confirm the memory cost for 64 channels (64 × 1024 × 2 bytes = 128 KB) is
   acceptable in the worklet's pre-reserved heap (`HEAP_RESERVE_BYTES`).
2. **What to do when a voice's channel is past `channel_count`.** Skip it; the rings are
   sized once.
3. **Fallback transport size.** 256 buckets × active channels × 2 bytes per batched
   message; confirm it is well under what the snapshot batch already carries and that the
   page-side copy is not on the audio thread.
4. **Headless harness.** `apps/starplayer-web/test/headless.mjs` currently fails before
   any of this work ("timed out waiting for the slider to grow by the fade with Repeat
   off"); do not fix it here, but do report whether your change alters the failure.

## Verification

```sh
cargo test --workspace
cargo test -p starplayer-engine --features telemetry
cargo xtask goldens --check                       # byte-identical: the tap never touches the mix
cargo xtask ci --job rt-safety                    # the rings are preallocated
cargo xtask ci --job no-std-check
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
cargo xtask wasm && node apps/starplayer-web/test/worklet-harness.mjs && node apps/starplayer-web/test/ring-harness.mjs
```

New tests: a scripted source triggering one voice on channel 2 with a known PCM ramp
produces exactly the expected 32 bucket values per quantum, identical at host block sizes
1, 3, 64, 128, 4096 and 8191 (add this to `block_size_determinism.rs` under the telemetry
feature); a torn read (writer advancing during `latest`) never panics; two voices on one
channel sum saturating.

Report the exact commands run and their results. **Do not commit** — the reviewer commits.

## Out of scope

Per-channel buses, any change to `accumulate_masked` or `mix_run`, the TUI, moving the VU
level, and the M6 filter.
