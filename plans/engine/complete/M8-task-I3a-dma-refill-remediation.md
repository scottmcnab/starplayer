# M8 — I3a: the DMA refill can wedge at start-up (remediation)

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](../M8-master-plan.md)), remediating [I3](M8-task-I3-esp32-a1s-bringup.md)'s `audio.rs` |
| Status | **Complete 2026-09-14.** The heap-backed constrained handoff and normal 48 kHz production image pass objective hardware checks, and the owner accepts normal music as sounding good. The original DMA refill remediation goal is complete |
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

### 32-bit CPU-generated framing experiment (2026-09-14)

The owner reports that the gain-staged 16-bit build still sounds grainy. `../star-fx`'s
same-device ADC-to-DAC path sounds clean, which proves that this ES8388, its analog path, clocks
and board wiring can produce clean audio. It does not prove StarPlayer's CPU-generated sample
packing: Star FX configures `Data32Channel32` and `DACCONTROL1 = 0x20`, receives native `i32`
slots, processes them as `i32`, then byte-casts that matching representation back to TX. An
RX/TX representation can round-trip consistently even when a separately generated `i16` stream
would need different slot alignment.

The next isolated image therefore keeps the engine 1/4 cap, codec analog −12 dB, headphone-only
routing, module and all refill recovery behavior unchanged. It changes only the serial framing:

- esp-hal uses `Data32Channel32` and the ES8388 uses 32-bit Philips (`DACCONTROL1 = 0x20`);
- each signed engine `i16` is sign-extended to `i32`, shifted into the high 16 bits and emitted
  as explicit little-endian bytes, with the same allocation-free packer in prefill and steady
  refill;
- each stereo frame is eight DMA bytes, so the required transport rate is 352 800 B/s;
- the small ring is six quanta = 6 144 bytes, split into three equal 2 048-byte descriptors of
  two quanta or 256 frames. The ring is about 17.4 ms and each descriptor about 5.8 ms, making
  the eight muted recovery handoffs about 46 ms. Compile-time assertions retain three whole,
  equal descriptors and the 8 184-byte small-ring limit.

All 60 firmware-common host tests pass, including cases that pin zero, ±1, both signed extrema,
stereo ordering and refusal to write a partial 32-bit slot. Default and `web` A1S release builds
also pass.

### 32-bit framing hardware run (2026-09-14)

The merged 32-bit image passed its fresh-boot framing and bounded cadence check:

- Exact `main` image SHA-256:
  `6582f9f0a8b384fd61f6f9a1c4a2e0568142316d2f333d440d01d822fd14565c`
  (477 744 bytes). The post-flash hash matched.
- Boot reported the ES8388 as a 32-bit Philips slave with headphone analog −12 dB and the
  speaker pair at minimum, held muted until readiness. I2S reported 44 100 Hz stereo 32-bit
  slots, a six-quantum / 768-frame / 17 ms ring, and eight muted pre-roll handoffs in about
  46 ms.
- The play line confirmed engine master `1/4`, maximum `1/4`, headphone output only and speaker
  PA off.
- From +1.75 through +11.76 seconds, engine time advanced from 0:00 through 0:10. `written`
  advanced from 354 304 to 3 909 632 bytes and `pushes` from 116 to 1 273. The 3 555 328-byte
  delta over 10.01 seconds is about 355.2 KB/s when sampled on one-second telemetry boundaries,
  consistent with the required 352 800 B/s while engine time tracked wall time.
- `underruns=0` throughout. `dma_errors=1` was stable across the interval, the recovered startup
  transient seen in the accepted 16-bit runs rather than a recurring steady-state error.

This accepts 32-bit codec/I2S framing, CPU sample packing, ring geometry and refill cadence.
**Owner subjective listening remains pending** for clean recognizable stereo in both ears and
left/right identity; keep this plan open and unarchived until that check is complete.

### Listening-finding amendment: direct diagnostic tone (2026-09-14)

The owner reports that the 32-bit-slot image remains grainy. Gain distribution and CPU slot
packing are therefore not sufficient explanations. Add an A1S-only diagnostic feature which
isolates the remaining path without replacing the normal firmware behavior:

- under an explicit `tone` feature, continue rendering the bundled module so engine load and
  refill scheduling remain representative, then overwrite each outgoing stereo `i16` quantum
  immediately before the existing 32-bit packer;
- generate equal left/right signed samples from a compile-time sine table and integer phase
  accumulator only. Use a conservative amplitude near 4 096 (about −18 dBFS before the fixed
  codec −12 dB) and alternate about one second of tone with about one second of exact digital
  zero. Choose the gate length at a table-cycle boundary to avoid an intentional transition
  click;
- keep 32-bit Philips codec/I2S configuration, ring geometry, pre-roll, refill recovery, analog
  gain and speaker routing byte-for-byte equivalent to the music image;
- make the boot log unambiguously identify the diagnostic tone, its frequency/amplitude and
  tone/silence cadence. Add host tests for signed extrema/range, stereo identity, phase
  continuity, the zero interval and cycle-boundary gating;
- build and flash only the `tone` image for this listening comparison. If the tone is clean, the
  grain is upstream in module/sample rendering. If the tone is grainy, the defect is in the
  remaining I2S/codec/analog output path. Record the owner observation before choosing another
  change.

Implemented as an A1S-only, default-off `tone` feature. It is compile-time incompatible with the
no-audio `bench` feature. A 256-entry rounded sine table at signed amplitude 4 096 advances by one
table entry per frame, producing 172.265625 Hz at 44 100 Hz and returning to its zero crossing
every 256 frames. The active and exact-zero intervals are each 44 032 frames, or 172 full table
cycles and about 998.46 ms. One integer-only state is created before the normal
real-audio ring prefill and carried into steady refill. Both paths use the same quantum helper:
it advances the engine, applies channel routing, then overwrites the interleaved samples
immediately before the unchanged 32-bit packer. The eight muted recovery handoffs remain zeros
and do not disturb the tone state. There is no allocation, logging, panic or transcendental
calculation in the refill.

Star FX's clean same-device ADC-to-DAC path remains useful but cannot settle this comparison. Its
native 32-bit RX representation is byte-cast back to native 32-bit TX, so a matching representation
can round-trip consistently without proving separately generated CPU sample packing or rendering.

All 63 firmware-common host tests pass, including the diagnostic table's exact signed peaks and
opposite halves, range, dual-mono identity, continuity across calls, exact-zero interval and
cycle-boundary reset. Normal, `tone`, and `tone,web` A1S release builds pass. The normal image
must remain module playback, and this plan remains open until the owner has recorded whether the
gated sine itself is clean.

### Direct-tone hardware run (2026-09-14)

The main-checkout diagnostic image passed its objective flash, boot and bounded transport checks:

- Exact image: `embedded/target/starplayer-a1s-tone-merged.bin`, 478 944 bytes, SHA-256
  `cecdd7c752dcce50a8be37731500c74e273cf61010a3890a9efcf4910679cbb6`. Flash verification
  passed on the A1S at MAC `b4:bf:e9:dd:d3:e4`.
