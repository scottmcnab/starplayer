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
| ~~MOD loop enabled only when loop length > 4~~ | `ConvertSamps` | **Not reproduced.** This was the DOS converter's own rule, not a MOD convention: ProTracker loops whenever the repeat length is more than one word (≥ 4 bytes), as libxmp's `loop_size > 1` does, so a 2-word loop is a real, audible loop. C3b moved the loader to `loop_length >= 4` |
| MOD `Dxx` pattern break read as **BCD**: `(xx>>4)*10 + (xx&0F)`, wrapping to row 0 past 63 | PT 2.3D `mt_PatternBreak` | ProTracker behaviour. MultiTracker reads the same byte as hexadecimal, which is why this is the `QuirkSet` field `mod_break_parameter` rather than a constant |
| MOD `F00` ends the song | PT 2.3D `mt_SetSpeed` | ProTracker behaviour. MultiTracker has no such rule and libxmp ignores a zero speed parameter outright, so this is the `QuirkSet` field `mod_f00_stops_song` |
| MOD LRRL channel panning map (`0,8,9,1,2,10,11,3,…`) | `CHANNELSETTINGS` | The Amiga interleave |
| MOD Amiga-limits flag derived from whether the whole song stays in octaves 3–5 | `ConvertValues` `limitflag` | Clever and correct: extended-range MODs are not clamped |
| MOD/MTM finetune → C2SPD tables (two different orderings: signed nibble for MOD, monotonic for `S2x`) | `C2SPD_Table`, `FineTuneTable` | Both are correct for their context |
| Sample sign conventions: MOD signed (XOR 0x80 on load), S3M and MTM unsigned | `CopySamples` `Temp_XOR` | Format facts |
| XM arpeggio reads `arpeggioTab[tick & 31]`, whose real length is **16**: a speed above 16 reads the sixteen bytes that follow it in FastTracker 2's binary, which are the start of the vibrato table | `ft2_replayer.c` `arpeggio`; `ft2_tables.c` `arpeggioTab`; OpenMPT `kFT2Arpeggio` | FastTracker 2 behaviour, and audible — every overflow byte above 1 selects the arpeggio's second nibble, which is why FT2 arpeggios at high speeds sound the way they do (`openmpt/xm/Arpeggio.xm`) |
| XM tremolo's **ramp** waveform takes its sign from the *vibrato* position, not the tremolo's | `ft2_replayer.c` `tremolo` (`FT2 bug, should've been ch->tremoloPos`) | FastTracker 2 behaviour. OpenMPT and libxmp both use the tremolo position, which is deviation D47 |
| XM `E60` marks the loop target and `E6x` counts **down** per channel, and a `Bxx` or `Dxx` anywhere on the same row wins over the loop jump while the loop's own target row replaces the jump's destination row | `ft2_replayer.c` `patternLoop` / `positionJump` / `patternBreak` / `getNextPos`, all through `song.pBreakPos`; OpenMPT `kFT2PatternLoopWithJumps` | FastTracker 2 behaviour, spelled as `PatternFlow::generic()` with `shared_break` |
| XM sample finetune is quantised to **sixteen** steps: the period table is indexed by `(finetune >> 3) + 16` | `ft2_replayer.c` `triggerNote`; OpenMPT `kFT2FinetunePrecision` | FastTracker 2 behaviour. libxmp interpolates `finetune / 128` continuously, which is deviation D42 |
| An XM note whose transposed value leaves C-0..B-9 updates the channel's instrument and sample but **not** its note or period, and `B-(-1)` — a transpose landing exactly one semitone below C-0 — updates the key but not the note | `ft2_replayer.c` `triggerNote` (`note += relativeNote; if (note >= 10*12) return;`); OpenMPT `kFT2Transpose` | FastTracker 2 behaviour (`openmpt/xm/NoteLimit.xm`, `data/ft2_note_range.xm`) |
| An XM note whose instrument, sub-instrument or sample is unusable **cuts** the channel rather than leaving the previous note sounding, and takes volume 0 and pan 0x80 from FastTracker 2's placeholder instrument | `ft2_replayer.c` `triggerNote` with a null `dataPtr`; libxmp `read_event.c` "playing with an active invalid sample cuts the channel" | FastTracker 2 behaviour (`data/ft2_invalid_ins_defaults.xm`) |
| XM `Rxy` retriggers on tick **zero** when the volume column reduces to zero after the tick-zero volume handlers have run — so a volume column of `$10` makes `Rxy` fire immediately | `ft2_replayer.c` `handleEffects_TickZero` (the `newVolCol` copy) | FastTracker 2 behaviour, and the reason `Rxy`'s tick-zero handler takes a *copy* of the volume column |
| XM `Lxx` sets the **panning** envelope position only when the *volume* envelope's sustain flag is set | `ft2_replayer.c` `setEnvelopePos` (`FT2 logic bug: should've been ins->panEnvFlags`); OpenMPT `kFT2SetPanEnvPos` | FastTracker 2 behaviour (`openmpt/xm/SetEnvPos.xm`) |
| XM's volume-column pan slide left of zero sets the pan to zero outright | `ft2_replayer.c` `v_PanSlideLeft` (`includes an FT2 bug`) | FastTracker 2 behaviour |
| A cell carrying both a volume-column `Mx` and an effect-column `3xx` **discards** the `3xx` parameter and applies the `Mx` rate twice in a tick | `ft2_replayer.c` `getNewNote` (the volume-column branch returns before the effect column's parameter is read); libxmp `read_event.c:522-529`; OpenMPT `GetVolCmdTonePorta` (`vol *= 2`, `clearEffectColumn`) | FastTracker 2 behaviour (`data/ft2_double_toneporta.xm`). ModPlug Tracker, MadTracker 2 and rst's SoundTracker give each column its own rate and apply the sum once, which is the `QuirkSet` field `xm_double_portamento_doubles_volume_column_rate` |
| An XM `9xx` that points past the end of the sample **stops** the channel, and the note is not picked up by a later portamento | `ft2_replayer.c` `triggerNote` (`smpStartPos`) with the mixer's own bounds test; OpenMPT `kFT2ST3OffsetOutOfRange`; libxmp `read_event.c:714-726` | FastTracker 2 behaviour (`openmpt/xm/3xx-no-old-samp.xm`). Skale Tracker does not emulate it — Armada Tanks' music breaks if it is applied — which is the `QuirkSet` field `xm_offset_past_sample_end_stops_channel` |
| An `E6x` loop jump leaves its **target row** in the shared break position, so the next pattern to end normally starts on that row rather than row zero | `ft2_replayer.c` `patternLoop` (`song.pBreakPos`) and `getNextPos`, which clears it only in the branch a position change takes; OpenMPT `kFT2LoopE60Restart` (`Snd_fx.cpp:6351`, `Sndmix.cpp:805-817`) | FastTracker 2 behaviour, and the `QuirkSet` field `xm_loop_target_becomes_next_break_row` for the trackers that do not have it. Task F6's order-list wrap let both fixtures reach it: `openmpt/xm/PatLoop-Weird.xm` now passes, and `PatLoop-Break.xm` is where the carry crosses the wrap and libxmp does not follow — **D88** |
| A note delay next to a **key-off** with no instrument column swallows the volume column's `Cxx` panning | `ft2_replayer.c` `noteDelay` writes `ch->outPan` without raising `CS_UPDATE_PAN`, and neither `triggerNote` (which returns early for a key-off) nor `resetVolumes` (which an instrument column would have run) raises it either; OpenMPT `kFT2PanWithDelayedNoteOff`; libxmp `read_event.c:511-518` | FastTracker 2 behaviour (`openmpt/xm/PanOff.xm`, `data/ft2_delay_volume_column.xm`). StarPlayer suppresses the write where FastTracker 2 suppresses only its visibility; the two could differ only on a channel whose panning envelope raises the flag every tick, and no corpus case has one |
| An instrument number naming a slot the file does not hold still reloads a fadeout — the placeholder instrument's `0x80` — and the channel keeps that placeholder until a **note** moves it | `ft2_replayer.c` `triggerNote` (`ins = instr[0]`), `triggerInstrument` (`ch->fadeoutSpeed = ins->fadeout`), `allocateInstr` → `setStdEnvelope`; libxmp `read_event.c:580-583` ("unused instruments have fade 0x80") | FastTracker 2 behaviour, and the reason an instrument-without-note row after an out-of-range one fades at the placeholder's rate rather than its own (`data/ft2_instrument_fade_update.xm`) |
| XM `Xxy` is restricted to `X1x` and `X2x`; every other sub-command is ignored | `ft2_replayer.c` `extraFinePitchSlide`; OpenMPT `kFT2RestrictXCommand` | FastTracker 2 behaviour. ModPlug's `X9E`/`X9F` extension is deviation D45 |
| An IT filter whose cutoff is fully open with no resonance keeps the coefficients it already has; only a **note trigger** on the same tick disengages it | OpenMPT `Snd_flt.cpp` `SetupChannelFilter` (`kITFilterBehaviour` returns `-1` and only `chn.triggerNote` clears `CHN_FILTER`); libxmp `player.c:1334` spells it `cutoff < 0xfe \|\| resonance > 0 \|\| xc->filter.can_disable` | Impulse Tracker behaviour. `Z7F` next to a note switches the filter off; the same `Z7F` on its own does not (`filter-reset.it`, `filter-reset-carry.it`, `filter-nna.it`) |
| IT's `SCx` zeroes the voice's increment and its fadeout but leaves the **note, instrument and sample on the channel**; a `^^` note cut takes the channel away | OpenMPT `Snd_fx.cpp` `NoteCut` under `kITSCxStopsSample`; libxmp reclaims a zero-volume voice only past the module's own tracks (`virtual.c:325`) | Impulse Tracker behaviour, and the reason a lone sample or instrument number after an `SCx` retriggers the note rather than doing nothing (`scx.it`, `SCx-Reset.it`) |
| An IT MIDI macro's `u`, `v` and `y` letters read the channel's **previous tick** volume and pan, not the voice's current one | OpenMPT `MIDIMacroParser.cpp` (`chn.nCalcVolume`, `chn.nRealPan`, both written at the end of the tick); libxmp `xc->macro.finalvol`/`notepan` | Impulse Tracker behaviour. The macro runs at the top of the tick, so the value belongs to the channel and survives a note change and an idle row (`fltmacro.it`, `Volume-Macro-Letters.it`) |
| An IT instrument's empty note-map slot leaves the **previous note playing** rather than cutting the channel | OpenMPT `Snd_fx.cpp` `NoteChange` under `kITEmptyNoteMapSlot` (`emptyslot.it`, `PortaInsNum.it`, `gxsmp.it`, `gxsmp2.it`) | Impulse Tracker behaviour per OpenMPT. libxmp's dumps show the channel cut, which is `conformance/known-failures.md` `G6-IT-002` and the open question G6 could not settle |

## 2. Deliberate quirks reproduced only under `quirks-starplayer`

Behaviour that is authentically the original's but that a modern default should not
have. Available behind the `quirks-starplayer` feature / `QuirkSet` profile, off by
default.

| Quirk | Default | Under `quirks-starplayer` |
|---|---|---|
| Tick length `(rate * 10 / bpm) >> 2`, truncating twice — ~1.3 s of drift over a 4-minute song at 130 BPM | `TempoModel::ExactFixedPoint` — `rate * 2.5 / bpm` in Q32.32, accumulated, drift-free | `TempoModel::St3Truncating` |
| Impulse Tracker's tick length `rate * 5 / (2 * bpm)`, truncated once to a whole output frame and not carried | `TempoModel::ItModern` **for IT modules only** — see below | unchanged |
| MOD and MTM interpreted through an in-memory S3M conversion | Native per-format effect processors | Not offered — see §4 |

The second row is a **format** decision rather than a profile one, and it is the answer to
task G3's research point 4. Impulse Tracker reloads a whole-sample gap length every tick
and does not carry the remainder; so does every replayer measured against it — libxmp
truncates the same expression to an `int` (`src/mixer.c:440`) and OpenMPT's classic path
does the same. The project's default tempo model is drift-free because a drifting clock is
a defect in a *modern* player, but for IT the drift is **observable output**: a voice's
sample position after N ticks is `step · Σ frames_per_tick`, and the corpus compares that
position frame by frame. Both numbers were measured on the pinned corpus at the same
commit: under `ExactFixedPoint` the IT set gives 10 passes and 59 cases whose first
divergence is `position`; under `ItModern` it gives 13 passes and the same category shrinks
once the harness pairs on libxmp's own reporting clock. libxmp keeps two clocks that
disagree with each other — its rendered tick is truncated while the `time` column every
record carries accumulates the exact value in a `double` — so the conformance adapter pairs
on the exact model (it is modelling libxmp) while the engine renders on the truncated one
(it is modelling Impulse Tracker). MOD, MTM and S3M are unaffected: `ItModern` is reached
only from the four `FormatDialect::ImpulseTracker*` variants.

`QuirkSet::canonical()` and `QuirkSet::starplayer_classic()` differ in exactly the first
row — the tempo model — and a unit test in `starplayer-core`'s `quirks` module asserts
that field by field, so a quirk cannot be added to the profile without this table being
updated. The second row is not a field at all: it is §4's "not offered". The
`quirks-starplayer` feature is read in exactly one place, `QuirkSet::profile_default()`,
because every §2 quirk is data rather than a code path and there is nothing left for a
feature gate to remove.

### Tracker dialects

A **dialect** is not a profile. A profile is what the *host* chooses; a dialect is what
the *file* says, detected from its header by the loader, stored in
`ModuleHeader::dialect`, and mapped to a `QuirkSet` by `FormatDialect::quirks()`. The
precedence is in the type: `QuirkSelection::FromDialect` takes the loader's answer and
`QuirkSelection::Override` takes the host's, and the resolved set is fixed for the
lifetime of the loaded module. There is no global and no mid-playback setter.

`mod_timing` is the one exception to "detected from its header by the loader", and it is
an exception because the header genuinely cannot answer. ProTracker could clock its
replayer from the CIA timer or from the vertical blank, and the `M.K.` tag covers both;
only `M&K!` and `N.T.` — NoiseTracker, which has no CIA mode at all — settle it from the
header. What *can* answer is the pattern data, and failing that the song's own length:
reading a VBlank module as CIA turns an `Fxx` of 32 or more from a long row into a slow
tempo, which usually makes the song several times longer. So the loader gathers the cheap
evidence and `starplayer::scan_song` — which already scans every module a host loads —
decides, handing the host both the timeline and the `QuirkSet` it was measured under. The
`FormatDialect` is still set once from the header and never revised; the timing arrives as
a `QuirkSelection::Override`, the same route any other host decision takes.

| Field | Detected from | Values |
|---|---|---|
| `s3m_pattern_loop` | S3M `Cwt/v` (`0x28`), reproducing libxmp `src/loaders/s3m_load.c:390-432` | `ScreamTracker321` (default), `ScreamTracker301` (`< 0x1303`), `ModPlug116` (`0x1320` with libxmp's ModPlug fingerprint), `ImagoOrpheus` (high nibble 2, but not `0x2013`, which is PlayerPRO byte-swapped) |
| `mod_pattern_loop` | the MOD tag at offset 1080 | `ProTracker` (default), `Octalyser` (`CD61`, `CD81`), `DigitalTracker` (`FA04`, `FA06`, `FA08`) |
| `mod_timing` | the MOD tag (`M&K!` / `N.T.` are VBlank-only), then the pattern cells, then — where those are ambiguous — a comparison of the song's length under both timings, reproducing libxmp `src/loaders/mod_load.c:816-950` and `src/scan.c:50` and `:671-708` | `Cia` (default), `VBlank` |
| `it_pattern_loop` | IT `Cwt/v`, reproducing libxmp `src/loaders/it_load.c:394-400` | `ImpulseTracker210` (default), `ImpulseTracker200`, `ImpulseTracker104`, `ImpulseTracker100` |
| `xm_pattern_loop` | the XM tracker name, plus ModPlug Tracker's own tells, reproducing libxmp `src/loaders/xm_load.c:846-893` and `:936-1013` | `FastTracker2` (default), `Generic`, `ModPlug116`, `SkaleTracker` |
| `xm_double_portamento_doubles_volume_column_rate`, `xm_offset_past_sample_end_stops_channel`, `xm_loop_target_becomes_next_break_row` | the same tracker name: on for FastTracker 2, its bug-compatible clones and OpenMPT, off for everything else — libxmp's one `QUIRK_FT2BUGS` bit (`src/loaders/xm_load.c:855-885`), and OpenMPT's `m_playBehaviour.reset()` for a ModPlug-made XM (`Load_xm.cpp:1046-1050`) | `true` (default), `false` |

The first two map to a `PatternFlow` — fifteen named booleans whose meanings are libxmp's
`FLOW_LOOP_*` bits, because the pinned oracle dumps were generated by exactly that code —
and `starplayer-engine`'s `PatternFlowState` executes it. The differences are not only in
how a loop jump interacts with a break or jump on the same row: ProTracker's loop target
and counter are **per channel** while Scream Tracker 3, Octalyser and Digital Tracker keep
**one of each for the whole song**, ModPlug 1.16 starts a loop only while no other channel
is looping, Digital Tracker executes only the first `E6x` on a row, Octalyser ignores an
`E60` while a loop is running, and Scream Tracker 3.01 makes an `SBx` with no `SB0` before
it target its own row.

The MOD timing detection, in libxmp's order: a `M&K!` or `N.T.` tag is VBlank outright;
otherwise detection runs for `M.K.` and `M!K!` only, and is turned off again by a sample
header of 32768 words or more (an OpenMPT file, not an Amiga one). A row carrying both a
low and a high `Fxx` is CIA. Eight orders or more with every high `Fxx` confined to a
pattern the last two orders play, the last such value not `0x7D`, is VBlank — the
silence-at-the-end idiom. Any remaining high `Fxx` asks for the length comparison: scan as
CIA, and if one pass runs eight minutes or longer, or the scan ran out its budget, scan
again as VBlank and keep the shorter, ties to CIA. A deliberately slow short song is
therefore never sped up. D41 records what the original DOS player did instead.

The dialect fields are `s3m_pattern_loop`, `mod_pattern_loop`, `mod_timing`,
`it_pattern_loop`, `xm_pattern_loop` and the three XM booleans above. Every other
`QuirkSet` field is either a §1 entry above (`mod_break_parameter`, `mod_f00_stops_song`),
a §3 deviation (`mod_paula_clock` is D14, `protracker_sample_swap_at_boundary` is D12,
`protracker_tremolo_ramp_from_vibrato_phase` is D20) or this section's tempo model.

The XM detection, in libxmp's order: the tracker name at offset `0x26` is compared whole.
`OpenMPT ` and `MilkyTracker` prefixes, `Fasttracker II clone`, and `FastTracker v2.00   `
with a header size of 276 are FastTracker 2 and its bug-compatible clones; `Skale Tracker`
and `Sk@le Tracker` — NUL-terminated, as libxmp's `strcmp` requires — are their own dialect;
`FastTracker v 2.00  `, with the extra mid-string space, is ModPlug Tracker 1.0; and
everything else is `FormatDialect::UnknownXm`, which carries none of FastTracker 2's replay
bugs. One case the name cannot settle is a ModPlug Tracker 1.16 file that signs itself
`FastTracker v2.00   `, so the loader revises that one answer after reading the body, from
the two tells libxmp's `is_mpt_116` uses: an unused instrument's zero-filled `0x107`-byte
header, or one of ModPlug's own `text` / `MIDI` / `PNAM` / `CNAM` / `CHFX` / `XTPM` chunks
after the sample data.

`FormatDialect::MilkyTracker` is the one place StarPlayer follows OpenMPT rather than
libxmp: libxmp turns `QUIRK_FT2BUGS` off for it only because its test matches the exact
FastTracker 2 name, while OpenMPT keeps every `kFT2*` behaviour and changes only the mix
levels (`Load_xm.cpp:658-663`). MilkyTracker is a deliberate FastTracker 2 clone, and
neither of the two MilkyTracker corpus cases turns on the difference.

`FormatDialect::UnknownXm` covers MadTracker 2 and rst's SoundTracker as well as an
unrecognised name. OpenMPT splits those two finer — MadTracker keeps every FastTracker 2
behaviour but `kFT2PortaNoNote` and `kFT2Arpeggio`, Skale Tracker keeps every one but
`kFT2ST3OffsetOutOfRange` and `kFT2Arpeggio` — and a corpus case that turned on one of
those would justify its own variant. None does: the three fixtures that name these
trackers, `data/mt2_xm_double_toneporta.xm`, `data/rstst_double_toneporta.xm` and
`openmpt/xm/3xx-no-old-samp-noft.xm`, all agree with libxmp's single bit.

## 3. Documented deviations from the reference implementations

Entries D1–D9 and D21–D32 are coding defects in the original assembly and are implemented
canonically. D33–D39 are representation differences in the libxmp oracle rather than
disagreements about Scream Tracker 3, D40 is a disagreement between the original and
both secondary oracles that no available reference can settle, and D41 is a MOD boundary
the original got wrong in a way that happened to help. The rest record deliberate
determinism/architecture choices or visible conformance gaps; none may be hidden behind
an accuracy claim. D42–D47, D75–D79 and D87–D88 are M5's XM entries, and D80–D86 are M6's IT ones;
both have the same shape as D33–D39:
they are places where **libxmp**, the conformance oracle, represents or computes something
differently from FastTracker 2, and the accuracy policy's own rule for XM — "the format
specifications and OpenMPT's documented compatibility behaviour are the reference" — makes
FastTracker 2, read through `ft2-clone`'s `src/ft2_replayer.c`, the thing to match. D79 is
the exception: it is a gap in the pinned corpus rather than a disagreement about
FastTracker 2.

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
| D12 | ProTracker 1/2 queues an instrument-only or tone-portamento sample swap and changes the DMA sample pointer at the current sample's end or loop boundary | OpenMPT `PortaSmpChange_PT`, `PortaSwapPT`, `PTInstrSwap`, `PTStoppedSwap`, `PTSwapEmpty`, `PTSwapNoLoop`; libxmp `virt_queuepatch` / `mixer.c:700-755` | **Implemented by C3b**, not a deviation any more. Volume and finetune apply immediately; the sample itself is one fixed-size `Option<SampleRegion>` per voice, adopted by the render kernel at the forward-loop wrap or the one-shot end — no allocation, no lock, and no branch on the hot path when the slot is empty. A one-shot queued behind a one-shot, and ProTracker's null (empty-slot) sample, both stop the voice at that boundary instead of swapping. A channel whose sample has already run out has no boundary left to wait for, so PT starts the queued sample at once, from its loop point if the channel still owned the voice and from frame zero if the channel had been cut. Two residual differences against the mixer-dump oracle are representational rather than behavioural, and both belong to D18's family: on the tick where a stopped channel is revived libxmp reports the *previous* sample's parked position before its own in-tick swap, and where a queued stop or swap lands inside a tick interval libxmp omits the channel for that whole tick while C1 snapshots the voice at the boundary before it. `openmpt-mod-stopped-swap` waives `position` for the first; `openmpt-mod-swap-no-loop` and `openmpt-mod-portamento-sample-change-pt` remain excluded for the second, which the adapter's one-shot D18 projection cannot recognise because the channel's reported sample is already the queued replacement |
| D13 | A ProTracker `EDx` note delay greater than the current speed can leak into the next row, under narrow conditions, without restarting the sample | OpenMPT `NoteDelay-NextRow.mod` (documented-only in the pinned libxmp suite) | Clear the delayed-note latch when the next row is read. Reproducing the leak needs cross-row deferred-trigger state and an exact oracle for its continuation rules; the upstream case is not currently a libxmp frame-state gate, so it remains an explicit compatibility gap |
| D14 | libxmp represents ProTracker pitch differently from ProTracker, on the same PAL clock: it derives a finetuned period continuously as `428 · 2^(-(note + finetune/128)/12)` (162.65, 453.45) where PT reads one of the 16 integer finetune tables (163, 453), and its C-4 rate is the rounded integer `8287`, so `8287 × 428 = 3_546_836 Hz` against the exact PAL `3_546_895 Hz` (17 ppm) | libxmp `src/loaders/mod_load.c:1113-1123` — `c4rate` stays `C4_PAL_RATE` for the ProTracker/OpenMPT tracker ids, NTSC being chosen only for ScreamTracker3/FastTracker/TakeTracker/ModsGrave or `chn > 4`; `src/load_helpers.c:302`; `src/common.h:137`; PT 2.3D period tables | Keep PT's integer finetune tables and the exact PAL `3_546_895 Hz / period` clock. Both sides are PAL — every affected fixture is a 4-channel `M.K.` file, and its dump advances at PAL speed — so the residual is representational: about one source frame of position drift per tick on finetuned samples (`openmpt-mod-finetune`, `openmpt-mod-amiga-limits-finetune`, `openmpt-mod-instrument-swap`), and a 17 ppm accumulation that crosses libxmp's one-integer-sample bound only at the final compared tick of `openmpt-mod-instrument-volume`. Those four cases stay excluded; native playback is not retuned to a rounded secondary software-mixer oracle |
| D15 | libxmp's default MOD replay applies an `Fxx >= 32` tempo to the interval beginning at command tick zero; PT writes the CIA latch on that tick and the new interval begins at the following interrupt | libxmp `DelayBreak` / `VibratoReset`; PT 2.3D `setSpeed` and CIA update path | Keep PT's old-duration command interval and commit the new BPM at the next tracker event. The two mixer dumps whose first timestamp assumes libxmp timing are explicit conformance exclusions |
| D16 | libxmp's out-of-range MOD arpeggio path silences the voice when it crosses the supported Amiga range; PT indexes its physically flat period table and can reach its zero sentinel without writing channel volume | OpenMPT `ArpeggioWraparound.mod`; PT 2.3D arpeggio table access | Preserve PT's table-domain result, including period zero with unchanged channel volume. The libxmp volume-zero expectation is an explicit conformance exclusion rather than a reason to invent a volume command. Real Paula, as modelled by pt2-clone's `paulaSetPeriod`, additionally clamps any period below 113 to 113 and treats period 0 as 65536 — a near-silent ~54 Hz crawl, not a frozen DC hold. C3b applies both at step derivation. The zero rule is unconditional; the 113 floor follows the loader's Amiga-limits flag, because it is the Amiga's limit and not the format's — extended-range MODs and MultiTracker put their top octaves below 113 by design and libxmp plays them unclamped. Under Amiga limits a period below 113 is clamped up to 113, and a written zero always becomes 65536, which is what keeps a slide below the range from producing a runaway step or a DC hold |
| D17 | The default OpenMPT/libxmp `PortaSmpChange.mod` oracle deliberately selects a non-ProTracker sample-change compatibility mode | OpenMPT `PortaSmpChange.mod` and `PortaSmpChange_PT.mod` | Offer the native ProTracker 1/2 interpretation only. The default-profile case is excluded; the `_PT` case separately exercises the queued boundary swap recorded by D12 |
| D18 | libxmp's mixer-state test observes channels after `xmp_play_frame` has rendered the tick and omits a voice marked `NOTE_SAMPLE_END`; C1 traces format state at the exact event boundary before that interval is rendered | libxmp `test_player_mtm_tempo` / `compare_mixer_data.c`; C1 trace contract | Keep the event-boundary trace, where the `TEMPO.MTM` one-shot is still active at row 12 tick 4 and ends inside the following interval. The executed case is excluded rather than changing the trace contract or hiding active-set mismatches globally |
| D19 | libxmp truncates every mixer tick to `(int)(rate * 2.5 / bpm)`, discarding the fractional remainder; at high MTM tempos its integer source position differs by two frames by tick 53 | libxmp `src/mixer.c:libxmp_mixer_get_ticksize`; `TEMPO2.MTM` | Keep `TempoModel::ExactFixedPoint`, which carries the rational remainder and is required for sample-exact, drift-free timing. The adapter retains libxmp's one-frame source-position bound and the executed case is excluded instead of loosening it |
| D20 | ProTracker's `mt_Tremolo2` selects which half of the ramp waveform to read by testing `n_vibratopos` — the *vibrato* phase — instead of `n_tremolopos`, so an `E71` tremolo takes its ramp shape from the vibrato phase while its sign comes from the tremolo phase | PT 2.3D `mt_Tremolo2`; pt2-clone `tremolo` | Read the tremolo ramp from the tremolo phase, which is the canonical shape and also what libxmp does, so no corpus oracle can observe the bug either way. PT's cross-wired phase is reproduced as a `QuirkSet` field delivered by `plans/engine/complete/M2-task-C5-quirks-and-tempo-models.md`, off under `canonical()` |
| D21 | ProTracker's `mt_RetrigNote` retriggers on **tick zero** of a row that carries `E9x` and no packed note — the tick-zero guard only skips a row that *does* carry one. libxmp handles retrigger from tick one onwards, so its voice runs on instead of restarting | PT 2.3D `mt_RetrigNote`; pt2-clone `retrigNote`; libxmp `libxmp_process_fx` retrigger path | Keep PT's tick-zero retrigger. In `DelayBreak.mod` this is the whole difference: PT keeps restarting a 672-frame one-shot every tick, while libxmp lets it run out during row 1 tick 0 and drops the channel for the rest of the module. `openmpt-mod-delay-break` is excluded for it, on top of the D15 CIA-latch waiver the same case already carries |
| D22 | ProTracker's `mt_PlayVoice` latches a row's **instrument column** — volume, finetune and the sample pointer — at tick zero and only `mt_SetPeriod` defers the DMA restart for `EDx`. libxmp copies the whole event into `xc->delayed_event` and re-reads it at the delay tick, so its voice's reported instrument changes late | PT 2.3D `mt_PlayVoice` / `mt_SetPeriod`; libxmp `check_delay` (`src/player.c:762`) | Follow PT: the instrument's volume and finetune apply on the row, the note restarts at the delay tick. The audio agrees; what differs is the instrument the oracle reports for the ticks between. `openmpt-mod-portamento-swap-pt` is excluded for the three ticks of `ED3` on its row 25. This is adjacent to D13 but distinct — D13 is about a delay longer than the row |
| D23 | ProTracker's `mt_PerNop` restores a channel's un-modulated period and volume on the first *minor* tick of a row that carries a command but no note; StarPlayer's `reset_row` does it while latching the row, one tick earlier | PT 2.3D `mt_PlayVoice` / `mt_PerNop`; `crates/starplayer-mod/src/processor.rs` `reset_row` | Keep the earlier restore. It is one tick of a vibrato or tremolo offset on a note-less row, no corpus case observes it, and moving the restore into the tick path would put a per-channel "did this row carry a note" test on the hot row boundary for no audible gain. Recorded so the difference is a decision rather than an oversight |
| D24 | `ClearChannels` gives every unassigned or mono channel pan nibble **7**; Scream Tracker 3 uses **8**, which is what its own 32-byte default-pan blocks contain and what libxmp (`mod->xxc[i].pan = 0x80`) and OpenMPT both decode a centred S3M channel to | `S3MLIB.ASM` `ClearChannels` 3555–3600; libxmp `src/loaders/s3m_load.c:319-333` | `header::PAN_CENTRE` is 8. Neither 7 nor 8 is exactly centre on the 4-bit GUS balance grid — the pan law keeps `(2n-15)/15`, so 3 and 12 stay symmetric — but 8 is the value ST3 writes and the value the corpus expects |
| D25 | The original keeps one memory per effect family (`_PortaValue`, `_VolSlideValue`, `_VibValue`, `_RetrigValue`, `_ArpValue`, `_OffsetValue`). ST3 keeps **one** parameter memory per channel that every command with a non-zero parameter writes — including commands it does not implement — and that `Dxy`, `Exx`, `Fxx`, `Ixy`, `Jxy`, `Kxy`, `Lxy`, `Qxy`, `Sxy` and `Txx` read back when their own parameter is zero | OpenMPT `ParamMemory.s3m`, `NOP.s3m`; libxmp `EFFECT_MEMORY` / `EFFECT_MEMORY_S3M` under `QUIRK_ST3BUGS` | `S3mChannel::parameter_memory`, written at the head of every tick-zero command dispatch. `Gxx` and the `Hxy`/`Rxy`/`Uxy` family keep their own read memories and write the shared one, as libxmp's `EFFECT_MEMORY_SETONLY` does |
| D26 | The output period has no lower bound: `M_FX_F` pins it at 1 and plays on. ST3 clamps the **output** period to at least 64 (its period × 4 domain) while the channel period keeps sliding underneath, and stops the channel outright when the channel period reaches zero | OpenMPT `PeriodLimit.s3m`, `FreqLimits.s3m`; libxmp `src/player.c:1207-1290` | `clip_pitch` clamps `actual_period` to 64 and leaves `current_period` alone, so a later downward slide resumes from the true value; a `current_period` of zero stops the voice while leaving the instrument latched, so a following note with an empty instrument column still sounds |
| D27 | `M_FX_D` tests the high nibble first, so `D82` slides *up* by 8. ST3 gives the down nibble priority: with both nibbles set the slide is downward | OpenMPT `ParamMemory.s3m` (`D00` recalling `82` must equal `D02`); libxmp `QUIRK_VOLPDN` | `minor_volume_slide` slides down by the low nibble whenever it is non-zero. `DFy`, `DxF` and the single-nibble forms are unaffected |
| D28 | `@@tryanewnote` assigns `_SampleNum` and `_C4SPD` from the instrument column before it looks at the command, so a `Gxx`/`Lxx` row naming a different instrument changes the sounding sample. ST3 keeps the sounding sample and its C2SPD and adopts only the new instrument's default volume | OpenMPT `PortaSmpChange.s3m`; libxmp `src/read_event.c:770-790` | `latch_cell` computes the tone-portamento condition first and skips the sample and reference-rate assignment when it holds |
| D29 | `@@cutnote` clears `_SampleNum` and raises `_CHN_NewSamp`, stopping the voice. ST3's `^^^` and `SCx` only take the channel volume to zero: the sample keeps running through its loop, which is why `Qxy` cannot revive a cut channel and why a later tone portamento still has a voice to slide | OpenMPT `RetrigAfterNoteCut.s3m`; libxmp `s3m_sample_porta.s3m`, which keeps the cut voice looping at volume zero for the rest of the module | `cut_note` zeroes the channel volume and sets a `note_cut` latch that blocks `Qxy`; the voice is left alone. Audio is unchanged — a silent voice mixes to silence — but the voice lifetime and the state a later row inherits now match |
| D30 | The `SBx` pattern loop counts *up* towards the parameter, never advances its target, and lets a later channel's `Bxx`/`Cxx` replace the loop jump | libxmp `FLOW_MODE_ST3_321` (`src/flow.c`, `src/common.h:371`); libxmp `pattern_loop_st321.s3m` and `pattern_loop_st321_breakjump.s3m` | ST3.21's flow: the first `SBx` on a row stores its parameter in the one global counter and every further `SBx` on that row spends one iteration of it; when the loop ends the global target advances to the `SBx` row plus one, so a following `SBx` with no `SB0` loops the rows in between; a loop jump cancels a `Bxx`/`Cxx` already seen on the row and blocks any that follow, whichever channel carries it; and every position change resets both target and counter |
| D31 | `ClipPitch` applies the Amiga limits to `_ActualPeriod` only, so `_CurrentPeriod` keeps sliding past 113 × 4 and 856 × 4 and a slide back out starts from a runaway value | OpenMPT `AmigaLimits.s3m`; OpenMPT `Snd_fx.cpp` clamps `chn.nPeriod` | Clamp the channel period as well as the output period. The bounds themselves — 452 and 3424 — are the original's and OpenMPT's alike |
| D32 | `S_FX_S SEx` writes `_MRowDelay` unconditionally, so the rightmost channel's value wins and an `SE0` cancels a delay an earlier channel asked for | OpenMPT `PatternDelays.s3m`; libxmp `pattern delay` handling | Only the first non-zero `SEx` on a row sets the delay. ST3's own left-then-right channel evaluation order is **not** reproduced; no corpus case distinguishes it, and it is recorded here as a known remaining difference |
| D33 | `__UpdateTracker` decrements `_MRowDelay` and skips the whole row, so an `SEx` repeat is nothing but more ticks. ST3 treats the first tick of every repeat as a first tick and runs the row's tick-zero effects again, without re-triggering its notes | OpenMPT `PatternDelaysRetrig.s3m` | `TrackerProcessor::row_repeat`, defaulting to `tick` so MOD and MTM keep the original's behaviour, overridden by `S3mProcessor` to re-run the row's tick-zero effects only |
| D34 | `M_FX_I` reloads the tremor counter and then spends a whole further tick on it, so each phase runs one tick long. ST3's on phase is exactly `x` ticks and its off phase exactly `y`, counting the tick the effect starts on, and a zero nibble counts as one | OpenMPT `ParamMemory.s3m` rows 23–26; libxmp `tremor_s3m` | Reload-then-decrement in one step, with both nibbles floored at one |
| D35 | `M_FX_R` copies `M_FX_H`'s `sar eax,7` but leaves out its `sal edx,2` — the line is commented out in the source — so the tremolo swings half as far as it should | `S3MLIB.ASM` `M_FX_R` 3380–3415; ProTracker, libxmp (`/(1<<6)`) and OpenMPT all use `table × depth / 64` | `table × depth / 64`, truncating towards zero as the reference implementations do. Confirmed against the corpus: at sine phase 40 with depth 2 both StarPlayer and libxmp now remove five from the channel volume |
| D36 | libxmp derives every S3M period from the continuous `13696 / 2^(n/12)` formula where ST3 divides its 12-entry integer `Period_Table` by the sample's C2SPD; the table is up to 0.3 % away from the ideal for notes other than C (`G#` is 1076 against 1079.4), and ST3's division to an integer period quantises coarsely at very short periods | libxmp `src/period.c:libxmp_note_to_period`; `S3MLIB.ASM` `Period_Table` / `PeriodFromNote` | Keep ST3's table and its integer period. This is the S3M twin of D14: the residual is representational, showing up as a few native period units on non-C notes and as source-position drift wherever the period is short. The affected cases waive `position` (and `period` for `libxmp-s3m-pattern-loop-st321`) and enforce every other field |
| D37 | libxmp represents an S3M sample's C2SPD as a note transpose plus a finetune (`libxmp_c2spd_to_note`), so the note its mixer voice reports is a pitch-domain note — `F-8` for an `F#5` played on a 65535 Hz sample — where ST3, and StarPlayer's trace, report the note the pattern actually contains | libxmp `src/period.c:251-263`, `src/loaders/s3m_load.c:596` | Keep the tracker note. The two fixtures whose samples are not 8363 Hz (`PeriodLimit.s3m` at 65535 Hz, `PatternDelaysRetrig.s3m` at 44100 and 22050 Hz) waive `note`; every other field, the derived period included, is enforced |
| D38 | libxmp applies a `D0F` or `DF0` volume slide on tick zero as well as on every later tick, on the strength of a user report that ST3.21 "should process volume slides in all frames like ST300"; its own comment records this as a suspicion | libxmp `src/effects.c:355-378`; ST3's fine-slide classification as implemented by OpenMPT and by `S_FX_D` | Keep the canonical classification: only `DFy` with `y != 0` and `DxF` with `x != 0` act on tick zero. `libxmp-s3m-pattern-loop-st321` waives `volume` for this; the effect under test there is the pattern loop, not the slide |
| D39 | libxmp computes the S3M vibrato in its Amiga period domain and truncates the LFO with `/(1 << 9)`, discarding the two low bits ST3 keeps by working in its period × 4 domain: an `H82` at sine phase 8 moves ST3's period by 11 quarter-units and libxmp's by 8 | libxmp `src/player.c:1185-1200`; `S3MLIB.ASM` `M_FX_H` 3228–3253 | Keep ST3's `(table << 2) × depth >> 7` in the native period × 4 domain. `openmpt-s3m-parameter-memory` waives `period` and `position` for it |
| D40 | libxmp and OpenMPT classify an S3M `Dxy` volume slide by testing the **fine-up** form first — `(xy & 0F) == 0F and hi != 0` — so `DFF` slides *up* by 15. `S_FX_D` tests `> 0F0h` first, so `DFF` is a fine slide *down* by 15 | `S3MLIB.ASM` `S_FX_D` 2743-2786 (`cmp al,0F0h / jbe @@notspotvoldown`); libxmp `src/effects.c`; OpenMPT `Snd_fx.cpp` | Keep the original's order, which §1 already records as ST3's own classification: `> 0F0h` is a fine slide down, `xF` with a non-zero high nibble is a fine slide up, and `DF0` / `D0F` fall through to a normal slide. `DFF` is the one byte where the two orders disagree, and only `libxmp-s3m-pattern-loop-imf` contains it; that case waives `volume`. Resolving which of the two Scream Tracker 3 really did needs the DOS reference harness of `plans/engine/M2-task-C8-dos-reference-harness.md`, so nothing was changed on the strength of a secondary oracle |
| D41 | `ConvertValues` splits `Fxx` with an **inclusive** boundary — `cmp al,32 / jbe @speed` — so `F20` is speed 32 where ProTracker 2's `mt_SetSpeed` (`CMP.B #32 / BHS SetTempo`) makes it 32 BPM. It has no VBlank reading at all: every `Fxx` above 32 is a tempo | `STARPLAY-2.25s/S3MLIB.ASM` `ConvertValues` 1771-1772, identical at `STARPLAY/S3MLIB.ASM:1815-1816` | ProTracker's exclusive boundary, plus the VBlank detection §2 describes. The off-by-one is not copied. It is worth recording because it made the original *right by accident* on `K-P-K.MOD`'s `F20` — an intended 32-tick fermata — and still wrong on the same song's `F30`, which is above 32 even for the original and drops it to 48 BPM for the ten orders that follow. Detecting the timing gets both rows right for the right reason |
| D42 | FastTracker 2 quantises a sample's finetune to **sixteen** steps — its 1936-entry period table is indexed `note * 16 + ((finetune >> 3) + 16)` — where libxmp derives a continuous period from `(240 - (note + finetune/128)) * 16`. The two therefore play the same pattern note up to `finetune/2 - 4*(finetune >> 3)` native period units apart, at most 3.5, and the source position drifts from the first tick | `ft2_replayer.c` `triggerNote` 592-597, `ft2_tables.c` `linearPeriodLUT`; libxmp `src/period.c:184-188`; OpenMPT `kFT2FinetunePrecision` | Keep FastTracker 2's quantised table, exactly as D14 keeps ProTracker's sixteen integer finetune tables against the same continuous libxmp formula. The harness compares XM periods with a **four**-unit tolerance, which is that bound rounded up, and the affected cases waive `position` — 38 of the 93 XM cases, every one of them an OpenMPT fixture whose samples carry a finetune that is not a multiple of eight. Where glissando then rounds the period to a note, the two can land a whole semitone apart (`openmpt-xm-glissando`), and where a one-shot sample runs out the 0.18 % pitch difference moves the tick it ends on, which is why five cases additionally waive `active` |
| D43 | FastTracker 2's linear period is converted to a mixer delta through `invPeriod = (12*192*4 - period) & 0xFFFF`, so a period above **9216** — reachable only by sliding below C-0 — underflows into a shift of 26 or more and the voice falls silent. libxmp clamps its own linear period at 7680 and keeps mixing at the continuous frequency | `ft2_replayer.c` `period2Ft2Delta` 239-263 (`mask needed for FT2 period overflow quirk`); libxmp `src/player.c:1558-1561`; OpenMPT `kFT2Periods` | Reproduce FastTracker 2's wraparound: it is what `openmpt/xm/FreqWraparound.xm` exists to pin and what OpenMPT reproduces. `libxmp-xm-finefx-ft2` waives `position` for it; both sides still report the same period |
| D44 | FastTracker 2 interpolates an envelope in Q8 and **accumulates** a per-tick delta of `((y1 - y0) << 8) / (x1 - x0)`; libxmp recomputes `y0 + (y1 - y0) * (x - x0) / (x1 - x0)` in whole envelope units with a division that truncates towards zero. On a falling segment libxmp is up to one whole unit high | `ft2_replayer.c` `updateVolPanAutoVib` 1494-1560; libxmp `src/player.c:88-118` | Keep FastTracker 2's Q8 accumulation, which is both what FT2 does and the more accurate of the two. The harness compares XM volume with a **one**-unit tolerance and XM pan with **four** — one envelope unit reaches four pan units through FT2's `((envelope - 32*256) * panMul) >> 16`, whose multiplier is 1024 at the centre. A channel with no envelope is compared exactly on both sides |
| D45 | ModPlug Tracker extends XM's `Xxy` with `X9E` play-forward and `X9F` play-reverse, and libxmp implements them; FastTracker 2 accepts only `X1x` and `X2x` | libxmp `src/loaders/xm_load.c:210-222`; OpenMPT `kFT2RestrictXCommand` | Ignore everything but `X1x` and `X2x`, and report the unhandled command for the UI. `libxmp-xm-reverse-xm` is excluded whole: it starts its sample at the end, which StarPlayer never does |
| D46 | FastTracker 2 advances the auto-vibrato phase **before** reading its sine table, and that table is `round(64 * sin(-i * 2 * PI / 256))` — negative for its first half — so the very first tick of a note already bends the pitch up. libxmp reads its positive-going LFO first and advances it afterwards, so its auto-vibrato starts a tick later and in the opposite direction | `ft2_replayer.c` `updateVolPanAutoVib` 1732-1750, `ft2_tables.c` `autoVibSineTab`; libxmp `src/player.c:1174-1180` | Keep FastTracker 2's order and sign. `openmpt-xm-keyoff-instr` waives `period` and `position`; its periods are symmetric about the un-vibratoed 4608, which is what makes the difference legible |
| D47 | The tremolo **ramp** waveform: FastTracker 2 takes the ramp's sign from `ch->vibratoPos` rather than `ch->tremoloPos`, so a channel running vibrato and tremolo together inverts its tremolo half the time. OpenMPT says outright that it does not reproduce this, and libxmp follows OpenMPT | `ft2_replayer.c` `tremolo` 1993-2002 (`FT2 bug, should've been ch->tremoloPos`); `openmpt/xm/TremoloWaveforms.xm`'s own comment | Reproduce FastTracker 2's bug, because §1's rule for XM is FT2's behaviour and OpenMPT documents this as a deliberate *non*-reproduction rather than as FT2 being different. `openmpt-xm-tremolowaveforms` waives `volume`; its sine and square waveforms are enforced and agree |
| D60 | Impulse Tracker never played stereo samples (`ITTECH.TXT`: "Stereo samples not supported yet"); OpenMPT added them as an extension, storing the right channel as a second block after the left | OpenMPT `ITSample::GetSampleFormat` `SampleIO::stereoSplit`, and `ITDecompression`'s per-channel block loop | Downmix `(left + right) / 2` at load time, for both raw and compressed data. StarPlayer's voices are mono-source by architecture — one `SampleRegion`, one position, pan applied at the mixer — so keeping two channels would mean a second voice per note or a stereo kernel, and dropping the right channel would silence half of a hard-panned sample. Five samples in the pinned libxmp corpus are affected. The stereo flag is only believed from `Cwt/v` 0x0214 onwards, as OpenMPT does, because older Impulse Tracker versions set it by accident on import |
| D61 | An IT file never states its channel count: `ChnPan` and `ChnVol` are always 64 entries, and the answer has to be inferred from the patterns. OpenMPT and libxmp infer it differently — OpenMPT counts a channel used only when an event's mask **low nybble** is non-zero and does not mask the channel byte to six bits, libxmp counts any channel an event *names* and masks with `& 63` | OpenMPT `Load_it.cpp` `ReadIT` pre-scan (`chnMask[ch] & 0x0F`); libxmp `src/loaders/it_load.c` `max_ch` scan | Follow OpenMPT: the highest channel that any pattern actually writes a note, instrument, volume or command to, plus one, clamped to the format's 64 columns and floored at 1. A channel the header disables with `ChnPan` bit 7 still counts, because IT processes effects in muted channels; the disabled and surround flags are carried verbatim in `format_data` for G4. The two rules differ only on a file whose pattern names a channel without writing to it (libxmp would widen the song, this does not) and on a channel byte above 64 (libxmp wraps it into 0..63, this parses it against its own memories and then discards it) |
| D62 | The note byte `0xFD` is "note fade" to `ITTECH.TXT` and to libxmp, and "no note" to OpenMPT — OpenMPT reserves it so that `.mptm` can write a fade without breaking older readers | `ITTECH.TXT` *Impulse Pattern Format* ("Others = note fade"); libxmp `it_load.c:1084`; OpenMPT `Load_it.cpp` (`note == 0xFD && GetType() != MOD_TYPE_MPT`) | Follow ITTECH.TXT and libxmp: every note byte from 120 to 253 normalises to the loader's `NOTE_FADE` (253). That frees byte 252 for the loader's own "this cell has no note", which no file can therefore collide with. The choice only matters for `.it` files that write 0xFD in a note column, which Impulse Tracker's editor cannot produce |
| D63 | `ITTECH.TXT` allows an initial tempo down to 31 BPM and OpenMPT clamps to exactly that | `ITTECH.TXT` header `IT` field; OpenMPT `Load_it.cpp` `SetDefaultTempoInt(std::max(uint8(31), ...))` | Clamp to 32, the same floor the S3M loader applies, because that is where the sequencer's tick arithmetic is specified. One BPM below what any file in the pinned corpus asks for, and recorded so the difference is a decision rather than an oversight |
| D70 | The `2 × damping factor` law is `pow(10, −3·resonance/320)`, which OpenMPT evaluates with `std::pow` at run time and Schism Tracker ships as a table of 128 `f32` literals. Design goal 5 bans the first from the RT path and the second from the fixed path | OpenMPT `Snd_flt.cpp` `SetupChannelFilter` (`dmpfac`); Schism `player/filters.c` `resonance_table` | Transcribe it as `IT_RESONANCE_TABLE_Q24`, 128 Q0.24 integers, and derive the float path's value from the same table. One entry (resonance 40) differs by one Q0.24 LSB from Schism's printed `f32`, because that `f32` lands exactly halfway between two Q0.24 integers and rounds the other way; the shipped table follows the law rather than the `f32`. That is `6 × 10⁻⁸` of a damping factor, and every entry is checked against the formula in `it_resonance_table_matches_the_reference` |
| D71 | With IT's **extended filter range** the cutoff law is `110 · 2^(0.25 + cutoff/20)`, whose exponent in 1/768ths of an octave is `192 + 38.4·cutoff` — not an integer, so it does not land on an entry of `LINEAR_FREQUENCY_TABLE` | OpenMPT `Snd_flt.cpp` `CutOffToFrequency` (the `20.0f × 512.0f` divisor under `SONG_EXFILTERRANGE`) | Interpolate linearly between the two neighbouring table entries, which are `2^(1/768)` apart, rather than rounding the index. The chord error is under `10⁻⁷` relative — below the `f32` path's own precision, and far below one Q8.24 LSB of the resulting coefficients. The standard range needs none of this: `192 + 32·cutoff` is exact |
| D72 | The fixed path reduces its Q8.24 filter arithmetic with the workspace's C6 rule — round to nearest, **ties away from zero** — where OpenMPT's integer mixer adds a half and arithmetic-shifts, which rounds ties toward `+∞`. The same applies to removing the 256× pre-amplification, where OpenMPT divides and so truncates toward zero | OpenMPT `IntMixer.h` `ResonantFilter::operator()` (`mpt::rshift_signed(… + (1 << 23), 24)`, then `val / MIXING_FILTER_PREAMP`) | Keep C6's rule, which every other fixed-path precision reduction in this engine already uses and which has no negative-slope DC bias. The disagreement is at most one LSB of a pre-amplified sample — 1/256 of an `i16` LSB — and only on exact half-way values. Consistency inside the engine is worth more than bit-equality with one of two reference implementations, particularly as OpenMPT's own float mixer does not round at all |
| D73 | OpenMPT resets the filter's delay line only when `chn.triggerNote` is set, so a **tone-portamento sample swap keeps it**; the M6-G2 task file asked for a reset on `Voice::set_region` as well | OpenMPT `Sndmix.cpp` `HandleNoteChangeFilter` → `SetupChannelFilter(chn, true)`; test cases `FilterPortaSmpChange.it`, `FilterPortaSmpChange-InsMode.it` | Follow OpenMPT rather than the task file: `Voice::new` and `Voice::retrigger` zero the delay line, `Voice::set_region` does not. A swap under `Gxx` is a change of waveform in the middle of a sounding note, and clearing a two-pole's memory there is both inaccurate and a click. Recorded because it is a deliberate departure from a written instruction |
| D74 | IT's cutoff can be modulated at sub-integer resolution: `SetupChannelFilter` computes `cutoff · (envModifier + 256)` over `24 × 512`, so a filter envelope addresses the cutoff in **halves** of an IT unit | OpenMPT `Snd_flt.cpp` `CutOffToFrequency(nCutOff, envModifier)` | The mixer's coefficient entry point takes IT's own seven-bit `0..=127`, so an envelope's half-step is rounded to the nearest whole cutoff unit before it reaches the filter. One unit is `2^(1/24)`, 2.9% of the cutoff frequency, so the rounding is at most 1.5% and the visible consequence is that a filter envelope sweeps in 128 steps rather than 256. Revisit if a corpus case shows it: `FilterParams` carries a `U0F16` and has the bits spare to express the half-step whenever the coefficient entry point wants them |

D1, D2, D5, D24, D27, D30, D31 and D32 change audible output on modules that use those
waveforms, wide arpeggios, two-nibble volume slides, nested pattern loops, row delays,
tremor or tremolo. That is intended: the canonical behaviour is what Scream Tracker 3 produced,
and matching it is what the "most accurate S3M playback" claim actually means.

**Tracker dialects are not deviations and are not refusals.** Where a corpus case encodes
another tracker's flow semantics rather than a disagreement about the reference — the S3M
pattern-loop modes libxmp selects from the `cwtv` field (`src/loaders/s3m_load.c:390-432`:
`0x1301` ST3.01, `0x1303` ST3.21, `0x1320` ModPlug 1.16, `0x2100` Imago Orpheus), or the
MOD tracker variants behind the `CD61` Octalyser and `FA04` / `FA06` Digital Tracker
signatures — the answer is a `FormatDialect` detected from the file header and mapped to
`QuirkSet` fields. That work landed as
`plans/engine/complete/M2-task-C5-quirks-and-tempo-models.md`; §2's *Tracker dialects* section
above is the result. Each dialect field flipped exactly the corpus cases that name it and
left every other result unchanged under `canonical()`. Nine of the ten now pass — two of
them with no waiver at all — and the tenth, `libxmp-mod-pattern-loop-dt`, is a harness
pairing record (`conformance/known-failures.md`, `C2-MOD-001`) rather than a replay
difference: its complete 488-tick row sequence matches the oracle's.

### D64 — the IT comparison axis is libxmp's whole Amiga period

**Cause.** Impulse Tracker holds a voice's pitch as a **frequency in whole hertz**
(ITTECH.TXT's `PitchTable` is a 16.16 ratio and `Frequency = (C5Speed · PitchTable[note]) >> 16`),
and so does StarPlayer. libxmp holds it as a note plus a bend and converts only at the
mixer: it dumps `xc->info_period = MIN(final_period · 4096, INT_MAX)` with
`final_period = PERIOD_BASE / 2^((note + bend/12800)/12)` and `PERIOD_BASE = 13696`
(`src/period.c:205`, `src/player.c:1298`). Its own IT loader additionally folds every
sample's `C5Speed` into a relative note and a finetune (`libxmp_c2spd_to_note`,
`src/loaders/it_load.c:906`) and then mixes at the fixed `m->c4rate`, so its period
numerator is the constant `C4_PERIOD · 8363 = 428 · 8363`.

**Behaviour chosen.** The C1 trace reports `428 · 8363 / frequency` — libxmp's own whole
period — and `project_period` divides the oracle's Q12 column by 4096, the same projection
MOD and MTM use, with the existing tolerance of one whole period. The `note` column is
projected by removing the same relative-note offset libxmp's loader added, so the two sit
on IT's own note numbering. A pitch difference smaller than one whole period at the
sounding note is therefore below the oracle's resolution: about 0.23 % at C-5 and 1.9 % at
C-8, against IT's own finest slide step of 1/64 semitone (0.09 %). The axis catches note,
octave, slide-direction and accumulated-drift errors; tightening it is a G6 question.

### D65 — the IT pattern-loop baseline is Impulse Tracker 2.10

**Cause.** `SBx` changed three times inside Impulse Tracker's own life: 1.00 kept one
global loop target and counter as Scream Tracker 3 does, 1.04 made both per channel, 2.00
made a loop jump block a `Cxx` later on the same row, and 2.10 reintroduced ST3's
advancement of the loop target past the `SBx` row when the count runs out. libxmp selects
the same four profiles from `Cwt/v` (`src/loaders/it_load.c:394-400`,
`FLOW_MODE_IT_100`/`_104`/`_200`/`_210`), and the pinned corpus carries one fixture per
profile.

**Behaviour chosen.** `ItLoopDialect` with the 2.10 profile as the default, selected by
`FormatDialect::ImpulseTracker{,200,104,100}` from `Cwt/v`. Every IT clone — Schism,
OpenMPT, ModPlug — takes the 2.10 baseline, which is what libxmp does with them too.
Task G3's research point 3 could not corroborate the "2.10 versus 2.14" split the task
file assumed: OpenMPT gates its loop behaviours on *OpenMPT* version rather than on IT's,
and no source found describes a change after 2.10.

### D66 — an IT surround channel is rendered at centre

**Cause.** `S91` puts a channel into surround, which Impulse Tracker's software mixer
renders by inverting the right channel's phase — a Dolby Pro-Logic trick. libxmp models it
with a `PAN_SURROUND` sentinel (`src/mixer.h:23`) which it dumps verbatim in the pan
column, and 48 records of `SmpInsPanSurround.data` carry it. StarPlayer's voice carries one
`pan: I1F15` and the mixer has no phase-inverted rear bus; a surround voice has nowhere to
go until the DSP graph grows one in M7.

**Behaviour chosen.** Surround is rendered at centre pan, and the conformance adapter
projects libxmp's sentinel onto centre so the rest of that trace stays enforced. Everything
*around* surround is implemented: `S90`/`S91` set and clear it, `Xxx`, `S8x` and a
sample's or instrument's own panning cancel it (`kITNoSurroundPan`, `kPanOverride`), and
`Pxy` and `Yxy` are no-ops while it is on.

### D67 — a whole-hertz playback rate against libxmp's double-precision period

**Cause.** The consequence of D64 seen from the other side. Impulse Tracker's playback rate
is an integer number of hertz and every slide multiplies that integer by a Q16.16 ratio,
so each step rounds; libxmp carries a `double` period end to end. The two rates agree to
within the rounding of one period — below the resolution of the `period` column — but the
sample *position* is the running integral of the rate, so the difference accumulates over
a trace and eventually exceeds libxmp's own one-source-frame position bound.

**Behaviour chosen.** Integer hertz, because that is Impulse Tracker's own arithmetic and
OpenMPT's (`ModChannel::nPeriod` is a `uint32` holding a frequency for IT). Twenty-six IT
cases therefore carry `waive=position` and enforce every other field for the whole trace —
the same shape as D14's ProTracker finetune drift.

### D68 — `Pxy` does nothing on a surround channel

**Cause.** Impulse Tracker's own replayer returns from the panning-slide handler when the
channel's pan is the surround sentinel (`it2play` `it_m_eff.c`, `InitCommandP`:
`if (pan == PAN_SURROUND) return;`). OpenMPT slides anyway, and Schism clears the surround
flag unconditionally.

**Behaviour chosen.** Impulse Tracker's, since `it2play` is a direct port of IT2's own
code and §0's rule makes the format's own program the reference where OpenMPT's
compatibility notes do not contradict it. `Yxy` panbrello is skipped on a surround channel
for the same reason.

### D69 — `Qxy`'s two-thirds and three-halves retrigger volumes

**Cause.** IT's `Qxy` volume table multiplies by exactly 2/3 for `x = 6` and 3/2 for
`x = E` (`it2play`: `(vol << 1) / 3` and `(vol * 3) >> 1`). OpenMPT approximates both
through one sixteenths table — `10/16` and `24/16` — so a note at volume 64 comes out at
40 where Impulse Tracker plays 42.

**Behaviour chosen.** Impulse Tracker's exact arithmetic, for the same reason as D68.

### D75 — FastTracker 2's Amiga period table against libxmp's continuous formula

**Cause.** An XM whose header flag 0 is clear plays in *Amiga* mode, where FastTracker 2
reads a 96-entry integer table (`amigaPeriodLUT`, `ft2_tables.c`) exactly as Scream Tracker
3 does, and the comparison axis is four times ProTracker's period. libxmp evaluates
`13696 / 2^(n/12)` continuously and reports the result at Q12 (`src/period.c:205`), so
`F#3` is FastTracker 2's 604 against libxmp's 605.25 — five quarter-units on the
comparison axis. This is the XM twin of **D36**, and the reason it shows here and not on
the four other Amiga-mode fixtures is that D43's four-unit XM tolerance was derived in the
**linear** domain, where four units is a sixteenth of a semitone; in the Amiga domain the
same fraction of a semitone is a different number of units at every pitch.

