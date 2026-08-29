# Reference — the original `S3MLIB.ASM` replay engine

Archaeology of `STARPLAY/S3MLIB.ASM` (6,317 lines) and `STARPLAY/S3MLIB.INC` (153
lines): the reusable S3M/MOD/MTM replay library from StarPlayer 2.25, 80386 TASM
assembly, ~1994–96. It identifies itself internally as
`-+ oxyplay +- music library (c) scott mcnab 1994/95 (jedi / oxygen)`.

This document is the **specification** for MOD/S3M/MTM effect semantics in the Rust
engine. Where it and `plans/product/03-accuracy-policy.md` disagree about what to
implement, the accuracy policy wins — it records the deliberate deviations.

`STARPLAY/` is read-only. Never modify it.

---

## 0. The source is gutted

Almost the entire engine sits inside TASM `comment %` … `%` blocks and therefore does
not assemble. Only `S3MLIB.OBJ` (2,685 bytes) was ever shipped from it.

| Region | Lines | Contents |
|---|---|---|
| small blocks | 595–614, 620–633, 655–701, 708–730, 736–770, 778–803, 813–850, 861–944 | bodies of most `PM_*` API functions |
| big block 1 | 949–2267 | `GetFileType`, `LoadModFile`, the MOD converter, the MTM converter, `ParseModule`, `FindSampsSize` |
| big block 2 | 2370–3832 | **the whole tracker**: `__UpdateTracker`, `ClipPitch`, all `S_FX_*` / `M_FX_*`, `PeriodFromNote`, `PeriodToPitch`, `ClearChannels`, `LoadPanSettings`, `PtrToSample`, DMA helpers |
| big block 3 | 3838–4155 | GUS interface, init, start-song, timer |
| big block 4 | 4234–6314 | GUS IRQ + voice code, DRAM dump, `UltraReset`/peek/poke, **all** SoundBlaster code |

Live code: `PM_SetLoopCode`, `SetAllVol`, parts of `PM_InitSystem`/`PM_CloseSystem`,
`GUS_Read_Env`, `UltraMix*`, `GF1_Delay`, `UltraPing/Peek/Poke/DRAMSize`.

**The text is complete and readable**, so the sources are fully usable as a
specification — they are just not currently buildable. The shipped `STAR.EXE` was built
from this same stripped source and is therefore itself crippled.

Build chain (`P.BAT`, intact): `tasm /ml /m2` × 3 → `wlink @pmodew.lnk system pmodew
file star file cdromlib file s3mlib` → `pmwbind /R /O /Sdos4gw.exe star.exe`.

---

## 1. Architecture

**The single most important architectural fact: everything is converted to S3M in
memory.** MOD and MTM are transcoded to a synthetic S3M image (header + packed patterns
+ sample headers + sample data) at load time; the replay core only ever understands S3M.
`S3MLIB.INC:147-148` says so plainly: `TYPE_MOD ;Module type is MOD (converts to S3M)`.

This is precisely why the original's MOD playback has known inaccuracies — the
conversion destroyed the format's identity before the player ever saw it. **The Rust
engine does not repeat this** (see `plans/product/00-vision.md` decision 4).

### Public API

```
PM_InitSystem    ; AL=device, BX=mixing rate, DX=SB buffer size
PM_LoadModule    ; EDX=filename, EBX=Module struct, AL=load flags
PM_PlayModule    ; EBX=Module, AL=forced mixing volume
PM_StopModule / PM_ReleaseModule / PM_CloseSystem
PM_GetDeviceRAM / PM_GetMasterVol / PM_SetMasterVol
PM_SetLoopCode   ; EDX = near proc called when the order list wraps
PM_GetFileType   ; sniff file + return 28-char title
```

Load flags: `LOAD_HI`, `LOAD_LO`, `LOAD_DUMP` (dump samples after loading), `LOAD_FREE`
(free sample RAM after dumping).

### Routine map