- A fresh reset explicitly reported the 1 033.59375 Hz dual-mono diagnostic at signed amplitude
  4 096 with 44 032-frame tone and silence gates. It also retained the ES8388's 32-bit Philips
  slave mode, headphone analog −12 dB, 44 100 Hz stereo 32-bit I2S and the six-quantum ring.
- At engine `0:00`, telemetry reported `written=360448` and `pushes=119`; at `0:10`, it reported
  `written=3930112` and `pushes=1280`. Engine time advanced with wall time. The 3 569 664-byte
  delta over the ten displayed song seconds is about 357.0 KB/s when sampled on telemetry
  boundaries, consistent with the required steady 352 800 B/s.
- `underruns=0` throughout. `dma_errors=1` stayed fixed at the recovered startup transient.

This accepts the diagnostic image identity, configuration and transport cadence. The owner
subsequently judged the high tone apparently clean but difficult to assess, leading to the
lower-frequency refinement below. **Owner subjective acceptance remains pending** for that
refined tone and each gated silence interval. Keep this plan open and unarchived.

### Lower-frequency listening refinement (2026-09-14)

The owner reports that the 1 033.59375 Hz tone appears clean, but its pitch makes the remaining
grain difficult to judge. Refine only the default-off diagnostic before recording acceptance:

- retain the same 256-entry table, signed amplitude 4 096, dual-mono output, engine workload,
  post-render override point, 32-bit packing, codec configuration and approximately one-second
  tone/silence gates;
- advance by one table entry per frame, producing exactly 172.265625 Hz at 44 100 Hz with a
  256-frame table cycle;
- retain 44 032 frames per gate interval, now exactly 172 complete cycles, so tone-to-silence and
  silence-to-tone transitions remain on the table's zero-crossing boundary;
- update tests, boot text and documentation which name the frequency or cycle count, and verify
  firmware-common host tests plus normal, `tone`, and `tone,web` A1S release builds;
- build the lower-frequency `tone` image from the main checkout, flash it, capture a fresh reset
  and bounded transport run, and leave the plan open for the owner's clean/grainy and gated-silence
  observation.

Implemented by changing only the integer phase step and its exact frequency/cycle descriptions.
The same 256-entry table now advances one entry per frame, so one table cycle is 256 frames and
44 032 frames is exactly 172 cycles. Compile-time assertions pin both gate intervals to that
geometry. The existing range/extrema, dual-mono, split-call continuity, exact-silence and
cycle-boundary tests now exercise the longer table cycle. All 63 firmware-common host tests and
normal, `tone`, and `tone,web` A1S release builds pass. The hardware run below accepts the refined
image's objective behavior; its listening observation remains pending.

### Lower-frequency direct-tone hardware run (2026-09-14)

The main-checkout lower-frequency image passed its objective flash, boot and bounded transport
checks:

- Exact image: `embedded/target/starplayer-a1s-tone-merged.bin`, 478 944 bytes, SHA-256
  `77451cbadf3aea86942ef5315778e2c277a485e81cba55e8ca240123893f5522`. Flash verification
  passed on the A1S at MAC `b4:bf:e9:dd:d3:e4`.
- A fresh reset explicitly reported 172.265625 Hz at signed amplitude 4 096 with 44 032-frame
  tone and silence gates. It retained the ES8388's 32-bit Philips slave mode, headphone analog
  −12 dB, 44 100 Hz stereo 32-bit I2S and the six-quantum ring.
- At engine `0:00`, telemetry reported `written=360448` and `pushes=119`; at `0:06`, it reported
  `written=2500608` and `pushes=818`. Engine time advanced with wall time. The 2 140 160-byte
  delta over the six displayed song seconds is about 356.7 KB/s when sampled on telemetry
  boundaries, consistent with the required steady 352 800 B/s.
- `underruns=0` throughout. `dma_errors=1` stayed fixed at the recovered startup transient.

This accepts the lower-frequency image identity, configuration and transport cadence. The owner
listening result is recorded immediately below. Keep this plan open and unarchived until normal
music is clean.

### Direct-tone listening result and engine-path comparison (2026-09-14)

The owner reports that the 172.265625 Hz tone is clean and the gated intervals are silent. This
accepts CPU-generated signed samples, high-aligned 32-bit slot packing, DMA refill, I2S framing,
the ES8388 configuration and the headphone analog path together. The remaining music grain is
upstream of the post-render override. `REFLEX.S3M` is itself weak evidence of an engine defect: it
is a 16 KB competition module whose four audible sources are 8-bit mono samples only 34, 130,
1 978 and 34 frames long.

Add one more A1S-only comparison which sends a controlled 16-bit sample through the actual
sequencer, fixed mixer, linear interpolator, master gain and transport:

- add a default-off `engine-tone` feature, compile-time incompatible with `bench`, the existing
  post-render `tone`, and `web`; normal builds must continue to open `REFLEX.S3M` from its flash
  image without constructing this diagnostic;
- before the audio task starts, build a one-channel native S3M `Module` with `ModuleBuilder`.
  Its single forward-looping sample is the existing 256-entry sine scaled to signed amplitude
  28 672, retains values with real 16-bit precision, and has reference rate 32 000 Hz. Row zero
  plays C-4 at volume 64; all remaining cells are native empty S3M cells. At 44 100 Hz output,
  this produces a 125 Hz tone through a fractional resample step and repeatedly crosses the
  sample loop seam;
- keep the board's engine master at 1/4, ES8388 headphone gain at −12 dB, channel routing, PCM
  packer and complete DMA path unchanged. Do not apply `OutputOverride` in this feature: every
  audible sample must come from `EmbeddedPlayer<Linear>`;
- make the boot log unambiguously identify `ENGINE-TONE`, its 125 Hz output frequency, 32 kHz
  16-bit source, amplitude, loop length, interpolation and gain. Avoid allocation, logging,
  locks and panics in `render()`; module construction may allocate before playback starts;
- add host tests for module structure, native fixed-stride S3M pattern identity, full 16-bit
  sample values, loop geometry, non-silent equal stereo render, bounded peak and warning-free
  multi-quantum `EmbeddedPlayer<Linear>` output. Verify firmware-common host tests and A1S
  normal, `tone`, `engine-tone`, and `engine-tone,lcd` release builds;
- build and flash only the standalone `engine-tone` image. A clean uninterrupted 125 Hz tone
  places the reported grain in `REFLEX.S3M`'s tiny 8-bit source material. A grainy tone leaves
  the sequencer/mixer/interpolator/master path under investigation. Record the listening result
  before changing normal playback or fidelity policy.

