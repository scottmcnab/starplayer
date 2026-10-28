# NO-08 — Resolve independent full-branch review findings

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Integrated on feature/note-overlap-audition by reviewed single-change squash; task commit 20dbc07; all eight corrections accepted by final Opus/Astra reviews; automated verification complete; owner/device acceptance pending |
| Depends on | NO-01–07 and NO-W1 at 55479442f198f16be0492e8da9821f35a02762ec |
| Worker | gpt-6.1-sol (high) |
| Coordinator | gpt-6.1-sol (xhigh) |
| Independent reviewers | claude-opus-5-5 (high supplied); gpt-6-astra (medium) |
| Parallel with | None; one task/reviewer/build stream at a time |

## Context for a fresh agent

Read AGENTS.md, NO-master-plan.md, all current NO task contracts, relevant product architecture/accuracy foundations, and [the independent branch-review reports and coordinator dispositions](NO-branch-review-2026-10-28.md). The current instrument-only/onset-volume contract supersedes historical verbatim reports. Baseline feature head 55479442f198f16be0492e8da9821f35a02762ec, comparison main d0cdcc53dd320675edd64258aacf247f8cc57f0c. Work only in the designated sibling task worktree/branch. No main merge/deployment, cargo fmt, historical assembly changes, golden regeneration, unrelated formatting, attribution, broad staging, or unapproved model substitution. No nested agents. Keep builds/tests serial and limited to two Cargo jobs, no incremental compilation and two Rust test threads; Node --test-concurrency=1 and one Chromium mode at a time.

Independent initial reports: Opus full source pass found two low issues plus wire hardening; Astra source pass found three P2 issues plus the same wire issue. At assignment the findings were source-derived; the evidence below records their subsequent failing and passing runtime regressions. Do not claim checks that were not run. Raw reports must remain verbatim. Exact Opus source anchors sometimes describe patch offsets; use actual current functions below. The supplemental Opus source pass adds the two accepted findings below. Both independent initial reports and the supplemental report are retained verbatim with coordinator dispositions.

## Deliverables

1. **ASTRA-01, external feed visibility.** Refresh external feed lookahead at the current source frame before calculating/rendering the first segment of each quantum. Prepared MidiSource currently exposes only peeked feed deadlines; initial and newly nonempty idle queues remain invisible until after mixing. Constructor-only priming is insufficient. Separate polling from articulation; preserve exact frame splitting, native source order, retained cadence and 128-frame DSP quanta. Do not restore per-source articulation or introduce periodic lateness. Review all EventSource::advance_to implementations for same-frame behavior.
2. **ASTRA-02, deferred first attachment.** Preserve preparation-time logical song position and whole/fractional tick/event remainder until actual first audio-side attachment. First attachment of a prepared tracker or SMF can occur later than control-thread preparation under supported retirement/queue pressure. Rebase sequenced deadlines/origins by this delay exactly once, for playing, paused and stopped targets; subsequent Pause/Resume retains its existing semantics. Never rebase absolute live event timestamps. Preserve feature-off and unconfigured behavior; no allocation/deallocation at attachment.
3. **ASTRA-03, IT resurrection after clear.** Track interactive sound-clear suppression in IT channel memory and prevent instrument-only/effect synthetic restarts until a genuine note is accepted. Preserve effect/instrument memories, actual delayed onsets and native instrument-only restarts after natural completion. Apply relevant clear scopes; checkbox bypass/live-only cleanup must not suppress native song behavior. Keep marker absent when the feature is compiled out where practical.
4. **NO-R1, zero/sub-frame cadence.** Clamp installed Q32.32 synthesized cadence to a minimum one output frame consistently with the legacy clock's progress guarantee. Preserve all valid intervals/fractional phase and existing infallible construction; avoid introducing a new public error solely for an already-clamped input class. Update cadence docs. Zero/sub-frame configured intervals must not execute repeated control ticks at a single frame or raise zero-advance/event-limit warnings.
5. **NO-R2, burst telemetry.** Coalesce interactive transition publication to the end of the bounded engine command drain, rather than publishing every individual command into the depth-three ring. Actual locations: Engine::drain_commands (~878), apply_interactive (~1012/1052), publish_interactive (~1070). Ensure final live/transport/cursor state is immediately available at the publication boundary. Preserve generic telemetry ring semantics and canonical publish behavior; avoid optional Engine storage/heap growth. The report's speculative replacement-cursor extension is not independently established and is not a claimed reproduced defect.
6. **NO-I1 / ASTRA-04, malformed v2 groups.** Propagate malformed shared/fallback members to explicit whole-group rejection instead of silently filtering them. Preserve requested/current identity where representable, no partial application, bounded allocation-free shared drain, v1 compatibility and 63-live-record batch throughput. Cover invalid argument/opcode/identity and invalid-only groups; malformed/foreign producer hardening must not redefine valid commands.

