# NO-04 — Legacy-format overlap and bounded tails

| Field | Value |
|---|---|
| Feature | [NO master plan](NO-master-plan.md) |
| Status | Reviewed; ready for coordinator handoff; do not implement until requested |
| Depends on | NO-01 |
| Blocks | NO-05, NO-06 |
| Parallel with | NO-02/03 only with isolated worktrees and explicit shared-seam ownership |
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

Read MOD/S3M/MTM note handling and flush/retrigger paths plus ChannelTable trigger/detach,
VoicePool release/iteration and SampleRegion queued swaps. These formats currently cut
on replacement and reserve one voice per channel. Implement only opt-in behavior.

## Deliverables

- Implement policy resolution and duplicate matching in a reusable concrete engine
  helper; format processors explicitly identify new-note starts. Incoming identity
  chooses duplicate matching; outgoing actual voice identity chooses survival.
- Track custom-tail generation, age and originating channel. Keep original voice sample,
  parameters and routing when detaching. Different-notes cuts all matching instrument/
  original-note candidates in the channel; no global instrument match.
- Enforce prepared custom budget: oldest custom tail first, stable slot tie-break,
  never unrelated foreground. When no eligible capacity exists, replace foreground
  normally and report diagnostic. Release/ramp behavior uses mixer primitives.
- Reject custom detachment for looping/sustain/non-PCM regions. Discard queued sample
  swap only on a successfully detached eligible one-shot, so it cannot loop later.
- Wire MOD/S3M/MTM delayed musical notes; preserve native portamento, effect retrigger,
  sample-only, offset-past-end and invalid-note behavior. Blank/no-op rows create no tail.
- Policy edits affect subsequent note starts; bypass clears custom-provenance tails.
  Module replacement clears voices and ownership together. Legacy seek/restart retains
  custom ownership for any still-sounding tails; never discard bookkeeping alone.
  Interactive seek uses scoped clearing. Preserve mute/channel insert routing.
- Add event-frame tests and update accuracy/architecture implementation status.

## Interfaces consumed and cleanup integration

Consume NO-01 ownership/provenance and layout contracts; do not create another age/
provenance table. Own MOD/S3M/MTM trigger seams and the concrete engine overlap helper
consumed by NO-05. Integrate NO-03 scoped clears after its landing: Stop and global
all-sound-off clear tails/bindings/metadata, interactive seek clears song tails only,
and bypass clears custom tails only. Retiring ramps keep ownership but not active quota; use the master’s per-domain retirement reserve/fallback. If this
task is prepared before NO-03, coordinator must integrate and test the clear hooks before
landing it. Add Stop/seek/bypass tests with active tails and unrelated live voices.
Player shadow state retains policy selections on rebuild, resets on explicit user load.

## Research points

- Inspect dirty-bit SAMPLE triggers that are not actual notes, especially MOD queued
  swaps/revival and S3M retrigger. Carry cause through delayed processing explicitly.
- Inspect original-note storage for raw MOD periods and unusual pitch values; use stable
  original trigger identity, not current slide pitch or a lossy nearest-note conversion.
- Check release ramp, pending swaps and natural sample completion do not leak ownership.

## Verification

- `cargo test -p starplayer-mod`, `cargo test -p starplayer-s3m`,
  `cargo test -p starplayer-mtm`, `cargo test -p starplayer-engine` and goldens.
- Snare→kick, C→G→C, repeated same note, same instrument across channels, mixed policy
  changes, invalid sample, all loop types, pending MOD swap, pool saturation/reuse.
- Block-size determinism and no-allocation tests with overlap active; compare all-original
  feature-on output against feature-off golden output. Never update canonical goldens.

## Out of scope

XM/IT articulation, loop release synthesis, UI, changing canonical effect semantics,
new mixer implementation or editing historical assembly.
