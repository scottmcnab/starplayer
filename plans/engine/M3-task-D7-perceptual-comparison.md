# M3 — D7: Perceptual comparison against libopenmpt (T10)

| Field | Value |
|---|---|
| Milestone | M3 ([master plan](M3-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | M2 complete |
| Blocks | Nothing hard; F4/G5 use it as the second XM/IT oracle |
| Parallel with | D3, D6, F1, G1 |
| Recommended model | Claude Opus (a from-source build of a C++ project, an FFT, and a CI workflow) |
| Verified by | agent (`cargo xtask perceptual` locally over the committed fixtures, the workflow file validated), then reviewer; the nightly job is the ongoing check |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine with `std` hosts and tooling.
Read `AGENTS.md` first — the working agreements apply to tooling too, and `xtask` has a
standing rule of **no dependencies** (it shells out to helper binaries instead).

The accuracy machinery so far is exact: per-tick traces diffed against libxmp's dumps,
SHA-256 goldens of the fixed-point render. Roadmap test **T10** adds the one thing hashes
cannot express — "sounds wrong" — by rendering every fixture through StarPlayer's float
path and through **libopenmpt**, the most trusted tracker replayer there is, and scoring
the two renders against each other with segmental SNR and a spectral distance. It is a
**nightly job with a tolerance, not a gate**: numbers go into a report and a trend, and
a regression is something a human reads, not something that blocks a merge. It matters
now because XM (M5) and IT (M6) are being built concurrently and libopenmpt is *their*
reference implementation; a perceptual score against it is the fastest "is this close?"
signal those streams can get before their conformance harness cases are wired.

### How libopenmpt is obtained — no root, no FFI

`libopenmpt` is not installed on the dev machine and there is no passwordless `sudo`;
FFI bindings would need `unsafe` in a crate that forbids it. So use **`openmpt123`**, the
command-line renderer shipped in the libopenmpt source tree, and build it **from a
pinned, checksum-verified source tarball into `target/openmpt/`**, exactly the way
`cargo xtask conformance --fetch-only` pins and caches the libxmp corpus. libopenmpt's
plain `Makefile` builds `bin/openmpt123` with `make` and a C++17 compiler; every optional
backend is switched off with the `NO_*=1` variables (`NO_ZLIB`, `NO_MPG123`, `NO_OGG`,
`NO_VORBIS`, `NO_VORBISFILE`, `NO_FLAC`, `NO_SNDFILE`, `NO_PORTAUDIO`, `NO_PORTAUDIOCPP`,
`NO_PULSEAUDIO`, `NO_SDL2`), which leaves a renderer that writes WAV files and needs
nothing else. On the CI runner the same xtask command builds it the same way (the runner
has a compiler); `apt` is an optional shortcut, not the mechanism.

Render settings that make the two renders comparable: 44 100 Hz, stereo, float or 16-bit
WAV, **linear interpolation** (`--filter 2`), `--repeat 0`, gain 0 dB, no dithering, and
the same length as StarPlayer's `render_song` with `RenderLength::default_for(44_100)`.
Master level still differs between the two engines, so **normalise both renders to equal
RMS** before scoring, and align them at frame zero.

### Code you must read before changing anything

- `xtask/src/main.rs` — `conformance` (the pinned download, checksum, cache marker,
  `--offline` / `--fetch-only`), `goldens`, `trace` (how xtask calls helper bins), `JOBS`
  and `OPT_IN_JOBS`, `job_conformance`.
- `crates/starplayer-offline/src/lib.rs` — `render_song`, `RenderLength`,
  `segmental_snr_db` (already exists: the float-versus-fixed check uses it), `GoldenFormat`,
  `scanned_song`; `src/bin/starplayer-goldens.rs` (the fixture list and the bin shape).
- `crates/starplayer-testkit/src/lib.rs` and `Cargo.toml` — where analysis code lives;
  `crates/starplayer-offline/src/fixtures.rs` — the synthetic MOD/MTM.
- `.github/workflows/{ci,fuzz}.yml` — the job shape, caching, artifact upload.
- `conformance/README.md` — the licensing note: corpus modules are fetched into an
  ignored cache, never committed. The same applies to WAV renders and to the libopenmpt
  source.
- `plans/product/02-roadmap.md` T10; `plans/product/03-accuracy-policy.md` §5 item 7.

## Deliverables

### 1. `cargo xtask openmpt [--fetch-only|--offline]`

Pins a libopenmpt release tarball (`LIBOPENMPT_VERSION`, `LIBOPENMPT_URL`,
`LIBOPENMPT_SHA256` constants; choose the newest 0.7.x release and verify the checksum
with `sha256sum` like the corpus does), unpacks it under `target/openmpt/src/`, runs
`make -j` with the `NO_*` switches into `target/openmpt/`, and records a marker file with
the version so the build is skipped when current. `--offline` refuses to download.
Reports the `openmpt123 --version` line at the end. No xtask dependencies.

### 2. `starplayer-perceptual` (a bin in `starplayer-testkit`, `std`)

For each fixture: render StarPlayer float stereo 44.1 kHz linear via `render_song`
(the `GoldenFormat` fixture set: the five owner S3Ms and the synthetic MOD and MTM;
later the XM/IT corpus once F4/G5 land — accept an optional `--corpus` list of extra
module paths so those streams can point it at `openmpt/xm/*.xm` today), invoke
`openmpt123` to render the same bytes to a WAV in `target/perceptual/`, load the WAV,
RMS-normalise both, trim to the shorter, and compute:

- **segmental SNR** in dB over 4096-frame segments (reuse `segmental_snr_db`);
- **log-spectral distance** in dB: a small radix-2 real FFT (write it; no dependency),
  Hann window, 4096-frame frames with 50 % overlap, mean over frames of the RMS
  difference of the log-magnitude spectra;
- the **RMS ratio** before normalisation (a level mismatch is itself a finding).

Print one table row per fixture (`module, frames, seg_snr_db, lsd_db, rms_ratio`) and
write it as `target/perceptual/report.tsv`. Exit non-zero only on a *tooling* failure
(missing `openmpt123`, unreadable WAV), never on a score.

### 3. `cargo xtask perceptual [--corpus PATH...] [--threshold-snr DB]`

Runs deliverable 1 (`--offline` in CI), then the bin, then prints the table and, when
`--threshold-snr` is given, a summary line naming any fixture below it. Still exit zero
on scores; the threshold is advisory (T10: "with a tolerance", "not a gate").

### 4. `.github/workflows/perceptual.yml`

Nightly cron plus dispatch, like `fuzz.yml`: cache `target/openmpt` keyed on
`LIBOPENMPT_SHA256`, build it if missing, `cargo xtask perceptual --threshold-snr 20`,
upload `target/perceptual/report.tsv` as an artifact. Not in `ci.yml`; it is explicitly
not a gate.

### 5. Documentation

`plans/product/02-roadmap.md` T10 row and `03-accuracy-policy.md` §5 item 7 note the
mechanism (openmpt123 from a pinned source build, RMS-normalised, segmental SNR and
log-spectral distance, nightly). `conformance/README.md` gains a short "perceptual
comparison" section with the render settings and the licensing note (libopenmpt is
BSD-3-Clause; its source is cached in `target/`, never committed).

## Research points

1. **Which libopenmpt release and which `make` switches** produce `openmpt123` with no
   optional dependencies on a bare Ubuntu image and on this machine. Record the exact
   command line and the build time.
2. **`openmpt123` render flags.** Confirm the flag names for interpolation (`--filter`),
   repeat count, gain, dithering (`--dither 0`), output format, and that `--force` writes
   without prompting. Record the exact invocation.
3. **What scores look like on the known-good fixtures.** Run it on the five S3Ms; record
   the numbers as the baseline in the research resolution so a future regression has a
   reference. If a fixture scores poorly, investigate whether it is a real difference
   (report it as a finding for the owner; do not tune anything in the engine) or an
   alignment/length artefact of the comparison.
4. **Length matching.** `openmpt123 --repeat 0` stops at libopenmpt's own end-of-song,
   which is not always StarPlayer's; trimming to the shorter render is the pragmatic
   rule — confirm it and note any fixture where the two lengths differ by more than a
   second.

## Verification

```sh
cargo xtask openmpt                                  # builds target/openmpt/bin/openmpt123
cargo xtask perceptual --threshold-snr 20            # the table over the committed fixtures
cargo test -p starplayer-testkit                     # FFT unit tests: a pure tone lands in one bin; Parseval within 1e-6
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
```

Report the exact commands run and their results, including the baseline table. **Do not
commit** — the reviewer commits. Do not commit anything under `target/`.

## Out of scope

Gating on scores. FFI bindings to libopenmpt. Rendering the XM/IT corpus (their wiring
tasks do that, through `--corpus`). Any engine change.