7. **NO-R4, natural end → Pause → Play.** Preserve and consume an armed natural-end rewind on the next Play/Resume regardless of held state. Pause must not turn that into a spurious second natural completion/playlist ended event. Keep explicit user Stop's existing rewind cancellation. Test native host and controller/WASM boundary as relevant.
8. **NO-R5, musically invalid interactive seek.** Preserve current voices and all DSP insert state for invalid order/row/frame requests that pass wire numeric validation. Native audio-side can_seek checks already reject them, but Player::request_seek queues ResetAll before that validation. Couple reset to authoritative successful interactive seek, respecting whole 128-frame DSP processing, retained live audition, RT safety and queue reservations. Do not solve only in UI or widen source validation. Preserve valid seek reset and scoped legacy behavior; do not silently alter native seek semantics or introduce a cross-thread validity race.

## Research points

- First-attachment source state currently anchors at the sampled preparation frame; distinguish this from ordinary held transitions. Cursor conversion and SMF absolute event translations have different roles.
- Interactive Stop and AllSoundOff must not resurrect remembered notes; authored native instrument-only restarts remain canonical outside that explicit clear.
- Existing six-size equality tests can be identically late. Prove event placement independently of output equality, voice attack ramps and normal web lead time.
- Do not change atomic group policy or normal rejection cleanup merely because the reviewer observed a design risk; the coordinator did not identify that intentional behavior as a defect.

## Verification

Write/run focused failing regressions before fixes and retain exact logs; then run them with the fixes. Do not weaken existing assertions or fixtures to pass.

- Feed: exact dispatch/deadline, zero late-event diagnostics and appropriate nonzero PCM window for frame0/17/18, installation at nonzero engine time, and newly admitted event within the next quantum after idle. All six host block sizes; prepared live and relevant song path. Preserve exact fixed/float schedule tests.
- Attachment: actual delayed retirement/queue installation tests, fresh stopped song frame0 and one initial tick; saved paused/stopped/playing tracker cursor with nonzero fractional phase and delay beyond several ticks; correct resume at R+D. SMF counterpart. Confirm live timestamps do not shift.
- IT: instrument- and sample-mode note→Stop→Play→instrument-only stays silent; AllSoundOff equivalent; real note afterward sounds. Native natural-completion/instrument-only control still restarts. Legacy/mapped/native-NNA tests and canonical pairs unchanged.
- Cadence: direct ControlClock zero/sub-frame and configured Engine progress/no-warning tests under each relevant feature. Valid fractional schedules unchanged at six block sizes.
- Telemetry: more than three interactive commands followed by live clear/Pause in one drain; immediate snapshot has final live/state/cursor, not an intermediate. Direct-engine and host/replacement boundaries as relevant. Strict allocator/resource assertions remain exact.
- Wire: actual worklet+WASM on SAB/fallback, both invalid/valid orders, invalid-only groups, rejection identity/no partial state and valid retry; retain high-rate63 event group and v1 tests.
- Rewind: natural AtEnd Stop/FadeOut → Pause → Play/Resume starts at zero, clears natural-end, stays playing beyond eight quanta and emits ended once. Explicit Stop cancellation test remains strict.
- Seek: ringing delay/master insert and song/live voices survive musically invalid order/row/frame requests unchanged; valid seek still clears expected insert state and retains live voice semantics. Test real host/audio and relevant wire path, not only producer rejection.
- Final affected Rust suites and strict lint/no_std/independent feature checks; six-size fixed/float integration and canonical feature-off matrix; goldens --check (no regeneration); final JS suite, packaged WASM/ring/actual worklet and relevant sequential browser modes. Run full root CI on the final corrected production source (all 11 jobs). Report historical baseline failures separately.
- Record all exact commands/results/base and source changes. Return the diff unstaged/uncommitted for coordinator review; do not stage or commit without coordinator instruction.

## Out of scope

New product controls, altered duplicate identity or timing contract, unrelated baseline fidelity repairs, weakened resource/determinism/RT invariants, new M11 extraction, main merge/deployment, physical hardware or listening claims.


## Worker handoff

Worktree `/home/scott/projects/starplayer-no-08-review-fixes`, branch
`task/no-08-review-fixes`, base `55479442f198f16be0492e8da9821f35a02762ec`. Only the task
specification and raw review/disposition file exist before implementation; the index is
empty. The ignored `target` symlink points to the task-owned shared cache
`/tmp/starplayer-no-target`; use only one build/browser stream. Do not modify the frozen
feature worktree or other task worktrees. Follow up with the coordinator if a purported
finding is disproved or a material contract conflict is discovered; record evidence.


## Implementation and verification evidence — 2026-10-28

Working branch `task/no-08-review-fixes`, base `55479442f198f16be0492e8da9821f35a02762ec`,
isolated checkout `/home/scott/projects/starplayer-no-08-review-fixes`. Changes remain
unstaged and uncommitted for coordinator source review. Historical reports are unchanged.
No cargo fmt, golden regeneration, main merge/deployment, hardware action or listening
claim. Cargo uses `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 RUST_TEST_THREADS=2`;
Node tests use `--test-concurrency=1`; Cargo/browser workloads run sequentially. The ignored
target link remains `/tmp/starplayer-no-target`. Starting disk availability was 7.4 GiB;
monitoring during the affected runs remained above 6 GiB.

### Failing-first evidence

All eight accepted findings reproduced before the corresponding production correction:

| Finding | Focused reproduction and retained exact log |
|---|---|
| ASTRA-01 | `cargo test -p starplayer-engine -p starplayer-host -p starplayer-it -p starplayer-midi -p starplayer --features note-overlap,interactive-audition no08 -- --nocapture`; `/tmp/no08-rust-before.log`: prepared XM external event0 remained undispatched through the first128 output frames |
| ASTRA-02 tracker | `cargo test -p starplayer-engine --features note-overlap,interactive-audition,telemetry --test interactive_transport no08 -- --nocapture`; `/tmp/no08-engine-integration-before.log`: restored Playing song cursor5376 instead of1152; `/tmp/no08-starplayer-host-before.log`: actual retirement-deferred fresh load song8192 instead of0 |
| ASTRA-02 SMF | `cargo test -p starplayer-midi --features smf,note-overlap,interactive-audition no08 -- --nocapture`; `/tmp/no08-smf-before.log`: delayed first attachment reports song4219 instead of123 |
| ASTRA-03 | `cargo test -p starplayer-it --features note-overlap,interactive-audition no08 -- --nocapture`; `/tmp/no08-starplayer-it-before.log`: lone sample/instrument after All clear allocated a foreground voice |
| NO-R1 | `cargo test -p starplayer-engine --features note-overlap,interactive-audition no08 -- --nocapture`; `/tmp/no08-starplayer-engine-before.log`: cadence tick stayed atframe17; configured zero interval raised zero-advance in `/tmp/no08-engine-integration-before.log` |
| NO-R2 | `/tmp/no08-engine-integration-before.log`: final snapshot remainedPlaying after a burst ending with AllSoundOff/Pause |
| NO-I1/ASTRA-04 | `cargo xtask wasm` `/tmp/no08-wasm-before.log`, then `node apps/starplayer-web/test/worklet-harness.mjs` `/tmp/no08-worklet-before.log` and `STARPLAYER_NO08_TRANSPORT=fallback node apps/starplayer-web/test/worklet-harness.mjs` `/tmp/no08-worklet-fallback-before.log`: actual packaged worklet/WASM accepted mixed valid/malformed v2 groups on each transport |
| NO-R4 | `cargo test -p starplayer-host --features note-overlap,interactive-audition --test interactive_transport no08_pause -- --nocapture`; `/tmp/no08-rewind-before.log`: natural Stop→Pause→Play remains at5376 instead of0 |
| NO-R5 | `/tmp/no08-starplayer-host-before.log`: musically invalid order999 changes real master/channel delay PCM against an independently reset no-seek reference |

Diagnostic runs are retained honestly: the first host rewind fixture used a looping song
with AtEnd::Stop, which intentionally continues looping; the settled failing regression
uses F00 termination for Stop and the loop/fade policy for FadeOut. Initial all-package
filtering stopped at the facade failure, so each remaining package was run separately.
The first MIDI-only command omitted `smf` and ran zero tests; `/tmp/no08-smf-before.log`
is the actual reproduction. The first corrected seek control read old forwarded telemetry;
reading at preparation boundaries establishes fresh voice-state assertions without changing
PCM expectations (`/tmp/no08-seek-diagnostic.log`). Build diagnostics for a missing match
comma and an unavailable direct DSP test dependency are in `/tmp/no08-affected.log` and
`/tmp/no08-affected-retry.log`; the native mixer control now lives in facade tests using
its existing dependency graph. No manifest or test expectation was weakened.

### Corrective interfaces and regression coverage

- Engine polls all sources at its current source frame before the first quantum segment.
  `advance_to` implementations were audited: tracker/scripted/silent are no-ops, wrappers
  forward, and MidiSource refreshes lookahead only. Same-frame polling does no articulation.
  Prepared XM/IT external queues assert exact dispatch0/17/18 atbase0/1024, silence before
  onset and nonzero PCM within16 frames, no late warnings, newly nonempty idle admission,
  and all six host block sizes. Live timestamps are observed unchanged.
- Tracker retains a feature-gated preparation anchor updated by clock/cursor preparation;
  its first transport attachment translates clock/origin once. Explicit interactive SMF
  uses a consumed preparation anchor and its existing lazy event translation. Subsequent
  held transitions preserve their phase. Tests cover actual retirement-queue deferral of
  stopped fresh loads and saved Playing/Paused/Stopped tracker+SMF cursors, multiple-tick
  delays, exact tracker resumed deadline and nonzero fractional remainder.
- IT's audition-only clear marker suppresses remembered-note synthesis on instrument-only
  rows until actual mapped note admission. Genuine note-only starts recover remembered
  instrument identity; native sample/instrument modes, All/Song/lane scopes, effect
  retrigger suppression, real mixer natural-completion restarts and unrelated Live/custom
  cleanup have explicit controls. Existing native/mapped/NNA suites remain required.
- `ControlClock::set_cadence` clamps below `1<<32`; valid intervals/remainders stay exact.
  Direct clock and configured engine tests prove progress/no warning. Command publication
  uses one stack-local dirty boolean per bounded drain, no Engine storage/ring policy change.
- Shared v2 drain forwards every consumed raw member to Rust atomic admission. Fallback
  emits an invalid opcode0 sentinel for malformed members, retaining every representable
  identity word, and only normalizes validated numeric words (BigInt/NaN/null are rejected
  safely). Invalid-only/mixed/order/opcode/identity/BigInt cases use actual worklet/WASM;
  valid retry, v1 and 63-event throughput remain mandatory.
