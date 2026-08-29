# M1-task-B4 — The ST3 effect processor

| Field | Value |
|---|---|
| Milestone | M1 ([master plan](M1-master-plan.md)) |
| Depends on | B2 (loader), B3 (render loop + RowClock) |
| Blocks | M1 exit; M2 (MOD/MTM reuse the machinery, not the semantics) |
| Parallel with | B5 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests per effect) + **owner (listening check on real modules)** |

## Context for a fresh agent

**This is the accuracy core of the project.** The original StarPlayer's reputation rested
on getting these semantics right, and the subtleties below — tick-0 versus per-tick
behaviour, shared parameter memories, clamping rules, counters that survive across rows —
are exactly where tracker players differ from one another.

Three documents are mandatory reading before writing any code:

1. `plans/reference/original-s3mlib-analysis.md` §4 — the specification, effect by
   effect, transcribed from the assembly.
2. `plans/product/03-accuracy-policy.md` — §1 lists the deliberate quirks that **are**
   reproduced; §3 lists the seven defects (D1–D7) that are **not**.
3. `plans/product/01-technical-architecture.md` §§2, 4 — why effects write `VoiceParams`
   fields and set `DirtyBits` directly rather than emitting events, and why `RowClock`
   exposes an absolute tick index.

The original's own source is at `STARPLAY/S3MLIB.ASM`, inside `comment %` blocks (the
text is complete and readable). Line references are in the analysis document's routine
map. **`STARPLAY/` is read-only.**

### The shape of the work

Effects are numbered 1 = `A` … 26 = `Z`. The original dispatches through two jump tables:
`StaticJumpTable` on tick 0 (`S_FX_*` handlers) and `MinorJumpTable` on ticks 1..n−1
(`M_FX_*` handlers). Keep that split — it maps directly onto `RowClock`'s predicates and
it is how the semantics are actually organised.

Effects do **not** emit events. They compute absolute values in ST3's own integer domain
and write `VoiceParams` fields, setting `DirtyBits`. This mirrors the original exactly:
its `_CHN_NewVol` / `_NewSamp` / `_NewPitch` / `_NewPan` / `_NewBPM` flags are the direct
ancestors of `DirtyBits`.

## Deliverables

### 1. Per-channel state

Port `ChannelData` (analysis §2) to a Rust struct, with **full names** per the working
agreements — `volume_slide_memory`, not `_VolSlideValue`. Keep a comment mapping each
field to its original name so the analysis document stays navigable.

The shared memories are load-bearing and must be shared, not duplicated:

- **`Dxx`, `Exx` and `Fxx` share one parameter memory** (`_VolSlideValue`).
- **`Hxx`, `Rxx` and `Uxx` share both the parameter (`_VibValue`) and the waveform phase
  counter (`_VibCount`).**

### 2. The row-start reset

Reproduce `__UpdateTracker`'s per-channel row reset exactly (analysis §3):

```
display command/data cleared
if current_period != actual_period: actual_period = current_period; dirty |= PITCH
if last command != 17 (Q): special_value = 0        // retrigger phase survives rows
command = 0
```

Two subtleties, both reproduced:
- `actual_period` **is** restored from `current_period`.
- `actual_vol` is **not** restored from `current_vol` — so tremolo and tremor volume
  offsets persist into the next row until something writes a volume.

### 3. The effect set

Implement every row, citing the analysis section in a comment at each handler.

