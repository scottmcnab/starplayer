# M3 — D2: A song whose order list runs out *ends*; only an explicit jump *loops*

| Field | Value |
|---|---|
| Milestone | M3 (native surfaces / offline) — follow-up to D1, pulled forward for the web player |
| Status | Landed 2026-09-03; owner listening check outstanding |
| Depends on | D1 (song timeline, loop detection, `AtEnd`), W2 (progress slider, Repeat toggle) |
| Blocks | Owner listening check of Repeat-off behaviour |
| Recommended model | Claude Opus (touches the sequencer's dispatch path and the loop detector) |
| Verified by | agent (`cargo test --workspace`, `cargo xtask ci`, goldens unchanged), then owner listening check |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first: the
design invariants there (sample-exact ticks, buffer-size-independent output, no
allocation/locks/panics in `render()`) are non-negotiable. Then read
`plans/engine/complete/M3-task-D1-song-timeline-and-loop-detection.md` — this task
corrects one decision D1 made — and `plans/apps/complete/W2-task-progress-slider.md` for
how the web player consumes the result.

### The owner's report (2026-09-03)

> I just tested NICETUNE.S3M with Repeat off, and the total duration shows as 0:30, but
> with Repeat on it shows 0:25. With Repeat off, the last 5 seconds of the track is
> continued playing from the beginning with a 5 second fade. If Repeat is off and there
> is no explicit pattern jump, this should stop playback immediately.

`NICETUNE.S3M` is in `crates/starplayer-s3m/tests/fixtures/`. Its order list simply runs
out; nothing in it jumps backwards.

### Why it happens

The MOD, S3M and MTM sequencers are built with `EndOfSongPolicy::Loop`
(`crates/starplayer-s3m/src/processor.rs:873`, `crates/starplayer-mod/src/processor.rs:962`).
When the order list runs out, `PatternSequencer::move_to_order`
(`crates/starplayer-engine/src/sequencer.rs`, "The order list came round") sets
`last_arrival = RowArrival::Wrapped` and moves to the restart order. The loop detector
then treats *any* arrival onto an already-played row — `Wrapped` included — as
`Visit::Looped` (`crates/starplayer-engine/src/timeline.rs:273-291`, and the unit test at
`:769` says so in words: "wrapping onto a played row is the loop point"). So the scan
reports `EndReason::Looped { target: order 0 }`, telemetry reports `SongEnd::Loops`, the
web page adds the fade to the displayed length (`apps/starplayer-web/www/app.js`
`updateProgress`, `fadesAtEnd`), and with Repeat off the sequencer under `AtEnd::FadeOut`
plays straight on into the second pass while the host fades it over `SONG_FADE_SECONDS`.

D1 chose this deliberately ("Keep the `EndOfSongPolicy::Loop` wrap exactly as it is — the
detector's `Wrapped` visit is what turns it into the loop point"). The owner has now
decided otherwise: **running out of order list is the end of the song, not a loop.**

### The rule

- The song **loops** when a row already played is reached **through the song's own
  flow**: a `Bxx` / `Cxx` / `Dxx` jump (`RowArrival::Jump`), a pattern loop
  (`RowArrival::PatternLoop`), or the ordinary `Sequential` / `NextOrder` advance after
  such a jump. Nothing changes for these: `EndReason::Looped`, `SongEnd::Loops`, Repeat
  off fades over the second pass, Repeat on wraps at the loop point.
- The song **ends** when the order list runs out (`RowArrival::Wrapped`), whatever row the
  restart order points at and whether or not that row has been played. This includes a
  `Cxx` / `Dxx` break on the last order and a `Bxx` to an order at or past the list's end
  (ProTracker's `mt_PosJump` clamps such a jump to a restart as well), and an S3M order
  list that reaches its `0xFF` terminator. It also includes what D1 already called a
  natural end: the order list running out under `EndOfSongPolicy::Stop`, and a stop
  marker (`F00`, an unplayable row).
- With **Repeat on** (`AtEnd::Continue`) an ended song restarts from the restart order
  with the elapsed clock rebased, exactly as a looped song wraps today.
- With **Repeat off** (`AtEnd::FadeOut` or `AtEnd::Stop`) an ended song **stops at the end
  frame**: no second pass, no fade, no ten-second ring-out. The transport stops through
  the host's ordinary 64-frame click-free glide and rewinds to the top, so the next Play
  starts from the beginning.
- Without a timeline (a host that never scanned — the goldens, the conformance trace, a
  bare `PatternSequencer`) nothing changes at all: `EndOfSongPolicy::Loop` still wraps and
  keeps playing, `EndOfSongPolicy::Stop` still stops.

## Deliverables

1. **`Visit::Wrapped` and `EndReason::Ended`.** In `crates/starplayer-engine/src/timeline.rs`
   the detector answers `Visit::Wrapped` for a `RowArrival::Wrapped` arrival before it
   looks at whether the row was played (a wrap ends the pass regardless), and the scan
   loop turns that into a new `EndReason::Ended` with `end_frame = visit.mark.frame` — the
   same frame `Looped` records today, the first tick after the last row, so `NICETUNE.S3M`
   still scans to the same length. Decide whether `Ended` carries the restart
   `SongPosition`; it is not needed by anything below, so leave it out unless a consumer
   needs it, and say which in the Research resolution. Update `EndReason`'s and
   `SongTimeline`'s doc comments, `loop_length_frames` (an ended song has no repeating
   section — `None`), and the unit test at `:769` to the new semantics; add tests for a
   run-out order list, a `B00` on the last row (still `Looped`), a `Dxx` past the end
   (`Ended`), a `Bxx` past the end (`Ended`), and an S3M `0xFF` terminator (`Ended`).

2. **The sequencer at the end.** In `crates/starplayer-engine/src/sequencer.rs`, where the
   dispatch path matches on the visit (`if !matches!(visit, Visit::Looped | Visit::Budget)`
   and the `match self.at_end` after it), handle `Visit::Wrapped`: set `end_reached`;
   under `Continue` call `wrap_at_loop_point(frame)` and keep playing (this already rebases
   onto `self.position`, which `move_to_order` has set to the restart row, and re-arms
   the detector); under `FadeOut` and `Stop` return `false` so the wrapped row's tick is
   **not** dispatched and no further events are reported — the same thing `AtEnd::Stop`
   does at a loop point today. Read `end_naturally` and keep the two ends consistent:
   after this task the only difference between them is which policy the sequencer was
   built with. Preserve the no-timeline behaviour exactly (see the rule above); check who
   reads `song_looped` and keep its meaning honest. Telemetry: map `EndReason::Ended` to
   `SongEnd::Stops` (`sequencer.rs` around `:1178`). Nothing here may allocate.

3. **The web host stops instead of fading.** In `crates/starplayer-host-wasm/src/lib.rs`
   `process`, the block that arms the song fade must arm it **only when the song loops**
   (`snapshot.transport.song_end == SongEnd::Loops`). When `end_reached` is set with any
   other `song_end` and the mode is `FadeOut` or `Stop`, stop the transport the way the
   Stop button does: the 64-frame transport glide, then the typed `Command::Stop` via the
   existing `pending_engine_stop` path, then `request_seek(SeekKind::Frame(0))` so Play
   restarts from the top — reuse the Stop-button code rather than duplicating it. Guard it
   the same way the fade is guarded (only while the transport is running, arm once), so a
   stopped engine whose snapshot still says `end_reached` cannot re-stop or re-rewind.
   This also fixes the pre-existing ten-second hang at an `F00` / order-list-under-`Stop`
   natural end, which used the same fade path. Update the doc comments and
   `apps/starplayer-web/README.md`'s description of Repeat.

4. **The web page.** `apps/starplayer-web/www/app.js` needs no logic change if the flags
   are right: `fadesAtEnd` already keys on `SONG_FLAG_LOOPS`, so the displayed length of
   `NICETUNE.S3M` becomes 0:25 with Repeat off, and `endReachedStopped` already resets the
   slider when the transport stops at the end. Verify both by reading the code, and fix
   any comment that says a run-out is a loop.

5. **Offline renderer.** In `crates/starplayer-offline/src/lib.rs` `RenderLength` handles
   `EndReason::Ended` like `Stopped`: one pass to `end_frame`, no fade, and `repeat_count`
   extra passes if the code already supports repeats for a stopped song (do not add a
   feature; match the `Stopped` arm). The test near `:947` asserts a fixture "loops"; if
   that fixture merely runs out, switch the test to a synthetic module with an explicit
   `B00` and add its twin asserting a run-out module renders exactly one pass with no
   fade. `song_timeline` on `NICETUNE.S3M` reports `Ended` and the same `end_frame` as
   before this task (assert the frame count, taken from the current build before you
   change anything).

6. **Goldens and conformance unchanged.** The goldens render ten fixed seconds without a
   timeline and the conformance trace runs under `EndOfSongPolicy::Stop`; both must be
   byte-identical. Do not regenerate anything. If a golden changes, stop and report.

7. **Docs.** Grep `plans/` and `apps/starplayer-web/README.md` for "loop point", "wraps",
   "Repeat" and "restart" and correct every statement that a run-out order list is the
   loop point; `plans/product/01-technical-architecture.md` if it describes the end-of-song
   policy; the D1 archive gets a one-paragraph *Superseded by D2* note at the top of its
   post-landing section rather than a rewrite.

## Research points

1. **The restart order under Repeat on.** A MOD whose restart byte points into the middle
   of the song (say order 2) and whose order list then runs out: under `Continue` the song
   restarts at order 2 with the elapsed clock rebased to order 2's frame, and the second
   pass ends at the same order-list end. Confirm `wrap_at_loop_point` does this and that
   the *third* pass also works (the detector was re-marked before the restart row, so the
   second wrap must again be `Visit::Wrapped`, not a spurious `Looped`).
2. **A wrap onto an unplayed row.** If a `Bxx` skipped over the restart order on the first
   pass, the wrap lands on a row the detector has not marked. It is still `Ended`. Confirm
   the timeline's `frame_at` for the restart row returns `None` in that case and that
   `wrap_at_loop_point`'s `None` arm (origin reset, detector reset) is the right thing.
3. **`Visit::Budget` and `EndReason::Budget`.** Unchanged, but confirm a wrap that happens
   *after* the detector has given up is still reported as `Ended`, not `Budget`.

## Verification

```sh
cargo test --workspace
cargo xtask ci
```

- All new tests pass; the buffer-size-independence test and the goldens pass with nothing
  regenerated; the conformance count in `plans/README.md` does not move.
- `cargo xtask wasm` builds.
- Owner listening check (not the agent's): `NICETUNE.S3M` with Repeat off shows 0:25, plays
  once and stops cleanly at the end with the slider back at 0:00; with Repeat on it shows
  0:25 and restarts seamlessly. A module with an explicit `Bxx` loop still fades over five
  seconds with Repeat off.

## Out of scope

- A "ring out the last notes before stopping" grace period. The owner asked for an
  immediate stop; a later task can add a short tail if wanted.
- Changing `SONG_FADE_SECONDS` or the fade shape.
- Any change to the loop detector's pattern-loop budget.

## Research resolution

Recorded at implementation time; each answer is the decision, not a summary of the
question. There is also a fourth entry the deliverables asked for a decision on.

### 1. The restart order under Repeat on — **confirmed; every pass after the first ends the same way**

`wrap_at_loop_point` does exactly what deliverable 2 assumed, and the third pass works.
On a `Visit::Wrapped` under `AtEnd::Continue` the cursor has already been moved to the
restart row by `move_to_order`, so `wrap_at_loop_point` looks that row up in the timeline,
rebases `song_origin` onto its scanned frame, calls `reset_marking_before` (which re-arms
the detector and marks only the rows *before* that frame) and then `mark_visited` on the
restart row itself. The re-arm is what makes the second wrap fire: `Visit::Wrapped` sets
`armed = false` on its way out, and `reset_marking_before` sets it back. It cannot degrade
into a spurious `Looped` either, because a `Wrapped` arrival is answered before the map is
consulted at all — the marks the second pass lays down are irrelevant to it.
`a_restart_order_in_the_middle_of_the_song_wraps_onto_its_own_frame_every_pass` drives
three passes of a four-order song restarting at order 2 and asserts the position, the
`end_reached` pulse and the rebased elapsed frame on each.

One correction to the premise. The scenario "a restart byte pointing into the middle of the
song" cannot arise from a loader today and could not be spelled in one sequencer either:
`PatternSequencer::new` **starts** the song at `settings.restart_order`, so a song whose
restart order is 2 also begins at order 2 and its scanned `frame_at(2, 0)` is 0 — the
rebase is then to zero, not to a mid-song frame. `restart_order` is hard-coded to 0 by
every format builder and D1 put the MOD Noisetracker restart byte out of scope. The test
therefore scans with `restart_order: 0` and plays back with `restart_order: 2`, which is
the only way to exercise a non-zero rebase target until that byte is read; the note in the
test says so.

### 2. A wrap onto an unplayed row — **still `Ended`; the `None` arm is right, and is defensive rather than reachable**

It is `Ended`: `Visit::Wrapped` never asks whether the row was played, so an unmarked
restart row ends the pass exactly like a marked one. And `wrap_at_loop_point`'s `None` arm
— `song_origin = frame`, `detector.reset()` — is the right thing: a row the scan never
reached has no elapsed position of its own, so the honest answer is to start the elapsed
clock afresh at zero rather than to invent a position, and to forget every mark rather than
re-mark against a frame that does not exist. `a_wrap_onto_an_unplayed_row_still_ends_the_song_and_restarts_its_clock`
pins both, and pins that the next wrap still fires afterwards.

But the scenario as written — "a `Bxx` skipped over the restart order on the first pass" —
cannot happen through the scan-then-play flow a host uses, for the same reason as research
point 1: the wrap target *is* the start of the song, so the scan's very first `RowMark` is
the restart row and `frame_at` on it always answers `Some`. The `None` arm is reachable
only when the sequencer's restart order and the installed timeline disagree — a playback
`restart_order` the scan never visited (what the test builds), an empty timeline, or a
timeline scanned from different quirks — and for the `Visit::Looped` path after a seek into
an unreachable "hidden" order. It stays.

### 3. `Visit::Budget` and `EndReason::Budget` — **a wrap can never be reported as `Budget`, in either order**

Unchanged, and the two cannot collide. A `RowArrival::Wrapped` clears `loop_arrivals` at
the top of `visit` (it is one of the arrivals that leave a pattern loop), so by the time
the budget test would run the counter is zero: the budget can never fire on the same visit
as a wrap however close it was to biting. The `Wrapped` check is placed before the budget
check anyway, so the precedence is explicit rather than incidental; a comment says why both
orderings are equivalent. `a_wrap_on_the_brink_of_the_pattern_loop_budget_is_still_a_wrap`
drives `MAX_PATTERN_LOOP_ARRIVALS` pattern-loop arrivals and then wraps.

The other half of the question — a wrap that happens *after* the detector has given up —
is not reachable in a scan: `scan_timeline` breaks out of its loop on `Visit::Budget`, so
the timeline's end is `Budget` and no later wrap is ever seen. In a live sequencer playing
past a `Budget` end the detector is disarmed, every later visit is `Visit::Repeat`, and the
sequencer plays on; that is D1's behaviour and D2 does not change it.

### 4. Does `EndReason::Ended` carry the restart `SongPosition`? — **no**

Left out, as the deliverable's default. Nothing consumes it: `loop_length_frames` answers
`None` for an ended song, the offline `RenderLength` restarts a repeat from frame 0 rather
than from a target, the telemetry wire carries only a `SongEnd` tag, and the sequencer
already knows where to restart because `move_to_order` has put the cursor there. A payload
no reader reads would also have to be kept honest against `restart_order` and against
`resolve_order`'s skip-marker walk, for nothing. `EndReason::Ended` is a unit variant, and
`EndReason::Stopped` stays beside it as the same kind of end reached under
`EndOfSongPolicy::Stop`.

## Post-landing notes (2026-09-03)

Residuals from the review, all pre-existing in kind and left as recorded follow-ups:

- **Repeat-toggle race, about one row wide.** Under `Continue` the sequencer's
  `end_reached` pulses true for the ticks of the wrapped row. Unticking Repeat inside that
  pulse makes the host read `FadeOut` with `end_reached` set and stop at once. Before D2
  the same race armed the fade instead. A proper fix has the host distinguish "reached on
  this pass" from "still latched", which is a design change rather than a patch.
- **`AtEnd::Stop` with a looping song** leaves the transport at unity gain with no rewind
  once the sequencer stops itself. The page never sends `AT_END_STOP`, so it is
  unreachable from the web player; an embedder using `Stop` should mirror the host's
  end-stop path.
- **A bare sequencer under `FadeOut` with no timeline** now stops at a wrap where it used
  to play on. Both hosts install a timeline before playback and the conformance trace
  runs under `EndOfSongPolicy::Stop`, so no caller in the repo sees the difference.
