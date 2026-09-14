# M8 — I3a: the DMA refill can wedge at start-up (remediation)

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md)), remediating [I3](complete/M8-task-I3-esp32-a1s-bringup.md)'s `audio.rs` |
| Status | **Open, written 2026-09-14.** Digital transport accepted on hardware after a 310-second soak; owner headphone listening and stereo acceptance remain pending |
| Depends on | — (the diagnosis is done; see `plans/reference/embedded-budget.md` §4a) |
| Blocks | Reliable A1S playback. M8's exit criterion is "sound from the headphone jack", transport progress and codec output routing must both be verified |
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

The initial task treated §4a's descriptor-granularity correction as documentation-only. The
hardware-finding amendment below disproves that assumption: steady refill must align rendering
and pushes to whole descriptors.

### Code you must read before changing anything

- `embedded/boards/starplayer-a1s/src/audio.rs` — `refill_task`, `fill`, `start`, and the
  hazard section now in `refill_task`'s own documentation.
- `~/.cargo/registry/src/index.crates.io-*/esp-hal-1.1.2/src/i2s/master.rs` — the `asynch`
  module's `available`, `push` and `push_with`. The whole finding is that `push_with` writes
  `let _avail = self.available().await;` and `push` writes `self.available().await?`.
- `~/.cargo/registry/src/index.crates.io-*/esp-hal-1.1.2/src/dma/mod.rs` —
  `TxCircularState::{new, update, push_with}` and `DescriptorChain::fill`.
- `../star-fx/firmware/src/audio/i2s.rs` — a worked implementation of the pre-roll and the
  general danger of partial steady `push_with` calls. The amendment below records the stricter
  equal-descriptor, full-consumption use required by this board run.

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

**Initial constraint, superseded by the hardware amendment below:** do not adopt unconstrained
steady `push_with` by analogy with `../star-fx`'s pre-roll. It returns descriptor ownership even
when the closure writes nothing, so a short or partial offer can desynchronise the ring. The
later board runs require `push_with` here because plain `push` wedges on its second availability
check, but only with equal whole descriptors and a closure that consumes every complete
descriptor it is offered.

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

1. **Answered by the amendment below:** the firmware recovered startup accounting, but the old
   23 ms ring and steady `push_with` discipline supplied only about half the required byte rate.
2. **Answered for this implementation:** the nine-quantum, three-descriptor ring completed a
   310-second soak with no underruns and the required steady 176 400 B/s write rate. A smaller
   ring is not required for I3a acceptance.

## Verification

