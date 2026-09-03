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

## Research resolution

### 1. libxmp's IT dumps — **verdict: background voices *are* dumped, as virtual channels at or above `mod->chn`, and the adapter reproduces libxmp's own numbering**

`test-dev/gen_mixer_data.c` and `compare_mixer_data.c` both loop
`for (i = 0; i < max_channels; i++)` with `max_channels = p->virt.virt_channels`, and
`virt_channels = num_tracks + libxmp_mixer_numvoices(ctx, -1)` whenever the format has
`QUIRK_VIRTUAL` — which IT does. So every column of the dump is a **virtual** channel, and
an NNA background voice appears under an index at or above the module's channel count.

The numbering is not arbitrary. `libxmp_virt_setpatch` (`src/virtual.c:517-523`) moves the
*displaced* voice to the lowest free virtual channel at or above `num_tracks`:

```c
for (chn = p->virt.num_tracks; chn < p->virt.virt_channels &&
     p->virt.virt_channel[chn++].map > FREE;) ;
p->virt.voice_array[voc].chn = --chn;
p->virt.virt_channel[chn].map = voc;
```

and `libxmp_virt_resetvoice` frees the slot again. That rule is reproducible from trace v2
alone, so `background_virtual_channels` in `crates/starplayer-testkit/src/conformance.rs`
assigns each ` vc=` row the lowest free index at or above the channel count, holds it while
the voice is alive and releases it when the voice goes. The projection then folds those
rows into the same channel axis the ` ch=` rows use, and every oracle column — background
ones included — is enforced. Both players walk channels in ascending order and both hand
out the lowest free slot, so two detachments in one tick are numbered the same way.

**The period domain.** libxmp dumps `xc->info_period`, which is
`MIN(final_period * 4096, INT_MAX)` with
`final_period = libxmp_note_to_period_mix(note, bend) = PERIOD_BASE / 2^((note + bend/12800)/12)`
and `PERIOD_BASE = 13696.0` (`src/period.h:6`). It is a **period** even for a linear-slide
file: libxmp keeps IT's pitch as a note plus a bend and only converts at the end, where
StarPlayer — like IT itself — keeps a frequency. The two are related by libxmp's own mixer
step, `step = C4_PERIOD * c5spd / rate / period` with `C4_PERIOD = 428`, so

> `info_period = 4096 · 428 · C5Speed / frequency`

The IT processor traces exactly `428 · C5Speed / frequency` — libxmp's whole period — and
`project_period` divides the oracle's Q12 column by 4096, the same projection MOD and MTM
use, with the existing tolerance of one whole period. Recorded as accuracy-policy entry
**D64**: the comparison axis is a whole libxmp period, so a pitch difference smaller than
one period at the sounding note is below the oracle's resolution. That is about 0.23 % at
C-5 and 1.9 % at C-8; IT's own finest slide step is 1/64 semitone (0.09 %), so the axis
catches note, octave, direction and accumulated-drift errors but not a single fine-slide
unit at the top of the keyboard. Tightening it is G6's call, not a silent choice here.

**`cutoff` / `resonance`.** libxmp stores IT's 0..127 file values doubled:
`apply_midi_macro_effect` does `xc->filter.cutoff = val << 1` and
`xc->filter.resonance = val << 1`, the channel default is `0xFF`, the filter envelope
scales it as `cutoff * envelope >> 8`, and `vi->filter.*` is only written while the filter
is actually engaged — so a voice that never engaged one dumps the `calloc` zero. This fixes
the `FilterParams` encoding jointly with G2 (its research point 1): `FilterParams::from_it`
encodes **`value · 2 · 257`**, because `255 · 257 == 65535` exactly. The C1 trace's
`unit_to_scale(bits, 255)` then reads back libxmp's 0..255 column with no rounding at all,
and `FilterParams::to_it` — what G2's coefficient code takes — reads back IT's 0..127 pair
exactly. `from_it_scaled` takes an already-envelope-scaled 0..255 cutoff for the same
encoding. G2's task file proposed `cutoff · 516`; that round-trips the 0..127 domain but
disagrees with the oracle's 0..255 column at 63 of its 128 values (cutoff 64 reads 129
where libxmp says 128), so the agreed encoding is `· 2 · 257`. `is_bypass()` is IT's own
rule — full cutoff with no resonance is not a filter at all — so `from_it(127, 0)` is
`BYPASS` and traces as 255, which is what the adapter's `cutoff == 0 → 255` sentinel
already expects. The projection now also mirrors libxmp's own comparator, which accepts any
two values at or above 254 as the same fully-open cutoff.

