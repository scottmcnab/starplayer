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

## Research resolution

Recorded at implementation time on 2026-09-03, on the development machine (32-thread
x86-64, Ubuntu 24.04, g++ 13.3.0, GNU make 4.3). Each answer is the decision and the
measurement, not a summary of the question. A fifth entry records the one deviation from
the deliverables as written.

### 1. Which libopenmpt release and which `make` switches — **0.7.21, the `makefile` tarball, 32 s**

`libopenmpt-0.7.21+release.makefile.tar.gz` is the newest 0.7.x on
`https://lib.openmpt.org/files/libopenmpt/src/` (0.8.x exists up to 0.8.9; the task asks
for 0.7.x and that is what is pinned). SHA-256
`5366db541b4ac59906a3af526e5bed54033519005814d08bccc73fdb0d58abb3`. The **makefile**
tarball is the one to take: the `autotools` tarball has no top-level `Makefile` and would
need `./configure`.

The exact command, as `LIBOPENMPT_MAKE_FLAGS` in `xtask/src/main.rs` spells it:

```sh
make -j32 CONFIG=gcc EXAMPLES=0 TEST=0 SHARED_LIB=0 STATIC_LIB=1 DYNLINK=0 OPENMPT123=1 \
     NO_ZLIB=1 NO_MPG123=1 NO_OGG=1 NO_VORBIS=1 NO_VORBISFILE=1 NO_FLAC=1 NO_SNDFILE=1 \
     NO_PORTAUDIO=1 NO_PORTAUDIOCPP=1 NO_PULSEAUDIO=1 NO_SDL2=1
```

31.6 s wall on 32 threads (10 min 2 s of CPU) for a clean tree, including the download and
the checksum. Four flags beyond the `NO_*` set the task named are worth their line:
`EXAMPLES=0` and `TEST=0` drop targets nothing here runs; `SHARED_LIB=0 STATIC_LIB=1`
with `DYNLINK=0` links `openmpt123` against `bin/libopenmpt.a`, so the binary runs out of
`target/openmpt/bin/` with no `make install` and no `LD_LIBRARY_PATH`. `ldd` on the result
shows only `libstdc++`, `libm`, `libgcc_s` and `libc` — no optional backend survived.
`CONFIG=gcc` is explicit rather than left to autodetection so the pinned build is the same
on the runner as here.

`libsndfile` is **not** installed on this machine and `apt` was never touched: the build
needs a C++17 compiler and GNU make, and nothing else.

### 2. `openmpt123` render flags — **confirmed, except that WAV output does not exist in this build**

The invocation, from `render_with_openmpt123`:

```sh
openmpt123 --quiet --banner 0 --no-progress --no-meters --no-details \
           --samplerate 44100 --channels 2 --float --gain 0 --filter 2 --dither 0 \
           --repeat 0 --batch --force -o <out>.raw -- <module>
```

`--filter n` is "interpolation filter taps to n [1,2,4,8]", so `--filter 2` is the linear
kernel and matches `GOLDEN_INTERPOLATOR`. `--gain` is in dB, `--dither 0` is off, `--float`
is 32-bit float (and is already the default), `--repeat 0` plays the song once, `--force`
overwrites without prompting. `-o` applies to `--batch` (and `--ui`) mode; `--render`
writes next to the input file instead, which is why `--batch -o` is used. `--` guards a
module whose name starts with a dash. `stdin` is `/dev/null`ed so nothing can wait on a
terminal.

**The one thing that does not work is `-o out.wav`.** It fails with
`error: file format handler 'wav' not found`. `openmpt123/openmpt123.cpp:236-256` dispatches
on the output extension: `raw` unconditionally, `wav` only under
`MPT_OS_WINDOWS && !MPT_OS_WINDOWS_WINRT` (an MMIO writer), `flac` under `MPT_WITH_FLAC`,
and everything else through `MPT_WITH_SNDFILE`. A dependency-free build on Linux therefore
has exactly one file writer: `raw`. Since the rate, channel count and sample format are
ours to set on the command line, the RIFF header carries nothing this comparison needs, so
the renders are headerless interleaved `f32` little-endian. StarPlayer's render is written
in the same shape next to it (`<fixture>.starplayer.raw` beside
`<fixture>.openmpt.raw`), so a bad score can be listened to rather than only read.

