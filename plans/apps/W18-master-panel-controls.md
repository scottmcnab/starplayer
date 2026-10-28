# W18 — Instruments policies and compact Playlist selection

| Field | Value |
|---|---|
| Status | Implemented, Astra-reviewed and verified; feature branch intentionally unmerged |
| Baseline | main 4d03bcb, compact Instruments W17 |
| Least-capable safe worker | GPT-6.1-sol (high) |
| Review | Plan reviewed by GPT-6 Astra medium; entire completed branch must receive another Astra review |

## Context for a fresh agent

The owner approved the two detailed tasks linked below. The latest direction explicitly
forbids merging to main. Implement in sibling worktrees, review and verify before
committing, then get GPT-6 Astra medium to review the ENTIRE feature branch against
main and fix agreed findings. Leave plans here until landing, with honest evidence.
Do not touch historical sources, user scratch files or independent W6 worktree.
No cargo fmt, broad staging, unrelated formatting or commit attribution.

## Deliverables

- [Instruments, selected-note audition and page-session playback policies](W18-task-instruments-policies.md).
- [Compact Playlist table and Library-style selection](W18-task-playlist-selection.md).
- [Page policy reconciliation and transport replay](W18-task-policy-replay.md), split
  from the Instruments task for independent implementation and review.
- Amend architecture/accuracy foundations, application help and README to the approved
  contracts. Coordinator owns foundation docs/index and combined acceptance.
- Feature branch remains unmerged. No deployment or push requested.

Detailed review findings and final evidence: [W18 review and verification](W18-review-and-verification.md).

## Research points

The Astra plan review found four critical boundaries: incoming AND candidate voices
must be song for the new cross-channel rule; saved Playing restoration and pending
Play/Resume must wait for policy replay; desired page preferences cannot be replaced
by older acknowledgements; selected-note viability must agree UI/host/rack, including
XM tuning and inspection failure. Both task specs incorporate these findings.

## Verification

Run task checks plus cargo xtask ci and full packaged Chromium checks (SAB, forced
fallback, plain HTTP), including desktop/phone screenshots. Serialize Cargo/browser
work; CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 RUST_TEST_THREADS=2; Node concurrency1.
Use /tmp/starplayer-no-target as shared build cache, with target symlink for wasm
packaging. Record exact checks and any limitations. Coordinator independently reviews
worker changes, tests combined code and resolves review findings before committing.

## Out of scope

Main merge, deployment, public wire compatibility migrations, canonical effect fixes,
golden regeneration, persistence beyond current page and unrelated cleanup.
