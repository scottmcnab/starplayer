# StarPlayer revival — scoping & milestone overview

> The approved scoping plan, 2026-08-28. This is a **historical record** of the scoping
> exercise and the reasoning behind it. The live index is [`README.md`](README.md), and
> the foundation documents in [`product/`](product/) are the source of truth — where this
> document and one of those disagree, the foundation document wins.

## Context

`STARPLAY/` holds the original 1990s DOS StarPlayer sources: `S3MLIB.ASM` (6,317 lines
— the reusable S3M/MOD/MTM replay engine plus GUS and SoundBlaster drivers),
`STAR.ASM` (1,421 lines — the front-end), `CDROMLIB.ASM`, built with TASM + Watcom
`wlink` against Tran's PMODE/W. It was known for unusually accurate S3M interpretation
and click-free GUS output via hardware volume ramping with a channel-count-adaptive
mixing rate.

**Important finding from the source survey:** the released sources are *deliberately
gutted*. Almost the whole engine and front-end sit inside TASM `comment %` blocks
(`S3MLIB.ASM` 949–2267, 2370–3832, 3838–4155, 4234–6314; `STAR.ASM` keeps only 16
support procs), so the shipped `STAR.EXE` is itself a crippled build. The **text is
completely intact and readable**, however — the entire tracker, both hardware drivers,
all three loaders, and every UI screen table survived. The sources are therefore fully
usable as a specification; they are just not currently buildable as-is.

The goal is not to port the assembly. It is to build a **new, reusable Rust
tracked-music engine** that (a) captures the original's per-tick effect semantics
where they are right, (b) generalises well beyond tracker patterns into a real-time
event-driven synthesis engine (MIDI in, SMF playback, FM/wavetable/SID/SoundFont,
VST/CLAP), and (c) runs everywhere — WASM first, then native desktop, then `no_std`
embedded (esp-rs + embassy).

### Owner decisions taken during planning

| Decision | Choice |
|---|---|
| First audible deliverable | **WASM AudioWorklet web player** |
| Fidelity target | **Semantic fidelity** to per-tick effect/period/volume behaviour; modern high-quality mixing. No retro-mixer emulation. |
| Fidelity *reference* | **Canonical ST3/ProTracker behaviour, deviations documented.** The ASM is the primary spec, but where it deviates through an outright defect, implement the canonical behaviour and record it. |
| MOD/MTM handling | **Native per-format effect processors.** Do *not* replicate the original's MOD/MTM → S3M in-memory conversion; do replicate its conversion tables as MOD/MTM *semantics*. |
| DOS reference capture | **Deferred.** Read the assembly as the spec; pull in a DOSBox reconstruction only if the port hits an ambiguity the source can't settle. |
| Portability | **`no_std` + `alloc` from day one**, CI-enforced on a bare-metal target. |
| Licence | **Deferred.** Repo stays private; no licence files or SPDX headers yet. |

Defects in the original that the "canonical" decision explicitly resolves (each gets a
line in the accuracy policy): `Vib_Pulse_Table` is only 62 entries so phases 62/63 read
into the adjacent random table; arpeggio applies a single octave carry so large nibbles
index past `Period_Table` into `Volume_Table`; the `Ixy` tremor per-tick handler reads
the channel through `edi` while the loop passes it in `esi`; the SB `PostTable` index is
sign-extended and used unbounded.

---

## Part 1 — Architecture

### 1.1 The central problem: keeping events sample-exact

MOD/S3M effects (per-tick portamento, vibrato, volume slides, retrigger, tremor,
note-cut/delay) only sound right if each tracker tick is applied at an exact output
sample. Applying ticks at audio-buffer granularity is audibly wrong and
non-deterministic across buffer sizes.

**The original already solved this and we should copy it.** `SB_IRQ_Handler` fills the
DMA buffer in slices bounded by the tick gap:

```
@@fine:  if _SB_GapCount == 0 { __UpdateTracker(); SB_ProcessTracks(); _SB_GapCount = _SB_GapLength }
         ecx = min(_SB_GapCount, _SB_BufCount); Mixer_8bitMono(ecx); advance; loop
```

with `SetSBTempo` computing `_SB_GapLength = (mixing_rate * 10 / bpm) >> 2`. That is
"split the output block at tick boundaries", in 1996.

Two refinements over the original:

