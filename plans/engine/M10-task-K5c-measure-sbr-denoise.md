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

*Written 2026-09-10, on branch `k5c`.*

Read this alongside the harness table below, which is the verbatim output of
`cargo run -p starplayer-offline --example enhance_report --release`. Two of the six
acceptance criteria are **not met**; both are stated with their numbers and with what was
tried, and neither was relaxed to make it pass.

### 1. The harness table

Every row renders the degraded instrument — band-limited, decimated to 8 363 Hz, rounded
to 8 bits and widened by 256 — through the named chain and scores it against a render of
the 16-bit 44 100 Hz ground truth. Both renders go through `render_song_with_options` at
44 100 Hz on the fixed path with the linear kernel, playing a one-channel, one-sample IT
whose `C5Speed` is the sample's own rate, so the ground truth plays at unity step. SNRs in
dB, higher better; LSD in dB, lower better; `tail` is the in-band SNR over the last 40 % of
the render; `first 10 ms` is the attack RMS in dBFS.

#### (a) plucked decay

| chain | full-band SNR | in-band SNR | LSD | tail in-band SNR | first 10 ms |
|---|---:|---:|---:|---:|---:|
| `none` | 7.18 | 9.17 | 9.84 | 1.04 | -15.20 |
| `sinc4x` | 7.50 | 10.22 | 8.85 | 0.67 | -14.71 |
| `denoise` | 7.23 | 9.19 | 9.54 | 1.18 | -15.20 |
| `denoise+sinc4x` | 7.68 | 10.43 | 8.61 | 1.06 | -14.71 |
| `sinc4x+sbr` | 7.28 | 9.79 | 9.01 | 0.68 | -14.73 |
| `denoise+sinc4x+sbr` | 7.45 | 9.99 | 8.76 | 1.06 | -14.73 |
| `denoise+sinc4x+sbr+loop=64` | 7.45 | 9.99 | 8.76 | 1.06 | -14.73 |

Ground truth's own first 10 ms: -14.39 dB.

#### (b) sustained loop

| chain | full-band SNR | in-band SNR | LSD | tail in-band SNR | first 10 ms |
|---|---:|---:|---:|---:|---:|
| `none` | 14.45 | 18.53 | 16.96 | 18.54 | -16.21 |
| `sinc4x` | 15.56 | 22.39 | 12.80 | 22.39 | -15.93 |
| `denoise` | 14.45 | 18.53 | 16.96 | 18.54 | -16.21 |
| `denoise+sinc4x` | 15.56 | 22.39 | 12.80 | 22.39 | -15.93 |
| `sinc4x+sbr` | 15.53 | 22.39 | 13.58 | 22.39 | -15.93 |
| `denoise+sinc4x+sbr` | 15.53 | 22.39 | 13.58 | 22.39 | -15.93 |
| `denoise+sinc4x+sbr+loop=64` | 15.44 | 22.39 | 13.58 | 22.39 | -15.93 |

Ground truth's own first 10 ms: -15.76 dB.

#### (c) noise drum

| chain | full-band SNR | in-band SNR | LSD | tail in-band SNR | first 10 ms |
|---|---:|---:|---:|---:|---:|
| `none` | 2.50 | 5.96 | 14.90 | NaN | -17.38 |
| `sinc4x` | 2.52 | 6.75 | 19.73 | NaN | -16.42 |
| `denoise` | 2.51 | 5.97 | 15.14 | NaN | -17.38 |
| `denoise+sinc4x` | 2.56 | 6.86 | 19.90 | NaN | -16.42 |
| `sinc4x+sbr` | 2.51 | 6.73 | 18.37 | NaN | -16.42 |
| `denoise+sinc4x+sbr` | 2.55 | 6.84 | 18.54 | NaN | -16.42 |
| `denoise+sinc4x+sbr+loop=64` | 2.55 | 6.84 | 18.54 | NaN | -16.42 |

Ground truth's own first 10 ms: -14.11 dB.

`NaN` in the drum's tail column is the metric saying so honestly: the burst is over by
80 ms and the last 40 % of a 250 ms render has no reference energy above the silence gate,
so there is nothing to score. It is not a failure and not a zero.

#### (d) dark pluck (3 kHz)