| Area | Routine | Lines |
|---|---|---|
| API | `PM_SetLoopCode` … `PM_LoadModule` | 587–947 |
| Type sniffing | `GetFileType` (SCRM@0x2C → M.K.@1080 → MTM@0) | 951–1014 |
| | `LoadModFile` | 1016–1066 |
| MOD loader | `ConvertMOD` | 1072–1217 |
| | `ConvertPatterns` / `ConvertIt` / `ConvertNote` / `ConvertValues` / `ConvertECmd` | 1629–1917 |
| | `SetChannels`, `ConvertSamps`, `GetNumChans`, `CmpIdCode` | 1919–2158 |
| MTM loader | `ConvertMTM`, `ConvertMTMPats`, `ConvertItMTM`, `ConvertNoteMTM`, `ConvertMTMSamps` | 1222–1566, 2023–2099 |
| Shared | `SetStartHeader`, `CopySamples` | 1569–1627 |
| S3M parse | `ParseModule`, `FindSampsSize` | 2163–2265 |
| **Tick handler** | `__UpdateTracker` (major = row, minor = tick) | 2376–2632 |
| | `ClipPitch` (glissando + Amiga limits) | 2634–2688 |
| | `FindPatternPos` (seek to a row in packed data) | 2690–2716 |
| Tick-0 effects | `StaticJumpTable` + `__HandleCommands` + `S_FX_*` | 2722–3164 |
| Per-tick effects | `MinorJumpTable` + `HandleMinor` + `M_FX_*` | 3167–3538 |
| Tuning | `PeriodFromNote`, `PeriodToPitch` | 3541–3571 |
| Helpers | `FetchChannel`, `LoadPanSettings`, `ClearChannels`, `PtrToSample` | 3574–3687 |
| DMA (8237) | `ProgramDMAChip`, `GetDMARegisters`, `GetDMAPage`, `GetPageFixedBuf` | 3699–3830 |
| GUS driver | `GUS_SetInterface`, `GUS_Init`, `GUS_Close`, `GUS_StartSong` | 3840–4113 |
| | `Calc_GUS_BPM`, `_SetGUSTimerSpd`, `GUS_Read_Env` | 4116–4232 |
| | `GUS_IRQ_Handler`, `_GIRQStartVoice`, `GUS_ProcessTracks`, `_GRampToVol` | 4244–4689 |
| | `FetchGUSFreq`, `FetchGUSVolume` | 4692–4723 |
| | `G_DumpSams2DRAM`, `G_DumpS3MSample`, `GUS_DumpSamLoop`, `ProgramGUSDMA` | 4726–4931 |
| | `UltraReset`, ICS mixer, `Ultra{Ping,Peek,Poke,DRAMSize}` | 4934–5379 |
| SB driver | `SB_Init`, `SB_GetDSPVersion`, `InitMixingTables` | 5385–5543 |
| | `SB_StartSong`, `SetSBTempo`, `SB_IRQ_Handler`, `SB_ProcessTracks` | 5546–5836 |
| | `_Mix8bitMono` macro + `Mixer_8bitMono` | 5840–6006 |
| | `SB_StopSong`, `ProgramSBChip`, `SwapSBBuffers`, `SetSBMixingRate`, `SetTimeConstant`, `SB_ResetMono`, `SB_Close`, `SB_Read_Env` | 6009–6310 |

**The layering is clean, and worth copying.** `__UpdateTracker` is device-independent
and writes into per-channel state plus a dirty-flag byte; each device driver has a
`*_ProcessTracks` that consumes the dirty flags and pushes to hardware. This is the
direct ancestor of `VoiceParams` + `DirtyBits` in the Rust design.

---

## 2. Data structures

### `ChannelData` (`S3MLIB.INC:15-51`) — 32 instances

| Field | Type | Meaning |
|---|---|---|
| `_ChannelNumber` | u8 | channel index 0–31 |
| `_SampleNum` | u8 | current instrument 1–99; **255 = no sample / cut** |
| `_ChannelFlag` | u8 | dirty flags |
| `_SampleOffset` | u16 | byte offset for retrigger / `Oxx` |
| `_CurrentVol` | u8 | base channel volume 0–64 (what tremolo/tremor modulate) |
| `_ActualVol` | u8 | volume actually sent to hardware |
| `_CurrentNote` | u8 | note byte `(oct<<4)|note`; **0 = none** |
| `_CurrentPeriod` | u32 | base ST3 period (no vibrato/arpeggio) |
| `_TargetNote` | u8 | portamento destination (informational) |
| `_TargetPeriod` | u32 | portamento destination period |
| `_ActualPeriod` | u32 | period actually sent (post vibrato/arp/gliss/clip) |
| `_CommandValue` | u8 | effect number 0–26 for tick processing |
| `_DataValue` | u8 | effect parameter for tick processing |
| `_PortaValue` | u8 | `Gxx` memory |
| `_VolSlideValue` | u8 | **shared** memory for `Dxx`, `Exx` *and* `Fxx` |
| `_VibValue` | u8 | **shared** memory for `Hxx` / `Rxx` / `Uxx` (`speed<<4 | depth`) |
| `_VibCount` | u8 | **shared** waveform phase 0–63 for vibrato *and* tremolo |
| `_VibTable` | u8 | vibrato waveform 0–3 |
| `_TremTable` | u8 | tremolo waveform 0–3 |
| `_RetrigValue` | u8 | `Qxx` memory |
| `_C4SPD` | u32 | middle-C rate of the current sample (default 8363) |
| `_SampleVolume` | u8 | default volume of the last sample |
| `_PanPosition` | u8 | 0–15 (GUS balance nibble) |
| `_SpecialValue` | u8 | multi-use counter: `Qxx` tick counter, `SCx` counter, `SDx` saved flags |
| `_TremorCount` / `_TremorFlag` | u8 | `Ixx` counter and on/off phase |
| `_ArpCount` / `_ArpValue` | u8 | `Jxx` 3-phase counter and memory |
| `_OffsetValue` | u8 | `Oxx` memory |
| `_GlissFlag` | u8 | `S1x` glissando enable |
| `_VUBarLevel` | u8 | display only; decays 2 per tick |
| `_CMDVal` / `_CMDData` | u8 | display only (raw row command) |
| `_ActiveFlag` | u8 | **written by the driver**; 1 = voice still sounding. Used by the porta-vs-new-note decision, so **not** purely informational |

Dirty flags (`S3MLIB.INC:55-65`) — the direct ancestor of `DirtyBits`:

```
_CHN_NewVol    01h   _CHN_NewSamp   02h   _CHN_NewPitch  04h
_CHN_NewPan    08h   _CHN_NewBPM    10h   _CHN_StopVoice 80h   ; GUS only
```