- **Drift.** `(rate*10/bpm)>>2` truncates twice — at 44100/130 BPM it yields 848 where
  the true value is 848.077, roughly 1.3 s of drift over a four-minute song. Tick
  length becomes a **policy**, not a constant, so "drift-free" and "bug-compatible"
  can coexist:

  ```rust
  pub trait TempoModel {
      /// Frames per tick, Q32.32.
      fn frames_per_tick(&self, sample_rate_hz: u32, tempo_bpm: u16, speed: u8) -> u64;
  }
  pub struct ExactFixedPoint;   // default: rate * 2.5 / bpm in Q32.32, accumulated
  pub struct St3Truncating;     // reproduces (rate*10/bpm)>>2
  pub struct ItModern;          // openmpt semantics incl. tempo slides
  ```

- **DSP block granularity.** This is the highest-probability silent failure in the
  whole design. If per-channel reverb, the compressor and the SIMD kernels consume
  whatever ragged 3/17/411-frame segment the event split hands them, output depends on
  the host buffer size — offline will not match real-time, and neither will match
  across hosts. The rule:

  > **Split for voice mixing; quantise for DSP.** The engine renders on an internal
  > fixed `RENDER_QUANTUM = 128` frames (exactly AudioWorklet's quantum — free on the
  > first target). Events split *within* a quantum for voice accumulation; the DSP
  > graph and master bus only ever see whole quanta. Arbitrary host block sizes are
  > adapted by a small output ring.

  **Day-one test:** render the same module at host block sizes 1, 3, 64, 128, 4096 and
  8191 and assert byte-identical output. With that test present from M0, this bug class
  can never land.

### 1.2 Three kinds of state, not two layers of events

The owner's instinct — a MIDI-like timed-event core fed by format-specific pattern
interpreters — is right in its *semantics* but wrong if taken literally as one event
queue. Three genuinely different things get conflated:

| | shape | rate | needs a timestamp? | crosses a thread? |
|---|---|---|---|---|
| **Timeline events** — NoteOn, CC, SMF, live MIDI | sparse, discrete | low | yes | yes |
| **Voice parameters** — pitch, volume, pan, filter | dense, continuous | every tick, every voice | no — the instant is already known | no |
| **Control commands** — load module, seek, master volume | very sparse | ~0 | no | yes |

At 32 channels × 50 ticks/s with vibrato + tremolo + volume envelope + auto-vibrato +
filter envelope, the middle row is 5,000–10,000 "events" per second that are just field
writes to a struct you already hold a pointer to. Encoding, queueing and matching those
is strictly worse than what the 80386 code did: **write field, set dirty bit.**

So: **voice parameters are a struct with dirty bits, mutated in place — not events.**

```rust
// Timeline. The only thing carrying a timestamp.
pub struct TimedEvent { pub frame: u64, pub target: Target, pub event: Event }
pub enum Target { Channel(u16), Voice(VoiceId), Global }
pub enum Event {
    NoteOn { note: Note, velocity: U0F16 },   // NOT u7 — see below
    NoteOff { note: Note, velocity: U0F16 },
    KeyOff, FadeOut, Cut,
    Aftertouch { .. }, PitchBend(I1F15), Controller { .. }, Program(InstrumentId),
    Trigger(TriggerSpec),        // tracker-native note-on: sample, offset, flags
    Param(VoiceParam),           // absolute voice-level set — the escape hatch
    Tempo { .. }, GlobalVolume(U0F16),
}

// Voice state. Written directly, never queued.
pub struct VoiceParams {
    pub step: Step,              // Q32.32 sample-position increment per output frame
    pub volume: U0F16,
    pub pan: I1F15,
    pub filter: FilterParams,
    pub dirty: DirtyBits,        // the modern _CHN_NewVol / _NewPitch / _NewPan / _NewSamp
}

// Control plane. SPSC, its own enum.
pub enum Command { LoadModule(Arc<Module>), Play, Stop, SeekOrder(u16), SetMasterVolume(U0F16), .. }
```

`DirtyBits` is a direct descendant of the original's `_CHN_New*` flags in
`S3MLIB.INC:55-65` — the design is 30 years old and still correct.

Three supporting points:

- **Testability was the real reason to want an event stream.** Get it for free with a
  `#[cfg(feature = "trace")]` hook on every parameter write: zero cost in release, a
  full serialisable per-tick trace in test builds. Better than paying for a queue in
  the RT path forever.
- **Layer 1 is MIDI-*convertible*, not MIDI-*shaped*.** Seven-bit velocity cannot
  round-trip S3M volume (0–64), IT global volume (0–128), MOD/S3M pan (0–15 / 0–255)
  or IT pan (0–64 plus surround). Use `U0F16` velocity and
  `Note { semitone: u8, cents: i16 }` — microtonality comes free, and FM/SID/physical
  modelling will want it. MIDI byte parsing lives in `starplayer-midi` as a *codec*
  that converts into `Event`.
