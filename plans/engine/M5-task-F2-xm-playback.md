# M5 — F2: XM playback — instruments, envelopes, the effect set and the conformance loop

| Field | Value |
|---|---|
| Milestone | M5 ([master plan](M5-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | F1 (loader), E1–E3, D3 — all landed |
| Blocks | F5 (conformance repairs), M5 exit |
| Parallel with | D4, D5, G1, G3 |
| Recommended model | Claude Opus (the M5 accuracy core; the largest single task of the milestone, like B4 was for S3M) |
| Verified by | agent (`cargo xtask conformance --offline` with the XM cases wired, goldens, block-size determinism), then reviewer, then the owner listens to XMs in the web player |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there are non-negotiable, above all: sample-exact tick timing, no
allocation/locks/panics in `render()`, no transcendental functions on the RT path (tables
only), byte-identical output at every host block size, and **format identity**: XM gets
its own effect processor and is never lowered into S3M.

Task F1 landed the loader: `crates/starplayer-xm` produces a `Module` whose patterns are
fixed five-byte cells (`XmCell { note, instrument, volume, effect, parameter }`,
`pattern.rs`), whose instruments carry a 96-entry map of global sample ids, volume and
panning `Envelope`s (sustain as a `start == end` span), `fadeout` and per-sample
`auto_vibrato`, and whose samples carry raw `relative_note` and `finetune`. The header's
`format_extra` decodes to `XmFormatExtra { restart_position, flags }` and
`ModuleFlags::linear_slides` says whether pitch is linear. `XmPatternData` implements
`PatternData`. **Nothing plays it yet.** This task makes it play, and proves it against
the corpus.

This is the milestone's accuracy core. The reference is **not** the original DOS player
— it never supported XM — but FastTracker 2 itself, as documented by
`plans/product/03-accuracy-policy.md`: "For XM and IT: the format specifications and
OpenMPT's documented compatibility behaviour are the reference." Concretely:

- **ft2-clone** (`8bitbubsy/ft2-clone` on GitHub, `src/ft2_replayer.c`) is a
  cycle-faithful C port of FT2's replayer and the most exact statement of what FT2 does
  per tick: `fixaEnvelopeVibrato`, `getNewNote`, `doEffects`, `volume` column handling,
  `linearPeriod2Hz`, `relocateTon`, the arpeggio and retrig counters. Read it with
  WebFetch and treat it as the specification.
- **OpenMPT** (`OpenMPT/openmpt`, `soundlib/Snd_fx.cpp`, `Sndmix.cpp`, `Sndfile.h`'s
  `PlayBehaviour` enum) names every FT2 quirk it reproduces (`kFT2*`), and the OpenMPT
  wiki's XM compatibility page explains each. Read them.
- **libxmp** is the *oracle*: the pinned corpus (`cargo xtask conformance --fetch-only`)
  holds **50 `openmpt/xm/*.xm` fixtures with per-tick mixer dumps** (`*.data`), each
  isolating one FT2 behaviour, plus 49 `data/*.xm` player fixtures without dumps whose
  `test_player_ft2_*.c` sources state expected per-frame channel state inline. Where
  libxmp and FT2 disagree, FT2 wins and the case gets an exclusion with an accuracy-policy
  entry, exactly as M2 did for libxmp's S3M representation differences (D33–D39).

### The engine seam you are implementing

`TrackerProcessor` (`crates/starplayer-engine/src/sequencer.rs`): `row` / `tick` /
`reset` / `row_repeat` / `recommended_voice_capacity`, writing through `TickContext`
(`trigger_channel`, `stop_channel`, `write_voice_param`, `mark_voice_dirty`,
`report_effect`, `report_note`, `report_trace_channel`) and returning a `#[must_use]
TickOutcome` with the tempo, speed, pattern delay and jump in effect at the **end** of the
tick. `RowClock` gives you the absolute tick within a row. `S3mProcessor`
(`crates/starplayer-s3m/src/processor.rs`) is the template: per-channel state struct,
`latch_cell` → tick-zero effects → per-tick effects → `flush_channel` turning dirty
channel state into voice writes; `sequencer_for` / `sequencer_with_quirks` constructors.
The MOD processor (`crates/starplayer-mod/src/processor.rs`) shows a second, larger one,
including how `PatternFlowState` executes pattern loops and breaks.

### Per-voice state is format-owned (the M4-lite decision)

Architecture §5.3, as amended by E3: there is no engine envelope runner and no
`Instrument` trait. `XmProcessor` keeps envelope positions, fadeout level, key-off state
and auto-vibrato phase **per channel** — XM has exactly one voice per channel and never
detaches one — and advances them in its own `tick()`. Envelope output is written straight
to the voice's `params` (through `context.voices.get_mut(voice)`), *not* through
`write_voice_param`, so the trace's per-tick dirty flags stay effect-driven. Read the
"Why the parallel-array design is sound" section of
`plans/engine/complete/M4-task-E3-voice-lifecycle-and-trace-v2.md`; for XM the parallel
array degenerates to per-channel state, which is simpler.

### Code you must read before changing anything

- `crates/starplayer-xm/src/{lib,pattern,header,instrument,sample,data,loader}.rs` and
  `plans/engine/complete/M5-task-F1-xm-loader.md` (its research resolutions record what
  the loader decided).
- `crates/starplayer-engine/src/sequencer.rs` (all of it), `flow.rs`
  (`PatternFlowState`), `channel.rs`, `timeline.rs`, `trace.rs`.
- `crates/starplayer-s3m/src/processor.rs` and `crates/starplayer-mod/src/processor.rs`.
- `crates/starplayer-core/src/{tables,note,row_clock,quirks,fixed}.rs` —
  `LINEAR_FREQUENCY_TABLE`, `linear_frequency_q24`, the waveform tables, `Step::from_ratio`,
  `QuirkSet`/`FormatDialect` (the XM variants carry no quirks yet; add fields only when a
  corpus case names one, with a policy entry and a test, per C5's rules).
- `crates/starplayer-model/src/{instrument,sample,pattern}.rs` — what the loader filled in.
- `crates/starplayer/src/{lib,sequencer}.rs` — where the `xm` arms go (`probe`, `load`,
  `scan_song`, `NativeSequencer`, `supports`, `recommended_voice_capacity`).
- `crates/starplayer-testkit/src/conformance.rs` (the whole libxmp adapter: `ConformanceFormat`,
  `project_note`, `project_period`, `project_pan`, `project_actual_position`,
  `step_per_output_frame`, `SampleGeometry`, `TraceTolerances`, `tick_end_frames`),
  `bin/starplayer-conformance.rs`, `conformance/{README.md,cases.tsv,exclusions.tsv,known-failures.md}`.
- `crates/starplayer-offline/src/{lib,fixtures}.rs`, `src/bin/starplayer-goldens.rs`
  (`GoldenFormat`, the synthetic fixtures), `tests/fuzz_seeds.rs` (drop the F1
  dev-dependency workaround once the facade has `xm` arms).
- `crates/starplayer-host-wasm/src/lib.rs`, `apps/starplayer-web/www/app.js` (accepted
  extensions, `effect_names`), `apps/starplayer-web/src/lib.rs` (`effect_names`,
  `pattern_window`), `apps/starplayer-cli` if D5 has landed.
- `plans/product/03-accuracy-policy.md` (§3's table shape; XM entries are **D42–D59**),
  `plans/engine/complete/M1-task-B4-s3m-effects.md` (how an effect-by-effect spec reads)
  and `M2-task-C9-s3m-conformance-repairs.md` (how exclusions are justified).

## Deliverables

Land them in this order; each is testable before the next.

### 1. Wiring, so the corpus can be run from the start

- Facade: `xm` arms in `probe`, `load`, `scan_song`, `NativeSequencer` (`XmSequencer`
  alias), `supports`, `recommended_voice_capacity`; `xm` joins the facade's default
  features. Remove the `starplayer-xm` dev-dependency F1 added to `starplayer-offline`.
- Harness: `ConformanceFormat::Xm` with its projection arms, `SampleGeometry`, and
  `matches_target_extension`; rows in `conformance/cases.tsv` for every `openmpt/xm`
  fixture that has a `.data` oracle (ids `openmpt-xm-<name>`, source `openmpt`), with the
  behaviour column taken from the matching `test_openmpt_xm_*.c` comment; the format's
  oracle semantics documented in `conformance/README.md` (research point 1 decides the
  note and period projections).
- Offline: `GoldenFormat::Xm` and `fixtures::synthetic_xm()` — a licence-free generated
  XM that exercises linear frequency, an instrument with two samples, both envelopes,
  key-off and a ping-pong loop — hashed by `cargo xtask goldens` once the processor is
  right.
- Web player and CLI accept `.xm` (`EffectNames::XM` for the effect column; the volume
  column shown through `XM_VOLUME_COLUMN`).

At this point `cargo xtask conformance --offline` runs the XM cases and fails them all,
honestly.

### 2. `XmProcessor`: notes, instruments, pitch, volume, pan

`XmChannel` state (mirror ft2-clone's `channel_t`, field for field, with its names in
the doc comments as `S3mChannel` does for `ChannelData`): current note, instrument and
sample, period and target period, volume, pan, finetune and relative note in effect,
effect memories (one per effect family as FT2 keeps them), vibrato/tremolo phase and
waveform, pattern-loop state, note-delay latch, retrig and tremor counters, and the
articulation state of deliverable 3.

- **Note → sample** through `note_sample_map`; note-off (`97`) and instrument-only rows
  follow FT2's `getNewNote` exactly (an instrument without a note resets volume and
  envelopes but does not retrigger; an invalid instrument number cuts — `ft2_invalid_ins_defaults.xm`
  and friends).
- **Pitch**: linear mode `period = 7680 − note·64 − finetune/2` (FT2's
  `relocateTon`/period arithmetic with `relative_note` folded into the note) and
  `hz = 8363 · 2^((4608 − period)/768)` through `linear_frequency_q24` — integer Hz as FT2
  truncates it, then `Step::from_ratio(hz, rate)`; Amiga mode through FT2's finetuned
  `amigaPeriods` table, generated at construction from the 16 finetune tables (no
  floats — transcribe or derive the way ft2-clone does). Record the D14-style
  representation gap against libxmp's continuous formula as a policy entry if the corpus
  shows it.
- **Volume** 0..64 with FT2's clamping; **pan** 0..255 from the sample's default pan on
  note-on, `8xx`/`E8x`/volume column afterwards; global volume `Gxx`/`Hxy`.
- `flush_channel` as in S3M: trigger through `trigger_channel` with a `VoiceTag`, then
  `Step`/`Volume`/`Pan` writes for dirty fields. XM never detaches a voice;
  `recommended_voice_capacity` keeps the default.

### 3. Instrument articulation: envelopes, key-off, fadeout, auto-vibrato

Per channel, advanced once per tick in `tick()` (and on tick zero after the row's note
handling, in the order ft2-clone's `fixaEnvelopeVibrato` uses):

- **Volume and panning envelopes** with point interpolation, the sustain point held
  while the note is on, the loop span, `Lxx` setting the position, and FT2's escape
  rules (`ft2_envelope_reset.xm`, `EnvLoops`, `EnvOff` cases).
- **Key-off** (`97`, `Kxx` at its tick, and the volume column's) releasing the sustain;
  **fadeout** subtracting `fadeout` from a 0..32768 level per tick after key-off — and
  immediately on a key-off with no volume envelope, as FT2 does (`ft2_note_off_fade.xm`,
  `ft2_volume_fadeout.xm`, `NoteOffFadeNoEnv`).
- **Auto-vibrato** with sweep, the four waveforms (`AutoVibratoWaveform::RampUp` included),
  applied to the period every tick (`ft2_*` and `openmpt/xm/AutoVibrato*` cases).
- The final voice volume is `channel volume × envelope × fadeout × global volume`, written
  directly to `params` every tick; pan likewise. `report_trace_channel` reports the
  post-envelope 0..64 volume and the FT2 period.

### 4. The effect set and the volume column

Effect by effect, tick-zero and per-tick behaviour and parameter memory, from
ft2-clone: `0` arpeggio (with FT2's tick-counter quirk at speeds above 16), `1`/`2`
portamento (with `X1x`/`X2x` extra-fine and `E1x`/`E2x` fine), `3` tone portamento
(`E3x` glissando; FT2's target-note-without-instrument rules —
`ft2_invalid_porta_target.xm`, `ft2_double_toneporta.xm`), `4` vibrato (`E4x`
waveform), `5`/`6` combined slides, `7` tremolo (`E7x`), `8` pan, `9` offset (FT2's
out-of-range behaviour — `ft2_offset_memory.xm`), `A` volume slide (`EAx`/`EBx` fine),
`B` position jump and `D` pattern break (with `restart_position` and FT2's
break-past-end behaviour), `C` volume, `E6x` pattern loop (through `PatternFlowState`
with FT2's flow semantics), `E9x` retrigger, `ECx` note cut, `EDx` note delay (FT2's
interaction with instrument and volume column — the five `ft2_delay_*.xm` fixtures),
`EEx` pattern delay, `F` speed/tempo (`< 32` speed, else BPM; `F00` per FT2),
`G`/`H` global volume, `K` key-off at tick, `L` envelope position, `P` pan slide, `R`
multi-retrigger (`Rxy` with its volume changes), `T` tremor (`ft2_tremor_*.xm`), `W`
and `X`-beyond-`X2` ignored with a report. The **volume column** vocabulary:
`0x10..=0x50` set, `0x6x`/`0x7x` slides, `0x8x`/`0x9x` fine slides, `0xAx` vibrato
speed, `0xBx` vibrato, `0xCx` pan, `0xDx`/`0xEx` pan slides, `0xFx` tone portamento —
with FT2's rule that the volume column and the effect column interact (`ft2_delay_volume_column.xm`,
`kFT2VolColMemory`). `report_effect` with `EffectNames::XM` names.

### 5. The corpus loop

Run `cargo xtask conformance --offline`. For every XM case that fails: fix the processor
if FT2 (ft2-clone) agrees with libxmp; if they disagree, keep FT2 and add an
`exclusions.tsv` row whose reference is `plans/product/03-accuracy-policy.md` and a
**D42+** entry naming both sources by file and line, with waivers for the field libxmp
represents differently (the D33–D39 shape). A case you could not settle goes to
`known-failures.md` with a `C`-style id (`F2-XM-001`, …) for F5. **No case is left
unexplained.** Also run every `data/*.xm` player fixture through `scan_song` and a
10-second render with the allocator hook (`render_allocation.rs` already walks the
corpus — extend its format list) to prove no panic, no allocation and no engine warning.

### 6. Goldens, determinism, docs

`cargo xtask goldens` for the synthetic XM (commit the hash); the block-size
determinism test gains an XM render; `plans/README.md` M5 row; the accuracy policy gains
its XM section (D42+) and a note under §1 for FT2 quirks reproduced by design;
`plans/engine/M5-master-plan.md` records what landed. Any `QuirkSet` field added is
documented per C5's rules with its corpus case and test.

## Research points

1. **What libxmp's XM dumps contain.** `period` for a linear-frequency XM (libxmp's
   internal period, linear or Amiga?), `note` numbering (libxmp's key versus the XM note
   byte), `volume ×16` (post-envelope), `pan`, `position`, and how the adapter's
   `step_per_output_frame`/`project_period` must project them. Settle from
   `src/player.c`, `src/period.c`, `src/loaders/xm_load.c` and `test-dev/compare_mixer_data.c`
   in the pinned tree, and write it into `conformance/README.md` before writing effect code.
2. **Integer Hz.** FT2 truncates `linearPeriod2Hz` to an integer; libxmp does not. Measure
   the position drift on the corpus and record it (a D42-family entry) rather than
   changing the engine to a rounded secondary oracle — the D14 precedent.
3. **Which `kFT2*` behaviours the 50 cases exercise.** List them from the `test_openmpt_xm_*.c`
   comments and the OpenMPT wiki before implementing, so each gets a test.
4. **FT2's volume ramping** is not a replay quirk (it is a mixer property); confirm no
   oracle field depends on it and leave the mixer's 64-frame ramp alone.
5. **The `data/*.xm` inline expectations** in `test_player_ft2_*.c`: worth a second
   oracle type? Record how many could be mechanically converted; do not build it here.

## Verification

```sh
cargo test -p starplayer-xm
cargo test --workspace
cargo xtask conformance --offline                 # XM cases wired; every failure excluded with a reason or in known-failures
cargo xtask conformance --offline --strict        # report the count of known failures it names
cargo xtask goldens --check                       # existing 7 unchanged; the XM golden added
cargo test -p starplayer-engine --test block_size_determinism
cargo xtask ci --job rt-safety                    # the corpus walk now includes XM
cargo xtask ci --job no-std-check
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
cargo xtask ci --job trace-zero-cost
cargo xtask wasm && node apps/starplayer-web/test/worklet-harness.mjs
cargo xtask perceptual --corpus target/conformance/corpora/libxmp-*/test-dev/openmpt/xm/*.xm   # advisory; report the LSD column
```

Report the conformance table (XM passed / accepted / known / gated), the exact commands
and results, and every accuracy-policy entry you added. **Do not commit** — the reviewer
commits.

## Out of scope

NNA and anything IT (M6). MIDI. Changing the S3M/MOD/MTM processors or the shared engine
envelope-free design. Fixing the pre-existing `headless.mjs` failure.