### 3. What the scores look like on the known-good fixtures — **the baseline, and it is dominated by a deliberate tempo difference**

`cargo xtask perceptual --threshold-snr 20`, 2026-09-03, openmpt123 v0.7.21 /
libopenmpt 0.7.21+r25656.pkg:

| module | frames | seg_snr_db | lsd_db | rms_ratio |
|---|---|---|---|---|
| mod/synthetic | 509943 | 0.22 | 17.86 | 2.104 |
| mtm/synthetic | 627984 | −2.89 | 16.77 | 2.851 |
| s3m/armani | 5423418 | 1.36 | 4.21 | 3.549 |
| s3m/movement | 411029 | 2.27 | 4.40 | 3.770 |
| s3m/nicetune | 1128960 | 13.24 | 6.04 | 3.154 |
| s3m/petri | 1814400 | −3.24 | 6.53 | 2.906 |
| s3m/reflex | 6096384 | 14.56 | 6.10 | 3.635 |

Every fixture is below the 20 dB the deliverable names, so the threshold summary lists all
seven. That is not a defect in any of them, and **nothing in the engine was tuned**. Three
separate causes, established by cross-correlating the two renders at 1, 2, 3, 5 and 8
seconds and reporting the best lag, the SNR at that lag and the optimal gain:

**a. A constant level difference, not a fault.** `rms_ratio` is 2.1–3.8 on every fixture:
StarPlayer renders 6–11 dB hotter than libopenmpt at unity gain. It is consistent across
formats and modules, which is what a fixed headroom difference between two mixers looks
like. It is normalised out before scoring and reported as its own column, which is exactly
why the column exists.

**b. The tempo models diverge by design, and a sample-domain SNR cannot forgive it.**
StarPlayer's canonical `QuirkSet` uses `TempoModelId::ExactFixedPoint` — the exact
`rate * 5 / (2 * bpm)` carried in Q32.32 — while libopenmpt reproduces ST3's whole-frame
tick. The predicted drift is `exact − truncated` frames per tick:

| module | BPM | exact frames/tick | ST3 truncated | drift/tick | predicted at 8 s | measured lag at 8 s |
|---|---|---|---|---|---|---|
| s3m/nicetune | 125 | 882.000 | 882 | 0 | 0 | 0 |
| s3m/armani | 125 | 882.000 | 882 | 0 | 0 | +21 |
| s3m/movement | 103 | 1070.388 | 1070 | 0.388 | −128 | −114 |
| s3m/petri | 140 | 787.500 | 787 | 0.500 | −224 | −225 |

(`armani`'s 8-second probe is the one weak measurement in the table: its best lag there
comes back with a *negative* optimal gain, which means that window correlates poorly at
any lag rather than that the render is 21 frames out. At 1–5 s its lag is −4 to −11.)

Petri lands on the prediction to within one frame, and the sign is right on every fixture
that drifts at all:
StarPlayer's ticks are the longer ones, so StarPlayer falls progressively behind. Two
hundred frames is five milliseconds — inaudible, and fatal to a sample-aligned SNR. This
is accuracy-policy §2's `tempo_model` row observed from the outside, not a regression.
`petri` (140 BPM) and `movement` (103 BPM) are exactly the two fixtures whose segmental
SNR is worst, and `nicetune` and `reflex` (125 BPM, integer tick) are the two best.

**c. The log-spectral distance is the metric that survives all of that.** It sits at
4.2–6.5 dB for the five S3Ms and does not move with the drift, because a 4096-frame Hann
frame does not care about a 200-frame offset. It is the number to trend.

Two findings the owner should hear rather than read:

- **`s3m/reflex`** is the one fixture whose difference is *not* explained by drift. Its
  best-lag SNR is only 4–7 dB at every probe point and the best lag wanders (+10, −332,
  +408, +315, −168), which is what a genuine content difference looks like rather than a
  clock difference. It is also the fixture with the highest aggregate segmental SNR, so
  the whole-song average is hiding it. Worth a listen to both renders in
  `target/perceptual/s3m-reflex.{starplayer,openmpt}.raw`.