- Play/Resume consumes an armed natural rewind regardless of held state. Native Stop and
  FadeOut tests cover Pause then both Play/Resume through eight further quanta; explicit
  user Stop cancellation remains strict. Actual wire/controller checks count ended once.
- Successful interactive audio-side seek validation sets a dispatch-local `EngineContext`
  reset signal, consumed by Engine before whole128-frame DSP. No Engine field, allocation,
  lock or insert-ring command is added; the context reference compiles out feature-off.
  Invalid order/row/frame keeps real song/live voice and ringing master/channel delay PCM
  identical. A reset probe proves only valid seeks reset both chains once and retain the
  same live VoiceId. v2 seek has no insert reservation; full real insert mutation still
  rejects its entire seek group, and a seek-only group can use a full insert ring.

Final verification is being recorded below as each exact command completes.


### ASTRA-03 clarification from coordinator source review

The no-resurrection contract concerns actual previously started sound. Initial setup
Stopped has no voice/note to resurrect and must preserve IT's native first instrument-only
C0 onset. Neither default C0 memory nor a merely latched instrument proves that a voice
has started. Suppression therefore needs an explicit accepted-start history, with a
three-state audition-only marker (never started / remembered start / explicitly cleared).
A real accepted voice moves to remembered; relevant clear scopes suppress only remembered
starts; a genuine subsequent note admits normally. Fresh configured canonical behavior
remains mandatory. MOD/XM initial synthetic-start paths are inspected and changed only
if the corresponding actual regression is demonstrated. This clarification tightens the
existing faithful-native/no-cleared-voice-resurrection boundary; no new product policy.

The first full CI passed all 11 jobs (`/tmp/no08-ci.log`) before this justified follow-up.
The remaining matrix was interrupted cleanly at the next active command; completed checks
remain evidence for that source. A new failing configured fresh-load regression, corrected
source and a fresh whole CI are required below before claiming final verification.


### Accepted-start refinement verification and chronology (prior to retained-memory follow-up)

The fresh initial-instrument-only configured IT control failed before the history
refinement (`cargo test -p starplayer --features note-overlap,interactive-audition
--test xm_it_overlap no08_fresh -- --nocapture`, `/tmp/no08-it-fresh-before.log`).
After the three-state refinement, `/tmp/no08-it-history-after-fixed.log` passes real
note-only recovery, prior-note clear suppression, actual natural sample completion,
fresh IT C0 equivalence and XM fresh-silence controls, and a merely latched instrument
with no accepted start. A derive-placement compile diagnostic is retained separately
in `/tmp/no08-it-history-after.log`.

Affected command at that stage: `cargo test -p starplayer -p starplayer-engine -p starplayer-host
-p starplayer-mod -p starplayer-s3m -p starplayer-mtm -p starplayer-xm -p starplayer-it
-p starplayer-midi -p starplayer-host-wasm --features note-overlap,interactive-audition`;
`/tmp/no08-affected-refined.log`: **962 tests passed in 51 result groups, exit 0**.

Full command at that stage: `cargo xtask ci`; `/tmp/no08-ci-refined-final.log`: **all 11 jobs passed,
exit 0**, including strict Clippy, RT safety, both bare-metal checks/purity, WASM, SIMD,
FMA and trace exclusion. Conformance remains261 cases / 179 passes / 0 unexpected /
16 accepted deviations / 66 known / 0 gated. All eight committed goldens remain unchanged.
The original `/tmp/no08-ci.log` pass is retained as evidence of the earlier marker tree;
it is not substituted for this final refined-source run.

After final CI exited, the focused feed test alone extended its phase duration from 128
to 8192 frames so every named host size—including 8191—runs as an actual call size.
Production Rust stayed frozen. `cargo test --offline -p starplayer --features
note-overlap,interactive-audition --test live_audition no08` passed
(`/tmp/no08-feed-real-blocks-final.log`), retaining strict frame0/17/18, initial/idle,
nonzero-install, PCM-window and no-late assertions. The final engine attachment/live
absolute-timestamp and cadence/burst checks also pass
(`/tmp/no08-attachment-live-final.log`). This test-duration-only tightening is covered
by its focused check and final matrix, without another whole CI solely for that change.

The final serial matrix command manifest is `/tmp/no08-commands.tsv`; its orchestration
is `/tmp/no08-verify-rest.py` and progress `/tmp/no08-rest-status-final.log`. Earlier
partial runs were deliberately interrupted for the IT source refinement. The embedded
resource command uses dependency feature names `starplayer/note-overlap` and
`starplayer/interactive-audition`, because the host-embedded package declares neither
feature itself. Final results below refer only to the refined production source.


NO-08 source-review clarification: a genuine IT note-only recovery after explicit clear
must resolve its remembered instrument for sample mapping without treating that memory
as an authored instrument column. Current native base volume, IFC/IFR filter memory and
passive onset-volume identity must survive; only an actual instrument change replays
sample/instrument defaults. This preserves Stop's existing effect-memory contract.


### Retained note-only memory follow-up and browser diagnostics

