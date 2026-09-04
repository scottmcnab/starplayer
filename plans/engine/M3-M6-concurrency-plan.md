# M3 ∥ M4-lite → M5 ∥ M6 — concurrency plan and dependency graph

| Field | Value |
|---|---|
| Scope | The rest of M3, a cut-down M4 ("M4-lite"), all of M5 (XM) and all of M6 (IT), run concurrently |
| Decided | 2026-09-03, by the owner: shared *machinery* rather than a shared `Instrument` trait; the wasm retrofit behind the M3 host trait is deferred |
| Depends on | M2 complete (owner accepted 2026-09-03) |
| Produces | Four prerequisite task files (E1–E3, D3) now; M3/M5/M6 task files once those land |

This is a cross-milestone plan, not a task file. It records what the code provides today,
which deliverables are prerequisites for others, what can run concurrently, and the shared
files where the streams will collide. The master plans for M3–M6 still describe *what*;
this describes *in what order*.

## Status (2026-09-04)

Landed on `main`, each as a `--no-ff` merge with its task file archived: E1, E2, E3 (M4-lite,
complete); D3, D4, D5, D6, D7, D8 (M3 complete pending the owner's listening and scope
checks); F1, F2, F5 (XM: 87 of 93 conformance cases pass, 4 sharpened known failures); G1,
G2, G3, G6 (IT: 55 of 121 pass, 66 regrouped as `G6-IT-*` with the evidence that would
settle them). Whole corpus: 175 of 261 pass, 0 fail, 15 accepted deviations.
D9 (the wasm host behind `AudioBackend`) landed the same day, so both hosts share one
lifecycle and the headless web harness passes again. F6 (the trace path's order-list wrap) landed too, taking XM to 88 of 93 and the corpus to
176 of 261 with 0 failures. M4-full is planned as E4–E7 (see the M4 master plan) and parked with the owner. Then the
owner's acceptance of M3, M5 and M6, and the owner's call on `TempoModel::ItModern`
(accuracy policy §2).

## Task letters

Task ids follow the repository convention (M0 = A, M1 = B, M2 = C, M3 = D): **M4 = E**,
**M5 = F**, **M6 = G**. The prerequisite wave is therefore `E1`–`E3` (M4-lite) and `D3` (M3).

## What exists today (verified in code, 2026-09-03)

| Area | State |
|---|---|
| Format seam | `PatternData` + `TrackerProcessor` (`crates/starplayer-engine/src/sequencer.rs`), generic not dyn. Every format is `XPatternData(Arc<Module>)`, `XProcessor`, `sequencer_for` / `sequencer_with_quirks`. S3M's `flush_channel` (`crates/starplayer-s3m/src/processor.rs`) is where channel state becomes voice writes |
| Instrument model | `InstrumentDef` carries `note_sample_map: [u8; 120]`, vol/pan/pitch `Envelope`, `fadeout`, NNA/DCT/DCA — declared, never read. `SampleSpec` lacks every XM/IT per-sample field. `EnvelopePoint.value` is `u16`; IT pan and pitch envelopes are signed |
| Ping-pong | The kernel implements ping-pong; `ModuleBuilder::add_sample` gives PingPong zeroed guard frames, disagreeing with the mixer's `append_guarded_sample` |
| Voices | One global generational `VoicePool`: `iter` (immutable only), `get_mut`, `release` (hard cut); every release path bumps the generation before reuse. `VoiceTag.sample` is `u8`. `Channel { foreground, muted }` only; `ChannelTable::trigger` releases the previous foreground before allocating |
| Filter | `VoiceParams.filter` is writable; kernel, `MixPath` and `starplayer-dsp` ignore it |
| Frequency | Period→`Step` is per format crate; no linear-frequency table anywhere; `ModuleFlags::linear_slides` is unread |
| Control clock | `ControlClock` exists; nothing advances on it |
| Tempo | `TempoModel::ItModern` behaves as `ExactFixedPoint`, pinned by a test |
| Voice capacity | Every host sizes the pool to `channel_count`; the wasm host hard-codes 64 voices and **32 channels**, so an IT's channels 32–63 would be silently dropped |
| Hosts | wasm is the only real host; its private `NativeSequencer` enum is the format dispatch. `starplayer-host-cpal`, `starplayer-cli`, `starplayer-tui` are stubs. No WAV writer |
| Trace | v1 rows are `0..channel_count` from `channels.foreground`; a voice no channel owns is invisible |
| Telemetry | Snapshot (a) landed; no scope rings (b). No per-channel buses: `accumulate_masked` sums every voice into one accumulator |
| Harness | `ConformanceFormat {Mod, S3m, Mtm}` with per-format projection arms; the libxmp pin contains XM/IT fixtures and dumps, not yet surveyed |

## The central decision: what "just enough Instrument abstraction" means

**Commit shared machinery, not a shared trait; per-voice articulation state is
format-owned.**

- Architecture §5.3's `trait Instrument` has one consumer that cannot call concrete code: a
  MIDI-driven sample player. XM and IT processors are tracker code that already write voice
  parameters through `TickContext`. The trait is extracted in M4-full, when the MIDI sample
  player gives it its second real implementation, which keeps §10.1 intact.
- Envelope, fadeout and NNA state must outlive the channel (IT background voices), so it is
  per voice. XM and IT envelope semantics differ — sustain point versus sustain loop, carry,
  pitch-versus-filter envelope, IT's per-sample auto-vibrato — and a shared engine envelope
  runner is exactly the cross-format branch §4 warns about. So each format keeps a parallel
  `Box<[XmVoiceState]>` / `Box<[ItVoiceState]>` sized to the pool, indexed by
  `VoiceId::index()` and validated by `generation`, allocated once in the constructor,
  cleared in `reset()`, and advanced inside its own `TrackerProcessor::tick()` by walking
  `context.voices`. A slot the format did not allocate is skipped, never adopted. Envelope
  output is written straight to `voice.params`, so `write_voice_param` stays the
  effect-driven, traced path.
- The engine never runs an envelope; `ControlClock` stays unconsumed until M4-full.

## Prerequisite wave — lands on `main` before M5/M6 branch

Land directly on `main` via `git merge --no-ff`. E1 and E2 first (no dependents between
them), then E3, then D3.

| Task | Crates | Deliverable | Unblocks |
|---|---|---|---|
| [E1](complete/M4-task-E1-instrument-and-sample-model.md) | model, core | The XM+IT instrument and sample model, designed against both specs at once: raw `relative_note`/`finetune`, sample default pan, per-sample auto-vibrato, **sample sustain loops**, a `u16` note→sample map plus IT's note transpose map, signed envelope values, IT instrument fields (global volume, pan, pitch-pan, random variation, initial filter, filter-envelope flag), per-channel default volumes and an opaque `format_data` blob, ping-pong guard frames, and the shared xorshift32 RNG | F1, F2, G1, G3 |
| [E2](complete/M4-task-E2-linear-frequency-and-format-dialects.md) | core | The `2^(n/768)` linear-frequency table, and the XM/IT `FormatDialect` variants (no new `QuirkSet` fields until a corpus case names one) | F2, G3 |
| [E3](complete/M4-task-E3-voice-lifecycle-and-trace-v2.md) | engine, mixer, testkit | `VoicePool::iter_mut`, `ChannelTable::detach_foreground`, `VoiceTag.sample: u16`, single-sourced voice capacity (`TrackerProcessor::recommended_voice_capacity`, `MAX_VOICE_CAPACITY`), and **trace format v2** with per-voice lines for voices no channel owns | F2, G3, G5 |
| [D3](complete/M3-task-D3-facade-format-dispatch.md) | facade, host-wasm, offline, testkit | One `NativeSequencer` in the facade, lifted from the wasm host; every host, the offline renderer, the trace path and the conformance bin dispatch through it, and honour the format's voice capacity | D4, D5, F4, G5 |

## M3 — remaining deliverables

| Task | Deliverable | Depends on | Notes |
|---|---|---|---|
| D3 | facade dispatch (above) | E3 | The only M3 task M5/M6 wait for |
| D4 | Host abstraction + `starplayer-host-cpal` | D3 | Shape the trait from the wasm host's lifecycle; implement it for cpal. **The wasm retrofit is a separate follow-up task** (owner decision) so cpal is not blocked on it |
| D5 | `starplayer-cli` (`play`, `render`, `info`, `trace`) + WAV writer in `starplayer-offline` | D3; D4 for `play` | The WAV writer is its own small task so `render` does not wait on cpal |
| D6 | Telemetry (b): per-channel scope rings, VU moves out of the snapshot | — | **Do not tap the mix.** Per-channel buses would change float summation order and break the goldens. Sample voice state at each segment start — `pcm[position + k·16·step]` scaled by `current_gain_units()` per 16-frame bucket, keyed by `tag.channel` — into `Arc<[AtomicU32]>` rings via `starplayer_rt::atomic`. No mixer change, output unaffected, ignores interpolation, ramps and the M6 filter, which architecture §9(b) tolerates. Keeps `kernel.rs` free for G2 |
| D7 | T10 perceptual comparison vs libopenmpt, nightly | — | System `libopenmpt` on the runner plus std-only bindings in testkit. Independent, and libopenmpt is the XM/IT accuracy reference, so it is M5/M6's second oracle: **schedule early** |

Engine `Command::Seek*` stay flagged unsupported: cpal and the CLI own their sequencer the
way the worklet does and seek through the D3 enum.

## M4-lite — scope

In: E1, E2, E3, and an architecture amendment (§5.3, §5.4, §10.1) recording that per-voice
articulation is format-owned, the trait extraction moves to M4-full, and the tracker tick
is the control tick with no engine-side consumer yet. No `ModInstrument` /
`S3mInstrument` / `MtmInstrument` wrappers: they would be implementations of a trait that
does not exist.

Deferred to M4-full: `starplayer-midi` codec and SMF parser, `SmfSequencer`,
`ExternalEventQueue`, live MIDI (midir, Web MIDI), the computer-keyboard map, the
`SourceMux` acceptance case, synthesised control-tick consumers, and `trait Instrument`
with the MIDI sample player as its non-tracker implementation.

## M5 — XM task graph

| Task | Deliverable | Depends on | Parallel with |
|---|---|---|---|
| F1 | Loader: header, packed patterns unpacked to a fixed-stride cell layout in the blob (`row_bytes` must return one slice per row), instruments with 96-entry maps resolved to global `SampleId`s, delta 8/16-bit, ping-pong, zero-sample instruments; fuzz targets and seeds | E1 | F2 |
| F2 | Instrument runtime inside `starplayer-xm`: note→sample map, linear (E2) and Amiga (its own FT2 table) pitch, volume/pan envelopes with sustain point and loop, key-off and fadeout, auto-vibrato with sweep — the parallel per-voice state array | E1, E2, E3 | F1 |
| F3 | Effect set and volume column, FT2 quirks from the OpenMPT wiki; `XmProcessor: TrackerProcessor` | F2 | — |
| F4 | Wiring: facade arm, `GoldenFormat::Xm`, `ConformanceFormat::Xm` and projection arms (played note, format-native integer period, post-envelope volume 0..64), `FormatIntegration`, `cases.tsv` rows, goldens; web player and CLI accept `.xm` | F1, F3, D3 | — |
| F5 | Conformance repairs and exclusions with accuracy-policy entries (the C9 shape) | F4 | — |

XM never creates a background voice and never steals.

## M6 — IT task graph

| Task | Deliverable | Depends on | Parallel with |
|---|---|---|---|
| G1 | Loader: header, orders, instruments, samples including sustain loops and stereo downmix (policy entry), IT 2.14 8/16-bit decompression, sample- versus instrument-mode (`format_extra`), MIDI macros into `format_data`; fuzz | E1 | G2, G3 |
| G2 | Per-voice resonant low-pass: `MixPath` gains a filtered mono step, `mix_run` gets `const FILTERED: bool` so the unfiltered body stays byte-identical (`cargo xtask goldens --check` proves it); integer coefficient tables for `FixedPath`, `f32` for `FloatPath`; delay state in `Voice` | D6 landed | G1, G3 |
| G3 | Instrument runtime, NNA/DCT/DCA, and a concrete stealing policy inside `starplayer-it` (scan the pool, `release`, retry); envelopes with carry, sustain loop, pitch/filter envelope, per-sample auto-vibrato, random variation via the core RNG. Settle architecture **Q3** from OpenMPT `Sndmix.cpp` and record it in the architecture document | E1, E2, E3 | G1, G2 |
| G4 | Effect set including `Sxx`, `Zxx`, old-versus-new effects; `TempoModel::ItModern` decided (IT/OpenMPT classic truncates per tick, or drift-free — an accuracy-policy entry either way; the conformance adapter must use the case's model) plus tempo slides; IT `QuirkSet` fields as corpus cases name them | G3 | — |
| G5 | Wiring: facade arm, goldens, `ConformanceFormat::It` and projection (cutoff encoded so the oracle's 0..255 round-trips exactly), `FormatIntegration`, `cases.tsv`; web player and CLI accept `.it`; hosts at `MAX_VOICE_CAPACITY` | G1, G2, G4, D3 | — |
| G6 | Conformance repairs and exclusions | G5 | — |

## Ordering summary

```
main ── E1 ∥ E2 → E3 → D3 ──┬── M3: D4 → D5 ; D6 ; D7             (branch m3)
                            ├── M5: F1 ∥ F2 → F3 → F4 → F5        (branch m5)
                            └── M6: G1 ∥ G2 ∥ G3 → G4 → G5 → G6   (branch m6)
```

- **Hard**: E1 before F1/F2/G1/G3; E2 before F2/G3; E3 before F2/G3/D3; D3 before
  D4/D5/F4/G5; D6 before G2.
- **Soft**: D7 early; M5 lands its `conformance.rs` and `quirks.rs` arms first and M6
  rebases; G2's `kernel.rs`/`path.rs` change lands last of everything.
- M5 and M6 are independent of each other.

## Branches and worktrees

The prerequisite wave lands on `main` as four `--no-ff` merges from sibling worktrees
(`../starplayer-e1` and so on). Then `m3`, `m5` and `m6` branch from `main`; each task in a
stream is a branch off its milestone branch in its own sibling worktree, merged `--no-ff`
into the milestone branch from that branch's checkout, and each milestone branch is merged
`--no-ff` into `main` from the `main` checkout. Rebase a milestone branch onto `main` only
when another stream has landed something it needs.

## Shared files (merge-conflict list)

- `crates/starplayer-model/src/{instrument,sample,builder,module,header}.rs` — E1, M5, M6
- `crates/starplayer-core/src/{tables,quirks,tempo}.rs` — E2, M5, M6 (`quirks.rs` hotspot)
- `crates/starplayer-mixer/src/{voice,kernel,path}.rs` — E3, G2
- `crates/starplayer-engine/src/{channel,sequencer,trace,telemetry,engine}.rs` — E3, D6, M5, M6
- `crates/starplayer/src/lib.rs`, `Cargo.toml` — D3, D5, F4, G5
- `crates/starplayer-host-wasm/src/lib.rs`, `crates/starplayer-offline/src/lib.rs` — D3, D5/D6, F4, G5
- `crates/starplayer-testkit/src/{conformance,lib}.rs`, `bin/starplayer-conformance.rs`, `conformance/*.tsv`, `conformance/README.md`, `xtask/src/main.rs` — F4/F5, G5/G6
- `plans/product/03-accuracy-policy.md` §3 (append-only: reserve **D42–D59 for XM** and **D60 onward for IT**), `plans/product/01-technical-architecture.md`, `plans/README.md` — everyone

## Research points to carry into the M5/M6 task files

1. Survey the pinned libxmp `test-dev` for XM/IT cases and dumps (`cargo xtask conformance
   --fetch-only`), and confirm whether libxmp's mixer dumps loop over `mod->chn` (background
   voices absent; the harness only has to skip them) or over virtual channels (pair by
   `(root channel, instrument, note, sample)`).
2. OpenMPT's voice-stealing heuristic (Q3): `Sndmix.cpp` `GetNNAChannel` / `FindFreeChannel`.
3. `ItModern` tempo semantics: an accuracy-policy decision for the owner in G4.
4. Stereo IT samples downmixed at load: record as a deviation.
