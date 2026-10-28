# W18 — Branch review and verification

| Field | Value |
|---|---|
| Branch | feature/panel-playback-controls |
| Baseline | main 4d03bcbd995288e1153085f7cce355693e4e1e3d |
| Status | Landed on main via merge 14bec2c; owner acceptance pending |
| Review | GPT-6 Astra, medium; entire branch and untracked source/tests, then two follow-ups |
| Landing | Owner subsequently authorized --no-ff merge, origin push and merged-worktree cleanup; merged as 14bec2c |

## Scope and coordinator review

Implemented the approved W18 Instruments/reference-note, page-policy and Playlist tasks
in independent sibling worktrees with GPT-6.1-sol high workers. Coordinator reviewed
production changes and tests, integrated them in the feature checkout, and independently
ran combined checks. Historical assembly, the independent W6 checkout and the four
pre-existing untracked files in main were untouched. No cargo fmt or golden regeneration.

Selected-note metadata is copied from native-map/tuning preflight before asynchronous
inspection can change modules. UI, host and prepared rack validate the chosen key; a
held press retains its original looping-release behavior. MIDI60 remains the fixed tuning
anchor and default, displayed C-5 (60), middle C. Cross-channel matching adds only the
approved incoming-song to Custom/Background/custom_detached song-tail predicate;
immutable identity, same-channel behavior and PCM/Ring/global eligibility remain intact.

Page desires are distinct from identity-scoped applied policy. Replay gates actual
transport execution, including same-node saved Playing. UUID Playlist selection is
independent of the persistent model, and bulk removal preserves navigation cancellation
and last-upload-reference cleanup. Compact controls retain focus and native activation.

## Astra findings and resolutions

All four P2 findings were accepted and fixed. Final follow-up reported no remaining
actionable issues and independently passed the policy, interactive UI and metadata tests.

1. **Play after natural completion during replacement did not rewind.** Deferred saved
   restoration now distinguishes an explicit user intent from an unchanged saved state.
   Shared host action effects consume the armed rewind and seek zero for Play/Resume,
   preserve it for Pause and clear it for Stop. Native/saved-only restoration preserves
   exact cursor/ramp behavior. Regression uses a naturally ended module.
2. **Failed-load recovery lost pending transport.** Retained-module reconciliation
   carries the latest transport intent and continues accepting matching identity updates.
   Regressions cover pending Play, Pause and Stop through a proven retained installation.
3. **A newer policy could arrive after release was queued but before audio rendered.**
   The existing PlayerTaps allocation holds a release epoch. Browser-only transport
   commands check it before mutation; late policy, newer transport or superseding load
   revokes an obsolete queued release. The worklet keeps replay pending through final
   audio acknowledgement and filters old release revisions. Saved state remains available
   for exact retry; native commands stay ungated. Tests assert actual rendering order,
   fresh Play/Resume and saved Playing, same-policy Stop/Pause and supersession.
4. **Transport could arrive after the worklet sent readiness but before the page received
   it.** A transport revision travels with pending commands and the audio acknowledgement.
   Latest matching page completion submits only a newer intent missing from that ack.
   Tests cover Stop/Pause and Play→Stop→Play without duplicating an applied Play.

The new host callback regression exercises deferred installation, revoked and retried
saved restoration, and revoked/final fresh browser transport under the existing allocator
hook. No extra engine-command expansion, render allocation, lock or panic was introduced.

## Verification evidence

Coordinator verification passed against the final production source, with one Cargo/browser
stream, two Cargo jobs, incremental compilation disabled, two Rust test threads and Node
concurrency one. Final acknowledgement-race edits changed only JS and received a fresh
full Node run, WASM package, worklet harness and all browser modes.

- `cargo xtask ci`: **all 11 jobs passed**, including workspace/trace/telemetry tests,
  RT corpus checks, canonical and SIMD goldens, six-size determinism, FMA/trace audits,
  WASM, both bare-metal targets, clippy and no_std purity. Log `/tmp/w18-ci.log`.
  Conformance: 261 cases, 179 pass, 16 accepted deviations, 66 existing known failures,
  zero unexpected failures. Exclusions and golden hashes were not changed.
- Shipping `cargo test --offline -p starplayer-host -p starplayer-host-wasm -p
  starplayer-web --features note-overlap,interactive-audition`: **194 tests plus one
  doctest passed**, including all 11 allocator guards, four deferred-transport cases,
  18 transport tests, 64 WASM host tests and 24 page tests. `/tmp/w18-final-rust.log`.