Coordinator source review found that early raw instrument substitution after a clear
replayed sample default volume and IFC/IFR. The focused native/sample-mode regression
failed with base volume 256 instead of 68 (`/tmp/no08-it-memory-before.log`). Mapping now
resolves inside `note_change`, keeping the actual absent-instrument path. Native no-clear
controls, post-clear volume/filter memory and passive onset identity all pass
(`/tmp/no08-it-memory-after.log`), as does the full affected command
(`/tmp/no08-affected-settled.log`). This correction follows the earlier refined CI, so
that older pass is retained prior-source evidence; another whole CI is required below.

The initial ordinary SAB browser attempt timed out at Stop after optimistic reverb
removal (`/tmp/no08-browser-ordinary-sab-before-memory-refinement.log`). No console error
or proved production defect was established. A fully instrumented ordinary SAB rerun
passed (`/tmp/no08-browser-stop-diagnostic.log`): Stop was enabled with current identity,
while removal had not yet acknowledged. Forced immediate remove→Stop also passed in
actual packaged worklet/WASM on both transports (`/tmp/no08-worklet-pending-stop.log`).
Focused Chromium tests exercise Stop after 0/1/5/10/20/50ms of pending preparation with
strict Stopped and frozen-row assertions on both SAB and fallback
(`/tmp/no08-browser-stop-stress-{sab,fallback}.log`). A one-second diagnostic window
failed the unchanged pattern-progress assertion; the five-second diagnostic preserves
that assertion. Ordinary final modes retain their original 60-second render window.

The ordinary None round-trip now proves the accepted removal for its actual request and
current session/build before the separate normal Stop check. Pending-state Stop coverage
remains independent and awaits no removal acknowledgement. Test-only diagnostics capture
enabled state, request/identity, wire commands and applied acks on failure. The original
timeout remains a diagnostic, not a hidden pass or a claimed production fix. No production
JavaScript was changed for it.


### Settled Rust verification and first web pass

The settled Rust production hashes remain unchanged after `cargo xtask ci` in `/tmp/no08-ci-settled-final.log`: **all 11 jobs passed, exit 0**,
with unchanged 261/179/0/16/66 conformance and all eight committed goldens. The settled
affected suites pass **963 tests** (`/tmp/no08-affected-settled.log`). The prior two full
CI logs remain evidence of their earlier marker variants and are not substituted here.

Final matrix orchestration: `/tmp/no08-verify-rest.py`; exact commands:
`/tmp/no08-commands.tsv`; final-only results: `/tmp/no08-rest-status-settled-final.log`.
Every command uses the serialized resource limits declared above. The final matrix
reruns completed prior-tree checks after the memory correction.

| Final check | Exact command family / retained final evidence | Result |
|---|---|---|
| Independent tests | `cargo test --offline -p PACKAGE --no-default-features [--features FLAGS]`, PACKAGE core/engine/host/IT; MIDI adds `smf`; off/overlap/audition/both; `/tmp/no08-{PACKAGE}-{PROFILE}-test.log`, `/tmp/no08-smf-{PROFILE}-test.log` |20 isolated package suites pass, including actual audition-only IT memory and SMF attachment tests |
| Independent graphs | `cargo {check,tree} --offline -p PACKAGE --no-default-features [--features FLAGS]`; tree adds `-e features,no-dev`; `/tmp/no08-{PACKAGE}-{PROFILE}-{check,tree}.log` |48 checks and 48 trees pass; all 12 off trees exclude both optional flags (`/tmp/no08-off-trees-audit.log`) |
| Portable | `cargo check --offline -p starplayer --no-default-features --features mod,s3m,mtm,xm,it,midi,smf,telemetry[,FLAGS] --target TARGET`; `/tmp/no08-portable-{PROFILE}-{TARGET}.log` |12 checks pass across riscv32imc-unknown-none-elf, riscv32imac-unknown-none-elf and wasm32-unknown-unknown |
| Exact resources | `cargo test --offline -p starplayer-host-embedded --no-default-features [--features starplayer/FLAGS] --test render_allocation`; `/tmp/no08-budget-{PROFILE}.log`; isolated core+telemetry `/tmp/no08-off-layout.log` |All four profiles pass five strict budget/RT assertions each; exact feature-off layout retained |
| Feature unification | `cargo run --offline --manifest-path crates/starplayer-{engine,host}/tests/{core_feature_unification,feature_unification}/Cargo.toml`; corresponding `/tmp/no08-*_unification.log` |Both deliberate unification fixtures pass without implicit runtime preparation |
| Canonical and timing PCM | `STARPLAYER_NO07_ARTIFACTS=/tmp/no08-artifacts python3 xtask/no07-feature-matrix.py`; `/tmp/no08-pcm-matrix.log` and `/tmp/no08-artifacts/test-*.log` |All six isolated profiles pass; five formats, all six actual host sizes, fixed and float schedules; canonical pairs agree |
| Goldens | `cargo xtask goldens --check` `/tmp/no08-goldens.log`; `cargo run --offline --manifest-path xtask/no07-artifacts/Cargo.toml -- /tmp/no08-artifacts/owner` `/tmp/no08-prepared-goldens.log` |Eight committed hashes unchanged; all 24 prepared canonical hashes unchanged; no regeneration |
| JavaScript | `node --test --test-concurrency=1 apps/starplayer-web/test/*.test.mjs`; `/tmp/no08-js.log` |344 pass |
| Packaged web | `cargo xtask wasm` `/tmp/no08-wasm.log`; `cargo test -p starplayer-web` `/tmp/no08-web.log` |Final settled source packages successfully;22 web tests pass |
| Actual wire/audio | `node apps/starplayer-web/test/{ring,worklet}-harness.mjs`; `/tmp/no08-{ring,worklet}.log` |SAB/fallback pass, including malformed groups, BigInt/identity rejection, invalid-only groups/retry, v1,63-event throughput, final burst state, exactly-once ended and exact delay/voice seek PCM |
| Pending-preparation Stop | `node apps/starplayer-web/test/headless.mjs --mode MODE --seconds 5 --transport-only`; `/tmp/no08-browser-stop-stress-{sab,fallback}.log` |Both transports pass 0/1/5/10/20/50ms phases, enabled Stop and strict frozen-row assertions; normal round trip correlates actual accepted removal request |

