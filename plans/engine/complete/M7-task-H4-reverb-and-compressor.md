# M7 — H4: Reverb and compressor inserts

| Field | Value |
|---|---|
| Milestone | M7 ([master plan](M7-master-plan.md), "The task graph" section) |
| Status | Landed 2026-09-05 |
| Depends on | H1 (the `Insert` trait, `SmoothedParam`, `InsertKind`/`build`), H2 (`DspSample`, tables, `DelayLine`) |
| Blocks | H6, H7; the M7 exit criterion ("reverb on channel 1 alone, audible and correct") |
| Parallel with | H3 |
| Recommended model | Claude Opus (a fixed-point reverb and a table-driven gain computer, each proven on both paths) |
| Verified by | agent (block-size determinism with both active, allocation hook, RT60 and gain-curve tests, `fma-check`), then reviewer |

## Context for a fresh agent

H1 landed per-channel buses and the `Insert` trait in `starplayer-dsp`
(`crates/starplayer-dsp/src/insert.rs`), the gain insert (`effects/gain.rs`), `SmoothedParam`
(`smooth.rs`) and the engine's install/parameter ring. H2 landed `DspSample` for `f32` and
`i32`, the `pow2`/`log2`/`exp` and dB tables, `time_constant_q24`, and `DelayLine`. Read both
task files' `## Research resolution` sections in `plans/engine/complete/` first — they record
what actually landed.

This task adds the two effects the milestone's exit criterion and the master bus most want. The
reverb is the one the owner will listen to; the compressor is the one whose gain computer is
the hardest thing in M7 to keep transcendental-free. Both are generic over `S: DspSample`,
smooth every audible parameter, never allocate after `build`, and never put a float on the
fixed path. H3 (EQ, delay, chorus) runs concurrently and shares `effects/mod.rs`; keep your
`InsertKind` and `build` additions to one line each.

### Code you must read before changing anything

- `crates/starplayer-dsp/src/{insert,smooth,sample,tables,delay_line}.rs`, `effects/{mod,gain}.rs`.
- `crates/starplayer-engine/src/engine.rs` (how a chain runs; `InsertCommand`) and
  `crates/starplayer-engine/tests/block_size_determinism.rs` (the insert scenario H1 added).
- `crates/starplayer-mixer/src/master.rs` — the soft limiter the master chain feeds; the
  compressor sits before it and must not fight it.
- `crates/starplayer-offline/tests/render_allocation.rs`.
- `plans/product/01-technical-architecture.md` §7.2, §7.3; `CLAUDE.md`.

## Deliverables

### 1. `Reverb` (`effects/reverb.rs`)