| Cmd | Tick 0 | Ticks 1..n−1 | Key semantics |
|---|---|---|---|
| `Axx` set speed | ✓ | — | Sets speed. **Ignore `A00`** — accuracy-policy **D6** |
| `Bxx` order jump | ✓ | — | `pos = xx-1`, row marker `0FEh`, break row 0 |
| `Cxx` pattern break | ✓ | — | **`xx` read as decimal**: `(xx>>4)*10 + (xx&0F)` |
| `Dxy` volume slide | ✓ | ✓ | Classification order below; **high nibble wins** per tick |
| `Exx` porta down | ✓ | ✓ | Shares memory with `Dxx`; normal ×4, fine ×4, extra-fine ×1 |
| `Fxx` porta up | ✓ | ✓ | Same classification as `Exx` |
| `Gxx` tone porta | ✓ | ✓ | Step `xx*4`; overshoot-guarded in both directions |
| `Hxy` vibrato | ✓ | ✓ | Merge rule below; phase resets only on a real new note |
| `Ixy` tremor | ✓ | ✓ | Tick 0 also runs the tick handler. Fix **D3** |
| `Jxy` arpeggio | ✓ | ✓ | Tick 0 also runs the tick handler. Fix **D2** |
| `Kxy` vib + vol slide | ✓ | ✓ | `M_FX_D` then `M_FX_H` |
| `Lxy` porta + vol slide | ✓ | ✓ | `M_FX_D` then `M_FX_G` |
| `Oxx` sample offset | ✓ | — | Gated on a note being present |
| `Qxy` retrigger | ✓ | ✓ | Counter survives rows; `Retrig_Table` + 4 multiplicative cases |
| `Rxy` tremolo | reuses `S_FX_H` | ✓ | Shares table, counter and parameter with vibrato |
| `Sxy` special | ✓ | ✓ | Sub-effects below |
| `Txx` set tempo | ✓ | — | Clamp `>= 20h`; sets `DirtyBits::TEMPO` |
| `Uxy` fine vibrato | reuses `S_FX_H` | ✓ | Identical to `Hxy` **without the `<< 2`** — quarter depth |
| `Vxx` global volume | ✓ | — | Clamp 0–64; marks every channel `DirtyBits::VOLUME` |
| `Xxx` set pan | ✓ | — | `pan = xx>>3; if pan >= 10h { pan -= 1 }; pan &= 0Fh` |

`M`, `N`, `P`, `W`, `Y`, `Z` are not implemented: clear both the command and the data so
no per-tick work happens.

**`Dxy` classification, in this exact order:**
```
if xy == 0: xy = volume_slide_memory
if xy > 0F0h                        -> fine slide DOWN by (xy & 0F), instantly
elif (xy & 0F) == 0F and (xy>>4)!=0 -> fine slide UP   by (xy>>4), instantly
else                                -> store for per-tick processing
```
So `DFx` (x≠0) is fine-down, `DxF` (x≠0) is fine-up, and `DF0` / `D0F` fall through to
the normal path. Per tick the high nibble wins. Underflow clamps to 0 (the original used
the unsigned trick `cmp al,64 / jna`; any value 65–255 after subtraction clamps).

**`Exx` / `Fxx` classification:**
```
if xx == 0: xx = volume_slide_memory        // SHARED with Dxx
volume_slide_memory = xx
if      xx <= 0DFh -> normal, per tick: period +/- xx*4
elif    xx <= 0EFh -> extra-fine, instant: period +/- (xx & 0F) * 1
else               -> fine,       instant: period +/- (xx & 0F) * 4
```
The `×4` throughout reflects **S3M periods = Amiga periods × 4**.

**`Hxy` parameter merge:**
```
if   xy == 0   : xy = old
elif xy <= 0Fh : xy = (old & F0h) | xy      // depth-only update keeps the old speed
```
Per tick: `delta = (sign_extend(table[wave][phase]) << 2) * (param & 0F) >> 7`;
`actual_period = current_period + delta`; `phase = (phase + (param >> 4)) mod 64`.
`Uxy` omits the `<< 2`. `Rxy` omits it too, adds to `current_vol`, clamps 0–64 and writes
`actual_vol`.

**`Sxy` sub-effects:** `S1x` glissando; `S2x` set finetune from the monotonic
`FineTuneTable` (**a different ordering** from the MOD finetune table — do not share
them); `S3x`/`S4x` waveform select where `x >= 3` subtracts 4 **and resets the shared
phase**, so 3 and 7 both select the random table and `S4x` disturbs vibrato phase;
`S8x` pan; `SBx` pattern loop; `SCx` note cut; `SDx` note delay; `SEx` pattern delay;
`SFx` ignored.

**`SDx` note delay is the clever one.** The tick-0 handler saves the *entire* dirty-flag
byte into `special_value` and zeroes it, so the note, volume and pitch the row already
latched simply never reach the voice. The tick handler counts down and, at 0, restores
the saved flags. Implement it this way rather than inventing a "pending note" mechanism —
the original's approach is simpler and is what makes `SDx` interact correctly with
everything else on the row.