- **Layer 1 and the tracker path are peers, not a pipeline.** MOD/S3M/MTM patterns
  never produce a NoteOn; they produce "trigger sample N at period P, volume V, offset
  O". Only XM/IT instrument mode has a note→sample-map indirection resembling a MIDI
  program. Drawing L1 → lowering → L2 would build a MIDI-shaped bottleneck that costs
  accuracy on every format. The correct picture:

```
PatternSequencer ──Trigger + direct VoiceParams writes──┐
SmfSequencer / LiveMIDI ──NoteOn/CC──> Instrument ──────┼──> VoicePool ──> Mixer ──> DSP
KeyboardInput ──NoteOn──────────────────────────────────┘
```

- **`Step`, not `Hz`.** The mixer wants a resample increment. The original computes it
  explicitly (`PeriodToPitch` → divide by `__MixingRate` → `_Mix_HighSpeed` /
  `_Mix_LowSpeed`, a 32.32 split across two dwords). Carry the same thing. The
  division belongs in the format code, which knows both the sample's C-5 rate and the
  output rate, and runs at tick rate. Q16.16 Hz would also cap at 65535 Hz for no
  reason.

### 1.3 Sources: pull, with absolute frames

```rust
pub trait EventSource {
    /// Absolute frame of the next event, or None if idle. Never earlier than the engine clock.
    fn next_event_frame(&self) -> Option<u64>;
    /// Advance internal state to `frame` without producing events.
    fn advance_to(&mut self, frame: u64);
    /// Produce everything due exactly at `frame`.
    fn dispatch(&mut self, frame: u64, ctx: &mut EngineContext<'_>);
}
```

Absolute frames, not deltas — one engine-owned `u64` clock (12 million years at 48 kHz)
instead of every source keeping its own elapsed-time bookkeeping and eventually
desyncing. `dispatch` takes a concrete `EngineContext`; only `EventSource` itself needs
to be `dyn`, and dyn dispatch at ~50 Hz is free.

Three rules that make it work:

1. **Never cache the next-event frame across a dispatch.** `Txx`, `SEx`, `SDx`, `Bxx`
   and `Dxx` all change *when the next tick is* from inside the tick just processed.
   Compute the next boundary at the *end* of processing the current tick. This is also
   the decisive argument against pre-materialising a per-block event schedule
   internally — that would just be running the sequencer with extra steps.
2. **Guard zero-advance.** S3M speed 0, `A00`, MOD `E60` self-loops, pattern-break-to-
   self, SMF zero-delta meta events — any of these can make a source return the same
   frame forever and spin the render loop *inside the audio callback*. Every tracker
   has shipped this bug. Cap consecutive same-frame dispatches (64) and events per
   block (4096); on breach force-advance one frame and set a telemetry warning. **Never
   panic** — a panic in an AudioWorklet kills audio for the page permanently.
3. **Deterministic tie-breaking.** Sort by `(frame, source_slot, sequence)` with a
   stable generational slot, not a `Vec` position that shifts on removal. Otherwise
   offline ≠ real-time whenever two sources coincide.

Pre-materialised event buffers are rejected *internally* but adopted *at the edge*:
VST3/CLAP hand you a pre-sorted timestamped list per block, and so should live MIDI. An
`ExternalEventQueue { ring: SpscConsumer<TimedEvent>, .. }` that implements
`EventSource` gives the plugin and MIDI-in path the right shape without contaminating
the tracker path.

### 1.4 Pattern delay, row delay and the row clock

The original does the naive thing (`if _MRowDelay != 0 { dec; goto @@nonewpattern }` —
effects re-run, notes are not re-fetched), and MOD `EEx`, S3M `SEx`, XM `EEx` and IT
`SEx` all differ subtly, with IT's `SEx` × `SDx` interaction a known minefield. Model
the row's tick budget explicitly, exposing the *absolute* tick index across repeats,
because that is what `Qxy` retrigger, `Ixy` tremor, `SDx` note delay and `SCx` note cut
all key off:

```rust
pub struct RowClock {
    pub speed: u8,            // ticks per row
    pub pattern_delay: u8,    // extra row repeats (SEx / EEx)
    pub tick_in_row: u16,     // 0..speed*(1+pattern_delay) — ABSOLUTE across repeats
    pub repeat_index: u8,
}
```

Each format's effect processor picks the predicate it cares about
(`is_first_tick_of_row` vs `is_first_tick_of_repeat`). Deliberately **not** one shared
`if tick == 0 { .. } else { .. }` — that shared branch is where every cross-format bug
would live. `Axx` (set speed) mid-row changes `speed` while `tick_in_row` is already
past it; the wrap rule is stated per format rather than falling out of modulo
arithmetic by accident.

### 1.5 Channels, voices, instruments — and IT without contaminating S3M

