# StarPlayer — Technical Architecture

The source of truth for engine design. Task files record *how*; this records *what and
why*. A change that contradicts this document needs an amendment here, in the same
commit.

---

## 1. The central problem: keeping events sample-exact

MOD/S3M effects — per-tick portamento, vibrato, volume slides, retrigger, tremor,
note-cut, note-delay — only sound right if each tracker tick is applied at an exact
output sample frame. Applying ticks at audio-buffer granularity is audibly wrong and,
worse, non-deterministic across buffer sizes.

### 1.1 The original already solved this

`SB_IRQ_Handler` (`S3MLIB.ASM` ~5634) fills each DMA buffer in slices bounded by the
remaining tick gap:

```
@@fine:  if _SB_GapCount == 0 { __UpdateTracker(); SB_ProcessTracks(); _SB_GapCount = _SB_GapLength }
@domix:  ecx = min(_SB_GapCount, _SB_BufCount)
         Mixer_8bitMono(edi, ecx)
         edi += ecx; _SB_GapCount -= ecx; _SB_BufCount -= ecx; loop
```

with `SetSBTempo` (~5621) computing `_SB_GapLength = (mixing_rate * 10 / bpm) >> 2`.
That is "split the output block at tick boundaries", in 1996. Multiple ticks per buffer
and a tick spanning two buffers both fall out correctly. We copy it.

### 1.2 The render loop

```rust
pub fn render(&mut self, out: &mut OutputBuffer) {
    let mut done = 0;
    while done < out.frames() {
        self.drain_commands();                      // SPSC, RT-safe
        let mut zero_advance = 0u32;
        while self.sources.next_event_frame() == Some(self.frame) {
            self.sources.dispatch(self.frame, &mut self.ctx);
            zero_advance += 1;
            if zero_advance > MAX_ZERO_ADVANCE { self.force_advance(); break }
        }
        let gap = self.sources.next_event_frame()
            .map(|f| (f - self.frame) as u32)
            .unwrap_or(u32::MAX);
        let n = gap.min((out.frames() - done) as u32);
        self.voices.accumulate(self.buses.window(done, n));
        self.sources.advance_to(self.frame + n as u64);
        self.frame += n as u64;
        done += n as usize;
    }
}
```

Note what is *absent*: the DSP graph. See §1.4.

**Two clocks (M1-B3).** `self.frame` above is really two counters. The **output clock**
counts frames handed to the host and never stops; the **source clock** is what event
sources report against, and it stops while the transport is paused. They are equal for an
engine that has never been paused. The split is what makes `Command::Stop` implementable
at all: sources report *absolute* frames, so freezing the musical clock is the only way to
pause without replaying every missed tick in a burst on resume — which the zero-advance
guard would survive but no listener would.

### 1.3 Tick length is a policy, not a constant

`(rate * 10 / bpm) >> 2` truncates twice. At 44100 Hz and 130 BPM it yields 848 where
the true value is 848.077 — roughly 1.3 seconds of drift over a four-minute song. Both
behaviours are wanted, so tick length is a policy:

```rust
pub trait TempoModel {
    /// Frames per tick, Q32.32.
    fn frames_per_tick(&self, sample_rate_hz: u32, tempo_bpm: u16, speed: u8) -> u64;
}

pub struct ExactFixedPoint;  // default: rate * 2.5 / bpm in Q32.32, remainder accumulated
pub struct St3Truncating;    // reproduces (rate * 10 / bpm) >> 2
pub struct ItModern;         // openmpt semantics including tempo slides
```

The fractional remainder is carried in an accumulator so `ExactFixedPoint` never drifts.

`TempoModel` is one of only two traits committed before a second implementation exists
(§10), because it has three from the start.

### 1.4 Split for voice mixing, quantise for DSP

**This is the highest-probability silent failure in the whole design.** If per-channel
reverb, the compressor and the SIMD kernels consume whatever ragged 3-, 17- or
411-frame segment the event split hands them, output depends on the host's buffer size:
offline will not match real-time, and neither will match across hosts.

> The engine renders on an internal fixed `RENDER_QUANTUM = 128` frames — exactly
> AudioWorklet's quantum, so it is free on the first target. Events split *within* a
> quantum for voice accumulation into per-channel buses. The DSP graph and master bus
> only ever see whole quanta. Arbitrary host block sizes are adapted by a small output
> ring.

So the real top-level loop is:

```
for each whole RENDER_QUANTUM:
    voice accumulation, split at event boundaries within the quantum
    per-channel DSP inserts   (whole quantum)
    bus summing               (whole quantum)
    master DSP                (whole quantum)
    output conversion → ring
host block is served from the ring
```

**Day-one test:** render the same module at host block sizes 1, 3, 64, 128, 4096 and
8191; assert byte-identical output. Present from M0, never weakened.

---

## 2. Three kinds of state, not two layers of events

A single timestamped event queue for everything conflates three genuinely different
things:

| | shape | rate | needs a timestamp? | crosses a thread? |
|---|---|---|---|---|
| **Timeline events** — NoteOn, CC, SMF, live MIDI | sparse, discrete | low | yes | yes |
| **Voice parameters** — pitch, volume, pan, filter | dense, continuous | every tick, every voice | no — the instant is already known | no |
| **Control commands** — load module, seek, master volume | very sparse | ~0 | no | yes |

At 32 channels × 50 ticks/s with vibrato + tremolo + volume envelope + auto-vibrato +
filter envelope, the middle row is 5,000–10,000 "events" per second that are just field
writes to a struct we already hold a pointer to. Encoding, queueing and matching those
is strictly worse than what the 80386 code did: **write field, set dirty bit.**

### 2.1 The three types

```rust
// (1) Timeline. The only thing carrying a timestamp.
pub struct TimedEvent { pub frame: Frame, pub target: Target, pub event: Event }  // Frame = u64 newtype

pub enum Target { Channel(ChannelId), Voice(VoiceId), Global }

pub enum Event {
    NoteOn  { note: Note, velocity: U0F16 },
    NoteOff { note: Note, velocity: U0F16 },
    KeyOff, FadeOut, Cut,
    PolyAftertouch { note: Note, pressure: U0F16 },
    ChannelAftertouch(U0F16),
    Controller { number: u16, value: U0F16 },
    PitchBend(I1F15),
    Program(InstrumentId),
    AllNotesOff, AllSoundOff,

    Trigger(TriggerSpec),   // tracker-native note-on: sample, offset, flags
    Param(VoiceParam),      // absolute voice-level set — the escape hatch
    Tempo { bpm: u16, speed: u8 },
    GlobalVolume(U0F16),
}

// (2) Voice state. Written directly, never queued.
pub struct VoiceParams {
    pub step: Step,          // Q32.32 sample-position increment per output frame
    pub volume: U0F16,
    pub pan: I1F15,
    pub filter: FilterParams,
    pub dirty: DirtyBits,
}

bitflags! {
    pub struct DirtyBits: u8 {
        const VOLUME = 0b0000_0001;   // was _CHN_NewVol
        const SAMPLE = 0b0000_0010;   // was _CHN_NewSamp
        const PITCH  = 0b0000_0100;   // was _CHN_NewPitch
        const PAN    = 0b0000_1000;   // was _CHN_NewPan
        const TEMPO  = 0b0001_0000;   // was _CHN_NewBPM
        const STOP   = 0b1000_0000;   // was _CHN_StopVoice
    }
}

// (3) Control plane. SPSC, its own enum, never on the timeline.
pub enum Command {
    LoadModule(Arc<Module>), Play, Stop, SeekOrder(u16), SeekRow(u16),
    SeekFrame(u64), SetAtEnd(AtEnd),
    SetMasterVolume(U0F16), MuteChannel { channel: ChannelId, muted: bool },
    SetInterpolator(Interpolator), SetTempoModel(TempoModelId),
}
```

`DirtyBits` is a direct descendant of the original's `_CHN_New*` flags
(`S3MLIB.INC:55-65`). The design is 30 years old and still correct.

### 2.2 Testability comes from a trace hook, not from the queue

The real reason to want an event stream was diffability. Take it directly:

```rust
#[cfg(feature = "trace")]
#[inline] fn trace_param(&mut self, v: VoiceId, p: VoiceParam) { self.tracer.record(self.frame, v, p) }
#[cfg(not(feature = "trace"))]
#[inline] fn trace_param(&mut self, _: VoiceId, _: VoiceParam) {}
```

Zero cost in release, a full serialisable per-tick trace in test builds.

**Trace format v2 (M4-lite task E3)** adds a second kind of row. After a tick's one ` ch=`
line per module channel come zero or more ` vc=` lines, one for every active voice that is
**no channel's foreground**, in pool slot order. IT's New Note Actions detach a channel's
sounding voice into the background, where it runs its own envelopes and fadeout owned by
nobody; before v2 such a voice appeared on no row at all and the diff harness could not
see an NNA happen. A ` vc=` row is keyed by the voice's pool slot and carries `root`, the
channel that triggered it; its other fields are the ` ch=` row's minus `act`, since a
listed voice is active by definition. A parameter write is attributed to the ` ch=` row
when its voice is that channel's foreground and to the voice's own row otherwise. Formats
that never detach a voice — MOD, S3M, MTM — emit no ` vc=` lines, so their traces differ
from v1 only in the version header and the widened `smp` field.