| chain | full-band SNR | in-band SNR | LSD | tail in-band SNR | first 10 ms |
|---|---:|---:|---:|---:|---:|
| `none` | 10.66 | 11.46 | 9.45 | 1.13 | -15.26 |
| `sinc4x` | 14.39 | 14.83 | 8.17 | 0.77 | -14.82 |
| `denoise` | 10.68 | 11.45 | 9.14 | 1.27 | -15.26 |
| `denoise+sinc4x` | 14.58 | 15.04 | 7.90 | 1.16 | -14.82 |
| `sinc4x+sbr` | 14.37 | 15.25 | 7.46 | 0.97 | -14.82 |
| `denoise+sinc4x+sbr` | 14.53 | 15.41 | 7.23 | 1.26 | -14.82 |
| `denoise+sinc4x+sbr+loop=64` | 14.53 | 15.41 | 7.23 | 1.26 | -14.82 |

Ground truth's own first 10 ms: -14.78 dB.

### 2. The constants chosen

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
| `MINIMUM_EXTENSION_GAIN` | 0.08 | With the default −6 dB of extra roll-off already in the gain, this trips when the source is falling faster than about 16 dB per octave at its own top. |
| `MAXIMUM_PATCHED_OCTAVES` | **1** | Measured, not assumed. See the octave sweep below. |
| default `tilt_db` | −6 | As specified. The sweep below is the evidence for keeping it. |

### 3. Deliverable 2's acceptance criteria

| Criterion | Target | Measured | Outcome |
|---|---|---|---|
| (a) tail-only in-band SNR improves | ≥ 3 dB | **+0.39 dB** (`sinc4x` 0.67 → `denoise+sinc4x` 1.06) | **NOT MET** |
| (b) full-band SNR drops | < 0.5 dB | **0.00 dB** (15.56 → 15.56) | met |
| (c) attack first-10 ms RMS | within 0.5 dB | **0.00 dB** (−16.42 → −16.42) | met |

**Why (a) cannot be met by a per-block scalar gain, and what was tried.**

The degraded instrument (a) is *digital silence* for its final quarter. A decay reaching
−60 dB from a peak near full scale passes below half an 8-bit step at about t = 1.2 s, and
every value after that rounds to zero — so there is no noise there to remove and no signal
there to keep. Rendered and scored in tenths, instrument (a) looks like this
(`sinc4x` against `denoise+sinc4x`, in-band SNR per tenth):

| tenth | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| reference RMS | 4910 | 2474 | 1240 | 621 | 312 | 156 | 78 | 39 | 20 | 10 |
| `sinc4x` | 19.33 | 19.18 | 18.79 | 17.34 | 14.30 | 9.39 | 2.88 | 0.01 | 0.00 | 0.00 |
| `denoise+sinc4x` | 19.33 | 19.18 | 18.78 | 17.33 | 14.31 | 9.91 | 4.36 | 0.02 | 0.00 | 0.00 |

The last 40 % is tenths 6 to 9. Three of those four are pinned at 0.00 dB *whatever any
enhancer does*: the candidate is silent, so the error is the reference and the ratio is
one. The whole of the available improvement lives in tenth 6, where the decay crosses its
own noise floor, and there the denoiser gains **+1.48 dB** — against a theoretical
per-block Wiener ideal of +2.09 dB at that block's signal-to-floor ratio, so the
implementation is within 0.6 dB of the best its own class can do.

What was tried, and what it bought on the specified tail window:

* **Strength**, swept 0/50/100/150/200/300/400 %: tail 0.67 / 0.96 / **1.06** / 1.06 /
  0.99 / 0.81 / 0.64. The plain Wiener gain is the optimum, which is what the theory says
  it should be — it is the MSE-minimising scalar gain per block — so no strength setting
  reaches 3 dB.
* **Release fraction**, swept 0.125 / 0.25 / 0.5 / 0.75 / 1.0 (instant): tail 1.04 / 1.06 /
  1.06 / — / 1.04. The release is not what is limiting it.
* **A larger or smaller block** cannot help either, for the same reason the strength sweep
  cannot: the gap to the per-block Wiener ideal is 0.6 dB and the criterion needs 2.6 dB
  more than that.

The route to ≥ 3 dB, measured out of scope here and named for whoever wants it, is a
**spectral** rather than scalar Wiener gain: in the tail the signal is concentrated in a
few harmonic bins while the quantisation noise is spread flat across every bin, so a
per-bin gain can keep the harmonics and remove the rest, which a broadband gain cannot.
The STFT this task built for `sbr` is exactly the machinery that would need. It is a
different enhancer, not a tuning of this one, and the task specified this one.

`crates/starplayer-offline/tests/enhance_quality.rs` pins the achieved +0.39 dB with a
margin rather than deleting the criterion, and says in its own documentation that it should
be raised if the spectral denoiser ever lands.

### 4. Deliverable 3's acceptance criteria