**Behaviour chosen.** Keep FastTracker 2's table, as D36 keeps Scream Tracker 3's and D14
keeps ProTracker's, and waive `period` and `position` on `libxmp-xm-pattern-loop-mpt` —
the one Amiga-mode fixture whose notes land where the two disagree by more than four
quarter-units. Widening the tolerance for Amiga-mode XM was rejected: it would loosen the
four fixtures that currently pass with the period enforced.

### D76 — libxmp applies an XM vibrato on tick zero of a row

**Cause.** FastTracker 2's `doVibrato` reaches the period only from
`JumpTab_TickNonZero[4]`: on a row's first tick the channel keeps the `outPeriod` the
previous tick left, and the LFO neither reads nor advances. libxmp computes and applies its
vibrato on every frame including the first (`src/player.c:1184-1199`; only
`QUIRK_PROTRACK`, which XM does not have, suppresses it), using the phase the previous row
left un-advanced. The two tables agree — FastTracker 2's 32-entry `vibratoTab` is the first
half of libxmp's 64-entry `sine_wave`, value for value, and the inverted ramp
`get_lfo_ft2` implements is `doVibrato`'s `~tmpVib` — so the *shapes* never differ; the
positions do, by one tick, at a row boundary.

**Behaviour chosen.** FastTracker 2's. `openmpt-xm-vibratowaveforms` waives `period` and
`position`: sixteen records of 768 differ, every one of them a row's tick zero, and the
largest difference is one full vibrato amplitude on a square-wave row. This is the same
shape as `C2-S3M-009`'s second part, where libxmp applies the S3M tremolo on tick zero too.