### 2.3 The musical layer is MIDI-*convertible*, not MIDI-*shaped*

Seven-bit velocity cannot round-trip S3M volume (0–64), IT global volume (0–128),
MOD/S3M pan (0–15 / 0–255) or IT pan (0–64 plus surround). So:

- velocity, controller values, volumes: `U0F16`;
- pitch bend: `I1F15`;
- `Note { semitone: u8, cents: i16 }` — microtonality comes free, and FM/SID/physical
  modelling will want it.

MIDI byte parsing lives in `starplayer-midi` as a **codec** that converts to and from
`Event`. MIDI's representation never becomes the internal one.

### 2.4 The musical layer and the tracker path are peers, not a pipeline

MOD/S3M/MTM patterns never produce a `NoteOn`. They produce "trigger sample N at period
P, volume V, offset O". Only XM/IT *instrument* mode has a note→sample-map indirection
resembling a MIDI program change. Drawing musical → lowering → voice-level would build a
MIDI-shaped bottleneck that costs accuracy on every format.

```
PatternSequencer ──Trigger + direct VoiceParams writes──┐
SmfSequencer / LiveMIDI ──NoteOn/CC──> Instrument ──────┼──> VoicePool ──> Buses ──> DSP
KeyboardInput ──NoteOn──────────────────────────────────┘
```

### 2.5 `Step`, not `Hz`

The mixer wants a resample increment, not a frequency. The original computes exactly
that (`SB_ProcessTracks` ~5788): `hz = 14317056 / period`, then
`HighSpeed = hz / rate` with the fractional part in `LowSpeed` — a 32.32 split across
two dwords.

```rust
pub struct Step(pub u64);  // Q32.32 sample-position increment per output frame
```

The division belongs in format code, which knows both the sample's reference rate and
the output rate, and runs at tick rate. Q16.16 Hz would also cap at 65535 Hz for no
reason.

---

## 3. Sources: pull, with absolute frames

```rust
pub trait EventSource {
    /// Absolute frame of the next event, or None if idle.
    /// Must never return a frame earlier than the engine clock.
    fn next_event_frame(&self) -> Option<u64>;

    /// Advance internal state to `frame` without producing events.
    /// `frame` is never past `next_event_frame()`.
    fn advance_to(&mut self, frame: u64);

    /// Produce everything due exactly at `frame`.
    /// Only called when `next_event_frame() == Some(frame)`.
    fn dispatch(&mut self, frame: u64, ctx: &mut EngineContext<'_>);
}
```

**Absolute frames, not deltas.** One engine-owned `u64` clock (12 million years at
48 kHz) instead of every source keeping its own elapsed-time bookkeeping and eventually
desyncing.

`dispatch` takes a **concrete** `EngineContext`, not `&mut dyn EventSink`. Only
`EventSource` itself needs to be `dyn` (sources are heterogeneous), and dyn dispatch at
~50 Hz is free.

### 3.1 Three rules

1. **Never cache the next-event frame across a dispatch.** `Txx`, `Axx`, `SEx`, `SDx`,
   `Bxx` and `Cxx` all change *when the next tick is* from inside the tick just
   processed. Compute the next boundary at the **end** of processing the current tick.
   This is also the decisive argument against pre-materialising a per-block schedule
   internally — that would be running the sequencer with extra steps.

   **How M1-B3 makes this structural rather than documented.** `PatternSequencer` is a
   three-state machine — `Ready` / `Processing` / `Stopped` — and `next_event_frame()`
   answers `None` in `Processing`. *While a tick is being processed there is no next
   boundary*, so a cached one cannot be observed, because it does not exist yet. The
   boundary is a function of the `#[must_use]` `TickOutcome` the format's effect processor
   returns, which carries the tempo and speed in effect at the **end** of the tick and is
   the only argument to the sequencer's single call site of `FrameClock::advance_tick`.
   `FrameClock` itself has no setter, so the only way to learn where the next tick lands is
   to commit the clock to it.

2. **Guard zero-advance.** S3M speed 0, `A00`, MOD `E60` self-loops,
   pattern-break-to-self, SMF zero-delta meta events: any of these can make a source
   return the same frame forever and spin the render loop *inside the audio callback*,
   permanently. Every tracker has shipped this bug.

   ```rust
   const MAX_ZERO_ADVANCE: u32 = 64;      // consecutive dispatches at the same frame
   const MAX_EVENTS_PER_BLOCK: u32 = 4096;
   ```

   On breach: force-advance one frame, set a telemetry warning flag, keep rendering.
   **Never panic** — a panic in an AudioWorklet kills audio for the page permanently.

3. **Deterministic tie-breaking.** Two sources with events at the same frame must
   dispatch in a stable order or offline ≠ real-time. Sort by
   `(frame, source_slot, sequence_within_source)`, where `source_slot` is a stable
   generational slot, not a `Vec` position that shifts on removal.

### 3.2 Pre-materialised buffers: rejected internally, adopted at the edge

VST3 and CLAP hand you a pre-sorted timestamped event list per block, and so should live
MIDI. That is exactly the right shape *at the boundary*:

```rust
pub struct ExternalEventQueue { ring: SpscConsumer<TimedEvent>, peeked: Option<TimedEvent> }
impl EventSource for ExternalEventQueue { .. }
```

The plugin and MIDI-in paths become sources implementing the same trait. The CLAP/VST
fit comes for free without contaminating the tracker path.

### 3.3 Implementations

| Source | Role |
|---|---|
| `PatternSequencer` | order list → pattern → row → tick; owns tempo/speed and the format's effect processor |
| `MidiSource<Feed>` | **landed M4-E4.** Plays any `EventFeed` through an `InstrumentRack`, and carries its own control tick (§5.4). The only `EventSource` on the musical side — a feed is not a source |
| `ExternalEventQueue` | **landed M4-E4.** An `EventFeed`, not an `EventSource`: the SPSC ring of `TimedEvent`s that live MIDI, the keyboard and a plugin host push into, with a one-event lookahead refreshed per render segment |
| `SmfSequencer` | M4-E5. The other `EventFeed`: a cursor over a MIDI file's sorted event list |
| `SourceMux` | merges several sources with the §3.1.3 tie-break — "play a module and jam over it". Landed at **M1-B3** rather than M4: it is a hundred lines, and the tie-break rule is only testable once two real sources can collide |

The split between `EventSource` and `EventFeed` is worth stating: a **source** is asked
when it next has something to do, is advanced, and dispatches into the engine; a **feed**
only answers "what is the next event, and may I have it". Everything a feed's events mean —
which instrument plays them, what the channel's controllers say, when the control tick
falls — belongs to `MidiSource`, so there is exactly one implementation of it rather than
one per input.

`starplayer::NativeSequencer` (M3-D3) is the host-side dispatch above them: one enum, one
arm per format crate compiled in, that turns a runtime `ModuleFormat` into the typed
`PatternSequencer` for it and forwards the seek and song-clock surface. `Engine` still
stores `Box<dyn EventSource>` and still cannot seek from its command handler (§1.2) — the
enum is what every host owns *outside* the engine, so a format is wired up once rather than
once per host, and a new host consumes one type.

---

## 4. The row clock

The original does the naive thing (`__UpdateTracker` ~2423):
`if _MRowDelay != 0 { dec; goto @@nonewpattern }` — the row's effects re-run on each
repeat, notes are not re-fetched. MOD `EEx`, S3M `SEx`, XM `EEx` and IT `SEx` all differ
subtly, and IT's `SEx` × `SDx` interaction is a known minefield.

Model the row's tick budget explicitly, exposing the **absolute** tick index across
repeats — that is what `Qxy` retrigger, `Ixy` tremor, `SDx` note delay and `SCx` note
cut all key off:

```rust
pub struct RowClock {
    pub speed: u8,          // ticks per row
    pub pattern_delay: u8,  // extra row repeats (SEx / EEx)
    pub tick_in_row: u16,   // 0..speed*(1+pattern_delay) — ABSOLUTE across repeats
    pub repeat_index: u8,
}

impl RowClock {
    pub fn is_first_tick_of_row(&self) -> bool { self.tick_in_row == 0 }
    pub fn is_first_tick_of_repeat(&self) -> bool { self.tick_in_row % self.speed as u16 == 0 }
    pub fn total_ticks(&self) -> u16 { self.speed as u16 * (1 + self.pattern_delay as u16) }
}
```

Each format's effect processor picks the predicate it cares about. Deliberately **not**
one shared `if tick == 0 { .. } else { .. }` across formats — that shared branch is
where every cross-format bug would live.

`Axx` mid-row changes `speed` while `tick_in_row` is already past it. The wrap rule is
stated explicitly per format rather than falling out of modulo arithmetic by accident.

### 4.1 How long is a song? The loop detector and the song clock (M3-D1)

A module does not declare its length, and no arithmetic over the file can recover one:
order lists wrap, `Bxx` jumps backwards, `SBx`/`E6x` revisit rows, `Txx`/`Axx` change how
long a tick lasts mid-song. The only way to know is to play it.