### `Module` (`S3MLIB.INC:68-117`)

Header/global: `_Pointer`, `_Size`, `_Type` (1=S3M, 2=MOD, 3=MTM), `_DeviceType`,
`_Title[29]`, `_SampleFlag`, `_SampleSize`, `_Ordnum`, `_Insnum`, `_Patnum`,
`_globalvol`, `_mastervol` (bit 7 = stereo), `_initialspd`, `_initialBPM`,
`_stereoflag`, `_generalflags` (S3M header word @0x26; **bit 4 = Amiga limits**),
`_RealGlobalVol`, `_TotalChanNum`.

Playback cursor: `_MCurrentSpd`, `_MCurrentBPM`, `_BreakToRow`, `_MCurrentRow`,
`_MRowPointer` (raw pointer into the packed pattern), `_MCurrentPos` (order index),
`_MCurrentPatt`, `_MCurrentTick` (down-counter), `_MRowDelay`, `_MRowLoopStart`,
`_MRowLoopCount`.

Plus a snapshot set `_MActualRow` / `_MActualPos` / `_MActualPatt` / `_MActualTick`
taken at the top of each row, **so the UI sees the row that is currently sounding**
rather than the one being parsed. Worth reproducing in the telemetry design.

Commented-out XM scaffolding (`TYPE_XM`, `_XMPattTable`, `XM_MAXSAMPS`) shows XM was
planned and never landed.

### `MixData` — the SoundBlaster mixer voice, 32 instances

`_Mix_CurrentPtr` (linear sample pointer), `_Mix_LoopEnd`, `_Mix_LoopLen`,
`_Mix_LowSpeed` (32-bit fractional step), `_Mix_HighSpeed` (integer step), `_Mix_Count`
(32-bit fraction accumulator), `_Mix_ScaleRate` (2^32/step), `_Mix_Volume` (`vol<<8`,
a `VolumeTable` row offset), `_Mix_PanPos` (stored, unused by the mono mixer),
`_Mix_ActiveFlag`.

---

## 3. Playback timing

Two completely different clock sources, both calling the same `__UpdateTracker`.

### GUS — hardware timer IRQ (not audio-locked)

`Calc_GUS_BPM` (4116): `count = 31250 / BPM`. The GF1 timer-1 base is 80 µs (12500 Hz),
and ticks/sec = BPM × 2 / 5, so 12500 / (0.4 × BPM) = 31250 / BPM.

Timer 1 is an 8-bit down-counter (the value written is `256 - n`), so counts > 256
(BPM < 122) are chained by `_SetGUSTimerSpd` (4131–4153): subtract 256 per IRQ from
`_GUS_TimerLeft`, write 0 (= a full 256), and enforce a minimum residue of 16 for the
last chunk. `__UpdateTracker` runs only when `_GUS_TimerLeft == 0`.

On GUS the tick is **not** tied to audio — the GF1 plays voices autonomously and the
tracker just reprograms voice registers on a timer.

### SoundBlaster — mixing-buffer refill (audio-locked) — **this is the model to copy**

`SetSBTempo` (5621): `_SB_GapLength = (MixingRate * 10 / BPM) >> 2`, i.e.
`rate × 2.5 / BPM`.

`SB_IRQ_Handler` (5634) fires on DMA buffer completion, flips the DMA buffers
(`SwapSBBuffers`), then fills the *other* buffer in slices:

```
@@fine:  if _SB_GapCount == 0:
             __UpdateTracker ; SB_ProcessTracks ; _SB_GapCount = _SB_GapLength
         ecx = min(_SB_GapCount, _SB_BufCount)
         Mixer_8bitMono(edi, ecx)
         edi += ecx ; _SB_GapCount -= ecx ; _SB_BufCount -= ecx ; loop
```

**Sample-exact event placement.** A tick boundary always lands on the correct output
sample regardless of buffer size; multiple ticks per buffer and a tick spanning two
buffers are both handled. This is a genuine accuracy feature and it is reproduced
verbatim in the Rust render loop.

The one flaw: `(rate*10/bpm)>>2` truncates twice — 848 instead of 848.077 at 44100/130,
about 1.3 s of drift over a four-minute song. The Rust engine makes this a `TempoModel`
policy so drift-free is the default and the truncating behaviour stays available.

After each buffer, `_Mix_ActiveFlag` is copied back into `_ActiveFlag` per channel, and
the song-loop callback fires only when `_MActualPos == 0`.

### `__UpdateTracker` structure (2376)

```
decay VU bars (−2 per tick)
dec _MCurrentTick
  ≠0 → MINOR: for each channel { HandleMinor ; ClipPitch }
  =0 → MAJOR: snapshot _MActual*
              if _MRowDelay: dec, skip the row, reload the tick counter
              per-channel row reset:
                 _CMDVal = _CMDData = 0
                 if _CurrentPeriod != _ActualPeriod:
                        _ActualPeriod = _CurrentPeriod ; flag NewPitch
                 if last _CommandValue != 17 (Q): _SpecialValue = 0
                 _CommandValue = 0
              decode the packed row → per channel: instrument, note, volume, command
              advance row / order / pattern
              _MCurrentTick = _MCurrentSpd
```

Two subtleties worth stating explicitly, both reproduced:

