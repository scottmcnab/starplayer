# W17 — Compact Instruments table and bulk note-overlap selection

| Field | Value |
|---|---|
| Milestone | Web player maintenance; note-overlap and audition interface |
| Status | Landed on main 2026-10-28; independently verified; owner acceptance pending |
| Depends on | NO-W1/NO-08 landed on main; W16 Library selection behavior |
| Least-capable safe worker | GPT-6.1-sol (high) |
| Verified by | Worker and independent coordinator review; Node/Rust tests and packaged Chromium SAB/fallback/plain checks |

## Context for a fresh agent

The owner finds the Instruments list too heavyweight. Replace the multi-line widgets
with a compact table matching the Library listing's density. Column order is exactly
**# / Play / Name / Loop / [select-all checkbox] / Frames**. The checkbox heading
replaces the Ring label. Every eligible row has a native note-overlap checkbox;
ineligible rows have an empty cell. The header selects or clears every eligible
instrument in the current module, including offscreen rows. The owner explicitly
chose total source frames across distinct mapped samples for multisample instruments.

Read AGENTS.md. Historical assembly trees are read-only; no cargo fmt, broad staging,
unrelated formatting, unsafe code, or commit/PR attribution. Use a task branch and
sibling worktree. The implementation is a bounded UI and page-metadata change, not
an engine/protocol redesign. Existing audition, native effect and RT contracts apply.

Relevant sources: apps/starplayer-web/www/{index.html,style.css,app.js}, Library
checkbox behavior in www/library-view.js, page metadata exports in src/lib.rs,
test/{interactive-ui.test.mjs,no-headless.mjs,headless.mjs}. Read the archived
NO-W1 web task and W16 Library task for existing accessibility/session contracts.
Product architecture and accuracy policy remain the source of truth for overlap.

The instrument capability bits distinguish different facts: 1 live-playable;
2 any nonempty non-looping mapped PCM (overlap-eligible); 4 any mapped looping PCM;
8 reference-note-60 audition playable; 16 reference audition looping. PCM presence
is not bit 1: native low-map instruments can have PCM and bit 2/4 without bit 1/8.
The existing instrument_sample_length export only reads instrument.sample, which
is None for XM/IT instrument maps. Correct that query for this table.

## Deliverables

### Compact table and input

- Replace the ordered list with a semantic table, in the exact column order above.
  Retain the existing instrument-list ID for the row container where practical.
  Use single-line 32 px rows, subtle separators, a sticky heading and a bounded
  scrollable viewport (retain roughly the current 330 px height). Name takes the
  flexible width with ellipsis and full-name tooltip; Frames aligns right. Preserve
  original instrument order, one-based numbering and existing empty-name fallback.
  Match Library spacing/colors without adding virtualization or a generic framework.
- Row pointer/keyboard activation chooses the keyboard/MIDI target and highlights
  the selection. Preserve readiness and bit-1 gating from selectInstrument, its
  cleanup/routing behavior, and accessible selected/focus states. Define row keyboard
  activation as Enter/Space when the row itself is the target; native row controls
  must not bubble into selection. Retain existing musical keyboard shortcuts.
- Show an icon-only Play button (triangle SVG or equivalent) whenever total source
  frames are nonzero. Enable only when interactiveReady and bit 8; provide a clear
  accessible audition name and disabled explanation. No button/icon with zero PCM.
  Keep native pointer-down/up/capture/cancel, keyboard-down/up, synthetic click,
  focus-loss and one-shot/looping release semantics. Playing does not select a row.
- Loop shows only infinity when bit 4 is set, including sustain loops; otherwise
  it is blank. Preserve accessible mixed-map/native-loop explanations through labels
  or tooltips rather than a second row of status text.
- Ring cells contain a native checkbox only when bit 2 is set, independently of
  live-playability; ineligible cells stay empty. Preserve row labels explaining
  Let notes ring and native IT behavior, bypass editing and audio readiness gating.