### D77 — libxmp defers a delayed row's volume column to the delay tick

**Cause.** `getNewNote` latches `ch->volColumnVol = p->vol` **before** it returns for an
`ED1`..`EDF` note delay, so FastTracker 2 runs the volume column's effect from tick one
whether or not the note has fired; OpenMPT models the same thing as `kFT2VolColDelay`
(`Snd_fx.cpp:3229-3236`, which is false only on tick zero and on the delayed tick when the
row also carries an instrument number). libxmp instead copies the **whole event** into
`xc->delayed_event` and reads none of it until the delay expires (`src/player.c:779-812`,
`:1619-1622`), so a volume-column slide next to an `EDx` starts one tick late and stays one
tick behind for the rest of the row.

**Behaviour chosen.** FastTracker 2's, which OpenMPT agrees with. `openmpt-xm-delay3`
waives `volume` — with `position` and `active` for D42's finetune drift — and the `ED0`
rows the fixture exists to test, 18 to 28 of both patterns, agree exactly.

### D78 — the envelope tick a key-off resumes on

**Cause.** FastTracker 2's envelope is an accumulator with a point cursor: on the tick the
key-off releases a sustained envelope, `updateVolPanAutoVib` finds `volEnvTick` equal to the
sustain point's own tick, reloads the exact point value, recomputes the segment's Q8 delta
and sets `envDidInterpolate`, which **suppresses** the accumulate for that tick. libxmp
recomputes `y0 + (y1 - y0)·(x - x0)/(x1 - x0)` from a position that is already one step into
the segment, and its division truncates toward zero. On `EnvLoops.xm`'s falling release the
two are 64 and 62 of 64 on the same tick, which is D44's mechanism plus one whole step of
phase.

