# M8 — I9: A1S multichannel and IT voice-capacity benchmark

| Field | Value |
|---|---|
| Milestone | M8 |
| Status | Complete; hardware verified 2026-09-15; production capacity owner acceptance pending |
| Recommended model | GPT-5.6-sol, high |
| Depends on | I8d PSRAM module decoding and verified 48 kHz A1S audio path |

## Context for a fresh agent

The A1S web firmware currently constructs a fixed eight-channel/eight-voice player. The
engine and IT implementation support 64 pattern channels and 256 IT-owned virtual voices,
but internal RAM and real-time CPU limits on the 240 MHz dual-core ESP32 have not been
measured. The owner wants large multichannel songs, with 32 simultaneous voices as the
minimum useful target and 256 as the IT-format maximum. Every audible A1S result must use
48 kHz: the earlier DMA/audio investigation established that 44.1 kHz produces buzzing on
this codec path.

This task establishes two limits: audio-only with Wi-Fi absent, and the normal web
personality under HTTP/WebSocket/upload load. It starts at 64 channels/256 voices and uses
binary bisection rather than exhaustively soaking every capacity. A host script consumes
the long UART transcript, performs repeated validation, preserves raw evidence, and prints
only bounded summaries so routine hardware verification does not consume model context.

The existing 44.1 kHz golden bench remains unchanged. Historical STARPLAY trees are
read-only. Follow AGENTS.md, including sibling-worktree delegation, explicit staging, no
`cargo fmt`, no attribution, and `--no-ff` merges.

The accepted production DMA geometry is six render quanta in three descriptors: 256
frames per descriptor at 48 kHz. Capacity cases must retain that 5,333.33 µs deadline;
changing descriptor size would benchmark a latency budget the shipping path does not
have. Because engine pools are fixed allocations, the host controller builds, flashes
and captures one compile-time-sized candidate per reboot, preserves each raw UART log,
and assembles one sequenced four-mode proof. It must execute both boundary soaks rather
than expect one firmware build to report the whole search.

## Deliverables

1. Add a dedicated A1S `voice-bench` personality using the production 48 kHz fixed-point
   stereo engine, codec setup, core assignment, I2S refill path, DMA geometry and PSRAM
   sample access. Support an audio-only build where Wi-Fi is not initialized and a web
   build driven under the production network workload.
   Give rustc worker threads a 256 MiB minimum stack for the repeated Xtensa thin-LTO
   candidate builds; a build failure remains fatal rather than capacity evidence.
   Before web benchmark network startup, require 56 KiB total free internal heap: the
   radio's 48 KiB dynamic allowance plus the benchmark's retained 8 KiB floor. Return a
   structured setup rejection instead of entering an infallible esp-rtos task allocation.
2. Provide a deterministic native IT stress module/workload with up to 64 active pattern
   channels and controlled NNA plateaus through 256 simultaneous voices. Use looping,
   non-silent PSRAM-resident PCM, distributed pitches/pans, and deterministic gain scaling
   so the workload stresses mixing without deliberately clipping. Provide filtered and
   unfiltered cases; retain click-prevention gain ramps, the limiter, IT voice stealing and
   the fixed-point IT resonant filter.
3. Add an embedded compact engine layout suitable for the A1S: master-bus-only mixing with
   one reusable quantum routing buffer, no per-channel insert buffers, no scope rings,
   one-entry telemetry, and no unused
   floating-point filter state in a fixed-only mixer. Keep hot mixer voices and filter delay
   state in internal RAM. If required for higher capacities, use an explicit fixed,
   claim-only PSRAM arena for colder IT articulation/envelope state; do not add a general
   PSRAM allocator. Preserve the existing full engine layout and output for desktop, WASM,
   normal builds and all other paths.
4. Instrument each production 256-frame descriptor render. At 48 kHz its deadline is
   5,333.33 microseconds. Record maximum and bounded percentile timing, deadline misses, underruns,
   DMA-error delta from the startup baseline, engine warnings, transport frames/time,
   requested and peak active voices, steals, and minimum internal/external heap. Measurement
   must not allocate, lock, log or perform unbounded work on the refill path; publish via
   atomics or another fixed RT-safe structure and format on core 0.
   Sample internal-heap low water on every 10 ms control tick and at known web control
   boundaries so a transient dip cannot disappear from later records. After five seconds
   of workload settling, every periodic sample must retain the requested voice plateau.
5. Start at 64 channels/256 voices. First find the largest runtime-constructible configuration
   that preserves the 8 KiB internal-heap reserve. A build or link failure invalidates the
   run; it is not benchmark evidence. Verify the linker stack floor separately in every
   maximum release build. At 64 channels, bisect
   the integer voice range. If 64 channels cannot sustain 32 voices, hold voices at 32 and
   bisect channels. If at least one channel passes, repeat voice bisection at the resulting
   channel ceiling. Otherwise hold one channel and bisect voices from 1 through 31,
   beginning at 31, so the result records the actual limit below the desired minimum. Use
   60-second qualification cases, then run the highest pass and immediately higher reject
   for 600 seconds each. Repeat for audio-only/web-loaded and filtered/unfiltered cases.
   If a provisional qualification pass rejects during its long soak, retain all completed
   outcomes and soak the highest qualified lower capacity. Continue with bounded
   qualification/long-soak bisection when needed until a long passing capacity and its
   immediately higher long rejection are proven; do not exhaustively scan. Preserve the
   original case name of a failed provisional soak so interrupted runs can resume it.
   The search controller must be deterministic, terminate, and log enough bounds to prove
   the reported result; test its algorithm on the host. Preserve a known adjacent channel
   reject at 32 voices when the follow-up voice search reaches the 256-voice maximum.
