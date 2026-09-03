# M3 — D3: One format dispatch in the facade

| Field | Value |
|---|---|
| Milestone | M3 ([master plan](M3-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | E3 (`TrackerProcessor::recommended_voice_capacity`, `MAX_VOICE_CAPACITY`) |
| Blocks | D4 (cpal host), D5 (CLI), F4 (XM wiring), G5 (IT wiring) |
| Parallel with | D6, D7, and the M5/M6 loader and runtime tasks |
| Recommended model | Claude Opus (refactors the wasm host's live audio path and the golden/trace paths) |
| Verified by | agent (`cargo test --workspace`, `cargo xtask goldens --check`, `cargo xtask conformance --offline`, `cargo xtask ci --job wasm-build`, the Node worklet harness), then reviewer, then the owner's listening check in the web player |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there are non-negotiable.

Every host has to turn a loaded `Module` into a playing `PatternSequencer` for the module's
format, and today each does it with its own `match` over `ModuleFormat`:

- `crates/starplayer-host-wasm/src/lib.rs` — the private `NativeSequencer` enum
  (`S3m | Mod | Mtm`), nine methods each a three-arm match, plus the constructor match in
  `source_for`;
- `crates/starplayer/src/lib.rs` — `scan_song` (three arms plus `scan_mod`), `probe`, `load`;
- `crates/starplayer-offline/src/lib.rs` — `playback_source`, `golden_source`,
  `load_golden`, and `trace_loaded`, which bypasses `sequencer_with_quirks` and builds
  `PatternSequencer::new(ExactFixedPoint, XPatternData, XProcessor::new(..), settings)` by
  hand so it can pass `EndOfSongPolicy::Stop`;
- `crates/starplayer-testkit/src/bin/starplayer-conformance.rs` — the `FormatIntegration`
  table of `load_*` / `capture_*` function pairs.

Two more formats are about to arrive concurrently (XM in M5, IT in M6) and two more hosts
(cpal in D4, the CLI in D5). Without this task each of those touches all four sites and
the four streams conflict in the same lines. With it, a new format is one arm in the
facade and a new host is a consumer of one type. `Engine` still stores
`Box<dyn EventSource>` and its command handler still cannot seek — that design is
deliberate (architecture §1.2, §3) and unchanged; the facade enum is the *host-side*
bridge the wasm host already built, made reusable.

### Code you must read before changing anything

- `crates/starplayer/src/lib.rs` — everything; note the feature gating (`s3m`, `mod`,
  `mtm`, `xm`, `it`) and the `probe` ordering test.
- `crates/starplayer-host-wasm/src/lib.rs` — `NativeSequencer`, `SeekableModuleSource`,
  `BuiltSource`, `source_for`, `Host::with_mode` (engine settings), `load_module_with_options`.
- `crates/starplayer-offline/src/lib.rs` — `GoldenFormat`, `playback_source`,
  `golden_source`, `load_golden`, `trace_loaded`, `render_song`, `scanned_song`, and every
  `EngineSettings { voice_capacity: … }` site.
- `crates/starplayer-testkit/src/bin/starplayer-conformance.rs` — `FormatIntegration`,
  `load_*`, `capture_*`, `run_case` (the `GATED` outcome is a feature: a format with no
  entry is a visible gate, not an exclusion).
- `crates/starplayer-engine/src/sequencer.rs` — `PatternSequencer`'s public surface
  (`new`, `seek_row`, `seek_order_at`, `seek_frame`, `restart_clock_at`, `set_timeline`,
  `set_at_end`, `song_frame`, `processor`, `data`, `sample_rate_hz`), `SequencerSettings`,
  `EndOfSongPolicy`; `crates/starplayer-engine/src/source.rs` — `EventSource`.
- `crates/starplayer-{s3m,mod,mtm}/src/processor.rs` — `sequencer_with_quirks`,
  `sequencer_for`, the private `sequencer_settings`, and `XProcessor::new` versus
  `with_quirks` (confirm `new` is `with_quirks(QuirkSelection::FromDialect)`).
- `crates/starplayer-engine/src/timeline.rs` — `scan_timeline`.
- `plans/engine/complete/M3-task-D1-song-timeline-and-loop-detection.md` and
  `M3-task-D2-natural-end-versus-loop.md` — the seek and at-end semantics the enum forwards.

## Deliverables

### 1. `starplayer::NativeSequencer`

In `crates/starplayer/src/lib.rs` (or a `sequencer` module of it), feature-gated per arm:

```rust
pub enum NativeSequencer {
    #[cfg(feature = "s3m")] S3m(PatternSequencer<TempoModelId, S3mProcessor, S3mPatternData>),
    #[cfg(feature = "mod")] Mod(…),
    #[cfg(feature = "mtm")] Mtm(…),
    // xm and it arms arrive with F4 and G5
}

impl NativeSequencer {
    /// The playback sequencer for `module`'s format, built exactly as `sequencer_with_quirks` builds it.
    pub fn new(module: Arc<Module>, sample_rate_hz: u32, quirks: QuirkSelection) -> Result<NativeSequencer, Error>;
    /// The same with explicit settings — what the trace path needs for `EndOfSongPolicy::Stop`.
    pub fn with_settings(module: Arc<Module>, quirks: QuirkSelection, settings: SequencerSettings) -> Result<NativeSequencer, Error>;
    pub fn format(&self) -> ModuleFormat;
    pub fn sample_rate_hz(&self) -> u32;
    pub fn recommended_voice_capacity(&self) -> usize;   // forwards TrackerProcessor::recommended_voice_capacity (E3)
    pub fn seek_row(&mut self, row: u16);
    pub fn seek_order_at(&mut self, order: u16, now: Frame) -> bool;
    pub fn seek_frame(&mut self, song_frame: u64, now: Frame) -> Option<RowMark>;
    pub fn restart_clock_at(&mut self, frame: Frame);
    pub fn set_timeline(&mut self, timeline: SongTimeline);
    pub fn set_at_end(&mut self, at_end: AtEnd);
    pub fn song_frame(&self, now: Frame) -> u64;
    pub fn song_length_frames(&self) -> Option<u64>;
    pub fn end_reached(&self) -> bool;   // the D1/D2 surface the hosts read
}
impl EventSource for NativeSequencer { … }   // next_event_frame / advance_to / dispatch
```

Unsupported formats return `Error::Invalid("no native processor for this module format")`
as `scan_song` does today. The `Result` is what keeps the enum honest when a feature is off.

`scan_song` and `scan_mod_with` build their throwaway sequencers through
`NativeSequencer::new`. Add `pub fn recommended_voice_capacity(module: &Module) -> usize`
as a convenience that builds nothing (it needs the format's constant, so give each format
crate a `pub const` and match on `ModuleFormat` here), documented as the pool size a
per-module host should use, and `pub const MAX_VOICE_CAPACITY` re-exported from the engine
for persistent hosts.

### 2. The wasm host uses it

Delete the private enum and its nine matches; `SeekableModuleSource` wraps
`starplayer::NativeSequencer`. Behaviour is identical: the same command opcodes, the same
seek mailbox, the same telemetry words. `Host::with_mode` keeps the E3 maxima.

### 3. Offline uses it

- `playback_source`, `golden_source`: one `NativeSequencer::new` each. `GoldenFormat` stays
  as the *fixture* enum the golden bin iterates, but its playback dispatch is gone.
- `trace_loaded`: `NativeSequencer::with_settings(module, QuirkSelection::FromDialect,
  settings)` with the existing `EndOfSongPolicy::Stop` settings. Confirm (research point 1)
  that `TempoModelId` resolved from the dialect equals the `ExactFixedPoint` the code
  passes today, so every conformance result is unchanged. `trace_module` becomes the one
  public entry; keep `trace_s3m` / `trace_mod` / `trace_mtm` as thin wrappers if tests use
  them.
- `load_golden` → `starplayer::load`.
- Every `EngineSettings { voice_capacity: channel_count … }` site uses
  `recommended_voice_capacity` (E3 left `// D3` notes at each).

### 4. The conformance bin uses it

`FormatIntegration` stays — the explicit registry and its `GATED` outcome are the point —
but its entries become `starplayer::load` plus `trace_module`, keyed by
`ConformanceFormat → ModuleFormat`. A format the facade cannot build reports `GATED`
exactly as a missing entry does today.

### 5. Documentation

`crates/starplayer/src/lib.rs` crate docs: the host recipe now reads `load` → `Arc::new` →
`scan_song` → `NativeSequencer::new(…, Override(scanned.quirks))` → `set_timeline`.
`plans/product/01-technical-architecture.md` §3.3 gains one line on `NativeSequencer` as
the host-side dispatch. `plans/engine/M3-master-plan.md` lists D3.

## Research points

1. **Trace path equivalence.** `XProcessor::new(module, rate)` versus
   `with_quirks(module, rate, QuirkSelection::FromDialect)`, and `ExactFixedPoint` versus
   `TempoModelId::ExactFixedPoint` through the dialect's `QuirkSet` under the default
   feature set. Prove equivalence by running the conformance suite before and after and
   diffing the per-case output, not by reasoning.
2. **Feature matrix.** The facade must build with any subset of `s3m`/`mod`/`mtm` off
   (`cargo check -p starplayer --no-default-features --features s3m`, and so on) — the enum
   with zero arms included. `no-std-purity` builds the facade last; make sure a
   `#[cfg]`-empty enum does not trip clippy.
3. **What the D4 cpal host and the D5 CLI need.** List the methods they will call so the
   enum's surface is complete now: at least everything the wasm host calls, plus
   `format()` for `info`.

## Verification

```sh
cargo test --workspace
cargo xtask goldens --check                                   # byte-identical
cargo xtask conformance --offline                             # per-case output identical to before
cargo xtask ci --job wasm-build
cargo xtask wasm && node apps/starplayer-web/test/worklet-harness.mjs
cargo xtask ci --job no-std-check
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
cargo check -p starplayer --no-default-features --features s3m,float-mix,linear-interp
```

Then the owner plays `NICETUNE.S3M`, `K-P-K.MOD` and an MTM in the web player: seek by
slider, prev/next order, Repeat on and off — all unchanged.

Report the exact commands run and their results. **Do not commit** — the reviewer commits.

## Out of scope

Making `Engine` handle `Command::Seek*` itself. The cpal host (D4), the CLI (D5), XM and IT
arms (F4, G5). Changing any golden, trace or conformance result.
