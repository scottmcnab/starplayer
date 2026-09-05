# M7 — H3: EQ, delay and chorus inserts

| Field | Value |
|---|---|
| Milestone | M7 ([master plan](M7-master-plan.md), "The task graph" section) |
| Status | Ready once H1 and H2 have landed |
| Depends on | H1 (the `Insert` trait, `SmoothedParam`, `InsertKind`/`build`), H2 (`DspSample`, tables, `BiquadCoefficients`, `DelayLine`, `Lfo`) |
| Blocks | H6, H7 |
| Parallel with | H4 |
| Recommended model | Claude Opus (three effects on two arithmetic paths, each with a determinism and an audibility proof) |
| Verified by | agent (block-size determinism with every effect active, allocation hook, spectral/impulse tests, `fma-check`), then reviewer |

## Context for a fresh agent

H1 landed per-channel buses and the `Insert` trait in `starplayer-dsp`
(`crates/starplayer-dsp/src/insert.rs`), with a gain insert as its first implementation
(`effects/gain.rs`), `SmoothedParam` (`smooth.rs`), and the engine-side install/parameter ring.
H2 landed the arithmetic and the tables: `DspSample` for `f32` and `i32`, `pow2`/`log2`/`sin`
lookups, RBJ biquad cookers in Q8.24, `DelayLine` with fractional reads, and an integer-phase
`Lfo`. Read both task files' `## Research resolution` sections in `plans/engine/complete/`
before starting — they record what actually landed where the task files above guessed.

This task adds the first three real effects. Each one is generic over `S: DspSample`, so it
exists on both mix paths; each smooths every audible parameter; none allocates after
construction; none uses a float on the fixed path or a transcendental anywhere. The reverb and
compressor are H4, concurrently — you share `effects/mod.rs` (`InsertKind`, `build`); keep
your additions one line each so the merge is a union.

### Code you must read before changing anything

- `crates/starplayer-dsp/src/{insert,smooth,sample,tables,biquad,delay_line,lfo}.rs`,
  `effects/{mod,gain}.rs`.
- `crates/starplayer-engine/src/engine.rs` (how a chain is run; `InsertCommand`) and
  `crates/starplayer-engine/tests/block_size_determinism.rs` (the insert scenario H1 added —
  you extend it).
- `crates/starplayer-offline/tests/render_allocation.rs`.
- `plans/product/01-technical-architecture.md` §7.2, §7.3; `CLAUDE.md`.

## Deliverables

### 1. `Eq` (`effects/eq.rs`)

Three bands: low shelf, peaking, high shelf, each with frequency (Hz, 20–20000), gain
(centi-dB, −2400..+2400) and Q/slope (×100), plus a master `enabled`. Coefficients are
re-cooked from the tables when a parameter's smoothed value moves, once per block (not per
frame): cooking per block is the compromise between a click and per-frame `sin` lookups —
document it and make the block-rate step small enough that a full sweep over `SMOOTH_FRAMES`
is inaudible (test: no step in the output larger than the per-block coefficient delta implies).
State per band `[S; 2]` for each of left and right.

### 2. `Delay` (`effects/delay.rs`)

Stereo delay: time (ms, 1–2000; capacity chosen at `build` from the sample rate and the
maximum), feedback (percent, 0–100 — clamp below unity with a margin so the fixed path cannot
run away), mix (percent), `ping_pong` switch, and a one-pole low-pass in the feedback path
(cutoff Hz). Time changes read fractionally through `DelayLine::read_fractional` at the
smoothed delay, so a time sweep pitch-shifts like tape rather than crackling. Wet/dry is
equal-power via `sin_q15`/`cos_q15`.

### 3. `Chorus` (`effects/chorus.rs`)

Two or three modulated taps per channel (`voices` switch 2/3), rate (centi-Hz, 5–500), depth
(ms ×100), base delay (ms ×100), mix (percent), stereo spread (the right channel's LFO phase
offset). LFO from H2's `Lfo` (sine); the delay read is fractional. Because the LFO phase is an
integer that advances once per frame, the output is a pure function of frames rendered — the
determinism scenario proves it at every block size.

### 4. Registry