**`volume ×16`.** libxmp's `vi->vol` is `finalvol`, whose maximum is 1024 — sixteen times
IT's 0..64 scale, and one sixteenth of OpenMPT's 14-bit `nRealVolume`. The existing
`(volume_x16 + 8) / 16` projection is therefore already the IT projection, and the trace's
`report_channel` caps `volume` at 64 regardless, so 0..64 is the axis for every format.

**`pan`.** libxmp dumps `vi->pan`, which is `finalpan - 0x80` in −128..127 — *or*
`PAN_SURROUND` (`0x8000`) for a channel `S91` put into surround (`src/player.c:1408`,
`src/mixer.h:23`). 48 lines of `SmpInsPanSurround.data` carry it, and the old
`parse_libxmp_dump` rejected the file outright. The column is now `i32`, the sentinel is
accepted, and `project_pan` maps it to centre — which is what StarPlayer renders for a
surround voice (accuracy-policy entry **D66**). Every other IT pan compares as a full byte,
since IT's 0..64 pan is scaled by four inside the replayer rather than shifted into a
nibble as S3M's is.

**The manifest.** `audit_pinned_corpus` insists the case manifest is exactly the set of
`compare_mixer_data*` pairs in the pinned tree for every target extension, so adding `it`
to `matches_target_extension` brings in **121** IT cases, not the 59 `openmpt/it` ones the
task named: 59 `openmpt/it` fixtures plus 62 `data/*.it` ones from `test_effect_it_*`,
`test_player_it_*`, `test_storlek_*` and two fuzzer cases. All 121 are in `cases.tsv`.

### 2. Q3 — OpenMPT's `GetNNAChannel` — **verdict: lowest free background voice first; otherwise the quietest background voice, halved for a looped sample, and never quieter than the note that wants to steal it**

`CSoundFile::GetNNAChannel` (`soundlib/Snd_fx.cpp:2257`) is two passes over the background
range `[GetNumChannels(), Chn.size())` only — a foreground voice of another channel is
never a candidate:

1. **A free voice wins outright**, taking the lowest index: `if(c.nLength) continue;` then
   `return i`.
2. Otherwise a score `v = (nRealVolume << 9) | nVolume` — the 14-bit post-envelope,
   post-fadeout mixing volume with the 0..256 note volume as a tie-breaker — is computed
   for every background voice. `if(c.dwFlags[CHN_LOOP]) v /= 2;` gives a looped sample half
   priority, because it will ring forever otherwise. A voice that is *playing but fully
   faded* (`c.nLength && !c.nFadeOutVol`) is returned immediately. The lowest score wins,
   and on a tie the voice further through its volume envelope — or with no volume envelope
   at all — wins.
3. The threshold starts at the **stealing note's own score**
   (`vol = (srcChn.nRealVolume << 9) | srcChn.nVolume`), so a background voice louder than
   the note that wants its slot is never stolen and the new note simply does not sound.
   And if the source channel is itself already fully faded, `CHANNELINDEX_INVALID` is
   returned before the search: nothing is allocated and the old voice is just dropped.

Schism (`player/effects.c:1640`) agrees on the shape and differs in two details: it folds
the fadeout into the score explicitly (`v = volume * fadeout_volume` for a fading voice,
`volume << 16` otherwise) instead of relying on `nRealVolume`, and it uses a fixed 25 %
threshold rather than the stealing note's own score.