Implementation result: the standalone feature builds the module once before playback with a
256-frame signed-16 sample, full forward loop, 32 kHz reference rate, one-channel native S3M
pattern and fixed five-byte cells. Row zero is C-4/instrument 1/volume 64, rows 1..62 are exact
native empty cells, and row 63 carries `B00` to return to order zero. The order list remains the
small `[pattern 0, ORDER_END]`; `ControlHalf` defaults to `AtEnd::Continue`, so the detected song
loop runs indefinitely without a fade or stop. A rejected 64-copy order-list attempt sustained
the tone for about 8 minutes 11 seconds, but its scanned timeline required a 49 152-byte allocation
and the merged board image panicked before audio when that allocation failed. Native `B00` crosses
the same sample-loop seam continuously without growing the timeline. The existing
`EmbeddedPlayer<Linear>`, 1/4 master, 32-bit packer,
six-quantum DMA ring and codec path are unchanged, and no `OutputOverride` is present. Normal
firmware still borrows `REFLEX.S3M` from flash.

Native S3M panning has no mathematically exact centre: its centre value is nibble 8 of a 0..15
scale, about 6.7% right of centre. The host render test therefore requires non-silent output,
matching polarity in both channels and a tight native centre-pan balance bound instead of
byte-identical left and right samples. Changing the native pan law or duplicating samples after
render would invalidate this engine-path comparison. The structural tests additionally verify
every pattern cell including row 63's `B00`, every scaled sample value, real 16-bit precision,
extrema, loop geometry and guard data. A ten-second host render crosses the first 7.68-second
pattern loop, remains audible and warning-free, and stays within the 1/4-master peak bound.
`cargo test -p starplayer-firmware-common` passes all 66 tests, and the A1S normal, `tone`,
`engine-tone`, and `engine-tone,lcd` release builds all pass on `esp-1.97`. Owner listening of
the engine-rendered tone remains pending.

### Allocation-safe engine-tone hardware run (2026-09-14)

The final native-`B00` image passed its objective identity, boot, cadence and loop checks:

- Exact image: `embedded/target/starplayer-a1s-engine-tone-merged.bin`, 426 416 bytes,
  SHA-256 `2efdbd30c05b34f9175fb704d9eeb9b154079772cdd4e824727bb20dfa79a60d`.
  Flash verification passed on the A1S at MAC `b4:bf:e9:dd:d3:e4`.
- A fresh reset reported `ENGINE-TONE output=125 Hz source=32000 Hz signed16 amplitude=28672
  loop=256 frames song_loop=B00 interpolation=linear master=1/4`. The codec and I2S settings
  remained the accepted 32-bit Philips slave, headphone −12 dB, speaker disabled, 44 100 Hz
  stereo 32-bit transport and six-quantum ring. After `EmbeddedPlayer` opened, the 120 KiB heap
  reported 27 932 bytes used and 94 948 bytes free, confirming that the allocation-heavy
  64-order timeline is gone.
- Hardware crossed the native song loop: telemetry reached row 59 at `0:07`, wrapped to row 03
  at `0:00`, then continued through row 29 at `0:03`.
- The first telemetry line reported `written=356352`, `pushes=118`; the final line reported
  `written=4286464`, `pushes=1402`. Peak stayed at about 5 326–5 327,
  `underruns=0`, and the recovered startup `dma_errors=1` remained fixed.

This accepts the engine-path image, configuration, refill cadence and allocation-safe `B00`
loop. The owner's clean/grainy listening result for the 125 Hz engine-rendered tone remains
pending, as does clean normal-music acceptance. Keep this plan open and unarchived.

### Matched post-render A/B after engine-tone buzz (2026-09-14)

The owner reports that the engine-rendered 125 Hz tone is buzzing. A host capture of the exact
same `engine_tone_module()` path is smooth: after its startup second, its left and right peaks
are 4 796 and 5 327, the largest adjacent-sample change is 87, the 125 Hz fundamental measures
about 4 697 counts on the left, and its second harmonic is about −63 dB relative to full peak
with higher harmonics below −69 dB. There is no sample-loop discontinuity in the host output.
The earlier 172.265625 Hz, amplitude-4 096 dual-mono post-render tone was clean on this board,
so the next experiment must hold frequency, level, stereo balance, engine work, DMA and codec
configuration constant while changing only whether the audible samples came from the engine.

Implement a second A1S-only post-render diagnostic for this strict comparison:

- add a default-off `matched-tone` feature. It must construct and render the same controlled
  `engine_tone_module()` as `engine-tone`, at the same 1/4 master setting, while replacing every
  completed output quantum immediately before the shared PCM packer;
- generate a continuous 125 Hz signed integer sine at 44 100 Hz with a wrapping full-turn phase
  accumulator and the existing table-driven `starplayer::dsp::sin_q15`; do not use floating
  point or transcendental functions in the audio path;
- scale left and right to signed peaks 4 796 and 5 327 respectively, matching the measured
  steady host engine output and its native S3M centre-pan imbalance. Preserve phase across ring
  prefill, muted handoffs and every descriptor refill. This comparison is continuous and has no
  silence gate;
- make `matched-tone` compile-time incompatible with `bench`, `tone`, `engine-tone` and `web`.
  Normal, `tone` and `engine-tone` behavior must remain unchanged. Share the controlled-module
  selection cleanly between `engine-tone` and `matched-tone`, without enabling the normal flash
  image module in either diagnostic;
- print an unambiguous `MATCHED-TONE` boot line with 125 Hz, both peaks, integer Q15 generation,
  continuous operation, `engine-tone` render underneath, linear interpolation and master 1/4;
- add host tests that cover the exact rounded phase increment, independent left/right peaks,
  signed polarity, phase continuity across unequal calls, continuous output, safe handling of an
  unpaired trailing sample and a conservative adjacent-sample bound. The generator must allocate
  nothing, log nothing, lock nothing and panic nowhere in the refill path;
- verify `cargo test -p starplayer-firmware-common` and A1S release builds for normal, `tone`,
  `engine-tone`, `matched-tone`, and `matched-tone,lcd`. Build and flash only the standalone
  `matched-tone` image, record its exact size and SHA-256, reset the board, and capture enough
  serial output to prove its feature identity, codec/I2S configuration, render progress, refill
  cadence, bounded peak, zero underruns and stable DMA-error count.

Listening interpretation is binary. If this matched post-render tone is clean, the target-side
engine-rendered sample buffer differs from the clean host render and the next investigation must
compare target render data or hashes before packing. If it buzzes, the problem is downstream and
depends on the 125 Hz signal's exact level or stereo shape despite the earlier clean 172 Hz test.
Do not change normal playback, mixer arithmetic, panning law, packer, codec gain or DMA geometry
until this result is recorded.

Implementation result: `matched-tone` selects the same allocation-safe `engine_tone_module()`
as `engine-tone`, opens the same `EmbeddedPlayer<Linear>`, and applies the same 1/4 master. Its
audio override replaces each completed quantum immediately before the unchanged 32-bit packer.
The allocation-free generator calls the existing integer `sin_q15` with a wrapping `u32` phase,
uses the exact rounded increment 12 173 944 for 125 Hz at 44 100 Hz, and independently scales
left/right to signed peaks 4 796/5 327. Its state is carried from whole-ring prefill through the
muted handoffs into steady refill, with continuous output and no gate.