Freeverb's topology (Jezar's public-domain design): per channel eight parallel lowpass-feedback
comb filters into four series allpasses, the right channel's delay lengths offset by 23 frames
for width; lengths scaled from the 44.1 kHz tuning to the actual sample rate at `build`. It is
chosen because it is table-free, integer-friendly and universally recognised as "a reverb".
Parameters: room size (percent → comb feedback 0.7..0.98), damping (percent → the comb's
one-pole coefficient), width (percent), mix (percent), pre-delay (ms, 0–100, one `DelayLine`),
`freeze` switch. On the fixed path the comb and allpass feedback multiplies are `mul_q24`, the
comb state saturates, and the eight combs are summed with a `>> 3` headroom shift (state the
scaling so H6 can vectorise it without changing a bit). Comb and allpass delay lines are the
smallest power of two above each length (H2's `DelayLine`), which costs memory — record the
total per instance at 48 kHz stereo in the resolution; if it exceeds 256 KB, add a
`DelayLine::with_exact_capacity` that masks by a conditional subtract instead.

### 2. `Compressor` (`effects/compressor.rs`)

Feed-forward, stereo-linked peak detector with attack/release (ms, via `time_constant_q24`),
threshold (centi-dB), ratio (×100, 1.0–20.0, and ∞ as a switch = limiter), knee (centi-dB,
soft), make-up gain (centi-dB, with an `auto` switch), look-ahead 0 (no extra latency — the
master chain must not delay the mix against the scope taps and telemetry). The gain computer
runs **once per block** on the block's peak envelope, in the log domain through H2's
`log2_q16`/`db` tables, and the resulting gain is applied through a `SmoothedParam` across the
block, so per-frame cost is one multiply. Gain reduction in centi-dB is exposed through
`param(ParamId::GAIN_REDUCTION)` as a read-only parameter so H7 can show a meter.

### 3. Registry and descriptors

`InsertKind::{Reverb, Compressor}`, `build` arms, descriptors with units/ranges/defaults,
and the `descriptor_roundtrip` test pattern from H3 (if H3 has not merged, write it here — the
merge will de-duplicate).

### 4. Proof

- **Block-size determinism**: the H1 scenario gains a reverb on channel 1 and a compressor
  on the master, with parameter sweeps queued mid-song, on both paths, every block size.
  Byte-identical.
- **Allocation hook**: `render_allocation.rs` renders with both installed. Zero allocations.
- **Reverb**:
  - an impulse at room size 50 %, mix 100 % produces a tail whose RMS decays monotonically
    (per 100 ms window) and whose RT60 lies between 0.5 s and 3 s on both paths;
  - room size 0 % / damping 100 % decays below −60 dBFS within 300 ms;
  - `freeze` holds the tail's RMS within ±1 dB over 5 s;
  - fixed vs float segmental SNR ≥ 50 dB on a drum-loop fixture (the reverb's recursion
    amplifies rounding; 50 rather than 60 is the number to beat — record what you measure);
  - **the exit criterion**: render `reflex.s3m` (`crates/starplayer-offline`'s fixture set)
    with the reverb on channel 1 only, mix 50 %; channel 1's bus has a tail (its RMS in the
    200 ms after a note-off is > −40 dBFS) and every other channel's output is bit-identical
    to a render with no inserts. Write it as an offline test.
- **Compressor**:
  - static curve: sines at −40, −20, −10, 0 dBFS through threshold −20 dB / ratio 4:1 /
    hard knee come out at −40, −20, −17.5, −15 dBFS ± 0.3 dB after the attack has settled,
    on both paths;
  - attack/release: a −20 → 0 dBFS step reaches 90 % of its gain reduction within
    `attack × 2.3` and releases likewise;
  - the gain-reduction read-back matches the measured reduction ± 0.5 dB.
- `cargo xtask ci --job fma-check`.

### 5. Documentation

Architecture §7.2: add both to the effects table, note the reverb's memory and the
compressor's block-rate gain computer. Append `## Research resolution` here.

## Research points

1. **Freeverb on the fixed path**: the comb's lowpass-feedback loop `y = x + f·(y1·(1−d) + y2·d)`
   in Q8.24 with `i32` state — does it need `i64` state to avoid limit cycles at room size
   98 %? Measure the idle-noise floor after the tail decays; it must be silent (all zero) or
   below −90 dBFS.
2. **Compressor detector**: peak vs. RMS; peak is cheaper and the design says "peak" — confirm
   or argue for RMS with a table-driven square root (`pow2(log2(x)/2)` is available).
3. **Block-rate gain computing and pumping**: with a 128-frame block (2.9 ms at 44.1 kHz), is
   the smoothed per-block gain audibly different from per-frame? Test with a fast attack
   (0.1 ms) on a click train; if it is, compute per 32 frames.

## Verification

```
cargo test -p starplayer-dsp
cargo test -p starplayer-dsp --features std
cargo test -p starplayer-engine --test block_size_determinism
cargo test -p starplayer-offline
cargo test --workspace
cargo xtask goldens --check
cargo xtask ci --job host-tests
cargo xtask ci --job rt-safety
cargo xtask ci --job clippy
cargo xtask ci --job no-std-purity
cargo xtask ci --job fma-check
```

## Out of scope

EQ, delay, chorus (H3); host, CLI and web surfaces (H7); SIMD (H6); convolution or FDN
reverbs; side-chain input; look-ahead limiting.

## Research resolution

### Research point 1 — Freeverb on the fixed path: does the comb need `i64` state?

**No. `i32` state, six bits of headroom, and H3's flush.** Measured idle-noise floor after
the tail decays: **exactly zero**, at the top of the room-size knob where the problem is
worst.

The trap is real and it is the one H3 found in its delay's feedback, restated for Q8.24:
`DspSample::mul_q24` rounds to nearest with ties away from zero, so `round(v · f) == v` has
small non-zero solutions for every `f` above a half. At room size 100 % (`f = 0.98`) every
`|v| ≤ 24` is a fixed point — measured exhaustively in
`the_feedback_multiplys_fixed_points_are_below_minus_ninety_dbfs_and_are_flushed_anyway`,
not assumed. A comb whose feedback multiply cannot move its own state rings for ever.

Two things answer it, and 64-bit state is neither:

1. **Six bits of headroom** (`REVERB_HEADROOM_BITS`), the same device H3's EQ uses for its
   biquad cascade and `crate::filter` for IT's voice filter. It costs *nothing* here, which
   is why it is the right answer: Freeverb's input gain of 0.015 absorbs the multiplication
   (`INPUT_GAIN_Q24` reads 0.96) and the wet scale absorbs the division, so there is no
   extra multiply per frame at all. It moves the fixed points from **−62.7 dBFS to
   −98.7 dBFS** — under the −90 dBFS this research point requires on its own — and, measured
   by rebuilding the effect with `REVERB_HEADROOM_BITS = 0` and rerunning the drum-loop
   comparison, it moves fixed-versus-float agreement from **60.6 dB to 84.4 dB**.
2. **The flush** (`attenuate_q24`): a feedback multiply that leaves a sample exactly where
   it was is proof the tail has reached the arithmetic's floor, so the sample is zeroed.
   That is what turns −98.7 dBFS into exact zero. On the float path a below-unity multiply
   moves every value but zero, so it is a no-op there, and at unity — which is what `freeze`
   sets the feedback to — it cannot fire at all, which is what lets a frozen tail circulate
   bit-exactly.

`i64` comb state was rejected on its own terms as well as by this measurement: it would
double the reverb's memory (already the largest of any effect in the crate), and it would
not have removed the fixed points, only moved them — the rounding happens in `mul_q24`'s
narrowing back to the state's width whatever that width is.

**Memory per instance at 48 kHz stereo: 216,064 bytes (211 KB)** —
`the_memory_one_instance_costs_is_the_number_the_task_asked_for` asserts the exact figure.
Sixteen comb rings at 2048 frames (131,072 bytes), eight allpass rings (19,456 bytes) and a
100 ms stereo pre-delay (65,536 bytes). That is **inside the 256 KB** above which the task
required a `DelayLine::with_exact_capacity` masking by conditional subtract, so none was
added.

### Research point 2 — peak or RMS?

**Peak, as the design says.** The deciding argument is behavioural rather than arithmetic,
and it is in deliverable 2's own wording: the compressor sits in the master chain ahead of
`starplayer_mixer::master`'s soft limiter and "must not fight it". Not fighting it means
catching the peaks the limiter would otherwise have to, and an RMS detector by construction
does not see them. Measured on this crate's drum-loop fixture
(`a_peak_detector_sees_what_an_rms_detector_would_hide`) the crest factor is **9.8 dB**:
an RMS detector at the same threshold would hand the limiter peaks nearly ten decibels
hotter than it believes it is passing, and the limiter's soft knee would then be doing the
dynamics work the compressor was installed to do.

The cost is explicitly *not* the argument, and it is worth recording why the task's hint
about `pow2(log2(x)/2)` turned out not to matter. Because this gain computer already works
in the log domain, an RMS detector needs **no square root at all**: `log2(√x) = log2(x)/2`
is a shift of a value the computer already has. RMS would cost one extra multiply per sample
(the running sum of squares) and one extra shift per interval. That is affordable. It is
simply the wrong detector for this position in the chain. If a later task wants a
programme-loudness compressor for a different position, the change is two lines and half a
research point.

### Research point 3 — block-rate gain computing and pumping

**Measured, and the answer is 32 frames — but not for the reason the research point
anticipated.** The experiment mirrors H3's for the EQ's cooking interval:
`the_gain_computers_interval_is_close_to_a_per_frame_ideal` puts a full-scale click train
riding a steady −20 dBFS carrier of constant magnitude through a limiter at −20 dBFS with a
1 ms attack — the fastest the effect offers — and compares each interval's output against
the same effect computing **per frame**. The carrier is what makes the comparison mean
something: between the clicks its output *is* the applied gain, so the difference between
two intervals is their gain trajectories, which is what pumping is. The clicks themselves
are excluded; how much of a four-sample click escapes a compressor with no look-ahead is a
question about look-ahead.

| frames between gain computations | agreement with the per-frame ideal |
|---|---|
| 2 | 15.1 dB |
| 4 | 10.2 dB |
| 8 | 5.5 dB |
| 16 | 3.2 dB |
| **32 (chosen)** | **2.1 dB** |
| 64 | 1.6 dB |
| 128 (a whole block) | 1.4 dB |

The curve is monotone and has **no knee at all** — which is the finding, and it is a
negative one. At an attack of one millisecond (44 frames) *no* block-rate interval
reproduces a per-frame detector, because the interval's own peak hold is comparable to the
attack; and going from a whole block to 32 frames buys 0.7 dB, where going from 32 to 2
would buy 13. So the difference the research point asked about is real, and it is not
something a factor of four in the interval fixes.

What does decide the interval, and what the test therefore asserts, is the **timing
quantisation** the interval puts on `attack` and `release`. An update's gain is reached one
interval after the level that caused it (the ramp) plus up to one more (the observation), so
the ballistics carry up to two intervals of slack. At 32 frames that is **6 % of the default
10 ms attack's 2.3 τ**; at a whole block it would be 25 %, which is enough to make the
deliverable's own timing measurement meaningless. Sixteen would have been better still and
eight better again, at four and eight times the log/exp work; 32 is where the quantisation
stops being a material fraction of a realistic attack, and it is the number the task
proposed.

A related finding the research point did not anticipate, and which cost a failing test
before it was found: the **reduction has to be smoothed in Q8 centi-decibels, not in whole
ones**. Near the end of a long attack the one-pole's per-step change is smaller than a
centi-decibel — at 100 ms and a 1378 Hz control rate the step is 0.0109 dB while the state's
resolution was 0.01 dB — so rounding each step to the nearest centi-decibel systematically
shortened it, and a 100 ms attack measured **3 % long**. With the state in Q8 the same
measurement lands on the ideal exactly (10143 frames against 10143).

## Done differently from the task file, and why

1. **The compressor's attack/release one-pole sits on the gain reduction, not on the
   level.** Deliverable 2 says "peak detector with attack/release", which normally means
   ballistics on the detector. That topology cannot satisfy deliverable 4's own timing
   requirement, and the reason is arithmetic rather than incidental: a one-pole in the
   linear level domain reaches 90 % of a *level* step in 2.3 τ, but the gain reduction it
   produces is a logarithm of that level, and a logarithm compresses the end of an
   exponential approach. Worked through for the deliverable's own −20 → 0 dBFS step at
   threshold −20 / 4:1, a level-domain ballistic reaches 90 % of its gain change at **1.5 τ**
   on the attack and **3.6 τ** on the release — one ballistic, two very different answers,
   and the second is over half again the stated budget. Putting the one-pole on the
   reduction in centi-decibels — the quantity the requirement is *about* — makes 90 % land
   at 2.3 τ in both directions. It is still a peak detector, still feed-forward, still
   stereo-linked, and `time_constant_q24` is still what cooks the coefficient; what moved is
   which signal it smooths.

2. **The timing budget carries two gain-computer intervals.** Deliverable 4 asks for 90 %
   "within `attack × 2.3`", which is the *continuous* ideal — any discretisation exceeds it
   by construction, since the crossing can only be observed at a control update and the gain
   only reaches that update's value at the end of the ramp that follows. The test asserts
   `2.3 τ + 2 · GAIN_COMPUTE_FRAMES` and records the measured frames beside the ideal, which
   is the honest form of the same claim. Measured: 1023 against an ideal 1014 at 10 ms, 2032
   against 2029 at 20 ms, 5087 against 5071 at 50 ms, 10143 against 10143 at 100 ms — the
   largest excess is 16 frames, a third of a millisecond.

3. **Attack and release stayed in whole milliseconds, so research point 3's experiment runs
   at 1 ms rather than 0.1 ms.** Deliverable 2 specifies "attack/release (ms, via
   `time_constant_q24`)" and `time_constant_q24`'s own unit is whole milliseconds, so 0.1 ms
   is not expressible without either a new `tables` entry in a centi-millisecond unit or a
   local coefficient cooker that bypasses the table module. Both were rejected as changes to
   H2's surface made for a measurement rather than for the effect; the experiment runs at
   the fastest attack the effect actually offers, which is what the question is about.

