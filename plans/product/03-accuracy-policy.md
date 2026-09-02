# StarPlayer — Accuracy Policy

What "accurate" means per format, what the reference is, how it is proven, and every
place we knowingly differ from the original assembly.

## The rule

For **MOD, S3M and MTM**: `STARPLAY/S3MLIB.ASM` is the primary specification. Where it
deviates from canonical ST3 / ProTracker behaviour through an outright defect —
a buffer overrun, a wrong register, an unbounded index — implement the **canonical**
behaviour and record the deviation in §3 below.

For **XM and IT**: the format specifications and OpenMPT's documented compatibility
behaviour are the reference. The original never supported them.

Behaviour that is *deliberate* in the original, however odd, is reproduced. Behaviour
that is *broken* in the original is not. The distinction is judged by asking: would
Scream Tracker 3 / ProTracker have done this? If yes, keep it. If the original is alone
in doing it and the cause is visibly a coding defect, fix it and write it down here.

## 1. Deliberate quirks that ARE reproduced

These are load-bearing. Removing any of them changes how real modules sound.

| Quirk | Where | Why it stays |
|---|---|---|
| `Dxy`, `Exx` and `Fxx` share one parameter memory (`_VolSlideValue`) | `S_FX_D`/`S_FX_E`/`S_FX_F` | ST3 shares D/E/F memory too |
| `Hxy`, `Rxy` and `Uxy` share `_VibValue` **and** the phase counter `_VibCount` | `S_FX_H`, `M_FX_R`, `M_FX_U` | ST3 behaviour; `S4x` disturbing vibrato phase is a real ST3 artefact |
| `Dxy` fine-slide classification order: `>0F0h` ⇒ fine down; `(xy&0F)==0F and hi!=0` ⇒ fine up; `DF0`/`D0F` fall through to normal | `S_FX_D` | Exactly ST3's classification |
| Per-tick `Dxy`: the **high nibble wins** — slide up if `xy & F0` is non-zero, else down | `M_FX_D` | ST3 behaviour |
| `Cxx` pattern break read as **decimal**: `(xx>>4)*10 + (xx&0F)` | `S_FX_C` | ST3 behaviour |
| `Exx`/`Fxx` slide classification: `<=0DFh` normal ×4; `<=0EFh` extra-fine ×1; else fine ×4 | `S_FX_E`/`S_FX_F` | ST3 behaviour |
| `SDx` note delay implemented by saving and restoring the whole dirty-flag byte | `S_FX_S` / `M_FX_S` | Elegant and correct — the row's latched note simply never reaches the voice |
| `_SpecialValue` survives across rows only when the previous row's command was `Qxy` | `__UpdateTracker` row reset | Retrigger phase must carry across rows |
| `_ActualPeriod` is restored from `_CurrentPeriod` at row start, but `_ActualVol` is **not** | `__UpdateTracker` row reset | Tremolo/tremor volume offsets persist into the next row until something writes volume — ST3 behaviour |
| A new instrument number reloads C2SPD and resets volume even with no note | `__UpdateTracker` | ST3 behaviour |
| A sample volume > 64 is coerced to **0**, not clamped to 64 | `__UpdateTracker` | ST3 behaviour |
| `S3x`/`S4x`: `x >= 3` resets phase and subtracts 4, so 3 and 7 both select the random table | `S_FX_S` | ST3 behaviour |
| Waveform 3 is a **fixed 128-entry table**, not an RNG | `Vib_Rand_Table` | Deterministic; reproducibility depends on it |
| `Txx` clamped to `>= 20h` | `S_FX_T` | ST3 behaviour |
| `Xxx` pan mapping: `pan = xx>>3; if pan >= 10h then pan -= 1; pan &= 0Fh` | `S_FX_X` | ST3 behaviour (0xFF → 15) |
| Portamento-vs-retrigger decided on whether the voice is still sounding | `__UpdateTracker` + driver `_ActiveFlag` | A porta onto a finished one-shot correctly retriggers |
| MOD loop enabled only when loop length > 4 | `ConvertSamps` | Reproduced by C3, but on a wrong attribution: this is the DOS converter's own rule, not a MOD convention. ProTracker loops whenever the repeat length is more than one word (≥ 4 bytes), as libxmp's `loop_size > 1` does, so a 2-word loop is a real loop there and a one-shot here. `plans/engine/M2-task-C3b-protracker-fidelity-repairs.md` moves the loader to `>= 4` |
| MOD LRRL channel panning map (`0,8,9,1,2,10,11,3,…`) | `CHANNELSETTINGS` | The Amiga interleave |
| MOD Amiga-limits flag derived from whether the whole song stays in octaves 3–5 | `ConvertValues` `limitflag` | Clever and correct: extended-range MODs are not clamped |
| MOD/MTM finetune → C2SPD tables (two different orderings: signed nibble for MOD, monotonic for `S2x`) | `C2SPD_Table`, `FineTuneTable` | Both are correct for their context |
| Sample sign conventions: MOD signed (XOR 0x80 on load), S3M and MTM unsigned | `CopySamples` `Temp_XOR` | Format facts |