`starplayer-engine::timeline` therefore plays it, **with the real sequencer and the real
format processor and no mixing**. `scan_timeline` drives a throwaway `PatternSequencer`
tick by tick and records a `RowMark { order, pattern, row, frame, speed, tempo_bpm }` the
first time each row is reached; the result is a `SongTimeline` — the marks in play order,
a per-order index, the length of one pass, and *how* it ends (`Looped { target }`,
`Stopped`, `Budget`). The comparison is OpenMPT's `GetLength`/`RowVisitor` and libxmp's
`scan_module`, both of which run a *simplified* replay; running the real one instead means
the timeline is correct by construction under every quirk, dialect and tempo model, with
no second player to drift out of step.

The same `LoopDetector` then rides in the **live** sequencer, so "the song has been heard
once through" fires on exactly the frame the scan predicted. It is a bitset over
`(order, row)` sized when the sequencer is built — never in `dispatch` — consulted once at
the first tick of every row, before the tick runs:

* the sequencer records **how** it reached the row (`RowArrival`: `Start`, `Sequential`,
  `NextOrder`, `Jump`, `PatternLoop`, `Wrapped`);
* `PatternLoop` sets `inside_loop`, every arrival but `Sequential` clears it;
* a `Wrapped` arrival — the order list ran out — is answered **before the map is consulted
  at all**: it is `Visit::Wrapped`, and the timeline's `EndReason::Ended`, whatever row the
  restart order names and whether or not it has been played. Running out of order list is
  not something the music asked for, so it is the *end* of the song rather than a loop
  (task D2). This covers a `Cxx`/`Dxx` break on the last order, a `Bxx` to an order at or
  past the end of the list, and an S3M `0xFF` terminator;
* every other row is **marked**, but the "have I been here before?" question is only
  **asked** when not inside a pattern loop. That is what lets an `E6x` body repeat four
  times without ending the song while a `Bxx` back to an earlier order ends it immediately
  — with `EndReason::Looped`, which is the only end the host fades over;
* a pattern-loop arrival budget catches the constructions that loop for ever inside one
  pattern, including a stuck ProTracker counter.

**`EndOfSongPolicy` and `AtEnd` answer different questions**, and conflating them is the
mistake this split exists to avoid. `EndOfSongPolicy` says what *the end of the order
list* means, which is a property of the module and its format: a MOD wraps, an S3M's
`0xFF` is a real end. `AtEnd { Continue, Stop, FadeOut }` says what the *host* wants once
the song has been heard through once — the media player's repeat button — and it is
answered by the loop detector, not by the order list. `AtEnd` is inert until a host
installs a timeline, so a sequencer without one behaves exactly as it did before any of
this existed.

`FadeOut` is the one place the two ends differ. At a real loop point the sequencer plays
straight on into the second pass so the host can fade the transport over it; at an
order-list end there is no second pass to fade into, so it stops on the end frame exactly
as `Stop` does and the host stops the transport through its ordinary click-free glide
(task D2).

The **song clock** is one signed offset, `song_origin`, and `song_frame(now) = now −
song_origin`. Wrapping under `Continue` rebases the origin onto the loop point rather than
resetting a counter, so elapsed time drops back to the top of the repeating section and
the progress slider stays honest across a loop. It is signed because seeking to the middle
of a song on an engine whose monotonic clock has only just started puts the origin before
frame zero, and a saturating `Frame` would quietly report that seek as "at the beginning".

`seek_frame(song_frame, now)` resolves a frame to a row through the timeline's binary
search, seeks there, and restores that row's **speed and tempo** — and nothing else. Global
volume (`Vxx`), effect memories and sample positions are whatever `TrackerProcessor::reset`
leaves them. That is a deliberate, documented limitation and not an oversight: OpenMPT's
`eAdjust` replays global state up to the seek target, and matching it is a separate piece
of work. On seek the detector is reset and every row the scan reached *before* the target
is re-marked, so the loop point after a seek is the canonical one rather than wherever the
seek happened to land.

---

## 5. Channels, voices, instruments

Three concepts, deliberately not conflated:

- **Channel** — a logical control lane. Holds controller state, effect memories, and a
  bound instrument. A tracker pattern column is a channel; a MIDI channel is a channel.
- **Voice** — one sounding sample or oscillator, drawn from a fixed-capacity global
  pool. Allocation-free at run time.
- **Instrument** — turns channel events into voice behaviour; owns format-specific note
  semantics, envelopes, NNA and auto-vibrato.

### 5.1 IT's NNA without contaminating MOD/S3M

The clean concept is **foreground vs background**, and it lives in the **voice
allocator** — not in `Channel`, not in the mixer.

In IT a channel has one *foreground* voice that receives channel effect updates, plus
zero or more *background* voices that have been detached and only run their own
envelopes and fadeout until they die.

```rust
pub struct Channel { pub foreground: Option<VoiceId>, /* effect memories, .. */ }

// Background voices are NOT owned by the channel — they live in the pool, tagged so
// Duplicate Check can find them.
pub struct VoiceTag { pub channel: u8, pub instrument: u8, pub sample: u8, pub note: u8 }

pub enum NewNoteAction  { Cut, Continue, NoteOff, NoteFade }
pub enum DuplicateCheck { Off, Note, Sample, Instrument }
pub enum DuplicateAction{ Cut, NoteOff, NoteFade }
```

MOD/S3M/MTM simply never create a background voice. `NewNoteAction::Cut` +
`DuplicateCheck::Off` early-outs the whole DCT matching loop. Cost to the simple path:
four bytes per voice and one branch.

**As landed (M6-G3).** The shape held. `VoiceTag` gained a sixteen-bit `sample` so the
Duplicate Check can never see two different samples compare equal through a byte clamp,
and the per-voice articulation is **format-owned** rather than a field of the pool: the IT
processor keeps a `Box<[ItVoiceState]>` of `VIRTUAL_CHANNELS` (256) entries indexed by
`VoiceId::index()` and validated by the stored `VoiceId`, allocated once in the constructor
and cleared in `reset()`. Each entry holds the voice's instrument and sample, its root
channel, three envelope positions with their sustain and loop state, its fadeout, its
key-off and note-fade flags, its auto-vibrato phase and sweep, its per-note random volume
and pan swing, its filter cutoff and resonance, and the New Note Action it will be detached
with. `tick()` walks the pool once and advances every owned voice — foreground or
background — through one code path, which is what makes an NNA voice keep sounding
correctly rather than by a second copy of the envelope code.

One rule turned out to be worth stating: **`NewNoteAction::Cut` allocates nothing.**
OpenMPT does move a cut voice to a background channel so its volume ramp can bleed out, but
libxmp frees a background voice the moment its volume reaches zero
(`libxmp_virt_setvol`), and the engine's own mixer already ramps a released voice, so a cut
note never occupies a virtual channel here. That keeps the sounding voice set the same
shape as the oracle's, which the conformance adapter depends on to number background
voices at all.

### 5.2 One global pool, generational handles

Not per-instrument sub-pools — those fragment, and IT needs global stealing across all
channels against a single virtual-channel limit.

```rust
#[derive(Copy, Clone, PartialEq)]
pub struct VoiceId { index: u16, generation: u16 }

impl VoicePool {
    pub fn get_mut(&mut self, id: VoiceId) -> Option<&mut Voice>;  // None if stolen
}
```

Generational, because a voice **can** be stolen out from under its owner and every
instrument must tolerate a stale handle. Silently getting the wrong voice is a
multi-day debugging session; `Option` makes it impossible.

IT's voice-stealing heuristic is **audible** on dense modules — which note gets cut is
part of the output. It is matched to libopenmpt, not guessed.

**Q3, settled in M6-G3.** OpenMPT's `CSoundFile::GetNNAChannel`
(`soundlib/Snd_fx.cpp:2257`) is two passes over the *background* range only — a foreground
voice of another channel is never a candidate:

1. **A free voice wins outright**, taking the lowest index: `if(c.nLength) continue;` then
   `return i`. There is no round-robin.
2. Otherwise every background voice is scored `v = (nRealVolume << 9) | nVolume` — the
   14-bit post-envelope, post-fadeout mixing volume with the 0..256 note volume as a
   tie-breaker — and the **lowest score wins**. `if(c.dwFlags[CHN_LOOP]) v /= 2;` gives a
   looped sample half priority, because it will ring for ever otherwise. A voice that is
   playing but fully faded (`c.nLength && !c.nFadeOutVol`) is returned immediately, and on
   a tie the voice further through its volume envelope — or with no volume envelope at all
   — wins.
3. The threshold starts at the **stealing note's own score**, so a background voice louder
   than the note that wants its slot is never stolen and the new note simply does not
   sound. If the source channel is itself already fully faded, nothing is allocated and the
   old voice is dropped.

Schism (`player/effects.c:1640`) agrees on the shape and differs in two details: it folds
the fadeout into the score explicitly (`v = volume * fadeout_volume` for a fading voice,
`volume << 16` otherwise) rather than relying on a cached mixing volume, and it uses a
fixed 25 % threshold instead of the stealing note's own score.