```sh
. ~/export-esp-1.97.sh
cd embedded && cargo xtask build --board a1s
# on the bench — an A1S is reachable over an rfc2217 bridge; see embedded-budget.md §4a
cargo xtask flash --board a1s && cargo xtask monitor --board a1s
#   → transport advances once a second, underruns=0, no recurring DMA errors, and music plays
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

Changes to `RenderHalf` or the render quantum beyond board-local descriptor staging. The C5.

Microphone removal, input-bank workarounds and other board modifications are outside this
playback task. Do not change hardware to chase Star FX input noise.

## Handoff amendment verification (2026-09-14)

Reviewed commit `631f785`, the current A1S codec initialization and refill code, Star FX
M1-B4 listening evidence, and the recovery constant. Documentation-only amendment;
`git diff --check` passed. No firmware build or flash performed, and no unsafe sites added.

## Hardware-finding amendment (2026-09-14)

This plan remains **open**. Successive bounded flashes established the following:

- Moving async driver construction to core 1 preserved correct interrupt ownership but did not
  fix the gating. The codec unmuted at +0.57 seconds and engine elapsed was only 0:24 at +49.86
  seconds, with `underruns=0` and `dma_errors=0`.
- The next diagnostic build reported `offered=written=88748` at +1.59 seconds, then only
  89–90 KB/s and about 43 pushes/s, with no empty offers. Stereo 16-bit at 44.1 kHz requires
  176 400 B/s, so engine time advanced at the exact rate at which the refill supplied bytes.
- esp-hal 1.1.2's `DescriptorChain::new` ignores the allocation macro's chunk size and uses its
  4 092-byte default when it fills the chain. The 4 096-byte ring was therefore split into three
  ragged 1 366 / 1 366 / 1 364-byte descriptors. Returning variable contiguous regions through
  steady `push_with` advanced descriptor ownership at only about half the physical byte rate.
- The first equal-descriptor build booted and completed exactly one 1 536-byte steady `push`.
  `offered=written=1536 pushes=1` then remained fixed, engine time stayed at 0:00, and
  `dma_errors` climbed by about 115/s. The plain `push` call's internal second `available()`
  returned `Late` after the outer check and render delay; the outer error path then skipped every
  later descriptor handoff.

The board-local remediation uses nine render quanta: a 4 608-byte ring split into three equal
1 536-byte descriptors, each exactly three render quanta or 384 stereo frames. Compile-time
assertions preserve that geometry. The steady loop preserves deliverable 1's explicit outer
`available()` and error count, then always falls through to descriptor-aligned `push_with`. Its
internal availability error is discarded before the closure renders, removing the fallible gap
between rendering and descriptor handoff. The closure loops over every complete 1 536-byte
descriptor in its contiguous offer, renders through static `i16` scratch, copies it, and returns
the full byte count; it never intentionally returns a partial descriptor. An empty recovery offer
returns zero without rendering. Unlike the old ragged-ring implementation, equal descriptor
geometry plus this full-consumption invariant keeps `push_with` aligned in steady state. The
eight muted recovery handoffs still take about 70 ms.

### Successful transport soak (2026-09-14)

The final descriptor-aligned build on `main` passed the bounded hardware transport run:

- Exact image SHA-256:
  `cfcad897bd3cdec42f128d9a3754ca404095c4a4298a5164b9f2359fec136f91`
  (477 904 bytes).
- A fresh-reset run soaked for 310 seconds. At the final telemetry line, engine time was 0:31
  after two complete 2:18 loops: about 307 seconds of module time, tracking wall time after boot.
- Final refill telemetry was `offered=63664128 written=54571008 pushes=23687`. The written rate
  was approximately 176 400 B/s, exactly the stereo i16 rate required at 44.1 kHz.
- `underruns=0`; `dma_errors=1` was already present in the first telemetry line and did not
  increase during the soak. This is one recovered startup transient, with no steady-state errors.
- `peak=1178`, `retired=0`, and `rejected=0`. The firmware booted at 1/16 master volume,
  selected headphone output only, and kept the speaker PA off.

This accepts the refill cadence, startup recovery, transport progress, ring geometry and output
routing configuration. **The plan remains open and must not be archived:** owner listening
acceptance is still pending recognizable clean stereo music in both ears and confirmation of
left/right identity through the headphone jack.

### Listening-finding amendment: output gain staging (2026-09-14)

The first owner run of the accepted transport played at the correct pitch and speed, but still
sounded noisy or clipped at the 1/16 master setting. The I2S samples are already signed
two's-complement `i16`: `FixedOut<i16, 2>` clamps the fixed accumulator, `RenderHalf` applies the
master setting in the engine before its limiter, and `bytemuck::cast_slice` preserves those bits
in the ESP32's native little-endian DMA buffer. The ES8388 is configured for 16-bit Philips I2S,
whose sample interpretation is signed two's-complement PCM. Do not add an unsigned bias.

The current gain distribution is nevertheless poor for headphones: the engine reduces the
signal by about 24 dB while `LOUT2VOL`/`ROUT2VOL` run at analog 0 dB. An offline fixed/i16 render
of the bundled `REFLEX.S3M` reaches full scale for 515 299 of 12 192 768 samples (4.226%), and a
post-scale approximation at 1/16 uses only 3 446 distinct values. Preserve the owner's accepted
maximum listening level while retaining more digital resolution:

- boot and cap the A1S engine master at 1/4 (−12.04 dB), retaining two more signal bits and
  enough headroom that this three-voice module does not reach the limiter;
- set the enabled ES8388 headphone pair to −12 dB (`LOUT2VOL = ROUT2VOL = 0x16`), so the combined
  nominal output remains about −24 dB; leave the disabled speaker pair at minimum and GPIO21 low;
- apply the same A1S maximum to button and web volume commands, so no control path bypasses the
  listening-safe cap; keep the existing 1/64 adjustment step;
- flash and repeat the listening check at the same nominal loudness. Keep 16-bit I2S for this
  controlled comparison. If the noise remains, the next isolated experiment is 32-bit Philips
  slots with each signed `i16` sample sign-extended and shifted into the high 16 bits, matching
  the known full-duplex board transport; do not combine that framing change with this test.

The transport counters must remain at the accepted cadence after the gain change. Owner
confirmation of clean stereo and left/right identity remains the final acceptance gate.

Implementation keeps the I2S path at signed 16-bit Philips framing. The A1S boot value and
board maximum are both exact `U0F16` bits 16 384, while the existing step remains bits 1 024.
Button increase/decrease and the web control-task command boundary share host-tested saturating
helpers, so an HTTP or WebSocket request cannot bypass the 1/4 cap. Codec initialization writes
`0x16` to the enabled `LOUT2VOL`/`ROUT2VOL` pair and `0x00` to the disabled speaker pair while
GPIO21 remains low. The firmware-common host suite passes all 57 tests, including the exact cap
and step cases, and both the default and `web` A1S release builds pass. The owner listening rerun
remains pending.

### Gain-staging hardware run (2026-09-14)

The gain-staged build passed its fresh-boot configuration and bounded transport check:

- Exact `main` image SHA-256:
  `1187941feaad1c06b9e143a5eddc5c1a695dcef67fa1626d6401c7f816235c06`
  (477 600 bytes). The post-flash hash matched.
- Boot reported the ES8388 as a 16-bit Philips slave with headphone analog −12 dB and the
  speaker pair at minimum, held muted until audio readiness. I2S remained 44 100 Hz stereo i16
  over the nine-quantum ring.
- The play line confirmed engine master `1/4`, maximum `1/4`, headphone output only and speaker
  PA off.
- The first five telemetry lines advanced from 0:00 through 0:04. `written` advanced from
  172 032 to 881 664 bytes and `pushes` from 76 to 385, with `underruns=0` throughout.
  `dma_errors=1` was stable across all five lines, the same recovered startup transient accepted
  by the longer transport soak rather than a recurring steady-state error.

This accepts the gain-staging register writes, boot cap and post-change refill cadence. **Owner
listening acceptance remains pending** for clean recognizable stereo in both ears and left/right
identity; keep this plan open and unarchived until that check is complete.