## 2. Deliberate quirks reproduced only under `quirks-starplayer`

Behaviour that is authentically the original's but that a modern default should not
have. Available behind the `quirks-starplayer` feature / `QuirkSet` profile, off by
default.

| Quirk | Default | Under `quirks-starplayer` |
|---|---|---|
| Tick length `(rate * 10 / bpm) >> 2`, truncating twice — ~1.3 s of drift over a 4-minute song at 130 BPM | `TempoModel::ExactFixedPoint` — `rate * 2.5 / bpm` in Q32.32, accumulated, drift-free | `TempoModel::St3Truncating` |
| MOD and MTM interpreted through an in-memory S3M conversion | Native per-format effect processors | Not offered — see §4 |

## 3. Documented deviations from the reference implementations

Entries D1–D9 are coding defects in the original assembly and are implemented
canonically. Later entries record deliberate determinism/architecture choices or visible
conformance gaps; none may be hidden behind an accuracy claim.

| # | Defect | Where | What we do instead |
|---|---|---|---|
| D1 | `Vib_Pulse_Table` has only **62** entries; phases 62 and 63 read past the end into `Vib_Rand_Table`, returning 105 and 17 instead of 255 | `Vib_Pulse_Table` | A full 64-entry square wave: 32 × 0 then 32 × 255 |
| D2 | Arpeggio applies a **single** octave carry, so a note index reaching 12–14 indexes past the 12-entry `Period_Table` into `Volume_Table` (0, 30832, 35888) | `M_FX_J` | Full carry: `octave += n/12; note = n%12`, clamped to the valid note range |
| D3 | The `Ixy` tremor per-tick handler reads `[edi+_CurrentVol]` while the minor-tick loop passes the channel in `esi`; `edi` is undefined there, so restoring volume can restore garbage | `M_FX_I` | Read the channel's own `current_vol` |
| D4 | The SoundBlaster post-processing index is sign-extended from the 16-bit accumulator and used unbounded as a `PostTable[2048]` index; heavy modules read outside the table | `Mixer_8bitMono` final pass | Not applicable — the retro mixer is out of scope. The modern mixer clamps/soft-limits explicitly |
| D5 | `Vib_Ramp_Table` has an irregular step at indices 32–33 (`-8, -0, 16, 24` — 8 is skipped) and ends at 255 rather than 256 | `Vib_Ramp_Table` | A regular ramp. The endpoint asymmetry (±255) is kept, since that matches ST3's amplitude |
| D6 | `Axx` with `xx == 0` sets speed 0 with no check, which stalls the tick clock | `S_FX_A` | Ignore `A00` (ST3 behaviour), and the engine's zero-advance guard is the backstop |
| D7 | Only the low 16 bits of the 32-bit C2SPD field are read from the S3M sample header | `__UpdateTracker` | Read the full 32-bit field |
| D8 | Only the low 16 bits of the S3M sample header's 24-bit `memseg` parapointer are read (`movzx edx,word ptr [esi+0eh]`), so sample data past the first megabyte of a file is fetched from the wrong offset | `SB_ProcessTracks` 5740, `_GIRQStartVoice` 4774 | Read the full 24 bits: the high byte at `0x0D`, then the word at `0x0E`. Every file in the owner's 1994–96 collection has that high byte zero, so no module the original could play loads differently |
| D9 | Tremolo advances `_VibCount` with `cmp al,64 / jbe`, retaining phase 64 when the sum lands exactly there; the next tick indexes one entry past every 64-entry waveform table | `M_FX_R` 3470–3473 | Reduce the shared vibrato/tremolo phase modulo 64, as the `Hxy` and `Uxy` handlers do and as ST3 requires |
| D10 | ProTracker `EFx` invert-loop rewrites successive bytes in a sample's loop in place; MTM adopts the same effect standard | PT 2.3D `UpdateFunk` / libxmp `test_effect_ef_invert_loop`; MultiTracker format document | Recognise and report `EFx` for MOD and MTM, but leave audio unchanged. StarPlayer's PCM is an immutable `Arc<Module>` shared with the RT mixer; canonical mutation would require a lock, RT allocation, or an unbounded per-voice overlay. Those all violate stronger architecture invariants. This is a known accuracy gap, not an original defect |
| D11 | MOD/MTM waveform selector 3 is random and has no portable canonical seed/sequence; libxmp normally seeds its player RNG from wall-clock time | libxmp `src/lfo.c`, `src/rng.c`; MultiTracker's ProTracker effect standard | Use a fixed-seed, integer-only xorshift32 stream owned by the shared effect core. The waveform remains random-shaped and independent per replay step, while repeated runs and x86/ARM/WASM fixed-point output remain byte-identical. This intentionally chooses reproducibility over matching an unspecified random sequence |
| D12 | ProTracker 1/2 queues an instrument-only or tone-portamento sample swap and changes the DMA sample pointer at the current sample's end or loop boundary | OpenMPT `PortaSmpChange_PT`, `PortaSwapPT`, `PTInstrSwap`, `PTStoppedSwap`, `PTSwapEmpty`, `PTSwapNoLoop` | Apply the new volume and finetune state immediately, but retain the sounding sample until an explicit note or retrigger. **This is a tracked gap, not an accepted deviation.** The boundary event is RT-safe and cheap: one fixed-size `Option<SampleRegion>` pending region per voice, applied by the voice kernel when the position wraps or the one-shot ends — no allocation, no lock, no branch on the hot path when the option is empty. `plans/engine/M2-task-C3b-protracker-fidelity-repairs.md` owns the mixer voice-boundary swap and the MOD/MTM processor use of it; the six cases stay excluded only until it lands |
| D13 | A ProTracker `EDx` note delay greater than the current speed can leak into the next row, under narrow conditions, without restarting the sample | OpenMPT `NoteDelay-NextRow.mod` (documented-only in the pinned libxmp suite) | Clear the delayed-note latch when the next row is read. Reproducing the leak needs cross-row deferred-trigger state and an exact oracle for its continuation rules; the upstream case is not currently a libxmp frame-state gate, so it remains an explicit compatibility gap |
| D14 | libxmp represents ProTracker pitch differently from ProTracker, on the same PAL clock: it derives a finetuned period continuously as `428 · 2^(-(note + finetune/128)/12)` (162.65, 453.45) where PT reads one of the 16 integer finetune tables (163, 453), and its C-4 rate is the rounded integer `8287`, so `8287 × 428 = 3_546_836 Hz` against the exact PAL `3_546_895 Hz` (17 ppm) | libxmp `src/loaders/mod_load.c:1113-1123` — `c4rate` stays `C4_PAL_RATE` for the ProTracker/OpenMPT tracker ids, NTSC being chosen only for ScreamTracker3/FastTracker/TakeTracker/ModsGrave or `chn > 4`; `src/load_helpers.c:302`; `src/common.h:137`; PT 2.3D period tables | Keep PT's integer finetune tables and the exact PAL `3_546_895 Hz / period` clock. Both sides are PAL — every affected fixture is a 4-channel `M.K.` file, and its dump advances at PAL speed — so the residual is representational: about one source frame of position drift per tick on finetuned samples (`openmpt-mod-finetune`, `openmpt-mod-amiga-limits-finetune`, `openmpt-mod-instrument-swap`), and a 17 ppm accumulation that crosses libxmp's one-integer-sample bound only at the final compared tick of `openmpt-mod-instrument-volume`. Those four cases stay excluded; native playback is not retuned to a rounded secondary software-mixer oracle |
| D15 | libxmp's default MOD replay applies an `Fxx >= 32` tempo to the interval beginning at command tick zero; PT writes the CIA latch on that tick and the new interval begins at the following interrupt | libxmp `DelayBreak` / `VibratoReset`; PT 2.3D `setSpeed` and CIA update path | Keep PT's old-duration command interval and commit the new BPM at the next tracker event. The two mixer dumps whose first timestamp assumes libxmp timing are explicit conformance exclusions |
| D16 | libxmp's out-of-range MOD arpeggio path silences the voice when it crosses the supported Amiga range; PT indexes its physically flat period table and can reach its zero sentinel without writing channel volume | OpenMPT `ArpeggioWraparound.mod`; PT 2.3D arpeggio table access | Preserve PT's table-domain result, including period zero with unchanged channel volume. The libxmp volume-zero expectation is an explicit conformance exclusion rather than a reason to invent a volume command. Real Paula, as modelled by pt2-clone's `paulaSetPeriod`, additionally clamps any period below 113 to 113 and treats period 0 as 65536 — a near-silent ~54 Hz crawl, not a frozen DC hold. As of C3 the step derivation floors the period at 1 with Amiga limits off and maps period 0 to a zero step; `plans/engine/M2-task-C3b-protracker-fidelity-repairs.md` applies the hardware floor and the 65536 rule at step derivation, which is also what keeps an extended-range MOD sliding below 113 from producing a runaway step |
| D17 | The default OpenMPT/libxmp `PortaSmpChange.mod` oracle deliberately selects a non-ProTracker sample-change compatibility mode | OpenMPT `PortaSmpChange.mod` and `PortaSmpChange_PT.mod` | Offer the native ProTracker 1/2 interpretation only. The default-profile case is excluded; the `_PT` case separately exercises the queued boundary swap recorded by D12 |
| D18 | libxmp's mixer-state test observes channels after `xmp_play_frame` has rendered the tick and omits a voice marked `NOTE_SAMPLE_END`; C1 traces format state at the exact event boundary before that interval is rendered | libxmp `test_player_mtm_tempo` / `compare_mixer_data.c`; C1 trace contract | Keep the event-boundary trace, where the `TEMPO.MTM` one-shot is still active at row 12 tick 4 and ends inside the following interval. The executed case is excluded rather than changing the trace contract or hiding active-set mismatches globally |
| D19 | libxmp truncates every mixer tick to `(int)(rate * 2.5 / bpm)`, discarding the fractional remainder; at high MTM tempos its integer source position differs by two frames by tick 53 | libxmp `src/mixer.c:libxmp_mixer_get_ticksize`; `TEMPO2.MTM` | Keep `TempoModel::ExactFixedPoint`, which carries the rational remainder and is required for sample-exact, drift-free timing. The adapter retains libxmp's one-frame source-position bound and the executed case is excluded instead of loosening it |
| D20 | ProTracker's `mt_Tremolo2` selects which half of the ramp waveform to read by testing `n_vibratopos` — the *vibrato* phase — instead of `n_tremolopos`, so an `E71` tremolo takes its ramp shape from the vibrato phase while its sign comes from the tremolo phase | PT 2.3D `mt_Tremolo2`; pt2-clone `tremolo` | Read the tremolo ramp from the tremolo phase, which is the canonical shape and also what libxmp does, so no corpus oracle can observe the bug either way. PT's cross-wired phase is reproduced as a `QuirkSet` field delivered by `plans/engine/M2-task-C5-quirks-and-tempo-models.md`, off under `canonical()` |

