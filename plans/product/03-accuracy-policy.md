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
| MOD loop enabled only when loop length > 4 | `ConvertSamps` | Universal MOD convention |
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
| D10 | ProTracker `EFx` invert-loop rewrites successive bytes in a sample's loop in place | PT 2.3D `UpdateFunk` / libxmp `test_effect_ef_invert_loop` | Recognise and report `EFx`, but leave audio unchanged. StarPlayer's PCM is an immutable `Arc<Module>` shared with the RT mixer; canonical mutation would require a lock, RT allocation, or an unbounded per-voice overlay. Those all violate stronger architecture invariants. This is a known MOD accuracy gap, not an original defect |
| D11 | MOD waveform selector 3 is random and has no portable canonical seed/sequence; libxmp normally seeds its player RNG from wall-clock time | libxmp `src/lfo.c`, `src/rng.c` | Use a fixed-seed, integer-only xorshift32 stream owned by `ModProcessor`. The waveform remains random-shaped and independent per replay step, while repeated runs and x86/ARM/WASM fixed-point output remain byte-identical. This intentionally chooses reproducibility over matching an unspecified random sequence |
| D12 | ProTracker 1/2 queues an instrument-only or tone-portamento sample swap and changes the DMA sample pointer at the current sample's end or loop boundary | OpenMPT `PortaSmpChange_PT`, `PortaSwapPT`, `PTInstrSwap`, `PTStoppedSwap`, `PTSwapEmpty`, `PTSwapNoLoop` | Apply the new volume and finetune state immediately, but retain the sounding sample until an explicit note or retrigger. The mixer currently has no fixed-size queued region replacement or processor callback at a voice boundary. These cases remain named C2 exclusions until that RT-safe boundary event exists |
| D13 | A ProTracker `EDx` note delay greater than the current speed can leak into the next row, under narrow conditions, without restarting the sample | OpenMPT `NoteDelay-NextRow.mod` (documented-only in the pinned libxmp suite) | Clear the delayed-note latch when the next row is read. Reproducing the leak needs cross-row deferred-trigger state and an exact oracle for its continuation rules; the upstream case is not currently a libxmp frame-state gate, so it remains an explicit compatibility gap |
| D14 | The pinned libxmp MOD loader selects its NTSC `8363` C-4 rate for default software mixing, while the primary ProTracker/Paula path advances samples from the PAL `3_546_895 Hz / period` clock | libxmp `src/loaders/mod_load.c`, `src/common.h`, `src/paula.h`; PT 2.3D replay | Keep the canonical PAL clock. The adapter floors C1's Q32.32 position and applies libxmp's own one-integer-sample bound, but cases whose NTSC position or one-shot lifetime still diverges remain individually excluded. Native playback is not retuned to make a secondary software-mixer oracle pass |
| D15 | libxmp's default MOD replay applies an `Fxx >= 32` tempo to the interval beginning at command tick zero; PT writes the CIA latch on that tick and the new interval begins at the following interrupt | libxmp `DelayBreak` / `VibratoReset`; PT 2.3D `setSpeed` and CIA update path | Keep PT's old-duration command interval and commit the new BPM at the next tracker event. The two mixer dumps whose first timestamp assumes libxmp timing are explicit conformance exclusions |
| D16 | libxmp's out-of-range MOD arpeggio path silences the voice when it crosses the supported Amiga range; PT indexes its physically flat period table and can reach its zero sentinel without writing channel volume | OpenMPT `ArpeggioWraparound.mod`; PT 2.3D arpeggio table access | Preserve PT's table-domain result, including period zero with unchanged channel volume. The libxmp volume-zero expectation is an explicit conformance exclusion rather than a reason to invent a volume command |
| D17 | The default OpenMPT/libxmp `PortaSmpChange.mod` oracle deliberately selects a non-ProTracker sample-change compatibility mode | OpenMPT `PortaSmpChange.mod` and `PortaSmpChange_PT.mod` | Offer the native ProTracker 1/2 interpretation only. The default-profile case is excluded; the `_PT` case separately exercises the queued boundary swap recorded by D12 |

D1, D2 and D5 change audible output on modules that use those waveforms or wide
arpeggios. That is intended: the canonical behaviour is what Scream Tracker 3 produced,
and matching it is what the "most accurate S3M playback" claim actually means.

## 4. Not offered at all

- **Retro mixer emulation.** The original's 8-bit unsigned mono SoundBlaster mixer, its
  65×256 volume lookup table, and its master-volume-derived `PostTable` soft-clip curve
  are documented in `plans/reference/original-s3mlib-analysis.md` but not implemented.
- **MOD/MTM via S3M conversion.** Not offered even as a compatibility mode. It destroys
  format identity before the player sees it and is a dead end once XM/IT arrive.
- **Tagless 15-sample Soundtracker MOD.** Its header offsets, loop units and effect/tempo
  dialect differ from tagged 31-sample ProTracker files; C3 rejects it explicitly.
- **Octalyser / Digital Tracker MOD dialects.** `CD61` and `FA04` / `FA06` are outside
  C3's accepted signature set and carry different pattern-loop behaviour.
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
