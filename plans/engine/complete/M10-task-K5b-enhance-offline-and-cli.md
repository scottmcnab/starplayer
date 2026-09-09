# M10 — K5b: `--enhance` in the CLI and the offline renderer

| Field | Value |
|---|---|
| Milestone | M10 ([master plan](../M10-master-plan.md), decision 6); the host half of [K5](../M10-task-K5-sample-enhancement.md) |
| Status | Landed 2026-09-09; owner listening outstanding (`/tmp/enhanced.wav` vs `/tmp/plain.wav`) |
| Depends on | [K5a](M10-task-K5a-enhancer-core.md) landed (`starplayer::enhance`, `Module::enhanced`, `CATALOGUE`) |
| Blocks | — |
| Parallel with | [W4](../../apps/W4-task-enhancement-checkboxes.md) |
| Recommended model | Claude Sonnet (plumbing an existing API through two entry points and a clap flag, with the `--insert` parser as the model) |
| Verified by | agent (goldens untouched, naming test, CLI tests), then **owner listening**: `/tmp/enhanced.wav` against the plain render |

## Context for a fresh agent

Read `CLAUDE.md`, then [K5a](M10-task-K5a-enhancer-core.md) for the API this task consumes:
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

## Research resolution

*Written 2026-09-09, on branch `k5b`.*

### 1. Whether `info` should always print the sample table

**Kept behind `--enhance`.** The owner has not asked for the enhanced-sample table to
appear unconditionally, and printing it only when `--enhance` names a chain keeps `info`'s
plain output — the header, quirks and length block every other task already relies on —
byte-for-byte what it always was. The table needs a rebuilt module to report on (`before`
and `after` frame counts and loop points only exist once an enhancer has run), so there is
no unconditional variant of it to print anyway; the option to widen this later is noted
here rather than built.

### 2. Does `sinc4x+loop` audibly change an 8-bit MOD's short-loop chip samples?

Nobody on this task can listen, so this is a numerical answer, not an audible one — the
owner's own listening check is still outstanding (`## Context for a fresh agent`'s
"Verified by" row).

**Method.** No real 8-bit MOD ships in this repository — `starplayer-offline`'s own doc
comment on `fixtures.rs` explains why (no licence-safe MOD exists to commit) — so two
scratch modules were built and rendered, neither committed:

* `starplayer_offline::fixtures::synthetic_mod()` itself (the repo's own MOD regression
  fixture): an 8-bit 128-frame ramp looped over its whole length, played among three other
  voices with vibrato, portamento and volume slides.
* A second, hand-built single-channel MOD with one 8-bit sample and a genuinely short
  16-frame forward loop (a plain ramp, so the loop wraps on a real discontinuity, the
  ProTracker chip-instrument case the question asks about — the 2–32-frame range OpenMPT's
  own test corpus and most Amiga chip loops sit in) and no other effects, so the
  measurement is not confounded by the first fixture's vibrato and slides.

Both were rendered plain and with `--enhance sinc4x+loop` (`starplayer-cli render <file>
-o <out>.wav [--enhance sinc4x+loop] --max-seconds 3`), then compared sample-for-sample
and by Goertzel power at a spread of frequencies from 1 kHz to 20 kHz, using a pure-Python
script with no dependency beyond the standard library (no `numpy`/`scipy` available in
this environment). `info --enhance sinc4x+loop` on the short-loop module confirms the
rebuild: sample 0 goes from 16 frames (loop `0..16`) to 64 frames (loop `0..64`) at ×4,
`loop=64` clamped internally to `min(64, 64/2) = 32` frames of crossfade by
`LoopSmoother`'s own rule.

**What differs, numerically.**