D1, D2 and D5 change audible output on modules that use those waveforms or wide
arpeggios. That is intended: the canonical behaviour is what Scream Tracker 3 produced,
and matching it is what the "most accurate S3M playback" claim actually means.

**Tracker dialects are not deviations and are not refusals.** Where a corpus case encodes
another tracker's flow semantics rather than a disagreement about the reference — the S3M
pattern-loop modes libxmp selects from the `cwtv` field (`src/loaders/s3m_load.c:390-432`:
`0x1301` ST3.01, `0x1303` ST3.21, `0x1320` ModPlug 1.16, `0x2100` Imago Orpheus), or the
MOD tracker variants behind the `CD61` Octalyser and `FA04` / `FA06` Digital Tracker
signatures — the answer is a `FormatDialect` detected from the file header and mapped to
`QuirkSet` fields. That work is `plans/engine/M2-task-C5-quirks-and-tempo-models.md`, and
each dialect field must flip exactly the corpus cases that name it while leaving every
other result unchanged under `canonical()`. Those cases are therefore neither §3
deviations nor §4 "not offered"; their exclusions cite the C5 task file as tracking until
it lands.

## 4. Not offered at all

- **Retro mixer emulation.** The original's 8-bit unsigned mono SoundBlaster mixer, its
  65×256 volume lookup table, and its master-volume-derived `PostTable` soft-clip curve
  are documented in `plans/reference/original-s3mlib-analysis.md` but not implemented.