| Criterion | Target | Measured | Outcome |
|---|---|---|---|
| (a) LSD lower with `sinc4x+sbr` than `sinc4x` | lower | **+0.16 dB worse** (8.85 → 9.01) | **NOT MET** |
| (b) LSD lower with `sinc4x+sbr` than `sinc4x` | lower | **+0.78 dB worse** (12.80 → 13.58) | **NOT MET** |
| (c) LSD does not rise | ≤ +1 dB | **−1.36 dB better** (19.73 → 18.37) | met |
| (d) detects the 3 kHz edge, LSD not worse | not worse | **−0.71 dB better** (8.17 → 7.46) | met |

**Why (a) and (b) go the wrong way, and what was tried.**

The extender is filling a band the *ground truth does not have either*. Instrument (a) is
eight harmonics of 700 Hz, so its highest partial is 5 600 Hz; instrument (b) is twelve
partials of 440 Hz, so its highest is 5 888 Hz. The degraded sample's band edge after a 4x
upsample sits at about 3 763 Hz. So the honest headroom — the band where the ground truth
has content and the degraded sample does not — is one third of an octave wide, and
everything the extender puts above 5.9 kHz lands where the reference is silent. The
log-spectral distance floors an empty bin at −120 dB and charges the full difference, so
content 60 dB below anything audible costs exactly as much as content that is wrong and
loud.

The per-band breakdown for instrument (a) (RMS dB error per band, `sinc4x` against
`sinc4x+sbr`, before the octave limit was set) shows it directly:

| band | 0–3 763 Hz | 3 763–6 000 Hz | 6 000–9 000 Hz | 9 000–13 000 Hz | 13 000–22 050 Hz |
|---|---:|---:|---:|---:|---:|
| `sinc4x` | 20.83 | 11.59 | 2.65 | 2.45 | 2.01 |
| `sinc4x+sbr` | 20.31 | 12.71 | 5.54 | 2.45 | 2.73 |

The cost is entirely in 6–9 kHz, which is above every harmonic instrument (a) has. On the
drum — the only test instrument whose ground truth genuinely runs past 6 kHz — the same
band improves from 20.30 to 17.49 and the band the extender is really aimed at,
3 763–6 000 Hz, improves from 21.78 to 13.87.

What was tried:

* **`tilt_db` swept** 0 / −3 / −6 / −9 / −12 / −18 / −24, as the change in LSD against
  `sinc4x` alone (a `+0.00` means the gain fell below `MINIMUM_EXTENSION_GAIN` and the
  extender declined the sample entirely):

  | tilt | 0 | −3 | −6 | −9 | −12 | −18 | −24 |
  |---|---:|---:|---:|---:|---:|---:|---:|
  | (a) | +2.09 | +1.22 | +0.67 | +0.33 | +0.10 | **−0.13** | +0.00 |
  | (b) | +2.17 | +1.52 | +1.06 | +0.73 | +0.00 | +0.00 | +0.00 |
  | (c) | **−2.40** | −1.80 | −1.41 | +0.00 | +0.00 | +0.00 | +0.00 |
  | (d) | −0.14 | −0.42 | −0.61 | **−0.73** | +0.00 | +0.00 | +0.00 |

  No tilt satisfies (a) and (b) together. At −18 instrument (a) finally improves by
  0.13 dB, but by then the extender has declined (b), (c) and (d) outright and is a no-op
  on three quarters of the corpus, which is a worse enhancer rather than a better score.
  The specified default of −6 is kept: it is the setting at which every instrument is
  still processed and the two instruments whose ground truth has a real top octave both
  improve.

* **A content mask before transposition.** Copying the source's complex spectrum verbatim
  transposes its 8-bit quantisation floor along with its partials, putting hiss into an
  octave that previously had none. The extender now transposes only bins standing 12 dB
  above the source's own noise floor — the same rule that found the edge. This is a real
  improvement (instrument (a) at tilt 0 went from +2.09 to +1.53) and is kept, but it does
  not change the sign.

* **The number of octaves patched**, swept against a `sinc4x` baseline at the default
  tilt:

  | octaves | (a) | (b) | (c) | (d) |
  |---|---:|---:|---:|---:|
  | 1 | **+0.15** | **+0.79** | −1.36 | **−0.70** |
  | 2 | +0.47 | +1.06 | **−1.41** | −0.64 |
  | 3 | +0.50 | +1.06 | −1.41 | −0.64 |

  One octave is better on three instruments and 0.05 dB worse on the fourth, so
  `MAXIMUM_PATCHED_OCTAVES` is **1** and the task's "again up to Nyquist" is a documented
  deviation. The reason is structural rather than a property of these instruments:
  doubling maps harmonic `n` to harmonic `2n`, which is a real harmonic of the same
  instrument, while quadrupling maps it to `4n`, which for most instruments is past where
  the instrument has any harmonics at all. One octave above a 4x-upsampled tracker sample's
  3.7 kHz edge reaches 7.5 kHz, which is the octave that carries an instrument's
  brightness.

