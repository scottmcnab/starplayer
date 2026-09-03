# M5 — F2: XM playback — instruments, envelopes, the effect set and the conformance loop

| Field | Value |
|---|---|
| Milestone | M5 ([master plan](M5-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Landed 2026-09-04; owner listening check outstanding |
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

## Research resolution

### 1. What libxmp's XM dumps contain — **verdict: an Amiga-style mixer period, not XM's linear one; the projection converts the oracle into FastTracker 2's domain**

Settled from the pinned tree before any effect code was written, and written into
`conformance/README.md` §"XM (task F2)".

`test-dev/compare_mixer_data.c:60-98` is the authority on the columns: `time row frame chan
xc->info_period vi->note vi->ins vi->vol vi->pan vi->pos0 [cutoff [resonance]]`, one line per
tick per channel with a mapped, sounding voice.

* **`period`.** `xc->info_period = MIN(final_period * 4096, INT_MAX)` where
  `final_period = libxmp_note_to_period_mix(xc->note, linear_bend)` is
  `13696 / 2^((note + bend/12800)/12)` (`src/player.c:1276-1298`, `src/period.c:205-209`).
  That is the **same continuous Amiga-style quantity libxmp reports for every format**; XM's
  linear period lives only in `xc->period` and is never dumped. FastTracker 2's linear period
  is `7744 - 64 * note - 4 * (finetune >> 3)`, so the adapter converts the *oracle* into FT2's
  domain — `ft2_period = 8448 + 768 * log2(mix_period / 13696)` — rather than the reverse.
  The direction matters: in FT2's domain a finetune-sized pitch difference is a fixed handful
  of period units at every pitch, where in libxmp's it is hundreds of Q12 units at a low
  period and a fraction of one at a high one, so no single tolerance would work. The anchor
  constant is FT2's own period at libxmp's `PERIOD_BASE`: libxmp note 60 is C-4, mixer period
  428, FT2 period 4608, and `4608 + 768 * log2(13696 / 428) == 8448`. **Amiga**-mode XM needs
  no logarithm at all — FT2's Amiga period is four times ProTracker's, so it divides by 1024
  exactly as S3M does — and which of the two applies is read from the loaded module's
  `linear_slides` flag through `SampleGeometry`, for the same reason the loop spans are.
* **`note`.** `vi->note`, set only by `libxmp_virt_setpatch`, and built as
  `(XM note byte - 1) + 12 + relative_note` (`src/read_event.c:549-598`, folding `sub->xpo`;
  `map[].xpo` is always zero for XM). `XmProcessor` traces the same "real note" FastTracker 2
  computes — the pattern's note byte plus the sample's relative note, zero-based from C-0 —
  so `project_note` is the same one-octave subtraction S3M and MTM already use. It reports the
  note the **sounding voice** started on rather than the latched note byte, because FT2 latches
  a note and then rejects a transposed value outside C-0..B-9 while leaving the previous note
  playing (`XmChannel::sounding_note`).
* **`volume`.** `vi->vol`, 0..1024, post-tremolo, post-fadeout, post-envelope,
  post-global-volume, post-track-volume and post-tremor, with truncations at `>> 6` and
  `>> 18` (`src/player.c:1061-1119`). The adapter's existing `(x + 8) / 16` lands it on the
  trace's 0..64, so the processor keeps its own final volume in a 0..65536 integer domain and
  **rounds** into 64 — truncating disagreed by one wherever the discarded bits were ≥ 8.
* **`pan`.** Signed −128..+127, post pan-envelope. XM keeps the whole byte, as MOD does,
  because an XM panning envelope moves it one unit at a time.
* **`position`.** `vi->pos0` is the truncated integer source position at the *start* of the
  tick (`src/mixer.c:543-553`), so XM floors its Q32.32 position the way MOD and MTM do and
  then applies libxmp's own one-integer-frame bound.
* **`step_per_output_frame`.** For XM the harness predicts a one-shot's end from
  `8363 * 2^((4608 - period) / 768)`, in `f64` — this is the comparison harness, not the
  engine, whose own path is `starplayer-core`'s table.
* **An empty dump is an oracle.** libxmp writes a line only for a mapped, sounding channel, so
  a fixture whose whole point is that nothing plays has a zero-byte `.data`.
  `parse_libxmp_dump` no longer rejects that, and `diff_libxmp_dump` reads it as "no channel
  may ever be active" and enforces it against the whole capture rather than comparing two
  empty projections. `openmpt/xm/DelayCombination.data` is exactly that and passes;
  `openmpt/xm/PanMemory.data` is empty for a module that *does* sound notes, which is
  `F2-XM-012`.

### 2. Integer Hz — **verdict: the premise is a UI function, not the replayer; the real drift is FastTracker 2's sixteen-step finetune, and it is accuracy policy D42**

`linearPeriod2Hz` in the task description is `getSampleC4Hz` (`ft2_replayer.c:286-311`), which
FT2 uses to *display* a sample's mid-C rate in the instrument editor. The **replayer** never
materialises a frequency in hertz at all: `period2Ft2Delta` (line 239) goes straight from the
period to a Q16.16 mixer delta through `logTab`, with no integer-hertz step anywhere. There is
therefore nothing to record about integer-hertz truncation, and StarPlayer's
`step_for_period` reads the same shared Q8.24 table with the same shift idiom and divides once
by the output rate, so its only rounding is the table's own.

What the research point was reaching for is real, but it is one layer up. FastTracker 2
quantises a sample's **finetune** to sixteen steps: its 1936-entry period table is indexed
`note * 16 + ((finetune >> 3) + 16)` (`triggerNote`, and OpenMPT's `kFT2FinetunePrecision`),
where libxmp interpolates `finetune / 128` continuously (`src/period.c:184-188`). The two
therefore play the same pattern note up to `finetune/2 - 4 * (finetune >> 3)` native period
units apart — at most 3.5 — and the source position drifts from the very first tick.

**Measured on the corpus:** of the 93 XM cases, **38** carry at least one sample whose
finetune is not a multiple of eight, and every one of those 38 diverges on `position` and on
nothing else. They are all OpenMPT fixtures, and 37 of them use finetune −28. The engine was
**not** changed to match libxmp, exactly as D14 keeps ProTracker's sixteen integer finetune
tables against the same continuous formula; the harness compares XM periods with a four-unit
tolerance, which is that bound rounded up, and the 38 cases waive `position`. Two consequences
are recorded with them: where glissando rounds the sliding period to a note the two can land a
whole semitone apart (`openmpt-xm-glissando`), and where a one-shot sample runs out the 0.18 %
pitch difference moves the tick it ends on, so five cases additionally waive `active`.

A **second**, unrelated position family turned up in the same sweep: 14 cases whose finetunes
are all multiples of eight still drift, and there the cause is D19 — libxmp truncates every
mixer tick to `(int)(rate * 2.5 / bpm)` where StarPlayer carries the rational remainder. That
is the entry MTM already uses, reused rather than duplicated.

### 3. Which `kFT2*` behaviours the cases exercise — **verdict: catalogued before implementing; nine are reproduced by design and named in accuracy policy §1**

The 50 `openmpt/xm` fixtures with oracles were catalogued from their `test_openmpt_xm_*.c`
comments before the processor was written, and the 43 `data/ft2_*.xm` fixtures with them (see
research point 5). Grouped by mechanism, with the `Snd_defs.h` names checked against the
OpenMPT tree rather than guessed:

| Mechanism | Behaviours | Fixtures |
|---|---|---|
| Note delay | `kFT2NoteDelayWithoutInstr`, `kFT2OutOfRangeDelay`, `kFT2RetrigWithNoteDelay`, `kFT2PanWithDelayedNoteOff`, `kFT2VolColDelay` | `delay1`..`delay3`, `DelayCombination`, `envretrig`, `OffDelay`, `PanOff`, `delaycut`, the five `ft2_delay_*` |
| Note-off / key-off | `kFT2KeyOff`, `kFT2NoteOffFlags`, `kFT2ReloadSampleSettings` | `key_off`, `KeyOff2`, `NoteOff`, `NoteOff2`, `NoteOffVolume`, `NoteOffFade`, `keyoff+instr`, `ft2_kxx`, `ft2_k00_*` |
| Envelopes | `kFT2EnvelopeEscape`, `kFT2SetPanEnvPos` | `EnvLoops`, `EnvOff`, `SetEnvPos`, `Pickup`, `3xxins`, `ft2_envelope_*` |
| Tone portamento | `kFT2PortaIgnoreInstr`, `kFT2PortaUpDownMemory`, `kFT2VolColMemory` | `porta-offset`, `Porta-Pickup`, `Porta-LinkMem`(`_old`), `TonePortamentoMemory`, `ft2_double_toneporta` |
| Offset | `kFT2ST3OffsetOutOfRange`, `kFT2OffsetMemoryRequiresNote` | `OffsetRange`, `3xx-no-old-samp`(`-noft`), `ft2_offset_memory` |
| Arpeggio and periods | `kFT2Arpeggio`, `kFT2Periods`, `kFT2FinetunePrecision` | `Arpeggio`, `ArpeggioClamp_old`, `ArpSlide_old`, `finetune`, `FreqWraparound` |
| Waveforms | `kFT2MODTremoloRampWaveform` | `TremoloWaveforms`, `VibratoWaveforms` |
| Tremor | `kFT2Tremor` | `Tremor`, `TremorInstr`, `TremorRecover`, `ft2_tremor_*` |
| Panning | `kFT2PanSlide`, `kFT2VolColMemory` | `PanSlideMem`, `PanMemory`, `PanMemory2` |
| Pattern flow | `kFT2PatternLoopWithJumps`, `kFT2LoopE60Restart`, `kFT2RestrictXCommand` | `PatLoop-Break`, `PatLoop-Weird`, `PatternDelays`, `PatternDelaysRetrig` |
| Note range | `kFT2Transpose` | `NoteLimit`, `NoteLimit2`, `ft2_note_range*` |
| Retrigger | `kFT2Retrigger` | `E90`, `ft2_*retrig*` |

Nine of these are now rows in `plans/product/03-accuracy-policy.md` §1 — the arpeggio table
overrun, the tremolo ramp's vibrato sign, the pattern-loop/jump interaction, the finetune
quantisation, the out-of-range transpose, the invalid-sample cut, `Rxy`'s tick-zero
retrigger, `Lxx`'s panning-envelope gate, the volume-column pan slide of zero, and the `Xxy`
restriction — because each is a behaviour StarPlayer reproduces on purpose and would
otherwise look like a bug. Three of the twelve `F2-XM-*` records name a quirk that is **not**
reproduced yet: `kFT2LoopE60Restart` (`F2-XM-009`), the Skale offset dialect (`F2-XM-007`)
and the ModPlug/MadTracker/rst double tone portamento (`F2-XM-001`).

### 4. FastTracker 2's volume ramping — **verdict: a mixer property; no oracle field depends on it, and the mixer's 64-frame ramp is untouched**

FT2's `CS_USE_QUICK_VOLRAMP` selects a ramp length in its own mixer
(`calcReplayerVars`: `quickVolRampSamples = audioFreq / (refFreq / (refFreq / 200))`), and
`ft2-clone` applies it in `ft2_mix.c`, not in the replayer. Nothing in the replayer's state
depends on it.

On the oracle side the dumped `vol` is `vi->vol`, which `libxmp_mixer_setvol` assigns
directly (`src/mixer.c:946-955`) — libxmp's own ramp lives in `vi->old_vl`/`vi->vol` deltas
computed inside `libxmp_mixer_softmixer` and is never dumped. So neither implementation's
ramp is observable through the comparison, and `starplayer-mixer`'s `RAMP_FRAMES` was not
touched. The one place a ramp *is* visible is the advisory perceptual comparison, where it is
part of the audio rather than of the state.

### 5. The `data/*.xm` inline expectations — **verdict: the task file's premise was wrong; 43 of them already carry `.data` oracles, and adding XM to the audit made them mandatory**

The task file expected the `data/*.xm` player fixtures to state their expectations inline in
`test_player_ft2_*.c`. They do not: 37 of the 40 XM player tests are a bare
`compare_mixer_data("data/ft2_X.xm", "data/ft2_X.data")` against a `.data` file in **exactly
the format** the OpenMPT fixtures use. So there is no second oracle type to consider and
nothing to convert — the whole corpus shares one schema.

That changed the scope of deliverable 1. `audit_pinned_corpus` requires the manifest to be
the complete set of `compare_mixer_data*` calls whose module has a target extension, so the
moment `matches_target_extension` gained `"xm"` **every** XM call had to appear in
`cases.tsv`, not only the `openmpt/xm` ones. The manifest therefore grew by 93 rows — 50
OpenMPT and 43 libxmp — rather than the 50 the task file anticipated, and all 93 are executed,
compared and accounted for. Leaving the libxmp ones out was not an option: the audit is a hard
error, and it is the check that stops the corpus quietly shrinking.

Three of the 40 XM player tests are bespoke and have no fixture at all —
`test_player_ft2_note_noins_after_invalid_ins`, `test_player_ft2_note_noins_after_keyoff` and
`test_player_xm_envelope_zero_loop` build a module in memory and assert on `mixer_voice`
fields directly. They are not convertible without hand-porting, and this task did not build a
second oracle type for them, as instructed.