Three distinct concepts: **Channel** (a control lane, holding effect memories and a
bound instrument), **Voice** (one sounding sample/oscillator from a fixed global pool),
**Instrument** (turns channel events into voice behaviour, owns format-specific note
semantics).

The clean concept for IT is **foreground vs background**, and it lives in the
allocator, not in `Channel` and not in the mixer. A channel has one foreground voice
receiving channel effect updates, plus zero or more detached background voices running
only their own envelopes and fadeout until they die.

```rust
pub struct Channel { pub foreground: Option<VoiceId>, .. }   // MOD/S3M/XM use only this
pub struct VoiceTag { pub channel: u8, pub instrument: u8, pub sample: u8, pub note: u8 }
pub enum NewNoteAction { Cut, Continue, NoteOff, NoteFade }
pub enum DuplicateCheck { Off, Note, Sample, Instrument }
```

MOD/S3M/MTM never create a background voice; `NewNoteAction::Cut` +
`DuplicateCheck::Off` early-outs the whole DCT matching loop. Cost to the simple path:
four bytes per voice and one branch.

**One global pool, generational handles.** Not per-instrument sub-pools — those
fragment, and IT needs global stealing against a single virtual-channel limit.
`VoiceId { index: u16, generation: u16 }` with `pool.get_mut(id) -> Option<&mut Voice>`,
because a voice *can* be stolen out from under its owner and silently getting the wrong
one is a multi-day debugging session. Note that IT's voice-stealing heuristic is
audible on dense modules — it is a policy trait with real time budgeted against it, not
something to guess.

### 1.6 Module memory layout — decide this now, not later

```rust
pub struct Module {
    blob: Box<[u8]>,               // decoded pattern data and everything non-sample
    pcm: Box<[i16]>,               // all samples decoded + delta-decoded, concatenated,
                                   //   each with N guard frames appended (loop-wrapped or
                                   //   zeroed) so interpolators need no branches
    samples: Box<[SampleIndex]>,   // { pcm_offset: u32, len: u32, loop_start, loop_end, .. }
    patterns: Box<[PatternIndex]>, // { blob_offset: u32, rows: u16, channels: u8 }
    ..
}
```

**Offsets, not references.** Trivially `Send + Sync`, trivially `Arc`-handed to the
audio thread, trivially hashable for golden tests, trivially fuzzable, mmap- and
flash-friendly on ESP32, no pointer chasing in the mixer. One decision, four
requirements unblocked.

Format crates keep their **native** pattern bytes. The shared model covers samples,
envelopes, instrument definitions and a display-only `PatternCell` view for the UI —
deliberately *not* a shared pattern-cell model, which is exactly the mistake that made
the original's MOD playback inaccurate.

### 1.7 Real-time safety

No allocation, no locks, no panics in `render()`. Specific landmines and their guards:

- Dropping the last `Arc<Module>` on the audio thread calls `free()`. **Garbage
  channel:** the audio thread pushes retired `Arc`s back over an SPSC for the control
  thread to drop.
- CI runs the engine under an allocator hook that panics on any allocation inside
  `render()`, across the whole corpus.
- `#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]` on the
  engine and mixer crates; `render()` outputs silence rather than panicking on an
  internal inconsistency.
- `#![forbid(unsafe_code)]` in the core crates per the repo's working agreements.

**Cross-target determinism:** x86 SSE2, ARM NEON and WASM SIMD agree on `+ - * /`
(IEEE-754 round-to-nearest) but *not* on `sin`/`exp`/`powf` — different libm — and FMA
contraction changes results. So: **transcendentals are banned from the RT path
entirely**; tables only (which is what trackers do anyway — the original ships
vibrato/tremolo tables). FMA contraction disabled. The **fixed-point i16 mixer is the
canonical bit-exact golden reference**; float goldens carry a tolerance. A CI job
renders the same module on x86-64, aarch64 and wasm32 and asserts identical hashes on
the fixed path.

### 1.8 Telemetry — split in two

A single triple-buffered snapshot does not work for oscilloscopes: 32 channels × 512
f32 is 64 KB per snapshot, ×3 buffers, copied at UI rate.

- **(a) Coherent scalar state** — order/row/tick/pattern/speed/BPM, per-channel
  note/instrument/volume/pan/effect (~1 KB): triple buffer or seqlock. The UI needs
  this internally consistent.
- **(b) Lossy audio taps** — scope waveform, VU peaks: per-channel fixed ring, audio
  thread stores a `Relaxed` write index, UI may read torn data. Tearing is invisible on
  a scope. Downsample in the audio thread (peak-per-16-frames) so it ships 32 frames
  per quantum, not 4096.