**Behaviour chosen.** FastTracker 2's. `openmpt-xm-envloops` waives `volume` and
`position`; the difference is at most three steps of the 0..64 axis and only while a
released segment is running.

### D79 — a zero-byte oracle for a module that sounds notes

**Cause.** libxmp's `compare_mixer_data` writes one line per tick per *sounding* channel, so
a fixture whose whole point is that nothing plays legitimately has a zero-byte `.data` —
`openmpt/xm/DelayCombination.data` is exactly that and passes with every field enforced.
`openmpt/xm/PanMemory.data` is zero bytes too, but `PanMemory.xm` sounds two notes at row 4
and its own comment says they "should be panned hard right". The dump is missing from the
pinned tree rather than empty on purpose, and the harness's only available reading of a
zero-byte file — no channel may ever be active — is one the module cannot satisfy.

**Behaviour chosen.** Record it, keep executing it, and leave the reading of an empty dump
alone: relaxing it would silently disarm `DelayCombination`, which is a real expectation.
`openmpt-xm-panmemory` is an accepted deviation rather than a known failure because nothing
about StarPlayer is wrong; the sibling `openmpt-xm-panmemory2`, which OpenMPT calls a more
thorough check of the same pan memory, passes with every field enforced. Regenerating the
dump would need libxmp built and run against the pinned tree, which the corpus pin exists to
avoid.