- Update accepted checkbox/header/selection state in place. Keep scroll and keyboard
  focus across same-module refreshes. An unrelated successful acknowledgement must
  not replace a held Play DOM node and trigger blur/lost-capture cleanup. New loads
  and intentional routing/lifecycle changes retain their existing cleanup behavior.
  Keep global overlap, Stop preview, All sound off and existing routing controls.

### Header checkbox and acknowledgement semantics

- Replace the Ring text header with a native checkbox, accessible name
  **Select all eligible instruments for note overlap**, and explanatory tooltip.
  Match Library hit-target and keyboard/touch behavior. None accepted is unchecked;
  some accepted is indeterminate; all eligible accepted is checked. Mixed activation
  selects all; clearing checked deselects all. Disable with no eligible rows or when
  interactive controls are unavailable. No header checkbox mutates row selection.
- Each header activation snapshots the current desired boolean and enqueues that
  value for EVERY eligible instrument in FIFO order. Do not filter against accepted
  values: previous requests may still be pending. A following bulk clear or individual
  override must follow earlier commands and win in the final accepted state.
- Reuse identity-tagged per-instrument commands and the existing bounded bridge.
  There is no new bulk opcode or whole-operation atomicity guarantee. Worklet batches
  admit independently (up to 63 controls); accepted state must reflect acknowledged
  successes. On a later rejection, preserve earlier accepted changes, restore rejected
  controls and show the truthful partial/mixed header through existing error handling.
- Compute header state separately from row checkboxes; selectors must not treat the
  header as a row with a missing instrument ID. Recompute after matching acknowledgements,
  rejection, new session and rebuild. Retain session/build identity filtering, queue
  cancellation/retention and settings revisions. Stale commands/replies cannot target
  the new module. Bulk edits work while global overlap is bypassed without enabling it.

### Source-frame metadata and documentation

- Extend instrument_sample_length(index) -> u32 to sum model source lengths of distinct
  sample IDs referenced by the instrument, before worklet enhancements and excluding
  guard frames. Deduplicate IDs, not sample content. Empty/invalid index returns zero.
  XM and IT instrument mode use only native note maps; IT sample mode and MOD/S3M/MTM
  use direct sample references. Count each nonempty sample once regardless of repeated
  map entries, and exclude unrelated/unmapped samples. Preserve single-sample counts.
  Keep computation on the page/inspection side, not in render or JS effect logic.
- Frames is blank for zero; its heading tooltip explains total source frames before
  enhancement. Use this metadata for PCM presence, bit 8 for Play enablement, bit 1
  for row activation, and bit 2 for Ring eligibility.
- Update apps/starplayer-web/README.md and plans/README.md with table/bulk/frame behavior.
  The existing product contract is unchanged; avoid rewriting archived implementation
  evidence. Report exactly which checks ran and any genuine limitations.

## Research points

- Follow all renderMetadata callers and acknowledgement paths before changing DOM
  lifetime. Existing helpers rebuild the whole list on selection/load rejection.
  Keep live input cleanup explicit and avoid new incidental blur/capture losses.
- Inspect queueInteractive/queueCommand/flushFallbackCommands, rAF queue draining,
  worklet applied barriers and load/rebuild capture. A bulk operation can exceed 64
  records and must not lose its remainder or bypass settings revision tracking.
- Inspect native_map in crates/starplayer/src/live.rs for format identity, and model
  source length/loop behavior. Prefer a small local pure metadata helper over a new
  engine abstraction. Retain capability independence from source-PCM totals.
- Migrate affected tests using semantic row/control selectors rather than brittle
  child offsets. Existing mocked DOM cannot prove bubbling/focus/pointer behavior;
  use the established Chromium/CDP harness for those guarantees.

## Verification

