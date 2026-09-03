# Conformance known failures

These records track executed corpus failures; they are not accepted accuracy-policy
deviations. Every record blocks the M2 exit until the engine conforms or the documented
behaviour is deliberately resolved in `plans/product/03-accuracy-policy.md`. The harness
still runs every excluded case, so a fix makes its exclusion fail as stale.

**Who owns what.** C9 (`plans/engine/complete/M2-task-C9-s3m-conformance-repairs.md`,
landed) left the one S3M record below; its exclusion row points here. The MOD record C5
opened, `C2-MOD-001`, was a harness limitation and is resolved below.
No MTM case is recorded here. The ProTracker fidelity repairs and the mixer
voice-boundary sample swap landed as
`plans/engine/complete/M2-task-C3b-protracker-fidelity-repairs.md` (landed); the four MOD cases that task
file listed as targets but did not fix turned out to be oracle-representation
differences, and each now cites `plans/product/03-accuracy-policy.md` — D21 for
ProTracker's tick-zero `E9x` retrigger (`openmpt-mod-delay-break`), D22 for its
tick-zero instrument latch under `EDx` (`openmpt-mod-portamento-swap-pt`), and D18 for a
queued stop that falls due inside a tick interval, which makes libxmp omit the channel
for that whole tick (`openmpt-mod-swap-no-loop`,
`openmpt-mod-portamento-sample-change-pt`).
The harness repairs — trace alignment, loop-aware position comparison, the D18 adapter
projection, the per-field waiver and the tick budget — landed as
`plans/engine/complete/M2-task-C2a-conformance-harness-repairs.md`, so every first divergence
quoted below is one the repaired harness observed, not an alignment position.

The ten tracker-dialect cases — the Imago Orpheus, ModPlug 1.16 and ST3.01 flow modes
libxmp selects from the `cwtv` field, and the Octalyser and Digital Tracker MOD tags —
were never failures against StarPlayer's reference, and C5 has landed their
`FormatDialect` and `QuirkSet` fields. All ten now pass (the last needed the harness
re-anchor recorded under `C2-MOD-001` below). The MOD failures are not recorded
here either: the ProTracker fidelity repairs and the voice-boundary sample swap are
`plans/engine/M2-task-C3b-protracker-fidelity-repairs.md`. The harness repairs — trace
alignment, loop-aware position comparison, the D18 adapter projection, the per-field
waiver and the tick budget — landed as
`plans/engine/complete/M2-task-C2a-conformance-harness-repairs.md`, so every first
divergence quoted below is one the repaired harness observed, not an alignment position.

## C2-S3M-009 — the Rxy tremolo phase and its first tick

`ParamMemory.s3m` is the last S3M record. Its shared parameter memory, the effect it
exists to test, now agrees for every family through row 58 of 63 with `period` and
`position` waived under accuracy-policy D39 (libxmp truncates the S3M vibrato in its
Amiga period domain, so `H82` moves its period by 8 quarter-units where ST3 moves it by
11). The remaining first divergence is **tick 353 channel 0 on `volume`, 64 expected
against 59 actual**, on the `Rxy` rows, and it has two parts:

1. ST3's `M_FX_R` reads and advances `_VibCount`, the phase it shares with `Hxy` and
   `Uxy`, so a preceding `Kxy` vibrato leaves the tremolo mid-waveform. libxmp and
   OpenMPT both give the tremolo its own position, which starts at zero here.
2. libxmp applies the tremolo delta on the first tick of a row as well, using the phase
   un-advanced, where the original runs `Rxy` only as a minor effect.

Both would also change `Hxy`'s tick-zero behaviour if adopted, which no corpus case
currently pins, so neither was changed on speculation. Resolving this needs a decision on
whether ST3 really shares `_VibCount` between vibrato and tremolo — the tremolo depth
scale itself was resolved and is accuracy-policy D35.

## C2-MOD-001 — resolved: the pairer re-anchors a waived timeline