`ItProcessor::steal_background_voice` implements OpenMPT's rule, with Schism's explicit
fadeout term folded in because StarPlayer's per-voice volume is recomputed from the
articulation state each tick rather than cached as a 14-bit mixing volume: the score is
`(voice_volume_14bit << 9) | note_volume`, halved for a looping region, and a voice whose
fadeout has reached zero is taken at once. The answer is recorded in
`plans/product/01-technical-architecture.md` §5.2 and the §12 open-question table.

### 3. IT pattern-loop flow — **verdict: four `Cwt/v`-gated profiles, and a `QuirkSet` field was needed after all**

it2play's `InitCommandS` (`it_m_eff.c`, a direct port of IT2's own replayer) is the
behaviour to reproduce:

```c
if (val == 0)                    hc->PattLoopStartRow = Song.CurrentRow;
else if (hc->PattLoopCount == 0) { hc->PattLoopCount = val; Song.ProcessRow = hc->PattLoopStartRow - 1; Song.PatternLooping = true; }
else if (--hc->PattLoopCount)    { Song.ProcessRow = hc->PattLoopStartRow - 1; Song.PatternLooping = true; }
else                             hc->PattLoopStartRow = Song.CurrentRow + 1;
```

— a **per-channel** target and counter, independent across channels, with the target
advancing past the `SBx` row when the count runs out. `Cxx` is suppressed while a loop is
running (`if (!Song.PatternLooping)` guards the break) and a pattern break does not reset
the counters.

The task file's premise of a documented **2.10 versus 2.14** split is not corroborated by
anything found: OpenMPT gates its pattern-loop behaviours on *OpenMPT* version rather than
on IT's, and Schism's own abuse-test page describes a single behaviour. But the premise
was right that a `QuirkSet` field is needed, because libxmp gates on `Cwt/v` at a
*different* boundary — `src/loaders/it_load.c:394-400`:

```c
if (ifh->cwt < 0x104)      m->flow_mode = FLOW_MODE_IT_100;
else if (ifh->cwt < 0x200) m->flow_mode = FLOW_MODE_IT_104;
else if (ifh->cwt < 0x210) m->flow_mode = FLOW_MODE_IT_200;
/* else the 0x351 default: FLOW_MODE_IT_210 */
```

with `FLOW_MODE_IT_100 = GLOBAL | UNSET_BREAK | UNSET_JUMP | JUMP_NO_ROW_SET`,
`IT_104` the same without `GLOBAL`, `IT_200 = IT_104 | DELAY_BREAK`, and
`IT_210 = IT_200 | END_ADVANCES` (`src/common.h:380-386`). The pinned corpus carries one
fixture per profile — `pattern_loop_it100.it`, `pattern_loop_it104.it`,
`pattern_loop_it200_breakjump.it`, `pattern_loop_it210.it` — which is exactly the corpus
evidence C5's rule demands before a field may exist.

So this task adds `ItLoopDialect` to `QuirkSet` with the four profiles, three new
`FormatDialect` variants (`ImpulseTracker200`, `ImpulseTracker104`, `ImpulseTracker100`)
for the early `Cwt/v` bands, and `ItLoopDialect::flow()` mapping each onto the
`PatternFlow` the engine's `PatternFlowState` already executes. Every clone — Schism,
OpenMPT, ModPlug — takes the 2.10 baseline, exactly as libxmp does
(`it_load.c:351` sets `FLOW_MODE_IT_210` before the `Cwt/v` switch narrows it, and only
the "Impulse Tracker" branch narrows it). Recorded as accuracy-policy entry **D65**, with
`the_it_flow_tables_match_libxmps_flow_mode_constants` asserting the four profiles field
by field against libxmp's constants. The IT loader's `dialect()` was extended to return
the finer variants, which changed one committed fuzz-seed expectation
(`old-instruments.it` carries a pre-2.00 `Cwt/v`).

### 4. `ItModern` — **verdict: Impulse Tracker truncates, the IT dialects select it, and the conformance adapter keeps pairing on the oracle's own exact clock**

