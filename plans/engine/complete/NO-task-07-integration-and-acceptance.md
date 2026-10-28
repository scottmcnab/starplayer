# NO-07 — Integration, determinism and feature exclusion

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Landed on main via e031eda (2026-10-28); implementation and automated verification complete; owner listening/device acceptance pending |
| Depends on | NO-W1 and NO-01–06 |
| Blocks | Feature landing; prepares owner listening acceptance |
| Parallel with | None; integration verification uses final combined tree |
| Least-capable safe worker | `gpt-6.1-sol` (high) |
| Coordinator | `gpt-6.1-sol` (xhigh) |
| Verified by | Worker runs Verification; coordinator reviews diff/evidence; owner listening where specified |

## Context for a fresh agent

Read `AGENTS.md`, the linked master (the behavioral contract), and product architecture
and accuracy policy before editing. No historical source edits, cargo fmt, broad staging,
commit/PR attribution, render allocations/locks/panics or weakened determinism tests.
Use a task branch and sibling worktree. This file plus the master is the handoff; no
conversation context is required. Resolve material contract changes with the coordinator
and amend the master/foundation documents before implementing them.

Read the master and every task verification record. This task proves the integrated
feature, not just each subsystem independently. Root CI excludes embedded; hardware
listening and embedded execution cannot be inferred from a successful root build.

## Deliverables

- Add end-to-end deterministic fixtures spanning all formats, instrument checkbox/global states, transport
  and timed audition. Include non-looping/mixed-loop XM/IT and native IT backgrounds.
- Exercise unchecked/global-bypass and feature-off canonical equivalence. Include equal
  note/equal-volume replacement, different-volume echo overlap, equal-volume echo limitation,
  velocity variation, onset volume commands/delays, later effects not changing identity
  and instrument independence across channels. No channel policy UI/API remains. Matrix neither,
  each feature alone, both; telemetry on/off and relevant fixed/float configurations.
- Verify paused/stopped clocks, wide-channel routing, pool caps and IT slot bounds, stale
  generation commands, rebuild lifecycle and same-channel duplicate semantics.
- Extend RT guard coverage to active overlap, control-only articulation and live notes.
  Record measured resource/capacity overhead; prove no optional state/reservations are
  constructed in feature-disabled embedded paths rather than merely hiding UI.
- Run documented CI/web checks and inspect embedded feature graph. Use installed board
  toolchains for compile checks only; do not flash devices or change firmware config.
  Report missing toolchains and device/listening checks as outstanding evidence.
- Write integration evidence with exact commands/results/base commit and known failures;
  no fabricated pass counts or conformance fixes hidden in fixture changes.
- Reconcile task/master/foundation status and narrow M11 integration notes. Do not archive
  until landing; owner listening can remain explicitly outstanding after code lands.

## Evidence ownership and feature exclusion

Consume every task's interface and verification record; own final integrated evidence.
Feature-off proof uses isolated per-package `-p ... --no-default-features` invocations
with neither/each/both flags and `cargo tree -e features`; workspace CI alone unifies
host-wasm defaults and is not proof. In the embedded workspace run
`cargo test -p starplayer-firmware-common` with its documented host toolchain in addition
to available board compile checks. Verify active capacity N+C+L, physical N+C+L+R, legacy runtime defaults under feature
unification, and fixed live lanes 64–80 alongside maximum-width tracker modules.

Include retained fade/seek/rebuild transport, SMF vs live pause, frozen progress, live
cadence, quota-domain isolation, XM Gxx background ordering, generation-envelope/ring
negotiation and paused telemetry in integrated tests. Produce browser fixtures and
renders and perform available automated/smoke checks. The listening matrix is an owner
acceptance checklist, not a worker completion gate; report owner/device evidence pending
honestly. No worker is asked to fabricate subjective listening acceptance.

Run all five formats with identity recording active in three from-start canonical pairs:
global enabled/all unchecked; bypassed/all checked; enabled/only wholly looping or otherwise
ineligible instruments checked (including authored IT NNA). Compare to feature-off. Add
mid-song enable-after-unchecked/bypass and scoped live/song bypass cleanup separately;
these do not promise retrospective canonical voice history. Include onset fine slides,
tremolo/tremor exclusion and live pre-CC7 velocity identity in the integrated fixtures.


## Research points

- Confirm test scheduling uses absolute frames, not host render calls, so buffer-size
  checks actually prove the invariant. Keep all required sizes, especially 8191.