`pattern_loop_dt.mod` alternates 255 BPM at speed 3 with 63 BPM at speed 1, and accuracy
policy D15 (ProTracker's CIA latch) puts StarPlayer's timeline one interval behind libxmp's
at every tempo command, flipping the residual frame offset by about 1300 frames. The
time-based pairer re-derived that offset only after a successful pairing, so three records
per flip fell back to the wrong tick and the case reported `row` 1 against 0 at tick 121
although its 488-tick `(row, tick_in_row)` sequence matched the oracle exactly.

Resolved in the harness after C5 landed: when a `frame`-waived timeline fails to pair a
record, `pair_by_time` re-anchors on the record's `(row, tick_in_row)`, searching forward
from the last pairing, and re-derives the offset there. The case now passes waiving
`frame,position` under D15 like its four Octalyser and Digital Tracker siblings.

## Resolved by M2-C9

`C2-S3M-001` through `C2-S3M-008` are closed. `libxmp-s3m-pattern-loop-st321`,
`libxmp-s3m-pattern-loop-st321-breakjump`, `openmpt-s3m-amiga-limits`,
`openmpt-s3m-frequency-limits`, `openmpt-s3m-pattern-delays-retrigger`,
`openmpt-s3m-period-limit`, `openmpt-s3m-portamento-sample-change` and
`libxmp-s3m-sample-portamento` all pass, seven of them with a per-field waiver naming an
accuracy-policy entry for a libxmp representation difference (D19, D36, D37, D38, D39)
rather than an engine gap. The engine repairs behind them are accuracy-policy D24–D35.
`libxmp-s3m-pattern-loop-mpt-breakjump`, a C5 dialect case, began passing as a side
effect of the ST3.21 flow repairs and no longer carries an exclusion.

## Resolved by M2-C5

All ten tracker-dialect cases are closed (`libxmp-mod-pattern-loop-dt` after the harness re-anchor above).
`libxmp-s3m-pattern-loop-imf-breakjump` and `libxmp-s3m-pattern-loop-st301-breakjump`
pass with **no** waiver at all and their exclusion rows are gone;
`libxmp-s3m-pattern-loop-imf`, `-mpt`, `-st301` and the four Octalyser / Digital Tracker
MOD cases pass under per-field waivers naming accuracy-policy D15, D19, D36, D37, D38 and
the new D40. Every one of those is a representation or timeline difference against the
oracle, not an engine gap.

## M5 — XM (tasks F2 and F5)

Task F2 landed the XM effect processor and wired all 93 XM cases the pinned tree's
`compare_mixer_data*` calls name; 72 passed and twenty were recorded here as `F2-XM-001`
through `F2-XM-012`. Task F5 settled them: **87 of the 93 now pass**, two are accepted
deviations (`libxmp-xm-reverse-xm` under D45 and `openmpt-xm-panmemory` under D79) and the
four cases below are what is left.

| Record | Disposition |
|---|---|
| `F2-XM-001` — `Mx` + `3xx` under a non-FastTracker-2 dialect | **Fixed.** `QuirkSet::xm_double_portamento_doubles_volume_column_rate`, off for `FormatDialect::ModPlugXm` and `UnknownXm`. All three cases pass with every field enforced |
| `F2-XM-002` — ModPlug and Skale pattern flow | **Fixed.** `QuirkSet::xm_pattern_loop` (`XmLoopDialect`), plus the loader's ModPlug Tracker 1.16 detection. All four cases pass; two of them waive `position` for D19 and D75 |
| `F2-XM-003` — a key-off whose sustain point is point zero | **Sharpened.** `data/ft2_note_off_sustain.xm` passes — it was the placeholder-instrument fadeout, not the key-off — and `libxmp-xm-ft2-kxx` remains, below |
| `F2-XM-004` — `Lxx` after an envelope loop | **Sharpened.** Five of the fixture's six orders now agree; the one that lands exactly on the loop end remains, below |
| `F2-XM-005` — the instrument-fade update rule | **Fixed.** FastTracker 2 parks the channel on its placeholder instrument, whose fadeout is `0x80`, and an instrument-without-note row reads that stale pointer. Accuracy policy §1 |
| `F2-XM-006` — the volume column under a delayed note-off | **Fixed.** `kFT2PanWithDelayedNoteOff`. Both cases pass, `openmpt-xm-panoff` waiving `position` for D42 |
| `F2-XM-007` — the Skale Tracker offset dialect | **Fixed.** `QuirkSet::xm_offset_past_sample_end_stops_channel`, off for every tracker that is not FastTracker 2. The case waives `position` and `active` for D42 |
| `F2-XM-008` — the vibrato ramp amplitude | **Accuracy policy D76.** The amplitudes were never the problem: the two tables are the same values, and libxmp applies its vibrato on a row's tick zero where FastTracker 2 does not |
| `F2-XM-009` — FastTracker 2's stale `song.pBreakPos` | **Half fixed, half sharpened.** `kFT2LoopE60Restart` is implemented and unit-tested; both fixtures are blocked behind the order-list wrap, below |
| `F2-XM-010` — `ED0` is not a rogue note delay | **Accuracy policy D77.** `ED0` was never wrong: libxmp defers a delayed row's volume column to the delay tick and FastTracker 2 does not |
| `F2-XM-011` — a looping envelope after 240 ticks | **Accuracy policy D78.** The tick a key-off resumes a sustained envelope on |
| `F2-XM-012` — a zero-byte oracle | **Accuracy policy D79.** Confirmed as the corpus, not the harness: `PanMemory.data` is missing where `DelayCombination.data` is deliberately empty, and the case is now an accepted record rather than a failure |

## F2-XM-003 — a key-off whose sustain point is the envelope's first point

`libxmp-xm-ft2-kxx`, and **four records of 192**: tick 1 of the two rows whose `Kxx`
releases a volume envelope whose sustain point is point **zero**. `ft2_replayer.c`'s
`keyOff` clamps `volEnvTick` back to `points[volEnvPos].tick - 1`, which for a sustain point
at tick 0 wraps a `uint16_t` to 65535; the next envelope tick therefore reaches 0 again,
re-evaluates point 0 and replays the segment leaving sustain from its start, so the sustain
value survives one tick longer than libxmp's envelope, which has already run past the last
point and reads zero. StarPlayer follows `ft2_replayer.c` line for line and the difference
was traced by hand through both implementations, but it was not settled against a real
FastTracker 2.

First divergence: tick 131 channel 0 field volume, expected 0 against actual 56.

**What would settle it:** a capture from a real FastTracker 2, or `ft2-clone` upstream
confirming that `keyOff`'s clamp is what the disassembly says. `data/ft2_note_off_sustain.xm`,
which F2 filed under the same record, turned out to be the placeholder-instrument fadeout
(`F2-XM-005`) and now passes with every field enforced.

## F2-XM-004 — an `Lxx` that lands exactly on an envelope's loop end

`libxmp-xm-lxx-after-loop`. The fixture plays six orders, each a different `Lxx` landing,
and its own ModPlug comment says all of them should produce the same long volume ramp.
Orders 0, 1, 3, 4 and 5 now agree; **order 2** — `L0C` on an instrument whose loop end is
point 1 at tick 12 — does not.

`ft2_replayer.c`'s `setEnvelopePos` walks the points until `tick` falls short of one, then
subtracts that point's tick; when the remainder is exactly zero it breaks *without*
advancing the point index and without writing a value or a delta, leaving `volEnvPos` at the
lower point and `volEnvTick` at `param - 1`. `updateVolPanAutoVib`'s very next tick then
finds `volEnvTick` equal to that point's own tick, sees it is the loop end and jumps back to
the loop start. OpenMPT stores `Lxx` as a *position* and derives the value from it, so its
envelope simply continues past the loop; libxmp does the same and the oracle follows.

First divergence: tick 144 channel 0 field volume, expected 0 against actual 64 — the value
at the loop start against the value at the loop end.

**What would settle it:** a real FastTracker 2. Both readings are defensible from the
sources available: ft2-clone's is a line-for-line disassembly and OpenMPT's is a model, and
they disagree only when the remainder is exactly zero.

## F2-XM-009 — the order list running out is not the end of an XM

`openmpt-xm-patloop-break` and `openmpt-xm-patloop-weird`. FastTracker 2's own
`kFT2LoopE60Restart` — an `E6x` loop jump leaving its target row in `song.pBreakPos`, so the
next pattern to end normally starts there — **is** implemented, is accuracy policy §1, and is
pinned by `starplayer-xm`'s `a_pattern_loops_target_row_starts_the_next_pattern`. It is not
what these two fixtures need.

Both of them run off the end of the order list. `PatLoop-Weird.xm` has a single order and a
`D03` on row 0; `PatLoop-Break.xm` has two and its second pattern simply ends. FastTracker
2's `getNextPos` wraps in both cases — `if (++song.songPos >= song.songLength) song.songPos
= song.songLoopStart;` — and the oracle records three passes of each. StarPlayer's sequencer
treats the order list running out as the end of the song under `EndOfSongPolicy::Stop`
(`PatternSequencer::move_to_order`), which is task D2's deliberate rule and the reason a
song that simply ends no longer scans as looping, so the capture stops after the first pass.

First divergence: tick 8 channel 1 field position for `PatLoop-Break.xm`, tick 1 channel 0
for `PatLoop-Weird.xm` — both of them the D42 finetune drift rather than the flow, which is
what makes the record legible: hand-simulating `PatLoop-Weird.xm` against `ft2_replayer.c`
reproduces the oracle's whole `0 3 1 0 3 1 2 3 1 2 3 1 2` row sequence once the wrap is
assumed, and needs no other change.

**What would settle it:** a decision, in `starplayer-engine` rather than in `starplayer-xm`,
about what a `Bxx`/`Cxx`/`Dxx` past the end of the order list means under
`EndOfSongPolicy::Stop`. Every format has the same gap — ProTracker, Scream Tracker 3 and
Impulse Tracker all wrap too — so it belongs to a sequencer task with its own effect on
scanned song lengths, not to the XM processor. F5 deliberately did not reach for it: making
the XM crate wrap on its own would give one format a rule the other four do not have.

## M6 — IT (task G3)

Task G3 landed IT playback: the effect processor, instrument articulation, New Note
Actions, duplicate checks and voice stealing, wired into 121 pinned corpus cases. Thirteen
pass with every field enforced and twenty-six more pass with `position` waived under
accuracy-policy entry **D67**. The eighty-two records below are the remainder, grouped by
the first field that diverges once `position` is set aside, so each group is one piece of
work rather than one case. They belong to **G6** (`plans/engine/M6-task-G6-*`), the
conformance-repair task M6 exits through; none of them is an accepted deviation, and every
excluded case is still executed on every run, so a fix makes its exclusion fail as stale.

The categories were measured at the same commit as the exclusion rows. A case appears in
exactly one group — the one its *first* divergence names — so fixing a group will move
cases into another group before it moves them into the pass column.

## G3-IT-001

**The volume chain's last bit — 24 cases.** The first difference is one or two steps of
the trace's 0..64 volume axis, usually while an envelope is attacking or a fadeout is
running. StarPlayer implements OpenMPT's chain (`Vol · VEV · NFC · CV · SV · IV · GV`
folded as `muldiv(vol14 · GV256, CV · insVol, 1 << 20)`); libxmp folds the sample and
instrument global volumes in *after* the envelope and the fadeout
(`QUIRK_INSVOL`, `src/player.c:1102`) and divides by different powers of two on the way.
The quantisation to six bits then turns a sub-percent difference into a whole step. G6
should settle which order Impulse Tracker itself uses — `it2play`'s `Music_*` volume code
is the authority — and either match it or record the difference as a deviation with a
one-step tolerance.

## G3-IT-002

**The filter envelope and the `Zxx` cutoff — 16 cases.** The first difference is two or
three steps of the 0..255 cutoff axis while a filter envelope is running. Both players
compute the same product — OpenMPT's `cutoff · (envModifier + 256) / 256` and libxmp's
`filter.cutoff · filter.envelope >> 8` are algebraically identical — so the difference is
the *envelope interpolation*: libxmp interpolates node values pre-multiplied by four
(`src/loaders/it_load.c:664`) and StarPlayer interpolates the model's −32..32 values scaled
by eight. libxmp's own `ZxxSecrets` test comment records that its filter-envelope handling
is wrong ("libxmp right shifting the cutoff by the filter envelope range instead of
deriving coefficients off of the product"), so G6 must decide this against OpenMPT rather
than against the oracle, and may have to record it as a deviation.

## G3-IT-003

**The sounding voice set — 12 cases.** The first difference is a channel or a virtual
channel that one player has sounding and the other does not: a New Note Action that should
not have allocated a background voice, a voice that should have been freed when its fadeout
reached zero, or a duplicate check that should have killed one. The virtual-channel
numbering the adapter reproduces (research point 1) is only as good as the set of voices it
is numbering, so a difference here also shifts every later background row.

## G3-IT-004

**Note and sample selection — 10 cases.** The first difference is the note or the sample a
channel is playing: the empty-note-map-slot rules (`kITEmptyNoteMapSlot`,
`kITEmptyNoteMapSlotIgnoreCell`), the lone-instrument-number rules
(`kITInstrWithoutNote`, `kITMultiSampleInstrumentNumber`), and the portamento sample-swap
rules (`kITPortamentoInstrument`, `kITPortamentoSwapResetsPos`) interact, and G3 implements
them from OpenMPT's description rather than from a per-case reading.

## G3-IT-005

**Pitch — 8 cases.** The first difference is the `period` column by more than one whole
period, so it is a real pitch error rather than D64's axis resolution: a slide that ran on
the wrong tick, a portamento target that was not consumed, or an arpeggio phase that is out
of step.

## G3-IT-006

**Row flow and the tick budget — 8 cases.** The first difference is `row`, `tick-in-row` or
`frame`: a pattern loop, break or jump that resolved differently, or a row whose tick
budget differs because `SEx`, `S6x` or a `Txx` tempo slide was counted differently. The
four `pattern_loop_it*` fixtures that exercise D65's dialects are **not** in this group —
three of them pass with the D67 waiver — so this is the `Cxx`/`Bxx`/`SBx` interaction
rather than the dialect selection.

## G3-IT-007

**Panning — 1 case.** The first difference is the pan column outside surround, so it is the
pan envelope's asymmetric scaling, pitch/pan separation, or the pan swing.

## G3-IT-008

**Sample position beyond the D67 waiver — 3 cases.** Two cases diverge on `position` by
more than the accumulated-rounding difference D67 covers, so something other than the
frequency's last bit moved the voice. The third, `openmpt-it-bidi-loops`, is the fixture
that exists to test it: Impulse Tracker's software mixer plays a ping-pong loop **one
sample short** of the file's, which OpenMPT emulates and G3 did not implement.
