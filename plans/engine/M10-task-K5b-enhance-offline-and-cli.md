# M10 — K5b: `--enhance` in the CLI and the offline renderer

| Field | Value |
|---|---|
| Milestone | M10 ([master plan](M10-master-plan.md), decision 6); the host half of [K5](M10-task-K5-sample-enhancement.md) |
| Status | Planned 2026-09-09 |
| Depends on | [K5a](complete/M10-task-K5a-enhancer-core.md) landed (`starplayer::enhance`, `Module::enhanced`, `CATALOGUE`) |
| Blocks | — |
| Parallel with | [W4](../apps/W4-task-enhancement-checkboxes.md) |
| Recommended model | Claude Sonnet (plumbing an existing API through two entry points and a clap flag, with the `--insert` parser as the model) |
| Verified by | agent (goldens untouched, naming test, CLI tests), then **owner listening**: `/tmp/enhanced.wav` against the plain render |

## Context for a fresh agent

Read `CLAUDE.md`, then [K5a](complete/M10-task-K5a-enhancer-core.md) for the API this task consumes:
`starplayer::enhance::{SampleEnhancer, SincUpsampler, UpsampleFactor, LoopSmoother, Chain, CATALOGUE}`
behind the facade feature `enhance`, and `Module::enhanced(&dyn SampleEnhancer)`. An
enhanced render is a **different configuration** from the goldens and gets a different
name (`plans/product/03-accuracy-policy.md` §5 item 5); `--golden` must refuse `--enhance`.

### Code you must read before changing anything

- `crates/starplayer-offline/src/lib.rs` — `render_song` (`:397`), `render_song_with_inserts`
  (`:416`; loads at `:430` via `load_golden`, then `Arc::new`), `InsertSpec` (`:39`),
  `golden_filename`/`golden_filename_for_interpolator` (`:604-620`) and the naming test
  near `:938`, `render_golden` (`:668`; "nothing else in this crate may name an
  interpolator type"), `song_timeline` (`:288`).
- `apps/starplayer-cli/src/{main,render,play,info,insert_arg}.rs` — `--list-effects`
  (`main.rs:96`), `InterpArg` and the 64-arm `render_dispatch` (`render.rs:369-443`;
  **do not widen it**), every knob's `conflicts_with = "golden"` (`render.rs:103-151`),
  `parse_insert_args` (`insert_arg.rs:34`) and its error style (quote the offending flag
  text verbatim), `play.rs:184-187` (`player.load(bytes)`), `info.rs:96-118`.
- `crates/starplayer-host/src/player.rs:657-671` — `load` vs `load_module`.

## Deliverables

### 1. Offline

- `pub struct RenderOptions<'a> { pub inserts: &'a [InsertSpec], pub enhancer: Option<&'a dyn SampleEnhancer> }`
  (`Default` = no inserts, no enhancer) and
  `render_song_with_options<Path, Interp, Out>(format, bytes, sample_rate_hz, host_block_frames, length, options: &RenderOptions<'_>)`.
  `render_song` and `render_song_with_inserts` delegate. The enhancer runs right after
  `load_golden`, before `Arc::new`, so timing (`song_timeline`) is unaffected.
- `golden_filename_for_configuration(module_stem, interpolator, enhancer_name: Option<&str>) -> String`
  → `stem__i16_mono_44100_<kernel>.sha256` when `None`, else
  `stem__i16_mono_44100_<kernel>_enh-<name>.sha256` (e.g. `_enh-sinc4x+loop=64`). The two
  existing functions delegate with `None`. A naming test pins both shapes so a later pin
  of an enhanced golden is visibly a new file. **No enhanced golden is committed.**
- Test: `render_song_with_options` with an `Identity` enhancer equals `render_song`
  byte-for-byte; with `sinc4x` the output differs and is block-size independent at
  1, 3, 64, 128, 4096, 8191 (copy `a_render_with_an_insert_installed_is_still_block_size_independent`, `lib.rs:1431`).

### 2. CLI

- `--enhance <SPEC>` on `render`, `play` and `info`. Grammar: entries joined with `+`,
  each an id from `CATALOGUE` — `sinc2x`, `sinc4x`, `loop` or `loop=<frames>` (default 64).
  Parsed in a new `enhance_arg.rs` after the `insert_arg.rs` model; unknown ids error
  with the valid list. `--list-effects`-style `--list-enhancers` prints the catalogue.
- `render`: `conflicts_with = "golden"`; the parsed `Chain` goes through `RenderOptions`
  (`render_dispatch` gains one runtime argument, no new arms). The rate ceiling is
  `2 × --rate`. The printed configuration line names the enhancer.
- `play`: `starplayer::load` → `Module::enhanced` → `Player::load_module` (`player.rs:663`).
- `info --enhance`: after the existing block, a per-sample table — id, name, reference
  rate, scale (`×1/×2/×4`), frames and loop points **before → after**.
- Tests (`apps/starplayer-cli/tests/`): `golden_reproduction.rs` still passes;
  `render --enhance sinc4x+loop` writes a WAV whose hash differs from the plain render;
  `render --golden --enhance sinc4x` is refused; `info --enhance sinc4x` on REFLEX.S3M
  prints `×4` rows.

### 3. Documentation

`apps/starplayer-cli` README/help text; accuracy policy §5 item 5 already carries the
naming rule from K5a — add the CLI example. Append `## Research resolution`.

## Research points

1. Whether `info` should always print the sample table (without `--enhance`); the owner
   has not asked for it — keep it behind the flag and note the option.
2. `play` on an 8-bit MOD with `--enhance sinc4x+loop`: does the sinc upsample audibly
   change ProTracker-style chip samples with 2–32-frame loops? Note what you hear.

## Verification

```
cargo test -p starplayer-offline
cargo test -p starplayer-cli
cargo test --workspace
cargo xtask goldens --check
cargo xtask ci --job clippy
cargo run -p starplayer-cli -- render crates/starplayer-s3m/tests/fixtures/REFLEX.S3M --enhance sinc4x+loop -o /tmp/enhanced.wav
cargo run -p starplayer-cli -- render crates/starplayer-s3m/tests/fixtures/REFLEX.S3M -o /tmp/plain.wav
cargo run -p starplayer-cli -- info crates/starplayer-s3m/tests/fixtures/REFLEX.S3M --enhance sinc4x
```

## Out of scope

The web checkboxes (W4); committing an enhanced golden; any change to K5a's enhancers.