- **MOD/MTM via S3M conversion.** Not offered even as a compatibility mode. It destroys
  format identity before the player sees it and is a dead end once XM/IT arrive.
- **Tagless 15-sample Soundtracker MOD.** Its header offsets, loop units and effect/tempo
  dialect differ from tagged 31-sample ProTracker files; C3 rejects it explicitly.
- **Startrekker AM synthesis.** The envelope and oscillator parameters live in a sibling
  `.NT` / `.AS` file which the byte-slice facade does not receive. Ordinary `FLT4` PCM
  and paired-pattern `FLT8` modules are supported; AM-synth fixtures are not.
- **GUS emulation.** The GUS driver's voice-count-adaptive `Divisor_Table` output rate
  and its deferred ramp-then-start note trigger are recorded as history. The modern
  mixer has no equivalent constraint.

## 5. How accuracy is proven

Ordered by value per unit of effort. Full detail in `02-roadmap.md`.

1. **Block-size determinism** — render at host block sizes 1, 3, 64, 128, 4096, 8191;
   assert byte-identical. Exists from M0.
2. **Per-tick state traces** — one line per channel per tick (note, instrument, volume,
   period, pan, sample position, dirty flags) behind `feature = "trace"`, diffable.
3. **libxmp's `test-dev/` suite** — hundreds of purpose-built one-behaviour-per-module
   files *with frame-by-frame expected channel-state dumps*. Both corpus and oracle.
   Mine it before writing effect code.
4. **OpenMPT's `test_*.{mod,s3m,xm,it}` collection** — each isolates one compatibility
   quirk, with expected behaviour documented on the OpenMPT wiki.
5. **Golden hashes** — SHA-256 of a fixed-point i16 mono 44100 Hz render, linear
   interpolation, DSP bypassed. The config is encoded in the filename so a change of
   interpolator is visibly a new golden rather than a silent break.
6. **Cross-target hash equality** — x86-64, aarch64 and wasm32 must agree bit-for-bit
   on the fixed-point path.
7. **Perceptual comparison vs libopenmpt** on the float path — spectral distance /
   segmental SNR with a tolerance. Nightly, not a gate.

A **contingency**: if a specific effect's behaviour cannot be settled from the assembly,
`plans/engine/M2-task-C8-dos-reference-harness.md` describes reconstructing a buildable
DOS reference and dumping the original's own per-tick `ChannelData`, which is directly
comparable to (2). This is deliberately *not* WAV-diffing DOSBox — its SoundBlaster
emulation resamples, so that would be diffing emulator artefacts.

## 6. Amending this document

When implementation discovers a new deviation, add it to §3 with its cause and the
canonical behaviour chosen, in the same commit as the code. A deviation that is not
written down here is a defect, not a decision.