4. **`ParamId::GAIN_REDUCTION` is a reserved meter identifier, not a parameter position.**
   Deliverable 2 asks for it "as a read-only parameter", and the task file writes it as
   `ParamId::GAIN_REDUCTION` — an associated constant rather than a descriptor index, which
   turned out to be exactly the right shape. `assert_descriptor_roundtrip` (H3's, reused
   unchanged, as instructed) requires every entry in `InsertDescriptor::params` to accept
   `set_param` at both bounds and to read back what was written, which a meter cannot do,
   and requires `param(ParamId(params.len()))` to be `None`, which rules out simply
   appending it. So `insert.rs` gained `ParamId::METER_BASE = ParamId(0xF0)`, an identifier
   range above any real parameter position (the largest descriptor in the crate has ten),
   `ParamId::GAIN_REDUCTION` in it, and `ParamId::is_meter`. Nothing else in the crate
   changed: no existing descriptor was touched, and the shared roundtrip helper was left
   exactly as H3 wrote it.

5. **`DspSample` gained two methods.** `saturate_at(bound)` because "the comb state
   saturates" means at the *headroom's* full scale, `32767 << 6`, and `DspSample::saturate`
   is fixed at 32767; it is the identity on the float path exactly as `saturate` is.
   `magnitude_i32` because a level detector has to turn a sample into a number it can compare
   against a threshold and hand to `gain_to_centi_db`, and it is the **only** place the
   compressor's two paths differ — everything after it, the envelope, the log-domain gain
   computer and the Q1.15 gain, is integer arithmetic shared by both. `f32::abs` is an
   inherent `std` method rather than a `core` one, so the float implementation clears the
   sign bit through `to_bits`/`from_bits`, in the same spirit as `crate::tables` building its
   own transcendentals.

