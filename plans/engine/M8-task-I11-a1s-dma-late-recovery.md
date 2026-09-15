# M8 — I11: Recover the A1S audio ring after a late DMA handoff

| Field | Value |
|---|---|
| Milestone | M8 |
| Status | Implemented 2026-09-15; host tests and release builds pass; awaiting owner hardware run |
| Recommended model | GPT-5.6-sol, high |
| Depends on | I3a A1S DMA recovery; I9 voice-capacity benchmark; I10 wide-channel admission |

## Context for a fresh agent

The A1S production refill task renders and packs one fixed 2,048-byte DMA descriptor,
awaits `I2sWriteDmaTransferAsync::available()`, then returns that descriptor with
`push_with`. The three-descriptor ring is deliberately fixed at 6,144 bytes so every
handoff contains two complete 128-frame render quanta.

Hardware playback of the owner's `/mnt/c/Users/scott/Downloads/unreal.s3m` in the web
firmware exceeded the measured render budget at order 0, pattern 5, row 32. The control
task stayed alive, but `offered`, `written` and `pushes` froze while `dma_errors` rose
about 190 per second. This is esp-hal's late circular-DMA state, not a device panic.
The current steady loop increments `DMA_ERRORS` and retries `available()` on every error,
so it never reaches the only operation which can return descriptor ownership.

The complete pinned-HAL sequence rules out calling `push_with` after that external error:
the first `available()` has cleared EOF and returned `Late` with zero bytes reserved, so
`push_with`'s own second availability check can await forever before its closure runs.
Instead reserve a valid whole descriptor before rendering. If DMA becomes late during the
expensive render, `push_with` discards its internal error while the prior reservation remains
available, allowing a complete write and ownership return. Follow `AGENTS.md`; do not modify
either historical source tree, run `cargo fmt`, or add attribution.

## Deliverables

1. Change steady refill to await and validate one whole-descriptor availability result before
   rendering. Render and pack against that reservation, then call `push_with` immediately so
   an internal `Late` cannot discard the reserved writable capacity.
2. Keep every handoff exactly `DESCRIPTOR_BYTES`. Never submit a
   partial descriptor, render a second descriptor inside the closure, allocate, log,
   lock, or panic in the refill path. Preserve sample-exact engine advancement: do not
   advance the renderer merely to manufacture recovery padding.
3. Retry an external availability error before rendering. Handle a failed or short handoff by
   preserving the staged descriptor without rendering again. Yield only through the actual
   async operations; do not introduce a time-based delay into normal refill.
4. Extract the smallest host-testable decision/state helper needed to regress the late
   path if direct esp-hal mocking is impractical. Tests must prove an availability error does
   not render or hand off, a valid whole offer reserves rendering, a full write advances to the
   next reservation, and short/error writes retry safely. Keep hardware ownership in `audio.rs`.
5. Update the A1S DMA documentation and embedded budget record with the failure signature,
   recovery behavior and owner hardware verification still required.
6. State the pinned-HAL boundary explicitly: this ordering prevents `Late` while a reservation
   exists, but the public async transfer cannot recover arbitrary complete lead exhaustion which
   has already reached zero-reservation `Late`. Do not claim that hardware result before the owner
   run.

## Research points

- Read the pinned esp-hal circular TX implementation before editing. Confirm how the external
  and internal `available()` calls clear EOF and affect `TxCircularState::available` in
  `DmaError::Late`.
- Preserve the previously verified invariant that descriptor pointer and byte offset move
  together. A recovery must not revive the rejected variable-region handoff design.
- Confirm that a pre-render reservation remains available to `state.push_with` after its inner
  availability check reports and discards `Late`.

## Verification

- Run the new targeted host tests and all `starplayer-firmware-common` tests.
- Run relevant `starplayer-host-embedded` tests.
- Build release A1S `web` and `web,lcd` on the repository's pinned `esp-1.97` toolchain;
  confirm the 32 KiB linker stack floor still passes.
- Run `git diff --check` and inspect `git status`. Stage only explicit task paths; never
  include `log.txt`, `embedded/log.txt`, owner modules, firmware images or caches.
- Hardware acceptance after merge: upload `unreal.s3m`, pass pattern 5 row 32, and verify
  playback time and descriptor counters continue advancing after any late event. Temporary
  audible breakup is acceptable; a frozen row or permanently rising error-only loop is not.

## Out of scope

Voice/channel admission limits, voice stealing policy, reducing mixer cost, changing the
DMA ring or descriptor sizes, format semantics, web scheduling, or promising clean audio
above the measured web-firmware capacity.
