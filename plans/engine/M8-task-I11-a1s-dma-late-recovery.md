# M8 — I11: Recover the A1S audio ring after a late DMA handoff

| Field | Value |
|---|---|
| Milestone | M8 |
| Status | Reserve-before-render 2026-09-15 and whole-ring re-ownership 2026-09-16 both **rejected on hardware**; steady ordering reverted to the soaked arrangement and a single gated handoff added 2026-09-16; host tests and release builds pass; owner reports that build plays better on hardware, full acceptance below still outstanding |
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

The first attempt reserved a valid whole descriptor before rendering, so that a `Late` arising
*during* the expensive render would be discarded by `push_with` while the prior reservation
remained available. Hardware rejected that ordering twice — it did not stop the wedge, and it
never played `REFLEX.S3M` cleanly either. **The steady loop must stay exactly as the two-minute
`underruns=0` soak left it**: render and pack one descriptor, hold it until a whole-descriptor
offer, then hand it over with nothing in between, and keep the pre-roll's explicit `available()`.
Only the availability error arm may differ. The clean-build evidence in `embedded/README.md`
belongs to the build *before* the reordering; any future change to this ordering needs its own
soak before it can be quoted as a baseline.

It argued the error branch could go on retrying because `push_with` after an external `Late`
would await forever. The owner's hardware run disproved that: the wedge reproduced unchanged,
and once wedged the device is silent for every later module, including the built-in
`REFLEX.S3M`, until reset. Two facts in the pinned HAL correct the analysis:

* `TxCircularState::update` returns `Late` **by value** when EOF is pending, and in circular
  mode `fill_for_tx` sets `suc_eof` on every descriptor, so EOF returns at the descriptor rate —
  187.5/s at 48 kHz, which is the ~190/s the wedge logged. `push_with`'s inner `available()`
  therefore waits at most one `DmaTxDoneChFuture` (~5.3 ms) and then reaches its closure.
* `state.push_with` hands one descriptor back to the DMA even when the closure returns 0, which
  is what breaks `update`'s ownership walk so the next check can credit the bytes that are free
  in hardware while `state.available` reads 0.

A second attempt then over-corrected, handing back `RING_DESCRIPTORS` descriptors on *any*
availability error so that the pointer would walk back into step with the write offset. Hardware
rejected it: `REFLEX.S3M`, clean at `underruns=0` for two minutes in the previous build, degraded
audibly and logged about four ring-empty events a second from the first second on. Two reasons,
both of which this task must now respect:

* A zero-byte handoff consumes nothing from `state.available`. Three of them leave the HAL
  believing a whole ring is writable while the DMA owns all of it, so every later handoff writes
  into a buffer the DMA is reading. **Hand back exactly one.**
* Not every `available()` error is a `Late`. The clean build's constant `dma_errors=1` was a
  startup transient — a true `Late` is self-perpetuating and would have wedged — and spending a
  ring-corrupting recovery on it is what destroyed the transport. **Match `Late` specifically,
  and require a run of them.**

This is the §4a remediation the budget record already carried — *reach `push_with` anyway* —
narrowed to the one state that needs it. Follow `AGENTS.md`; do not modify either historical
source tree, run `cargo fmt`, or add attribution.

## Deliverables

1. Leave the steady ordering alone. Render and pack one descriptor, preserve it until a validated
   whole-descriptor offer, then call `push_with` immediately with no render, pack, log or other
   await in between, and keep the pre-roll's explicit `available()` before each muted handoff.
2. Keep every handoff exactly `DESCRIPTOR_BYTES`. Never submit a
   partial descriptor, render a second descriptor inside the closure, allocate, log,
   lock, or panic in the refill path. Preserve sample-exact engine advancement: do not
   advance the renderer merely to manufacture recovery padding.
3. Recover a `Late`, and only a `Late`, with exactly one zero-byte handoff, on sight. Match
   `Error::DmaError(DmaError::Late)` specifically; retry every other error exactly as the clean
   build did. Do not wait for consecutive `Late` results before recovering: the failed check
   clears EOF, so each extra opinion costs a descriptor period, and an eight-deep gate spent
   501 ms of every overloaded wall second waiting. Handle a failed or short handoff by preserving
   the staged descriptor without rendering again. Yield only through the actual async operations;
   do not introduce a time-based delay into normal refill.
4. Extract the smallest host-testable decision/state helper needed to regress the late
   path if direct esp-hal mocking is impractical. Tests must prove a transient error only ever
   retries, a short run of `Late` only retries, a proven run asks for one re-ownership handoff
   without rendering or staging, a valid whole offer reserves rendering, a full write advances to
   the next reservation, and short/error writes retry safely. Keep hardware ownership in
   `audio.rs`.
5. Update the A1S DMA documentation and embedded budget record with the failure signature,
   recovery behavior and owner hardware verification still required.
6. Count the recovery so the next hardware run can be read from the log line alone: one
   `dma_errors` per failed check, plus one `underruns` and one `pushes` per recovery.
   `dma_errors` climbing alone is the wedge signature; `underruns` tracking `dma_errors`
   one-for-one is the signature of the disproved ring recovery. Do not claim the hardware result
   before the owner run.

## Research points

- Read the pinned esp-hal circular TX implementation before editing. Confirm how the external
  and internal `available()` calls clear EOF and affect `TxCircularState::available` in
  `DmaError::Late`.
- Preserve the previously verified invariant that descriptor pointer and byte offset move
  together. A recovery must not revive the rejected variable-region handoff design.
- Confirm that a pre-render reservation remains available to `state.push_with` after its inner
  availability check reports and discards `Late`.
- Confirm on the board, not on paper, that `push_with` after an external `Late` reaches its
  closure and that one zero-byte handoff clears the wedge. Three paper arguments have now been
  overturned by hardware in this task; prefer a measurement, and change one thing per flash.

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
  Overload pace is tracked separately: above the render budget the song currently stretches
  rather than keeping wall-clock time. That is a playback-policy question, not a DMA one.
  Then switch back to the built-in `REFLEX.S3M` and confirm it still plays — after the first
  attempt the wedge outlived the module that caused it. Check `REFLEX.S3M` alone **first**, and
  treat anything but `underruns=0` with a fixed `dma_errors` as a regression: that is how the
  second attempt failed.

## Out of scope

Voice/channel admission limits, voice stealing policy, reducing mixer cost, changing the
DMA ring or descriptor sizes, format semantics, web scheduling, or promising clean audio
above the measured web-firmware capacity.
