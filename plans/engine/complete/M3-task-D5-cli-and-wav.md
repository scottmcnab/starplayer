# M3 — D5: The CLI and the WAV writer

| Field | Value |
|---|---|
| Milestone | M3 ([master plan](M3-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Implemented by agent 2026-09-03; awaiting reviewer diff and commit |
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

## Research resolution

Recorded at implementation time; each answer is the decision, not a summary of the
question.

### 1. Golden render length — **a dedicated `--golden` switch, not the general flags**

`--path fixed --interp linear --depth 16 --mono --rate 44100 --max-seconds 10 --at-end
cut` reproduces the golden for four of the five owner S3Ms, because `render_song`'s
`RenderLength::frames_for` caps the render at `max_frames` — `min(end_frame, max_frames)`
under `AtEnd::Stop` — and for `ARMANI`, `NICETUNE`, `PETRI` and `REFLEX` one pass is
longer than the ten-second window, so the cap is what fires and nothing about the song's
own end ever comes into it.

`MOVEMENT.S3M`'s own pass is 9.32 s (411,029 frames at 44.1 kHz), *shorter* than the
window. `--at-end cut` correctly stops a general-purpose render at the song's own end —
that is the whole point of "cut" — so the flags above render exactly 411,029 frames, not
`GOLDEN_RENDER_FRAMES` (441,000). The canonical golden path (`render_fixed_mono` /
`render_golden` / `render_with_kernel`) has no concept of a song ending at all: it plays a
freshly built sequencer under its default `EndOfSongPolicy::Loop` for exactly
`GOLDEN_RENDER_FRAMES` raw samples, wrapping back to the restart order and continuing to
generate audio if the song's own pass is shorter. That is a real behavioural difference,
not a rounding one — the tail 29,971 frames of `MOVEMENT`'s golden are the start of a
second, partial pass that a "cut" render must never produce.

Reaching the golden for a song of *any* length through the general knobs alone would need
`--repeat` pushed past whatever the song's own structure calls for, purely to defeat
`--at-end cut`'s early stop — which is exactly the "flag surface honest" trap the research
point warns against: a flag combination whose correctness depends on a number the caller
cannot know without already having asked `info` for the song's length. `render`'s
deliverables 2 and 3 are two different rendering *policies*, not one with different knob
settings, so they get two entry points: `--golden` bypasses `RenderLength` entirely and
calls `render_fixed_mono` / `canonical_sha256` directly — the literal functions
`starplayer-goldens` regenerates hashes from — so a golden reproduces exactly regardless
of the module's own length. It conflicts with every other rendering flag (`clap`'s
`conflicts_with`), because those flags name a different, real render policy that this one
is not built from.

`the_cli_render_golden_reproduces_every_owner_s3m_golden` (`apps/starplayer-cli/tests/
golden_reproduction.rs`) covers all five S3Ms through `--golden`.
`the_general_flag_recipe_also_reproduces_the_golden_for_a_song_longer_than_the_window`
covers the task file's own literal verification command (`NICETUNE.S3M`, whose pass is
longer than the window) so that recipe's documented behaviour stays tested too.

### 2. The `trace` feature — **the CLI enables it always, and this surfaces a pre-existing lint debt in `starplayer-engine` outside this task's scope**

Chosen for the reason the research point itself gives: the CLI's render path is not the
golden job, `trace` changes no rendered sample (`Engine::render` never calls
`Out::convert_dithered`; the per-tick recorder is a passive observer), and the `trace`
subcommand has to work out of the box — the task's own verification command
(`cargo run -p starplayer-cli -- trace ... --ticks 4`) runs with no extra `--features`
flag.

This has a real, discovered cost. `starplayer-cli`'s dependency on
`starplayer-offline/trace` is a **normal** (non-dev, non-build) dependency feature
request, and Cargo's feature resolver unifies normal-dependency feature requests for one
package across a whole invocation — a `-p`-scoped build does not, but `cargo clippy
--workspace --all-targets` (what `cargo xtask ci --job clippy` runs, and what this task's
own Verification section names) does. That single compiled `starplayer-engine` now
carries `feature = "trace"` everywhere in that invocation, including when
`starplayer-engine`'s own `#[cfg(test)]` module builds as *its own* test target — which
is the one thing a `-p starplayer-offline -p starplayer-cli` build never triggers, since
`#[cfg(test)]` code for a dependency only compiles when that dependency is itself a
selected package.