On WASM, (b) wants `SharedArrayBuffer`, which needs COOP/COEP headers — a `postMessage`
fallback path is planned from the start.

### 1.9 Portability

- Core crates are `#![no_std]` + `alloc`. CI builds `riscv32imc-unknown-none-elf`
  (already installed here, matching the owner's esp-rs work) on every commit. Hard
  rule: **no default feature transitively enables `std`.**
- `portable-atomic` + `critical-section` for targets lacking CAS — `alloc::sync::Arc`
  will hit this on thumbv6m/xtensa.
- Loader IO is a minimal `trait ModuleReader` with a zero-copy `&[u8]` fast path.
  Loading is synchronous over an already-obtained byte source; *acquiring* the bytes is
  async and lives in the platform crates, so the core never needs an async runtime.

### 1.10 Trait discipline

One rule to prevent the failure mode where this project reaches 80% design and 0%
audio: **no trait is committed until its second real implementation exists.** Build S3M
concretely; extract `EventSource` and `Instrument` when MOD lands, and revisit them
when XM lands. The *concepts* above are designed up front and recorded in the
architecture document — the Rust trait boundaries follow the implementations. The
exceptions, which have two users from day one, are `TempoModel` and `EventSource`
(pattern sequencer + external queue).

### 1.11 Crate layout

```
starplayer/
├── AGENTS.md               # rewritten (CLAUDE.md is a symlink to it)
├── Cargo.toml              # virtual workspace, resolver = "2"
├── STARPLAY/               # original DOS sources — read-only historical reference
├── plans/                  # design + implementation plans (Part 2)
├── crates/
│   # ── no_std + alloc ──────────────────────────────────────────────────────
│   ├── starplayer-core       # fixed-point (Q16.16/Q32.32), Frame/Step/Note, Event,
│   │                         #   VoiceParams + DirtyBits, TempoModel, tables. No IO.
│   ├── starplayer-rt         # SPSC ring, triple buffer, seqlock, lossy tap ring
│   ├── starplayer-dsp        # interpolators, ramping, IT filter, biquad, reverb/chorus/
│   │                         #   compressor, SIMD backends (scalar/sse2/neon/simd128)
│   ├── starplayer-mixer      # VoicePool, voice render kernels, buses, output formats
│   ├── starplayer-model      # shared Module (blob + u32 offsets), Sample, Envelope,
│   │                         #   InstrumentDef, display-only PatternCell
│   ├── starplayer-engine     # render loop, EventSource, Instrument, channel binding,
│   │                         #   RENDER_QUANTUM adapter, command queue, telemetry
│   ├── starplayer-mod        # MOD loader + ProTracker effect processor
│   ├── starplayer-s3m        # S3M loader + ST3 effect processor (+ quirk profile)
│   ├── starplayer-mtm        # MTM loader + effect processor
│   ├── starplayer-xm         # XM  loader + effect processor
│   ├── starplayer-it         # IT  loader + effect processor + NNA policy
│   ├── starplayer-midi       # MIDI byte codec + SMF parser + Event mapping
│   ├── starplayer-telemetry  # snapshot types shared by every UI
│   └── starplayer            # facade: re-exports + format autodetect (the public crate)
│   # ── std ─────────────────────────────────────────────────────────────────
│   ├── starplayer-host-cpal  # native audio output
│   ├── starplayer-host-wasm  # AudioWorklet glue
│   ├── starplayer-offline    # WAV writer, deterministic render, trace dump
│   └── starplayer-testkit    # golden compare, libxmp/openmpt diff harness, trace differ
├── apps/
│   ├── starplayer-web        # wasm-bindgen + responsive web UI  ← first deliverable
│   ├── starplayer-cli        # CLI player / renderer
│   └── starplayer-tui        # STAR.EXE homage (ratatui)
└── xtask/                    # build orchestration, wasm packaging, golden regeneration
```

Loader and effect processor live in the **same crate per format**, because file
semantics and effect semantics are inseparable — one crate, one feature flag, one
dependency edge. Dependency edges are strictly one-directional; apps depend only on
the facade. Splitting `starplayer` out for crates.io later is a manifest change, not a
refactor.

Feature flags: `default = ["s3m", "mod", "mtm", "float-mix", "linear-interp"]`, plus
`std`, `simd`, `fixed-mix`, `telemetry`, `trace`, `quirks-starplayer`, `serde`, and one
per format.

Candidate dependencies: `fixed`, `bytemuck`, `heapless`, `portable-atomic`,
`rtrb`/`triple_buffer` (std hosts only), `midly`, `cpal`, `midir`, `wasm-bindgen`,
`ratatui`, `clack`/`nih-plug`. Test-only: `libopenmpt`/`libxmp` bindings, `cargo-fuzz`,
`assert_no_alloc`.

---

## Part 2 — Repo and documentation structure

Mirror the convention already proven in the owner's `ampkeeper` repo.

```
plans/
├── README.md                          # unified status index + milestone table
├── product/
│   ├── 00-vision.md                   # what StarPlayer is, audiences, reuse goals
│   ├── 01-technical-architecture.md   # Part 1, expanded — the source of truth
│   ├── 02-roadmap.md                  # sequencing, effort, risks
│   └── 03-accuracy-policy.md          # per-format fidelity targets, documented deviations
├── reference/
│   ├── original-s3mlib-analysis.md    # engine archaeology (structures, effects, timing, drivers)
│   ├── original-star-ui.md            # the STAR.EXE screen, hotkeys, options — for the TUI
│   └── format-notes-<fmt>.md          # per-format quirk notes, written as each loader lands
├── engine/                            # M<n>-master-plan.md + M<n>-task-<ID>-<name>.md (+ complete/)
└── apps/                              # A<n>-master-plan.md + A<n>-task-<ID>-<name>.md (+ complete/)
```

Task files follow ampkeeper's exact shape — a header table
(`Milestone / Depends on / Blocks / Recommended model / Verified by`) then **Context for
a fresh agent**, **Deliverables**, **Research points**, **Verification**, **Out of
scope** — written so an agent with no conversation history can execute them from the
file alone.

The two `reference/` archaeology documents are already substantially researched and
should be written up in full during this planning work: the engine analysis covers the
`ChannelData`/`Module` structures, the `__UpdateTracker` major/minor tick structure, all
26 effect handlers with their tick-0-vs-later semantics and shared parameter memories
(D/E/F share `_VolSlideValue`; H/R/U share `_VibValue` and `_VibCount`), the period and
finetune tables, the three loaders, and both hardware drivers. The UI analysis has the
full 80×25/80×50 screen layout, exact field positions, the CGA colour scheme, the 16-cell
green→yellow→red VU table, the `LFT`/`1`–`E`/`RGT` pan display, the **effect names spelled
out in English per channel** (the single most charming idea in the program), every popup
panel, all hotkeys, and the `DeCrunch` TheDraw screen bytecode.

### `AGENTS.md`

Gains, above the existing working agreements (which are restated verbatim): the
project's purpose; the non-negotiable design goals (sample-exact event timing;
split-for-mixing/quantise-for-DSP; buffer-size-independent output as a testable
invariant; `no_std`+`alloc` core; the RT-safety rules; canonical fidelity with
documented deviations); the crate map; a pointer to `plans/README.md`; and the rule that
`STARPLAY/` is a read-only historical reference that must never be modified.