- Compare code/feature graph with capacity docs and review logs; investigate discrepancies
  before accepting aggregate green CI.
- Inspect existing conformance baseline and unrelated failures separately from new ones.

## Verification

- Required sizes: 1, 3, 64, 128, 4096, 8191 with byte-identical outputs per fixed schedule.
- `cargo xtask goldens --check`, `cargo xtask ci`, current web suite and WASM build;
  feature-matrix crate tests and bare-metal/no-std checks per existing tooling.
- Prepare the owner browser listening matrix and available smoke checks: percussion snare→kick, piano C→G→C, mixed-loop XM,
  dense IT, Stop/Resume and audition with master inserts. Include fallback transport.
- Record browser, device availability and exact remaining owner acceptance items.
- Review final diff and explicit staged paths; no unrelated formatting or golden churn.

## Out of scope

New features, changing expected canonical audio to get green tests, claiming hardware
validation from compilation, implementing all M11, or merging without coordinator review.

## Implementation and verification evidence — 2026-10-28

Worktree `../starplayer-no-07-integration`, branch `task/no-07-integration`, combined base
`dd838b34fcd461c27f0eade099e4244a01378d1d`. Evidence below describes the worker diff before
coordinator review/commit. Main, deployment and owner listening/device acceptance remain
separate. All Cargo commands used `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0`; Rust tests also
used `RUST_TEST_THREADS=2`. Node test runs used `--test-concurrency=1`. Builds and Chromium
modes ran sequentially; no historical assembly, committed golden or firmware configuration
was changed.

### Integrated coverage and existing evidence boundaries

`crates/starplayer/tests/no_integration.rs` adds three combined tests, using native pattern
bytes and format processors rather than lowering formats. The five-format canonical test
renders 24,576 frames at 8 kHz for block sizes **1, 3, 64, 128, 4096, 8191**. It compares
all three fresh prepared states (enabled/unchecked; bypassed/checked; enabled/ineligible-only
checked) against independently compiled neither-feature PCM, and checks enabled overlap
actually changes audible PCM. The runner `python3 xtask/no07-feature-matrix.py` first writes
neither-feature PCM and verifies its feature tree, then checks overlap-only, audition-only,
both, telemetry/fixed-mix and telemetry/float-mix configurations. This is cross-build
exclusion evidence, not a runtime-off comparison in a feature-unified binary.

The combined schedule uses absolute event frames and command deadlines, including non-quantum
MIDI frames, equal/different pre-CC7 velocities, a later CC7 change, all live lanes 64–79,
mid-song selection/enable, scoped bypass, Preview, Pause/Resume, Stop/Play, StopPreview and
AllSoundOff. It runs native 64-channel song and prepared MidiSource together in every format.
Both actual `FixedPath/StereoI16` PCM and actual `FloatPath/StereoF32` bit patterns agree at
all six block sizes. Float is compared with itself across sizes, not with fixed PCM.
Mixed maps, fully looping/native IT NNA, onset fine slides and delayed starts are present.

The separate full-pressure test drives real `ItProcessor` rows and actual prepared MidiSource
through **256 N + 64 C + 16 L** active voices with all **339 physical slots**, including
slot 338. Pressure/retirement reaches physical ownership maxima N257/C65/L17 without
cross-domain stealing. The runtime's existing voice ownership remains the sole authority;
the fixture observes it through EngineContext, without a second ownership table.

NO-01–06's recorded fine-grained tests remain the evidence for exact identity retention to
slot reuse, retiring exclusion, provenance independence, native effect memory, tremolo/tremor
exclusion, XM Gxx background ordering, SMF pause/rebuild, fractional cursor clocks and
legacy-constructor behavior. This task adds integration at their boundaries rather than
claiming every subsystem assertion is newly duplicated. The existing host allocation guard
now renders overlapping native XM/IT song notes alongside prepared live pressure, held
control-only articulation and resumed playback; allocator/deallocator hooks remain strict.

### Source-backed defects resolved within the existing contracts

- Actual WASM/native queue tests reproduced partial v2 batch application: one accepted bit
  could reject a mixed stale/valid group after valid controls had already mutated. A fixed
  64-record preflight now reuses Player's pure validation and reserves separate Host,
  Engine, DSP-insert and live-event capacity before any group mutation. MIDI scalar/state
  validation is shared with actual execution; 63 live-only records still enter in one
  batch. Seek uses its actual cost (zero Host commands and one DSP ResetAll) and requires
  an applied tracker/scan before admission. Invalid seek mixed with enable cannot mutate
  state; queued insert pressure rejects without changing seek mailbox/shadows, then valid
  retries apply.