All final full Chromium modes passed on Chrome/156.0.8078.4, without skipping or
weakening the existing playback/library/transport assertions:

| Browser check | Exact command / final evidence | Result |
|---|---|---|
| Ordinary | `node apps/starplayer-web/test/headless.mjs --mode MODE`; `/tmp/no08-browser-ordinary-{sab,fallback,plain}.log` |All three modes pass with the default 60-second playback, all W15/remote/library/playlist groups and strict transport/insert readiness |
| Interactive | `STARPLAYER_NO07_ARTIFACTS=/tmp/no08-artifacts node apps/starplayer-web/test/headless.mjs --mode MODE --interactive-only`; `/tmp/no08-browser-interactive-{sab,fallback,plain}.log` |All three modes pass, including recovered controls after failure, audition/held transport/rebuild, owner fixtures and dense 64-channel IT |

The final serial orchestration exits0; every recorded final step exits0. No Rust production
hash changed after the final whole CI; the later JavaScript opcode guard has separate
post-guard package/browser evidence below. `git diff --check` passes, the index is empty,
and the diff remains deliberately unstaged/uncommitted. No main merge, deployment,
archive move, historical-source edit, golden regeneration or hardware action occurred.
Disk ended around 6.0 GiB available; no cache deletion was needed. No Cargo/browser process
remains after the final stream exits.

Artifacts and owner WAVs/browser fixtures are under `/tmp/no08-artifacts`. Subjective
listening, physical MIDI, actual phone/device behavior and firmware execution remain
owner acceptance, not inferred from browser automation or portable compile checks. The
historical 66 known conformance failures and 16 accepted deviations remain separately
reported; no new unexpected conformance failure was introduced.


First corrective review handoff, before NO08-A1: **18 code/test files**. Sorted file manifest
`/tmp/no08-code-test-sha256-before-opcode.txt` has SHA-256
`7fe9c3dba6cb861d2125413e6ad359938e511a5a740931d3b231798b429ac9bb`. The exact unstaged code/test diff is
`/tmp/no08-code-test-before-opcode.patch`, SHA-256
`e9f058c636700675754e9ec0ec4f1512b1d8f5c2292b495d654a47ba4b41dbda`. Complete changed-path inventory is
`/tmp/no08-changed-paths.txt`; it includes coordinator-seeded planning changes.
Independent corrective reviewer acceptance is still pending and is not claimed here.


### Coordinator regression — 2026-10-28

The coordinator independently ran `cargo test --offline -p starplayer-engine -p starplayer-host
-p starplayer-it -p starplayer-midi -p starplayer -p starplayer-host-wasm --features
note-overlap,interactive-audition,telemetry,midi,smf no08` with two build jobs, no incremental
compilation and two test threads: **18 tests passed in 29 result groups, exit 0**,
`/tmp/starplayer-no08-coordinator.log`. The frozen18-file source/test manifest matches
the worker handoff at that checkpoint. The later JS-only NO08-A1 change is verified
separately below; all Rust hashes remain identical. Reviewer follow-ups were pending
at that checkpoint and are now closed in the final disposition.


### Corrective review follow-up NO08-A1

Astra's medium-effort source follow-up verified all18 handoff hashes and resolved seven
corrections, but found that fallback opcodes14.5, "14" and14n can pass relational range
checks and be coerced/throw at WASM admission. The coordinator accepts this remaining
malformed-opcode gap within NO-I1/ASTRA-04. Require integer/type validation before
forwarding, with direct and actual-worklet failing-before/pass-after coverage for invalid
only and both mixed orders, exactly one rejected ack/current identity, no partial mutation
or exception, and a successful valid retry. Preserve v1 and 63-event throughput.

Rust production is unchanged by this bounded JavaScript correction, so the settled
11-job Rust CI/feature/resource/PCM evidence remains applicable; do not repeat it solely
for a JS guard. Rerun final JS, packaged WASM/web, ring/worklet and relevant sequential
browser checks. Explicitly distinguish final Rust verification before this JS guard from
final JS/package/browser verification afterward. Refresh manifests/source hashes and
return unstaged/uncommitted for focused Astra closure. Opus's full initial reviews were
complete, but its corrective pass hit the five-hour limit, reported reset12:20Perth;
that closing attempt is not accepted and no model substitution is authorized.