Feature selection excludes the normal flash module from both controlled-module diagnostics.
`matched-tone` is compile-time incompatible with `bench`, `tone`, `engine-tone` and `web`, while
`matched-tone,lcd` remains supported. Four host tests pin nearest-integer phase rounding, signed
independent peaks, same-polarity continuous output, unequal-call phase continuity, harmless
unpaired samples and a 128-count adjacent-sample bound. All 70 firmware-common tests pass. A1S
normal, `tone`, `engine-tone`, `matched-tone`, and `matched-tone,lcd` release builds pass on
`esp-1.97`. Hardware image identity and transport evidence are recorded below; owner listening
remains pending.

### Matched-tone objective hardware run (2026-09-14)

The strict post-render A/B image passed its objective identity, boot, allocation, cadence and
underlying-engine loop checks:

- Exact image: `embedded/target/starplayer-a1s-matched-tone-merged.bin`, 429 456 bytes,
  SHA-256 `9654d74c7df8121878345eaa1e956069b8f7eb700c6cd82d7cdcfdf6bb10e269`.
  Flash hash verification passed on the A1S at MAC `b4:bf:e9:dd:d3:e4`.
- A fresh reset identified `MATCHED-TONE output=125 Hz peaks=4796/5327
  generator=integer-Q15 continuous underlay=engine-tone interpolation=linear master=1/4`.
  `EmbeddedPlayer` open reported 27 932 heap bytes used and 94 948 free.
- Codec and transport remained the controlled settings: ES8388 32-bit Philips slave,
  headphone −12 dB, 44 100 Hz stereo 32-bit I2S and the six-quantum DMA ring.
- The rendered engine underneath the override crossed its native `B00` loop from row 59 to
  row 03. The first telemetry line reported `written=356352`, `pushes=117`; the final line
  reported `written=5347328`, `pushes=1743`. Peak reached the matched right-channel bound of
  5 327, `underruns=0`, and the recovered startup `dma_errors=1` remained fixed.

This accepts the matched-tone image, configuration, engine work, refill cadence, bounded peak
and native song loop objectively. The owner's clean/buzz listening result remains pending, as
does clean normal-music acceptance. Keep this plan open and unarchived.

### Recorded buzz and steady-`push_with` remediation (2026-09-14)

The owner reports that the matched post-render 125 Hz tone also buzzes and supplied a 4.27-second
phone recording. Analysis excludes the first and last second as requested, decodes the remaining
2.27 seconds from mono AAC at 48 kHz, and finds a digital spectral comb rather than analog
clipping. The strongest bin is the intended carrier at 125.244 Hz. The strongest unwanted line
is 297.363 Hz at only −10.34 dB relative to the carrier, followed by 383.057 Hz at −12.11 dB,
469.482 Hz at −12.92 dB, 555.908 Hz at −14.52 dB and 641.602 Hz at −16.47 dB. These lines are
spaced by approximately 86.13 Hz, exactly the cadence of 512 output frames, or two current DMA
descriptors, at 44.1 kHz.

This also explains why the earlier direct tone falsely accepted the transport. Its 256-entry
table advances by one entry per frame, so its waveform period is exactly one 256-frame DMA
descriptor. Its 44 032-frame active and silent gates are also whole-descriptor aligned. Repeating,
skipping or replaying a stale descriptor splices that diagnostic at the same waveform phase and
is inaudible. The continuous 125 Hz comparison deliberately changes phase across descriptors and
therefore reveals the transport splice.

The remaining suspect is now concrete: steady state uses `push_with`, while the same esp-hal
1.1.2 transport in `../star-fx` confines `push_with` to muted startup recovery and uses `push`
for every steady descriptor. `TxCircularState::push_with` returns descriptor ownership even when
its closure writes zero bytes; it advances `write_descr_ptr` by at least one descriptor while
advancing `write_offset` only by the returned byte count. Any empty or short steady offer can
therefore separate the descriptor pointer from the buffer offset. The existing counters miss an
inner `available()` failure because async `push_with` deliberately discards it.

Implement the steady-state correction without changing the accepted clock, codec or geometry:

- keep the eight muted startup `push_with` recovery handoffs. After `STARTUP_READY`, never call
  `push_with`; stage exactly one complete 2 048-byte descriptor and submit it with `push`;
- add static byte scratch beside the existing static `i16` descriptor scratch. Render and pack
  one descriptor once, before waiting for space, then preserve those exact bytes across any
  availability or push error so the engine timeline is not advanced again until submission
  succeeds;
- retain an explicit outer `available()` for diagnostics, but place it after rendering and
  immediately before `push`. There must be no rendering, packing, logging or other await between
  that check and `push`'s internal availability check. This is the ordering used by Star FX's
  clean steady loop;
- require the outer offer to be a whole number of 2 048-byte descriptors, record it in the
  existing offered counter, and submit only one descriptor even if two are free. Require a
  successful `push` to report exactly 2 048 bytes before advancing to the next render. Count
  full-ring offers as underruns and every malformed offer, short write or DMA error as an error;
- preserve all real-time rules and existing feature behavior. Do not allocate, log, lock or
  panic in refill. Do not change the six-quantum ring, three equal descriptors, 32-bit signed
  high-aligned PCM, 44.1 kHz clocks, codec registers, gain, channel routing, engine, mixer or
  tone generators;
- update the refill documentation and the embedded runbook so they no longer endorse steady
  `push_with`, clearly distinguish muted recovery from steady `push`, and record why the original
  descriptor-coherent direct tone masked this defect;
- run `cargo test -p starplayer-firmware-common`, `git diff --check`, and A1S release builds for
  normal, `tone`, `engine-tone`, `matched-tone`, and `matched-tone,lcd`. Build and flash only the
  matched-tone image. A fresh-reset capture must identify the image, cross native `B00`, advance
  at 352 800 bytes/s within whole-descriptor quantisation, keep `underruns=0`, and keep DMA errors
  stable after any recovered startup error.

The decisive acceptance is a clean owner listening result from the same continuous matched 125 Hz
tone. If clean, return to normal music without altering synthesis. If it still buzzes, capture a
second recording and compare its comb spacing before changing another layer. Keep this plan open
until normal music is accepted clean.

### Staged-refill image boot failure (2026-09-14)

The first merged staged-`push` image built and flash-verified but is not a valid listening image.
Exact image `embedded/target/starplayer-a1s-matched-tone-merged.bin` was 429 744 bytes with
SHA-256 `33e3e9c06868d0c32369cae666a246f530019efda898d5e6c7836e1e465bf4d2`. On a fresh reset it
tripped ProCpu's stack guard after codec setup and before the controlled-module identity line.
Decoded frames place the interrupted work in `ModuleBuilder::add_sample`, called by
`engine_tone_module()` at `main.rs:455`. Audio never started and the owner must not evaluate this
image.

