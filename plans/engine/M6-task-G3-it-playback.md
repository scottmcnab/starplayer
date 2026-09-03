# M6 — G3: IT playback — instruments, NNA, the effect set and the conformance loop

| Field | Value |
|---|---|
| Milestone | M6 ([master plan](M6-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | G1 (loader), E1–E3, D3 — all landed. G2 (the filter) is concurrent: this task writes `VoiceParam::Filter`; G2 makes it audible |
| Blocks | G6 (conformance repairs), M6 exit |
| Parallel with | G2, D4, F2 |
| Recommended model | Claude Opus (the hardest format in scope: NNA, voice stealing, and the most demanding corpus) |
| Verified by | agent (`cargo xtask conformance --offline` with the IT cases wired, goldens, determinism), then reviewer, then the owner listens to dense ITs in the web player |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there are non-negotiable, above all: sample-exact tick timing, no
allocation/locks/panics in `render()`, tables only on the RT path, byte-identical output at
every host block size, and **format identity**: IT gets its own effect processor and is
never lowered into another format.

Task G1 landed the loader: `crates/starplayer-it` produces a `Module` whose patterns are
fixed five-byte cells (`ItCell { note, instrument, volume, command, parameter }`, with
`NOTE_NONE = 252`, `NOTE_FADE = 253`, `NOTE_CUT = 254`, `NOTE_OFF = 255`,
`ItVolumeCommand` decoding the volume column), whose instruments carry the 120-entry
note→sample and note-transpose maps, three envelopes with carry and the filter bit,
fadeout, NNA/DCT/DCA, global volume, pan, pitch-pan, random variation and the initial
filter, and whose samples carry `C5Speed`, sustain loops, per-sample auto-vibrato and a
global volume in `format_data`. `ItFormatExtra` decodes the header flags (instrument
mode, old effects, compatible `Gxx`, linear slides, MIDI pitch controller, extended
filter range) and `ItFormatData` exposes the channel pan/surround/disabled table, channel
volumes, sample global volumes and the embedded MIDI macros. `ItPatternData` implements
`PatternData`. Read `plans/engine/complete/M6-task-G1-it-loader.md` — its research
resolutions and accuracy entries D60–D63 record what the loader decided. **Nothing plays
it yet.**

This is the milestone's accuracy core and the reason the architecture was shaped the way
it is: IT lets a channel keep sounding several voices at once (architecture §5.1), which
is why the voice pool is global and generational (§5.2), why `VoiceTag` exists, and why
task E3 added `detach_foreground`, `iter_mut` and trace v2's per-voice lines.

### The references

- **OpenMPT** (`OpenMPT/openmpt`): `soundlib/Snd_fx.cpp` (note handling, `InstrumentChange`,
  `NoteChange`, `CheckNNA`, every effect), `Sndmix.cpp` (`ProcessEffects`, envelopes,
  `ProcessInstrumentFade`, the `GetNNAChannel` voice-stealing heuristic — this settles
  architecture open question **Q3**), `Sndfile.h`'s `PlayBehaviour` (`kIT*`), and the
  OpenMPT wiki's IT compatibility page. Read them with WebFetch.
- **Schism Tracker** (`schismtracker/schismtracker`, `player/snd_fx.c`, `player/sndmix.c`)
  is a second faithful IT 2.14 replayer worth cross-reading where OpenMPT's history is
  tangled.
- **`ITTECH.TXT`** for the effect definitions, the `Sxx` family, the volume column and
  the MIDI macro defaults (`Zxx` → cutoff `F0F000z`, resonance `F0F001z`).
- **libxmp** is the *oracle*: the pinned corpus holds **59 `openmpt/it/*.it` fixtures with
  per-tick mixer dumps**, each isolating one behaviour, plus 85 `data/*.it` player fixtures
  whose `test_player_it_*.c` sources state per-frame expectations inline. Where libxmp and
  IT/OpenMPT disagree, IT wins and the case gets an exclusion with an accuracy-policy entry
  (the D33–D39 shape).

### The engine seam and the per-voice design

`TrackerProcessor` (`crates/starplayer-engine/src/sequencer.rs`), `TickContext`
(`trigger_channel`, `detach_channel`, `stop_channel`, `write_voice_param`,
`mark_voice_dirty`, `report_effect`, `report_note`, `report_trace_channel`,
`report_trace_voice`), `#[must_use] TickOutcome`, `RowClock`, `PatternFlowState`;
`S3mProcessor` is the template and `ModProcessor` the larger example.

**Per-voice articulation state is format-owned** (architecture §5.3 as amended by E3;
`plans/engine/M3-M6-concurrency-plan.md`): `ItProcessor` owns
`Box<[ItVoiceState]>` sized to `pub const VIRTUAL_CHANNELS: usize = 256`, indexed by
`VoiceId::index()` and validated by the stored `VoiceId` (generation), allocated once in
the constructor and cleared in `reset()`. Each entry holds the voice's instrument and
sample, root channel, envelope positions (volume, panning, pitch/filter) with their
sustain/loop state, fadeout level, key-off/fade flags, auto-vibrato phase and sweep,
random volume/pan offsets, the NNA it will be detached with, and its last written filter
params. `tick()` walks `context.voices.iter_mut()`, skips ids it does not own, advances
every owned voice — foreground or background — and writes the resulting volume, step,
pan and filter straight to `voice.params`. `recommended_voice_capacity` returns
`VIRTUAL_CHANNELS`. A pool smaller than that is legal (fewer voices); larger is legal
(ids past the array are skipped).

### Code you must read before changing anything

- `crates/starplayer-it/src/*` and its task file's research resolutions.
- `crates/starplayer-engine/src/{sequencer,channel,flow,timeline,trace,scope}.rs`;
  `crates/starplayer-mixer/src/{voice,sample}.rs` (`VoicePool::{iter_mut,get_mut,release,allocate}`,
  `Voice::{set_region,retrigger,stop}`, `SampleRegion` for the sustain-loop swap).
- `crates/starplayer-s3m/src/processor.rs`, `crates/starplayer-mod/src/processor.rs`.
- `crates/starplayer-core/src/{tables,tempo,quirks,random,event,fixed}.rs` —
  `LINEAR_FREQUENCY_TABLE`, the IT slide tables (`linear_slide_up_q16` …), `ItModern`
  (a stub equal to `ExactFixedPoint`, pinned by `it_modern_is_currently_exact_fixed_point`),
  `Xorshift32`, `FilterParams`, `QuirkSet` (add IT fields only when a corpus case names
  one, with a policy entry and a test).
- `crates/starplayer-model/src/{instrument,sample,pattern,header}.rs`.
- `crates/starplayer/src/{lib,sequencer}.rs` — the `it` arms; `crates/starplayer-xm` if F2
  has landed by the time you start (it is the sibling implementation of the same design).
- `crates/starplayer-testkit/src/conformance.rs` (the whole adapter, including the
  `cutoff`/`resonance` oracle fields already parsed), `bin/starplayer-conformance.rs`,
  `conformance/{README.md,cases.tsv,exclusions.tsv,known-failures.md}`.
- `crates/starplayer-offline/src/{lib,fixtures}.rs`, `tests/{fuzz_seeds,render_allocation}.rs`.
- `crates/starplayer-host-wasm/src/lib.rs`, `apps/starplayer-web/{src/lib.rs,www/app.js}`,
  `apps/starplayer-cli`.
- `plans/product/01-technical-architecture.md` §5 (all), §12 Q3;
  `03-accuracy-policy.md` (§3 shape; IT entries continue at **D64**; G2 has reserved
  D70–D74, so take D64–D69 then D75 onward); `plans/engine/M6-task-G2-resonant-filter.md`
  (research point 1 there fixes the `FilterParams` encoding jointly with this task — agree
  on `FilterParams::from_it(cutoff, resonance)` in `starplayer-core` and let whichever task
  lands first add it).

## Deliverables

Land them in this order; each is testable before the next.

### 1. Wiring, so the corpus runs from the start

Facade `it` arms (`probe`, `load`, `scan_song`, `NativeSequencer::It`, `supports`,
`recommended_voice_capacity` = `VIRTUAL_CHANNELS`), `it` in the default features, drop
the `starplayer-it` dev-dependency G1 added to `starplayer-offline`; `ConformanceFormat::It`
with projection arms and `SampleGeometry`; `cases.tsv` rows for every `openmpt/it` fixture
with a `.data` oracle (`openmpt-it-<name>`, behaviour from the `test_openmpt_it_*.c`
comment); the oracle semantics for IT in `conformance/README.md` (research point 1:
periods, notes, cutoff/resonance, and **how libxmp dumps NNA background voices**);
`GoldenFormat::It` with `fixtures::synthetic_it()` (instrument mode, two samples, NNA
`Continue` on one instrument, all three envelopes, a sustain loop, no filter); web player
and CLI accept `.it`. At this point every IT case runs and fails honestly.

### 2. `ItProcessor`: channels, notes, instruments, pitch, volume, pan

`ItChannel` mirrors OpenMPT's `ModChannel` where IT-relevant, with the names in doc
comments: the row's note/instrument/volume/command, the foreground voice, channel volume
(`Mxx`), pan (and surround), the effect memories (IT keeps **one shared memory per
channel** for most effects, plus the `EFG`-linked memory under compatible `Gxx`),
portamento target, vibrato/tremolo/panbrello state, pattern-loop and delay state, the
retrig, tremor and arpeggio counters, the `S7x` past-note flags, and the active `SFx`
macro.

- **Instrument mode versus sample mode** (`ItFormatExtra::is_instrument_mode`): in sample
  mode the instrument column names a sample and there are no envelopes, NNA or maps.
- **Note handling** per OpenMPT's `InstrumentChange`/`NoteChange`: the note→sample and
  transpose maps, `NOTE_OFF` (release envelopes, start fadeout when no volume envelope or
  the envelope is past its loop), `NOTE_CUT`, `NOTE_FADE`, an instrument without a note
  (IT re-applies volume and pan without retriggering — `it_ins_199`, `it_instrument_memory_default`),
  invalid instrument numbers (`it_cut_invalid_ins`), the empty-slot behaviour
  (`openmpt/it/emptyslot`).
- **Pitch**: linear (`ModuleFlags::linear_slides`) through `LINEAR_FREQUENCY_TABLE` and
  the slide tables, or Amiga periods; `C5Speed` as the reference rate; auto-vibrato per
  sample; pitch envelope; `Step::from_ratio`. Record the representation gap against
  libxmp's period domain as a policy entry if the corpus shows it.
- **Volume**: sample volume × sample global volume × instrument global volume × channel
  volume × global volume × envelope × fadeout × random variation; **pan**: sample/instrument
  defaults, pitch-pan separation, pan envelope, random variation, surround treated as
  centre (policy entry).
- `flush` writes: trigger through `trigger_channel` with a `VoiceTag` whose `sample` is
  the global sample number; then per-tick `params` writes for every owned voice.

### 3. New Note Actions, duplicate checks, voice stealing

- On a new note in instrument mode: evaluate the **previous** voice's NNA (`Cut` releases;
  `Continue` / `NoteOff` / `NoteFade` call `context.detach_channel` then mark the entry's
  state accordingly), then **DCT/DCA** across every voice with the same root channel
  (`VoiceTag.channel`) and matching note / sample / instrument, then trigger the new voice.
  `S7x` (`S70`–`S72` past-note cut/off/fade, `S73`–`S76` NNA override, `S77`–`S7C`
  envelope on/off) acts on the channel's background voices.
- Background voices run only their own envelopes, fadeout and auto-vibrato; a fadeout that
  reaches zero, or a finished one-shot, ends them (`stop_channel` is for foreground; use
  `voice.stop()` via `get_mut` for background).
- **Voice stealing**: when `trigger_channel` returns `None` (pool full), choose a victim
  by OpenMPT's `GetNNAChannel` rule — settle **Q3** by reading it: the lowest-volume
  background voice, preferring fading ones, never the foreground of another channel unless
  nothing else exists — `release` it, retry the trigger, and record the answer in
  architecture §5.2 and §12. A concrete policy in `starplayer-it`, no trait.

### 4. The effect set and the volume column

Effect by effect from `ITTECH.TXT` and OpenMPT, tick-zero and per-tick, with the shared
parameter memory: `Axx` speed, `Bxx`/`Cxx` jump and break, `Dxy` volume slide (`DFx`,
`DxF`, the `D0F`/`DF0` ambiguity as IT resolves it), `Exx`/`Fxx` portamento with `EEx`/`EFx`
fine and extra-fine, `Gxx` tone portamento (compatible-`Gxx` memory; OpenMPT's
`kITPortaTargetReached`, `it_portamento_*` fixtures), `Hxy`/`Uxy` vibrato with IT's tables
and the old-effects flag, `Ixy` tremor, `Jxy` arpeggio, `Kxy`/`Lxy`, `Mxx`/`Nxy` channel
volume, `Oxx` offset with `SAy` high offset (`it_high_offset_memory*`), `Pxy` pan slide,
`Qxy` retrigger, `Rxy` tremolo, **`Sxx`** (`S1x` glissando, `S2x` finetune, `S3x`–`S5x`
waveforms, `S6x` fine pattern delay, `S7x`, `S8x` pan, `S9x` sound control (surround),
`SAy`, `SBx` pattern loop through `PatternFlowState` with IT's flow semantics — research
point 3, `SCx` note cut, `SDx` note delay, `SEx` pattern delay and the `SDx`×`SEx`
interaction, `SFx` macro select), `Txx` tempo and **tempo slides** (`T0x`/`T1x`),
`Vxx`/`Wxy` global volume, `Xxx` pan, `Yxy` panbrello (its own waveform and RNG use),
**`Zxx`** MIDI macros: parse the `format_data` macro block (or the defaults when absent),
apply cutoff (`F0F000z`) and resonance (`F0F001z`) to the voice's `FilterParams`, and
report anything else. The **volume column**: set volume/pan, fine and coarse slides,
portamento (`ItVolumeCommand`'s ranges), vibrato depth, the OpenMPT offset extension.
`report_effect` with `EffectNames::IT`.

**`TempoModel::ItModern`** is filled in: IT and OpenMPT-classic compute whole frames per
tick as `rate · 5 / (2 · bpm)` truncated, with tempo slides applying per tick — research
point 4 decides whether the canonical profile keeps `ExactFixedPoint` (drift-free, the
project default) or truncates; either way `ItModern` gets real semantics, the pinning test
changes, and the conformance adapter's `tick_end_frames` uses the case's model.

### 5. The corpus loop

Run `cargo xtask conformance --offline`. Every IT case ends passing, excluded with a
policy entry (D64–D69, D75+) naming both sources, or in `known-failures.md` with a
`G3-IT-NNN` id for G6. Run every `data/*.it` through `scan_song` and the allocator hook
(extend `render_allocation.rs`'s format list). Dense real-world behaviour: render the
three `data/m/*.it` modules for 30 s with the allocator hook and no engine warning, and
report the peak voice count.

### 6. Goldens, determinism, docs

`cargo xtask goldens` for the synthetic IT; the block-size determinism test gains an IT
render with NNA active; `plans/README.md` M6 row; the accuracy policy's IT section; the
architecture document's Q3 answer and §5.1–§5.2 as landed; `plans/engine/M6-master-plan.md`.

## Research points

1. **libxmp's IT dumps.** How `compare_mixer_data` represents NNA background voices (virtual
   channels beyond `mod->chn`, or omitted); the `period` domain for linear-slide files;
   `cutoff`/`resonance` fields (0..255 from 0..127 — fix the `FilterParams` encoding with
   G2's task); `volume ×16` post-envelope. Write it into `conformance/README.md` and, if
   background voices are dumped, extend the adapter to pair trace v2's ` vc=` rows with
   them.
2. **Q3.** OpenMPT's `GetNNAChannel`: quote its rule, implement it, and record it in the
   architecture document (§5.2 and the §12 table).
3. **IT pattern-loop flow.** libxmp's `FLOW_MODE_IT_210`/`IT_214` bits versus the
   `PatternFlow` the engine already executes; add a `FormatDialect`-driven `QuirkSet` field
   only if a corpus case needs the 2.10 variant.
4. **`ItModern`.** IT truncates; the project's canonical tempo model is drift-free.
   Measure both against the corpus (`tick_end_frames`), propose the default, and record it
   as a §2 entry either way — this is the owner's call, so present both numbers.
5. **`S9x` surround and `X` pan on a surround channel**, and `MIDI pitch controller`:
   confirm what OpenMPT does and whether any corpus case observes it.

## Verification

```sh
cargo test -p starplayer-it
cargo test --workspace
cargo xtask conformance --offline                 # IT cases wired; every failure excluded with a reason or in known-failures
cargo xtask conformance --offline --strict        # report the count of known failures
cargo xtask goldens --check                       # existing goldens unchanged; the IT golden added
cargo test -p starplayer-engine --test block_size_determinism
cargo xtask ci --job rt-safety
cargo xtask ci --job no-std-check
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
cargo xtask ci --job trace-zero-cost
cargo xtask wasm && node apps/starplayer-web/test/worklet-harness.mjs
cargo xtask perceptual --corpus target/conformance/corpora/libxmp-*/test-dev/openmpt/it/*.it   # advisory; report the LSD column
```

Report the IT conformance table, the peak voice counts on the `data/m` modules, the exact
commands and results, every policy entry added, the Q3 answer, and anything left for G6.
**Do not commit** — the reviewer commits.

## Out of scope

The filter's audio (G2 — this task only writes the params). MIDI output of macros. `.mptm`.
Changing the S3M/MOD/MTM/XM processors or the shared envelope-free engine design.