**What (d) actually showed, which is the opposite of what was expected.** The task guards
against "a false extension of a dark sample". On instrument (d) the extender extends — and
*improves* the log-spectral distance by 0.71 dB, more than it improves any other
instrument. The reason is that the 3 kHz low-pass is a 127-tap windowed sinc with a real
stopband rather than an infinite wall, so the ground truth does have content above 3 kHz,
at −45 dB and falling, while the degraded and upsampled candidate has nothing at all above
its 3.7 kHz polyphase cutoff. The extender's own tilt measurement makes its patch quiet on
a source that is already rolling off, and quiet content in that band is closer to the truth
than silence. The criterion "must not get worse" is met with room to spare, and the guard
is kept as a safety rail rather than as the thing that saved it.

**Two edges, not one — the detection deviation that made (d) checkable at all.** A
resampled tracker sample has *two* band limits: the polyphase filter's stopband a hundred
decibels down above the source's Nyquist, and the source's own 8-bit quantisation noise
spread flat across everything below it. A floor read from the top tenth of the whole
spectrum measures the stopband, so the highest bin standing 12 dB above it is the
*resampler's cutoff* and not the point where the instrument stops — which is the same
answer for a bright sample and a dark one. `band_edge` therefore runs **twice**: the first
pass finds that hard limit, which is what the headroom refusal is about, and the second
re-reads the floor from the top tenth of the band below it — the source's own noise floor —
and finds where the content really ends against it. On instrument (d) the second pass is
what lands on 3 kHz rather than on 3.7 kHz. Two passes and no more: iterating to a fixed
point would keep walking a smoothly decaying spectrum downwards with nothing to stop it.

The floor is also the **median** of those top bins rather than their mean. The two agree on
the sample this enhancer exists for, where the top tenth is nothing but stopband. They part
company on a sample that already fills its band: its top tenth carries real partials, their
mean is dominated by them, the threshold rises, the true edge is hidden, and a full-band
sample looks like one with headroom to fill. `a_sample_with_no_headroom_is_returned_unchanged`
is that case as a test.

### 5. Research point 1 — a noise-shaped floor for 16-bit sources

**No, and the measurement found something worse than "no useful gain": the specified floor
estimator is actively destructive on 16-bit material, and that is now fixed.**

Running `denoise+sinc4x` on the *ground-truth* (16-bit, 44 100 Hz) instruments rather than
on their degraded twins:

| instrument | estimated floor (mean square) | in-band SNR, `sinc4x` | in-band SNR, `denoise+sinc4x` |
|---|---:|---:|---:|
| (a) plucked decay | 128.47 | 120.00 | 70.87 |
| (b) sustained loop, **before** the cap | 34 557 631 | 120.00 | **5.22** |
| (b) sustained loop, **after** the cap | 6 308.66 | 120.00 | 79.50 |

A sustained tone has no quiet passage: its quietest 5 % of blocks are as loud as its
loudest, so "the RMS of the quietest blocks" *is* the signal, the Wiener gain reads the
whole sample as noise, and the enhancer flattens it. `MAXIMUM_ESTIMATED_FLOOR_FRACTION`
refuses an estimate that comes back within 40 dB of the whole sample's own mean square —
a floor worth removing is at least that far down, and an 8-bit source's is 47 dB down — and
the same tone then comes back at 79.5 dB, an inaudible 0.0016 dB of level change. Eight-bit
sources take the arithmetic floor rather than the estimate and are bit-for-bit unaffected
by the cap; the harness table above is identical with and without it.

With the cap in place, `denoise` on a 16-bit source is a small, inaudible loss and no gain,
which is the honest answer: there is no quantisation floor there to remove. A noise-shaped
floor would not change that — shaping describes where a floor sits in frequency, and a
16-bit source's floor is 96 dB down wherever it sits. **Noted, not built.**

### 6. Research point 2 — chip loops

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

### 7. Research point 3 — worklet cost

`Module::enhanced` on `PETRI.S3M` (31 998 source frames across five samples), native
release build, on the development machine:

