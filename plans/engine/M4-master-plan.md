# M4 — Generalise to a synthesis engine

| Field | Value |
|---|---|
| Goal | MIDI in, SMF playback, keyboard triggering, source multiplexing |
| Estimate | 1.5u |
| Depends on | M2 for M4-lite; M3 for M4-full |
| Blocks | M5, M6 (M4-lite); M9 (M4-full) |
| Status | **Split 2026-09-03** — M4-lite in progress ([concurrency plan](M3-M6-concurrency-plan.md)); M4-full deferred |

## Why here and not earlier

This is where `Instrument` is extracted as a real trait and the musical event vocabulary
is finalised — **informed by two real format implementations rather than by speculation
in M0**. That is the "no trait until its second implementation exists" rule
(`plans/product/01-technical-architecture.md` §10.1) being cashed in.

It is also the milestone that turns StarPlayer from a module player into an engine. The
architecture has been built for it throughout — `EventSource`, the two-way event
vocabulary, the control clock — but nothing has exercised the non-tracker path yet.

## M4-lite and M4-full (owner decision, 2026-09-03)

M4 is split so that XM (M5) and IT (M6) can be built concurrently, without waiting for
MIDI. **M4-lite** is only the shared machinery those two formats need; **M4-full** is
everything below that plays MIDI, and it stays deferred until the owner pulls it.

The decision that shapes M4-lite: **no `trait Instrument` yet.** Architecture §5.3's
sketch has one consumer that cannot call concrete code — a MIDI-driven sample player — and
that consumer is M4-full's. XM and IT are tracker processors that already write voice
parameters through `TickContext`, so each keeps its own per-voice articulation state
(envelope positions, fadeout, key-off, auto-vibrato phase) in a parallel array indexed by
`VoiceId` and advances it inside its own `tick()`. The engine never runs an envelope, and
no cross-format envelope branch exists. The trait is extracted in M4-full with the MIDI
sample player as its non-tracker implementation, which is the moment its second real
implementation exists (§10.1).

| M4-lite task | Deliverable |
|---|---|
| [E1](complete/M4-task-E1-instrument-and-sample-model.md) | The XM+IT instrument and sample model in `starplayer-model`, designed against both specs at once; sample sustain loops; ping-pong guard frames; the shared xorshift32 |
| [E2](complete/M4-task-E2-linear-frequency-and-format-dialects.md) | The `2^(n/768)` linear-frequency table; XM/IT `FormatDialect` variants |
| [E3](complete/M4-task-E3-voice-lifecycle-and-trace-v2.md) | `VoicePool::iter_mut`, `ChannelTable::detach_foreground`, `VoiceTag.sample: u16`, single-sourced voice capacity, trace format v2 with per-voice lines |

Deliverables 1–8 below are M4-full, except that deliverable 1's "extract `Instrument`" now
reads "extract `Instrument` from `XmProcessor`, `ItProcessor` and the MIDI sample player".
`SourceMux` already landed at M1-B3; its acceptance case is M4-full's.

## Deliverables

1. **Extract `Instrument`** (architecture §5.3) with `ModInstrument`, `S3mInstrument` and
   `MtmInstrument` as its first implementations, plus a simple sample-player instrument
   for MIDI-driven use.
2. **`starplayer-midi`** — the MIDI byte codec (a *converter* to and from `Event`, never
   the internal representation; architecture §2.3) and an SMF parser.
3. **`SmfSequencer`** as an `EventSource`.
4. **`ExternalEventQueue`** (architecture §3.2) — the SPSC-backed source that live MIDI,
   computer-keyboard input and, later, a plugin host all feed. Pre-materialised
   timestamped event lists are exactly the right shape *at the edge*, and this is where
   that shape is admitted.
5. **Live MIDI input** via `midir` on native hosts, and Web MIDI in the browser.
6. **Computer-keyboard triggering** — a tracker-style keyboard map, so the web player and
   the TUI can play notes.
7. **`SourceMux`** — merge several sources with the deterministic tie-break from
   architecture §3.1 rule 3. "Play a module and jam over it" is the acceptance case.
8. **The synthesised control clock** for pure-MIDI use, where no tracker tick exists to
   advance envelopes (architecture §5.4).

## Exit criteria

A MIDI file plays; a connected MIDI keyboard triggers notes from a loaded module's
instruments; both at once, over a playing module, with deterministic ordering.

## Out of scope

Non-sample instruments (M10). Plugin hosting (M9).