- `_ActualPeriod` **is** restored from `_CurrentPeriod` at row start.
- `_ActualVol` is **not** restored from `_CurrentVol` — so tremolo and tremor volume
  offsets persist into the next row until something writes a volume.

---

## 4. Effects

Effect numbers are 1 = `A` … 26 = `Z`. Dispatch: `__HandleCommands` (2752) on tick 0 via
`StaticJumpTable` (2722); `HandleMinor` (3196) on ticks 1..n−1 via `MinorJumpTable`
(3167).

| Cmd | Tick-0 (`S_FX_*`) | Per-tick (`M_FX_*`) | Notes |
|---|---|---|---|
| `Axx` set speed | 2776 | — | `_MCurrentSpd = xx`, **no zero check** (see accuracy policy D6) |
| `Bxx` order jump | 2782 | — | `pos = xx-1`, `row = 0FEh`, `BreakToRow = 0` |
| `Cxx` pattern break | 2792 | — | **`xx` read as decimal**: `(xx>>4)*10 + (xx&0F)` |
| `Dxy` volume slide | 2805 | 3213 | see below |
| `Exx` porta down | 2850 | 3236 | shares `_VolSlideValue` |
| `Fxx` porta up | 2879 | 3246 | shares `_VolSlideValue` |
| `Gxx` tone porta | 2908 | 3256 | overshoot-guarded both directions |
| `Hxy` vibrato | 2920 | 3290 | |
| `Ixy` tremor | 2941 | 3317 | tick 0 also runs the tick handler |
| `Jxy` arpeggio | 2956 | 3340 | tick 0 also runs the tick handler |
| `Kxy` vibrato + vol slide | 2973 | 3372 | `M_FX_D` then `M_FX_H` |
| `Lxy` porta + vol slide | 2987 | 3378 | `M_FX_D` then `M_FX_G` |
| `Oxx` sample offset | 3001 | — | gated on a note being present |
| `Qxy` retrigger | 3015 | 3384 | |
| `Rxy` tremolo | reuses `S_FX_H` | 3442 | shares table, counter and `_VibValue` with vibrato |
| `Sxy` special | 3038 | 3479 | |
| `Txx` set tempo | 3130 | — | clamped `>= 20h` |
| `Uxy` fine vibrato | reuses `S_FX_H` | 3514 | |
| `Vxx` global volume | 3140 | — | clamp 0–64, marks all channels `NewVol` |
| `Xxx` set pan | 3154 | — | |

Not implemented (mapped to `S_FX_0`): **M, N, P, W, Y, Z**. `S_FX_0` clears both
`_CommandValue` and `_DataValue`, so no per-tick work happens.

### The non-obvious behaviours

**`Dxy` volume slide.** Tick-0 classification, in this exact order:

```
if xy == 0: xy = _VolSlideValue                          ; parameter memory
if xy > 0F0h                       → fine slide DOWN by (xy & 0F), applied instantly
elif (xy & 0F) == 0F and (xy>>4)!=0 → fine slide UP   by (xy>>4), applied instantly
else                                → store for per-tick processing
```

So `DFx` (x≠0) is fine-down, `DxF` (x≠0) is fine-up, and `DF0` / `D0F` fall through to
the *normal* slide path. Per tick (`M_FX_D`) the **high nibble wins**: slide up by the
high nibble if `xy & F0` is non-zero, otherwise down by the low nibble. Underflow is
detected with the unsigned trick `cmp al,64 / jna ok` — any value 65–255 after
subtraction clamps to 0.

**`Exx` / `Fxx` pitch slides.**

```
if xx == 0: xx = _VolSlideValue        ; ← SHARED with the volume slide memory
_VolSlideValue = xx
if      xx <= 0DFh → normal slide, per tick: period ±= xx*4
elif    xx <= 0EFh → extra-fine, instant:    period ±= (xx & 0F) * 1
else               → fine,       instant:    period ±= (xx & 0F) * 4
```

The shared `_VolSlideValue` across D/E/F is real ST3 behaviour, not a bug. The `×4`
everywhere reflects **S3M periods = Amiga periods × 4**.

**`Gxx` tone portamento.** Step `= xx*4` per tick with explicit overshoot prevention in
both directions. On arrival, `_CurrentPeriod = _ActualPeriod = _TargetPeriod`.

The tick-0 porta *detection* lives in `__UpdateTracker` (2544–2569):

```
if _ActiveFlag != 0 and (command == 7 (G) or command == 12 (L)) and _SampleNum != 255:
        _TargetNote / _TargetPeriod = the new note      ; do NOT retrigger
else:   normal note: CurrentPeriod = TargetPeriod = ActualPeriod = period
        _SampleOffset = 0 ; flag _CHN_NewSamp
```

Critically this depends on `_ActiveFlag`, which the **driver** maintains — a portamento
onto a channel whose one-shot sample has already finished behaves as a fresh trigger.

**`Hxy` vibrato.** Parameter merge rule:

```
if   xy == 0    : xy = old
elif xy <= 0Fh  : xy = (old & F0h) | xy       ; a depth-only update keeps the old speed
```

The phase counter is reset **only if `_CHN_NewSamp` is set this row** (a real new note).
Per tick:

```
delta = (sign_extend(Table[_VibTable][_VibCount]) << 2) * (_VibValue & 0F) >> 7
_ActualPeriod = _CurrentPeriod + delta
_VibCount = (_VibCount + (_VibValue >> 4)) mod 64
```

`Uxy` (fine vibrato) is identical **without the `<< 2`** — exactly quarter depth.
`Rxy` tremolo uses the same table, counter and `_VibValue`, without the `<< 2`, adds to
`_CurrentVol`, clamps 0–64 and writes `_ActualVol`.

**Waveform tables** (435–470), 64 signed 16-bit entries each, amplitude ±255:

- `Vib_Sine_Table` — 64-point sine: `0,25,50,74,98,120,142,162,180,197,212,225,236,244,
  250,254,255,254,…` then negated for the second half.
- `Vib_Ramp_Table` — a **rising** ramp from −255 to +255 (index 0 = −255, index 32 = −0,
  index 63 = 255). Irregular step at indices 32–33 (`…,-8, -0, 16, 24,…` — 8 is skipped)
  and it ends at 255 rather than 256. *See accuracy policy D5.*
- `Vib_Pulse_Table` — **only 62 entries** (31 zeros then 31 × 255). Indices 62 and 63
  read past the end into `Vib_Rand_Table` and return **105** and **17**.
  *See accuracy policy D1 — this is a defect and we do not reproduce it.*
- `Vib_Rand_Table` — 128 **fixed** pseudo-random entries (only 0–63 reachable). A fixed
  table, not an RNG, so waveform 3 is fully deterministic.

**`Ixy` tremor.** Memory in `_DataValue`; the tick handler also runs on tick 0.

```
if _TremorCount != 0 : dec ; return
if _TremorFlag == 1  : flag = 0 ; count = xy & 0F ; _ActualVol = 0
else                 : flag = 1 ; count = xy >> 4 ; _ActualVol = _CurrentVol
```

The "restore volume" branch reads `[edi+_CurrentVol]` while the minor-tick loop passes
the channel in **esi**; `edi` is undefined there. *See accuracy policy D3.*

**`Jxy` arpeggio.** `_ArpCount = 1` on tick 0 and the tick handler runs immediately,
giving the cycle **base, x, y, base, x, y…** (decrement to 0 ⇒ base and reload 3;
count == 2 ⇒ high nibble; else low nibble). The note arithmetic operates on the packed
note byte:

```
dl = note & 0F ; dh = note & F0
dl += semitones
if dl >= 12 : dh += 10h ; dl -= 12          ; SINGLE carry only
```

Because only one carry is applied and `PeriodFromNote` masks with `and bl,0Fh`, an
arpeggio pushing the note index to 12–14 indexes **past the 12-entry `Period_Table` into
`Volume_Table`** (values 0, 30832, 35888). *See accuracy policy D2.*

**`Qxy` retrigger.** `_SpecialValue` is the down-counter, and the row-start reset loop
**skips clearing it** when the previous row's command was 17 (`Q`) — so retrigger phase
carries across rows. On tick 0, if a retrigger was already in flight, `M_FX_Q` runs
immediately. Volume operations use:

```
Retrig_Table db 0,-1,-2,-4,-8,-16,0,0,0,1,2,4,8,16,0,0
```

with four multiplicative special cases: `x=6 → vol*2/3`, `x=7 → vol/2`,
`x=E → vol*3/2`, `x=F → vol*2`. The result is clamped 0–64 and written to **both**
`_CurrentVol` and `_ActualVol`; `_SampleOffset = 0` and `_CHN_NewSamp` is set.

**`Sxy` special.**

- `S1x` — glissando on/off → `_GlissFlag`.
- `S2x` — set finetune → `_C4SPD = FineTuneTable[x]`, where
  `FineTuneTable dw 7895,7941,7985,8046,8107,8169,8232,8280,8363,8413,8463,8529,8581,
  8651,8723,8757` — a **monotonic** 0–15 table (ST3 semantics), a *different ordering*
  from the `C2SPD_Table` the MOD loader uses for MOD's signed finetune nibble.
- `S3x` / `S4x` — vibrato / tremolo waveform:
  ```
  if x >= 3 : x -= 4 ; _VibCount = 0        ; (x == 3 wraps to 0FFh)
  table = x & 3
  ```
  So x = 0,1,2 keep phase; x = 3..7 reset it; **x = 3 and x = 7 both select the random
  table**. `S4x` also resets the *shared* `_VibCount`, so setting a tremolo waveform
  disturbs vibrato phase. All reproduced.
- `S8x` — set pan 0–15 (`_CHN_NewPan`).
- `SBx` — pattern loop. `SB0` sets `_MRowLoopStart = _MCurrentRow`. `SBx`: if
  `_MRowLoopCount < x` then increment the count, `dec _MCurrentPos`,
  `_MCurrentRow = 0FEh`, `_BreakToRow = _MRowLoopStart`; else reset the count to 0.
  `_MRowLoopStart` resets to 0 whenever a pattern is left.
- `SCx` — note cut. Stores `x` in `_SpecialValue`; the tick handler decrements and, at
  0, cuts: `_CurrentNote = 0`, all three periods = 1712, `_SampleNum = 255`,
  `_SampleOffset = 0`, `_CHN_NewSamp`, `_CommandValue = 0`.
