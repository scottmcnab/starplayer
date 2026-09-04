# M4 — Generalise to a synthesis engine

| Field | Value |
|---|---|
| Goal | MIDI in, SMF playback, keyboard triggering, source multiplexing |
| Estimate | 1.5u |
| Depends on | M2 for M4-lite; M3 for M4-full |
| Blocks | M5, M6 (M4-lite); M9 (M4-full) |
| Status | M4-lite **landed** 2026-09-03; M4-full **in progress** — E4 landed 2026-09-04, E5–E7 below outstanding |

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

## M4-full — the task graph (planned 2026-09-04)

Everything M4-full needs from the engine is landed: `SourceMux` with its deterministic
tie-break (M1-B3), `ControlClock` (M1-B3, unconsumed), the `Event`/`TimedEvent` vocabulary
(M0-A2, declared and unused by the tracker path), `starplayer-rt`'s SPSC ring, the
`2^(n/768)` table for pitch bend (E2), `VoicePool::iter_mut` and `detach_foreground`
(E3), `starplayer-host`'s `Player` on both the cpal and the wasm host (D4, D9), and the
five format crates with their instrument and sample models (E1, F1, G1). The
`starplayer-midi` crate is an empty shell with its `midi`/`smf` facade features wired.

### Decisions that shape the tasks

1. **`Instrument` is committed in E4 with two implementations**, satisfying §10.1:
   `SampleInstrument` (MOD, S3M and MTM: one sample, one volume) and `MappedInstrument`
   (XM and IT: the note→sample and transpose maps, per-sample tuning). Neither runs
   envelopes, NNA or auto-vibrato: a MIDI-driven XM or IT instrument with its format's
   articulation is M11's deliverable (architecture Q4 stays open for M10). The tracker
   processors do **not** implement the trait; they remain the format-owned tick code the
   M4-lite decision made them.
2. **A MIDI-driven source carries its own control tick.** Architecture §5.4 wants the
   tracker's tick to be the control tick when one is playing; for a sample player with no
   envelopes the only per-tick work is note-off ramps, so E4's `InstrumentRack` ticks at
   the engine's `ControlClock` interval (~1 ms) whether or not a tracker is in the mux, and
   §5.4 is amended to say so. Revisit when M11 gives MIDI instruments envelopes.
3. **MIDI channels live above the module's.** `MIDI_CHANNEL_BASE = 48`: MIDI channel `n`
   is engine `ChannelId(48 + n)`, so a module of up to 48 channels and sixteen MIDI
   channels coexist in the 64-lane table and the telemetry snapshot shows both. A wider
   IT shares its top lanes with MIDI; documented, not prevented.
4. **Pitch convention.** MIDI note 60 plays a sample at its reference rate (Scream
   Tracker's C-4); XM/IT maps and per-sample tuning apply on top. Pitch bend is ±2
   semitones through the linear-frequency table. Velocity is linear.
5. **An SMF needs instruments.** There is no General MIDI bank until M10; `starplayer
   play song.mid --instruments module.it` plays the file with that module's instruments,
   program numbers indexing them.
6. **Hosts stamp events.** `ExternalEventQueue` carries absolute frames; `Player::send_event`
   stamps `output_frame + lead` (two quanta by default). Late events dispatch at the
   current frame and count a warning.

### Tasks

| Task | Deliverable | Depends on | Model |
|---|---|---|---|
| [E4](complete/M4-task-E4-instrument-and-midi-source.md) | **Landed 2026-09-04.** `Instrument` trait, `SampleInstrument`, `MappedInstrument`, `InstrumentRack`, the `EventFeed` trait, `ExternalEventQueue`, `MidiSource` (an `EventSource` over any event feed with the rack inside), the control tick, channel base, telemetry, an offline determinism test with scripted events | — | Opus |
| [E5](complete/M4-task-E5-midi-codec-and-smf.md) | `starplayer-midi`: the byte codec to and from `Event`, the SMF parser and tempo map, `SmfSequencer`; facade `midi`/`smf` arms; `.mid` in the CLI's `info`/`render`/`play` with `--instruments` | E4 | Sonnet |
| [E6](M4-task-E6-live-input.md) | `Player::send_event`; `midir` input on the cpal host and the CLI's `--midi`; Web MIDI and the tracker-style keyboard map in the web player through a new ring opcode | E4 | Opus |
| [E7](M4-task-E7-jam-mode.md) | Jam mode: `Player::jam` installing a `SourceMux` of the module's sequencer and the `MidiSource`; the CLI and web player switches; the two-source block-size determinism test; docs; M4 exit | E4, E5, E6 | Sonnet |

E5 and E6 are independent of each other and can run concurrently once E4 lands; E7 closes
the milestone. Exit: a MIDI file plays with a module's instruments; a keyboard triggers
notes from a loaded module's instruments; both at once over a playing module, with
deterministic ordering.

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