6. **The `>> 3` comb-sum shift is `mul_q24(1 << 21)`.** Arithmetically it is exactly `2^-3`,
   but going through `DspSample::mul_q24` rounds to nearest rather than towards negative
   infinity — a truncating shift would put a half-LSB DC offset into the allpass chain,
   which is a recursion — and it makes the float path perform the identical (exact)
   division rather than a differently-rounded one. The scaling is stated in the module
   documentation for H6: the eight comb outputs are summed in `i32` (integer addition is
   associative, so a wide sum may reduce in any order) and the sum is then scaled once, so
   **the wet signal is the mean of the eight combs, not their sum**, and Jezar's
   `scalewet = 3` is folded into `WET_SCALE_Q24` together with that 8 and the headroom's 64.

7. **RT60 is measured by fitting the decay slope, not by waiting for 60 dB.** The tail
   reaches the arithmetic's floor — exact zero, by research point 1 — long before it has
   fallen 60 dB from its own start, so a literal "time to −60 dB" would be measuring the
   flush. The test fits a line through the 100 ms windows between 5 dB and 35 dB below the
   first and extrapolates to 60 dB, which is what an acoustician does with a real room and
   for the same reason. Measured at room size 50 %, mix 100 %: **1.141 s** on the fixed path
   and **1.132 s** on the float path, inside the 0.5–3 s the deliverable requires and within
   9 ms of each other.