`ItModern` was a stub equal to `ExactFixedPoint`. It is now
`(sample_rate · 5) / (2 · bpm)` truncated **once** to a whole output frame, with no
remainder carried — which is what Impulse Tracker's driver does (it reloads a whole-sample
gap length every tick), what libxmp does (`src/mixer.c:440`, `ticksize = (int)calc`) and
what OpenMPT's classic path does. It is *not* `St3Truncating`, which truncates twice
(`(rate * 10 / bpm) >> 2`); the two agree wherever the quotient is whole and differ by up
to three frames elsewhere. The pinning test
`it_modern_is_currently_exact_fixed_point` is replaced by one asserting the truncation.

**Both numbers, measured on the pinned corpus at the same commit and with every other
change in place.** Under `ExactFixedPoint` the IT set gave **10 passes** with 59 cases
whose first divergence is `position`; under `ItModern` it gives **13 passes** with the
same category shrinking, and after the two harness projections below it reaches **39**
(13 with every field enforced, 26 with `position` waived under **D67**). The reason the
tempo model shows up as a *position* difference at all is that a voice's sample position
after N ticks is `step · Σ frames_per_tick`, so a fractional tick length that the engine
carries and the oracle does not walks the two apart on every module whose BPM does not
give a whole number of frames per tick.

**The recommendation is `ItModern`, for IT modules only**, selected by
`FormatDialect::ImpulseTracker*` through `QuirkSet::impulse_tracker()`. MOD, MTM and S3M
keep `ExactFixedPoint`; the project's drift-free default is unchanged for every format
whose own program did not truncate. The argument is that for IT the exact clock is not a
more accurate reading of the same behaviour — it is a different behaviour, audible as a
different sample position — where for MOD and S3M the truncation is a defect of one
*player* (the 1990s StarPlayer) rather than of the format, which is why §2 keeps it behind
`quirks-starplayer` there. **This is the owner's call to confirm**; both numbers are above
and the change is one line of `QuirkSet::impulse_tracker()`.

One consequence for the harness, and it is worth stating because it is not obvious:
**libxmp keeps two clocks that disagree with each other.** The number of frames it renders
per tick is truncated, but the `time` column every dump record carries accumulates the
exact `time_factor · rrate / bpm` in a `double` (`src/player.c:2171`). The dump's
timestamps therefore lie on the *exact* timeline and its sample positions on the
*truncated* one. `tick_end_frames` pairs records by timestamp, so it models libxmp and
keeps `ExactFixedPoint` for every format including IT; the engine renders on the truncated
model. Pairing IT on `ItModern` was tried first and pushed the accumulated frame offset
past the harness's own 45-frame pairing tolerance on traces of eighty ticks or more.

### 5. `S9x` surround, `Xxx` on a surround channel, and the MIDI pitch controller — **verdict: surround renders as centre (D66); `Xxx` and `S8x` cancel surround; the MIDI pitch controller is out of scope and no corpus case observes it**

* **`S91` turns surround on, `S90` off.** `S90` is a ModPlug extension — it2play's
  `case 0x90` only acts on `val == 1` — but implementing both is harmless and matches
  every modern replayer. `S9A`/`S9B` (song-global surround mode) and `S9C`–`S9F` are
  ModPlug extensions; `S9E`/`S9F` (play forward/backward) are not implemented here and are
  reported through `report_effect` only.