Remove the controlled module builder's sensitivity to small firmware-layout changes before
reflashing:

- replace its function-local 256-sample PCM array and 320-byte pattern array with small temporary
  heap vectors built before playback. Reserve exact capacities, fill without panicking access,
  pass their slices to `ModuleBuilder`, and let them drop when the completed module owns its copy;
- preserve the module byte-for-byte at the public model boundary: identical signed 16-bit sample
  values, guard data, native fixed-stride S3M cells, `B00`, orders, loop and reference rate. This
  is boot-time construction only; no allocation may enter `render()` or refill;
- add or retain host assertions that prove structure and rendered output are unchanged. Run the
  70 firmware-common tests and rebuild `engine-tone`, `matched-tone`, and `matched-tone,lcd` in
  release mode;
- rebuild the main-checkout xtask, build and flash a new matched-tone image, then fresh-reset it.
  It must pass module construction and heap reporting before the staged-refill cadence can be
  evaluated. Record the new image identity and objective run separately from the rejected image.

Implementation result: `engine_tone_module()` now constructs its 256 signed PCM samples and 320
native pattern bytes in exact-capacity temporary heap `Vec`s instead of function-local arrays.
The vectors are filled by iteration and slice extension without indexed or panicking access,
then passed unchanged to `ModuleBuilder`; they drop after the completed module owns its copies.
This keeps the 512-byte PCM and 320-byte pattern off ProCpu's boot stack while leaving all
allocation before playback. Existing host tests continue to pin every source sample, its linear
guard, the fixed-stride S3M cells and `B00`, orders, loop, reference rate, ten-second rendered
signal and warning-free loop crossing. All 70 firmware-common tests and A1S `engine-tone`,
`matched-tone`, and `matched-tone,lcd` release builds pass. A replacement hardware run remains
pending.

### Staged-`push` image steady-start failure (2026-09-14)

The heap-backed replacement proves the module-construction fix but is still not a listening
image. Exact image size is 430 528 bytes and SHA-256 is
`bed2c6fcb5d03b6c9445254f6336ebda9211194dc07d8b4a1113338d5f96147c`; flash verification
passed. A fresh reset reaches the `MATCHED-TONE` identity, reports the accepted 27 932-byte heap
use, completes muted pre-roll, starts core 1 and prints `PLAY`, then panics before its first
telemetry line. Two timestamped resets reproduce the failure immediately after `PLAY`. The
compiled refill task pool is only 224 bytes and its poll entry frame is 96 bytes; both descriptor
buffers are static. Do not enlarge a stack in response to this result.

The plain async `push` path repeats `available()` internally. Earlier hardware already showed
that this second check can fail after the explicit check; the new image confirms it cannot be
used as the steady handoff on this transport. Retain the useful part of the staged design while
removing that second fallible check:

- continue to render and pack exactly one 2 048-byte descriptor into static scratch before the
  outer availability wait, preserving it until a complete handoff;
- after a valid outer offer, call `push_with` immediately, but use its closure only to copy the
  already-staged first 2 048 bytes and return exactly 2 048. Do no rendering, packing or other
  work in the closure, never consume a second descriptor from a larger contiguous offer, and
  require the returned count to equal 2 048 before advancing;
- document why this constrained steady use differs from the rejected implementation: the old
  closure rendered and consumed every available descriptor, commonly batching 512 frames and
  producing the recording's 86.13 Hz comb. The new closure has fixed one-descriptor work and
  keeps `write_descr_ptr` and `write_offset` advancing by the same complete descriptor;
- preserve all counters and accepted geometry/settings. Verify firmware-common tests and the
  normal, `tone`, `engine-tone`, `matched-tone`, and `matched-tone,lcd` release builds, then flash
  only `matched-tone` and capture a fresh reset through native `B00` before owner listening.

### Constrained-handoff image main-stack failure (2026-09-14)

The constrained one-descriptor handoff image is also rejected before listening. Exact image size
is 430 208 bytes, SHA-256
`d3ea857dc449559b03f9c138d730d9ba1eab81e19c465ba2b87ded96f56dafd3`, and flash verification
passed. Its full fresh-reset log identifies `MATCHED-TONE` and then trips the ProCpu stack guard
before `HEAP after open`. The decoded backtrace is
`linked_list_allocator::HoleList::allocate_first_fit` → `NativeSequencer::new` → `build_source`
→ `ControlHalf::load` → `EmbeddedPlayer::open` at `main.rs:472`. This confirms that moving the
diagnostic module temporaries to the heap worked, and separately shows that adding the 2 048-byte
packed descriptor as `.bss` removed too much headroom from the main stack during player open.

Keep the staged handoff without spending another byte of `.bss` on its packed buffer:

- remove `PACKED_DESCRIPTOR_SCRATCH`. Allocate one exact 2 048-byte zeroed buffer from the
  already-reserved firmware heap only after `EmbeddedPlayer::open` has completed and before DMA
  construction/playback. Move ownership into `AudioTransfer` and then the refill task; refill
  must only borrow the fixed buffer and must never allocate, resize or free it;
- make allocation failure an ordinary audio-start error where practical. Avoid constructing a
  2 048-byte temporary array on either core's stack. Keep the existing static `i16` descriptor
  scratch, whose earlier matched-tone image passed player open;
- preserve the constrained one-descriptor `push_with` logic, all module data and all accepted
  settings. Update memory documentation to distinguish reserved `.bss` heap capacity from bytes
  consumed after player open;
- run firmware-common tests and the normal, `tone`, `engine-tone`, `matched-tone`,
  `matched-tone,lcd`, and `web` A1S release builds. Rebuild and flash only matched-tone. Require
  successful player open, heap reporting, audio start, telemetry and native `B00` crossing before
  owner listening. Record the exact replacement image separately.

Implementation result: the six-quantum ring and three equal 2 048-byte descriptors remain
unchanged. `PACKED_DESCRIPTOR_SCRATCH` has been removed from `.bss`. After
`EmbeddedPlayer::open`, audio start fallibly reserves and zero-fills one exact 2 048-byte heap
buffer without a stack array, moves its box through `AudioTransfer`, and gives refill sole
ownership. Refill never allocates, resizes or frees it; the existing static `i16` render scratch
remains. For each steady descriptor it renders and packs once, then retains those exact bytes
inside a retry loop until submission succeeds. The explicit outer
`available()` rejects and counts offers smaller than or not divisible by 2 048, adds one staged
descriptor to `offered` for each valid offer, and counts a full-ring offer as an underrun. It is
followed immediately by a constrained `push_with`: the closure only copies the first staged 2 048
bytes and returns exactly 2 048, even when the contiguous offer is larger. It does not render,
pack or consume a second descriptor. A DMA error or short result is counted and retries the same
bytes; only an exact 2 048-byte result advances the engine to the next descriptor.

