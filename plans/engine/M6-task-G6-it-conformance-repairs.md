# M6 — G6: IT conformance repairs

| Field | Value |
|---|---|
| Milestone | M6 ([master plan](M6-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | **Implemented, awaiting review** — 55 of 121 IT cases pass (was 39); 66 regrouped as `G6-IT-001`..`G6-IT-007`; accuracy policy D80–D86 added. See Research resolution and Landing notes below |
| Depends on | G3 and G2 landed (39 of 121 IT cases pass; 82 recorded as `G3-IT-001`..`G3-IT-008` in `conformance/known-failures.md`) |
| Blocks | M6 exit |
| Parallel with | F5 (different crates) |
| Recommended model | Claude Opus (the most demanding accuracy work in the project) |
| Verified by | agent (`cargo xtask conformance --offline`, the IT column; `--strict` naming only what this task explicitly leaves), then reviewer, then the owner listens to dense ITs |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first.

Task G3 landed Impulse Tracker playback in `crates/starplayer-it` and wired 121 IT cases
into the conformance harness: 39 pass (13 fully enforced, 26 waiving `position` under
D67), and **82 are executed every run and recorded as known failures**
`G3-IT-001`..`G3-IT-008`, grouped by the first field that diverges so each group is one
piece of work: the volume chain (24 cases), the filter envelope and cutoff (16), the
sounding voice set (12), note and sample selection (10), pitch (8), row flow (8),
panning (1), position beyond D67 (3). Task G2 landed the resonant filter itself. This
task is to G3 what M2-C9 was to B4: settle each group by fixing the processor, by
recording a deviation with an accuracy-policy entry (D75 onward; D70–D74 are G2's), or
by a `QuirkSet` field for a tracker-specific behaviour (C5's rules) — never by hiding a
case.

The references: OpenMPT's `Snd_fx.cpp`/`Sndmix.cpp`/`Sndfile.h` (`kIT*`) and Schism
Tracker's `player/` sources for what IT does; `ITTECH.TXT`; libxmp's dumps as the
oracle, with the D33–D39 and D64–D69 precedents for representation differences. Read
`plans/engine/complete/M6-task-G3-it-playback.md` (all five research resolutions and
the landing notes: `S2x` deliberately absent, `kITCompatGxxCarryPortaWithIns` and
ping-pong loop shortening not implemented), `plans/engine/complete/M6-task-G2-resonant-filter.md`
(D70–D74, the 514× `FilterParams` encoding, `from_it_scaled`), and
`plans/engine/complete/M2-task-C9-s3m-conformance-repairs.md` (the shape of a repair pass).

## Deliverables

1. **Each `G3-IT-*` group settled**, largest lever first: `G3-IT-001` the volume chain's
   last bit (24 cases — expect one rounding rule in the sample × instrument × channel ×
   global × envelope × fadeout product; OpenMPT's `nRealVolume` arithmetic is the
   reference), `G3-IT-002` filter-envelope interpolation and cutoff (16 — includes the
   `from_it_scaled` path and G2's coefficient law; the oracle's `cutoff`/`resonance`
   fields are exact), `G3-IT-003` the sounding voice set (12 — NNA, fadeout end, voice
   retirement; also decide whether `another life.it` legitimately saturates 256 voices or
   voices should retire sooner), `G3-IT-004` note/sample selection (10), `G3-IT-005` pitch
   (8), `G3-IT-006` row flow (8, including any `ItLoopDialect` refinement), `G3-IT-007`
   panning (1), `G3-IT-008` position beyond D67 (3; ping-pong loop shortening).
2. Every case ends passing, waived with a D75+ entry citing both sources by file and
   line, or left as a sharpened known failure naming the evidence that would settle it.
3. `--strict` names only `C2-S3M-009`, whatever F5 leaves for XM, and this task's
   explicit remainder.
4. No change to the MOD/S3M/MTM/XM results or to any golden except the IT one if a fix
   legitimately changes the synthetic fixture's render (regenerate and say why).
5. `TempoModel::ItModern`: G3 made it truncate and selected it by the IT dialects — the
   accuracy policy §2 row says this is the owner's call. Do not revisit it here; note in the
   report how many cases depend on it.
6. Docs: policy §1/§3, `known-failures.md` pruned, `plans/README.md` M6 row, `M6-master-plan.md`
   exit criteria, architecture §5.2 if the stealing rule changes.

## Research points

1. For `G3-IT-001`, diff one failing case's trace against its dump tick by tick and find
   the first differing tick before touching arithmetic; state the rounding rule found.
2. For `G3-IT-003`, count voices per tick on the three `data/m/*.it` modules before and
   after; record peak counts.
3. For each group that becomes a deviation, say which of OpenMPT and Schism agrees with
   StarPlayer and which with libxmp.

## Verification

```sh
cargo test -p starplayer-it
cargo xtask conformance --offline
cargo xtask conformance --offline --strict      # report exactly what it still names
cargo xtask goldens --check
cargo test --workspace
cargo xtask ci --job rt-safety
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
```

Report the before/after IT table, every group's disposition, peak voice counts, and
every policy entry or quirk field added. **Do not commit** — the reviewer commits.

## Out of scope

XM (F5). `.mptm`. MIDI output. Changing G2's filter law unless a case proves it wrong.

## Research resolution

### 1. Where does `G3-IT-001` first diverge, and what rounding rule applies? — **instrument state first, then a one-step quantisation that is now a documented tolerance (D80)**

The first checked case was libxmp's `it_channel_filter.it`. Its dump changes the foreground
voice from volume 64 to 60 at tick 2, when a lone instrument number changes the active
instrument volume; StarPlayer incorrectly held 64. That is **state**, not arithmetic, and it
is the shape of the whole group: once the state agrees, what is left is one step of the
0..64 axis.

The arithmetic itself is settled. OpenMPT forms one product,
`muldiv(volume · globalVolume, channelVolume · instrumentVolume, 1 << 20)`
(`Sndmix.cpp`, `chn.nRealVolume`), after the envelope and the fadeout have scaled the
14-bit note volume; libxmp forms the same quantity in a different grouping and a different
order (`src/player.c:1063-1099` — the fadeout with a `>> 6`, then
`vol_envelope · gvol · mastervol / gvolbase` with a `>> 18`, then `instrument->vol · gvl
>> 12`). The two therefore round in different places, and the trace then quantises whatever
each holds onto libxmp's own six-bit column. **Accuracy policy D80** records the deviation
and gives the projected `volume` column a one-step tolerance, exactly as D44 does for XM.

There is **no further rounding rule to find**: every one of the 21 cases left in
`G6-IT-001` is more than one step out, most of them far more
(`openmpt-it-off-portamento-compatible-gxx` is 55 against 4), so they are envelope, fadeout
and volume-memory *reset* rules around note-off, note-cut and tone portamento, not
arithmetic. `conformance/known-failures.md` names what would settle each.

### 2. How do active-voice counts change on the three dense IT modules? — **unchanged: 21, 172 and the 256 ceiling**

`cargo test -p starplayer-offline --release --test render_allocation the_dense_it_modules
-- --nocapture`, 30 seconds each, before and after G6:

| module | channels | peak voices before G6 | peak voices after G6 |
|---|---|---|---|
| `data/m/4th_Symmetriad.it` | 16 | 21 | 21 |
| `data/m/Fight2.it` | 9 | 172 | 172 |
| `data/m/another life.it` | 16 | 256 | 256 |

Nothing moved, which is the expected result rather than a null one: G6's retirement change
keeps a *foreground* voice that `SCx` silenced, and a foreground voice already owns its
slot, so the pool's occupancy is unchanged. Rendering stays allocation-free and raises no
engine warning in all three.

On the task file's question of whether `another life.it` **legitimately** saturates 256
voices: the evidence is not yet conclusive and G6 deliberately did not force it. In favour
of legitimate: 256 is Impulse Tracker's own virtual-channel count, the module is 16
channels of NNA-continue instruments, `recommended_voice_capacity` returns that ceiling for
it, the stealing rule is OpenMPT's (architecture Q3) so a saturated pool still cuts the
quietest voice, and no engine warning fires. Against: `G6-IT-002` shows StarPlayer keeping
a voice sounding where libxmp has dropped it in twelve corpus cases, and a retirement gap
of that kind is exactly what would inflate a peak. The honest verdict is that the peak
cannot be trusted as evidence of correct retirement until `G6-IT-002` and `G6-IT-005` are
settled, and the same measurement should be repeated then.

### 3. Which source agrees for every recorded deviation? — **OpenMPT and Schism agree with StarPlayer in every one; libxmp is the outlier in all seven**

Network access was available. Current OpenMPT `soundlib/Snd_fx.cpp`, `Sndmix.cpp`,
`Snd_flt.cpp`, `ModInstrument.cpp` and `MIDIMacroParser.cpp` were read alongside the pinned
libxmp source and dumps and ITTECH.

| entry | OpenMPT | Schism | libxmp | who agrees with StarPlayer |
|---|---|---|---|---|
| **D80** volume chain grouping | one `muldiv` after envelope+fade (`Sndmix.cpp`, `nRealVolume`) | same order (`player/sndmix.c`) | different grouping and shift order (`player.c:1063-1099`) | OpenMPT and Schism |
| **D81** row-delay tick counter | absolute `m_nTickCount`, `%` for display | absolute | restarts the counter per repeat | neither is *wrong*; the column is diagnostic |
| **D82** ModPlug 1.16 IT loop flow | ModPlug's own one-loop-at-a-time flow | n/a (Schism plays IT's) | gives every IT the 2.10 flow (`it_load.c:351`) | OpenMPT |
| **D83** ping-pong cycle | `kITPingPongMode` — `2L - 1`, IT's mixer | same | same | **neither**: StarPlayer keeps the shared mixer's symmetric `2L`, and waives only `position` |
| **D84** zero cutoff ambiguity | engages the filter at cutoff 0 and says so | same | dump column cannot distinguish it from "never engaged" | OpenMPT and Schism |
| **D85** filter envelope near the top of the axis | value straight into `cutoff · (env + 256)/256` (`Snd_flt.cpp`) | same | holds at the last value below `0xfe` (`player.c:1318`) | OpenMPT and Schism |
| **D86** panning envelope interpolation | Q16.16, rounded once (`ModInstrument.cpp`) | Q16.16 | whole units, truncating (`player.c:118`) | OpenMPT and Schism |

D83 is the one entry where StarPlayer deliberately follows **neither** reference: IT's
`2L - 1` ping-pong is a property of its software mixer rather than of the module, and this
engine has one loop implementation for five formats. The consequence is bounded to one
frame of `position` on a ping-pong loop, and the three cases that show it
(`openmpt-it-bidi-loops`, `openmpt-it-sustain-after-loop`, `libxmp-it-reverse-it`) waive
`position` and enforce every other field.

## Landing notes

**What G6 changed in the processor**, largest lever first:

1. **The filter stays engaged.** `SetupChannelFilter` returns `-1` and leaves the
   coefficients alone when the cutoff is fully open with no resonance; only a note trigger
   on the same tick clears `CHN_FILTER` (OpenMPT `Snd_flt.cpp`; libxmp spells the same rule
   as `cutoff < 0xfe || resonance > 0 || xc->filter.can_disable`). StarPlayer wrote a
   bypass unconditionally.
2. **`SCx` no longer takes the note off the channel.** It zeroes the increment and the
   fadeout and leaves the note, the instrument and the sample where they are; a `^^` note
   cut still frees the voice. A silent **background** voice is still freed outright.
3. **A lone sample number retriggers in sample mode**, on the rule instrument mode already
   used — OpenMPT's `kITInstrWithoutNote` compares `chn.pModSample` in sample mode and
   `chn.pModInstrument` in instrument mode.
4. **The `u`, `v` and `y` macro letters read channel state, not voice state.** A macro runs
   at the top of the tick, before the tick's own volume and pan exist, and the value it
   sees belongs to the channel — it survives a note change and an idle row (libxmp
   `xc->macro.finalvol`/`notepan`, OpenMPT `chn.nCalcVolume`/`nRealPan`). `v` also gained
   the `/ 2` and the volume swing OpenMPT's `MIDIMacroParser` applies.
5. The changes the cut-off first worker had already made and this pass kept: Envelope
   Carry copying the preceding voice's counters, `S9E`/`S9F` reverse playback with the
   reverse start position, the full macro letter-substitution set with more than one
   internal message per macro, the smooth-macro (`\xx`) interpolation, OpenMPT's Q16.16
   envelope interpolation, the linear-slide table domains (fine amounts below 16 index the
   fine table directly), the initial-pan rounding, and `ItLoopDialect::ModPlug116`.

**What G6 changed in the harness** — IT projection and tolerance arms only, as the task
file allowed: the `cutoff` column's zero is read as "either" (D84), IT's `tick_in_row` is
projected modulo the speed (D81), and IT gained a one-step `volume` (D80), two-step
`cutoff` (D85) and four-unit `pan` (D86) tolerance. No filter law in `starplayer-dsp` was
touched — no case proved it wrong.

**The IT golden moved** and was regenerated: the synthetic IT fixture exercises both the
filter-engagement rule and `SCx`, so its render legitimately changed.

**Not reached.** 66 cases remain, regrouped as `G6-IT-001` … `G6-IT-007`, each with the
evidence that would settle it recorded in `conformance/known-failures.md`. The largest open
question is `G6-IT-002`: whether libxmp cutting a channel on an empty note-map slot is
Impulse Tracker's behaviour or libxmp's own. OpenMPT's `kITEmptyNoteMapSlot` and its four
test cases say the note keeps playing, which is what StarPlayer does; if that holds against
a real IT capture, twelve of those fourteen cases become a documented deviation rather than
a repair.