### Delegation workflow (standing guideline, recorded in `AGENTS.md`)

1. Write a detailed task spec under `plans/engine/` or `plans/apps/`.
2. Create a branch and a **sibling worktree** (`../starplayer-<branch>`).
3. Delegate implementation to a **GPT-5.6-sol** worker against the task file.
4. Review the diff, run the task's verification section, then commit — staging explicit
   paths only, per the existing `git add` rule.
5. Move the plan to the area's `complete/` once landed and only owner acceptance remains.

---

## Part 3 — Milestones

Effort units: 1u ≈ one focused week.

| # | Milestone | Exit criterion | Est. |
|---|---|---|---|
| M0 | Foundations + WASM spike | **Sound from a browser tab** (Rust sine wave through AudioWorklet) | 1u |
| M1 | S3M in the browser | **A real `.s3m` plays correctly in a browser** | 2.5u |
| M2 | MOD + MTM native, accuracy machinery | Conformance corpus green for MOD/S3M/MTM | 2u |
| M3 | Native surfaces | cpal host, CLI player, offline WAV renderer, telemetry split | 1.5u |
| M4 | Generalise to a synthesis engine | MIDI in, SMF playback, keyboard triggering, source mux | 1.5u |
| M5 | XM support | Envelopes, key-off/fadeout, linear frequency, multi-sample instruments | 1.5u |
| M6 | IT support | NNA/DCT/DCA, global voice pool + stealing policy, resonant filter, compressed samples | 2u |
| M7 | DSP graph | Per-channel inserts, master bus, reverb/chorus/compressor, SIMD, higher-order interpolation | 1.5u |
| M8 | Embedded proof | esp32 riscv playing a module from flash via I2S under embassy | 1u |
| M9 | Plugin surfaces | CLAP instrument, then effect hosting; VST3 via wrapper if warranted | 2u |
| M10 | Alternative synths | FM, wavetable, SID, SoundFont; sample-enhancement plugin API | pull-driven |
| A1 | TUI STAR.EXE homage | The original screen, in a modern terminal | 1.5u |
| A2/A3 | Desktop & mobile shells | Windows/macOS, iOS/Android over the same engine | pull-driven |