### 4. Portamento-versus-retrigger

Reproduce the tick-0 decision from `__UpdateTracker` (analysis §4, "`Gxx` tone
portamento"):

```
if voice_is_sounding && (command == G || command == L) && instrument != none:
        set target note/period only            // do NOT retrigger
else:   current = target = actual = period; sample_offset = 0; dirty |= SAMPLE
```

This depends on whether the voice is **still sounding** — in the original,
`_ActiveFlag`, maintained by the driver. So a portamento onto a channel whose one-shot
sample has already finished correctly behaves as a fresh trigger. Wire this to the voice
pool's actual state, not to a sequencer-side guess.

### 5. Instrument change

A new instrument number always reloads the reference rate and resets
`current_vol = actual_vol = ` the sample's default volume, flagging `DirtyBits::VOLUME`,
**even with no note**. A sample volume **> 64 is coerced to 0**, not clamped to 64 — that
is ST3 behaviour and is reproduced. Read the **full 32-bit** C2SPD (deviation **D7**).

### 6. Pitch clipping

Port `ClipPitch` (analysis §4):
```
if glissando_enabled:
        amiga = actual_period * reference_rate / (8363*16)
        scan Period_Table doubling per octave for the nearest entry
        actual_period = nearest * 8363*16 >> octave / reference_rate     (minimum 1)
if amiga_limits (generalflags bit 4):
        clamp actual_period to [452 .. 3424]
```

### 7. The deviations

Implement **D1, D2, D3, D6 and D7** canonically per `03-accuracy-policy.md` §3, with a
comment at each site naming the deviation. D1 and D5 are already handled in the core
tables (M0-A2). D4 does not apply — the retro mixer is out of scope.

If implementation uncovers an **eighth** deviation, add it to §3 of the accuracy policy
in the same commit. A deviation that is not written down is a defect, not a decision.

## Research points

1. Where ST3's real behaviour is known to differ from what the assembly does, beyond
   D1–D7. The OpenMPT wiki's ST3 compatibility notes are the best secondary source. Any
   difference found is a new accuracy-policy entry, not a silent choice.
2. `Bxx` and `Cxx` occurring on the same row — the original's order-advance path implies
   an interaction; confirm it against ST3 before locking it in.
3. `SBx` pattern loop interacting with `Bxx`/`Cxx` on the same row. This is a classic
   source of divergence between players.

## Verification

Per-effect unit tests driving the sequencer over a hand-built pattern and asserting the
resulting per-channel state (period, volume, pan, sample position) tick by tick. Keep
`assert` calls on one line per the working agreements. At minimum:

- `Dxy`: each of the four classification branches, including `DF0` and `D0F` falling
  through to the normal path; high-nibble-wins per tick; clamping at 0 and 64.
- Shared memory: `D05` then `E00` uses 5 as the porta parameter. Assert it.
- Shared vibrato phase: `H41` for four ticks, then `S40`, then `R41` — assert the phase
  reset and that tremolo continues from 0.
- `Cxx`: `C10` breaks to row **10**, not row 16.
- `SDx`: a note with `SD3` sounds on tick 3, and its volume and pan arrive with it.
- `SEx`: `SE2` runs the row's per-tick effects for three row-lengths without re-fetching
  notes; `RowClock::total_ticks()` reflects it.
- `SBx`: a two-iteration loop plays the range exactly twice.
- `Qxy`: retrigger phase carries across a row boundary; each of the four multiplicative
  volume cases.
- Portamento onto a finished one-shot retriggers; onto a sounding voice it does not.
- Deviations: `Jxy` with a nibble of 13 produces a valid in-range note (D2, not a table
  overrun); vibrato waveform 2 at phases 62 and 63 gives 255 (D1); `A00` is ignored (D6).

**Owner check**: at least three real `.s3m` modules that use vibrato, tremolo, portamento
and the `Sxy` family sound correct.

## Out of scope

MOD and MTM semantics (M2). Trace diffing and the conformance corpus (M2). XM/IT effects.
Any effect the original mapped to `S_FX_0`.
