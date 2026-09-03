# M5 — F5: XM conformance repairs

| Field | Value |
|---|---|
| Milestone | M5 ([master plan](M5-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Landed 2026-09-04; owner listening check outstanding |
| Depends on | F2 landed (72 of 93 XM cases pass; 20 recorded as `F2-XM-001`..`F2-XM-012` in `conformance/known-failures.md`) |
| Blocks | M5 exit (`--strict` must name no XM known failure that is not a documented dialect gap) |
| Parallel with | G3 (different crates), the G6 IT repairs |
| Recommended model | Claude Opus (accuracy work against two disagreeing references) |
| Verified by | agent (`cargo xtask conformance --offline --strict` reporting only the pre-existing `C2-S3M-009` and any XM entry this task converts into a documented dialect gap), then reviewer, then the owner listens |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first.

Task F2 landed FastTracker 2 playback in `crates/starplayer-xm` and wired 93 XM cases
into the conformance harness: 72 pass (46 of them with a per-field waiver naming an
accuracy-policy entry D42–D47), one is an accepted deviation, and **20 are executed every
run and recorded as known failures** `F2-XM-001`..`F2-XM-012` in
`conformance/known-failures.md`. This task is to F2 what M2-C9 was to B4: settle each of
the twelve entries by fixing the processor, by recording a deviation with an
accuracy-policy entry (D48 onward), or by turning a tracker-specific behaviour into a
`FormatDialect`/`QuirkSet` field the way C5 did — never by hiding a case.

The references are unchanged: ft2-clone's `ft2_replayer.c` is the specification of what
FastTracker 2 does; OpenMPT's `Snd_fx.cpp`/`Sndfile.h` (`kFT2*`) and its wiki name the
compatibility behaviours; libxmp's dumps are the oracle, and where libxmp follows OpenMPT
rather than FT2 the case is a deviation, not a bug. Read
`plans/engine/complete/M5-task-F2-xm-playback.md` (its research resolutions, especially 2
and 5), `plans/engine/complete/M2-task-C9-s3m-conformance-repairs.md` (the shape of a
repair pass) and `plans/engine/complete/M2-task-C5-quirks-and-tempo-models.md` (how a
dialect becomes a `QuirkSet` field: a policy entry, a detection rule, a test that flips
exactly the cases that name it).

## Deliverables

1. **Each `F2-XM-*` entry settled**, in this order of preference: a processor fix that
   makes the case pass with no waiver; a waiver with a D48+ policy entry citing both
   sources by file and line; a `FormatDialect` variant / `QuirkSet` field for a
   tracker-specific behaviour (`F2-XM-001` ModPlug/MadTracker/rst tone portamento,
   `F2-XM-002` ModPlug/Skale pattern flow, `F2-XM-007` Skale offset) detected from the
   tracker name the loader already classifies. An entry that cannot be settled against a
   real FastTracker 2 (F2 named `F2-XM-003`) stays a known failure with the reason
   sharpened and a note of what evidence would settle it.
2. **`F2-XM-012`** (a zero-byte oracle in the pinned corpus): confirm it is the corpus,
   not the harness, and make it an accepted harness record like `C2-MOD-001`.
3. `--strict` output: only `C2-S3M-009` plus whatever this task explicitly leaves as a
   sharpened known failure.
4. No change to the MOD/S3M/MTM results (33 of 47) or to any golden except the XM one if
   a fix legitimately changes the synthetic fixture's render (then regenerate it and say
   why in the commit).
5. Docs: policy §1/§3 updated, `known-failures.md` pruned, `plans/README.md` M5 row,
   `M5-master-plan.md` exit criteria.

## Research points

1. For each dialect candidate, confirm from `Load_xm.cpp` what OpenMPT keys the
   behaviour on (tracker name, version, both) and reuse E2's `FormatDialect` variants.
2. `F2-XM-009` (`kFT2LoopE60Restart`, the stale break position): decide whether it is
   FT2 behaviour the canonical profile should reproduce (it is FT2's) or a bug the policy
   records — the §1 "deliberate quirks reproduced" test applies.
3. `F2-XM-008` vibrato ramp amplitude and `F2-XM-011` the looping envelope after 240
   ticks: check ft2-clone line by line before touching the processor.

## Verification

```sh
cargo test -p starplayer-xm
cargo xtask conformance --offline
cargo xtask conformance --offline --strict      # report exactly what it still names
cargo xtask goldens --check
cargo test --workspace
cargo xtask ci --job rt-safety
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
```

Report the before/after XM table, every entry's disposition, and every policy entry or
quirk field added. **Do not commit** — the reviewer commits.

## Out of scope

IT (G3/G6). New XM features. Changing tolerances the D42/D44 arithmetic does not justify.

## Research resolution

### 1. What OpenMPT keys each dialect candidate on — **verdict: libxmp keys all of them on one bit and the tracker name, OpenMPT splits it finer; E2's variants needed two additions**

`Load_xm.cpp:619-691` is OpenMPT's classifier and `xm_load.c:846-893` plus `:936-1013` is
libxmp's, and they are the same predicate read two ways. Both look at the twenty-byte
tracker name at offset `0x26` and nothing else, except for one case each: OpenMPT requires
`fileHeader.size == 276` alongside `FastTracker v2.00   ` before it will call a file
FastTracker 2, and libxmp revises `FastTracker v2.00   ` to ModPlug Tracker 1.16 after
reading the body.

Where they differ is what the answer *selects*. libxmp has one bit, `QUIRK_FT2BUGS`, set
only for `FastTracker v2.00`, `Fasttracker II clone` and an `OpenMPT ` prefix, cleared for
a ModPlug-detected file, and consulted in ten places; plus two flow bits,
`FLOW_JUMP_NO_ROW_SET` for Skale and `FLOW_MODE_MPT_116` for ModPlug. OpenMPT resets
individual `kFT2*` behaviours by name — `kFT2PortaNoNote` and `kFT2Arpeggio` for MadTracker
2, `kFT2ST3OffsetOutOfRange` and `kFT2Arpeggio` for Skale — and calls
`m_playBehaviour.reset()` wholesale only for a ModPlug-made file
(`if(m_dwLastSavedWithVersion && !madeWith[verOpenMPT])`, `Load_xm.cpp:1046-1050`), which
is libxmp's ModPlug case exactly.

The oracle is libxmp, so StarPlayer models libxmp's bit — but taken apart into the three
behaviours a corpus case actually observes, rather than kept as one opaque flag: a `QuirkSet`
with `xm_double_portamento_doubles_volume_column_rate`,
`xm_offset_past_sample_end_stops_channel` and `xm_loop_target_becomes_next_break_row`, plus
`xm_pattern_loop` for the flow. Each of the first two and the fourth flips exactly the cases
that name it; the third is the `kFT2LoopE60Restart` bundle-mate, pinned by unit tests
because its own two fixtures are blocked elsewhere (research point 2).

E2's variants covered `FastTracker2`, `MilkyTracker`, `ModPlugXm` and `OpenMptXm`; two more
were needed. `SkaleTracker`, because `data/pattern_jump_skale_break.xm` needs
`FLOW_JUMP_NO_ROW_SET` and nothing else does. And `UnknownXm`, because the fall-through had
to stop being `FormatDialect::Unknown`: an XM tracker name that is *not* FastTracker 2 is
evidence, not the absence of it, and three fixtures — `mt2_xm_double_toneporta.xm`,
`rstst_double_toneporta.xm` and `3xx-no-old-samp-noft.xm` — depend on it. `ModPlugXm` was
reused for both of libxmp's ModPlug paths, the `FastTracker v 2.00  ` name and the
`is_mpt_116` body detection, because libxmp gives them identical quirks.

Two judgement calls are recorded in the policy. `MilkyTracker` keeps FastTracker 2's quirks,
following OpenMPT rather than libxmp — libxmp turns them off only because its test matches
the exact FastTracker 2 name, MilkyTracker is a deliberate clone, and neither MilkyTracker
corpus case turns on the difference. And MadTracker 2 and rst's SoundTracker land on
`UnknownXm` rather than getting OpenMPT's finer split, because no corpus case distinguishes
them: all three non-FastTracker-2 fixtures agree with libxmp's single bit.

Detection was checked against every one of the 93 XM fixtures: 35 `FastTracker2`, 48
`OpenMptXm`, 2 `MilkyTracker`, 4 `ModPlugXm`, 1 `SkaleTracker`, 3 `UnknownXm` — the same
ten non-FastTracker-2 files libxmp's own classifier picks out, and no others.

### 2. `F2-XM-009` and `kFT2LoopE60Restart` — **verdict: it is FastTracker 2's, it is reproduced, and it is not what the two fixtures were failing on**

It is FastTracker 2's, so §1 applies and the canonical profile reproduces it. The mechanism
is not the one `F2-XM-009` described, though. `ft2_replayer.c`'s `patternLoop` writes
`song.pBreakPos` when the loop **jumps** (`param != 0`), not when `E60` marks the target,
and `getNextPos` clears `pBreakPos` only inside the `row >= currNumRows || posJumpFlag`
branch — so the *loop target* survives the pattern it was set in and becomes the next
pattern's first row. OpenMPT models it the same way (`state.m_nextPatStartRow =
chn.nPatternLoop` in `PatternLoop`, cleared by `PositionJump` and `PatternBreak`, consumed
in `SetupNextRow`). libxmp reaches almost the same place from the other end, writing
`f->jumpline = row` on the `E60` itself (`flow.c:66-70`), which agrees whenever the target
is the row that set it — which is every case either fixture exercises.

It is spelled in the XM processor rather than in `PatternFlowState`, because
`PatternFlowState::begin_row` clears `jump_row` every row and the whole point of this quirk
is that FastTracker 2's copy does not. `XmProcessor::carried_break_row` is the surviving
value and `XmProcessor::carried_jump` is `getNextPos`'s "the pattern ran out of rows" branch:
where the flow state asks for no jump and the row is the pattern's last, it emits
`Jump::break_to_row(carried)`, which the sequencer already reads as "the next order, at this
row".

Neither `openmpt/xm/PatLoop-Break.xm` nor `PatLoop-Weird.xm` can see it, which is why they
are still recorded. Both run off the end of the order list, where FastTracker 2's
`getNextPos` wraps to `song.songLoopStart` and StarPlayer's sequencer ends the song — task
D2's deliberate rule under `EndOfSongPolicy::Stop`. Hand-simulating `PatLoop-Weird.xm`
against `ft2_replayer.c` reproduces the oracle's entire `0 3 1 0 3 1 2 3 1 2 3 1 2` row
sequence once the wrap is assumed and needs no other change, and `PatLoop-Break.xm` needs
the same wrap at a *natural* pattern end. Making the XM crate wrap on its own was rejected:
ProTracker, Scream Tracker 3 and Impulse Tracker all wrap too, so the question belongs to
`starplayer-engine`'s `move_to_order` and to a task that owns its effect on scanned song
lengths. The record says so, and the quirk itself is pinned by
`a_pattern_loops_target_row_starts_the_next_pattern` and
`a_break_clears_the_carried_loop_target_and_another_tracker_never_sets_it`.

### 3. `F2-XM-008` and `F2-XM-011` — **verdict: neither was what its record said; both are one tick of phase, and both are now accuracy-policy entries**

**`F2-XM-008` was not an amplitude difference at all.** FastTracker 2's 32-entry
`vibratoTab` (`ft2_tables.c:52`) is `0, 24, 49, 74, … 253` and libxmp's 64-entry `sine_wave`
(`lfo.c:27`) begins with exactly those thirty-two values; `doVibrato`'s `(tmpVib *
vibratoDepth) >> 5` over FastTracker 2's linear period and libxmp's `sine · (depth << 2) /
512` over its own scale — whose unit is four of FastTracker 2's — are the same amplitude to
within a unit. Comparing the whole trace through the harness's own projection instead of the
two shift counts, **all sixteen divergent records of 768 are a row's tick zero**:
`doVibrato` reaches the period only from `JumpTab_TickNonZero[4]`, so FastTracker 2's first
tick of a row keeps the previous `outPeriod` and neither reads nor advances the LFO, while
libxmp applies its vibrato on every frame including the first (`player.c:1184-1199`; only
`QUIRK_PROTRACK` suppresses it, and XM does not have it). That is accuracy policy **D76**,
and the same shape as `C2-S3M-009`'s second part.

**`F2-XM-011` is one envelope step, not two units of interpolation.** The divergence is
`EnvLoops.xm`'s instrument 2, whose sustain point is point 0: on the tick the key-off
releases it, `updateVolPanAutoVib` finds `volEnvTick` equal to that point's own tick, reloads
the exact point value, recomputes the segment's Q8 delta and sets `envDidInterpolate`, which
suppresses the accumulate for that tick — so FastTracker 2 reports 64 of 64 where libxmp,
recomputing `y0 + (y1 - y0)·(x - x0)/(x1 - x0)` from a position already one step in and
truncating toward zero, reports 62. That is D44's mechanism plus one whole step of phase,
which is why the two units exceeded D44's bound. Accuracy policy **D78**.

Two of the remaining records changed shape in the same way once the traces were read rather
than the summaries. **`F2-XM-010`** is not about `ED0` — rows 18 to 28 of both patterns,
which are the `ED0` rows the fixture exists to test, agree exactly. Every divergent record
is on a row carrying both `EDx` and a volume-column slide, and it is libxmp deferring the
delayed row's *whole event*, volume column included, to the delay tick (`player.c:779-812`,
`:1619-1622`) where FastTracker 2 latches `ch->volColumnVol` before `getNewNote` returns and
OpenMPT models the same thing as `kFT2VolColDelay`. Accuracy policy **D77**. And
**`F2-XM-003`**'s second fixture, `data/ft2_note_off_sustain.xm`, was never the key-off at
all: it was the placeholder instrument's fadeout (`F2-XM-005`), and it now passes with every
field enforced.

### 4. `F2-XM-012` — **verdict: the corpus, and now an accepted record**

`openmpt/xm/PanMemory.data` is zero bytes in the pinned tree while `PanMemory.xm` sounds two
notes at row 4 and its own comment says they "should be panned hard right", so there is no
expectation to compare against. The harness is not at fault: an empty dump *is* a legitimate
oracle — libxmp writes a line only for a sounding channel, and
`openmpt/xm/DelayCombination.data` is deliberately empty and passes with every field
enforced — so relaxing the reading would silently disarm that case. Recorded as accuracy
policy **D79** and moved from a known failure to an accepted deviation, still executed on
every run.
