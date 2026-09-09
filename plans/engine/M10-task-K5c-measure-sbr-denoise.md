# M10 — K5c: The enhancement measurement harness, bandwidth extension, and decay denoising

| Field | Value |
|---|---|
| Milestone | M10 ([master plan](M10-master-plan.md), decision 6); follows [K5a](complete/M10-task-K5a-enhancer-core.md), [K5b](complete/M10-task-K5b-enhance-offline-and-cli.md), [W4](../apps/complete/W4-task-enhancement-checkboxes.md) |
| Status | Planned 2026-09-09 |
| Depends on | K5a, K5b, W4 (all landed) |
| Blocks | — |
| Parallel with | K1, K2 |
| Recommended model | Claude Opus (two STFT/gain-domain DSP designs with a cross-target determinism contract, judged by a measurement harness the task also builds) |
| Verified by | agent (the harness's acceptance numbers, goldens untouched, determinism hashes, headless), then owner listening |

## Context for a fresh agent

Read `CLAUDE.md`, then K5a's task file and its `## Research resolution` (the trait,
`Module::enhanced`, the catalogue, the committed-table determinism pattern, and research
point 3's measurements: an 8-bit source carries ~47 dB SNR, only 1.4 % of its noise sits
above the source Nyquist, so a plain low-pass and dither at the rebuild are both useless).

The owner asked what would improve 8-bit samples further and chose this order: **measure
first**, then two classical, deterministic enhancers — **bandwidth extension** (the missing
band above an 8 kHz sample's 4 kHz Nyquist is a larger perceptual loss than its noise
floor) and **decay denoising** (quantisation noise is most audible in tails, where the
signal falls toward the LSB). A learned residual network and M11 scan-time work
(large super-resolution models, known-sample replacement) are **deferred**, not in scope.

Everything downstream is already catalogue-driven: the CLI parser (`apps/starplayer-cli/src/enhance_arg.rs`)
resolves ids through `descriptor`/`enhancer_for_id`, the web page builds its checkboxes
from `enhancements_json()`, and `from_flags` walks `CATALOGUE` in order. The one place
that is *not* generic is the wasm host's `build_enhancement`
(`crates/starplayer-host-wasm/src/lib.rs:227`), which special-cases the two existing bits;
this task generalises it.

### Code you must read before changing anything

- `crates/starplayer-enhance/src/{lib,catalogue,chain,upsample,polyphase,loop_smooth}.rs`
  — the API, `InfiniteSource` (the loop-aware periodic/reflected extension you should
  reuse for any windowed process), the committed-table + generator-test pattern.
- `crates/starplayer-model/src/enhance.rs` — `SamplePcm` (`rate_hz` is the rate the frames
  *arrive* at: `reference_rate_hz << rate_scale_log2`), `EnhancedPcm::unchanged`.
- `crates/starplayer-testkit/src/bin/starplayer-perceptual/analysis.rs` — `Fft` (radix-2,
  f64, twiddles computed with `sin`/`cos` — fine in a std harness, **not** in the
  enhancer), `log_spectral_distance_db`, the Hann window at `:183`.
- `crates/starplayer-offline/src/{lib.rs, fixtures.rs}` and `examples/bench_render.rs` —
  `render_song_with_options`, `RenderOptions`, `segmental_snr_db` (`:693`), the synthetic
  module generators (the IT one accepts an arbitrary C5 speed, which is what a
  44 100 Hz ground-truth sample needs).
- `crates/starplayer-host-wasm/src/lib.rs:80-95, :175-260` — flag constants, `decode`,
  `build_enhancement`, `enhancements_json`; `apps/starplayer-web/www/app.js`
  `ensureEnhancementCheckboxes` (builds from the JSON; nothing to change unless the JSON
  shape changes); `apps/starplayer-web/README.md` "Load options".
- `plans/product/03-accuracy-policy.md` §5 items 5 and 7.

## Deliverables

### 1. The measurement harness (std, in `starplayer-offline`)

