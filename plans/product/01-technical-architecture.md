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
pub struct TimedEvent { pub frame: u64, pub target: Target, pub event: Event }

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
| `SmfSequencer` | a sorted MIDI-file event list |
| `ExternalEventQueue` | live MIDI, keyboard, plugin host events |
| `SourceMux` | merges several sources with the §3.1.3 tie-break — "play a module and jam over it" |

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
part of the output. It is a policy trait with real time budgeted against it, matched to
libopenmpt, not guessed.

### 5.3 The instrument surface

Sketched, not committed — extracted at M4 once XM gives it a second real implementation
(§10):

```rust
pub trait Instrument {
    fn note_on(&self, ch: &mut Channel, pool: &mut VoicePool, note: NoteParams);
    fn note_off(&self, ch: &mut Channel, pool: &mut VoicePool);
    fn control_tick(&self, ch: &mut Channel, pool: &mut VoicePool);  // envelopes, NNA, fadeout
    fn render(&self, voice: &mut Voice, out: &mut MixBuffer);
}
```

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

---

## 6. Module memory layout

Decide this now, not later:

```rust
pub struct Module {
    blob: Box<[u8]>,               // decoded pattern data and everything non-sample
    pcm: Box<[i16]>,               // all samples decoded + delta-decoded, concatenated,
                                   //   each with N guard frames appended (loop-wrapped
                                   //   or zeroed) so interpolators need no branches
    samples: Box<[SampleIndex]>,   // { pcm_offset: u32, len: u32, loop_start, loop_end, .. }
    patterns: Box<[PatternIndex]>, // { blob_offset: u32, rows: u16, channels: u8 }
    orders: Box<[u16]>,
    instruments: Box<[InstrumentDef]>,
    header: ModuleHeader,
}
```

**Offsets, not references.** Consequences, all of which we need:

- trivially `Send + Sync`, so `Arc<Module>` hands to the audio thread with no ceremony;
- trivially hashable, so golden tests can fingerprint a loaded module;
- trivially fuzzable — a loader either produces a valid index set or an `Err`;
- mmap- and flash-friendly on ESP32, where sample data may be borrowed rather than owned;
- no pointer chasing in the mixer inner loop.

**Guard frames** are the other half: appending N frames to each sample's PCM (loop-
wrapped for looping samples, zeroed otherwise) lets the interpolator read past the loop
point without a branch in the inner loop.

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

SIMD (via `core::simd` behind a feature) is an optimisation *inside* the monomorphised
loop, never a semantic change. A scalar-equivalence test gates it.

### 7.2 DSP graph

Per-channel insert chains and a master bus, both operating on whole `RENDER_QUANTUM`
blocks (§1.4). Effects: reverb, chorus, delay, compressor, EQ, plus IT's resonant
filter, which is a *voice*-level filter rather than an insert.

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
| A `Vec` growth in a "just for telemetry" path | Allocator hook in CI that panics on any allocation inside `render()`, run over the whole corpus |
| A stray `unwrap()` or slice index | `#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]` on `starplayer-engine` and `starplayer-mixer` |
| An internal inconsistency reaching the user as a crash | `render()` outputs silence rather than panicking |
| Unsafe creeping in | `#![forbid(unsafe_code)]` in the core crates |

Commands flow in over a single-producer/single-consumer ring. Modules are loaded on a
worker thread or task and handed in as `Arc<Module>`; the audio thread swaps the pointer
and returns the old `Arc` down the garbage channel.

---

## 9. Telemetry

A single triple-buffered snapshot does not work for oscilloscopes: 32 channels × 512 f32
is 64 KB per snapshot, ×3 buffers, copied at UI rate. Split it:

**(a) Coherent scalar state** — order, row, tick, pattern, speed, BPM, global volume;
per-channel note, instrument, volume, pan, active effect (~1 KB). Published through a
triple buffer or seqlock. The UI needs this internally consistent — a row number from
one tick with a note from another would render wrong.