**Retirement, settled in M6-G6.** The complement of stealing is when a voice *leaves* the
pool, and Impulse Tracker's rule is asymmetric: libxmp reclaims a zero-volume voice only
when its channel index is past the module's own tracks (`libxmp_virt_setvol`,
`src/virtual.c:325`), and OpenMPT's `NoteCut` under `kITSCxStopsSample` zeroes the
increment and the fadeout but leaves the note, the instrument and the sample on the
channel. `ItProcessor` therefore frees a silent **background** voice outright and keeps a
foreground one that `SCx` silenced, so the channel keeps reporting its note until another
note replaces it. It does not yet keep a foreground voice whose *fadeout* reached zero;
that is `G6-IT-005`, and it needs a reclaim rule to come with it.

`ItProcessor::choose_victim` implements OpenMPT's rule with Schism's explicit fadeout term
folded in, because StarPlayer recomputes a voice's volume from its articulation each tick
rather than caching a 14-bit mixing volume. It is **a concrete policy in `starplayer-it`,
not a trait**: design goal 8 keeps a trait uncommitted until its second real
implementation, and XM's is the same allocator with a different NNA set rather than a
different heuristic.

### 5.3 The instrument surface

**Committed in M4-full task E4**, with the two implementations design goal 8 asks for —
`SampleInstrument` (MOD, S3M, MTM) and `MappedInstrument` (XM, IT), neither of them a
tracker processor. It lives in `starplayer_engine::instrument`:

```rust
pub struct NoteParams { pub note: Note, pub velocity: U0F16, pub pan_override: Option<I1F15> }

pub trait Instrument: Send {
    fn note_on(&self, channel: ChannelId, params: NoteParams, context: &mut EngineContext<'_>) -> Option<VoiceId>;
    fn note_off(&self, channel: ChannelId, context: &mut EngineContext<'_>);
    fn control_tick(&self, channel: ChannelId, context: &mut EngineContext<'_>);
    fn set_bend(&self, channel: ChannelId, cents: i16, context: &mut EngineContext<'_>);
}
```

Four things moved from the sketch, each for a reason worth keeping:

* **`&self`, and the channel is a parameter.** An instrument is shared, immutable knowledge
  about a module's samples; everything that varies per channel — held notes, controller
  state, the bend — belongs to the `InstrumentRack` that owns it. That is what lets one
  `Box<dyn Instrument>` serve all sixteen MIDI channels at once.
* **`EngineContext` rather than `(&mut Channel, &mut VoicePool)`.** The sketch's pair is
  half of what `note_on` needs: a voice is started through `ChannelTable::trigger`, which
  owns the release-then-allocate ordering NNA depends on, and the context is already the
  thing every `EventSource` is handed.
* **`render` is gone.** No instrument renders: the mixer does, monomorphised over the path
  and the interpolator (§7.1). A `dyn` call per sample was never on the table.
* **`set_bend` arrived**, because pitch bend is the one musical gesture with no tracker
  peer at all — the rack scales the wheel by its range and the instrument re-derives the
  step from the sounding voice's own tag.

`InstrumentRack` is the channel-state half: sixteen MIDI channels at
`MIDI_CHANNEL_BASE = 48` (a module of up to 48 channels and sixteen MIDI channels coexist
in the 64-lane table), each with its program, CC7 volume, CC10 pan, CC64 sustain, pitch
bend and a fixed array of held notes. It consumes `Event`s and reports `Trigger`/`Param` as
unsupported: the tracker vocabulary is a **peer** of the musical one (§2.4), never a
lowering of it.

Neither implementation runs envelopes, fadeout, NNA or auto-vibrato. Giving a MIDI-driven
XM or IT instrument its format's own articulation is M11's deliverable, and Q4 — whether
this surface survives a non-sample instrument — is still M10's to settle.

**Owner decision, 2026-09-03** (the history this replaced) (`plans/engine/M3-M6-concurrency-plan.md`, delivered by
M4-lite task E3): the extraction moves to **M4-full**, and XM and IT are implemented
without it.

The reason is design goal 8. XM and IT are being built concurrently, so neither is
finished when the other starts, and a trait committed now would be extracted from one
implementation and a guess — which is the mistake the goal exists to prevent. The second
*genuinely different* implementation is the MIDI sample player of M4-full: a non-tracker
instrument, driven by a synthesised control tick rather than a tracker tick, which is what
will actually show whether the surface above is the right one.

Until then the division is:

- **Shared, in the engine and the mixer** — the voice pool and its generational handles,
  the channel table and the foreground binding, `ChannelTable::detach_foreground` (the
  NNA primitive), `VoicePool::iter_mut` (the per-tick walk), and the trace.
- **Format-owned, in each format crate** — all per-voice articulation: envelope
  positions, fadeout level, key-off flag, auto-vibrato phase. Each format keeps it in a
  **parallel array indexed by `VoiceId::index()` and validated by the id's generation**,
  and advances it from inside its own `TrackerProcessor::tick()` by walking
  `VoicePool::iter_mut()`. A slot the format did not allocate shows an id that does not
  match the one it stored, so it is skipped rather than adopted.

`TrackerProcessor::recommended_voice_capacity(channel_count)` is how the pool and that
parallel array are sized from one number. Its default is the channel count — MOD, S3M and
MTM sound one voice per channel and never detach — and a format with a parallel array
sizes both the array and its answer from one constant. A pool **larger** than the answer
is legal, and is what a persistent host builds (`MAX_VOICE_CAPACITY`, IT's virtual-channel
limit of 256): the format must reach its voices through `get_mut` and skip ids past the
end of its array. A pool **smaller** is legal too — fewer voices sound.

### 5.4 The control clock

XM and IT envelopes, auto-vibrato, fadeout and NNA advance **exactly once per tracker
tick** — not per sample, not per buffer. Rather than special-casing this, the control
clock is itself an `EventSource`:

- with a tracker sequencer driving, *it* is the control clock and its tick advances
  every envelope in the engine;
- with no tracker (pure MIDI or live use), the engine synthesises a control tick at a
  configured rate, default ~1 ms.

One uniform rule — "envelopes advance on control ticks" — exactly right for trackers,
perfectly adequate for synth instruments.

**Amendment, M4-full (task E4).** A **MIDI-driven source carries its own control tick**,
whether or not a tracker is in the mux. The rule above ties the control rate to the
module's tempo, which is right for a format's envelopes and wrong for a keyboard jammed
over a stopped or absent module: a MIDI channel would then tick at the mercy of somebody
else's `Fxx`, or never. `MidiSource` therefore keeps a `ControlClock` of its own at the
engine rate (~1 ms), reports `min(feed, control)` as its next event frame, and ticks it
from inside its own `dispatch`. Nothing in the engine changed for it — the render loop
already splits its segments at whatever frame a source reports — and the engine's own
synthesised clock keeps running alongside, unread by the source, so a tracker taking that
clock over cannot silence a MIDI instrument. Revisit when M11 gives MIDI instruments
envelopes and the two rates have to agree about something.

**Where this stands after M4-lite (task E3).** With a tracker sequencer driving, the
tracker tick *is* the control tick — `PatternSequencer::dispatch` calls
`ControlClock::tick_from_tracker` on every tick — and **nothing in the engine consumes it
yet**. XM's and IT's envelopes advance from inside their own `TrackerProcessor::tick()`,
which is that same tick, so the rule holds as written; what has not been built is an
engine-side consumer of a *synthesised* control tick, because the first thing that needs
one is the MIDI sample player of M4-full. The clock is driven and observable now so that
the consumer can be added without moving the tick.

---

## 6. Module memory layout

Decide this now, not later:

```rust
pub struct Module {
    blob: Box<[u8]>,               // decoded pattern data and everything non-sample
    pcm: Box<[i16]>,               // all samples decoded + delta-decoded, concatenated,
                                   //   each with N guard frames appended (loop-wrapped,
                                   //   reflected, or zeroed) so interpolators need no
                                   //   branches
    samples: Box<[SampleIndex]>,   // { pcm_offset: u32, len: u32, loop_start, loop_end,
                                   //   sustain_loop: Option<SustainLoop>, .. }
    patterns: Box<[PatternIndex]>, // { blob_offset: u32, rows: u16, channels: u8 }
    orders: Box<[u16]>,
    instruments: Box<[InstrumentDef]>,
    header: ModuleHeader,          // .. format_data: Box<[u8]>, format-owned bytes the
                                   //    engine never interprets
}
```

**Offsets, not references.** Consequences, all of which we need:

- trivially `Send + Sync`, so `Arc<Module>` hands to the audio thread with no ceremony;
- trivially hashable, so golden tests can fingerprint a loaded module;
- trivially fuzzable — a loader either produces a valid index set or an `Err`;
- mmap- and flash-friendly on ESP32, where sample data may be borrowed rather than owned;
- no pointer chasing in the mixer inner loop.