`InsertKind::{Eq, Delay, Chorus}` and their `build` arms; each effect's `InsertDescriptor`
lists its parameters with units, ranges and defaults in the order their `ParamId`s are numbered.
A `descriptor_roundtrip` test per effect: every `ParamSpec::default` is what `param()` returns
after `build`, and `set_param` of each bound is accepted and clamped.

### 5. Proof

- **Block-size determinism**: the H1 scenario gains one instance of each effect (EQ on channel
  0, delay on channel 1, chorus on the master), with a parameter sweep queued mid-song, on
  both paths, at every block size. Byte-identical.
- **Allocation hook**: `render_allocation.rs` renders with all three installed. Zero
  allocations.
- **Audibility, per effect, both paths**:
  - EQ: a peaking band +12 dB at 1 kHz on white noise (the core `Xorshift32`) raises the 1 kHz
    bin by 12 ± 0.5 dB in a 4096-point DFT computed in the test (a plain `f64` DFT under `std`
    is fine) and leaves 100 Hz and 10 kHz within ±0.5 dB.
  - Delay: an impulse at 300 ms / 50 % feedback / 100 % mix produces echoes at 300, 600, 900 ms
    with amplitudes 1, 0.5, 0.25 (±1 LSB on the fixed path); ping-pong alternates channels.
  - Chorus: a 1 kHz sine through the chorus has energy within ±depth-implied Hz of 1 kHz and
    the output never exceeds the input peak by more than the summed tap gains.