M0–M1 is the critical path to something usable. M2–M3 make it provable and portable.
M4–M6 complete the engine and format story. M7 onward is explicitly pull-driven,
mirroring ampkeeper's M7 backlog convention.

### Milestone notes

**M0 — Foundations + WASM spike.** No tracker code. Workspace skeleton; `starplayer-core`
fixed-point types and the frame clock; a one-voice mixer; **an AudioWorklet playing a
Rust-generated sine wave in a browser**; CI (host tests, `wasm32-unknown-unknown`,
`riscv32imc-unknown-none-elf` `no_std` check, clippy, `forbid(unsafe_code)`); the
block-size determinism test. Rewrite `AGENTS.md`, create the `plans/` tree and both
`reference/` archaeology documents.

The AudioWorklet path is deliberately first because it is the highest-uncertainty
infrastructure in the project *and* it is on the critical path: no `SharedArrayBuffer`
without COOP/COEP; no `fetch`/dynamic import inside a worklet in some browsers, so the
wasm module must arrive as bytes via `postMessage`; a hard 128-frame quantum; growing
wasm memory on the audio thread causes an audible glitch; `wasm-bindgen` output needs
manual massaging for worklet scope. Prove all of it with a sine wave before any tracker
code exists. Prerequisites not yet installed here: `rustup target add
wasm32-unknown-unknown` and `wasm-bindgen-cli`/`wasm-pack` (riscv32 targets and Node 24
are already present).

**M1 — S3M in the browser.** The `Module` blob-and-offsets model; the S3M loader
(`SCRM` validation, parapointers, packed pattern unpacking, unsigned 8-bit samples,
default panning from the channel-settings array and the optional 32-byte pan block); the
ST3 effect processor (`Axx`–`Xxx` with tick-0 vs per-tick semantics, shared D/E/F and
H/R/U parameter memories, `Cxx` read as decimal, `SDx` note delay via the saved dirty-flag
byte, `SBx` pattern loop, `SEx` pattern delay, glissando and Amiga-limit clipping); the
engine render loop with the 128-frame DSP quantum; linear-interpolated float stereo
mixing; telemetry v1; and the web player with transport controls plus order/row/channel
display. The task spec is written effect-by-effect from the assembly, each stating its
tick-0 and per-tick behaviour and any canonical deviation being taken.

**M2 — MOD + MTM native, plus accuracy machinery.** ProTracker and MTM effect processors
written natively rather than via S3M conversion, but reproducing the original's
conversion tables as *semantics*: the finetune → C2SPD tables, the LRRL channel panning
map, the loop-length > 4 gate, the Amiga-limits derivation from the song's octave range,
and the sign conventions. Plus `QuirkSet`, the `TempoModel` policies, the fixed-point
mixer path, the per-tick trace format, golden hashes, and loader fuzzing.

**M3 — Native surfaces.** Deliberately *after* WASM, so the host abstraction is proven
under the harder constraint first.

**M4 — Generalise.** This is where `EventSource` and `Instrument` are extracted properly
and the Layer-1 event vocabulary is designed — informed by two real implementations
rather than by speculation in M0.

**M8 — Embedded proof.** The `no_std` CI check proves it compiles; this proves it plays.
Where the offsets-not-references decision pays off.

### Testing strategy

Ordered by value per unit of effort:

1. **Block-size determinism** (M0, before any format code) — render at block sizes
   1, 3, 64, 128, 4096, 8191; assert byte-identical.
2. **Per-tick state trace diffing** — the highest-value tool in the project. A stable
   text format behind `feature = "trace"`, one line per channel per tick with note,
   instrument, volume, period, pan, sample position and dirty flags.
3. **libxmp's `test-dev/` suite** — the real gift here. Hundreds of purpose-built
   one-behaviour-per-module files *with frame-by-frame expected channel-state dumps*:
   both corpus and oracle for MOD/S3M/XM/IT effect semantics. Mine it **before** writing
   effect code. OpenMPT's `test_*.it/.xm/.s3m/.mod` collection is the second source, with
   documented expected behaviour on the OpenMPT wiki.
4. **Golden hashes** — SHA-256 of a fixed-point i16 mono 44100 Hz render with linear
   interpolation and DSP bypassed. The filename encodes the config
   (`s3m/foo__i16_mono_44100_linear.sha256`) so an interpolator change is visibly a new
   golden rather than a silent break. WAVs stay out of the repo; regenerate via
   `cargo xtask goldens`.
