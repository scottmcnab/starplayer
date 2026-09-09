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
