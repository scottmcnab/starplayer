# M5 — F6: The order-list wrap in the conformance trace

| Field | Value |
|---|---|
| Milestone | M5 ([master plan](M5-master-plan.md)) — a follow-up F5 surfaced |
| Status | Landed 2026-09-04 |
| Depends on | F5 landed |
| Blocks | The last two `F2-XM-009` cases (`openmpt-xm-patloop-break`, `openmpt-xm-patloop-weird`); possibly IT cases in G6 |
| Recommended model | Claude Opus (touches `PatternSequencer::move_to_order`, shared by every format, and the scanned song length) |
| Verified by | agent (conformance for all five formats unchanged except the cases it unblocks; goldens unchanged; D1/D2 timeline tests), then reviewer |

## Context for a fresh agent

Read `AGENTS.md`, then `plans/engine/complete/M3-task-D1-song-timeline-and-loop-detection.md`
and `M3-task-D2-natural-end-versus-loop.md`: running out of order list is *the end of the
song* for the timeline (D2), and the conformance trace path runs every module under
`EndOfSongPolicy::Stop`. FastTracker 2's `getNextPos` instead wraps to the header's
`restart_position` (`XmFormatExtra::restart_position`), and libxmp's oracle dumps for
`PatLoop-Break.xm` and `PatLoop-Weird.xm` continue past that wrap, so the trace ends early
and the cases cannot pass (`conformance/known-failures.md` `F2-XM-009`). IT has the same
shape through its order list's `ORDER_END`.

The question is not whether the *player* wraps — `sequencer_with_quirks` already sets
`restart_order`, and the live sequencer with a timeline installed does what D2 says — but
what the **conformance trace** should do: follow the oracle's own end-of-song rule per
format (libxmp's `xmp_play_frame` keeps going through the restart position until its loop
detector fires) without changing D2's rule for hosts.

## Deliverables

1. `TraceOptions` (or `SequencerSettings` on the trace path) gains a per-format end-of-song
   policy that mirrors the oracle: XM wraps to `restart_position` (FT2 `getNextPos`), IT
   wraps to order 0 past `ORDER_END`'s marker per `ITTECH.TXT`, MOD/S3M/MTM keep today's
   behaviour exactly (their 47 cases must not move).
2. The two `F2-XM-009` cases pass or become documented deviations; `known-failures.md`
   updated.
3. Goldens, timelines (`song_timeline`, `scan_song`) and the web player are untouched —
   this is the trace path only. Prove it with the existing D1/D2 tests.

## Verification

```sh
cargo xtask conformance --offline          # MOD/S3M/MTM unchanged; XM +2 or documented
cargo xtask goldens --check
cargo test --workspace
cargo xtask ci --job host-tests
cargo xtask ci --job clippy
```

**Do not commit** — the reviewer commits.

## Research resolution

### 1. `TraceOptions` or `SequencerSettings` — **verdict: the trace path's own `SequencerSettings`, with no new knob**

`starplayer_offline::trace_loaded` built one hard-coded `SequencerSettings` for every format
(`restart_order: 0`, `end_of_song: EndOfSongPolicy::Stop`). It now calls a new private
`trace_sequencer_settings(&Module)`, which chooses per `ModuleFormat`: XM wraps to
`XmFormatExtra::from_header(...).restart_position`, IT wraps past its `0xFF` end marker to
order zero, and MOD, S3M and MTM keep `Stop` exactly as they had it. No public option was
added: nothing in the workspace wants the old rule for XM or IT, a `TraceOptions` field with
one caller would be dead API, and a capture that disagrees with the harness is a debugging
trap — `starplayer trace` and `cargo xtask conformance` now dispatch identically, which is
what the harness's own header comment promises.

### 2. Reusing `EndOfSongPolicy::Loop` — **verdict: no; the wrap has to carry the break row, and the player must not**

`Loop` was not enough, and the first attempt proved it: with `Loop`, `PatLoop-Weird.xm` sat
on row 0 forever, because `move_to_order`'s wrap branch discards the row the jump asked for
and restarts the pattern at row zero. `ft2_replayer.c`'s `getNextPos` does the opposite — the
same branch that wraps `song.songPos` to `song.songLoopStart` first assigns `song.row =
song.pBreakPos` — so a `D03` on the only order re-enters the song at row 3, and the capture
then reproduces the oracle's whole `0 3 1 0 3 1 2 3 1 2 3 1 2` row sequence.