- **Fixed vs float**: for each effect, render the same input on both paths and assert segmental
  SNR ≥ 60 dB (`starplayer_offline::segmental_snr_db` — or a local copy in dsp's tests).
- **Bit-exactness of the fixed path across targets** is by construction (tables and integer
  arithmetic); note in the resolution that no float enters the fixed path.
- `cargo xtask ci --job fma-check` (dsp is audited; the float biquad must not fuse).

### 6. Documentation

Architecture §7.2: a short table of the effects, their parameters and the cooking rate.
Append `## Research resolution` here.

## Research points

1. **Cooking rate**: per block vs. per N frames for the EQ; measure the largest sample-to-sample
   step during a full-range gain sweep and pick.
2. **Delay feedback stability on the fixed path**: with the feedback low-pass and saturation,
   show that feedback 100 % settles rather than grows (the margin you clamp to).
3. **Chorus tap count and spread** — what sounds like a chorus rather than a flanger; record
   the defaults you chose and why.

## Verification

```
cargo test -p starplayer-dsp
cargo test -p starplayer-dsp --features std
cargo test -p starplayer-engine --test block_size_determinism
cargo test --workspace
cargo xtask goldens --check
cargo xtask ci --job host-tests
cargo xtask ci --job rt-safety
cargo xtask ci --job clippy
cargo xtask ci --job no-std-purity
cargo xtask ci --job fma-check
```

## Out of scope

Reverb and compressor (H4); host, CLI and web surfaces (H7); SIMD (H6); parametric EQ with
more than three bands; tempo-synced delay times (the engine's tempo is a source-side notion).

## Research resolution

### Research point 1 — cooking rate: per block, or per N frames?

**Per 16 frames, measured.** The task file's deliverable 1 assumed a whole block; the
research point asked for a measurement, and the measurement says a whole block is too
coarse.

The experiment (`effects/eq.rs::swept_by_hand`, kept as a test) drives a 200 Hz sine at
half scale — a low tone, whose own per-sample step is small, so a coefficient transient has
nowhere to hide — through a peaking bell swept from −24 dB to +24 dB over `SMOOTH_FRAMES`,
which is the most violent automation the parameter range allows. The bell is cooked every
`N` frames and the largest sample-to-sample step in the output is recorded:

| frames between cooks | largest step | peak output |
|---|---|---|
| 1 (the unaffordable ideal) | 1251 | 27950 |
| 4 | 1302 | 28091 |
| **16 (chosen)** | **1635** | **30111** |
| 32 | 2990 | 36866 |
| 64 | 4969 | 48517 |
| 128 (a whole block) | 7304 | 62159 |

A whole block is 5.8× the ideal's step and overshoots the settled output by 7 dB — a
resonant filter whose coefficients jump 6 dB at a time rings, and 6 dB is what a 48 dB
sweep over two quanta delivers per block. Sixteen frames is 1.31× the ideal and overshoots
by 0.6 dB, and the curve's knee is right there: going from 16 to 4 buys 20 % of a step for
4× the cooking. Sixteen also divides `DSP_BLOCK_FRAMES` (a `const` assertion pins that), so
the sub-block split is still a function of frames rendered and nothing about it depends on
the host's buffer size.

Two things make the cost acceptable at that rate. First, a band is re-cooked only when its
three smoothed values have actually moved, so a **steady EQ cooks nothing at all** and the
per-span cost in the common case is three `is_moving` comparisons per band. Second, the
cooking that does happen is bounded: eight spans per block, three bands, and only while a
ramp is live — 256 frames after a host moves a slider.

Two tests pin it: `the_cooking_interval_is_close_to_a_per_frame_ideal` asserts the ratio
above (chosen ≤ 1.5× the ideal, a whole block > 2× the chosen), and
`a_full_gain_sweep_never_steps_more_at_a_cook_boundary_than_between_them` asserts the thing
the task file actually asked for — that in the production effect the largest step across a
cook boundary is no larger than the largest step between boundaries, i.e. that the cooking
contributes no discontinuity of its own.

The delay's damping coefficient and the chorus's LFO increment stay at **once per block**.
Neither is a resonant recursion: a one-pole's coefficient change is a first-order pole
moving, and an LFO increment change is a phase velocity change. Neither rings.

### Research point 2 — delay feedback stability on the fixed path

**It settles, and the margin is 0.95 — but the clamp is not what makes it settle.** Three
bounds hold, and the third is the one that actually matters:

1. **Feedback is clamped below unity.** `FEEDBACK_MAX_Q15 = 31130` (0.95): 100 % on the
   knob loses 0.45 dB per repeat. The knob is *linear* below the clamp, so 50 % is exactly
   16384 — which is what makes the echo train's amplitudes exactly 1, ½, ¼ rather than
   almost.
2. **What is written to the line is saturated** (`DspSample::saturate`, ±32767 on the fixed
   path), so however hot the input the line's contents are bounded by construction and the
   loop gain always applies to a bounded value.
3. **A feedback multiply that leaves the sample where it was flushes it to silence.** This
   was found by measurement, not anticipated. `DspSample::scale_q15` rounds to nearest with
   ties away from zero, so `round(v · g) == v` has solutions for every `g ≥ 0.5`: at
   `g = 0.95` every `|v| ≤ 10` is a fixed point, and at *any* `g ≥ 0.5` `v = ±1` is one.
   A clamped-below-unity feedback gain therefore does **not** decay to silence on its own —
   it decays to a permanent ring at −70 dBFS carrying the shape of whatever was in the
   line. `attenuate` treats "the gain could not move this sample" as proof that the tail has
   reached the arithmetic's floor and zeroes it. On the float path a below-unity multiply
   leaves only zero unchanged, so it is a no-op there and the two paths still agree at
   79.9 dB.

`full_feedback_settles_to_silence_on_the_fixed_path` is the proof: a 1 ms delay at 100 %
feedback and fully wet, fed one impulse, is at **exactly zero** across the last block of a
32,768-frame render, and is already below a quarter of the impulse amplitude half way
through. Without the flush that test fails on the first assertion, with a steady ±10.

The damping filter is not part of the stability argument, deliberately: at DC a one-pole
low-pass has unity gain, so it bounds nothing the feedback gain does not already bound. It
is a tone control. Its top position (`DAMPING_OFF_HZ`, 20 kHz) means **off** rather than
"a 20 kHz low-pass", at every sample rate — `Delay::damping_q24_at` special-cases it above
what `tables::one_pole_cutoff_q24` does at Nyquist, because at 96 kHz Nyquist is 48 kHz and
"no damping" would otherwise be unreachable. That is also what makes the echo-amplitude
measurement possible at all: a one-pole spreads an impulse and lowers its peak, so a delay
with damping permanently on could not produce 1, ½, ¼.

### Research point 3 — chorus tap count and spread

Recorded as a table in `effects/chorus.rs`'s module documentation; the short form:

* **Base delay 15 ms** (`CHORUS_BASE_DEFAULT`). Under about 10 ms the dry and the delayed
  copy sum into a comb filter whose notches sweep audibly — a flanger. Past about 15 ms the
  ear stops hearing a comb and starts hearing a second, slightly detuned voice. The
  *range* still starts at 1 ms so a host that wants a flanger can have one; what this
  effect will not give it is feedback, which is the other half of a flanger and the thing
  that would make the fixed path's stability an open question all over again.
* **Depth 2 ms, rate 0.6 Hz.** Together these set the detuning: peak deviation is
  `f · depth · 2π · rate`, so ±7.5 Hz on a 1 kHz tone — about 13 cents, a chorus's worth.
* **Three taps**, at LFO phases 0, ⅓ and ⅔ of a turn. One tap detunes but does not thicken;
  two in antiphase cross at the centre delay twice a cycle and comb audibly when they do;
  three never coincide. Each tap is scaled by `1/voices`, so the taps sum to unity and the
  wet signal can never exceed the input's own peak — which is what makes the deliverable's
  "never exceeds the input peak by more than the summed tap gains" bound provable rather
  than empirical.
* **Spread 100 %**, mapping to *half* a turn of LFO phase offset on the right channel, so
  the two channels are in antiphase: when the left voice detunes sharp the right detunes
  flat. That is what makes a chorus wide rather than merely doubled. `spread = 0` gives a
  bit-identical pair of channels, which `the_spread_puts_the_channels_out_of_step` pins from
  both ends.

## Done differently from the task file, and why

1. **The EQ cooks every 16 frames, not once per block** — research point 1 above. The task
   file's deliverable 1 named a block; the research point asked for a measurement and the
   measurement disagreed with the guess.

2. **The EQ's fixed path pre-amplifies by six bits.** Not mentioned in the task file, and it
   turned out to be the difference between passing and failing deliverable 5. A biquad at a
   low corner frequency has its poles close to `z = 1`, where a direct form's quantisation
   noise is amplified by roughly `1/(1 − p)²` — about 70 dB for the default 120 Hz shelf at
   44.1 kHz. Measured fixed-versus-float agreement was **38.0 dB**, well short of the
   required 60. Multiplying by `2^6` on the way into the cascade and dividing by it on the
   way out moves it to **73.5 dB**. Six bits and not eight because the scaling goes through
   `DspSample::mul_q24`, whose Q8.24 coefficient is an `i32` and for which `1 << (24 + 6)`
   is the largest power of two that fits; six also leaves 256× of headroom over full scale,
   which no ±24 dB setting comes close to using. This is the same device `crate::filter`
   already uses for IT's voice filter (which pre-amplifies its delay line by 256), noted in
   §7.2. On the float path the two multiplies are by exact powers of two and cancel exactly.

3. **The delay's time smooths in Q8 frames and the chorus's depth and base in Q16 frames**,
   not in their host-facing units. `SmoothedParam`'s documentation says the unit is the
   caller's; a delay is the case where the *host's* unit is the wrong one to smooth in,
   because one millisecond is forty-four frames and a millisecond-resolution ramp would move
   the read head in forty-four-frame jumps — exactly the crackle smoothing exists to
   prevent. `Insert::param` still hands back whole milliseconds, so nothing about the host
   surface changes. The two effects use different fixed-point scales because the delay's
   two-second maximum is 88,200 frames, which does not fit an `i32` in Q16.

   A related trap, found by a failing test: the millisecond-to-frames conversion must be
   computed from both inputs each time rather than from a precomputed
   frames-per-millisecond constant. `round(44100 × 256 / 1000) = 11290` makes 300 ms
   13230.47 frames rather than the exact 13230, which spreads an echo across two frames by
   fractional interpolation and breaks the "amplitudes 1, ½, ¼" measurement outright.

4. **Four new `ParamUnit` variants**: `Hertz`, `CentiHertz`, `CentiMilliseconds` and
   `Count`. The task file specifies frequencies in hertz, a chorus rate in centi-hertz and
   depths in `ms × 100`, none of which the H1 enum could name. `Count` is for the chorus's
   `voices`, which the task file calls a "switch 2/3" but is a count of taps rather than an
   on/off. Each is one line in the enum, so a merge with H4 is a union. `Q`/slope reuses the
   existing `Ratio` (a ratio ×100), which is exactly what they are.

5. **`ParamUnit` gained variants, and `insert.rs` gained a `#[cfg(test)]`
   `assert_descriptor_roundtrip`.** The task file asks for "a `descriptor_roundtrip` test
   per effect"; writing the same twenty lines three times would have guaranteed the three
   drifted apart, so the body lives in `insert.rs` next to `ParamSpec`'s own contract and
   each effect's `descriptor_roundtrip` test calls it on both paths. **H4 should use it**
   rather than writing its own — that is why it is `pub(crate)` in `insert.rs` rather than
   in `effects/testing.rs`, which is where the measurement helpers went.

6. **`DspSample` gained a `'static` supertrait.** An effect that *holds* a `Sample` — every
   one of these three does — cannot be coerced to `Box<dyn Insert<Sample>>`, which is a
   `'static` trait object, without it. H1's gain insert held no sample, so the bound was
   never needed before. Both implementations are plain scalars, so it costs nothing and
   `build_insert`'s signature is unchanged.

7. **The block-size determinism scenario is a new function, not an extension of H1's.**
   Deliverable 5 says "the H1 scenario gains one instance of each effect". H1's scenario is
   also the subject of
   `an_insert_parameter_change_lands_on_the_same_quantum_at_every_block_size`, which asserts
   that the *first differing sample* between two runs is exactly the frame the `SetParam`
   landed on — putting a delay with a 47 ms tail into it would have made that claim about a
   different thing. `render_with_h3_effects_at_block_size` is therefore its own scenario
   beside H1's, sharing `render_phases` (whose `phases` parameter widened from `[_; 3]` to
   an `IntoIterator` so a scenario can queue as many commands as it has boundaries) and
   asserting the same invariant with all three effects live and a sweep on each.

8. **H1's gain insert now calls H2's `tables::db_to_gain_q15`**, and its own 73-entry
   whole-decibel table is gone. The conditions the task's coordination note set both hold
   and were checked before the switch: `db_to_gain_q15(0)` is **32768 either way**, so a
   gain insert at its default is still bit-transparent on both paths — the property the
   whole "buses are always on" argument rests on — and `+12 dB` is 130452 either way as
   well. The function is renamed `fader_gain_q15` rather than left sharing a name with the
   table's, because it is *not* the same function: `GAIN_MIN_CENTI_DB` means −∞ (a gain of
   exactly 0), where `tables::db_to_gain_q15` has no notion of a fader's bottom and returns
   33 there. At a *fractional* decibel the two disagree by up to 203 in Q1.15 (0.6 %),
   because the old table interpolated a chord under the exponential between whole decibels
   and the new one evaluates the exponential itself; nothing pins those values — no golden
   installs an insert — and the new numbers are the more accurate ones. It also stops being
   a `const fn`, which nothing depended on.

