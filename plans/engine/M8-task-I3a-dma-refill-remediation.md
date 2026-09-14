# M8 — I3a: the DMA refill can wedge at start-up (remediation)

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md)), remediating [I3](complete/M8-task-I3-esp32-a1s-bringup.md)'s `audio.rs` |
| Status | **Open, written 2026-09-14.** Found on hardware by the sibling project `../star-fx`; not yet reproduced here, because nothing in `embedded/` has ever been flashed |
| Depends on | — (the diagnosis is done; see `plans/reference/embedded-budget.md` §4a) |
| Blocks | The first audible A1S run. M8's exit criterion is "sound from the headphone jack", transport progress and codec output routing must both be verified |
| Parallel with | Work outside the A1S audio, codec and boot files |
| Recommended model | GPT-5.6-sol, high effort — it is a handful of lines, but they are the real-time path, the failure mode is silence with a counter climbing, and the fix needs both esp-hal source review and a board run |
| Verified by | the owner or an agent with the board: a bounded boot/run transcript with advancing transport and reported DMA error/underrun counters, plus clear stereo music from the headphone jack |

## Context for a fresh agent

`refill_task` in `embedded/boards/starplayer-a1s/src/audio.rs` takes `continue` when
`transfer.available()` returns an error, which skips `push_with` — and `push_with` is the only
call that can recover the state that error reports. The loop can therefore spin for ever,
counting `DMA_ERRORS` at hundreds of thousands per second, with the DAC silent and
`underruns=` reassuringly 0.

**Read `plans/reference/embedded-budget.md` §4a first.** It has the whole diagnosis: the two
esp-hal functions' differing treatment of `available()`'s error, why `TxCircularState::update`
reports nothing free through the ring's first pass and then returns `DmaError::Late`, the
evidence from the A1S bench (`../star-fx`'s transport froze at one block with its overrun
counter climbing by ~650 000/s, and ran 901 544 consecutive clean blocks once the code reached
`push_with` anyway), and the two hypotheses already disproved on hardware — ring depth and
arming order — which must not be re-tested here.

The same file's §4a also corrects a second, harmless claim: a DMA descriptor is not one render
quantum. `audio.rs`'s module documentation has been corrected in place; no code depends on it.

### Code you must read before changing anything

- `embedded/boards/starplayer-a1s/src/audio.rs` — `refill_task`, `fill`, `start`, and the
  hazard section now in `refill_task`'s own documentation.
- `~/.cargo/registry/src/index.crates.io-*/esp-hal-1.1.2/src/i2s/master.rs` — the `asynch`
  module's `available`, `push` and `push_with`. The whole finding is that `push_with` writes
  `let _avail = self.available().await;` and `push` writes `self.available().await?`.
- `~/.cargo/registry/src/index.crates.io-*/esp-hal-1.1.2/src/dma/mod.rs` —
  `TxCircularState::{new, update, push_with}` and `DescriptorChain::fill`.
- `../star-fx/firmware/src/audio/i2s.rs` — a worked implementation of the pre-roll, including
  why the steady-state loop must keep `push` rather than `push_with`.

## Deliverables

### 1. Reach `push_with` even when `available()` failed

Do not `continue` on the error arm. Count the error — it is still worth seeing — and fall
through to the `push_with` call, which is what hands a descriptor back to the DMA and brings
the accounting good again.

### 2. Consider priming the transfer before the steady loop

`../star-fx`'s `Transport::begin` pushes descriptors of silence with `push_with` before the
first real block is due, so the accounting is already live when the audio starts. This
firmware prefills its ring *before* starting the transfer, which is not the same thing — the
prefill puts bytes in memory, whereas the problem is the descriptor bookkeeping. Decide
whether a pre-roll is wanted here or whether deliverable 1 alone suffices, and say which and
why in the module documentation.

**Do not** adopt `push_with` for the steady-state loop by analogy with `../star-fx`'s
pre-roll: it returns the descriptor to the DMA whether or not anything was written, so a short
offer desynchronises the ring for good. That firmware measured four builds wedging after 4–14
blocks before confining `push_with` to the pre-roll. This firmware happens to be safe with it
today only because `fill` renders whatever it is offered.

### 3. Make a wedge visible rather than silent