### D80 — the last bit of IT's volume projection

**Cause.** Impulse Tracker's final channel volume is one integer product —
`muldiv(volume · globalVolume, channelVolume · instrumentVolume, 1 << 20)` in OpenMPT's
`Sndmix.cpp` (`chn.nRealVolume`) — evaluated after the envelope and the fadeout have
already scaled the 14-bit note volume. libxmp forms the same quantity in a different
grouping and a different order (`src/player.c:1063-1099`: `finalvol` is scaled by the
fadeout with a `>> 6`, then by `vol_envelope · gvol · mastervol` over `gvolbase` with a
`>> 18`, then by `instrument->vol · gvl >> 12`), so the two chains round in different
places. The trace then quantises whatever each side holds onto libxmp's own 0..64 column.

**Behaviour chosen.** OpenMPT's single product, with a **one**-unit tolerance on the
projected `volume` column for IT — the same shape as D44's for XM, and the only slack the
IT comparison has. Every case whose volume is more than one unit out stays a known failure
(`G6-IT-001`).

### D81 — IT's row-delay tick counter against the engine's absolute row clock

**Cause.** `SEx` repeats a row, and Impulse Tracker restarts the player-visible tick
counter on every repeat, which is what libxmp's dump `frame` column carries. StarPlayer's
`RowClock` deliberately exposes the **absolute** tick budget of the row so a processor can
tell a repeated first tick from the row's real first tick — that distinction is what makes
`FineVolRowDelayMultiple.it`-shaped fixtures decidable at all.