### NO08-A1 implementation and failing-first evidence

Astra's remaining opcode-scalar finding reproduced before the guard:

- `node --test --test-concurrency=1 --test-name-pattern=NO08A1 apps/starplayer-web/test/interactive-wire.test.mjs` failed all three opcode types. `/tmp/no08-a1-unit-before-all-orders.log` records direct-drain forwarding of 14.5, "14" and 14n in invalid-only and both mixed orders, rather than invalid sentinels.
- `node apps/starplayer-web/test/worklet-harness.mjs`, `/tmp/no08-a1-worklet-before.log`, ran all nine actual packaged fallback/WASM cases. Float/string opcodes became accepted Pause actions; both mixed orders also applied the valid overlap change. BigInt threw `TypeError: Cannot convert a BigInt value to a number`; in valid-before-invalid order the staged overlap change later applied and acknowledged accepted. Invalid-only/invalid-before BigInt produced no rejection acknowledgement.

The sole production correction is an integer/supported-range guard at the start of
`validInteractiveArguments`. `Number.isInteger` rejects non-Number and fractional values
before relational comparisons; the supported 13–24 range remains explicit. Fallback sends
them through the existing opcode-zero invalid-member path, retaining representable
identity and group rejection. Legacy validation/v1 semantics and the bounded shared
drain are unchanged. No Rust source/test, manifest, DSP or timing change was made.

The focused direct suite passes 10 tests (`/tmp/no08-a1-unit-after.log`). Actual packaged
worklet coverage checks each malformed type alone/before/after a valid current-identity
overlap command: no exception/fault, exactly one rejected acknowledgement with current
identity, no Pause or partial overlap mutation, then one accepted valid retry.
The final existing v1, shared integer and 63-event throughput assertions remain strict.

All Rust file hashes were checked against `/tmp/no08-code-test-sha256-before-opcode.txt`
before this follow-up's verification. The settled 11-job Rust CI in
`/tmp/no08-ci-settled-final.log`, Rust feature/resource/portable/PCM matrices and 963-test
Rust regression results precede this JS guard and remain evidence for unchanged Rust.
They were not rerun solely for the JavaScript predicate. Final JS/package/browser
commands after the guard are logged separately in `/tmp/no08-a1-commands.tsv`, with
orchestration `/tmp/no08-a1-verify.py` and progress `/tmp/no08-a1-status.log`.

The prior manifest/patch remain `/tmp/no08-code-test-sha256-before-opcode.txt` and
`/tmp/no08-code-test-before-opcode.patch`; the prior production manifest is preserved as
`/tmp/no08-production-sha256-before-opcode.txt`. Final follow-up results and refreshed
hashes are recorded after the remaining sequential browser modes finish. Astra closure
and the quota-interrupted Opus corrective acceptance remain unclaimed.


### NO08-A1 final post-guard verification and handoff

All **13** post-guard steps exit 0 (`/tmp/no08-a1-status.log`), serialized with the same
two-job/no-incremental/two-test-thread Cargo limits and Node test concurrency 1:

| Check after the JavaScript guard | Exact command / final log | Result |
|---|---|---|
| Full JS | `node --test --test-concurrency=1 apps/starplayer-web/test/*.test.mjs`; `/tmp/no08-a1-js.log` |347 tests pass |
| Package | `cargo xtask wasm`; `/tmp/no08-a1-wasm.log` |Final guard is present in packaged worklet |
| Web | `cargo test -p starplayer-web`; `/tmp/no08-a1-web.log` |22 tests pass |
| Wire and actual worklet | `node apps/starplayer-web/test/{ring,worklet}-harness.mjs`; `/tmp/no08-a1-{ring,worklet}.log` |All existing SAB/fallback/v1/63-event assertions pass, plus all nine malformed opcode type/order cases, exactly one rejected ack/current identity/no exception/no partial state/valid retry |
| Pending Stop | `node apps/starplayer-web/test/headless.mjs --mode MODE --seconds 5 --transport-only`; `/tmp/no08-a1-browser-stop-stress-{sab,fallback}.log` |Both transports retain all six pending-insert timing phases and strict held-row assertions |
| Ordinary Chromium | `node apps/starplayer-web/test/headless.mjs --mode MODE`; `/tmp/no08-a1-browser-ordinary-{sab,fallback,plain}.log` |All three modes pass with default 60-second playback, complete existing library/playlist/W15 checks, strict Stop and exact accepted-removal request/current-identity precondition |
| Interactive Chromium | `STARPLAYER_NO07_ARTIFACTS=/tmp/no08-artifacts node apps/starplayer-web/test/headless.mjs --mode MODE --interactive-only`; `/tmp/no08-a1-browser-interactive-{sab,fallback,plain}.log` |All three modes pass |

This is JavaScript/package/browser verification **after** the opcode guard. The settled
11-job Rust CI, 963-test affected Rust run and isolated Rust matrices are **before** the
JS guard; all Rust source/test hashes still match their reviewed handoff, so that Rust
evidence remains applicable without claiming a new full CI run. The only three code/test
files changed since that handoff are `www/ring.js`, `test/interactive-wire.test.mjs` and
`test/worklet-harness.mjs` under the web app.

