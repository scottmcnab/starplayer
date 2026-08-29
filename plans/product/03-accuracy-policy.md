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

## 3. Defects in the original that we deliberately do NOT reproduce

Each is a coding defect, not a format behaviour. Each is implemented canonically.

| # | Defect | Where | What we do instead |
|---|---|---|---|
| D1 | `Vib_Pulse_Table` has only **62** entries; phases 62 and 63 read past the end into `Vib_Rand_Table`, returning 105 and 17 instead of 255 | `Vib_Pulse_Table` | A full 64-entry square wave: 32 × 0 then 32 × 255 |
| D2 | Arpeggio applies a **single** octave carry, so a note index reaching 12–14 indexes past the 12-entry `Period_Table` into `Volume_Table` (0, 30832, 35888) | `M_FX_J` | Full carry: `octave += n/12; note = n%12`, clamped to the valid note range |
| D3 | The `Ixy` tremor per-tick handler reads `[edi+_CurrentVol]` while the minor-tick loop passes the channel in `esi`; `edi` is undefined there, so restoring volume can restore garbage | `M_FX_I` | Read the channel's own `current_vol` |
| D4 | The SoundBlaster post-processing index is sign-extended from the 16-bit accumulator and used unbounded as a `PostTable[2048]` index; heavy modules read outside the table | `Mixer_8bitMono` final pass | Not applicable — the retro mixer is out of scope. The modern mixer clamps/soft-limits explicitly |
| D5 | `Vib_Ramp_Table` has an irregular step at indices 32–33 (`-8, -0, 16, 24` — 8 is skipped) and ends at 255 rather than 256 | `Vib_Ramp_Table` | A regular ramp. The endpoint asymmetry (±255) is kept, since that matches ST3's amplitude |
| D6 | `Axx` with `xx == 0` sets speed 0 with no check, which stalls the tick clock | `S_FX_A` | Ignore `A00` (ST3 behaviour), and the engine's zero-advance guard is the backstop |
| D7 | Only the low 16 bits of the 32-bit C2SPD field are read from the S3M sample header | `__UpdateTracker` | Read the full 32-bit field |

D1, D2 and D5 change audible output on modules that use those waveforms or wide
arpeggios. That is intended: the canonical behaviour is what Scream Tracker 3 produced,
and matching it is what the "most accurate S3M playback" claim actually means.

## 4. Not offered at all

- **Retro mixer emulation.** The original's 8-bit unsigned mono SoundBlaster mixer, its
  65×256 volume lookup table, and its master-volume-derived `PostTable` soft-clip curve
  are documented in `plans/reference/original-s3mlib-analysis.md` but not implemented.
- **MOD/MTM via S3M conversion.** Not offered even as a compatibility mode. It destroys
  format identity before the player sees it and is a dead end once XM/IT arrive.
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