8. **"Decays below −60 dBFS within 300 ms" is measured as one RMS over everything past
   300 ms.** A decaying reverb tail is a dense series of echoes, so a peak reading answers a
   different question than the one asked; the test takes the RMS of the whole remainder of
   the render, which is **−75.4 dBFS**. Per 100 ms window the tail reads −33.1, −41.8,
   −55.0, −65.2, −74.9, −83.9, −92.7, −108.0 dBFS and is then exactly zero, so the level
   crosses −60 dBFS at about 270 ms.

9. **The freeze measurement uses one-second windows.** Deliverable 4 asks that freeze hold
   the tail's RMS within ±1 dB over 5 s. A frozen tail is eight comb buffers circulating at
   their own periods of 1116–1617 frames, so a 100 ms window sees a different part of each
   one and reads a level that moves for reasons that are a measurement artefact rather than
   the tail changing. Over one-second windows, after a second to let the pre-freeze input
   run out of the pre-delay and the allpasses, the tail moves **0.06 dB** across five
   seconds (−19.60 to −19.54 dBFS).

10. **The exit criterion is proved by soloing, and muting is the note-off.** "Every other
    channel's output is bit-identical" needs a per-channel output, which the engine does not
    expose — the buses are summed inside `render()`. Each channel is therefore rendered
    *soloed*, by setting every other lane's `muted` flag on `Engine::channels_mut` before the
    first frame: `accumulate_masked` checks `is_muted` before it looks for a bus (H1), so a
    muted channel's voices never reach one and the mix is exactly the soloed channel's bus.
    The flags are set directly rather than sent as `Command::MuteChannel`s so that they are
    in force from frame zero rather than from whenever the command ring drained. For the
    tail, channel 1 is muted at a whole-quantum boundary four seconds in — which is a
    note-off as the *bus* sees one, and unlike hunting for a gap in the music does not depend
    on what `reflex.s3m` happens to be playing. Rounding that boundary to a whole number of
    quanta matters and cost a failing test: otherwise the engine's output ring hands the tail
    window up to a quantum of music rendered *before* the mute.

