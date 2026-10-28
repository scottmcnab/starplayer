# W18-I — Instruments reference note and session playback policies

| Field | Value |
|---|---|
| Status | Implemented and verified on feature branch; intentionally unmerged |
| Baseline | main 4d03bcb (W17 compact Instruments) |
| Least-capable safe worker | GPT-6.1-sol (high) |
| Coordinator | W18 master; worker leaves changes uncommitted for review |

## Context for a fresh agent

Read AGENTS.md, W18 master, architecture NO sections and accuracy policy Optional
creative playback. Implement all requirements below in your assigned sibling checkout.
No main merge, cargo fmt, historical edits, broad staging or attribution. Owner asked
another Astra medium review of the ENTIRE final branch; coordinator arranges it.

Source entry points: www/app.js readMetadata/renderMetadata/updateInstrumentControls,
activateModuleOnNode/activateLoadedModule/onWorkletMessage/queueCommand; www/ring.js;
www/worklet-processor.js startReplay/loadModule/stageLoadTransport; page src/lib.rs;
core interactive actions; engine ownership.rs/overlap.rs/instrument/live.rs;
facade live.rs; host player.rs/replacement.rs; host-wasm lib.rs. Fixed reference
REFERENCE_NOTE=60 is the tuning anchor, NOT a user preference to change globally.

## Deliverables

### Instruments UI and selected note

- Leave Play and Loop headings visually blank, retaining accessible names. Play
  icon button borderless/transparent with existing hit area, disabled and focus states.
- Native Reference Note combobox notes 0..127, default60. Reuse formatNote:60 is
  C-5 (60), middle C; do not claim C-3. Reference only affects button audition,
  not keyboard/MIDI tuning. Retain value in page memory across track and graph changes;
  page reload resets60. No storage persistence.
- New selected-note preview action; preserve PreviewInstrument(InstrumentId) and
  opcode20 semantics reference60. Add opcode25 explicit-note preview (arg instrument,
  extra note) and opcode26 uniqueness bool (arg0/1, extra0). Extend UNDEPLOYED wirev2
  in place, keep 7word identified records; no v3 or compatibility migrations.
- Immutable per-note playable/looping metadata is captured with committed module
  metadata. Failed candidate inspection must never let the UI consult a different
  inspection WASM module. Selected-note viability must agree UI, host and rack,
  including XM relative-note/finetune native pitch preflight and unmapped keys.
- Play presence still depends on source PCM count. Enable by chosen-note viability.
  Capture note and looping at press for release; a combobox change affects next press.
  Preserve pointer capture/cancel/keyboard/focus-loss and stable same-module controls.
  Aggregate Loop indicator, Ring eligibility and source frame totals stay W17 semantics.

### Cross-channel uniqueness

- New checkbox Unique notes across channels defaults off. Global overlap+per-instrument
  Ring still required. Current same-channel duplicate behavior unchanged.
- Additional predicate: incoming SONG voice can retire an eligible Custom-domain,
  Background-lifecycle, custom_detached SONG tail on another channel iff exact
  instrument, original note and captured native onset volume match. Raw MOD period
  remains raw. Keep immutable original channel; only ignore it in this extra predicate.
- Foregrounds, native-created backgrounds and live/audition voices are excluded from
  the new branch IN BOTH DIRECTIONS. Existing same-channel live behavior stays.
  Reuse current sounding-sample eligibility and retirement ramps. Settings affect
  subsequent onsets only; toggling uniqueness does not scan/retire current tails.
- Keep production predicate and PlaybackState::duplicate_candidates helper aligned.
  Update core actions, all format call sites as necessary, feature gates, host shadow,
  direct snapshot/restore and transient replacement codec in lockstep. Current codec
  is not deployed; no legacy snapshot migration. No RT allocation/locks/panics.

### Page-session desired versus audio-applied policies

- Enable note overlap and Unique notes across channels persist for CURRENT PAGE
  session, including new modules, explicit same-byte reloads, playlist advances,
  enhancement/renderer/output rebuilds. Reset false ONLY on page reload. No storage.
  Per-instrument Ring resets on new module, retained on same-module rebuild.