The offered counter deliberately adds 2 048 rather than the HAL's complete available byte count.
When two descriptors are free, this iteration submits only one and the next iteration can observe
the other again; adding the complete offer both times would overcount physical descriptor bytes.

The earlier steady `push_with` closure rendered and consumed every complete descriptor in its
offer, commonly batching two descriptors or 512 frames; the recording exposed a comb at precisely
that cadence. The new closure performs fixed one-descriptor work on already-staged bytes, keeping
the descriptor pointer and byte offset moving together while avoiding plain `push`'s second
fallible availability check. The default build's heap capacity remains reserved in `.bss` and the
web heap remains in `dram2_seg`; consuming 2 048 bytes from either fixed heap after player open
does not move `_bss_end` or reduce ProCpu's main-stack address space. The accepted clocks, codec
registers, gain, routing, PCM packing, engine and diagnostic generators are unchanged. All 70
firmware-common tests and A1S normal, `tone`, `engine-tone`, `matched-tone`,
`matched-tone,lcd`, and `web` release builds pass. The objective hardware result follows.

### Heap-backed constrained-handoff objective hardware run (2026-09-14)

The replacement image passes its objective identity, allocation timing, codec, transport,
cadence and native-loop checks:

- Exact image: `embedded/target/starplayer-a1s-matched-tone-merged.bin`, 431 232 bytes,
  SHA-256 `abea2c63d31ede16a6570c50a2bcf3b1be5fca2328986e93a6308c4e19c0476a`.
  Flash hash verification passed on the A1S at MAC `b4:bf:e9:dd:d3:e4`.
- A fresh reset identified `MATCHED-TONE`. Heap use was 27 932 bytes with 94 948 free after
  `EmbeddedPlayer::open`, then 29 980 used with 92 900 free after audio start: the exact 2 048-byte
  increase required by the packed descriptor, allocated only after the player opened.
- The controlled transport remained ES8388 32-bit Philips slave with headphone analog −12 dB,
  44 100 Hz stereo 32-bit I2S and the six-quantum ring.
- The native S3M crossed `B00` from row 59 to row 03. The first transport line reported
  `offered=362496 written=362496 pushes=177`; the final line reported
  `offered=5330944 written=5330944 pushes=2603`. Peak stayed at or below 5 327,
  `underruns=0`, and the recovered startup `dma_errors=1` remained fixed.

This accepts the heap-backed constrained-handoff image objectively: player construction,
post-open allocation, audio startup, complete one-descriptor handoffs, sustained byte cadence and
the native song loop all ran without a stack failure or growing transport counter. The owner's
clean/buzz result for the continuous matched 125 Hz output remains pending, as does clean normal
music. Keep this plan open and unarchived.

### Irregular buzz and swapped-channel discriminator (2026-09-14)

The owner reports that the heap-backed constrained-handoff tone still buzzes, now irregularly.
Pressing KEY1 once leaves the 125 Hz tone present in both channels but moves the buzz to the right
channel; pressing KEY1 again returns the buzz to the centre. In the six-key build KEY1 only
toggles `ControlHalf` between play and stop. `MatchedTone` runs after render with independent
state and fixed samples, so KEY1 changes neither tone phase nor left/right samples, packer, DMA,
codec or gain. The observation therefore proves that the central component depends on whether
the underlying engine is actively rendering. The stopped right-only component could either be a
physical right-path defect or follow the right channel's deliberately higher 5 327-count peak
versus 4 796 on the left.

Add one final controlled channel discriminator without changing the accepted handoff:

- add a default-off A1S `swapped-tone` feature which selects the same controlled engine module,
  1/4 master, 125 Hz Q15 generator, phase increment and continuous post-render placement as
  `matched-tone`, but swaps only the generator peaks to 5 327 left and 4 796 right;
- preserve `matched-tone` unchanged for reproducibility. Share generator implementation and add
  an explicit constructor/configuration for swapped peaks; phase must still continue from prefill
  through every handoff. Print `SWAPPED-TONE` with both channel peaks and the engine underlay;
- make the feature incompatible with `bench`, `tone`, `engine-tone`, `matched-tone` and `web`,
  while allowing `lcd`. KEY1 must retain its ordinary play/stop mapping so the owner can compare
  engine-active and engine-stopped states;
- add host assertions that the swapped generator reverses the exact signed peaks while retaining
  polarity, phase and continuity. Keep refill allocation-free and leave the heap-backed packed
  descriptor, constrained one-descriptor `push_with`, counters, geometry, PCM packing, clocks and
  codec registers unchanged;
- verify firmware-common tests and A1S normal, `matched-tone`, `swapped-tone`, and
  `swapped-tone,lcd` release builds. Build and flash only `swapped-tone`, capture objective boot
  and cadence evidence through `B00`, then have the owner listen once while playing, press KEY1
  once and listen while stopped, and report where the buzz is in each state.

Interpretation: if stopped buzz moves to the left, it follows the higher numeric level; if it
stays on the right, it follows the physical right codec/headphone path. If the centred irregular
component disappears whenever stopped in both images, it follows engine workload or timing. Use
that result to choose the next fix; do not alter normal playback yet.

Implementation result: the default-off A1S `swapped-tone` feature selects the same heap-built
native S3M, 1/4 master, post-render position, integer-Q15 lookup and 12 173 944-unit phase step as
`matched-tone`. The shared `MatchedTone::with_peaks` constructor stores independent channel peaks;
the existing `new` constructor still selects 4 796 left and 5 327 right byte-for-byte, while the
swapped build selects 5 327 left and 4 796 right. One generator state remains in `OutputOverride`
from synchronous prefill through muted startup and steady refill, so neither phase nor placement
changes at a handoff. Boot identifies `SWAPPED-TONE` and both peaks. The feature rejects `bench`,
`tone`, `engine-tone`, `matched-tone` and `web`, permits `lcd`, and does not touch KEY1 handling.

Host assertions pin both positive and negative swapped extrema, same-polarity channels, bounded
adjacent samples and byte-identical phase continuity across unequal calls. All 71 firmware-common
tests and A1S normal, `matched-tone`, `swapped-tone`, and `swapped-tone,lcd` release builds pass. The
heap-backed buffer, constrained one-descriptor `push_with`, counters, descriptor geometry, PCM
packing, clocks, codec and original `matched-tone` configuration are unchanged. Objective hardware
evidence follows. The owner's playing/stopped channel result remains pending; keep this plan open.

### Swapped-tone objective hardware run (2026-09-14)

The flashed discriminator image passes its objective identity, allocation, codec, transport,
cadence and native-loop checks:

- Exact image: `embedded/target/starplayer-a1s-swapped-tone-merged.bin`, 431 424 bytes,
  SHA-256 `6ce03f768fde170c8a11e039d8ef29a9197039e9608a16f75c61b4682d7575b6`.
  `esptool` flash-hash verification passed on the ESP32 at MAC `b4:bf:e9:dd:d3:e4`.
