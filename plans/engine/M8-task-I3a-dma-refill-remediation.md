# M8 — I3a: the DMA refill can wedge at start-up (remediation)

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md)), remediating [I3](complete/M8-task-I3-esp32-a1s-bringup.md)'s `audio.rs` |
| Status | **Open, written 2026-09-14.** The lower-frequency direct tone and gated silence are accepted clean; the allocation-safe engine-tone image, configuration, cadence and native `B00` loop are objectively accepted. Owner clean/grainy 125 Hz listening and normal-music acceptance remain pending |
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
