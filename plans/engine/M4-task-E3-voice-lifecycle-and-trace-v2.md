# M4 — E3: Voice lifecycle hooks and trace format v2

| Field | Value |
|---|---|
| Milestone | M4-lite ([master plan](M4-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | M2 complete. Independent of E1/E2 in code; land after them to keep the merge order simple |
| Blocks | D3 (facade dispatch uses the capacity API), F2 (XM runtime), G3, G5 (IT runtime and wiring) |
| Parallel with | E1, E2, D6, D7 |
| Recommended model | Claude Opus (touches the voice pool, the channel table and the trace contract; RT path) |
| Verified by | agent (`cargo test --workspace`, `cargo xtask goldens --check`, `cargo xtask ci --job rt-safety`, `--job trace-zero-cost`, `--job conformance --offline`), then reviewer |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there are non-negotiable, above all: no allocation, locks or panics in
`render()`, and byte-identical output at every host block size.

XM (M5) and IT (M6) are about to be implemented concurrently on top of the engine. The
owner decided (2026-09-03, [concurrency plan](M3-M6-concurrency-plan.md)) that there is
**no shared `Instrument` trait yet**: each format keeps its own per-voice articulation
state — envelope positions, fadeout level, key-off flag, auto-vibrato phase — in a
parallel array indexed by `VoiceId::index()` and validated by `generation`, and advances it
from inside its own `TrackerProcessor::tick()` by walking the voice pool. IT additionally
*detaches* a channel's foreground voice into the background (New Note Actions), where it
keeps sounding under its own envelopes with no channel owning it.

The engine and mixer need four small, format-neutral changes for that design to work, and
the per-tick trace needs to be able to show a voice that no channel owns. That is this
task. It adds **no behaviour to any existing format**: goldens, traces and conformance
results are byte-identical afterwards except for the trace version header.

### Why the parallel-array design is sound (verified; do not redesign it)

- `TickContext.voices` is `&mut VoicePool`, so a processor's `tick()` can reach every
  active slot, owned or not. Only a *mutable* walk is missing.
- Every release path — `release`, `release_all`, and the `Finished` path inside
  `accumulate_masked` — bumps the slot's generation **before** the slot is reused, and
  `allocate` hands out `(index, current_generation)`. A format that stores the `VoiceId`
  it was given therefore never matches a later occupant of the same slot.
- A slot the format did not allocate (a future MIDI sample player sharing the pool, a
  scripted test source) shows an active voice whose stored id does not match: the format
  skips it, never adopts it.
- The per-tick envelope write goes straight to `voice.params` (`set_volume` and friends),
  not through `TickContext::write_voice_param`, so the trace's per-tick dirty flags stay
  what they are today: effect-driven writes only.

### Code you must read before changing anything

- `crates/starplayer-mixer/src/voice.rs` — `VoiceTag`, `Voice`, `VoiceSlot`, `VoicePool`
  (`allocate`, `release`, `release_all`, `get`, `get_mut`, `iter`, `accumulate_masked`,
  `live_slot_mut`).
- `crates/starplayer-engine/src/channel.rs` — `Channel`, `ChannelTable` (`trigger`
  releases the previous foreground *before* allocating; `stop`, `release_finished`).
- `crates/starplayer-engine/src/sequencer.rs` — `TickContext` (`trigger_channel`,
  `stop_channel`, `queue_channel_region`, `write_voice_param`, `mark_voice_dirty`,
  `report_trace_channel`), `TrackerProcessor`, `PatternSequencer::new`, `SequencerSettings`.
- `crates/starplayer-engine/src/engine.rs` — `EngineSettings` (`voice_capacity`,
  `channel_count`), `render_quantum`.
- `crates/starplayer-engine/src/timeline.rs` — `scan_timeline` sizes its own throwaway
  `VoicePool` from `channel_count`.
- `crates/starplayer-engine/src/trace.rs` — the v1 text contract in the module docs,
  `TraceRecorder` (`begin_tick`, `report_channel`, `record_voice_flags`,
  `record_channel_flags`, `finish_tick`), `TraceTick`, `TraceChannel`.
- `crates/starplayer-testkit/src/lib.rs` — `parse_trace`, `parse_channel`, `diff_traces`,
  `TraceField`; `crates/starplayer-testkit/src/conformance.rs` — how the libxmp adapter
  consumes `TraceTick.channels` (it must be unaffected by the new voice lines).
- Every `EngineSettings { voice_capacity: … }` site: `crates/starplayer-offline/src/lib.rs`
  (four), `crates/starplayer-host-wasm/src/lib.rs` (`with_mode`, hard-coded 64 voices and
  **32 channels**), and the tests under `crates/starplayer-engine/tests/`.
- `plans/product/01-technical-architecture.md` §5.1–§5.3, §2.2, §8.

## Deliverables

### 1. `VoicePool::iter_mut`

```rust
pub fn iter_mut(&mut self) -> impl Iterator<Item = (VoiceId, &mut Voice)>
```

Slot order, active slots only, same id construction as `iter`. No allocation. Document
that a format walks this once per tick to advance its parallel articulation state.

### 2. `ChannelTable::detach_foreground` and `TickContext::detach_channel`

```rust
impl ChannelTable {
    /// Unbind `channel`'s foreground voice without touching it: it keeps sounding, keeps its tag (including `tag.channel`), and now belongs to nobody. IT's New Note Actions `Continue`, `NoteOff` and `NoteFade` start here.
    pub fn detach_foreground(&mut self, channel: ChannelId) -> Option<VoiceId>;
}
impl TickContext<'_> {
    pub fn detach_channel(&mut self, channel: ChannelId) -> Option<VoiceId>;  // records nothing in the trace: no parameter changed
}
```

Document the ordering IT must use — detach, then `trigger_channel` — because `trigger`
releases whatever foreground it finds.

### 3. `VoiceTag.sample: u16`

IT's Duplicate Check compares sample numbers and must not rely on a clamped `u8`. Widen
the field; update the S3M processor's clamp, the trace fallback (already `as u16`), and
the `VoiceTag` doc comment ("four bytes" becomes five, padded to six).

### 4. Single-sourced voice capacity

- `pub const MAX_VOICE_CAPACITY: usize = 256` in `starplayer-engine` (IT's virtual channel
  limit; the widest any in-scope format needs), documented next to
  `ChannelTable::MAX_CHANNELS`.
- `TrackerProcessor::recommended_voice_capacity(&self) -> usize` with a default returning
  the processor's channel count (S3M, MOD and MTM keep the default). A format that keeps a
  parallel per-voice array sizes both the array and this answer from one constant, so the
  pool and the array agree. Document that a pool *larger* than the answer is legal — the
  format must use `get_mut` and skip ids past its array — and a pool *smaller* is legal
  too, just fewer voices.
- `scan_timeline` sizes its pool from the processor's answer, not `channel_count`.
- Persistent hosts that build the engine before any module is loaded use the maxima: the
  wasm host's `with_mode` becomes `voice_capacity: MAX_VOICE_CAPACITY, channel_count:
  ChannelTable::MAX_CHANNELS`. Confirm (research point 1) that this changes no output —
  only pool size and channel-table length — so the web goldens and the Node harness pass.
- Offline's per-module sites switch to the processor's answer in D3, which introduces the
  facade enum; here, leave them at `channel_count` and add a `// D3` note.

### 5. Trace format v2

Bump `TRACE_FORMAT_VERSION` to 2. After the ` ch=` lines of a tick, emit one line per active
voice that is **no channel's foreground**, in slot order:

```text
 vc=017 root=03 note=C-5 ins=01 smp=0001 vol=32 per=001712 pan=128 pos=0000001234.91a2b3c4 cut=255 res=000 fl=--
```

`root` is `tag.channel`. The fields and their encodings are those of a ` ch=` line minus
`act` (a listed voice is active by definition); `smp` is four digits now that the tag is
`u16` — widen it on ` ch=` lines too. A tick with no background voices emits none, so
MOD/S3M/MTM traces differ from v1 only in the header.

- `TickContext::report_trace_voice(&mut self, voice: VoiceId, state: TraceChannelState)`
  lets a format report native state for a background voice, mirroring
  `report_trace_channel`; unreported voices fall back to the tag and `params` as channels
  do.
- `record_voice_flags` attributes a write to the ` ch=` row only when the voice **is** that
  channel's foreground; otherwise to the voice's own row.
- `TraceTick` gains `voices: Vec<TraceVoice>`; `parse_trace` accepts ` vc=` lines;
  `diff_traces` compares voice rows after channel rows and reports them as
  `TraceField::Voices` (one field, not per-attribute — refine when G5 needs it). The libxmp
  adapter in `conformance.rs` reads `channels` only and is unaffected.
- `cargo xtask trace` output and the committed expected-trace fixtures, if any, are
  regenerated (research point 2).

### 6. Documentation

`plans/product/01-technical-architecture.md`: §5.3 records the owner's decision (shared
machinery, format-owned per-voice state, `Instrument` trait deferred to M4-full with the
MIDI sample player), §5.4 notes that the tracker tick is the control tick and nothing else
consumes `ControlClock` yet, §2.2 mentions trace v2's voice lines. `crates/starplayer-engine/src/trace.rs`
module docs describe v2.

## Research points

1. **Does a bigger channel table or pool change output?** `ChannelTable::new` and
   `VoicePool::new` allocate once; the mixer walks active slots only. Confirm with the
   block-size determinism test and the wasm Node harness that 64/256 versus 32/64 leaves
   every rendered byte identical, and note the memory cost of 256 `VoiceSlot`s.
2. **Which tests pin the trace text?** Find every test comparing against a literal
   `starplayer-trace v=1` header or a stored trace file and update it. The
   `trace-zero-cost` CI job must still prove the hook compiles to nothing without the
   feature.
3. **`VoiceTag` layout.** After widening `sample`, check `size_of::<Voice>()` did not
   cross a cache-line-relevant boundary; it is fine if it did, but record the number.

## Verification

```sh
cargo test --workspace
cargo xtask goldens --check                        # byte-identical
cargo xtask conformance --offline                  # identical results (33 of 47)
cargo xtask ci --job rt-safety
cargo xtask ci --job trace-zero-cost
cargo xtask ci --job no-std-check
cargo xtask ci --job clippy
cargo xtask wasm && node apps/starplayer-web/test/worklet-harness.mjs   # or the harness the wasm-build job runs
```

New tests: `iter_mut` visits exactly the active slots in order; `detach_foreground` leaves
the voice sounding with `tag.channel` intact and `is_sounding` false; a scripted source
that detaches a voice produces a ` vc=` line that `parse_trace` round-trips; a parameter
write to a detached voice lands on its ` vc=` row, not on the channel's;
`recommended_voice_capacity` default equals the channel count; `scan_timeline` on the
synthetic MOD/MTM fixtures and the five S3Ms is unchanged.

Report the exact commands run and their results. **Do not commit** — the reviewer commits.

## Out of scope

Envelopes, NNA, DCT, stealing, the filter, any change to `mix_run` or `MixPath`. A
ramp-preserving `VoicePool::replace` for stealing (deferred until the IT corpus shows the
click matters). The `Instrument` trait. The facade enum and the offline capacity sites (D3).