9. **`tables.rs` gained `one_pole_cutoff_q24` and `equal_power_q15`.** Both are
   table-driven unit conversions of exactly the kind that module already holds, both are
   wanted by two of the three effects here, and **H4's reverb will want both** — a comb's
   damping filter and a room's wet/dry. Putting them in `tables.rs` rather than in
   `effects/mod.rs` keeps the shared merge file to one line per item.

   `equal_power_q15` lifts `sin_q15`'s own 32767 peak to `Q15_UNITY` at the endpoints, so a
   delay or a chorus at zero mix is **bit-transparent** rather than one LSB quiet — the same
   property `GainInsert` has at unity, and what lets a host install either effect on a bus
   without changing it at all.

10. **The chorus's audibility test measures a band rather than a single ratio.** The task
    file asks that "a 1 kHz sine through the chorus has energy within ±depth-implied Hz of
    1 kHz". A single-frequency wet/dry ratio at 1 kHz reads −4.06 dB, which is correct
    physics and not a failure: three taps at different delays sum incoherently at any one
    frequency, and an equal-power mix then contributes its own −3 dB. So the test states the
    claim the task file actually makes — that the energy is *in that band and nowhere else*
    — by comparing the ±5-bin band around 1 kHz against the same band at 700, 850, 1150,
    1300 and 2000 Hz.