**Behaviour chosen.** Keep the absolute clock in the engine and project it: the conformance
adapter reduces IT's `tick_in_row` modulo the speed before comparing
(`crates/starplayer-testkit/src/conformance.rs`, `project_tick_in_row`). This is a
representation difference in a diagnostic column, not a playback difference — no audible
state is derived from it.

### D82 — ModPlug Tracker 1.16's IT pattern-loop profile

**Cause.** libxmp gives every IT file `FLOW_MODE_IT_210` and narrows it only for Impulse
Tracker's own early `Cwt/v` values (`src/loaders/it_load.c:351,394-400`), so a file written
by ModPlug Tracker 1.16 gets Impulse Tracker's `SBx` flow. `pattern_loop_mpt.it` is exactly
that file, and its expected row sequence is ModPlug's own: one channel at a time owns the
loop, and a loop jump blocks every break or jump on the same row — the profile C5 already
carries for S3M as `S3mLoopDialect::ModPlug116`.

**Behaviour chosen.** `ItLoopDialect::ModPlug116`, selected by `FormatDialect::ModPlugIt`,
so one detected tracker gets one flow profile in both formats it wrote. The case named by
the field is `libxmp-it-pattern-loop-mpt`; `pattern_loop_it100/104/210.it` keep the
Impulse Tracker profiles they name.