**(b) Lossy audio taps** — scope waveform and VU peaks. Per-channel fixed ring; the
audio thread stores a `Relaxed` write index; the UI may read torn data. **Tearing is
invisible on a scope.** Downsample in the audio thread (peak-per-16-frames) so it ships
32 frames per quantum, not 4096.

VU behaviour follows the original: peak-hold set on a new note or a volume-column write,
decayed by a fixed amount per tick (`__UpdateTracker` decays `_VUBarLevel` by 2/tick).

On WASM, (b) wants `SharedArrayBuffer`, which needs COOP/COEP headers — a `postMessage`
fallback path is planned from the start.

This is a **first-class API**, not a debug hook. It is what the web UI, the TUI and any
future tracker editor render from. The original's `ChannelData` deliberately carried
`_CMDVal`, `_CMDData`, `_VUBarLevel` and `_ActiveFlag` "for host program" — same idea.

---

## 10. Portability

- Core crates are `#![no_std]` + `alloc`. CI checks `riscv32imc-unknown-none-elf` on
  every commit. **Hard rule: no default feature transitively enables `std`.**
- `portable-atomic` + `critical-section` for targets lacking CAS — `alloc::sync::Arc`
  hits this on thumbv6m and Xtensa.
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
                        telemetry publisher.        → core, rt, dsp, mixer, model
  starplayer-mod        MOD loader + ProTracker effect processor  ┐
  starplayer-s3m        S3M loader + ST3 effect processor         │ each → engine, model
  starplayer-mtm        MTM loader + effect processor             │
  starplayer-xm         XM  loader + effect processor             │
  starplayer-it         IT  loader + effect processor + NNA policy┘
  starplayer-midi       MIDI byte codec + SMF parser + Event mapping  → core
  starplayer-telemetry  snapshot types shared by every UI             → core, rt
  starplayer            facade: re-exports + format autodetect. THE public crate.
  # ── std ─────────────────────────────────────────────────────────────────────
  starplayer-host-cpal  native audio output                       → starplayer, cpal
  starplayer-host-wasm  AudioWorklet glue                → starplayer, wasm-bindgen
  starplayer-offline    WAV writer, deterministic render, trace dump
  starplayer-testkit    golden compare, libxmp/openmpt diff harness, trace differ
apps/
  starplayer-web        wasm-bindgen + responsive web UI
  starplayer-cli        CLI player / renderer
  starplayer-tui        STAR.EXE homage (ratatui)
xtask/                  build orchestration, wasm packaging, golden regeneration
```

**Loader and effect processor live in the same crate per format**, because file
semantics and effect semantics are inseparable — one crate, one feature flag, one
dependency edge. This is the concrete lesson from the original: it converted MOD and MTM
to S3M *before* the player saw them, and that is why its MOD playback was inaccurate.

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
`rtrb` / `triple_buffer` (std hosts only); `midly` (no_std-capable SMF); `cpal`,
`midir`, `wasm-bindgen`, `ratatui`; `clack` / `nih-plug` later.
Test-only: `libopenmpt` / `libxmp` bindings, `cargo-fuzz`, `assert_no_alloc`.

---

## 12. Open questions

Recorded rather than guessed. Each has a milestone where it must be settled.

| # | Question | Settle by |
|---|---|---|
| Q1 | Does `SharedArrayBuffer` + COOP/COEP work well enough for scope telemetry, or is `postMessage` the practical default? | M0 |
| Q2 | Is 128 frames the right `RENDER_QUANTUM` for embedded, or does the ESP32 path want a compile-time override? | M8 |
| Q3 | Which voice-stealing heuristic does libopenmpt actually use, exactly? | M6 |
| Q4 | Does the `Instrument` trait survive contact with a non-sample instrument (FM), or does it need a second tier? | M10 |
| Q5 | CLAP first with a VST3 wrapper, or nih-plug for both? | M9 |