`starplayer-engine/src/trace.rs`'s test module (only compiled with `feature = "trace"`
in the first place) has eight `clippy::indexing_slicing` violations —
`tick.channels[0]`, `tick.voices[0]`, and so on, at lines 694 and 715–721 — against the
crate's own `#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]`.
Nothing in the default workspace build has ever turned `trace` on for a `--workspace
--all-targets` clippy sweep before, so this is pre-existing tech debt this task is the
first to surface, not a regression in code this task wrote. `starplayer-engine` is not in
this task's scope fence (`apps/starplayer-cli/`, `crates/starplayer-offline/`, the
workspace `Cargo.toml`, `Cargo.lock`, and this task file), so it has been left alone.
`cargo xtask ci --job clippy` fails as a direct, sole consequence — see the Verification
results below and the report's own account of it. `cargo clippy -p starplayer-offline -p
starplayer-cli --all-targets -- -D warnings`, scoped to the crates this task actually
touches, is clean.

### 3. Dither and depth — **`render_song` cannot dither at any depth today; nothing to match yet**

`OutputFormat::convert_dithered` exists and both `HostSample::from_unit_f32` impls that
matter (`i8`, `i16`, `I24`) use it, but `Engine::render_quantum` (`crates/
starplayer-engine/src/engine.rs`) calls only `Out::convert(accumulator, destination)` —
the trait's default method, which builds a `Dither::OFF` and never advances or exposes
it. There is no setter or constructor argument anywhere between `EngineSettings`,
`Engine::with_settings` and `render_song` that reaches a `Dither::seeded` state; the
per-render-quantum dither instance simply does not exist in this call path. The wasm
host's rule (`crates/starplayer-host-wasm/src/lib.rs`) is real and does exactly what the
research point describes — the float path dithers at reduced depth, the fixed path's
16-bit `HostSample::from_i16_scale` impl is an exact, dither-independent scale — but it is
implemented entirely *outside* `Engine`: the worklet always runs `Engine` at its native
`StereoF32` / `StereoI16` output, then quantizes to the requested depth itself, sample by
sample, with its own `Dither` state (`quantize_float_sample` / `quantize_fixed_sample`).
Reproducing that in `starplayer-offline` would mean bypassing `render_song`'s
`OutputFormat` generic entirely and hand-writing the wasm host's quantization loop a
second time in a `std` crate — a real feature, and an engine-adjacent one, not "anything
`render` genuinely needs" for D5's stated deliverables. `render`'s depths are therefore
honestly undithered at every setting in this build: `--depth 16` from the float path
matches the fixed path's rule (no dither) because neither path dithers, not because the
two have been reconciled. Wiring a dither state through `Engine`/`render_song` is exactly
the kind of change this task's "any engine or format change" exclusion and scope fence
rule out; it is real, but it is not this task.

### 4. Zip entries — **confirmed; `--entry N` is the position in `list_modules`'s filtered result, matching the web player's auto-pick rule**

`apps/starplayer-web/www/app.js`: `const choice = entries.length === 1 ? entries[0] :
await chooseArchiveEntry(entries, label);` — a single recognised entry is chosen
automatically, and only an archive with more than one asks. `apps/starplayer-cli/src/
archive.rs::resolve_entry` does the same: no `--entry` and exactly one recognised module
picks it; no `--entry` and more than one is a one-line error naming the count and
pointing at `--entry N` and `starplayer info` (the CLI's stand-in for the web player's
picker dialog, since a batch tool has nothing to prompt). `--entry N` itself names the
**position** in `list_modules`'s already-filtered, zero-based result — the order
`starplayer info <archive>` lists entries in — rather than `ArchiveEntry::index` (the raw
ZIP central-directory index), which also counts directories and unrecognised entries and
so is not what a user counting "the second module in the zip" means. `info`, `render` and
`trace` all resolve `--entry` through the same `archive::resolve_entry`, so the numbering
agrees across every subcommand.

## Post-landing note (2026-09-03)

Research point 2 chose to enable `trace` unconditionally; that leaked the allocating
per-tick recorder into every workspace test build through feature unification, which the
cpal host's callback-allocation tests (D4) caught the moment both landed. The CLI now has
an optional `trace` feature (`cargo build -p starplayer-cli --features trace`); without it
the `trace` command explains how to get it. The host-tests job's trace guard is the gate.