## Measured numbers (2026-09-05)

Every figure below is produced by a test in `crates/starplayer-dsp/src/effects/`, run on
this branch.

**EQ, +12 dB bell at 1 kHz on white noise, 4096-point Hann-windowed DFT ratio against the
same noise unfiltered** (`a_peaking_band_raises_its_own_frequency_and_leaves_the_others_alone`,
budget ±0.5 dB):

| | 100 Hz | 1 kHz | 10 kHz |
|---|---|---|---|
| fixed path | +0.103 dB | **+11.969 dB** | +0.110 dB |
| float path | +0.103 dB | **+11.969 dB** | +0.110 dB |

**Delay, 20000-unit impulse into 300 ms / 50 % feedback / 100 % wet / damping off**
(`an_impulse_comes_back_at_every_multiple_of_the_delay_halving_each_time`, budget ±1 LSB):

| repeat | frame | fixed | float | expected |
|---|---|---|---|---|
| 1 | 13230 | **20000** | 20000 | 20000 |
| 2 | 26460 | **10000** | 10000 | 10000 |
| 3 | 39690 | **5000** | 5000 | 5000 |

Exact, not within a bit, on both paths. Ping-pong alternates left/right/left across the
same three repeats with the crossed repeat keeping its amplitude
(`ping_pong_alternates_the_channels`). At 100 % feedback and a 1 ms time the tail is
**exactly zero** through the last block of a 32,768-frame render.