- A fresh reset identified `SWAPPED-TONE` at 125 Hz with peaks 5 327 left and 4 796 right.
  Heap use was 27 932 bytes with 94 948 free after player open, then 29 980 used with
  92 900 free after audio start: the expected exact 2 048-byte increase.
- The codec remained ES8388 32-bit Philips slave with headphone analog at -12 dB. I2S remained
  44 100 Hz stereo 32-bit with the six-quantum ring.
- The native S3M crossed `B00` from row 59 to row 03. Final transport counters were
  `offered=5330944 written=5330944 pushes=2603`; peak stayed at or below 5 327,
  `underruns=0`, and the recovered startup `dma_errors=1` remained fixed.

This accepts the swapped-tone image objectively: the exact exchanged channel peaks, post-open
allocation, controlled codec/I2S configuration, sustained one-descriptor cadence and native song
loop all behaved as specified. The owner's playing/stopped listening result remains pending, as
does clean normal music. Keep this plan open and unarchived.

### Swapped-tone listening result and 48 kHz reference discriminator (2026-09-14)

The owner hears two independent faults in the swapped image while the engine plays: one buzz fixed
on the right and another centred. Pressing KEY1 once stops the engine underlay and removes only the
centred buzz; the right buzz remains. A very occasional centred pop also remains over the stopped
tone. Pressing KEY1 again resumes the engine and the centred buzz returns. Because `swapped-tone`
moved the larger numeric peak from right to left while the fixed buzz stayed right, sample
signedness, sample magnitude and left/right slot ordering do not explain that component. The
centred buzz follows active engine work even though the post-render samples are identical. The pop
shows that the stopped path is not perfectly continuous either.

The clean same-board reference in `../star-fx` uses the same ES8388 slave, 32-bit Philips slots,
256x MCLK ratio, output pair and esp-hal 1.1.2 I2S driver, but runs at 48 000 Hz rather than
StarPlayer's 44 100 Hz. At a 160 MHz I2S source, esp-hal selects MCLK dividers 14 + 5/29 for
44.1 kHz and 13 + 1/48 for an exact-average 48 kHz. Test that remaining clock/rate difference
without changing the accepted handoff:

- add a default-off A1S `reference-rate-tone` feature. It must use the same controlled native S3M
  underlay, linear interpolation, 1/4 master, original 4 796-left/5 327-right comparison peaks,
  post-render placement, heap-backed packed descriptor, constrained one-descriptor `push_with`,
  codec register sequence, DMA geometry and KEY1 play/stop mapping as `matched-tone`;
- change only the A1S output sample rate from 44 100 to 48 000 Hz and derive the continuous 125 Hz
  Q15 phase increment from that selected rate. The phase increment is 11 184 811 at 48 kHz. Keep
  `MatchedTone::new()` and the existing `matched-tone` and `swapped-tone` images byte-for-byte at
  their current 44.1 kHz phase increment. Share a safe explicit sample-rate/configuration path;
- use one selected A1S sample-rate value consistently for `EmbeddedPlayer::open`, I2S setup, DMA
  timing text and `NowPlaying` elapsed-time conversion. Do not change firmware-common's 44.1 kHz
  golden/bench constant or the C5 bench. Print `REFERENCE-RATE-TONE`, 48 000 Hz, 125 Hz and both
  peaks at boot so the flashed image is unambiguous;
- make `reference-rate-tone` incompatible with `bench`, `tone`, `engine-tone`, `matched-tone`,
  `swapped-tone` and `web`, while allowing `lcd`. Add host assertions for the exact 48 kHz phase
  increment, signed channel extrema, continuity across unequal descriptor-sized calls and the
  unchanged 44.1 kHz constructor;
- verify firmware-common tests and A1S normal, `matched-tone`, `swapped-tone`,
  `reference-rate-tone`, and `reference-rate-tone,lcd` release builds. Build and flash only
  `reference-rate-tone`, capture identity, heap, transport and `B00` evidence through at least one
  loop, then have the owner listen while playing and after one KEY1 press. The owner should report
  the centred buzz and intermittent pop separately and may ignore the already-isolated fixed-right
  buzz for this comparison.

If the centred buzz and pop disappear at 48 kHz, adopt 48 kHz as the A1S hardware rate and test
normal music before closing I3a. If they remain, the sample clock/rate difference is ruled out;
instrument render duration and replace the constrained steady `push_with` with the clean
reference's post-prime `push` in a separate controlled step. Keep normal playback unchanged until
this discriminator is heard.

Implementation result: the default-off A1S `reference-rate-tone` feature selects the same native
S3M underlay, linear interpolation, 1/4 master, original 4 796-left/5 327-right peaks and
post-render override as `matched-tone`. It preserves the heap-backed packed descriptor, constrained
one-descriptor handoff, codec sequence, ring geometry and KEY1 play/stop behavior. The feature is
incompatible with `bench`, `tone`, `engine-tone`, `matched-tone`, `swapped-tone` and `web`, and may
be combined with `lcd`.

One board-local `OUTPUT_SAMPLE_RATE_HZ` selects 48 000 only for this feature and otherwise aliases
firmware-common's unchanged 44 100 Hz constant. Player construction, I2S configuration, ring and
pre-roll timing text, and both display/web and UART `NowPlaying` elapsed conversions use that one
value. Boot identifies `REFERENCE-RATE-TONE`, 48 000 Hz, 125 Hz and the original channel peaks.
`MatchedTone` now stores its phase increment and offers a checked sample-rate constructor; the
reference build derives 11 184 811 from its selected rate, while `new()` and `with_peaks()` retain
the original 12 173 944 step and existing matched/swapped output.

Host tests reject a zero rate, prove nearest rounding at 48 kHz, pin both phase increments and the
signed channel extrema, and compare byte-identical output/state across unequal one- and
two-descriptor calls. All 73 firmware-common tests and A1S normal, `matched-tone`, `swapped-tone`,
`reference-rate-tone`, and `reference-rate-tone,lcd` release builds pass. Hardware evidence and
the owner's playing/stopped reference-rate result follow; normal-music acceptance remains pending.

### Reference-rate-tone hardware and listening run (2026-09-14)

The flashed 48 kHz discriminator passes its objective identity, allocation, codec, transport,
cadence and native-loop checks:

- Exact image: `embedded/target/starplayer-a1s-reference-rate-tone-merged.bin`, 431 504 bytes,
  SHA-256 `4a16eafd5ffb3fee26d101f0de9dc3e5455d36376afabc8c56653279c40f603e`.
  `esptool` flash-hash verification passed on the ESP32 at MAC `b4:bf:e9:dd:d3:e4`.
- A fresh reset identified `REFERENCE-RATE-TONE` at 48 000 Hz, output 125 Hz, with peaks
  4 796 left and 5 327 right. Heap use was 27 932 bytes with 94 948 free after player open,
  then 29 980 used with 92 900 free after audio start: the expected exact 2 048-byte increase.