- **Ground truth**: four deterministic 16-bit 44 100 Hz test instruments synthesised in
  code (no fixture files): (a) a plucked harmonic decay — eight harmonics at `1/n`,
  exponential decay to −60 dB over 1.5 s; (b) a sustained looped tone with slow vibrato
  and slight inharmonicity; (c) a noise-burst drum — filtered noise, 80 ms decay, from a
  seeded xorshift; (d) instrument (a) low-passed at 3 kHz — the "tape-sourced,
  oversampled" case. Each also exists as a **degraded** version: band-limited and
  decimated to 8 363 Hz (use `SincUpsampler`'s table or a matching sinc), rounded to 8
  bits and widened `× 256` — the exact shape the S3M fixtures have.
- **Evaluation through the real pipeline**: a one-sample, one-channel synthetic module
  (extend `starplayer_offline::fixtures` with a generator that takes the PCM and its rate;
  IT's C5 speed carries 44 100 Hz) playing the note at unity; render the ground-truth module
  and the degraded module under each chain with `render_song_with_options` at 44 100 Hz,
  linear kernel, fixed path; compare the two renders.
- **Metrics**: full-band segmental SNR (`segmental_snr_db`), **in-band SNR** below the
  degraded sample's Nyquist (4 181 Hz) via FFT band masking, and **log-spectral distance**
  — move `Fft`, the Hann window and `log_spectral_distance_db` out of the perceptual
  binary into a shared `starplayer_offline::analysis` module (std) and make the binary use
  it (`starplayer-testkit` already depends on `starplayer-offline`; if it does not, say so
  and copy instead).
- **Chains under test**: none, `sinc4x`, `denoise`, `denoise+sinc4x`, `sinc4x+sbr`,
  `denoise+sinc4x+sbr`, `denoise+sinc4x+sbr+loop`.
- **Outputs**: `cargo run -p starplayer-offline --example enhance_report --release` prints a
  markdown table (instrument × chain → full-band SNR, in-band SNR, LSD, and the tail-only
  in-band SNR over the last 40 % of instrument (a)); the final table goes verbatim into
  this file's Research resolution. Plus a CI-safe test `crates/starplayer-offline/tests/enhance_quality.rs`
  that pins the two acceptance criteria in deliverables 2 and 3 on instruments (a) and
  (d), so a later change that regresses them fails a build.

### 2. `DecayDenoiser` (`denoise`, catalogue bit 2) in `starplayer-enhance`

- Runs **before** the upsampler (catalogue order, below). **Floor**: when every frame is a
  multiple of 256 the source is 8-bit and the floor is `256 / √12` in i16 units (a
  committed constant); otherwise estimate it as the RMS of the quietest 5 % of 64-frame
  blocks, never below `1 / √12`. **Gain** per block: Wiener `g = max(0, 1 − floor² / rms²)`
  raised to a `strength` (default 1, the plain Wiener gain; expressed as an integer
  percent so `name()` is `denoise` or `denoise=<percent>`), with a fast attack and a
  slow release across blocks (state the constants), interpolated linearly between block
  centres so nothing steps. **Loops**: inside a forward or ping-pong loop region apply one
  constant gain, the gain of the loop's overall RMS, so the loop stays periodic and the
  seam is untouched; a sample with a sustain loop is treated the same for both loops.
  Arithmetic: i64/f64 with a fixed operation order, divisions only, no transcendental —
  bit-identical across targets.
- **Acceptance** (from the harness): on instrument (a) the tail-only in-band SNR improves
  by **≥ 3 dB** and on instrument (b) the full-band SNR drops by **< 0.5 dB**; on (c) the
  drum's attack is not shortened (first-10 ms RMS within 0.5 dB). Tune the constants to
  meet these, and if a criterion cannot be met say so with the numbers rather than
  relaxing it silently.

### 3. `BandwidthExtender` (`sbr`, catalogue bit 3) in `starplayer-enhance`

- Runs **after** the upsampler; with no headroom it is the identity (bit-identical):
  detect the **band edge** as the highest bin of the long-term average magnitude spectrum
  that exceeds the spectral floor (the top 10 % of bins) by 12 dB; if the edge is above
  0.45 × the arriving rate there is nothing to fill.
- STFT with a committed window and committed twiddle tables (`u64` bit patterns, generator
  test + regeneration gate exactly like `polyphase.rs`; choose one fixed size, e.g. 1 024,
  hop 256, and say why). Per frame, copy the complex spectrum of `[edge/2, edge)` onto
  `[edge, 2·edge)` and again up to Nyquist, each copy scaled by the source's own spectral
  tilt measured over the octave below the edge, extrapolated, plus an extra roll-off
  `tilt_db` per octave (default −6; `name()` is `sbr` or `sbr=<tilt_db>`), never above
  the level at the edge. Overlap-add back; **loops** are processed circularly through
  the periodic/reflected extension (`InfiniteSource`) so the seam stays continuous;
  one-shots are zero-extended. Output rate and loop points unchanged.
- **Acceptance**: on instruments (a) and (b) the LSD against ground truth is **lower**
  with `sinc4x+sbr` than with `sinc4x` alone; on (c) the LSD does not rise by more than
  1 dB; on (d) — content genuinely band-limited at 3 kHz — the extender must detect the
  3 kHz edge and the LSD must not get worse than `sinc4x` alone (a false extension of a
  dark sample is the failure mode to guard against).

### 4. Catalogue, hosts, docs

- `CATALOGUE` order becomes run order: `denoise` (bit 2), `sinc4x` (bit 0), `sinc2x` (no
  bit), `sbr` (bit 3), `loop` (bit 1). Bits 0 and 1 are persisted in browsers' localStorage
  and **must not change**. `from_flags`'s "unknown bit ignored" contract stays.
- wasm host: `build_enhancement` walks `CATALOGUE` in order; the upsampler entry keeps its
  frame-budget fallback; every other flagged entry is `enhancer_for_id(id, None)`;
  `applied_enhancement_flags` accumulates per entry. Unit tests for the new bits.
  `enhancements_json` needs no change if labels stay quote-free.
- CLI: `enhance_arg.rs` docs and `--list-enhancers` pick the entries up automatically;
  extend `apps/starplayer-cli/tests/enhance_flags.rs` with one `denoise+sinc4x+sbr` render.
- Docs: `apps/starplayer-web/README.md` "Load options" (the two new checkboxes and the
  recommended order); accuracy policy §5 item 5 (nothing new if the naming rule already
  covers `_enh-denoise+sinc4x+sbr`); architecture §11 crate line if it lists enhancers.
- Append `## Research resolution` with the harness table, the constants chosen, and every
  criterion's outcome.

## Research points

1. Whether `denoise` should also see a **noise-shaped** floor for 16-bit sources (rare in
   tracker modules); measure on a 16-bit synthetic and note.
2. The extender's behaviour on **chip loops** (2–64-frame single-cycle loops): with the
   loop processed circularly it is a harmonic series already; report whether `sbr` adds
   anything or should skip loops shorter than the STFT hop.
3. Worklet cost: time `denoise+sinc4x+sbr` natively on PETRI.S3M (K5a measured `sinc4x`
   at 6.9 ms release); if the chain is over ~200 ms on that fixture, say so — W4's
   research resolution set ~1 s as the threshold for moving the rebuild off the worklet.

## Verification

```
cargo test -p starplayer-enhance -p starplayer-offline -p starplayer-cli -p starplayer-host-wasm
cargo run -p starplayer-offline --example enhance_report --release
cargo test --workspace
cargo xtask goldens --check
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
cargo xtask ci --job wasm-build
cargo xtask wasm && node apps/starplayer-web/test/headless.mjs
cargo run -p starplayer-cli -- render crates/starplayer-s3m/tests/fixtures/REFLEX.S3M --enhance denoise+sinc4x+sbr+loop -o /tmp/enhanced-full.wav
```

## Out of scope

A learned residual network (browser-deliverable int8 inference), large super-resolution
models, known-sample replacement packs, anything that runs at M11 scan time; changing bits
0 and 1; a new web control beyond what the catalogue already produces.

## Research resolution

*Written 2026-09-10, on branch `k5c`. Sections 1, 3 and 4 were re-measured after the
harness itself was corrected — see section 2.*

Four of seven acceptance criteria are met. The three that are not are stated with their
numbers, with the sweeps run against them, and with what would actually reach them; none
was relaxed to make it pass, and the two pinned in
`crates/starplayer-offline/tests/enhance_quality.rs` are labelled there as regression
floors rather than as targets.

### 1. The harness table

Verbatim output of `cargo run -p starplayer-offline --example enhance_report --release`.
Both renders go through `render_song_with_options` at 44 100 Hz on the fixed path with the
linear kernel, playing a one-channel, one-sample IT whose `C5Speed` is the sample's own
rate, so the ground truth plays at unity step.

Every row renders the degraded instrument (band-limited, decimated to 8 363 Hz, rounded to 8 bits) through the named chain and scores it against a render of the 16-bit 44 100 Hz ground truth. SNRs in dB, higher better; LSD in dB, lower better.

The **log-spectral distance is floored 60 dB below the ground truth's loudest bin** (`analysis::AUDIBLE_FLOOR_BELOW_PEAK_DB`) rather than at the −120 dB the libopenmpt comparison uses: this harness asks whether something sounds closer, and a bin filled a hundred decibels down is not a difference anybody can hear. The floor is anchored to the signal rather than stated as an absolute magnitude because these magnitudes are normalised so a full-scale *sine* puts 0.25 in one bin — an absolute −60 dBFS floor left between 0.3 % and 7.2 % of the reference's bins above it and stopped the score being a measurement at all.

`tail` is the in-band SNR over the **decay window**: the frames where the ground truth's own level lies between -24 dB and -48 dB of its peak, which is where a decay crosses the 8-bit quantisation floor. `NaN` means the instrument has no such window — a sustained tone never gets that quiet.

Instruments (a) and (b) carry harmonics all the way to 20 kHz, so the band `sbr` fills is a band the ground truth genuinely has. (d) is deliberately dark and (c) is broadband noise.

#### (a) plucked decay

| chain | full-band SNR | in-band SNR | LSD | tail in-band SNR | first 10 ms |
|---|---:|---:|---:|---:|---:|
| `none` | 5.23 | 8.89 | 2.24 | 8.54 | -18.37 |
| `sinc4x` | 5.40 | 9.99 | 2.17 | 8.27 | -17.84 |
| `denoise` | 5.27 | 8.90 | 2.23 | 8.59 | -18.37 |
| `denoise+sinc4x` | 5.55 | 10.21 | 2.15 | 8.83 | -17.84 |
| `sinc4x+sbr` | 5.39 | 9.98 | 2.15 | 8.27 | -17.84 |
| `denoise+sinc4x+sbr` | 5.54 | 10.21 | 2.13 | 8.82 | -17.84 |
| `denoise+sinc4x+sbr+loop=64` | 5.54 | 10.21 | 2.13 | 8.82 | -17.84 |

Ground truth's own first 10 ms: -17.35 dB.

#### (b) sustained loop

| chain | full-band SNR | in-band SNR | LSD | tail in-band SNR | first 10 ms |
|---|---:|---:|---:|---:|---:|
| `none` | 12.07 | 19.50 | 6.98 | NaN | -18.17 |
| `sinc4x` | 12.85 | 24.72 | 6.46 | NaN | -17.88 |
| `denoise` | 12.07 | 19.49 | 6.98 | NaN | -18.17 |
| `denoise+sinc4x` | 12.85 | 24.71 | 6.46 | NaN | -17.89 |
| `sinc4x+sbr` | 12.84 | 24.72 | 6.49 | NaN | -17.88 |
| `denoise+sinc4x+sbr` | 12.84 | 24.71 | 6.49 | NaN | -17.89 |
| `denoise+sinc4x+sbr+loop=64` | 12.77 | 24.71 | 6.49 | NaN | -17.89 |

Ground truth's own first 10 ms: -17.61 dB.

#### (c) noise drum

| chain | full-band SNR | in-band SNR | LSD | tail in-band SNR | first 10 ms |
|---|---:|---:|---:|---:|---:|
| `none` | 2.41 | 6.04 | 15.78 | 6.77 | -17.32 |
| `sinc4x` | 2.54 | 7.08 | 21.30 | 7.41 | -16.30 |
| `denoise` | 2.42 | 6.05 | 16.09 | 6.80 | -17.32 |
| `denoise+sinc4x` | 2.58 | 7.19 | 21.49 | 7.67 | -16.30 |
| `sinc4x+sbr` | 2.52 | 7.08 | 19.41 | 7.51 | -16.30 |
| `denoise+sinc4x+sbr` | 2.56 | 7.20 | 19.59 | 7.77 | -16.30 |
| `denoise+sinc4x+sbr+loop=64` | 2.56 | 7.20 | 19.59 | 7.77 | -16.30 |

Ground truth's own first 10 ms: -14.11 dB.

#### (d) dark pluck (3 kHz)

| chain | full-band SNR | in-band SNR | LSD | tail in-band SNR | first 10 ms |
|---|---:|---:|---:|---:|---:|
| `none` | 9.64 | 10.88 | 0.87 | 8.85 | -18.45 |
| `sinc4x` | 12.62 | 13.63 | 0.72 | 8.26 | -18.01 |
| `denoise` | 9.65 | 10.87 | 0.86 | 8.89 | -18.45 |
| `denoise+sinc4x` | 12.82 | 13.84 | 0.70 | 8.79 | -18.01 |
| `sinc4x+sbr` | 12.40 | 13.46 | 0.76 | 8.25 | -18.01 |
| `denoise+sinc4x+sbr` | 12.60 | 13.67 | 0.73 | 8.78 | -18.01 |
| `denoise+sinc4x+sbr+loop=64` | 12.60 | 13.67 | 0.73 | 8.78 | -18.01 |

Ground truth's own first 10 ms: -17.98 dB.
### 2. The harness was judging two criteria by construction, and was corrected

Three things about the first version of this harness decided two of its own verdicts before
any enhancer ran. All three are fixed, and the fixes are why sections 3 and 4 differ from
the first draft of this resolution.

**The ground truths stopped at 5.6 kHz.** Instruments (a) and (b) were built with eight and
twelve partials, so their spectra were silent above 5.6 and 5.9 kHz — and a bandwidth
extender was then scored as *wrong* for putting anything up there, against a truth no real
instrument resembles. Both now carry `1/n` harmonics up to
[`INSTRUMENT_BANDWIDTH_HZ`] = 20 kHz, which is where hearing stops and therefore where an
instrument stops. (c) and (d) are unchanged in kind; (d) is still (a) low-passed at 3 kHz
and is still the deliberately dark case.

That change forced a second: the decimation filter went from **64 taps to 512**. A
Kaiser-windowed sinc's transition width is inversely proportional to its length, and at 64
taps it is about 4.4 kHz wide at 44 100 Hz — so with content up to 20 kHz, everything from
4 to 8 kHz folded back into the degraded sample and the harness would have been measuring
aliasing rather than the loss of a band. At 512 taps the transition is 550 Hz and fits
between the 3 847 Hz cutoff and the 4 181 Hz Nyquist with a hundred decibels to spare.

**The log-spectral distance charged full price for the inaudible.** Its floor was −120 dB,
so a bin the reference leaves empty and the candidate fills a hundred decibels down counted
as much as content that was wrong and loud. The harness now floors it 60 dB below the
ground truth's own loudest bin ([`AUDIBLE_FLOOR_BELOW_PEAK_DB`]); the libopenmpt comparison
keeps −120 dB, which is right for it, because *there* the question is whether two engines
agree rather than whether something sounds closer.

The floor is anchored to the signal rather than stated as an absolute magnitude, and that
detail is not cosmetic. These magnitudes are normalised so a full-scale **sine** puts 0.25
in one bin; a real instrument spreads the same energy over thousands of bins, so its
per-bin magnitudes sit 30 to 40 dB below its own level before anything is lost. An absolute
`0.25 × 10⁻³` floor was implemented first and measured: it left **0.3 % (d), 1.3 % (a),
2.9 % (c) and 7.2 % (b)** of the reference's bins above it, collapsed every chain's score
into a range of 0.05 dB, and did not stop inaudible differences dominating the score — it
stopped the score being a measurement. Sixty decibels below the loudest bin is the same
intent, delivered.

**The tail window was mostly digital silence.** "The last 40 %" of a decay that reaches
−60 dB is, at 8 bits, three quarters silence: every value below half a quantisation step
rounds to zero, so those frames score exactly 0 dB whatever any enhancer does. The window
is now the frames where the ground truth's level lies between
[`TAIL_WINDOW_UPPER_DB`] = −24 dB and [`TAIL_WINDOW_LOWER_DB`] = −48 dB of its peak.

Unlike the other two, **this one did not change the verdict**, and that is itself the
finding. Measured across five window placements on instrument (a):

| window (dB below peak) | frames | `sinc4x` | `denoise+sinc4x` | gain |
|---|---:|---:|---:|---:|
| −24 … −48 (the one now used) | 25 600 | 8.27 | 8.83 | **+0.56** |
| −30 … −54 | 26 624 | 4.26 | 4.78 | +0.51 |
| −36 … −60 | 26 624 | 1.60 | 2.08 | +0.48 |
| −40 … −64 | 25 600 | 0.40 | 0.71 | +0.30 |
| −24 … −72 | 45 670 | 5.13 | 5.47 | +0.34 |

The denoiser gains between 0.30 and 0.56 dB wherever the window is put. Section 3 explains
why that is the mechanism's ceiling rather than the window's fault. (The 8-bit floor lands
45.2 dB below this instrument's rendered peak, so the chosen window's lower edge is right
at the crossing — it is the best of the five, which is why it is kept.)

### 3. Deliverable 2's acceptance criteria

| Criterion | Target | Measured | Outcome |
|---|---|---|---|
| (a) decay-window in-band SNR improves | ≥ 3 dB | **+0.56 dB** (`sinc4x` 8.27 → `denoise+sinc4x` 8.83) | **NOT MET** |
| (b) full-band SNR drops | < 0.5 dB | **0.00 dB** (12.85 → 12.85) | met |
| (c) attack first-10 ms RMS | within 0.5 dB | **0.00 dB** (−16.30 → −16.30) | met |

**Why 3 dB is out of reach for a per-block scalar gain, in one line of arithmetic.** The
optimal scalar Wiener gain `g = s²/(s²+n²)` turns an error of `n²` into `s²n²/(s²+n²)`, so
it improves the signal-to-error ratio by exactly `10·log10(1 + n²/s²)` — a quantity that
depends only on how far the signal already is above the floor. Reaching **+3 dB requires
`s ≤ n`**: the window has to sit at or below the point where the decay crosses its own
noise floor. The measured baseline in the specified window is 8.27 dB, so `s²/n² = 6.71`
and the ceiling there is `10·log10(1.149)` = **+0.60 dB**. The implementation achieves
**+0.56 dB**, which is 93 % of it.

Moving the window down does not help, because below the crossing the 8-bit sample is
already digital silence and there is no noise left to remove — which is what the five-window
sweep in section 2 shows: +0.30 dB at −40…−64 dB, *worse* than +0.56 at −24…−48.

Also swept against the corrected harness, and neither is what limits it:

* **Strength**, as the decay-window in-band SNR: 8.27 (off) / 8.68 (50 %) / **8.83
  (100 %)** / 8.79 / 8.66 / 8.23 / 7.74 (400 %). The plain Wiener gain is the optimum, as
  the theory above says it must be — it is the MSE-minimising scalar gain per block — and
  every other setting is worse in both directions.
* **Release fraction** 0.125 / **0.25** / 0.5 / 1.0 (instant): 8.81 / 8.83 / 8.81 / 8.78.
  Within 0.05 dB of each other.

The route to ≥ 3 dB is a **spectral** rather than scalar Wiener gain — in a decay the
signal is concentrated in a few harmonic bins while the quantisation noise is flat across
every bin, so a per-bin gain can keep the harmonics and drop the rest, which no broadband
gain can. The STFT this task already builds for `sbr` is the machinery it would need. It is
a different enhancer, and the task specified this one.

### 4. Deliverable 3's acceptance criteria

| Criterion | Target | Measured | Outcome |
|---|---|---|---|
| (a) LSD lower with `sinc4x+sbr` than `sinc4x` | lower | **−0.02 dB** (2.17 → 2.15) | **met** |
| (b) LSD lower with `sinc4x+sbr` than `sinc4x` | lower | **+0.03 dB higher** (6.46 → 6.49) | **NOT MET** |
| (c) LSD does not rise | ≤ +1 dB | **−1.89 dB better** (21.30 → 19.41) | met |
| (d) detects the 3 kHz edge | yes | **2 875 Hz**, against a 3 887 Hz band limit | met |
| (d) LSD not worse than `sinc4x` | not worse | **+0.04 dB worse** (0.72 → 0.76) | **NOT MET** |

Criterion (a) is the one the harness correction turned around: against a truth that stopped
at 5.6 kHz the extender could only be wrong, and against one that carries harmonics to
20 kHz it is right. Two changes to the extender itself were needed to get there, and both
are improvements in their own right rather than tuning:

**Phase doubling.** Copying bin `b`'s complex value to bin `2b` puts the right magnitude in
the right place within one frame — but the frames overlap four to one, and a component at
bin `2b` must advance its phase by `2π·2b·hop/N` between frames while bin `b`'s value
advances by half that. Fed the wrong advance, successive frames fight each other and the
patched band partly cancels. Squaring a complex number doubles its phase and squares its
magnitude, so squaring and dividing by the magnitude once, per octave, gives the right
advance at the right level. On instrument (a) this moved the extender from +0.009 dB
(harmful) to −0.003 dB before the other change below.

**The patch is additive.** The extender used to resynthesise the whole signal, which cost
one fresh rounding to `i16` of every frame of a sample whose tail is a step or two tall:
0.29 dB of in-band SNR on instrument (a), spent on a band the enhancer had no business
touching. It now synthesises the added band alone — everything at or below the edge is
zeroed before the inverse transform — and sums it onto the original frames. The source's
own band comes back arithmetic for arithmetic, and the loop stays periodic for the same
reason it already did.

**Why (b) and (d) still go the wrong way, and what was tried.**

(b) is **inharmonic by construction**: its partials are stretched by `1 + 0.0008n²`, so
`2·f(n) ≠ f(2n)` and an octave transposition lands its copies *between* the true partials
of a sustained, sharply tonal spectrum — where the log-spectral distance charges most. (a),
whose harmonics are exact, improves under the identical mechanism. That is a real property
of transposition-based band replication and not a defect in this implementation: SBR is
right for a harmonic source and wrong for an inharmonic one.

(d) is dark, and the extender's own report of it is exactly right — a content edge of
2 875 Hz inside a band limit of 3 887 Hz, which is the 3 kHz the criterion names. It then
extends anyway, because a source that stops with a moderate roll-off is genuinely
indistinguishable from one whose sample rate ran out. Two guards were measured and neither
separates it:

| instrument | band limit | content edge | fill | gain at `tilt_db` = −6 | Δ LSD |
|---|---:|---:|---:|---:|---:|
| (a) plucked decay | 3 855 Hz | 3 528 Hz | 92 % | 0.41 | **−0.02** |
| (b) sustained loop | 4 966 Hz | 3 789 Hz | 76 % | 0.20 | +0.03 |
| (c) noise drum | 4 868 Hz | 3 887 Hz | 80 % | 0.13 | **−1.89** |
| (d) dark pluck | 3 887 Hz | 2 875 Hz | 74 % | 0.14 | +0.04 |

A **gain** guard cannot work: (d) at 0.14 sits *above* (c) at 0.13, and (c) is the
instrument that gains most. A **fill-ratio** guard could — anything between 76 % and 80 %
refuses (b) and (d) and keeps (a) and (c) — but a threshold placed in a four-point gap on
four synthetic instruments is fitted, not derived, and would refuse legitimately dull
samples that a listener might still want extended. It is deliberately not added; the
numbers are here so the owner can weigh it at the listening check.

The cost on (d) is worth its size: +0.04 dB on the instrument with the *lowest* absolute
distance of the four (0.72 against 2.17, 6.46 and 21.30), against −1.89 dB on the drum.

**The tilt sweep**, as the change in log-spectral distance against `sinc4x` alone (a `+0.00`
means the gain fell below `MINIMUM_EXTENSION_GAIN` and the extender declined the sample):

| `tilt_db` | 0 | −3 | −6 | −9 |
|---|---:|---:|---:|---:|
| (a) | −0.043 | −0.034 | −0.025 | −0.017 |
| (b) | +0.082 | +0.048 | +0.026 | +0.013 |
| (c) | −2.07 | −1.82 | −1.60 | −1.39 |
| (d) | +0.086 | +0.055 | +0.033 | +0.018 |

Every tilt has the same signs, so no setting satisfies (b) or (d); the specified default of
−6 is kept as the point where the two instruments that should improve do, by a useful
margin, and the two that should not are hurt least.

**The octave sweep**, at the default tilt, against the same baseline:

| octaves | (a) | (b) | (c) | (d) |
|---|---:|---:|---:|---:|
| 1 | −0.025 | +0.026 | −1.60 | **+0.033** |
| 2 | −0.024 | +0.026 | **−1.90** | +0.035 |
| 3 | −0.024 | +0.026 | −1.90 | +0.035 |

`MAXIMUM_PATCHED_OCTAVES` is **2**: the second octave buys 0.30 dB on the drum — the
instrument whose truth genuinely has broadband content up there, and the case this enhancer
exists for — and costs at most 0.003 dB on the other three, while a third buys nothing,
because two octaves above a 3.8 kHz edge is already 15 kHz. The first version of this
constant was **1**, chosen against the harness whose instruments stopped at 5.6 kHz, where
every octave of extension was scored against silence and fewer always won.

### 5. The constants chosen

**`DecayDenoiser`** (`crates/starplayer-enhance/src/denoise.rs`):

| Constant | Value | Why |
|---|---|---|
| `DENOISE_BLOCK_FRAMES` | 64 | 7.6 ms at 8 363 Hz: a drum's 80 ms decay is a dozen blocks, and a block's mean square is an estimate rather than a sample of the waveform. |
| `EIGHT_BIT_FLOOR_MEAN_SQUARE` | `256² / 12` = 5 461.33 | The exact variance of uniform quantisation with a step of 256. Taken whenever every frame is a multiple of 256, which is true of every sample in `REFLEX.S3M` and `PETRI.S3M`. |
| `MINIMUM_FLOOR_MEAN_SQUARE` | `1 / 12` | One `i16` step. A sample cannot be quieter than its own grid. |
| `MAXIMUM_ESTIMATED_FLOOR_FRACTION` | `10⁻⁴` (−40 dB) | Added after research point 1 measured the estimator destroying a 16-bit sustained tone. See below. |
| `QUIET_BLOCK_PERCENT` | 5 | As specified. |
| `RELEASE_FRACTION` | 0.25 per block | A 26 ms time constant at 8 363 Hz: slower than a tracker tick, so it cannot pump, and 20 time constants inside the decay it has to follow. The attack is **instant** (the gain takes any higher block gain immediately), which is what leaves a drum's first block untouched. |
| default `strength_percent` | 100 | The plain Wiener gain, and measurably the optimum — see the strength sweep below. |

Everything runs in the **mean-square** domain rather than in RMS, which is the same
quantity squared and removes every square root from the enhancer: the Wiener gain is a
ratio of mean squares and the strength is the only place a root appears at all. That is a
deviation in letter from the task's "RMS of the quietest 5 %"; the pooled mean square of
equal-length blocks is exactly the square of their pooled RMS, so it is the same number.

**`BandwidthExtender`** (`crates/starplayer-enhance/src/sbr.rs`):

| Constant | Value | Why |
|---|---|---|
| `STFT_SIZE` / `STFT_HOP` | 1 024 / 256 | 30.6 ms and 32.7 Hz at the 33 452 Hz an upsampled tracker sample arrives at: fine enough to resolve a bass note's partials, short enough that a drum's decay is three frames. A quarter hop is far past the overlap-add criterion, so the analysis **and** synthesis windows can both be applied and a patched frame cannot click against its neighbours. |
| `BAND_EDGE_THRESHOLD_DB` | 12 | As specified. |
| `SPECTRAL_FLOOR_TOP_PERCENT` | 10 | As specified — but the **median** of those bins, not their mean. See below. |
| `MAXIMUM_EDGE_FRACTION_PERCENT` | 45 | As specified. |
| `MINIMUM_EXTENSION_GAIN` | 0.08 | With the default −6 dB of extra roll-off already in the gain, this trips when the source is falling faster than about 16 dB per octave at its own top. It is a safety rail, not the thing that decides the dark case — see section 4. |
| `MAXIMUM_PATCHED_OCTAVES` | **2** | Measured, not assumed, and re-measured after the harness was corrected. See the octave sweep in section 4. |
| default `tilt_db` | −6 | As specified. The sweep in section 4 is the evidence for keeping it. |

### 6. Research point 1 — a noise-shaped floor for 16-bit sources

**No, and the measurement found something worse than "no useful gain": the specified floor
estimator is actively destructive on 16-bit material, and that is now fixed.**

Running `denoise+sinc4x` on the *ground-truth* (16-bit, 44 100 Hz) instruments rather than
on their degraded twins:

| instrument | estimated floor (mean square) | in-band SNR, `sinc4x` | in-band SNR, `denoise+sinc4x` |
|---|---:|---:|---:|
| (a) plucked decay | 64.9 | 68.29 | 53.28 |
| (b) sustained loop, **before** the cap | 34 557 631 | 120.00 | **5.22** |
| (b) sustained loop, **after** the cap | 4 123.7 | 86.98 | 79.20 |

A sustained tone has no quiet passage: its quietest 5 % of blocks are as loud as its
loudest, so "the RMS of the quietest blocks" *is* the signal, the Wiener gain reads the
whole sample as noise, and the enhancer flattens it. `MAXIMUM_ESTIMATED_FLOOR_FRACTION`
refuses an estimate that comes back within 40 dB of the whole sample's own mean square —
a floor worth removing is at least that far down, and an 8-bit source's is 47 dB down — and
the same tone then comes back at 79.2 dB, an inaudible 0.002 dB of level change. Eight-bit
sources take the arithmetic floor rather than the estimate and are bit-for-bit unaffected
by the cap; the harness table above is identical with and without it.

With the cap in place, `denoise` on a 16-bit source is a small, inaudible loss and no gain,
which is the honest answer: there is no quantisation floor there to remove. A noise-shaped
floor would not change that — shaping describes where a floor sits in frequency, and a
16-bit source's floor is 96 dB down wherever it sits. **Noted, not built.**

### 7. Research point 2 — chip loops

**The extender already declines every single-cycle loop, and should.** A 4x upsample of a
`k`-frame loop gives `4k` frames, and `MINIMUM_EXTENDABLE_FRAMES` is one hop, 256 frames.
Measured on square-wave single-cycle loops:

| source loop | frames after `sinc4x` | `sbr` changed it |
|---|---:|---|
| 2 | 8 | no |
| 16 | 64 | no |
| 32 | 128 | no |
| 64 | 256 | no |
| 128 | 512 | **yes** |

So the whole 2–64 frame range the research point asks about is declined, half of it by the
length rule and the 64-frame case by the band-edge refusals — a square wave that short has
its fundamental so high that its content already fills the band. No extra rule is needed.

The 128-frame case is worth noting as the opposite finding: a 128-frame single-cycle square
wave at 8 363 Hz *is* extended, and correctly so. A real square wave's harmonics continue
past the sample's Nyquist, the upsample band-limits them at 3.7 kHz, and transposing the
octave below that up by one octave puts back harmonics that genuinely belong to the
waveform. Because the analysis is folded circularly through the loop, the result is still
exactly periodic — `a_patched_loop_stays_periodic_at_its_seam` is that property as a test.

### 8. Research point 3 — worklet cost

`Module::enhanced` on `PETRI.S3M` (31 998 source frames across five samples), native
release build, on the development machine:

| chain | time | result |
|---|---:|---|
| `sinc4x` | 7.6 ms | 127 992 frames |
| `denoise` | 0.13 ms | 31 998 frames |
| `denoise+sinc4x` | 7.6 ms | 127 992 frames |
| `sinc4x+sbr` | 29.4 ms | 127 992 frames |
| `denoise+sinc4x+sbr` | 29.6 ms | 127 992 frames |
| `denoise+sinc4x+sbr+loop=64` | 30.1 ms | 127 992 frames |

K5a measured `sinc4x` at 6.9 ms on this fixture and it measures 7.5 ms here, so the machine
and the method agree. **The full chain costs 30 ms**, an order of magnitude under the
~200 ms the task named as the point at which to say something and two orders under W4's
~1 s threshold for moving the rebuild off the worklet. The extender is the expensive stage:
two 1 024-point transforms per 256 output frames, plus one Newton square root per patched
bin per frame for the phase doubling, which is what took it from 24 ms to 30 ms and is
worth every microsecond — see section 4. The cost is linear in the *rebuilt* sample length,
so a 4 MB IT would be around 4 s of rebuild rather than 30 ms and would want the frame
budget's fallback long before it wanted a different thread. Nothing to change.

The denoiser is essentially free: one pass of mean squares and one multiply per frame.

### 9. Deviations from the task file

**In the harness**, all three from section 2 and their consequences:

* **Instruments (a) and (b) carry harmonics to 20 kHz**, not the eight and twelve partials
  the task named. Section 2.
* **The log-spectral distance is floored 60 dB below the reference's loudest bin**, not at
  the −120 dB of the metric it was lifted from, and the floor is anchored to the signal
  rather than being an absolute magnitude. Section 2.
* **The tail metric is a level window, not "the last 40 %"**. Section 2.
* **The decimation filter is 512 taps at a 0.92 cutoff**, not 64 at 0.95 — forced by the
  richer instruments, or the harness would have measured aliasing. Section 2.
* **The synthetic module's mixing volume is IT's maximum**, so the render sits about 3 dB
  under full scale rather than 11 dB under it. Every decibel the render loses is a decibel
  of the instrument's decay that falls under the audibility floor and stops being measured.

**In the extender**:

* **`MAXIMUM_PATCHED_OCTAVES` is 2, not "up to Nyquist".** Section 4.
* **The spectral floor is the median of the top 10 % of bins, not their mean**, and the
  band edge is found in **two passes** rather than one. Without the second pass the
  extender cannot detect instrument (d)'s 3 kHz edge at all, which is a criterion the task
  states explicitly — and it reports 2 875 Hz against a 3 887 Hz band limit, so it does.
* **Only bins carrying content are transposed**, by the same 12 dB rule that finds the
  edge, rather than the whole complex spectrum. Transposing the source's own quantisation
  floor puts hiss into an octave that previously had none.
* **The phase is multiplied by `2^k`, not copied.** Section 4 — this is what makes the
  overlap-add reconstruct the patched band at the level the tilt asked for instead of
  partly cancelling it.
* **The patch is additive**: the extender synthesises the added band alone and sums it onto
  the original frames, rather than resynthesising the whole signal. Section 4.
* **"Never above the level at the edge" is satisfied by construction rather than by a
  clamp.** The first patched bin takes the bin an octave below it and scales it by exactly
  the measured ratio between those two levels, times an extra roll-off that is never above
  one. An explicit clamp was implemented first and removed: measured against the *detected*
  edge — which sits a little above the last real partial — it read the noise skirt as "the
  level at the edge" and refused every extension.

**In the denoiser**:

* **`MAXIMUM_ESTIMATED_FLOOR_FRACTION` is new**, and is the fix for research point 1's
  finding. Section 6.
* **It works in mean squares rather than RMS**, which is the same quantity squared and
  removes every square root from it. Section 5.

**Elsewhere**:

* **`InfiniteSource` grew a second index map rather than changing its existing one.**
  `body_index` is what the upsampler reads through, unchanged: before frame 0 it is
  silence, because a resampled frame really is played from frame 0 forwards with the
  module's own pre-roll in front of it. `periodic_body_index` continues a loop that begins
  at frame 0 *backwards* as well, which the extender needs because it folds its output past
  `loop_end` back into the loop and the two only agree if the reading is circular in both
  directions. Changing `at()` instead would have moved `sinc4x`'s output on every sample
  whose loop starts at zero, and `tests/module_rebuild.rs`'s pinned `REFLEX.S3M` hash is
  unchanged because it did not.
* **`deterministic.rs` is a new module the task did not name.** The strength knob is
  specified as a percent and the tilt as decibels, and raising a gain to a fractional power
  needs `powf` — which does not exist in `core` and is not bit-reproducible across libm
  implementations where it does exist. The crate therefore carries a Newton square root and
  a binary-expansion power built from `+ − × ÷` alone, pinned against `f64::powf` in tests.
  The square root earns its keep twice over: the phase doubling needs one per patched bin.
  `saturating_i16` moved there from `upsample.rs` unchanged so every enhancer rounds the
  same way.
* **The harness's instruments, degradation and metrics live in
  `starplayer_offline::enhance_measure` rather than in the example.** `starplayer-enhance`
  dev-depends on `starplayer-offline`, so the offline crate cannot depend on it; the module
  takes a `&dyn SampleEnhancer`, which it already has through `starplayer::model`, and both
  `examples/enhance_report.rs` and `tests/enhance_quality.rs` build the chains themselves
  from their shared dev-dependency.
* **`BandwidthExtender::describe` is public** so the harness and the pinned tests can check
  what the extender decided — the band limit, the content edge and the gain — rather than
  inferring it from output. It is diagnosis only; the enhancer works from the private
  `Plan`.
* **`starplayer-testkit` did already depend on `starplayer-offline`**, so `Fft`, the Hann
  window and `log_spectral_distance_db` moved rather than being copied, as the task's first
  preference asked. `log_spectral_distance_db` gained a `magnitude_floor` parameter so the
  two callers can disagree about it, and the perceptual binary passes the constant it
  always used.

### 10. Owner acceptance

Everything above is agent verification. What is left is the listening check:
`sinc4x+loop` against `denoise+sinc4x+sbr+loop` on the owner's own S3Ms and on the
third-party MODs in `music/old/`, with three questions the measurements cannot answer.

* Does the decay denoiser remove hiss without dulling attacks? It is measurably safe — it
  costs nothing on a sustained tone and nothing on a drum's first ten milliseconds — but
  +0.56 dB is a small effect and the owner may find it inaudible either way.
* Does the bandwidth extender's top octave sound like the instrument, or like a phaser? It
  measures clearly positive on broadband percussion (−1.89 dB) and mildly positive on a
  harmonic pluck (−0.02 dB), and mildly negative on a sustained inharmonic tone and on a
  deliberately dark one. Real modules contain all four.
* Should `sbr` be on by default? Nothing in this task turns it on, and the fill-ratio guard
  in section 4 — which would refuse the two instruments it hurts — is deliberately left
  unbuilt because a threshold fitted to a four-point gap is not a rule. The owner's ear on
  real material is the evidence that would justify one.