**Chorus, 1 kHz sine at 12000 units through the defaults**
(`a_sine_stays_at_its_own_frequency_and_the_output_stays_inside_its_input`):

| measurement | value |
|---|---|
| ±5-bin band at 1 kHz, against the dry | **−4.06 dB** (budget −6 … +3) |
| 700 Hz, below the tone | −74.5 dB |
| 850 Hz | −73.0 dB |
| 1150 Hz | −70.2 dB |
| 1300 Hz | −73.8 dB |
| 2000 Hz | −90.2 dB |
| input peak / output peak | 11999 / **16948** (bound 16969 = `√2 × peak`) |

**Fixed against float, segmental SNR over 4096 frames of white noise, 1024-frame segments**
(each effect's `the_fixed_and_float_paths_agree_to_better_than_sixty_decibels`, requirement
≥ 60 dB):

| effect | SNR |
|---|---|
| EQ (−9 dB low shelf, +12 dB bell, +6 dB high shelf) | **73.5 dB** |
| delay (37 ms, 60 % feedback, 50 % mix, 4 kHz damping) | **79.9 dB** |
| chorus (defaults) | **76.8 dB** |

**Bit-exactness of the fixed path across targets is by construction.** No float appears
anywhere on the fixed path in any of the three effects: every coefficient is an `i32` from
`crate::tables` or `crate::biquad` (`pow2_q24`, `exp_neg_q24`, `sin_q15`, the RBJ cookers'
Q32 integer arithmetic and integer square root), every gain is Q1.15, every delay position
is Q8 or Q16 frames, and the LFO phase is a `u32`. `cargo xtask ci --job fma-check` passes,
including its negative control, so the float path's multiplies are separately rounded as
§7.3 requires.

## Verification results (2026-09-05)

Every command in the Verification section above was run on branch `h3` and passes:

```
cargo test -p starplayer-dsp                             ok (156 passed, 0 failed, 1 ignored)
cargo test -p starplayer-dsp --features std              ok (156 passed, 0 failed, 1 ignored)
cargo test -p starplayer-engine --test block_size_determinism   ok (19 passed, 0 failed)
cargo test --workspace                                   ok (77 test binaries, 0 failures)
cargo xtask goldens --check                              ok (11/11 byte-identical)
cargo xtask ci --job host-tests                          ok
cargo xtask ci --job rt-safety                           ok
cargo xtask ci --job clippy                              ok
cargo xtask ci --job no-std-purity                       ok
cargo xtask ci --job fma-check                           ok (negative control found 1 finding)
```

The eleven goldens are unchanged, which they must be: no golden render installs an insert,
and nothing in this task touches the voice path.