- `SDx` — note delay. **The clever one.** The tick-0 handler saves the *entire*
  `_ChannelFlag` byte into `_SpecialValue` and zeroes `_ChannelFlag`, so the note,
  volume and pitch the row already latched simply never reach the hardware. The tick
  handler counts the low nibble of `_DataValue` down (rewriting it as `0D0h|n` each
  tick) and, on reaching 0, restores `_ChannelFlag = _SpecialValue`, releasing the note.
- `SEx` — pattern delay. `_MRowDelay = x`, consumed at the top of the major update: the
  row is **not** re-parsed, but the tick counter is reloaded, so the previous row's
  per-tick effects keep running for `x` extra rows.
- `SFx` — funk repeat, deliberately ignored.

**`Txx` tempo** clamps to `>= 20h` (32 BPM) and sets `_CHN_NewBPM`, which the driver
turns into a timer reprogram (GUS) or a new `_SB_GapLength` (SB).

**`Xxx` set pan.** `pan = xx >> 3; if pan >= 10h then pan -= 1; pan &= 0Fh` — maps
0–255 into 0–15 with 0xFF → 15.

**Note bytes.** 255 = no note; 254 = cut (same actions as `SCx` firing); 0 in
`_CurrentNote` means "no note present" and gates `Oxx` and `Qxy`.

**Instrument change** (2503–2525). A new instrument number always reloads `_C4SPD` and
resets `_CurrentVol = _ActualVol = ` the sample's default volume, flagging `_CHN_NewVol`
— even with no note. Two details: only the **low 16 bits** of the 32-bit C2SPD field are
read (*accuracy policy D7*), and a sample volume **> 64 is coerced to 0**, not clamped
to 64 (reproduced — it is ST3 behaviour).

**`ClipPitch`** (2634), run after every note/effect application:

```
if _GlissFlag:
        amiga = _ActualPeriod * _C4SPD / (8363*16)
        scan Period_Table, doubling per octave, for the nearest entry
        _ActualPeriod = nearest * 8363*16 >> octave / _C4SPD      (minimum 1)
if _generalflags & 16 (Amiga limits):
        clamp _ActualPeriod to [113*4 .. 856*4] = [452 .. 3424]
```

---

## 5. Format loaders

### S3M (`ParseModule`, 2163)

Validates `SCRM` at 0x2C. Reads title (0x00, 28 bytes), `Ordnum` (0x20), `Insnum`
(0x22), `Patnum` (0x24), `generalflags` (0x26), `globalvol` (0x30), `initialspd` (0x31),
`initialBPM` (0x32), `mastervol` (0x33, bit 7 → `_stereoflag`). `_TotalChanNum` is the
count of bytes in the 32-byte channel-settings array at 0x40 whose value is `<= 0Fh`.

Layout: `0x60` order list (`Ordnum` bytes) → `Insnum` u16 parapointers → `Patnum` u16
parapointers → optional 32 default-pan bytes (present iff byte 0x35 == 252) → data.
All parapointers are × 16.

Sample header (80 bytes), fields used: `[0]` = 1 for PCM, `[0x0E]` u16 data parapointer,
`[0x10]` length, `[0x14]` loop start, `[0x18]` loop end, `[0x1C]` volume, `[0x1F]` flags
(bit 0 loop, bit 2 16-bit), `[0x20]` C2SPD, `[0x2C]` **repurposed at runtime to hold the
GUS DRAM address**, `[0x30]` name, `[0x4C]` `SCRS`.

Packed pattern format: u16 packed length, then per row a byte stream; `0` terminates the
row; otherwise bits 0–4 = channel, bit 5 ⇒ 2 bytes follow (note, instrument), bit 6 ⇒ 1
byte (volume), bit 7 ⇒ 2 bytes (command, info). 64 rows per pattern. `FindPatternPos`
(2690) re-walks the stream to seek to a break row.

**S3M samples are unsigned 8-bit.**

### MOD (`ConvertMOD`, 1072)

- ID at offset 1080: `M.K.` / `FLT4` → 4 channels, `6CHN` → 6, `8CHN` / `FLT8` → 8, else
  a decimal `"ddCH"` → up to 32. **31 samples always** — 15-sample MODs unsupported.
- Title 20 bytes @ 0; 31 × 30-byte sample headers @ 20; song length @ 950; 128-byte
  order list @ 952; patterns @ 1084 (`4 × channels × 64` bytes each).
- `Patnum` = (max order value over all 128 entries, ignoring 255) + 1.
- Channel panning (`SetChannels`, 1919):
  `CHANNELSETTINGS db 0,8,9,1,2,10,11,3,4,12,13,5,6,14,15,7` repeated — the classic
  Amiga **L-R-R-L** interleave.
- Sample headers (`ConvertSamps`, 1944): name 22 bytes; length = BE u16 @22 × 2; volume
  @25 clamped to 64; loop start = BE u16 @26 × 2 clamped to length; loop length =
  BE u16 @28 × 2 — **loop enabled only if loop length > 4**; C2SPD from the finetune
  nibble @24 via
  ```
  C2SPD_Table dd 8363,8413,8463,8529,8581,8651,8723,8757,   ; finetune  0..+7
                 7895,7941,7985,8046,8107,8169,8232,8280    ; finetune -8..-1
  ```