- Source inspection showed Host can drain 64 while Engine and inserts drain the shared
  32-command quantum budget. Headroom includes pending load expansion, queued/held Host
  work and observed Engine backlog. The applied barrier additionally waits for the real
  Engine/insert queues and held Host work to finish. Tests prove actual song audio continues
  after rejected Pause, and a legacy Engine backlog cannot publish an applied barrier
  before its final zero gain affects PCM. No render allocation or per-record barriers.
- Failed barrier queueing now preserves requested identity. Mixed stale/current rejected
  groups prefer a current member's installed identity in either order; wholly stale groups
  remain stale and cannot roll back current controls or clear current live input. Actual
  WASM and VM regressions cover normal and barrier-failure paths and valid retry.
- Same-node rapid explicit loads ignore valid superseded acknowledgements for readiness
  and metadata. Stop after installation cancels autoplay; if installed identity differs
  from retained metadata, controls stay disabled and rebuild bytes clear until reload.
  Newer loads retain authority. Node tests cover both states, not only absence of Play.
  A scripted load-option event during a disabled fresh load now restores that request's
  actual posted options before capture/revision mutation; old retained bytes cannot use
  the newer session's cursor. Ordinary Chromium tests assert the fresh six-channel
  acknowledgement generation, actual audio-channel telemetry and metadata together.
  A separate posted-decode failure reproduced lost readiness despite old audio surviving.
  Recovery requires unchanged pre/post control identity matching accepted audio-applied
  identity/capabilities plus latest request/prior-ready/retained-metadata checks. Failure
  handlers rebase views after possible memory growth; partial installations get no old
  identity proof. Actual WASM tests cover failed decode, growth-before-failure and queued
  new installation with replay preparation failure; UI VM tests cover missing/mismatched
  proof. Ordinary malformed files fail earlier in page inspection and retain control
  usability; the interactive browser injects a later audio decode failure after valid
  inspection and requires accepted checkbox retries followed by actual Preview audio.
- The old ordinary panning test waited for note-derived pans before asserting restored
  order. Its fixture only triggers row zero; the reviewed full-cursor/fresh-processor
  rebuild resumes row one without replaying previous voices, so pans may arrive only
  after natural wrap. The test now strictly asserts the first playing snapshot of the
  replacement generation preserves forward-seek order before waiting for exact pans,
  then seeks order one again for the stopped-rebuild check. A native regression retains
  five capture phases and checks 80 subsequent quanta. No cursor implementation changed.
  Speed-bias waits likewise use acknowledged replacement generations instead of transient
  applied status text that a garbage-collection message can replace after 100 ms. Strict
  rendered speeds 5→7→1→5 and persistence/progress assertions remain.
- Ordinary live-input acceptance found the v2 UI never populated its lead display: only
  legacy midiInputApplied carried it. The off-render ready message now reports the
  existing WASM scheduling lead, stored per node and copied when candidate graphs install.
  Actual lead remains 256 frames, converted at that graph's rate; VM, actual WASM and
  browser assertions retain exact values, including fallback batching disclosure.
  MediaSession Pause acceptance now requires paused playback state, frozen row/progress
  and a continuing silent keep-alive element, preserving voice/live/DSP execution.
  The former stopped/paused-element expectation described legacy Stop behavior.
- Full ordinary library acceptance found cross-Host graph rebuild omitted effective
  at-end/fade replay (native Player rebuild already preserves it). A fresh WASM Host
  silently inherited Continue and wrapped instead of advancing the playlist. Existing
  hostSettings now replay the existing AtEnd opcode with candidate-rate fade frames
  before the final applied barrier. The actual-WASM regression proves natural-end Stop;
  removing only that replay line in an external negative-control harness reproduces the
  failure (`/tmp/no07-end-policy-negative.log`). The browser now strictly waits for its
  near-end seek to apply before suspending animation frames, preserving the actual
  background playlist-advance acceptance instead of relying on a queued seek.
- A deferred-candidate VM reproduced accepted Let-notes-ring changes overwritten by an
  older capture. Stateful page command/prepared-insert admission now advances a session
  settings revision; both graph candidates abort on change, while same-node capture
  retries. Unchanged candidates still install; MIDI/Preview do not advance this revision.
  Pending current-node optional controls disable and restore scripted unsent checkbox,
  global, keyboard/destination changes without changing transport intent forwarding.
