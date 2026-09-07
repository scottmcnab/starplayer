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

## M5 — XM (tasks F2, F5 and F6)

Task F2 landed the XM effect processor and wired all 93 XM cases the pinned tree's
`compare_mixer_data*` calls name; 72 passed and twenty were recorded here as `F2-XM-001`
through `F2-XM-012`. Task F5 settled all but four of them, and task F6 — the order-list wrap
on the conformance trace path — closed `F2-XM-009`: **88 of the 93 now pass**, three are
accepted deviations (`libxmp-xm-reverse-xm` under D45, `openmpt-xm-panmemory` under D79 and
`openmpt-xm-patloop-break` under D88) and the two cases below are what is left.

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
| `F2-XM-009` — FastTracker 2's stale `song.pBreakPos` | **Closed by F6.** The conformance trace now follows each format's own end-of-song rule, so an XM wraps to its restart position instead of ending. `openmpt-xm-patloop-weird` passes; `openmpt-xm-patloop-break` is accuracy policy **D88**, where FastTracker 2 carries the loop target across the wrap and libxmp does not |
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

## F2-XM-009 — resolved by M5-F6: the order list running out is not the end of an XM

`openmpt-xm-patloop-break` and `openmpt-xm-patloop-weird` both run off the end of the order
list. `PatLoop-Weird.xm` has a single order and a `D03` on row 0; `PatLoop-Break.xm` has two
and its second pattern simply ends. FastTracker 2's `getNextPos` wraps in both cases — `if
(++song.songPos >= song.songLength) song.songPos = song.songLoopStart;` — and the oracle
records three passes of each, while the trace path built every sequencer with
`EndOfSongPolicy::Stop` and stopped after the first pass.

**Resolved in the trace path, not in the player.** `starplayer_offline`'s
`trace_sequencer_settings` now chooses the end-of-song rule per format — an XM wraps to
`XmFormatExtra::restart_position`, an IT wraps past its `0xFF` end marker, and MOD, S3M and
MTM keep ending with the order list — through a new
`EndOfSongPolicy::WrapKeepingBreakRow`, which is the reference players' own wrap: it carries
the row a `Bxx` / `Cxx` / `Dxx` asked for across the wrap the way `getNextPos` applies
`song.pBreakPos` to the order it lands on. Task D2's rule for *hosts* is untouched —
`EndOfSongPolicy::Loop` and `Stop` behave exactly as before, no format crate's
`sequencer_settings` changed, and the scan, the nine goldens and the web player are all
unaffected.

`openmpt-xm-patloop-weird` now passes, waiving `frame` for **D87** (libxmp's dump clock
restarts at every pass of a wrapping module) and `position` for D42. `openmpt-xm-patloop-break`
agrees for its whole first pass and then diverges on **D88**: FastTracker 2 carries the `E62`
loop target left in `song.pBreakPos` across the wrap, so pattern 0 is re-entered at row 12
where libxmp re-enters it at row 0. It is an accepted deviation, not a known failure — it is
accuracy policy §1's own `kFT2LoopE60Restart` rule, which `starplayer-xm`'s
`a_pattern_loops_target_row_starts_the_next_pattern` pins.

## M6 — IT (tasks G3 and G6)

Task G3 landed IT playback and wired 121 pinned corpus cases into the harness; 39 passed
and 82 were recorded here. Task G6 was the repair pass: **55 now pass** — 14 with every
field enforced and 41 with `position` waived under accuracy-policy entry **D67** — and the
66 records below are the remainder, regrouped by the first field that diverges once
`position` is set aside. Each group is one piece of work rather than one case. None of them
is an accepted deviation, and every excluded case is still executed on every run, so a fix
makes its exclusion fail as stale.

G6 landed, and the accuracy policy now carries, **D80** (the volume chain's last bit, a
one-step `volume` tolerance), **D81** (the row-delay tick counter), **D82** (ModPlug Tracker
1.16's IT pattern-loop profile), **D83** (the ping-pong cycle and `S9E`/`S9F`), **D84**
(libxmp's ambiguous zero cutoff), **D85** (a two-step `cutoff` tolerance) and **D86** (a
four-unit `pan` tolerance). The engine repairs G6 made are, in order of cases moved: the
filter is left engaged when a fully-open cutoff arrives without a note trigger; `SCx`
silences a voice without taking it off the channel; a lone *sample* number retriggers in
sample mode on the same rule instrument mode already used; the `u`, `v` and `y` MIDI-macro
letters read the channel's previous-tick volume and pan rather than the current voice's;
Envelope Carry copies the preceding voice's counters; `S9E`/`S9F` reverse playback; and the
`Zxx` macro parser handles every letter substitution and more than one internal message per
macro.

The categories were measured at the same commit as the exclusion rows. A case appears in
exactly one group — the one its *first* divergence names — so fixing a group will move
cases into another group before it moves them into the pass column.

## G6-IT-001

**The volume chain — 18 cases.** `libxmp-it-duplicate-check-transpose`,
`libxmp-it-finevolrowdelaymultiple`, `libxmp-it-portamento-after-cut-fade-cg`, `libxmp-it-portamento-after-keyoff-cg`,
`libxmp-it-portamento-envelope-reset-cg`, `libxmp-it-portamento-nna-sample`,
`openmpt-it-cut-carry`, `openmpt-it-envelope-loops`, `openmpt-it-fade-portamento`,
`openmpt-it-fine-volume-column-slide`, `openmpt-it-instrument-number-change`,
`openmpt-it-macro-last-note`, `openmpt-it-note-off-portamento`, `openmpt-it-note-off-two`,
`openmpt-it-off-portamento`, `openmpt-it-off-portamento-compatible-gxx`,
`openmpt-it-volume-column-memory`, `openmpt-it-volume-envelope-carry`.

D80 settled the *arithmetic*: the one-step tolerance covers the two chains' different
grouping, and the volume products themselves now agree. Fading at IT's rate — `NFC`
counts down from 1024 by the raw `FadeOut` every tick, not by a thirty-second of it —
and routing `===` through the real key-off moved `libxmp-it-channel-filter`,
`libxmp-it-fade-env-reset` and `libxmp-it-note-delay-nna` into the pass column. What is left is **state**, not
rounding — every remaining first difference is more than one step, and most are far more
(`openmpt-it-off-portamento-compatible-gxx` is 55 against 4). The cases cluster on
envelope and fadeout *reset* rules around note-off, note-cut and tone portamento
(`kITResetFilterOnNoteOff`, `kITEnvelopeReset`, `kITPortamentoInstrument`), on volume
column memory, and on Envelope Carry.

**What would settle it:** read OpenMPT's `NoteChange`/`InstrumentChange` reset matrix case
by case against `ResetEnvNoteOffOldFx*.it`, `wnoteoff.it`, `noteoff3.it` and
`CarryNNA.it`, and check each against the dump tick by tick. G6 fixed the two that had a
common cause; the rest need one reading each.

## G6-IT-002

**The sounding voice set — 14 cases.** `libxmp-it-g00-nosuck`,
`libxmp-it-instrument-memory-default`, `libxmp-it-l00-nosuck`, `libxmp-it-noteoff-nosuck`,
`openmpt-it-empty-slot`, `openmpt-it-envelope-off-length`, `openmpt-it-gxx-test`,
`openmpt-it-no-map`, `openmpt-it-note-off-instrument`,
`openmpt-it-portamento-just-stopped-note`, `openmpt-it-s7x-instrument-number`,
`openmpt-it-scx`, `openmpt-it-stopped-instrument-swap`, `openmpt-it-zxx-secrets`.

Twelve of the fourteen are `expected false, actual true`: libxmp drops the channel's voice
where StarPlayer keeps it sounding. G6 established the two halves of the rule that are
certain — libxmp reclaims a zero-volume voice only when its channel index is past the
module's own tracks (`libxmp_virt_setvol`, `src/virtual.c:325`), and OpenMPT's `NoteCut`
leaves the note on the channel — and moving to it fixed `openmpt-it-scx`'s first four
ticks. The remaining shape is a note whose instrument maps the note to **no sample**:
OpenMPT's `kITEmptyNoteMapSlot` returns from `NoteChange` and leaves the old note playing
(`Snd_fx.cpp:1883-1889`, test cases `emptyslot.it`, `PortaInsNum.it`, `gxsmp.it`), which is
what StarPlayer does, while libxmp's dump shows the channel gone.

**What would settle it:** whether libxmp cutting the channel on an empty note-map slot is
Impulse Tracker's behaviour or libxmp's own. OpenMPT's comment and its four test cases say
the note keeps playing; if that holds against a real IT 2.14 capture, twelve of these
become a documented deviation rather than a repair, and the exclusion rows should say so.
`openmpt-it-note-off-instrument` and `openmpt-it-portamento-just-stopped-note` are the
opposite direction — libxmp keeps a voice we free — and are separate work.

## G6-IT-003

**Pitch — 12 cases.** `libxmp-it-double-toneporta`, `libxmp-it-mpt-it-double-toneporta`,
`libxmp-it-note-after-cut`, `libxmp-it-portamento-sustain`, `libxmp-it-storlek-01`,
`libxmp-it-storlek-24`, `openmpt-it-carry-nna`, `openmpt-it-envelope-loop-escape`,
`openmpt-it-portamento-instrument-number`, `openmpt-it-portamento-offset`,
`openmpt-it-portamento-sample`, `openmpt-it-retrigger`.

The first difference is the `period` column by more than one whole period, so it is a real
pitch error rather than D64's axis resolution. Two sub-shapes: a *double tone portamento*
(a `Gxx` in both the effect and the volume columns of one row — `libxmp-it-double-toneporta`
and its ModPlug sibling), and a portamento whose **target** was taken from the wrong note or
sample after an instrument or sample change (`openmpt-it-portamento-sample`,
`openmpt-it-portamento-instrument-number`, `libxmp-it-storlek-24`). G6 corrected the linear
slide's table domains — fine amounts below 16 index the fine table directly rather than
splitting into coarse and fine factors — which is why `openmpt-it-retrigger` is now two
units out rather than a whole semitone.

**What would settle it:** `kITPortamentoInstrument` and `kITMultiSampleInstrumentNumber`
read against `PortaInsNum.it` and `PortaSmpChange.it`, and a decision about which column's
`Gxx` wins when a row carries two.

## G6-IT-004

**The filter envelope and the `Zxx` cutoff — 7 cases.** `libxmp-it-fade-env-reset-carry`,
`libxmp-it-smooth-macro`, `openmpt-it-extreme-filter`, `openmpt-it-filter-envelope-carry`,
`openmpt-it-filter-envelope-reset`, `openmpt-it-filter-nna`,
`openmpt-it-filter-reset-carry`.

G6 removed nine of the sixteen G3 recorded: D84 settled libxmp's ambiguous zero, D85 the
`frq_envelope < 0xfe` hold, and the processor now leaves the filter engaged when a fully
open cutoff arrives without a note trigger. Three of the seven left are the **carried**
filter envelope: with Envelope Carry set, libxmp does not reset `xc->f_idx`
(`src/read_event.c:112`), so on the *song's first note* it evaluates the envelope at tick
1 where its uncarried counterpart would evaluate at tick 0 — its index is zeroed by
`calloc` rather than set to `-1` — and the whole trace is one envelope tick ahead of ours.
The other four are two to four cutoff steps out later in the trace, past D85's bound.

**What would settle it:** for the carry three, whether OpenMPT starts a carried envelope at
position 0 on the first note (it does, `chn.PitchEnv.nEnvPosition` is zero-initialised),
which would make this a deviation and not a repair. For the other four, a tick-by-tick read
of `filter-nna.it` and `extreme-filter-test-1.it` against OpenMPT's
`SetupChannelFilter` return value.

## G6-IT-005

**Note and sample identity — 6 cases.** `libxmp-it-cut-invalid-ins`,
`libxmp-it-portamento-after-cut-fade`, `libxmp-it-portamento-after-keyoff`,
`libxmp-it-portamento-envelope-reset`, `libxmp-it-test-keyoff`,
`openmpt-it-envelope-reset`.

All six are `expected Some(n), actual None`: libxmp still names a note on the channel where
StarPlayer's processor has already let go of its voice state, so the trace reports no note
even though the mixer voice is alive. This is the same lifetime question as `G6-IT-002`
seen from the other side, narrowed to the *fadeout* path — G6 fixed the `SCx` path but left
a note whose fadeout reached zero being freed outright.

**What would settle it:** deciding whether a foreground voice whose fadeout has reached
zero stays on the channel (libxmp and OpenMPT both keep it) and, if so, when it is ever
reclaimed. The naive change — keep every silent foreground voice — was measured during G6
and regressed `libxmp-it-high-offset-memory`, so the reclaim rule has to come with it.

## G6-IT-006

**Row flow and the tick budget — 4 cases.** `libxmp-it-play-it-globalvol-marker`,
`libxmp-it-storlek-17`, `libxmp-it-storlek-22`, `openmpt-it-s77`.

The first difference is the `frame` column beyond its 45-frame tolerance, so it is a tick
budget rather than a row sequence — `libxmp-it-storlek-22` is 7100 against 3445 at the very
first tick, which is a whole row of speed. Three of these have an initial-speed or
initial-tempo reading behind them; `openmpt-it-s77` drifts only after 192 ticks and is an
envelope-pause interaction with the budget. D65's four `pattern_loop_it*` fixtures and
D82's ModPlug one are **not** in this group: all five now agree on the row sequence.

**What would settle it:** the `frame` column for tick 0 read against the module header's
initial speed and tempo, and `S77`/`S79`/`S7B` against the row clock.

## G6-IT-007

**Panning — 2 cases.** `openmpt-it-gxx-sample-map`, `openmpt-it-gxx-sample-map-change`.

D86's four-unit envelope tolerance settled five of the six panning cases G3 recorded. These
two are 15 against 0 — a hard-panned voice, not an envelope step — on a `Gxx` that changes
which sample the note maps to, so it is the sample's own default pan being applied (or not)
across a portamento sample swap.

**What would settle it:** `kITPortamentoSwapResetsPos` and the sample default-pan rule read
against `gxsmp.it` and `gxsmp2.it`.
