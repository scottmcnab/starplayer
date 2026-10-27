# NO-07 — Integration, determinism and feature exclusion

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Instrument-only revision reviewed; staged/uncommitted for owner review; do not implement until requested |
| Depends on | NO-W1 and NO-01–06 |
| Blocks | Feature landing and owner listening acceptance |
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
