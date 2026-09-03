# M5 — F6: The order-list wrap in the conformance trace

| Field | Value |
|---|---|
| Milestone | M5 ([master plan](M5-master-plan.md)) — a follow-up F5 surfaced |
| Status | Ready — not yet dispatched |
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