- git diff --check; node --check on changed JS modules/harnesses. No cargo fmt.
- node --test --test-concurrency=1 apps/starplayer-web/test/*.test.mjs.
- CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 RUST_TEST_THREADS=2 cargo test -p starplayer-web.
- CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo xtask wasm, then
  STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --interactive-only
  (existing runner executes SAB, forced fallback and plain HTTP modes).
- Meaningful Node/Rust tests: column order/empty states/blank cells; PCM-bearing but
  live-unplayable instruments; loop/mixed/sustain flags; eligible-only checkboxes;
  source frame totals for single samples, empty/invalid IDs, repeated maps, several
  distinct samples, shared IDs, unmapped samples, XM and both IT modes.
- Bulk tests: none/some/all and mixed-to-all; global bypass; unavailable/no eligible;
  offscreen rows; >64 commands in shared-ring/fallback; immediate select then clear;
  row override before first ack; first chunk accepted and later rejected; fresh load
  during pending work; rebuild/capture with queued settings. Confirm final accepted
  values, truthful header and no stale writes, without weakening existing tests.
- Browser checks: real native Space/Enter and pointer/touch activation without row
  selection/double commands; pointer cancel/focus loss; held looping audition during
  an unrelated Ring ack; focus/scroll preservation; compact desktop and phone layouts,
  checkbox hit areas and no page-wide horizontal overflow. Retain paused/stopped
  audible audition, keyboard/MIDI destination, rebuild and explicit reload regressions.
- Coordinator reviews the diff and independently executes Verification before commit.
  Worker leaves work uncommitted for review. Coordinator records final evidence,
  lands as one named change according to AGENTS, and archives the plan on main.

## Out of scope

Sorting/filtering, row multi-selection, instrument virtualization, persistence of new
preferences, generic UI frameworks, external site publishing/deployment, source/sample
processing changes, engine/audio protocol changes, historical source edits, unrelated
formatting, and cargo fmt. Do not modify the independent W6 worktree or owner scratch files.


## Worker implementation and verification — 2026-10-28

Implementation is uncommitted in sibling checkout
`../starplayer-w17-compact-instruments-table`, branch
`w17-compact-instruments-table`, for coordinator review. No merge, push, deployment,
historical-source edits or cargo fmt was performed.

- Replaced the list with the six-column semantic table, sticky heading, bounded
  330 px viewport, exact 32 px rows and 40 × 32 px native checkbox hit targets.
  Name cells retain full tooltips; zero-PCM Play/Frames cells and ineligible Ring
  cells are blank. Native capability bits independently govern selection, audition,
  loop display and overlap eligibility. Row selection availability uses tabindex and
  a data marker, avoiding aria-disabled on ancestors of enabled native controls.
- Same-module selection, accepted acknowledgements and rejection restoration update
  existing rows/controls. The real browser proves a held looping Play node keeps
  capture/focus across an unrelated Ring acknowledgement, and offscreen checkboxes
  retain focus and scroll after acknowledgement.
- Bulk edits snapshot every eligible instrument and preserve FIFO order for later
  clears/individual overrides. Real 96-instrument XM coverage exposed the difference
  between the bridge's 63-record maximum and the native engine's 32-command quantum
  admission budget plus settlement reserves. Interactive page ring occupancy and
  fallback groups are now capped at 16, paced by the existing worklet applied barrier;
  v1 capacity is unchanged. No engine, worklet or wire protocol was changed.
- Fresh loads cancel unsent old-session instrument-overlap requests before posting
  the load. Already-admitted requests retain their acknowledged values on load
  failure; same-session replacement captures still drain/replay queued settings.
  Existing identity filters discard old remainders/replies at installation. Tests
  cover rejected-load recovery, pending fresh loads and bulk capture across rebuild.
- Page-side source-frame metadata sums distinct sample IDs from native XM/IT maps,
  ignores unrelated direct/unmapped samples in those modes, and preserves direct
  references for MOD/S3M/MTM/IT sample mode. Tests distinguish matching PCM content
  with different IDs, repeated maps, shared IDs, empty samples/maps and invalid
  instrument indices; guards are excluded.

Final checks passed with one Cargo/browser stream at a time, two Cargo jobs,
`CARGO_INCREMENTAL=0`, two Rust test threads and Node test concurrency one:

- `git diff --check` and `node --check` for app.js, interactive-ui.test.mjs,
  no-headless.mjs, headless.mjs and w17-headless.mjs.
- `node --test --test-concurrency=1 apps/starplayer-web/test/*.test.mjs`:
  **352 passed**, zero failures. Log `/tmp/w17-node.log`.
- `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 RUST_TEST_THREADS=2 cargo test -p
  starplayer-web`: **24 passed**, zero failures; zero doctests. Log
  `/tmp/w17-rust.log`.
- `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 cargo xtask wasm`: passed. Log
  `/tmp/w17-wasm.log`. The sibling target symlinks to the existing
  `/tmp/starplayer-no-target` cache to avoid duplicate build storage.
- `STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs
  --interactive-only`: **SAB, forced fallback and plain HTTP all passed**. Log
  `/tmp/w17-browser.log`. Retains paused/stopped audible audition, selected keyboard
  destination, posted-load failure recovery, enhancement/sample-rate rebuilds and
  explicit same-byte reload reset. New checks cover 96-row offscreen bulk operations,
  rapid select-clear/override, native row Enter/Space, native checkbox Space, pointer
  and CDP touch controls, pointer cancellation/focus-loss cleanup, held-loop
  acknowledgement stability, focus/scroll retention, bulk rebuild capture and fresh
  load during queued bulk edits. Native input checks wait for actual accepted acks.
- Desktop/390 px phone screenshots for every mode:
  `/tmp/w17-instruments-{sab,fallback,plain}-{desktop,phone}.png`. Worker visually
  inspected SAB screenshots; measured rows are 32 px and phone/desktop page width
  does not overflow. These are review artifacts, not repository assets.

Full CI, physical MIDI/hardware and owner listening were not run or claimed. The
optional NO-07 owner-fixture path was not enabled; the browser uses its existing
MOD fixture and the new native 96-instrument mixed-map XM fixture. Independent coordinator verification is recorded below; owner acceptance remains pending.


## Independent coordinator review — 2026-10-28

Reviewed the complete production/test diff and SAB desktop/phone screenshots.
Source metadata follows native mapping modes; live-selection, reference-audition and
overlap eligibility remain independent. Same-module updates retain native controls,
while row keyboard handlers exclude descendant controls. Rows unavailable for live
selection do not expose aria-disabled on their independently usable Ring controls.
Reviewed the 16-record page pacing against the existing 32-command engine admission
budget and applied barriers, and fresh-load cancellation against retained-load failure
and same-session capture behavior. No engine/worklet/protocol or historical files changed.

Independently repeated the task verification against the final source:

- Full Node suite: 352 passed, zero failures/skips; `/tmp/w17-review-node.log`.
- `cargo test --offline -p starplayer-web`: 24 passed; `/tmp/w17-review-rust.log`.
- `cargo xtask wasm`: passed; `/tmp/w17-review-wasm.log`.
- Interactive Chromium SAB, forced fallback and plain HTTP: all passed, including
  96-instrument bulk FIFO, native keyboard/touch, held-loop stability, rebuild capture
  and fresh load during queued edits; `/tmp/w17-review-browser.log`.
- JS syntax checks and `git diff --check`: passed. Packaged app.js, index.html and
  style.css match the reviewed sources byte for byte.
- Main landing checkout: `cargo xtask wasm` passed (`/tmp/w17-main-wasm.log`).
  Packaged HTML/CSS/app, loader glue, worklet bundle and both WASM binaries match
  the independently tested sibling artifacts byte for byte.

Resource limits and exclusions match worker verification. Owner browser/listening
acceptance remains pending; no deployment was requested or performed.