- Additional overlap-only engine uniqueness tests: two passed. Combined-feature
  production/helper tests, including live exclusions in both directions: three passed
  in CI. Explicit fixed-mix facade live-audition tests: 20 passed; absolute-frame,
  six-size song/live schedules and separately compiled default PCM pairs: three passed.
  `/tmp/w18-combined-rust.log` and `/tmp/w18-ci.log`.
- Worker checks compiled host-WASM feature-off, overlap-only and audition-only; the
  facade with both optional features, all five formats and fixed mixing compiled for
  `riscv32imc-unknown-none-elf`. `/tmp/w18-instruments-no-std.log`.
- Complete `node --test --test-concurrency=1 apps/starplayer-web/test/*.test.mjs`:
  **390 passed**, zero failures/skips. `/tmp/w18-final-node.log`. All changed JS modules
  and harnesses passed `node --check`; `git diff --check` passed.
- `cargo xtask wasm`, ring harness and actual packaged WASM worklet harness passed.
  `/tmp/w18-final-{wasm,ring,worklet}.log`. All 29 handwritten source assets match
  packaged files or the worklet bundle; current ring and processor sources are embedded.
- Chromium `headless.mjs --interactive-only`: **SAB, forced fallback and plain HTTP
  passed**. Includes retained failed loads, both policies across enhancement/output and
  explicit same-byte reload, page-reload reset, selected-note native XM tuning, captured
  loop release, 96-instrument FIFO bulk/capture/reload, keyboard/touch and stable controls.
  `/tmp/w18-final-interactive-browser.log`.
- Full Chromium `headless.mjs` with its default 60-second transport run: **all three
  modes passed**, including transport/background/media/output checks, Library/indexed
  folder/archive behavior, W15 discovery/metadata/worker/stress/rebind/persistence and
  Playlist selection/native-input checks. `/tmp/w18-final-browser.log`.
- Desktop/390px phone views were measured and visually inspected. Instrument and
  Playlist rows are 32px; phone tables stay within the page and Playlist horizontal
  scrolling is confined to its panel. Native checkbox and row keys, marker/tooltips,
  metadata focus, bulk removal, UUID selection and storage silence are covered.
  Full Instruments panel captures: `/tmp/w17-instruments-{sab,fallback,plain}-{desktop,phone}.png`;
  Playlist captures: `/tmp/starplayer-w18-playlist/playlist-{sab,fallback,plain}-{desktop,phone}.png`.
- The four current SAB desktop/phone previews were copied byte-for-byte to
  `/mnt/c/temp/w18-{instruments,playlist}-{desktop,phone}.png`.

Logs and screenshots are external review artifacts, not repository assets. Physical
MIDI/device testing and owner listening were not performed or claimed. The owner later
authorized --no-ff main landing, origin push and cleanup. Merge 14bec2c preserves the
reviewed feature as one unit; plans are archived with owner acceptance remaining.
No direct deployment was performed.

An exploratory worker `--all-features` run included diagnostic trace instrumentation
and therefore failed eight callback-allocation tests. The guard was not weakened. The
shipping note-overlap + interactive-audition configuration passes all eleven allocator
regressions, including the new browser-release path; the standard CI trace jobs exercise
trace separately. This exploratory configuration is not claimed as passing.

## Main follow-up — adjacent audition controls

After landing W18, the owner requested a single direct-to-main commit moving Play
between the note-overlap checkbox and Frames, reducing pointer travel while auditioning.
The resulting columns are **# / Name / Loop / checkbox / Play / Frames**; Play and Loop
headers remain visually blank. HTML column widths/headings, rendered cells, existing
layout regressions and the application README follow that order.

Astra reviewed the complete follow-up diff and approved it without actionable findings.
All 11 `cargo xtask ci` jobs and all 390 JavaScript tests passed; WASM packaging and
interactive Chromium SAB, forced fallback and plain HTTP checks passed. Desktop and
390px phone captures show adjacent checkbox/Play controls with the existing 32px rows.
Logs: `/tmp/w18-play-column-{ci,node,wasm}.log` and
`/tmp/w18-play-column-browser-final.log`.

The first SAB browser run intermittently failed the existing fresh-load-during-bulk
scenario with a settings-replay rejection. An unchanged isolated SAB rerun and the
subsequent full three-mode run passed. Astra found no column-layout regression; an
old-identity cleanup/replay race is a possible cause, but the failure log lacks command
identities to confirm it. The initial evidence remains in
`/tmp/w18-play-column-browser.log`; the isolated pass is in
`/tmp/w18-play-column-browser-retry.log`. Playback rejection checks were not weakened.

No engine, wire, preference-lifetime or input-handler changes are part of the column move.