That difference could have been folded into `Loop` itself, and ProTracker and Impulse Tracker
would agree with it. It was not, because `Loop` is what every format crate's
`sequencer_settings` gives the **player**, and changing it would change what a host without a
timeline hears — the golden renders included. So the third variant,
`EndOfSongPolicy::WrapKeepingBreakRow`, exists and only the trace path selects it. `Loop` and
`Stop` are byte-for-byte what they were: no format crate changed, `scan_song` and
`scan_timeline` are untouched, the D1/D2 timeline tests pass unchanged and all nine goldens
verify. Whether the *player*'s `Loop` should carry the break row too is a real question for a
later task; it is not this one's, and nothing in the corpus can answer it.

### 3. The two `F2-XM-009` fixtures — **verdict: one passes, one is accuracy policy D88**

`openmpt-xm-patloop-weird` **passes**, waiving `frame` (D87) and `position` (D42). Its wrap
row comes from a `D03`, which sets `posJumpFlag`, so both players wrap to row 3 and every
enforced field agrees for the whole trace.

`openmpt-xm-patloop-break` agrees for its whole first pass and then parts company on
**D88**. Its pattern 0 row 12 carries `E60` and its pattern 1 row 3 carries `E62`; the loop
jump back to row 12 is still in `song.pBreakPos` when pattern 1 runs off the two-entry order
list, so FastTracker 2 re-enters pattern 0 at row 12 — F5's own reading of `patternLoop` and
`getNextPos`, section 1 of the accuracy policy, and `starplayer-xm`'s
`a_pattern_loops_target_row_starts_the_next_pattern`. libxmp re-enters it at row 0 and every
later record shifts with it (first divergence, with `frame` and `position` waived, tick 174
channel 1 field `instrument`, 1 against 3). It is therefore an **accepted deviation**, not a
known failure, and `F2-XM-009` is closed.

A second oracle difference had to be written down to get there. libxmp timestamps each dump
record with `xmp_frame_info.time`, the time of the *position within the song*, so a wrapping
module replays its timestamps: `PatLoop-Weird.data` runs 78, 156, 78, 156, 234, 312 … and
`PatLoop-Break.data` reaches 2720 and starts again at 140. StarPlayer's trace carries the
monotonic output frame, which is the only reading compatible with design goal 3. That is
**D87**, and it is why both cases waive `frame`; the harness's `C2-MOD-001` re-anchor then
aligns them on `(row, tick_in_row)`, the axis the fixtures are actually about.

### 4. IT — **verdict: the rule is in place and no pinned IT case moves**

All 121 IT cases are byte-identical to the run before the change — same dispositions, same
first divergences, same divergent-tick counts. libxmp stops its dump at the first wrap for
every IT fixture in the pinned tree, so the extra ticks the capture now records are never
paired against anything. The IT rule is written down where it belongs rather than left for
G6 to rediscover; `ITTECH.TXT`'s `255` end marker and OpenMPT's and libxmp's restart from
the top are what it follows.

### 5. `restart_order` doubles as the *start* order — **verdict: a real conflation, deliberately left alone**

`PatternSequencer::new` positions the sequencer with `move_to_order(settings.restart_order,
0)`, so the field that says where a song *restarts* also says where it *starts*. For XM that
is wrong against FastTracker 2, which begins at song position 0 whatever `songLoopStart`
says. Nothing here changes it: every XM in the pinned corpus has restart position 0 (checked,
all 93), and the XM **player** already reads the same field into the same setting, so the
trace and the player agree rather than disagreeing. Splitting `start_order` from
`restart_order` would change playback for any XM with a non-zero restart position, which
deliverable 3 forbids. It belongs to a follow-up.

### 6. A capture that no longer stops on its own — **verdict: accepted, and documented where the caller can see it**

An XM or IT trace taken with `TraceOptions::ticks` of `None` now runs to `MAX_CAPTURE_TICKS`
and reports `TraceError::TickLimit` where it used to end with the order list. That is already
the outcome for any module that jumps backwards — most real ones — and the conformance
harness always passes the oracle-derived tick budget, so nothing in CI is affected. The
`ticks` field's documentation and `starplayer trace --ticks`'s help both say so now.