11. **The fixed-versus-float fixture is a synthesised drum loop in `effects/testing.rs`.**
    The task asks for "a drum-loop fixture" and the crate had only white noise. `drum_loop`
    is a half-second bar with five decaying noise bursts, built from the same fixed-seed
    `Xorshift32` `white_noise` uses, so it is identical on x86, ARM and WASM. Its crest
    factor of 9.8 dB is also what research point 2's measurement rests on.

## Measured numbers (2026-09-05)

Every figure is produced by a test on this branch.

**Reverb**

| measurement | value | requirement |
|---|---|---|
| RT60, room 50 % / mix 100 %, fixed path | **1.141 s** | 0.5–3 s |
| RT60, same, float path | **1.132 s** | 0.5–3 s |
| per-100 ms-window RMS, monotone from the second window | yes | monotone |
| room 0 % / damping 100 %, RMS past 300 ms | **−75.4 dBFS** | < −60 dBFS |
| room 0 % / damping 100 %, per-window | −33.1, −41.8, −55.0, −65.2, −74.9, −83.9, −92.7, −108.0, then zero | |
| freeze, one-second windows over 5 s | **−19.60 to −19.54 dBFS (0.06 dB)** | ±1 dB |
| idle noise floor after the tail, room 100 % | **exactly 0** | zero or < −90 dBFS |
| unflushed fixed-point floor, room 100 % | −98.7 dBFS (−62.7 dBFS without the headroom) | < −90 dBFS |
| fixed against float, segmental SNR, drum loop | **84.4 dB** (60.6 dB without the headroom) | ≥ 50 dB |
| memory per instance, 48 kHz stereo | **216,064 bytes** | ≤ 256 KB |

**Compressor** — static curve at threshold −20 dB / 4:1 / hard knee, on both paths:

| input | measured output | expected |
|---|---|---|
| −40 dBFS | **−40.00** | −40 ± 0.3 |
| −20 dBFS | **−20.00** | −20 ± 0.3 |
| −10 dBFS | **−17.50** | −17.5 ± 0.3 |
| 0 dBFS | **−15.00** | −15 ± 0.3 |

| measurement | value | requirement |
|---|---|---|
| frames to 90 % of the reduction, attack 10 / 20 / 50 / 100 ms | **1023 / 2032 / 5087 / 10143** | ideal 1014 / 2029 / 5071 / 10143 |
| frames to 90 %, release 10 / 20 / 50 / 100 ms | **1018 / 2032 / 5073 / 10146** | same |
| gain-reduction meter against the measured reduction | **15.00 dB against 15.00 dB** | ± 0.5 dB |
| fixed against float, segmental SNR, drum loop | **79.2 dB** | ≥ 50 dB |
| drum-loop crest factor (research point 2) | **9.8 dB** | |
| gain-computer interval against a per-frame ideal | 15.1 / 10.2 / 5.5 / 3.2 / **2.1** / 1.6 / 1.4 dB at 2 / 4 / 8 / 16 / **32** / 64 / 128 frames | |

**The exit criterion** (`crates/starplayer-offline/tests/reverb_exit_criterion.rs`):
`reflex.s3m` has three channels and all three sound. With a reverb at 50 % mix on
channel 1's bus, channels 0 and 2 rendered soloed are **byte-identical** to the same render
with no insert installed, and channel 1 is not. Over the 200 ms after channel 1 is muted,
the bus with no insert is **exactly zero** on every sample and the bus with the reverb is at
**−15.8 dBFS** and still decaying (its second half quieter than its first), against a floor
of −40 dBFS.