`DMA_ERRORS` already exists and `dma_errors()` is already exposed; confirm the once-a-second
transport line in `main.rs` actually prints it, and add it if not. A firmware that says
`dma=1930481` is diagnosable in seconds; one that only says `underruns=0` is not.

Optionally, evaluate a bounded recovery backstop. Star FX currently uses
`RX_LATE_RECOVERY_WINDOW_CYCLES = 4_000 * 240_000`, which is **4 seconds at 240 MHz**,
despite a stale comment saying 20 ms. Do not copy that comment as a measured requirement or
assume a 300 ms reboot time. Choose and document a player-specific recovery policy; record
reset reasons so repeated resets cannot masquerade as successful playback.

### 4. Correct the board output aliases and verify headphone routing

Read `plans/reference/embedded-budget.md` §4b and
`../star-fx/plans/M1-task-B4-headphone-routing.md` (or its `complete/` location).
In `embedded/boards/starplayer-a1s/src/es8388.rs`, correct `HEADPHONE` to
`LOUT2 | ROUT2 = 0x0c`, `SPEAKER` to `LOUT1 | ROUT1 = 0x30`, and the corresponding
volume/register comments. Keep individual register addresses and enable bits unchanged.
StarPlayer currently enables `ALL = 0x3c` and sets all four analog volumes to unity, so
these wrong aliases are masked in the current boot path; they do not prove it is silent.
If selecting headphone-only output, set pair-2 volumes explicitly and verify GPIO21 PA
handling against the board schematic. Make the output choice explicit in the run report.

## Research points

1. Does this firmware actually wedge on the board, or does its 23 ms ring and its
   render-anything `fill` happen to get the first `Ok` in before the ring drains? The bench
   answers this in one flash, and the answer decides whether deliverable 2 is needed at all.
2. With the fix in, what is the smallest `DMA_RING_QUANTA` that never underruns? That is M8-I3
   research point 4, still open, and it is now cheap to measure.

## Verification

```sh
. ~/export-esp-1.97.sh
cd embedded && cargo xtask build --board a1s
# on the bench — an A1S is reachable over an rfc2217 bridge; see embedded-budget.md §4a
cargo xtask flash --board a1s && cargo xtask monitor --board a1s
#   → the transport line advances once a second, underruns=0, dma errors 0, and music plays
```

### Board acceptance and shared-board coordination

The board was last moved to power-bank power for noise isolation; the bridge at
`192.168.0.151:8086` is not guaranteed to be connected. Coordinate serial ownership with the
Star FX session before flashing. Use project wrappers, preserve the Star FX presets partition,
and record the image being replaced and a known-good Star FX restore command/image. Do not
run competing monitors. Start listening at low volume with headphones off the owner's ears.

- Capture a fresh boot and at least five minutes of advancing transport. Report startup errors
  separately from steady-state errors; require no recurring DMA errors, underruns or resets.
- Confirm recognizable music in both ears without crackling, then test left/right identity
  with a known stereo signal. Counters alone cannot certify audible routing or channel order.
- Compare rendered digital silence with music under the same power arrangement. If noise
  persists, compare MacBook USB power and power-bank power. Record subjective observations
  separately from measured RMS/spectrum; do not promise that the DMA fix cures analog noise.
- For DAC-only playback, keep ADC and analog bypass/mixer routes disabled and verify the
  actual init writes. Audible microphone pickup in Star FX's ADC-to-DAC path does not prove
  microphone leakage in this DAC-only configuration.
- Host/build success is not board acceptance. If the board is unavailable, leave those gates
  explicitly pending and provide the exact build/flash/monitor commands supported by xtask.

## Out of scope

The descriptor-granularity correction (documentation only, already applied). Any change to
`fill`, `RenderHalf` or the render quantum. The C5.

Microphone removal, input-bank workarounds and other board modifications are outside this
playback task. Do not change hardware to chase Star FX input noise.

## Handoff amendment verification (2026-09-14)

Reviewed commit `631f785`, the current A1S codec initialization and refill code, Star FX
M1-B4 listening evidence, and the recovery constant. Documentation-only amendment;
`git diff --check` passed. No firmware build or flash performed, and no unsafe sites added.