- Native interactive frame seeks reject points beyond scanned music, while the looping
  slider also displays fade output time. The shared UI seek boundary now clamps known
  100%/fade-zone frame requests to `songLengthFrames - 1`; it leaves unknown-length and
  row/order requests unchanged. Focused shared/fallback VM cases cover those boundaries.
  The original PETRI fade-zone request remains in real Chromium, followed by unchanged
  strict fade, user Stop, natural end and rewind acceptance. Core seek policy is unchanged.
- Initial exact heap tests exposed a genuine optional Command payload (512 bytes per
  64-entry queue) and unconditional live summary storage. Both are feature-gated, with
  engine-off/direct-core-feature exhaustiveness tested in an isolated graph. Tiny shared
  transport flags remain permitted; no optional ownership/rack/reservation is constructed
  in the neither-feature path. Exact allocation equality is preserved, not rebaselined
  to an empirical delta.

### Commands and results

| Check | Command / evidence | Result |
|---|---|---|
| Full root CI | `cargo xtask ci`; `/tmp/no07-ci-clean.log` | **11 jobs passed, exit 0**: host-tests, pinned conformance, rt-safety, goldens, simd, fma-check, trace-zero-cost, wasm-build, no-std-check, clippy, no-std-purity |
| Canonical baseline | CI conformance at pinned `6ec0ba21b1b28f91e22b68a51d59207c6bbf6139` | 261 cases; 179 pass, 0 unexpected, 16 accepted deviations, 66 known, 0 gated; unchanged |
| Committed goldens | `cargo xtask goldens --check`; `/tmp/no07-goldens-check-final.log` | All eight unchanged; no writes |
| Prepared committed goldens | `cargo run --offline --manifest-path xtask/no07-artifacts/Cargo.toml`; `/tmp/no07-owner-artifacts-final.log` | All eight inputs × three fresh prepared states = 24 unchanged hashes; exact offline 44.1 kHz/10 s/MonoI16/Clamp/scanned-quirk pipeline, REFLEX Linear/Cubic/Sinc included |
| Separate-build PCM | `python3 xtask/no07-feature-matrix.py`; `/tmp/no07-pcm-matrix.log` and `/tmp/no07-artifacts/test-*.log` | Six isolated configurations pass; all five formats and six block sizes |
| Package feature graphs | `cargo {check,tree} --offline -p PACKAGE --no-default-features [--features FLAGS]`, tree adds `-e features,no-dev`; `/tmp/no07-package-matrix.log`, `/tmp/no07-starplayer-*-{off,overlap,audition,both}-{check,tree}.log` | 12 packages × neither/each/both = 48 checks + 48 trees; core/engine/facade/five formats/MIDI/host/host-wasm/web; neither trees exclude optional flags |
| Package tests / portable | `cargo test --offline -p {core,engine,host} --no-default-features [--features FLAGS]`; facade `cargo check --offline -p starplayer --no-default-features --features mod,s3m,mtm,xm,it,midi,smf,telemetry[,FLAGS] --target TARGET` | 12 package suites and 12 target checks pass; TARGET = riscv32imc, riscv32imac, wasm32; `/tmp/no07-portable-*.log` |
| Late wire validation matrix | `cargo test -p starplayer-host-wasm --features note-overlap,interactive-audition` plus neither/each `cargo check --no-default-features`; `/tmp/no07-seek-final.log`, `/tmp/no07-seek-{off,overlap,audition}.log` | Final seek/admission branches pass before clean CI |
| Direct core feature unification | `cargo run --offline --manifest-path crates/starplayer-engine/tests/core_feature_unification/Cargo.toml`; `/tmp/no07-core-unification.log` | Engine neither + core overlap compiles, safely reports unsupported, keeps default channels/voices/live base |
| Exact off layout / allocation | isolated core+telemetry tests; host-embedded `--test render_allocation` neither/each/both; `/tmp/no07-off-layout.log`, `/tmp/no07-budget-*-final.log` | Command16 and Snapshot2624 off guards; five exact allocator/RT tests per profile pass |
| Final packaged WASM | `cargo xtask wasm`; `/tmp/no07-wasm-settled.log` | Packaged final production JS/WASM pass |
| Actual WASM wire | `node apps/starplayer-web/test/ring-harness.mjs`, then `node apps/starplayer-web/test/worklet-harness.mjs`; `/tmp/no07-ring-settled.log`, `/tmp/no07-worklet-settled.log` | SAB/fallback command wire and actual rendered audio pass, including group rejection/identity/pressure/retry/high-rate MIDI |
| Node behavior | `node --test --test-concurrency=1 apps/starplayer-web/test/*.test.mjs`; `/tmp/no07-js-settled.log` | 343 tests pass; final settings/seek/readiness guards included (`/tmp/no07-js-settled.log`) |
| Final web package | `cargo test -p starplayer-web`; `/tmp/no07-web-settled.log` | 22 tests pass, including source/ABI checks on final app source |
| Ordinary Chromium | `node apps/starplayer-web/test/headless.mjs`; `/tmp/no07-browser-ordinary-settled.log` | SAB/fallback/plain pass sequentially: 60-second playback, panning/seek/speed/rebuild/media controls, all W15 groups, remote library, playlist-store and full library/end-policy/persistence cases |
| Exact real off command stride | isolated facade neither-feature tree then `STARPLAYER_NO07_PCM=/tmp/no07-artifacts/canonical cargo test --offline -p starplayer --no-default-features --features mod,s3m,mtm,xm,it,midi,smf --test no_integration`; `/tmp/no07-off-{tree,stride}-final.log` | Exact `Command<Arc<Module>>`16 guard and five-format/six-block canonical comparison pass; intentional direct-core-only unification remains separate |
| Interactive Chromium | `STARPLAYER_NO07_ARTIFACTS=/tmp/no07-artifacts node apps/starplayer-web/test/headless.mjs --interactive-only`; `/tmp/no07-browser-interactive-settled.log` | SAB/fallback/plain pass sequentially, including post-inspection audio failure/recovered actual controls, owner fixtures and dense 64-channel IT |