| chain | time | result |
|---|---:|---|
| `sinc4x` | 7.5 ms | 127 992 frames |
| `denoise` | 0.12 ms | 31 998 frames |
| `denoise+sinc4x` | 7.6 ms | 127 992 frames |
| `sinc4x+sbr` | 23.6 ms | 127 992 frames |
| `denoise+sinc4x+sbr` | 24.1 ms | 127 992 frames |
| `denoise+sinc4x+sbr+loop=64` | 24.2 ms | 127 992 frames |

K5a measured `sinc4x` at 6.9 ms on this fixture and it measures 7.5 ms here, so the machine
and the method agree. **The full chain costs 24 ms**, an order of magnitude under the
~200 ms the task named as the point at which to say something and two orders under W4's
~1 s threshold for moving the rebuild off the worklet. The extender is the expensive stage
and its cost is two 1 024-point transforms per 256 output frames — linear in the *rebuilt*
sample length, so a 4 MB IT would be around 3 s of rebuild rather than 24 ms and would want
the frame budget's fallback long before it wanted a different thread. Nothing to change.

The denoiser is essentially free: one pass of mean squares and one multiply per frame.

### 8. Deviations from the task file

* **`MAXIMUM_PATCHED_OCTAVES` is 1, not "up to Nyquist".** Measured; the sweep is in
  section 4.
* **The spectral floor is the median of the top 10 % of bins, not their mean**, and the
  band edge is found in **two passes** rather than one. Both are in section 4; without the
  second pass the extender cannot detect instrument (d)'s 3 kHz edge at all, which is a
  criterion the task states explicitly.
* **The extender transposes only bins carrying content**, by the same 12 dB rule that finds
  the edge, rather than the whole complex spectrum. Section 4.
* **`MAXIMUM_ESTIMATED_FLOOR_FRACTION` is new**, and is the fix for research point 1's
  finding. Section 5.
* **The denoiser works in mean squares rather than RMS**, which is the same quantity
  squared and removes every square root from it. Section 2.
* **"Never above the level at the edge" is satisfied by construction rather than by a
  clamp.** The first patched bin takes the bin an octave below it and scales it by exactly
  the measured ratio between those two levels, times an extra roll-off that is never above
  one, so the patch cannot land above the edge. An explicit clamp was implemented first and
  removed: measured against the *detected* edge — which sits a little above the last real
  partial — it read the noise skirt as "the level at the edge" and refused every extension.
* **`InfiniteSource` grew a second index map rather than changing its existing one.**
  `body_index` is what the upsampler reads through, unchanged: before frame 0 it is silence,
  because a resampled frame really is played from frame 0 forwards with the module's own
  pre-roll in front of it. `periodic_body_index` continues a loop that begins at frame 0
  *backwards* as well, which the extender needs because it folds its output past `loop_end`
  back into the loop and the two only agree if the reading is circular in both directions.
  Changing `at()` instead would have moved `sinc4x`'s output on every sample whose loop
  starts at zero, and `tests/module_rebuild.rs`'s pinned `REFLEX.S3M` hash is unchanged
  because it did not.
* **`deterministic.rs` is a new module the task did not name.** The strength knob is
  specified as a percent and the tilt as decibels, and raising a gain to a fractional power
  needs `powf` — which does not exist in `core` and is not bit-reproducible across libm
  implementations where it does exist. The crate therefore carries a Newton square root and
  a binary-expansion power built from `+ − × ÷` alone, pinned against `f64::powf` in tests.
  `saturating_i16` moved there from `upsample.rs` unchanged so every enhancer rounds the
  same way.
* **The harness's instruments, degradation and metrics live in
  `starplayer_offline::enhance_measure` rather than in the example.** `starplayer-enhance`
  dev-depends on `starplayer-offline`, so the offline crate cannot depend on it; the module
  takes a `&dyn SampleEnhancer`, which it already has through `starplayer::model`, and both
  `examples/enhance_report.rs` and `tests/enhance_quality.rs` build the chains themselves
  from their shared dev-dependency.
* **`starplayer-testkit` did already depend on `starplayer-offline`**, so `Fft`, the Hann
  window and `log_spectral_distance_db` moved rather than being copied, as the task's first
  preference asked.

### 9. Owner acceptance

Everything above is agent verification. What is left is the listening check: `sinc4x+loop`
against `denoise+sinc4x+sbr+loop` on the owner's own S3Ms and on the third-party MODs in
`music/old/`, with the question being whether the decay denoiser removes hiss without
dulling attacks and whether the bandwidth extender's top octave sounds like the instrument
rather than like a phaser. The measurements say the denoiser is safe and the extender is
worth its cost on percussive material; only listening can say whether the extender should
be on by default, and nothing in this task turns it on.