### D83 — the ping-pong loop cycle and a reversed sample's start position

**Cause.** Impulse Tracker's software mixer walks a ping-pong loop over `2L - 1` frames —
it does not repeat the endpoint — where the reusable mixer reflects at both addressable
endpoints, a `2L` cycle. `S9F` compounds it: a reversed voice that has not moved yet starts
at the sample's *final fractional* position (OpenMPT `Snd_fx.cpp` `ExtendedChannelEffect`,
`chn.position.Set(chn.nLength - 1, fractMax)`), so the two mixers' loop coordinates differ by
up to one frame for the whole life of the note even though every other field agrees.

**Behaviour chosen.** Keep the shared mixer's symmetric reflection — it is one loop
implementation for five formats, and IT's asymmetry is a property of *its* mixer rather
than of the module — and implement `S9E`/`S9F` direction and the reverse start position in
the IT processor. `openmpt-it-bidi-loops`, `openmpt-it-sustain-after-loop` and
`libxmp-it-reverse-it` waive only `position`; every other field is enforced.

### D84 — libxmp's cutoff column cannot say "no filter" from "cutoff zero"

**Cause.** The dump's `cutoff` column is a *mixer voice* field that libxmp writes only
when the filter actually engages (`src/player.c:1330-1341`, the
`libxmp_virt_seteffect(DSP_EFFECT_CUTOFF)` branch). A voice that never engaged one reports
the zero it was allocated with, and so does a voice that genuinely engaged at cutoff zero —
`ZxxSecrets.it` and `it_fade_env_reset.it` do exactly that, with a non-zero resonance beside
it to prove the filter is running.