6. Emit versioned single-line `VOICE_BENCH` records with fixed `key=value` fields and
   monotonically increasing sequence numbers. Include `START`, periodic `SAMPLE`, `END`,
   and final `RESULT` records. Human diagnostics remain distinguishable. A missing,
   duplicated, reordered or truncated record must be detectable from the transcript.
7. Add `embedded/tests/a1s_voice_bench.py`. Its `run` mode connects to the RFC2217 serial
   endpoint, captures UART verbatim, drives continuous web load when requested, validates
   incrementally, writes structured JSON, and suppresses periodic chatter. Its `verify`
   mode replays one or more saved logs without hardware. The live runner relies on a
   two-to-three-second firmware grace after reset so capture is open before workload setup
   or the first machine record. Arguments cover serial endpoint,
   board IP, 60/600-second durations, raw-log path, JSON-report path and explicit resume;
   defaults match the
   existing MacBook bridge. It must reject malformed/sequenced records, wrong 48 kHz
   geometry, counter regression, incorrect bisection, missed voice targets, heap-floor
   violations, panic/reboot/stall/deadline/underrun evidence and incomplete boundary soaks.
   Use locked host success/failure counts and firmware-side per-boot HTTP, invalid-upload
   and WebSocket success counters to prove sustained load from the candidate board. Allow
   five seconds for those clients to connect; from the first periodic sample at or after
   that boundary, every counter must be positive and must have advanced within the prior
   five seconds. Treat exactly five seconds as passing and apply the same window at `END`.
   Bind each RESULT to the exact mode/filter limits and expected rejection dimension.
   Preserve partial UART artifacts when live validation aborts.
   Stdout is bounded to one concise line per completed case plus final limits; exit zero
   only for a complete valid run. Resume may reuse only per-case UART artifacts that pass
   strict validation against the exact requested metadata; a mismatch fails closed. Raw
   detail stays in artifact files.
8. Document the measured constructibility, heap, descriptor timing distributions, passing
   boundary and adjacent rejection for the four modes. Recommend the lowest applicable
   verified limit for production, but leave the shipping eight/eight constants unchanged
   until owner acceptance.

## Interfaces and invariants

- The compact routing/storage selection must be an explicit engine setting or concrete
  fixed-path implementation with at least the existing full implementation and the A1S
  implementation present; do not commit a speculative trait.
- No allocation, locks, formatting, logging, panics or transcendental functions enter
  `render()`. Sample-exact event splitting, whole-quantum DSP and buffer-size-independent
  output remain intact.
- Disabling unused floating-point storage must not disable IT filters or change fixed-path
  output. Master-only routing must be byte-identical when every channel insert chain is
  empty. Add a direct equivalence test before using it in firmware.
- The machine log schema is versioned and parsed strictly. Unknown future keys may be
  retained in JSON, but missing required keys or unknown schema versions fail verification.
- A pass requires zero post-startup underruns, no DMA-error increase, zero descriptor
  deadline misses, correct 48 kHz transport rate, the requested active-voice plateau, no
  engine warnings/crash/allocation failure, at least 8 KiB internal heap, and maximum
  descriptor render time no greater than 4,266 microseconds (20% deadline headroom).

## Verification

- Host tests: stress IT validity, exact channel width, NNA plateaus, stealing at pool caps,
  deterministic output, non-clipping gain, filtered activity, compact/full routing
  equivalence, search convergence and exact boundary selection.
- Script tests: valid live/replay transcripts, sequence gap/duplicate/reordering,
  truncation, restart/panic, startup DMA baseline, counter regression, timing/transport
  failure, heap floor, incorrect next midpoint, absent reject boundary, long-soak fallback,
  forged stable-boundary evidence, bounded stdout and deterministic JSON. Use a fake
  RFC2217/log source; hardware is not required for tests.
- Existing root CI, conformance, RT-safety, goldens, buffer sizes 1/3/64/128/4096/8191,
  both bare-metal targets, embedded common/host tests and clippy appropriate to touched
  crates. Do not regenerate expectations or run `cargo fmt`.
- Release A1S builds: `voice-bench`, `voice-bench,web`, normal `web`, and `web,lcd`, all
  at `ESP_LOG=info`; inspect stack frames and preserve the 32 KiB linker assertion.
- Hardware: flash and capture audio-only then web-loaded searches, run the verifier again
  in replay mode, and retain raw UART/JSON/web-load artifacts. Listen at the winning
  web-loaded configuration. Startup DMA errors are a baseline; only increments fail.
- `git diff --check`, explicit status review, no generated assets, logs, owner modules or
  `__pycache__` committed.

## Out of scope

Changing the canonical 44.1 kHz golden rate, removing gain ramps or fixed IT filters,
testing the 16 MIDI-reserved slots, enabling channel inserts on the A1S, changing module
formats, a general PSRAM allocator, flash partition changes, and modifications to either
historical source tree.