On the short-loop module (the cleaner measurement — the multi-voice fixture's other
channels dominate a whole-file RMS and blur the loop's own contribution):

* The two renders are **not** identical: `diff RMS / plain RMS ≈ 2.2 %` over a 3-second
  render — small, but far from the identity case (K5a's own identity-rebuild test proves a
  zero-scale enhancer changes nothing at all); this is a real, if modest, change.
* Above 5 kHz — well above the source's own ~4.14 kHz Nyquist (8287 Hz effective rate at
  Amiga period 428, PAL clock ÷ (428×2)) — the enhanced render's Goertzel power is
  consistently **2–3 dB lower** than the plain render's at every probed frequency (5, 6, 8,
  10, 12, 18 kHz), and the above-Nyquist-to-below-Nyquist power ratio drops from
  **0.62 % to 0.48 %**. That is the direction `LoopSmoother`'s own documentation predicts:
  the plain render's 16-frame loop wraps on a real discontinuity every 16 source frames
  (every 64 frames of the rebuilt sample, once `sinc4x` has quadrupled it), and each wrap
  injects a broadband click; smoothing the seam removes some of that broadband energy
  rather than adding any, so the enhanced render should have *less* high-frequency content
  at the seam, not more — and that is what the measurement shows, not new spectral content
  invented above the source's own band.
* Both renders already carry very little energy above 5 kHz in absolute terms (roughly
  −87 to −98 dB relative to full scale in the probed bins on this fixture) — a 16-frame
  ramp's fundamental and low harmonics dominate; the click energy the loop seam
  contributes is a small fraction of the total even unsmoothed.

On `synthetic_mod()` (the multi-voice fixture, 128-frame loop, more representative of an
ordinary tracker mix): `diff RMS / plain RMS ≈ 1.8 %`, and the above/below-Nyquist power
ratio moves from 4.45 % to 3.08 % over the same probe window — the same direction, at
similar magnitude, on a file where the loop wrap is one voice among four rather than the
only thing playing.

**What this means for the question asked.** Numerically, `sinc4x+loop` on a short-looped
8-bit chip sample makes a small, real, and — by the mechanism `LoopSmoother` documents —
*corrective* rather than colouring change: it removes some of the loop-seam's own click
energy rather than adding new high-frequency content the source never had. `sinc4x` alone
(no `loop`) would be expected to leave the click intact while still reconstructing the
existing waveform through more taps than linear interpolation's two; this task did not
measure that combination in isolation, since the question is specifically about
`sinc4x+loop` together. Whether a change of this size is *audible* on real chip material —
as opposed to measurable — is exactly the judgement call that needs ears, not a Goertzel
bin, and is the owner's own listening check against `/tmp/enhanced.wav` and `/tmp/plain.wav`
(rendered from REFLEX.S3M, per this task's Verification block) plus whatever 8-bit MOD the
owner chooses to try `--enhance sinc4x+loop` against directly.

### Deviations from the task file

* `RenderTarget`/`RenderSmfTarget`'s `describe` helper gained a fifth parameter
  (`enhancer: Option<&dyn SampleEnhancer>`) rather than a second overload, so the printed
  configuration line has exactly one implementation for both the tracker and the `.mid`
  render paths; the `.mid` path always passes `None` since `--enhance` is refused there.
* `render`'s and `play`'s `file`/`output` clap attributes moved from
  `required_unless_present = "list_effects"` to `required_unless_present_any =
  ["list_effects", "list_enhancers"]`, which the task file's reference to the `--insert`
  model did not call out explicitly but is the same shape `--list-effects` already used.
* A top-level `starplayer --list-enhancers` was added beside the existing top-level
  `starplayer --list-effects` (`main.rs`), for the same reason `--list-effects` has one:
  a caller who has not yet typed a subcommand can still ask what a build's catalogue
  offers. Not required by the deliverable text, which only names `render`/`play`/`info`,
  but a small, consistent addition rather than a widening of scope.
* `--enhance` is explicitly refused (with a one-line error naming the flag) rather than
  silently ignored when the input is a Standard MIDI File, on both `render` (matching the
  existing `--insert`-on-`.mid` refusal already there) and `play` (which had no equivalent
  check for `--insert`, since inserts are bus effects that apply above any source; an
  enhancer, by contrast, has no module to rebuild in the SMF-via-`--instruments` path this
  task did not touch, so silently ignoring it would be a promise the flag does not keep).
* The research-point-2 scratch modules (`synthetic_mod()` dumped to a temp file, and the
  hand-built short-loop MOD) were generated by two `#[ignore]`d unit tests added
  temporarily to `crates/starplayer-offline/src/fixtures.rs`, run once each, then reverted
  with `git checkout` — neither is part of any commit on this branch. The spectral
  comparison script (`spectral_compare.py`, pure standard library) and the rendered WAVs
  it read live only in the scratchpad directory, not in the repository.