- Samples copied with `xor 128` — **MOD samples are signed**, converted to unsigned.
- Period → note (`ConvertValues`, 1757): a linear scan of `PeriodVals`
  (856×4…453×4, 856×2…453×2, 856…453, 428…226, 214…113, 214/2…113/2, 214/4…113/4)
  taking the **first entry `<= period`**, so inexact periods snap up in pitch.
- **Amiga-limits flag**: `limitflag` starts at 16 and is cleared the moment any note
  falls outside octaves 3–5. A MOD that never leaves the standard Amiga range gets
  period clipping; an extended-range MOD does not. Clever, and reproduced.
- Effect conversion (`FXConvTable`, 560) and `ConvertECmd` (1850) are documented in
  `plans/reference/format-notes-mod.md` when the MOD loader is written — the Rust engine
  implements MOD effects **natively**, so these tables are read as *MOD semantics*, not
  as a lowering step.

### MTM (`ConvertMTM`, 1222)

Header: `MTM` + version (4 bytes), title @4 (20), `numtracks` u16 @24, `lastpattern` u8
@26, `lastorder` u8 @27, `commentlen` u16 @28, `numsamples` u8 @30, attribute @31,
`beatspertrack` @32, `numchannels` @33 (1–32), 32 pan bytes @34.

Then: `numsamples` × 37-byte sample headers @66 → 128-byte order list → `numtracks` ×
192-byte tracks (64 rows × 3 bytes) → `(lastpattern+1)` × 32 u16 track numbers (0 =
empty) → comment → sample data. Track base = `66 + 37*numsamples + 128`; pattern table =
tracks + `192*numtracks`.

Sample header: name 22, length u32 @22, loop start u32 @26, loop end u32 @30 (**loop
enabled only if end − start > 4**), finetune nibble @34 → the same `C2SPD_Table`, volume
@35 clamped 64, attribute @36 bit 0 = 16-bit.

Note packing: `b0[7:2]` = pitch (0 = none), `b0[1:0]:b1[7:4]` = 6-bit instrument,
`b1[3:0]` = effect, `b2` = data. Pitch → note byte: `octave = pitch/12 + 2`,
`note = pitch%12`.

**MTM samples are already unsigned** (`Temp_XOR = 0`).

Note that the original's MTM conversion calls `ConvertValues` with a dummy period of
1712, which always lands in octave 2 and therefore **always clears the Amiga-limits
flag** for any MTM containing a note. The Rust MTM processor sets Amiga limits off
directly rather than inheriting this by accident.

### Panning at playback

`ClearChannels` (3617) per channel: default pan **7** (centre). If `_stereoflag`, read
S3M channel-setting byte `[0x40+chan]`: `>= 128` → 7; `< 8` → **3** (left); else →
**0Ch** (right). Then `LoadPanSettings` (3585) overrides from the 32-byte default-pan
array when header[0x35] == 252, taking `byte & 0x0F` only when bit 5 is set. Channel init
also sets `_SampleNum = 255`, all periods = 1712, `_C4SPD = 8363`, `_CHN_NewPan`.

---

## 6. Period and frequency maths

```
Period_Table dw 1712,1616,1524,1440,1356,1280,1208,1140,1076,1016,960,907
             ; (preceded by a stray dw 1814 at offset -2)
```

**Note → period** (`PeriodFromNote`, 3541):

```
oct = note >> 4 ; n = note & 0Fh
period = (Period_Table[n] * 8363 * 16 >> oct) / _C4SPD        ; minimum 1
```

`8363 * 16 = 133808`. With the default C2SPD of 8363 this gives `27392 >> oct`, so
octave 4 ⇒ 1712 = Amiga 428 × 4. **S3M periods are Amiga periods × 4** — which is why
every slide multiplies by 4 and the Amiga clamp is `[113*4, 856*4]`.

**Period → frequency** (`PeriodToPitch`, 3561): `hz = 14317056 / period`
(`0DA7600h`, the ST3 constant). **Note:** `0DA7600h` is 14,317,056, which is *not*
8363 × 1712 (= 14,317,456) — it is 400 low. `PeriodFromNote` uses `8363 * 16`, which
*does* give exactly 8363 × 1712 at octave 4, so a C-4 round-trips as 8362.77 Hz rather
than 8363 Hz. This is ST3's own inconsistency (the constant is ST3's), not a StarPlayer
defect, and it is reproduced as-is; `starplayer-core::tables` asserts the 400 gap.

**Frequency → mixer step** (`SB_ProcessTracks`, 5795): `HighSpeed = hz / rate`, with the
fractional part in `LowSpeed` via a 64-bit divide — a Q32.32 step split across two
dwords. This is exactly the Rust `Step(u64)` type.

**Frequency → GUS FC** (4692): `round(hz * 1024 / Divisor_Table[voices-14])`.

**Tick rate**: `ticks_per_second = BPM * 2 / 5`; GUS timer count = `31250 / BPM` at
80 µs; SB bytes per tick = `MixingRate * 2.5 / BPM`.

---

## 7. Hardware drivers (historical record — not reimplemented)

Recorded because the GUS behaviour is genuinely interesting engineering, and because the
SB mixer explains the original's sound. Neither is reimplemented: see
`plans/product/03-accuracy-policy.md` §4.

### Gravis Ultrasound