**Guard frames** are the other half: appending N frames to each sample's PCM (loop-
wrapped for a forward loop, reflected for a ping-pong loop, or zeroed for a one-shot or a
sample with a sustain loop) lets the interpolator read past the loop point without a
branch in the inner loop. A sample with a sustain loop (IT, task E1) stores its whole body
rather than truncating at a loop end, because the ordinary loop and the sustain loop may
each lie anywhere inside it; its guard is silence, and one frame of `Linear`
interpolation reads real PCM past a loop end there instead of a wrapped or reflected
copy — accepted for now, left for M7's kernels to reconsider.

Format crates keep their **native** pattern bytes in `blob`. The shared model covers
samples, envelopes, instrument definitions and a **display-only** `PatternCell` view for
UIs — deliberately *not* a shared pattern-cell model, which is exactly the mistake that
made the original's MOD playback inaccurate.

---

## 7. Mixing and DSP

### 7.1 Voice rendering

Generic over the accumulator type:

- **fixed-point** — `i16` sample, `i32`/`i64` accumulator. The canonical bit-exact
  reference path, and the embedded path.
- **float** — `f32`. The default for desktop and browser.

Interpolators, as a monomorphised parameter of the inner loop (never a `dyn` call per
sample):

| Interpolator | Use |
|---|---|
| `None` (nearest) | retro character, cheapest |
| `Linear` | default; the golden-hash reference |
| `Cubic` (Hermite) | quality real-time |
| `Sinc` (windowed) | offline / high-quality rendering |

Output conversion handles 8/16/24/32-bit integer and f32, mono and stereo, with
dithering for the reduced-depth cases.