The 11-job CI pass is on the settled Rust production implementation; subsequent JavaScript
integration corrections are covered by the final 343-test Node suite, packaged WASM/wire,
22-test web package and real browser checks above. The later native cursor regression is
covered by its focused worker check; coordinator verification is recorded separately.
No Rust production change follows
the clean CI pass.

All package/matrix commands ran separately, not a workspace feature union. Cargo check's
portable target names above abbreviate the full triples `riscv32imc-unknown-none-elf`,
`riscv32imac-unknown-none-elf`, `wasm32-unknown-unknown`. Root CI is not embedded proof.

Initial `/tmp/no07-ci.log` was diagnostic and failed exact heap baselines, exposing the
optional payload leaks above; a manifest change during that run also invalidated its old
resolver state. `/tmp/no07-ci-final.log` then passed ten jobs but failed one web test's brittle
single-line source-shape lookup after the intentional multiline cancellation guard. The test
now asserts guard → return → commit → metadata enrichment ordering, with behavioral Node
regressions preserved. The final whole `/tmp/no07-ci-clean.log` exits 0 with all 11 jobs.
No earlier nonzero run is represented as a pass.

### Coordinator verification and embedded workspace

The coordinator independently reviewed the final source and both settled Chromium logs,
then ran `cargo test -p starplayer -p starplayer-host-wasm --features
midi,smf,note-overlap,interactive-audition,telemetry`: **136 tests / 10 result groups,
exit 0**, `/tmp/starplayer-no07-coordinator.log`. No production or test source changes
follow that check. The separate firmware checks below also passed. The worker handed off the diff
unstaged/uncommitted; final coordinator integration is recorded below.

The firmware workspace is separate from root CI. Its only target directory in this
worktree is `embedded/target -> /tmp/starplayer-no07-embedded-target`, owned by this task;
main/other worktree targets are untouched. `cargo +1.97 test --offline --manifest-path
embedded/Cargo.toml -p starplayer-firmware-common --target x86_64-unknown-linux-gnu`
passed **96 tests**, `/tmp/no07-embedded-common.log`. The matching package-isolated
`cargo +1.97 tree ... -e features,no-dev` graph contains neither optional feature,
`/tmp/no07-embedded-common-tree.log`.

From `embedded/`, with `CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true`:

| Board | Documented build | Result / evidence |
|---|---|---|
| A1S production/default | `. ~/export-esp-1.97.sh`, then `cargo xtask build --board a1s` | Release link passed on `esp-1.97`, target `xtensa-esp32-none-elf`; `/tmp/no07-embedded-a1s.log` |
| C5 actual renderer | `cargo xtask build --board c5 --features bench` | Release link passed on `1.97`, target `riscv32imac-unknown-none-elf`; `/tmp/no07-embedded-c5.log` |

