# M4 — Generalise to a synthesis engine

| Field | Value |
|---|---|
| Goal | MIDI in, SMF playback, keyboard triggering, source multiplexing |
| Estimate | 1.5u |
| Depends on | M3 |
| Blocks | M5, M9 |

## Why here and not earlier

This is where `Instrument` is extracted as a real trait and the musical event vocabulary
is finalised — **informed by two real format implementations rather than by speculation
in M0**. That is the "no trait until its second implementation exists" rule
(`plans/product/01-technical-architecture.md` §10.1) being cashed in.

It is also the milestone that turns StarPlayer from a module player into an engine. The
architecture has been built for it throughout — `EventSource`, the two-way event
vocabulary, the control clock — but nothing has exercised the non-tracker path yet.

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