**The host owns the choice, not the engine (M1-B9).** Path, interpolator and output format
are type parameters of `Engine`, so selecting one is a re-instantiation, not a field write:
`Command::SetInterpolator` stays flagged as unsupported precisely because the engine cannot
rebuild itself from inside its own command handler. `starplayer-engine` therefore exports
only `MixerMode` — plain `no_std` data (path, interpolator, depth, dither, channels) with a
stable `u32` wire encoding — and a host holds an enum over the instantiations it is willing
to build. The web host's set is 2 paths × 2 interpolators × mono/stereo = 8 arms, built by
a macro. Depth and dither are **not** arms: they are a post-quantisation stage in the host,
applied with the mixer's own `HostSample` conversions and `Dither` after the output ring
and before the buffer the audio callback reads. That gives five depths on every arm without
forty instantiations, and leaves the fixed path's native `i16` untouched at `I16` depth, so
the golden bit-exact path stays bit-exact. Switching mode is a rebuild performed off the
render path, keeping the same `Arc<Module>` and the scan of it (§4.1 — a song timeline
depends on the output rate and the module's dialect, not on the mixer mode), seeking the
rebuilt sequencer to the song frame that was sounding and restarting its clock at the new
engine's frame.

SIMD (via `core::simd` behind a feature) is an optimisation *inside* the monomorphised
loop, never a semantic change. A scalar-equivalence test gates it.

### 7.2 DSP graph

Per-channel insert chains and a master bus, both operating on whole `RENDER_QUANTUM`
blocks (§1.4). Effects: reverb, chorus, delay, compressor, EQ.

#### IT's resonant filter is a *voice*-level filter, not an insert (M6-G2, landed)

It sits inside the render kernel, between the resampler and the pan gains — which is where
Impulse Tracker puts it and where OpenMPT's `SampleLoop` runs it
(`interpolate(); filter(); mix();`). Two voices of the same instrument on the same channel
filter independently, which an insert on a channel bus could not do, and which IT needs
because a New Note Action leaves the old voice sounding with a cutoff of its own.

**The law**, from OpenMPT's `CSoundFile::SetupChannelFilter` (`soundlib/Snd_flt.cpp`) on
the `kITFilterBehaviour` branch every IT module takes:

```text
frequency = 110 · 2^(0.25 + cutoff/24) Hz, clamped to [120, 20000] and then to sr/2
r         = sr / (2π · frequency)
damping   = 10^(−resonance · (24/128) / 20)
d         = damping·r + damping − 1;   e = r²
y[n] = x[n]/(1+d+e) + y[n−1]·(d+2e)/(1+d+e) − y[n−2]·e/(1+d+e)
```

With IT's extended filter range the exponent's divisor is 20 rather than 24 (a top cutoff
of 10670 Hz instead of 5124 Hz) and `d` comes from OpenMPT's other branch,
`d = (2·damping − min((1 − 2·damping)/r, 2)) · r`.

**Two tables, no transcendental** (§7.3). `2^(0.25 + cutoff/24)` is `2^(n/768)` at
`n = 192 + 32·cutoff`, an exact integer index into `LINEAR_FREQUENCY_TABLE`; the extended
range's index falls on a fifth of a table step, so it interpolates between two neighbouring
entries. `10^(−resonance·(24/128)/20)` is `IT_RESONANCE_TABLE_Q24`, Schism Tracker's
`resonance_table` transcribed as Q0.24 integers so the fixed path never touches a float.

**Two arms in the kernel.** `mix_run` carries a `const FILTERED: bool` alongside its
existing `RAMPING`/`REVERSE`, so the unfiltered body is textually the code that was there
before and every MOD, S3M and MTM golden is byte-identical across the change. The
coefficients are cached on the voice and recomputed only when `FilterParams` or the sample
rate moves — at tick rate in practice, never per frame — with no dirty bit, because
comparing the four bytes of `FilterParams` is cheaper than a bit test and cannot be missed
by an owner who forgets to raise one. `FilterParams::BYPASS` *is* IT's own "cutoff 127 with
resonance 0 is no filter at all", so a format without a filter pays one comparison per
voice per segment and nothing else.

**Fixed-point format**: coefficients in Q8.24 `i32` (OpenMPT's own
`MIXING_FILTER_PRECISION`), the delay line pre-amplified by 256 so that a quiet sample at a
low cutoff does not quantise to silence, and clamped to twice the input range before it is
fed back. See D70–D74 in the accuracy policy for where the fixed path's quantisation is
known to differ from OpenMPT's float.

**The reset rule**: a new note zeroes the delay line (`Voice::new`, `Voice::retrigger`); a
mid-note sample swap under tone portamento does not (D73). A muted voice's filter keeps
running, for the same reason its position does — the filter is a recursion, and a gap in it
would ring on unmute.

### 7.3 Cross-target determinism

x86 SSE2, ARM NEON and WASM SIMD agree on `+ - * /` (IEEE-754 round-to-nearest). They do
**not** agree on `sin`, `exp`, `powf` — different libm — and FMA contraction changes
results.

Therefore: **transcendental functions are banned from the RT path entirely.** Tables
only, which is what trackers do anyway — the original ships vibrato and tremolo tables.
FMA contraction is disabled. The **fixed-point i16 mixer is the canonical bit-exact
golden reference**; float goldens carry a tolerance. A CI job renders the same module on
x86-64, aarch64 and wasm32 and asserts identical hashes on the fixed path.

---

## 8. Real-time safety and the control plane

Hard rules for `render()`: no allocation, no locks, no panics, no `dyn` dispatch in the
inner sample loop (trait objects at block level are fine).

| Landmine | Guard |
|---|---|
| Dropping the last `Arc<Module>` on the audio thread calls `free()` | **Garbage channel**: the audio thread pushes retired `Arc`s back over an SPSC for the control thread to drop |
| A `Vec` growth in a "just for telemetry" path | Allocator hook in CI that fails on any allocation inside `render()`, run over the whole corpus (§8.2) |
| A stray `unwrap()` or slice index | `#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]` on `starplayer-engine` and `starplayer-mixer` |
| An internal inconsistency reaching the user as a crash | `render()` outputs silence rather than panicking |
| Unsafe creeping in | `#![forbid(unsafe_code)]` in the core crates |

Commands flow in over a single-producer/single-consumer ring. Modules are loaded on a
worker thread or task and handed in as `Arc<Module>`; the audio thread swaps the pointer
and returns the old `Arc` down the garbage channel.

### 8.1 The ring is a dependency, and why (M1-B3)

A wait-free SPSC ring **cannot be written in safe Rust**. Its whole point is that the
producer and the consumer hold disjoint mutable views of one buffer, disjoint by an
invariant expressed in two index atomics that the borrow checker cannot see, and `core`
offers no safe primitive granting interior mutability over an arbitrary `T` shared between
threads. Every core crate here is `#![forbid(unsafe_code)]`, so the choice was between
weakening that rule for the single most concurrency-sensitive file in the tree, or taking
a dependency whose unsafe is audited by more people than this project has.

`starplayer-rt` therefore wraps **`ringbuf`** (`no_std` + `alloc`, capacity fixed at
construction, wait-free `try_push` / `try_pop`) and re-exports its own `Producer` /
`Consumer` / `GarbageChannel` shapes, so the dependency stays replaceable. `ringbuf`
supports `portable-atomic`, which almost nothing else in this space does and which is what
makes it build for the bare-metal target at all.

Two policies fall out of "fixed capacity", and both are deliberate:

- **A full ring hands the value back** rather than dropping it. For `LoadModule` that means
  the control thread keeps the module it just spent milliseconds decoding.
- **A full garbage channel makes the audio thread drop the retired handle inline**, and
  raise `EngineWarnings::retired_module_dropped`. The alternative is an unbounded queue,
  which is an allocation on the audio thread. A breach means the host has stopped calling
  `collect_all_garbage`, so the warning is the useful signal.

### 8.2 How the two invariants are proved, not asserted (M2-C7)

Both rules in the table above were satisfied by construction from M0 and *checked by
inspection* until M2-C7. They now have automated enforcement, in two pieces.

**The allocator hook.** `crates/starplayer-offline/tests/render_allocation.rs` installs a
test-only `#[global_allocator]` that counts every allocation and deallocation made while a
thread-local "inside `render()`" flag is set, and drives every module the repository can
reach — the committed fuzz seeds, the owner's S3Ms, the synthesised MOD and MTM, and the
pinned libxmp corpus — through `render()` with it armed, at three host block sizes. It
records rather than panicking: a panic payload is boxed, so a panicking allocator re-enters
itself, and unwinding out of an allocation abandons the collection that asked for it. The
report therefore names the module that allocated, which an abort could not. A second test
arms the hook across a `LoadModule` swap, which is the moment the retired `Arc<Module>` goes
down the garbage channel. `cargo xtask ci --job rt-safety` is the gate, on the pinned
toolchain, and it runs the suite twice — once plain and once with `telemetry`, because the
per-tick publish is `#[cfg]`-gated and the default pass does not compile it into `render()`
at all.

The hook found one existing allocation, and it is the expected one: **`feature = "trace"`
allocates inside `render()`**, because the per-tick recorder appends a `TraceTick` per
tick. That is a diagnostic build by definition — `trace` is in no default feature set, the
host-tests job asserts the workspace never resolves it, and `--job trace-zero-cost` proves
the hook compiles to nothing without it — so the allocation test compiles to nothing under
`trace` rather than asserting a promise that build never made.

**The garbage channel across two threads.** `crates/starplayer-offline/tests/garbage_channel.rs`
puts an engine on its own thread calling `render()` in a loop, keeps the control handle on
the test thread, swaps modules mid-flight, and asserts the retired module's destructor ran
on the *control* thread's `ThreadId`. Removing the `retire` call makes it fail with the
audio thread's id.

**Loader fuzzing.** `fuzz/` holds six `cargo-fuzz` targets — a byte-level and a structured
one per format — with a dictionary of the format magic values, a per-input memory cap that
turns an OOM into a reproducible artifact rather than a killed process, and a seed corpus
built from the committed seeds plus the pinned conformance cache. `cargo fuzz` needs
nightly, so that job is the single CI job off the pinned toolchain; the seeds and every
crash regression are *also* replayed on the pinned toolchain inside `host-tests`, so the
regression suite never depends on nightly. One production consequence came out of this
work: the S3M loader now refuses a file that declares more decoded pattern data than its
own size can justify (`MINIMUM_PATTERN_BUDGET_BYTES`), because a 132 KB file may legally
declare 65,535 patterns, and surviving 670 MB of allocation is not the same as being right
about it.

---

## 9. Telemetry

A single triple-buffered snapshot does not work for oscilloscopes: 32 channels × 512 f32
is 64 KB per snapshot, ×3 buffers, copied at UI rate. Split it:

**(a) Coherent scalar state** — order, row, tick, pattern, speed, BPM, global volume;
per-channel note, instrument, volume, pan, active effect (~1 KB). Published through a
triple buffer or seqlock. The UI needs this internally consistent — a row number from
one tick with a note from another would render wrong.

**(b) Lossy audio taps** — scope waveform. Per-channel fixed ring; the audio thread
stores a `Relaxed` write index; the UI may read torn data. **Tearing is invisible on a
scope.** Downsample in the audio thread so it ships 32 values per quantum, not 4096. This
landed in M3-D6; §9.3 is what it landed as.

VU behaviour follows the original: peak-hold set on a new note or a volume-column write,
decayed by a fixed amount per tick (`__UpdateTracker` decays `_VUBarLevel` by 2/tick).
**The VU level stays in (a)**, contrary to the M1-B6 note that said it would move here at
M3: it is one scalar per channel, it already rides the snapshot at no cost, and every
consumer — the web player, the TUI, the wire header — reads it there. Moving it would
rewrite the 22-word wire header and every reader to buy nothing. Recorded as the D6
decision.

On WASM, (b) wants `SharedArrayBuffer`, which needs COOP/COEP headers — a `postMessage`
fallback path is planned from the start.

This is a **first-class API**, not a debug hook. It is what the web UI, the TUI and any
future tracker editor render from. The original's `ChannelData` deliberately carried
`_CMDVal`, `_CMDData`, `_VUBarLevel` and `_ActiveFlag` "for host program" — same idea.

### 9.1 What publishes (a), and why it is not a triple buffer (M1-B6)

A real triple buffer is not implementable under `#![forbid(unsafe_code)]`, for the same
reason §8.1 gives for the SPSC ring: its writer holds `&mut` to one slot while the reader
holds `&` to another, disjoint by an invariant living in an index atomic that the borrow
checker cannot see, so the slots must be `UnsafeCell`. The escape used for the ring — take
an audited dependency — has no equivalent here: `triple_buffer` is `std`-only (§11.2
already flags it), so it cannot serve the bare-metal target CI checks on every commit.
`ringbuf`'s own `push_overwrite` needs `&mut Rb`, not the producer half, and its docs say
so explicitly.

So (a) is published through **`starplayer_rt::snapshot`: a bounded SPSC channel of whole
snapshots**, three deep — the same three copies a triple buffer would have allocated. A
snapshot is moved into the ring in one piece and out in one piece, so it is atomic by
construction and no reader can observe a torn one. The writer never blocks: a publish that
finds the ring full is *dropped* and counted (`Snapshot::publishes_dropped`, plus a gap in
`Snapshot::sequence`), which a reader three ticks behind was never going to draw anyway.
It also needs strictly less than a triple buffer would — an SPSC ring only loads and stores
its two index atomics, so unlike `AtomicUsize::swap` there is no compare-and-swap anywhere
on this path and `riscv32imc` does not reach for `portable-atomic/critical-section` to
publish telemetry.

`Snapshot` carries a **fixed 64 channels**, not a const generic: 64 is IT's pattern channel
count and therefore the widest in scope, and a const generic would appear in the signature
of every function in every UI that touches a snapshot to save a couple of kilobytes on a
type that exists to be memcpy'd once per tick.

Cadence is **once per tracker tick**, published from the sequencer's dispatch after the
tick's outcome is committed. That is the `_MActual*` cadence the original used, and it is
the only rate at which every number in the snapshot is from the same moment. Per render
quantum would republish an unchanged snapshot six times a tick; per host block would make
the telemetry rate depend on the host's buffer size.

M3 revisits the primitive when the scope rings of (b) arrive.

### Q1 resolution (M0-A4)

**`SharedArrayBuffer` is the default; `postMessage` is a real fallback, not a second-class
one.** Both were built and exercised in
[M0-task-A4](../engine/complete/M0-task-A4-audioworklet-spike.md), and the page reports which is
live rather than assuming.

*What was tested.* A Rust sine oscillator rendered in wasm inside an AudioWorklet, with
`SetFrequency` travelling in over a lock-free SPSC ring and a peak level travelling back
out, under three configurations: cross-origin isolated with both paths on shared memory;
cross-origin isolated with the `postMessage` fallback forced on; and served **without**
COOP/COEP, where the page detects the loss and falls back on its own. Each ran 70 s of
continuous audio. In all three: the render quantum was 128 frames, matching
`RENDER_QUANTUM`; `memory.buffer.byteLength` was 1 703 936 bytes at start and unchanged
after 60 s; no commands were dropped; no console errors. Verified headlessly in
**Chromium 151** (a Playwright-cached build driven over the DevTools protocol), plus a
Node harness that runs the shipped worklet bundle against an `AudioWorkletGlobalScope`
stub and measures the rendered pitch by zero crossings. **Firefox and Safari were not
available on the build machine and have not been run; the owner's audible check is
against a real browser.**

*Why shared memory wins where it is available.* The two directions fail differently, and
neither failure is about throughput:

- **Commands (page → audio).** A `postMessage` per slider event is a structured clone and
  a task hop per event, so the fallback has to coalesce to at most one message per
  animation frame — which caps the control plane at frame rate and is wrong for anything
  finer-grained than a slider. The ring takes a write per event with no message at all,
  and the audio thread drains a bounded batch at the top of every render pass, which is
  the same `drain_commands()` shape §1.2 already specifies. The ring is where MIDI input
  and pattern jumps have to arrive in M1; the fallback is not a place they can live.
- **Telemetry (audio → page).** §9(b)'s scope data is the case that decides it. Peak
  levels are one float and survive either transport, but a `postMessage` carrying scope
  waveforms means **allocating on the audio thread**, every post, forever — the exact
  thing §8 forbids. Shared memory makes the audio thread's side a relaxed store into a
  block it already owns. So the split in §9 stands: scalar state can ride `postMessage`
  if it must, scope data cannot.

*The cost of requiring it.* COOP `same-origin` + COEP `require-corp` are a deployment
constraint, not a code one, and they are contagious: every cross-origin subresource an
embedder loads then needs CORP or CORS. That is why the fallback exists and why it is
kept working — a host that cannot set those headers still gets audio, a working slider
and a working VU meter, and loses only the scope's fidelity. **Neither path may be
allowed to rot: both are exercised on every check of the web player.**

*Safari (research point 4).* Not tested — no Safari on the build machine. From the
specifications: Safari 15.2+ implements COOP/COEP and gates `SharedArrayBuffer` on
cross-origin isolation the same way, so the same code path applies, and the fallback
covers it if it does not. Two Safari-specific hazards are worth carrying into M1-B7
rather than assuming away: Safari has historically been the strictest about no `fetch`
and no dynamic `import` inside worklet scope — which the single concatenated bundle and
the pre-compiled `WebAssembly.Module` handed over in `processorOptions` already avoid —
and it is the most likely to need a real user gesture on the `AudioContext`, which the
Start button provides. Confirm on hardware before M1-B7 ships.

### 9.2 The wire protocol the web player settled on (M1-B7)

A4 carried one command (`SetFrequency`) and one scalar back. B7 kept both transports and
both shapes and widened them, without changing the decisions above.

**Commands** are fixed three-word records — opcode, argument, extra — in the same SPSC
ring, covering play, stop, seek order, seek row, master volume, channel mute,
`SET_MIXER_MODE` (opcode 7, argument = `MixerMode::to_wire()`), `SEEK_FRAME` (opcode 8,
argument = a song frame) and `AT_END` (opcode 9, argument `0` fade out / `1` continue /
`2` stop, `extra` = the fade length in frames). The worklet decodes each
record into a `starplayer_core::Command` and stages it in a fixed-capacity queue drained
immediately before `Engine::render`, so the JavaScript edge never touches the engine's own
control ring. On the fallback path the page batches every control change made during one
animation frame into one message, so "no `postMessage` per interaction" holds on both
paths. `SET_MIXER_MODE` is the one opcode that is *not* staged for the render path:
rebuilding a typed engine allocates (§7.1), so whichever drain sees it only retains the
scalar, and the rebuild runs in a worklet message task — the page follows a ring push with
a `flushCommands` message to provide one.

**Snapshots** cross as a flat `Int32Array`: a 22-word header (sequence, dropped publishes,
channel count, voices, order, pattern, row, tick, speed, BPM, global volume, warning bits,
engine frame, pending garbage, module generation, playing, master peak, retired modules,
**active mixer mode**, song frame, song length in frames, song flags) then eight words per
channel for all 64. The last three arrived with the song timeline (§4.1): the song flags
are bit 0 length known, bit 1 ends by looping (clear for a song whose order list merely
runs out), bit 2 end reached, bit 3 transport fading. The mode word is what the
host actually built rather than what was requested, so the page can show the difference. The seqlock is the page's; the worklet copies the
whole block between an odd and an even sequence store. What does **not** cross is
`EffectDisplay::name` — a `&'static str` has no meaning in another address space, let alone
another realm. The **whole English name table crosses once**, at start-up, from the
page-side wasm instance, and the page resolves `(code, param)` against it. The names are
still `EffectNames`'s, which is the point: no second copy of the table in JavaScript.

**One realm hazard worth recording.** `AudioWorkletGlobalScope` has no `TextDecoder`, and
wasm-bindgen's glue constructs one unconditionally at the top of its IIFE. The bundle
therefore dies before `registerProcessor` runs, and the page learns about it only later,
as "the node name is not defined". `cargo xtask wasm` now concatenates a small UTF-8
decoder ahead of the glue and fails the build if the glue ever starts wanting to encode as
well. The Node worklet harness hides Node's own `TextDecoder` so it sees this too.

The no-modules glue has a second worklet-specific trap: its IIFE caches one WASM instance
for the entire realm. Output-channel rebuilds deliberately overlap the live and candidate
`AudioWorkletNode` in the same realm, so sharing that cache would make candidate `init`
replace the live Rust `HOST` and potentially grow its memory under the live callback.
Packaging therefore wraps the generated IIFE in a factory. Every processor owns an
independent WASM instance, Rust host, and memory, while all processors reuse the compiled
`WebAssembly.Module`. The Node harness constructs two processors simultaneously and
checks both memory identity and independent module generations.

### 9.3 What (b) landed as (M3-D6)

**The tap does not read the mix.** There are no per-channel buses: `VoicePool::accumulate_masked`
sums every voice into one accumulator in slot order, and that order is what makes the float
path block-size independent and what the golden hashes fingerprint. Accumulating per
channel to get a scope signal would change it. So the tap samples **voice state** and
never touches the accumulator, which is why `cargo xtask goldens --check` is byte-identical
with the taps compiled in.

**The sampling rule.** `Engine::render_quantum` already splits each 128-frame quantum into
segments at event boundaries. Immediately *before* each segment's accumulation, the engine
walks every sounding voice; for each tap bucket whose first frame `4·b` (for `b` in `0..32`)
lies inside that segment, it reads one PCM frame at the voice's position advanced by
`4·b − offset` steps — folded through the voice's region by the render kernel's own
`normalise_position`, exposed as `starplayer_mixer::folded_frame` so there is one loop rule
and not two — scales it by the voice's `params.volume`, and **sums it, saturating**, into
the ring of channel `voice.tag.channel`. A bucket's first frame lies in exactly one segment,
so each bucket is written exactly once per quantum and its value is a pure function of the
quantum and the engine state, never of the host's block size. `tests/block_size_determinism.rs`
asserts that at block sizes 1, 3, 64, 128, 4096 and 8191, against independently computed
expected values.

The tap deliberately ignores interpolation, the gain ramps, pan, the master bus and (from
M6) the per-voice filter — §9(b) tolerates exactly that; it is a picture, not the audio. A
**muted** channel is still tapped, because muting is a mixer-side discard and a per-channel
scope exists precisely to show a channel's own signal. A voice whose `tag.channel` is past
the ring count is skipped: the rings are sized once, at engine construction.

**The primitive.** `starplayer_rt::tap`: `TAP_BUCKET_FRAMES = 4` (so 32 buckets per
quantum), `TAP_RING_BUCKETS = 1024` (~93 ms at 44.1 kHz, a power of two), one
`Arc<[AtomicI16]>` plus one `Arc<AtomicU32>` write index per channel, allocated in
`TapRing::new` and never again. `Relaxed` everywhere, no compare-and-swap — so `riscv32imc`
does not reach for `portable-atomic/critical-section` to draw a scope — and no seqlock:
a reader may see a torn window, and that is the design. `Engine::scope_readers` hands the
reader halves out once, like `Engine::telemetry_reader`. 64 channels cost 128 KiB, once.

**The transports.** Unlike the snapshot, the scope block is its **own** `SharedArrayBuffer`,
so the 22-word snapshot header and its seqlock are untouched. The wasm host keeps a flat
`[64][256]` `i16` block plus one write-index word per channel, refreshed from the readers
every four quanta (~94 Hz at 48 kHz) for the *active* channels only, and exports it as
`scope_ptr` / `scope_len` / `scope_window_buckets` / `scope_bucket_frames` /
`scope_index_ptr` / `scope_generation` / `scope_channels`. The worklet views that block and
publishes it once per refresh rather than once per quantum. On the `postMessage` fallback
the same window rides the batched telemetry message the snapshot already goes in, trimmed
to the active channels — one copy per posted message, which is the copy the fallback path
already accepts. The page reads the shared window **in place**, with no copy at animation
frame rate, and reports which transport is live in the Engine panel.

---

## 10. Portability

- Core crates are `#![no_std]` + `alloc`. CI checks `riscv32imc-unknown-none-elf` on
  every commit. **Hard rule: no default feature transitively enables `std`.**
- `portable-atomic` + `critical-section` for targets lacking CAS — `alloc::sync::Arc`
  hits this on thumbv6m, Xtensa and the CI target `riscv32imc-unknown-none-elf`, which
  implements neither the `A` extension nor any of `core::sync::atomic`.

  **The arrangement, settled in M1-B3.** `starplayer-rt` re-exports
  `portable_atomic_util::Arc` as `starplayer_rt::Arc`, and every other crate names *that*
  one; writing `alloc::sync::Arc` in a `no_std` crate is a portability bug only the
  bare-metal CI job would catch. Plain atomic load/store — all an SPSC ring needs — are
  native on those targets with no features at all; only `Arc`'s refcount needs CAS. So
  `critical-section` is enabled by a **target condition** in the manifest,

  ```toml
  [target.'cfg(not(target_has_atomic = "ptr"))'.dependencies]
  portable-atomic = { workspace = true, features = ["critical-section"] }
  ```

  rather than by a cargo feature. A feature would have to be on by default for the
  bare-metal job to pass, and would then be on for every desktop and browser build that
  has real atomics. Embedded users of those targets provide a `critical-section`
  implementation, which is the standard arrangement there.
- Loader IO is a minimal `trait ModuleReader { read, seek, len }` with a zero-copy
  `&[u8]` fast path. Loading is **synchronous** over an already-obtained byte source;
  *acquiring* the bytes is async and lives in the platform crates, so the core never
  needs an async runtime. WASM (fetch → `Uint8Array`), native (mmap or `Vec<u8>`) and
  embedded (flash slice) all satisfy it.
- Sample data may be **borrowed** rather than owned, so an embedded target can play a
  module straight out of memory-mapped flash. This is what §6's offsets-not-references
  decision buys.

### 10.1 Trait discipline

One rule to prevent the failure mode where this project reaches 80% design and 0% audio:

> **No trait is committed until its second real implementation exists.**

Build S3M concretely with structs; extract `Instrument` when XM lands. The *concepts* in
this document are designed up front; the Rust trait boundaries follow the
implementations. The two exceptions, which have multiple users from day one, are
`TempoModel` (§1.3) and `EventSource` (§3, pattern sequencer + external queue).

The rule bit on its own author at M4. XM and IT are built **concurrently**, so "extract
`Instrument` when XM lands" would have meant extracting it from one implementation and a
guess about the other. The owner's decision of 2026-09-03 is therefore to keep per-voice
articulation format-owned through M5 and M6 and to extract the trait at M4-full, against
XM, IT *and* the MIDI sample player — see §5.3.

---

## 11. Crate layout

```
crates/
  # ── no_std + alloc ──────────────────────────────────────────────────────────
  starplayer-core       fixed-point (Q16.16 / Q32.32), Frame / Step / Note,
                        Event / TimedEvent, VoiceParams + DirtyBits, TempoModel,
                        RowClock, period & waveform tables, Error. No IO, no side effects.
  starplayer-rt         SPSC ring, triple buffer, seqlock, lossy tap ring,
                        portable-atomic shim.                    → core
  starplayer-dsp        interpolators, ramping, IT resonant filter, biquad,
                        reverb / chorus / delay / compressor,
                        SIMD backends (scalar / sse2 / neon / simd128).  → core
  starplayer-mixer      VoicePool, voice render kernels, buses,
                        output formats i8/i16/i24/i32/f32, mono & stereo. → core, dsp
  starplayer-model      shared Module (blob + u32 offsets), Sample, Envelope,
                        InstrumentDef, display-only PatternCell.  → core
  starplayer-engine     render loop, EventSource, Instrument, channel binding,
                        RENDER_QUANTUM adapter, command queue,
                        telemetry publisher.        → core, rt, dsp, mixer, model,
                                                      telemetry (feature `telemetry`)
  starplayer-mod        MOD loader + ProTracker effect processor  ┐
  starplayer-s3m        S3M loader + ST3 effect processor         │ each → engine, model
  starplayer-mtm        MTM loader + effect processor             │
  starplayer-xm         XM  loader + effect processor             │
  starplayer-it         IT  loader + effect processor + NNA policy┘
  starplayer-midi       MIDI byte codec + SMF parser + Event mapping  → core;
                        the `smf` feature (parser + SmfSequencer) also → engine, for
                        EventFeed and the MIDI_CHANNEL_BASE mapping (task E5)
  starplayer-telemetry  snapshot types shared by every UI             → core, rt
  starplayer            facade: re-exports + format autodetect. THE public crate.
  # ── std ─────────────────────────────────────────────────────────────────────
  starplayer-host       AudioBackend trait, AudioSpec/DeviceInfo/Stream, the
                        backend-neutral Player: engine + transport + seek
                        mailbox + output depth.                   → starplayer
  starplayer-host-cpal  native audio output          → starplayer, starplayer-host, cpal
  starplayer-midi-native  native MIDI *input*: a midir port decoded onto a Player's
                        live-input queue (M4-E6). Not part of the cpal crate: it is not
                        an audio backend, and a host on any other one still wants a
                        keyboard.               → starplayer, starplayer-host, midir
  starplayer-host-wasm  AudioWorklet glue  → starplayer, starplayer-host, wasm-bindgen
  starplayer-archive    zip (later: lha/rar?) container support   → model, zip
  starplayer-offline    WAV writer, deterministic render, trace dump
  starplayer-testkit    golden compare, libxmp/openmpt diff harness, trace differ
apps/
  starplayer-web        wasm-bindgen + responsive web UI
  starplayer-cli        CLI player / renderer
  starplayer-tui        STAR.EXE homage (ratatui)
xtask/                  build orchestration, wasm packaging, golden regeneration
```

The `starplayer-engine` → `starplayer-telemetry` edge is **optional**, behind the engine's
`telemetry` feature, and it runs that way round deliberately (M1-B6). Only the engine can
fill a snapshot in coherently — transport position, channel table and voice pool all have
to be read at one instant — so the engine owns the publisher and the format crates decorate
it through `TickContext::report_effect` / `report_note`, which compile to nothing when the
feature is off. The alternative, a `TelemetrySink` trait declared in the engine and
implemented in `starplayer-telemetry`, would put a `dyn` call on the tick path and split
the snapshot types across two crates for every UI to reassemble.

Note what `starplayer-telemetry` therefore still cannot see: `starplayer-model`, and so the
`EffectNames` table of English effect names that lives there beside the display-only
`PatternCell`. It is not duplicated. `EffectDisplay` carries the raw code, the parameter and
a `&'static str`, and the format crate — the only code that knew what the bytes meant —
resolves the name.

**Loader and effect processor live in the same crate per format**, because file
semantics and effect semantics are inseparable — one crate, one feature flag, one
dependency edge. This is the concrete lesson from the original: it converted MOD and MTM
to S3M *before* the player saw them, and that is why its MOD playback was inaccurate.

`starplayer-host` is the one crate a *host* may depend on besides the facade, and it is
still a host: it holds what is true of every backend — what a device is, what a stream is,
and the `Player` that turns a file into sound through one — so that adding a backend is
"implement `AudioBackend`" rather than "reimplement the transport" (task D4). **Both**
hosts are behind it: `starplayer-host-cpal` and `starplayer-host-wasm`'s `WorkletBackend`
implement the same `AudioBackend`, and both drive the same `Player` (task D9). `ManualBackend`
is the third, and lives in `starplayer-host` itself so the invariants are testable with no
device.

One trait covers a *pulling* backend and a *pushed* one because `AudioBackend::open` never
promised to start a thread — it takes ownership of a callback and promises to call it, and
which clock does the calling is the backend's own business. cpal's clock is a thread it
owns; the worklet's is the browser calling `process()` with 128 frames; `ManualBackend`'s is
the caller. What the worklet does need is exactly what `negotiate` is for: an `AudioContext`
is constructed at a sample rate and cannot be renegotiated, so it answers with the context's
rate whatever was asked for, and the engine and every module scan are built from that answer
(§4.1). A second push-shaped trait was considered and rejected in D9: it would have bought
one thing — a host would not have to keep the backend alive alongside its `Player` — and
cost two lifecycles per host and a duplicated `negotiate`.

What stays in each backend crate is what is genuinely platform. cpal keeps device
enumeration and its `i16` conversion; the wasm host keeps the wire command decoding, the
`SharedArrayBuffer` and `postMessage` transports, the scope-window copy, the planar output
buffer and the heap pre-reservation. Neither keeps a transport, an engine-arm enum, a seek
mailbox or an output-depth post-stage.

Dependency edges are strictly one-directional. Apps depend only on the facade.
Extracting `starplayer` for crates.io later is a manifest change, not a refactor.

### 11.1 Feature flags

```toml
[features]
default = ["s3m", "mod", "mtm", "float-mix", "linear-interp"]
std = []                      # never in default
alloc = []                    # always on for now
simd = []
float-mix = []                # both mix paths may be enabled at once
fixed-mix = []
telemetry = []
trace = []                    # per-tick state trace; test/debug only
quirks-starplayer = []        # see plans/product/03-accuracy-policy.md §2
serde = []
mod = []; s3m = []; mtm = []; xm = []; it = []
midi = []; smf = []
```

### 11.2 Candidate dependencies

`fixed`, `bytemuck`, `heapless`, `bitflags`, `portable-atomic`, `critical-section`;
`rtrb` / `triple_buffer` (std hosts only — and therefore **rejected** for the telemetry
snapshot, see §9.1); `midly` (no_std-capable SMF); `cpal`,
`midir`, `wasm-bindgen`, `ratatui`; `clack` / `nih-plug` later.
Test-only: `libopenmpt` / `libxmp` bindings, `cargo-fuzz`, `assert_no_alloc`.

---

## 12. Open questions

Recorded rather than guessed. Each has a milestone where it must be settled.

| # | Question | Settle by |
|---|---|---|
| Q1 | Does `SharedArrayBuffer` + COOP/COEP work well enough for scope telemetry, or is `postMessage` the practical default? | **Settled in M0-A4 — see §9** |
| Q2 | Is 128 frames the right `RENDER_QUANTUM` for embedded, or does the ESP32 path want a compile-time override? | M8 |
| Q3 | Which voice-stealing heuristic does libopenmpt actually use, exactly? | **Settled in M6-G3 — see §5.2** |
| Q4 | Does the `Instrument` trait survive contact with a non-sample instrument (FM), or does it need a second tier? | M10 — **still open**; M4-E4 committed the trait with two *sample* implementations (§5.3), which is what design goal 8 asked for and is not yet the question Q4 asks |
| Q5 | CLAP first with a VST3 wrapper, or nih-plug for both? | M9 |