Refreshed 18-file manifest `/tmp/no08-code-test-sha256.txt` SHA-256:
`ae781b908dd3fc82bff38cb1c152d715229e5547d239f235a0e922879ff99f83`. Exact full code/test diff
`/tmp/no08-code-test.patch` SHA-256:
`09b03327d6c72fbec225251d4df4357168529a0b4b4e0ff03026b668319cd923`. Production hashes are refreshed in
`/tmp/no08-production-sha256.txt`; complete changed-path inventory is
`/tmp/no08-changed-paths.txt`. Prior artifacts retain the `before-opcode` names above.

All changes remain unstaged/uncommitted, with an empty index and unchanged base HEAD.
No Rust edits, cargo fmt, golden regeneration, historical-source edits, main merge,
deployment or hardware actions occurred in this follow-up. Disk remains about 6.0 GiB
available. The sole build/browser stream has exited and is released for Astra closure.
Astra's final closure and Opus's quota-interrupted corrective acceptance are not claimed.


Coordinator NO08-A1 verification: `node --test --test-concurrency=1
apps/starplayer-web/test/interactive-wire.test.mjs` exited0
(`/tmp/starplayer-no08-wire-coordinator.log`). The coordinator independently verified
all final18 source/test hashes and that only ring.js plus its two JS test files changed
since the first Astra corrective review; all Rust hashes are identical. The earlier
18-test coordinator Rust regression therefore remains applicable.


### Astra final source acceptance — 2026-10-28

Astra's requested medium-effort final follow-up verified the refreshed18-file manifest
`ae781b908dd3fc82bff38cb1c152d715229e5547d239f235a0e922879ff99f83` and code/test patch
`09b03327d6c72fbec225251d4df4357168529a0b4b4e0ff03026b668319cd923`, closed NO08-A1
and accepted all eight corrections with no new concrete defect. The verbatim final report
is in [the review record](NO-branch-review-2026-10-28.md); this was source-only review.
Opus's initial full and supplemental reports completed, but its corrective follow-up was
quota-interrupted and is not yet accepted. The reported reset is12:20Perth.


### Coordinator fresh embedded compile checks — 2026-10-28

The changed core was additionally checked in the separate firmware workspace while
Opus's provider quota reset was pending. All checks ran serially, after task/reviewer
streams released, with two Cargo jobs/no incremental compilation, offline dependencies
and two test threads. `embedded/target` is an ignored symlink to the new task-owned
`/tmp/starplayer-no08-embedded-target`; other worktree targets were untouched.

- `cargo +1.97 test --offline --manifest-path embedded/Cargo.toml -p
  starplayer-firmware-common --target x86_64-unknown-linux-gnu`: **96 tests passed**,
  `/tmp/no08-embedded-common.log`. Its exact feature graph excludes both optional flags.
- From `embedded/`, `. /home/scott/export-esp-1.97.sh` then `cargo xtask build --board
  a1s`: **production/default release link passed** on esp-1.97/Xtensa,
  `/tmp/no08-embedded-a1s.log`.
- From `embedded/`, `cargo xtask build --board c5 --features bench`: **actual renderer
  release link passed** on1.97/RISC-V, `/tmp/no08-embedded-c5.log`.

Exact common/board `cargo +TOOLCHAIN tree --offline --manifest-path embedded/Cargo.toml
-p PACKAGE --target TARGET [--features bench] -e features,no-dev` results are in
`/tmp/no08-embedded-{common,a1s,c5}-tree.log`; all exclude note-overlap and
interactive-audition. `/tmp/no08-embedded-results.json` records exact commands/exits.
`file` confirms32-bit static Xtensa and RISC-V ELFs. All final18 source/test hashes remain
unchanged; no tracked firmware configuration/source changed. Only ignored build/assets
were generated. No flash, erase, monitor, device execution, new device timing/heap
measurement, physical MIDI or subjective listening occurred.


### Final coordinator acceptance — 2026-10-28

Opus's exact5.5/high closing retry succeeded after the reported quota reset (exit0,19
turns, no permission denials). Both final source reviewers accept all eight corrections
with no new concrete defect; [verbatim reports and dispositions](NO-branch-review-2026-10-28.md)
retain the initial quota failure, browser diagnostic, coverage limits and baseline
observations. All earlier pending/partial statements above are historical checkpoints.

Coordinator source/contract review,18 targeted Rust tests,10 wire tests, raw-report
integrity and current18-file hashes all pass. The available automated verification is
complete; task branch commit and single-change feature squash are authorized. No code/test
change follows the final reviewed manifest. Only plan status/landing records may change.
Owner listening, real MIDI, phone/device behavior and firmware execution/timing remain
pending; no main merge, deployment or archival is performed.


### Coordinator feature landing — 2026-10-28

Reviewed task commit `20dbc07` lands as one change by squash from the feature branch's
own checkout. The code/test patch and final manifest are identical to both accepted
review snapshots; only plan status and landing evidence changed after the task commit.
All accepted findings are closed. Historical diagnostics and source-review limits remain
recorded above and in the raw reports. No main merge/deployment or plan archival occurs.
Owner listening, physical MIDI, phone/device acceptance and firmware execution/timing
remain explicitly pending.