**Behaviour chosen.** Read a zero in the column as "either", exactly as libxmp's own
comparator already reads 254 and 255 as the same fully-open cutoff
(`test-dev/compare_mixer_data.c:84-87`): the projection accepts our 0 and our fully-open
255 against the oracle's 0, and enforces every other value exactly.

### D85 — libxmp holds a filter envelope at its last value near the top of the axis

**Cause.** libxmp only assigns `xc->filter.envelope = frq_envelope` when the envelope reads
below `0xfe` (`src/player.c:1318`), having initialised it to `0x100` on the note
(`src/read_event.c:134`). Impulse Tracker and OpenMPT feed the value straight through:
`SetupChannelFilter` computes `cutoff · (envModifier + 256) / 256` with `envModifier` from
`PitchEnv.GetValueFromPosition(envpos, 512, 64) - 256` (OpenMPT `Snd_flt.cpp`,
`Sndmix.cpp` `ProcessPitchFilterEnvelope`).

**Behaviour chosen.** OpenMPT's, with a **two**-step tolerance on the projected `cutoff`
column for IT. The bound is derived, not chosen: the envelope axis is `0..=256` and the
cutoff at most 254, so holding the two top envelope steps moves the cutoff by at most
`254 · 2 / 256`, which is under two steps of libxmp's doubled cutoff axis — one whole IT
cutoff unit.

### D86 — IT's panning envelope against libxmp's whole-unit interpolation

**Cause.** The same mechanism as D44, in IT's arithmetic. OpenMPT interpolates its
envelopes in Q16.16 and rounds once on the way out
(`ModInstrument.cpp` `InstrumentEnvelope::GetValueFromPosition`), where libxmp recomputes
`y1 + (y2 - y1)·(x - x1)/(x2 - x1)` in whole envelope units with a division that truncates
toward zero (`src/player.c:118`). IT applies the result as
`pan += envelope · (256 - pan)/32` or `envelope · pan/32` (OpenMPT `Sndmix.cpp`
`ProcessPanningEnvelope`; libxmp's `finalpan` at `src/player.c:1390` is the same
expression), so one envelope unit of disagreement is four pan units at the centre.

**Behaviour chosen.** OpenMPT's rounding, with the same **four**-unit `pan` tolerance XM
already carries. A channel with no panning envelope is compared exactly on both sides.

### D87 — libxmp's dump clock restarts at every pass of a wrapping module

**Cause.** libxmp times each record with `xmp_frame_info.time`, which is the time of the
*current position within the song* taken from its own scan, not elapsed output time. A
module that wraps therefore replays the same timestamps: `openmpt/xm/PatLoop-Weird.data`
runs 78, 156, 78, 156, 234, 312 … and `PatLoop-Break.data` reaches 2720 and then starts
again at 140. StarPlayer's trace timestamps every tick with the monotonic output frame it
was dispatched on, which is the only reading that keeps a trace comparable across host
block sizes (design goal 3), so from the first wrap onward the two clocks cannot agree by
construction.

**Behaviour chosen.** The monotonic frame. The two fixtures that wrap —
`openmpt-xm-patloop-weird` and `openmpt-xm-patloop-break` — waive `frame`, and the
harness's `pair_by_time` re-anchor (the `C2-MOD-001` repair) then aligns their records on
`(row, tick_in_row)`, which is the axis the fixtures are actually about. `position` is
waived alongside it for D42's finetune reason. Every other field stays enforced, so the
whole `0 3 1 0 3 1 2 3 1 2 3 1 2` row sequence, its notes, instruments, volumes, periods
and pans are compared exactly.

### D88 — FastTracker 2's carried pattern-loop break row survives the order-list wrap

**Cause.** Section 1's `kFT2LoopE60Restart` entry: `patternLoop` leaves its target row in
`song.pBreakPos`, and `getNextPos` clears that only in the branch a position change takes —
the same branch that assigns `song.row = song.pBreakPos` and wraps `song.songPos` to
`song.songLoopStart`. So when a pattern that has taken an `E6x` loop jump then ends
normally *on the last order*, FastTracker 2 wraps to the restart order and starts it on the
loop target row. `openmpt/xm/PatLoop-Break.xm` is exactly that shape: pattern 0 row 12
carries `E60`, pattern 1 row 3 carries `E62`, and the `E62` jump back to row 12 is still in
the break position when pattern 1 runs off the end of the two-entry order list. StarPlayer
re-enters pattern 0 at row 12; libxmp's dump re-enters it at row 0, and every later record
shifts with it (first divergence, with `frame` and `position` waived, at tick 174 channel 1
field `instrument`, 1 expected against 3).

**Behaviour chosen.** FastTracker 2's, unchanged — it is section 1's own rule, and
`starplayer-xm`'s `a_pattern_loops_target_row_starts_the_next_pattern` pins it. libxmp does
not carry the break position across the wrap, so `openmpt-xm-patloop-break` is an accepted
deviation rather than a known failure; its sibling `openmpt-xm-patloop-weird`, whose wrap
row comes from a `D03` rather than from a loop jump, passes with both players agreeing.
What would settle it beyond the two sources already read is a capture from a real
FastTracker 2.

## 4. Not offered at all

- **Retro mixer emulation.** The original's 8-bit unsigned mono SoundBlaster mixer, its
  65×256 volume lookup table, and its master-volume-derived `PostTable` soft-clip curve
  are documented in `plans/reference/original-s3mlib-analysis.md` but not implemented.
- **MOD/MTM via S3M conversion.** Not offered even as a compatibility mode. It destroys
  format identity before the player sees it and is a dead end once XM/IT arrive.
- **Tagless 15-sample Soundtracker MOD.** Its header offsets, loop units and effect/tempo
  dialect differ from tagged 31-sample ProTracker files; C3 rejects it explicitly, and C5
  research point 1 confirmed the rejection rather than adding a `FormatDialect` for it.
  A dialect is a `QuirkSet`; this is a second *header layout* — 15 sample records instead
  of 31, pattern data 480 bytes earlier, and no tag to detect it with, so a loader would
  have to guess from plausibility fields and would produce false positives on arbitrary
  input. It therefore belongs behind its own loader entry point whenever a real need for
  it appears, not behind a quirk field.
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
   **Linear remains the canonical kernel.** M7-H5 added
   `goldens/s3m/reflex__i16_mono_44100_{cubic,sinc}.sha256` beside the linear one; those
   two are **cross-target pins for the wide kernels** — the thing they catch is a cubic or
   sinc render that is not bit-identical on x86-64, aarch64 and wasm32 — not a second
   reference render. Accuracy statements in this document are about the linear hashes.
   **A canonical golden never sees a sample enhancer.** M10-K5's load-time enhancers
   (`starplayer-enhance`) rebuild a module before it is played, so an enhanced render is
   a different render of a different module and could not be an accuracy statement about
   anything. An enhanced configuration is hashed under its own filename —
   `<stem>__i16_mono_44100_linear_enh-<name>.sha256`, where `<name>` is the enhancer
   chain's own `name()` and therefore encodes every parameter that changes its output
   (`sinc4x`, `loop=64`, `sinc4x+loop=64`, a rate ceiling and all) — so turning an
   enhancer on is visibly a new golden rather than a silent break, exactly as a change of
   interpolator is. The `enhance` feature is never in `default` for the same reason.
   M10-K5b's `starplayer-cli render --enhance sinc4x+loop -o out.wav` names this same
   configuration on the command line — `--golden` refuses `--enhance` for exactly this
   reason — and `starplayer-offline::golden_filename_for_configuration` is the one place
   that spells the `_enh-<name>` suffix; no enhanced golden is committed.
6. **Cross-target hash equality** — x86-64, aarch64 and wasm32 must agree bit-for-bit
   on the fixed-point path.
7. **Perceptual comparison vs libopenmpt** on the float path — spectral distance /
   segmental SNR with a tolerance. Nightly, not a gate. `cargo xtask perceptual` builds
   `openmpt123` from a checksum-pinned libopenmpt source tarball into `target/openmpt/`
   (no root, no package, no FFI), renders every fixture through both engines at 44.1 kHz
   stereo with linear interpolation, normalises both to equal RMS, and reports a segmental
   SNR and a log-spectral distance per fixture. Nothing about libopenmpt is committed.
   The scores are a **trend**, not a threshold: the two engines' tick lengths differ by
   design — §2's `tempo_model` row — so a sample-domain SNR drifts on any module whose BPM
   does not give a whole number of frames per tick, while the spectral distance does not.
   A score never fails a build.

A **contingency**: if a specific effect's behaviour cannot be settled from the assembly,
`plans/engine/M2-task-C8-dos-reference-harness.md` describes reconstructing a buildable
DOS reference and dumping the original's own per-tick `ChannelData`, which is directly
comparable to (2). This is deliberately *not* WAV-diffing DOSBox — its SoundBlaster
emulation resamples, so that would be diffing emulator artefacts.

## 6. Amending this document

When implementation discovers a new deviation, add it to §3 with its cause and the
canonical behaviour chosen, in the same commit as the code. A deviation that is not
written down here is a defect, not a decision.