5. **Cross-target hash equality** — same module rendered on x86-64, aarch64 and wasm32,
   fixed-point path, identical hashes.
6. **Loader fuzzing** (`cargo-fuzz`) from the first loader. Malformed modules are the #1
   crash source in every tracker ever written. Invariant: never panic, never OOM, always
   `Err`.
7. **RT-safety** — allocator hook panicking on any allocation inside `render()`.
8. **Properties** — no NaN/Inf ever; the voice pool returns to zero active after song
   end; `next_event_frame()` never returns the past; the zero-advance guard never trips
   on the corpus.
9. **Perceptual comparison vs libopenmpt** for the float path (spectral distance /
   segmental SNR with tolerance) — nightly, not a gate.

The deferred DOS reference gets a task file with an explicit trigger: if M1/M2 hit an
effect ambiguity the assembly can't settle, strip the `comment %` wrappers into a
*separate reconstructed copy*, rebuild via the intact `P.BAT` chain, and write a small
DOS harness that calls `PM_LoadModule` then drives `__UpdateTracker` N times, dumping the
`ChannelData` array per tick. That yields a reference **state trace** directly comparable
to (2) — far more useful than WAV-diffing DOSBox, whose SB emulation resamples and would
have us diffing emulator artefacts.

### Top risks

1. **Effect-semantics fidelity.** The assembly is the only specification and the
   subtleties are exactly where players differ. *Mitigation:* effect-by-effect task
   specs, trace diffing, and the libxmp oracle mined before code is written.
2. **DSP block granularity silently breaking determinism.** *Mitigation:* the
   quantise-for-DSP rule plus the day-one block-size test.
3. **RT-safety eroding silently — `no_std` does not prevent it.** *Mitigation:* garbage
   channel for retired `Arc`s, allocator-hook CI, clippy denials.
4. **AudioWorklet plumbing.** *Mitigation:* it is M0, proven with a sine wave before
   anything depends on it.
5. **Abstraction paralysis.** Designing traits for SID + FM + physical modelling + VST +
   embassy before one note plays is how this project dies at 80% design and 0% audio.
   *Mitigation:* the §1.10 rule, and a roadmap that is explicitly pull-driven past M7.

Honourable mentions: IT filter and NNA accuracy (worth two weeks on their own);
interpolator choice silently invalidating every golden (pinned in the filename);
`alloc::sync::Arc` on no-CAS targets.

---

## Part 4 — What this plan produces

Documents only — no code.

| File | Content |
|---|---|
| `AGENTS.md` (rewrite) | Purpose, design goals, crate map, delegation workflow, `STARPLAY/` read-only rule, existing working agreements verbatim |
| `plans/README.md` | Index + milestone status table |
| `plans/product/00-vision.md` | Purpose, audiences, reuse goals, the seven owner decisions |
| `plans/product/01-technical-architecture.md` | Part 1, expanded with full type signatures and rationale |
| `plans/product/02-roadmap.md` | Part 3, with sequencing, effort, testing strategy and risks |
| `plans/product/03-accuracy-policy.md` | Per-format fidelity targets; the documented deviations from the original's defects; the `QuirkSet` design |
| `plans/reference/original-s3mlib-analysis.md` | Full engine archaeology — already researched |
| `plans/reference/original-star-ui.md` | Full UI archaeology — already researched, for A1 |
| `plans/engine/M0-master-plan.md` … `M2-master-plan.md` | Master plans for the critical path |
| `plans/engine/M<n>-task-*.md` | Delegation-ready task specs for M0–M2 |
| `plans/engine/M3-…-M10-master-plan.md` | One master plan each, task files written when pulled |
| `plans/apps/A1-master-plan.md` | The TUI homage |
| `plans/apps/A2-…A3-master-plan.md` | Desktop and mobile shells, both pull-driven |

---

## Part 5 — Verification

This plan's deliverable is documentation, so verification is a review pass:

1. `plans/README.md` links resolve; its milestone table matches `02-roadmap.md`.
2. The architecture document's crate layout matches what M0 will actually create.
3. Every M0–M2 task file is self-contained — executable by an agent with no
   conversation history, naming its dependencies, what it may run concurrently with,
   and its verification steps.
4. `03-accuracy-policy.md` lists every known deviation from the original with its
   justification, and every quirk deliberately preserved.
5. `AGENTS.md` restates the existing working agreements unchanged.

At M0 execution time the first real gates are: `cargo test --workspace`;
`cargo build --target wasm32-unknown-unknown`;
`cargo check --target riscv32imc-unknown-none-elf -p starplayer-core --no-default-features`;
`cargo clippy --workspace -- -D warnings`; the block-size determinism test; and a browser
tab producing a sine wave from Rust through an AudioWorklet.