- **`s3m/armani`** is aligned to within 11 frames over its first five seconds and reaches 19–39 dB SNR once
  that quarter-millisecond is removed, despite an aggregate of 1.36 dB. The two renders
  are very close; the metric is simply unforgiving. If a future task wants the SNR to mean
  what a listener means, the fix is a best-lag search per segment, not a change to the
  engine — but the task file says frame zero, and frame zero is what this implements.
- The **synthetic MOD and MTM** score 17–18 dB LSD, far worse than any S3M. That is
  expected and not alarming: `starplayer-offline::fixtures` generates a looped 128-frame
  ramp, which is an aliasing-rich signal chosen to exercise mixer paths, and its
  documentation already says the fixture is a regression contract rather than an accuracy
  one. Any resampling or panning-model difference between two engines is maximally visible
  in it.

### 4. Length matching — **trim to the shorter, confirmed; only `armani` diverges by more than a second**

libopenmpt's render is longer than StarPlayer's by a constant 4410 frames — exactly 100 ms
— on four of the seven fixtures, 4261 on `movement` and 3258 on `petri`: a short tail
libopenmpt renders past its own end-of-song. Trimming to the shorter therefore costs a
tenth of a second of tail and nothing else.

| module | StarPlayer | libopenmpt | difference |
|---|---|---|---|
| mod/synthetic | 509943 | 514353 | +4410 (+0.100 s) |
| mtm/synthetic | 627984 | 632394 | +4410 (+0.100 s) |
| s3m/armani | 5860008 | 5423418 | **−436590 (−9.900 s)** |
| s3m/movement | 411029 | 415290 | +4261 (+0.097 s) |
| s3m/nicetune | 1128960 | 1133370 | +4410 (+0.100 s) |
| s3m/petri | 1814400 | 1817658 | +3258 (+0.074 s) |
| s3m/reflex | 6096384 | 6100794 | +4410 (+0.100 s) |

`s3m/armani` is the only fixture past the one-second bound, and it is past it for a known
reason: armani loops, so `RenderLength::default_for` gives StarPlayer a ten-second fade
that `--repeat 0` does not give libopenmpt. Trimming to the shorter is what saves the
comparison here rather than what compromises it — the trim lands at libopenmpt's
end-of-song, which is where StarPlayer's fade begins, so the fade is excluded from the
scores instead of being compared against unfaded audio. The driver prints a
`length divergence:` line for any fixture past the bound, so the case is visible in the
nightly log rather than silent.

### 5. Deviation from the deliverables — **`raw` instead of `wav`, and `--threshold-snr 20` names every fixture**

Two things differ from the task file as written, both recorded above and neither
discretionary:

1. Deliverable 2 says "render the same bytes to a WAV" and "load the WAV". The
   dependency-free `openmpt123` the task mandates has no WAV writer on Linux (research
   point 2), so both renders are headerless interleaved `f32` instead. Nothing else about
   the comparison changes, and the samples are libopenmpt's own floats rather than a
   16-bit quantisation of them.
2. Deliverable 4's `--threshold-snr 20` currently names all seven fixtures. That is
   implemented as specified — the workflow passes 20 — but the advisory line is not useful
   at that value until either the SNR gains a per-segment lag search or the threshold moves
   to something the measured baseline can distinguish (−5 dB would isolate `petri` today).
   The comment at the top of `.github/workflows/perceptual.yml` says so and points here.
   **Owner decision**, since T10 is explicitly a tolerance a human reads.

One more note on the segmental SNR itself: `segmental_snr_db` is reused unchanged, and it
takes its reference as `i16`. libopenmpt's normalised render is quantised into that domain
to feed it. Both signals are scaled by one shared factor first so the louder of the two
peaks at −0.01 dBFS, which keeps the quantisation floor near 90 dB SNR — four decades
above anything this comparison measures — and cannot clip.
