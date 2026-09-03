# M3 — D5: The CLI and the WAV writer

| Field | Value |
|---|---|
| Milestone | M3 ([master plan](M3-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | D3 landed (`starplayer::NativeSequencer`, `load`, `scan_song`) |
| Blocks | The `play` follow-up (needs D4 too), A1 (shares the argument surface) |
| Parallel with | D4, D6, D7, F1, G1 |
| Recommended model | Claude Sonnet (std tooling over an existing offline API) |
| Verified by | agent (`cargo test`, a render whose PCM hashes to the committed golden), then reviewer |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine; the apps are `std` and
depend only on the `starplayer` facade and the `std` helper crates
(`starplayer-offline`, `starplayer-archive`). Read `AGENTS.md` first.

`apps/starplayer-cli/src/main.rs` is a 9-line stub. The offline renderer already does
the real work: `render_song` renders a module to a `Vec` of host samples for any mixer
path, interpolator and output format, with `RenderLength` deciding how long (one pass,
then a fade into the second pass, capped at an hour; `--repeat`, `--fade`, `--at-end` are
the three knobs the M3 master plan names), and `trace_module` captures the per-tick
trace. What is missing is a WAV writer and a command-line surface. The `play` command
needs the cpal host from task D4, which is being built concurrently, so this task lands
`info`, `render` and `trace`, and leaves `play` as a stub that names D4.

The argument surface deliberately echoes the original DOS player's where it still makes
sense — `plans/reference/original-star-ui.md` §7: a mixing rate, a buffer size, a device
selection.

### Code you must read before changing anything

- `crates/starplayer-offline/src/lib.rs` — `render_song`, `render_song_fixed_mono`,
  `RenderLength` (`default_for`, `frames_for`), `GoldenFormat`, `scanned_song`,
  `song_timeline`, `trace_module`, `TraceOptions`, `canonical_sha256`, `golden_filename`,
  `FadeSample`; `src/bin/starplayer-goldens.rs` and `src/bin/starplayer-trace.rs` (the
  two existing std bins, which the CLI subsumes in spirit but must not break — `xtask`
  calls them).
- `crates/starplayer/src/{lib,sequencer}.rs` — `probe`, `load`, `scan_song`,
  `NativeSequencer`, `ScannedSong`.
- `crates/starplayer-engine/src/timeline.rs` — `SongTimeline`, `EndReason`,
  `duration_seconds`; `crates/starplayer-core/src/event.rs` — `AtEnd`.
- `crates/starplayer-mixer/src/output.rs` — `HostSample`, `I24`, `Dither`, the
  `FloatOut`/`FixedOut` aliases; `crates/starplayer-engine` — `MixerMode`, `OutputDepth`.
- `crates/starplayer-model/src/{header,pattern}.rs` — what `info` prints (`ModuleHeader`,
  `FormatDialect`, `EffectNames`, `PatternCell`).
- `crates/starplayer-archive/src/lib.rs` — `is_zip`, `list_modules`, `extract`.
- `apps/starplayer-web/www/app.js` — how the web player words the same information, for
  consistency (order/pattern/row, dialect, length, "ends" versus "loops").
- `goldens/` and `xtask/src/main.rs` (`goldens` command) — the golden contract the
  render command must be able to reproduce.
- `plans/engine/complete/M3-task-D1-song-timeline-and-loop-detection.md` and
  `M3-task-D2-natural-end-versus-loop.md` — the render-length semantics.

## Deliverables

### 1. `starplayer_offline::wav` — a WAV writer

`write_wav(path, sample_rate_hz, channels, WavSamples)` for `i16`, `I24` (packed
24-bit), `i32` and `f32` (IEEE float, format tag 3), plus a `WavHeader` builder and a
`read_wav` for tests. RIFF sizes computed correctly for files over 4 GB rejected with an
error rather than a wrapped header. Unit tests round-trip each depth.

### 2. `apps/starplayer-cli` — the commands

Use `clap` (derive) pinned in `[workspace.dependencies]`. Subcommands:

- **`info <file> [--entry N]`** — format, dialect, title, channels, sample count,
  instrument count, pattern count, order count, initial speed/BPM, flags, the resolved
  `QuirkSet`'s tempo model and timing, and from `scan_song`: length `m:ss.mmm`, and
  whether the song ends, loops (to which order/row) or ran out of scan budget. For a zip
  (`starplayer_archive::is_zip`), list the entries or pick `--entry`.
- **`render <file> -o <out.wav> [--rate HZ] [--depth 16|24|32|f32] [--mono] [--path float|fixed] [--interp nearest|linear] [--repeat N] [--fade SECONDS] [--at-end cut|fade] [--max-seconds S] [--entry N]`**
  — `render_song` with `RenderLength::default_for(rate)` modified by the knobs;
  `--at-end cut` sets `fade_frames = 0` and `AtEnd::Stop`. A song that *ends* gets no fade
  whatever was asked (D2). Prints the length rendered and the SHA-256 of the PCM payload.
- **`trace <file> [--ticks N] [--entry N]`** — prints `trace_module`'s text to stdout;
  built with the `trace` feature (the CLI enables `starplayer-offline/trace` for this
  command's build — research point 2 decides whether via a cargo feature or always on).
- **`play <file>`** — prints "playback needs the cpal host from task D4" and exits 2. A
  follow-up wires it once D4 lands.
- Every error is a one-line message and a non-zero exit; no panics on bad input (feed it
  a fuzz seed).

### 3. Reproducing the goldens

`starplayer render <owner S3M> -o x.wav --path fixed --interp linear --depth 16 --mono --rate 44100 --max-seconds 10 --at-end cut`
must produce a PCM payload whose SHA-256 equals the committed
`goldens/s3m/<stem>__i16_mono_44100_linear.sha256`. Make that a test (spawn the binary
or call the same functions) for all five S3Ms. That is the M3 exit criterion "produces a
file byte-identical to the browser's output at the same settings", proven against the
same fingerprint the browser's fixed path is held to.

### 4. Documentation

`README`-level usage in `apps/starplayer-cli/src/main.rs` docs and a `--help` that reads
well; `plans/engine/M3-master-plan.md` deliverable 3 status.

## Research points

1. **Golden render length.** The golden is a fixed 10-second render at 128-frame blocks
   (`GOLDEN_RENDER_FRAMES`, `GOLDEN_HOST_BLOCK_FRAMES`). Confirm which `render` flags
   reproduce it exactly (a `--max-seconds 10` cut with no fade, or a dedicated
   `--golden` switch) and pick the one that keeps the flag surface honest.
2. **The `trace` feature.** `starplayer-offline`'s `trace` feature is deliberately off
   by default so goldens never compile the hook in. Decide whether the CLI enables it
   always (simplest; the CLI's render path is not the golden job) or behind its own
   feature, and record why.
3. **Dither and depth.** `--depth 16` from the float path should dither (the mixer's
   `Dither`); the fixed path at 16-bit must not. Confirm the wasm host's rule and match it.
4. **Zip entries.** `is_zip` + `list_modules` + `extract`: confirm the entry selection
   semantics match the web player's picker.

## Verification

```sh
cargo test -p starplayer-offline -p starplayer-cli
cargo run -p starplayer-cli -- info crates/starplayer-s3m/tests/fixtures/NICETUNE.S3M
cargo run -p starplayer-cli -- render crates/starplayer-s3m/tests/fixtures/NICETUNE.S3M -o /tmp/nicetune.wav
cargo run -p starplayer-cli -- render crates/starplayer-s3m/tests/fixtures/NICETUNE.S3M -o /tmp/g.wav --path fixed --depth 16 --mono --rate 44100 --max-seconds 10 --at-end cut   # PCM sha256 == goldens/s3m/nicetune__i16_mono_44100_linear.sha256
cargo run -p starplayer-cli -- trace crates/starplayer-s3m/tests/fixtures/REFLEX.S3M --ticks 4
cargo test --workspace
cargo xtask ci --job clippy
cargo xtask goldens --check
```

Report the exact commands run and their results. **Do not commit** — the reviewer commits.

## Out of scope

`play` (D4 plus a follow-up). The TUI (A1). Any engine or format change.