- The codec remained ES8388 32-bit Philips slave with headphone analog at −12 dB. I2S ran at
  48 000 Hz stereo 32-bit with the six-quantum, 768-frame, 16 ms ring; eight muted descriptor
  handoffs took about 42 ms.
- The native S3M crossed `B00` from row 59 to row 03. Final transport counters were
  `offered=5804032 written=5804032 pushes=2834`; peak stayed at or below 5 327,
  `underruns=0`, and the recovered startup `dma_errors=1` remained fixed.

This accepts the reference-rate image objectively: its exact rate and generator identity,
post-open allocation, controlled codec/I2S configuration, sustained one-descriptor cadence and
native song loop all behaved as specified. The owner also accepts the listening result: the 48 kHz
comparison is much better, with no buzzing at all, and pressing KEY1 has no audible effect. This
implicates the 44.1 kHz output configuration in the previously reported buzz components and
stopped-path pop. Clean normal music at 48 kHz remains pending. Keep this plan open and unarchived.

### Adopt 48 kHz for audible A1S firmware (2026-09-14)

The accepted comparison changes the diagnosis into a production fix. The same generated samples,
peaks, engine work, codec registers, DMA geometry and constrained handoff buzzed at 44.1 kHz and
were clean at 48 kHz. Make 48 000 Hz the hardware output rate for ordinary A1S playback and obtain
the final normal-music listening result:

- select 48 000 Hz for every production A1S audio personality: default, `lcd`, `web`, and
  `web,lcd`. Use that rate consistently for player construction, I2S, DMA/pre-roll timing text and
  `NowPlaying`, as the accepted reference image does. Keep `reference-rate-tone` at 48 kHz;
- retain firmware-common's 44 100 Hz constant and every A1S/C5 bench render at 44.1 kHz so the
  committed golden hashes and CPU-budget comparison remain byte-identical. Retain the historical
  `tone`, `engine-tone`, `matched-tone` and `swapped-tone` diagnostic images at 44.1 kHz so every
  accepted or failed diagnosis remains reproducible. Express this as one explicit board-rate
  selection rather than scattering feature checks among call sites;
- amend M8 master-plan decision 2: `Linear` and the `i16` stereo boundary remain fixed; 44.1 kHz is
  the canonical bench/golden rate, while objective and owner evidence in I3a establishes 48 kHz as
  the A1S ES8388 audible-output rate. Update the embedded budget configuration table and runbook
  wherever they still describe normal A1S output as 44.1 kHz. Do not change product-wide golden or
  accuracy policy;
- do not change codec routing or gain, DMA buffers/descriptors, refill behavior, PCM packing,
  engine semantics, module images, buttons or diagnostic generators. Normal playback must have no
  post-render override;
- verify all firmware-common tests and A1S release builds for normal, `lcd`, `web`, `web,lcd`,
  `bench`, `matched-tone`, and `reference-rate-tone`. The 44.1 kHz golden hash test must remain
  unchanged. Build and flash only the normal default image from merged `main`; capture its module
  identity, 48 kHz I2S line, heap, transport counters and at least one native song loop. Then have
  the owner listen to normal music, exercise several volume steps, stop/resume with KEY1, and report
  whether it is clean and free of buzz, pops, gating and speed/pitch errors.

If normal music is accepted, record the run, move this plan to `plans/engine/complete/`, update the
plans index and close I3a. If the generated tone is clean but normal music is not, keep the plan
open and diagnose only the remaining music-specific fault from that report.

Implementation result: one board-local selector now assigns 48 000 Hz to every audible production
A1S personality (`default`, `lcd`, `web` and `web,lcd`) and to `reference-rate-tone`. It assigns
firmware-common's unchanged 44 100 Hz rate to the historical `tone`, `engine-tone`, `matched-tone`
and `swapped-tone` audio diagnostics. The no-audio bench continues to use the firmware-common
constant directly. The existing single audio rate value still feeds player construction, I2S,
DMA and pre-roll timing text, and every `NowPlaying` conversion. Normal playback installs no
post-render override.

M8 decision 2 now distinguishes the canonical 44.1 kHz bench/golden rate from the accepted 48 kHz
A1S ES8388 audible-output rate. The embedded budget configuration table and runbook make the same
distinction, retain all historical rates, and show the production 48 kHz ring and pre-roll timing.
Codec routing and gain, DMA geometry and refill, PCM packing, engine semantics, module images,
buttons and diagnostic generators are unchanged.

All 73 firmware-common tests pass, including the unchanged 44.1 kHz REFLEX golden-hash assertion.
A1S normal, `lcd`, `web`, `web,lcd`, `bench`, `matched-tone` and `reference-rate-tone` release
builds pass. The merged-main normal hardware and listening evidence follows. No separate volume or
KEY1 acceptance is claimed.

### Normal 48 kHz production objective hardware run (2026-09-14)

The flashed default production image passes its objective identity, allocation, codec, transport,
song-end restart and sustained-stability checks:

- Exact image: `embedded/target/starplayer-a1s-merged.bin`, 479 072 bytes, SHA-256
  `4939a5bfa86178e87b829f60adc281f6e6c742688be8f9bab031d265dfba5cd1`.
  `esptool` flash-hash verification passed on the ESP32 at MAC `b4:bf:e9:dd:d3:e4`.
- A fresh normal boot identified the linked module image as 14 984 bytes with three channels and
  four samples. Heap use was 79 028 bytes with 43 852 free after player open, then 81 076 used
  with 41 804 free after audio start: the expected exact 2 048-byte descriptor allocation.
- The codec remained ES8388 32-bit Philips slave with headphone analog at −12 dB. I2S ran at
  48 000 Hz stereo 32-bit with the six-quantum, 768-frame, 16 ms ring; the eight muted descriptor
  handoffs took about 42 ms.
- REFLEX progressed to `2:16`, reached its end, restarted at order zero and continued to `0:07`.
  Final transport counters were `offered=57264128 written=57264128 pushes=27961`, with
  `underruns=0` and the recovered startup `dma_errors=1` fixed throughout. No reboot occurred.

This accepts the normal 48 kHz production image objectively: its module identity, post-open
allocation, codec/I2S configuration, complete one-descriptor cadence, song-end restart and long
run remained healthy. The owner's normal-music acceptance follows. Volume-step and KEY1 behavior
were not reported separately.

### Owner acceptance and completion (2026-09-14)

After listening to the exact normal 48 kHz production image documented above, the owner reported:
“Sounds good. How do I get it to play other tracks?” This accepts normal production sound and
completes I3a's original remediation goal: the refill recovers its startup transient, sustains one
complete descriptor per handoff at the required byte rate, restarts the song without gating or
rebooting, and the adopted ES8388 output rate is audibly clean.

The owner did not report a separate volume-step or KEY1 result, so this plan makes no such claim.
Those broader UI checks and loading other tracks remain ordinary M8 follow-up and do not block the
completed DMA/audio remediation. Archive this plan as complete.