- Config from the `ULTRASND=` environment variable (`GUS_Read_Env`, 4158): port, DMA,
  IRQ. `UltraPing` verifies by poking 0xAA/0x55 into DRAM address 0; `UltraDRAMSize`
  probes in 256 KB steps up to 1 MB.
- **Voice allocation is 1:1 with module channels.** `UltraReset` clamps the count to
  14–32 and programs `SET_VOICES`.
- **Mixing-rate adaptation** — the signature GUS behaviour. The GF1's output rate falls
  as active voices rise, so the frequency divisor comes from
  ```
  Divisor_Table dw 44100,41160,38587,36317,34300,32494,30870,29400,
                   28063,26843,25725,24696,23746,22866,22050,21289,
                   20580,19916,19293          ; index = numvoices - 14
  ```
- **Volume**: `v = (chanvol * globalvol) / 64` clamped to 64, then a 65-entry
  **logarithmic** `Volume_Table`, `shr 4` to a 12-bit GF1 volume.
- **Volume ramping / anti-click** — the distinctive part. On a new note,
  `GUS_ProcessTracks` does **not** start the voice; it ramps the old note down to 0 with
  a ramp-end IRQ. The IRQ handler then calls `_GIRQStartVoice`, which stops the voice,
  zeroes the accumulator, programs the addresses, frequency, volume and pan, and only
  then starts playback. **Every retrigger is click-free and deferred by one ramp.**
- **Looping**: looped samples use `VC_LOOP_ENABLE`; one-shots use `VC_WAVE_IRQ` with
  `end = base + length - min(length,128)` so a wave IRQ fires ~128 bytes early and the
  handler ramps the voice to silence. 16-bit samples set bit 2 and convert addresses via
  `(addr>>1 & 0x1FFFF) | (addr & 0xC0000)`.
- **DRAM management**: `_DRAM_Counter` starts at 32; each sample is allocated `len+1`
  rounded to 32 bytes, and one **extra byte** is poked one past the end (the loop-start
  byte for looped samples, the last byte otherwise) to kill loop clicks.

### SoundBlaster (mono only)

- Config from `BLASTER=` (`A`ddr, `I`rq, `D`ma).
- **Rate** (`SetSBMixingRate`, 6086): DSP ≥ 2.01 and requested > 21739 Hz → high-speed,
  `tc = (65536 - 256000000/rate) >> 8` capped at 233 ⇒ **max 43478 Hz**; otherwise
  `tc = 256 - 1000000/rate` capped at 210 ⇒ **max 21739 Hz**. The achieved rate is
  recomputed and stored back into `__MixingRate`.
- **Buffers**: two DMA buffers in low DOS memory via `GetPageFixedBuf`, which guarantees
  no 64 KB page crossing; filled with 0x80 (silence). Plus a hi-mem 16-bit accumulator.
- **Output format**: 8-bit **unsigned mono**. Panning is computed and stored but never
  used.
- **Mixing tables** (`InitMixingTables`, 5502):
  - `VolumeTable[65][256]`, entry `[vol][b] = ((b - 128) * vol) >> 6` as a signed byte —
    one lookup does both the unsigned→signed conversion and the volume scale.
  - `PostTable[2048]`, built per song from `_mastervol & 127` (minimum 0x10):
    ```
    c = 2048*16 / mastervol ; a = (2048 - c)/2 ; b = a + c
    PostTable[x] = 0                for x <= a
                 = 255              for x >= b
                 = (x-a)*256/(b-a)  otherwise
    ```
    A soft window with hard clipping — a higher master volume gives a narrower window and
    so more amplification. **The "amplification" is a clipping curve, not a multiply.**
    The `-a` command-line switch (clamped 16–127) feeds it.
- **Mixing loop** (`Mixer_8bitMono`, 5849): pure Q32.32 fixed-point resampling, **no
  interpolation**, unrolled ×16. The accumulator array is pre-filled with 1024 (the
  centre of `PostTable`). The final pass sign-extends the 16-bit accumulator and uses it
  **unbounded** as a `PostTable` index — *accuracy policy D4*.

---

## 8. Summary — what must be replicated

1. Sample-slice-accurate tick placement (mix in gap-length chunks, never one tick per
   buffer).
2. Shared parameter memories: **D/E/F share `_VolSlideValue`**; **H/R/U share
   `_VibValue` and `_VibCount`**.
3. `Dxy` fine-slide classification order, and the high-nibble-wins rule per tick.
4. `Cxx` pattern break interpreted as **decimal**.
5. `SDx` note delay implemented by stashing and restoring the whole dirty-flag byte.
6. `_SpecialValue` surviving across rows only for command 17 (`Qxy`).
7. `_ActualPeriod` restored at row start but `_ActualVol` **not**.
8. Portamento-vs-retrigger gated on whether the voice is still sounding.
9. `S3x`/`S4x` waveform selection offset (`x >= 3` resets phase; 3 and 7 both select
   random), and `S4x` clobbering the shared vibrato phase.
10. Sample volume > 64 becoming 0 on instrument change.
11. Waveform 3 being a fixed table, so playback is deterministic.
12. Loader-side decisions: the MOD loop-length > 4 gate, the LRRL channel map, the
    Amiga-limits flag derived from the song's octave range, and the sign conventions
    (MOD signed, S3M and MTM unsigned).

And what must **not** be replicated: see `plans/product/03-accuracy-policy.md` §3
(D1–D7).