- Separate desired page preferences from identity-scoped accepted audio-applied
  settings. Older acknowledgements update applied state but cannot overwrite newer
  desired intent (including same identity). Rejected latest DIRECT edits roll back
  to last applied value. Failed loads preserve desires and reconcile retained module.
- Policy edits during candidate installation/replay must be reconciled by revision;
  readiness/Play cannot proceed under obsolete policy. Extend host setting snapshots,
  page revision bookkeeping, worklet replay and supported opcode validators together.
- CRITICAL: worklet currently stages loadTransport before startReplay, including
  deferred loads; replacement host currently queues RestoreTransport too early.
  Defer pending Play/Resume AND saved Playing-state restoration until LATEST policy
  revision is actually audio-applied. Preserve saved cursor/ramp and newest Stop/Pause
  intent. Do NOT emulate restore with destructive Stop/Play. Large mute/insert replay
  spans multiple quanta: policy must precede actual first playing samples, not merely
  final ready notification. Retain cancellation identity and failed-load proof rules.
- Update batch validators/ranges, JS ring/SAB/fallback, Rust host-wasm decode/dispatch,
  prepared capability paths and admission reserves. Shared page groups remain16.

### Documentation and tests

- Update Instruments/help/Ring tooltip and relevant app README paragraphs for selected
  reference, page lifetimes and precise song-tail cross-channel scope. Coordinator
  edits product foundations/index; avoid conflicting with playlist worker's sections.
- Extend existing tests meaningfully; adapt old tests expecting global reset on new
  load to page lifetime, preserving separate Ring reset assertions. Do not weaken
  unrelated input, timing, failed-load, queue and native feature-off coverage.

## Research points

- Preview validation is hardcoded60 at page, facade, host and rack. Use selected-note
  preflight in all places. A sample's loop metadata is independent of pitch viability.
- Shared duplicate scan is called by live admission too. Candidate-only filtering
  erroneously permits live onsets to cut song tails. Inspect both incoming provenance
  and Custom/Background/custom_detached candidate. Retain native IT NNA/DCT behavior.
- Current checkbox state is accepted state; naive preservation in activateLoadedModule
  loses intent under overlapping acknowledgements. Trace fresh and candidate paths,
  capture revision retry, rejected direct edits, same-node retained load failures.
- Host MAX_COMMANDS_PER_QUANTUM32 includes settlement expansion/reserves. New settings
  and replacement transport gating must not bypass it or allocate in render.
- Use existing no-headless/w17 fixture and CDP harness, add selected-key map fixtures.

## Verification

- git diff --check; node --check touchedJS. No cargo fmt.
- Node entire test suite --test-concurrency=1; focused Rust tests for page/facade/
  engine/host/host-wasm with optional feature combinations and no_std checks.
- Exact match tests note/instrument/volume and raw periods; cross song tails across
  channels; unchanged off/default/same-channel; live in BOTH directions, native
  background and foreground exclusions; onset-only toggles. Production/helper agree.
- Note fixtures:60 unmapped but selected playable and inverse, XM tuning rejection,
  invalid note, captured looping release despite chooser edit, failed inspection.
- Policy rapid toggles/older acks/latest rejection; fresh and same-byte reload,
  playlist advance, enhancement/renderer/output; failed retained load and candidate
  revisions. Force multi-quantum mute/insert replay, assert actual applied order before
  Playing/Resume/saved Playing, newest Stop/Pause and cursor/ramp preservation.
- RT allocation and six-size1/3/64/128/4096/8191 byte-identical tests; canonical defaults
  unchanged. cargo xtask wasm and Chromium --interactive-only SAB/fallback/plain.
- Coordinate single Cargo/browser stream with parent. Worker leaves code uncommitted,
  records exact evidence and limitations. Coordinator independently checks/reviews.

## Out of scope

Keyboard/MIDI reference retuning, native tracker tuning/effect repairs, v3/migrations,
browser persistent policy storage, playlist implementation, foundation doc edits,
main merge/push/deploy, user scratch files, W6 worktree and canonical golden changes.

## Completion evidence

Implementation, independent verification and all resolved Astra findings are recorded
in [W18 review and verification](W18-review-and-verification.md). The owner requested
that the completed feature remain unmerged; no main landing, push or deployment occurred.