* **`Xxx` and `S8x` cancel surround.** it2play's `InitCommandX2` writes `sc->Pan` and
  `sc->PanSet` unconditionally, overwriting the `PAN_SURROUND` sentinel (100); OpenMPT
  spells the same rule `kITNoSurroundPan` — "Panning and surround are mutually exclusive" —
  and `kPanOverride` additionally zeroes the pan swing and the panbrello offset. A sample's
  or instrument's own panning also cancels it (`SmpInsPanSurround.it`), while `Pxy` is a
  **no-op** on a surround channel in IT2 (OpenMPT slides anyway — a deviation recorded as
  **D68**, resolved in IT2's favour).
* **Surround itself is rendered as centre**, accuracy-policy entry **D66**: StarPlayer's
  voice model carries one `pan: I1F15` and the mixer has no phase-inverted rear channel, so
  the Dolby Pro-Logic trick IT's software mixer plays (`newRightVol = -newRightVol`) has
  nowhere to live until the DSP graph grows a surround bus in M7. `SmpInsPanSurround.it` is
  the corpus case that observes it, and the adapter projects libxmp's `PAN_SURROUND`
  sentinel onto centre so the rest of that trace stays enforced.
* **MIDI pitch controller** (`Flags` bit 6, depth `PWD`) only matters for a channel routed
  to a MIDI instrument. This engine has no MIDI output — the task puts it out of scope —
  and no `openmpt/it` or `data/*.it` fixture in the pinned corpus uses a MIDI instrument, so
  nothing observes it. `ItFormatExtra::uses_midi_pitch_controller` stays unread.

## Landing notes

### What deviates from the task file, and why

* **121 corpus cases, not 59.** `audit_pinned_corpus` insists the manifest is exactly the
  set of `compare_mixer_data*` pairs in the pinned tree for every target extension, so
  adding `it` to `matches_target_extension` brings the 62 `data/*.it` pairs in with the 59
  `openmpt/it` ones. Wiring only the 59 would have made the audit fail.
* **The voice-stealing policy is not a trait** (master-plan deliverable 3 said "policy
  trait"). Design goal 8 keeps a trait uncommitted until its second real implementation,
  and XM's allocator is the same heuristic with a different New Note Action set.
  `ItProcessor::choose_victim` is a concrete policy in `starplayer-it`.
* **A `QuirkSet` field arrived that the task file did not anticipate**, and one it
  anticipated arrived differently. `it_pattern_loop` (research point 3) is named by four
  corpus fixtures, so C5's rule permits it; `tempo_model` on the IT dialects is research
  point 4's answer and is the owner's to confirm.
* **`S2x` set finetune is deliberately not implemented.** It was removed in Impulse
  Tracker 2 (`it2play` falls through to the no-op default; Schism's comment says "no longer
  implemented"). OpenMPT still implements it for S3M compatibility.
* **Envelope carry's Compatible-Gxx quirk is not implemented.**
  `kITCompatGxxCarryPortaWithIns` reads the envelope position back from a global
  "last moved NNA channel". `ItProcessor` tracks `last_moved_voice` but does not read it;
  the case that needs it (`CarryCompatGxxPortaWithIns.it`) has no oracle in the pinned
  corpus. Left for G6.
* **Ping-pong loop shortening is not implemented.** IT's software mixer plays a ping-pong
  loop one sample short of the file's; `openmpt-it-bidi-loops` is the fixture for it and is
  recorded under `G3-IT-008`.

### Overlap with G2 (the resonant filter), which landed on `main` separately

Both tasks needed `FilterParams::from_it` / `to_it`, and research point 1 here settled the
encoding jointly. **The two agree**: this branch encodes `value · 2 · 257`, which is
`value · 514`, and G2 landed `514×`. The merge will conflict textually in
`crates/starplayer-core/src/event.rs` — **keep G2's `from_it` / `to_it`** and take this
branch's extra `from_it_scaled(cutoff_0_255, resonance_0_127)`, which is the form the
filter envelope needs (IT scales the cutoff by the envelope *before* the coefficients are
derived, so the processor has a 0..255 cutoff in hand rather than a 0..127 one). This
branch's `FilterParams::IT_SCALE`, `saturating_double` and `scale_to_it` helpers exist only
to support those three functions and can go with whichever copy loses.

G2 also added `Voice::filter_mut().set_extended_range(..)`, which nothing sets. **This
branch cannot call it** — the method does not exist here — but the value it wants is
already read: `ItProcessor::has_extended_filter_range()` returns the module's `Flags` bit
`0x1000`. Wiring the one call is a post-merge line for the reviewer or for G6.