Exact board graphs were checked with `cargo +TOOLCHAIN tree --offline --manifest-path
embedded/Cargo.toml -p starplayer-BOARD --target TARGET [--features bench] -e
features,no-dev`; `/tmp/no07-embedded-{a1s,c5}-tree.log`. Neither optional feature appears.
`file` identifies the linked outputs as 32-bit static Xtensa and RISC-V executables.
The xtask automatically generated only ignored module images/gzipped firmware assets.
No flash, erase, monitor, firmware configuration change, device execution, new device
heap/timing measurement or subjective listening occurred. Root CI is not used as this
embedded evidence. Disk stayed sufficient; no further owned-cache cleanup was needed.

At worker handoff, `git diff --check` passed and no paths were staged; generated artifacts,
goldens and historical sources are outside the diff. The worker released the sole
build/browser stream after all checks. Final coordinator integration is recorded below.

### Resource proof

Pinned 64-bit host sizes and exact construction overhead, measured in isolated feature
configurations and justified from source-counted queue entries/snapshot copies:

| Features | Snapshot | Command | TransportState | Full fixed overhead | Master-only/depth 1 overhead |
|---|---:|---:|---:|---:|---:|
| Neither | 2,624 | 16 | 40 | 27,856 | 17,360 |
| Overlap | 2,624 | 24 | 40 | 28,368 | 17,872 |
| Audition / both | 2,952 | 24 | 360 | 30,336 | 18,528 |

Full overhead adds `168 × voices + 5,288 × channels`; compact adds `168 × voices + 8 × channels`.
The exact test formula retains the original 27,808/17,344 bases and adds
`2 × snapshot_depth × (Snapshot - 2,616) + 64 × (Command - 16)`. Neither gains only the
allowed eight snapshot bytes for shared transport flags (six copies = 48 full; two = 16
compact); it gains no optional runtime state. Separate neither-feature size assertions
prevent a future unconditional Command/live payload from making the formula auto-grow.
RenderHalf is 11,800 bytes neither, 13,280 both. These are host measurements; current
embedded compile evidence is recorded separately and no new device heap measurement is
claimed. See [embedded budget](../../reference/embedded-budget.md).

### Available artifacts and owner acceptance

`xtask/no07-artifacts` generates original synthetic percussion/piano MODs, mixed-map XM
and dense 64-channel native IT browser fixtures plus paired 48 kHz/8-second stereo native
and overlap WAVs under `/tmp/no07-artifacts/owner/`. The helper asserts successful native
load/audio and an audible PCM difference for percussion/piano; it does not judge sound
quality. Canonical stress PCM/WAVs and per-configuration feature trees/logs are under
`/tmp/no07-artifacts/canonical/` and its parent. Browser controls screenshot:
`/tmp/no07-artifacts/controls.png`. No generated artifact is staged.

Owner checklist (all subjective/device items **pending**, not worker completion gates):

- [ ] Listen to percussion snare→kick with checkbox/global bypass and compare paired WAVs.
- [ ] Listen to piano C→G→C, equal-volume replacement and quieter-echo preservation; confirm
  the documented equal-volume same-note limitation is acceptable.
- [ ] Listen to mixed-loop XM maps and dense native IT; confirm authored NNA remains native.
- [ ] Try Pause/Resume, Stop/Play, Preview/StopPreview, keyboard Jam/selected-instrument and sustained
  audition with master insert tails; compare SAB and fallback behavior interactively.
- [ ] Try a real MIDI device, velocity/repeated-note matching, CC7 and note-off cleanup.
- [ ] Check browser/device touch, focus loss, disconnect/reconnect and phone audio lifecycle.
- [ ] Run/measure approved firmware on actual hardware and listen to its default path.

No firmware flash, erase, monitor, physical MIDI, subjective listening, main merge or
deployment was performed by this worker. Plans remain unarchived until main landing.


### Coordinator acceptance — 2026-10-28

Source, task/master contracts, review dispositions, strict resource formulas and final
verification evidence are accepted for feature-branch integration. The coordinator's
independent 136-test regression passed; production/test source has not changed afterward.
Final review confirms the complete 39-path diff, an empty index before explicit staging,
no historical assembly or golden changes, and the actual browser controls screenshot.
Task commit `0d1eb0e` is accepted and integrated by the repository's single-change
squash rule from the feature branch's own checkout. Only plan status/landing evidence
changed after that tested task commit. Main merge/deployment remains outside authorization.
Owner listening, physical MIDI, phone/device acceptance and firmware execution remain
pending as listed above.
