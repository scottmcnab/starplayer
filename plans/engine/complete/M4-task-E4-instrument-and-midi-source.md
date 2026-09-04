# M4 — E4: The `Instrument` trait, the instrument rack and the MIDI event source

| Field | Value |
|---|---|
| Milestone | M4-full ([master plan](M4-master-plan.md), "M4-full — the task graph") |
| Status | Landed 2026-09-04 |
| Depends on | Everything landed by 2026-09-04 (E1–E3, D3–D9, F1–F6, G1–G6) |
| Blocks | E5, E6, E7 |
| Recommended model | Claude Opus (commits the trait; touches the engine's source and channel machinery) |
| Verified by | agent (`cargo test --workspace`, the new offline determinism test, `--job rt-safety`, goldens), then reviewer |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there are non-negotiable, above all: no allocation, locks or panics in
`render()`; byte-identical output at every host block size; tables only on the RT path;
and design goal 8, **no trait until its second real implementation exists** — which this
task satisfies rather than bends.

Architecture §2.4 draws the engine as two peers feeding one voice pool: tracker patterns
write voice parameters directly, and a musical layer (`SmfSequencer`, live MIDI, keyboard)
sends `NoteOn`/`NoteOff`/controller events to an **`Instrument`**, which turns them into
voices. The tracker half is done for five formats. Nothing on the musical half exists:
`Event`/`TimedEvent` (`crates/starplayer-core/src/event.rs`) are declared and unused by
any playing code, `ControlClock` (`crates/starplayer-engine/src/control.rs`) ticks and
nothing listens, and `starplayer-midi` is an empty crate. This task builds the musical
half's engine side. The decisions in the master plan's "Decisions that shape the tasks"
section are settled; do not reopen them, implement them.

### Code you must read before changing anything

- `crates/starplayer-core/src/event.rs` — `Event`, `TimedEvent`, `Target`, `Note`,
  `U0F16`/`I1F15`, `VoiceParams`, `TriggerSpec`, `Command`.
- `crates/starplayer-engine/src/{source,control,channel,sequencer,engine,scope,telemetry}.rs`
  — `EventSource`, `EngineContext`, `SourceMux` (`insert`, `take_first`, the tie-break),
  `ScriptedSource` (the shape of a scripted event feed for tests), `ControlClock`
  (`interval_frames`, `next_synthesised_frame`, `tick_synthesised`), `ChannelTable`
  (`trigger`, `stop`, `detach_foreground`, `MAX_CHANNELS`), `TickContext` (how the
  tracker path triggers and writes voices — the rack does the same through
  `EngineContext`), `Engine::set_source`/`replace_source`, `capture_channels`.
- `crates/starplayer-mixer/src/{voice,sample}.rs` — `VoicePool`, `VoiceTag`,
  `SampleRegion`, `Voice::stop`.
- `crates/starplayer-model/src/{instrument,sample,module}.rs` — `InstrumentDef`
  (`sample`, `note_sample_map`, `note_transpose_map`, `default_volume`, `global_volume`,
  `default_pan`), `SampleIndex` (`reference_rate_hz`, `relative_note`, `finetune`,
  `default_pan`, loops), `Module::instrument`/`sample`.
- `crates/starplayer-s3m/src/processor.rs` `sample_region` and `step_from_period`;
  `crates/starplayer-xm/src/processor.rs` and `crates/starplayer-it/src/processor.rs` —
  how each derives a `Step` from a note, a sample's `relative_note`/`finetune` or
  `C5Speed`, and linear frequency (`LINEAR_FREQUENCY_TABLE`); reuse their public helpers
  or lift a shared one into `starplayer-core` rather than writing a fourth.
- `crates/starplayer-rt/src/{spsc,snapshot}.rs` — the SPSC ring the queue wraps.
- `crates/starplayer-offline/src/lib.rs` — `render_song`, the block-size determinism
  tests; `crates/starplayer-engine/tests/block_size_determinism.rs`.
- `plans/product/01-technical-architecture.md` §2, §3, §5 (all), §8, §12 Q4;
  `plans/engine/M11-master-plan.md` (what is deliberately *not* done here).

## Deliverables

### 1. `starplayer_engine::instrument` — the trait and its two implementations

```rust
pub struct NoteParams { pub note: Note, pub velocity: U0F16, pub pan_override: Option<I1F15> }
pub trait Instrument {
    /// Start (or restart) the channel's voice for `note`. Returns the voice, or `None` if
    /// the instrument sounds nothing for that note or the pool is full.
    fn note_on(&self, channel: ChannelId, params: NoteParams, context: &mut EngineContext<'_>) -> Option<VoiceId>;
    /// Release the channel's voice: for a sample with no release stage, stop it.
    fn note_off(&self, channel: ChannelId, context: &mut EngineContext<'_>);
    /// Once per control tick: advance whatever articulation the instrument has.
    fn control_tick(&self, channel: ChannelId, context: &mut EngineContext<'_>);
    /// Apply a pitch bend in cents (already scaled by the rack's bend range) to the channel's voice.
    fn set_bend(&self, channel: ChannelId, cents: i16, context: &mut EngineContext<'_>);
}
```

- `SampleInstrument { module: Arc<Module>, instrument: InstrumentId }` for MOD, S3M and
  MTM (`InstrumentDef::sample`): MIDI note 60 plays the sample at `reference_rate_hz`;
  other notes through the ST3 period table or the linear table (research point 1 picks
  one integer path for all three); volume = velocity × `default_volume` × sample volume;
  pan = the override, else the sample's `default_pan`, else centre.
- `MappedInstrument` for XM and IT: `note_transpose_map` then `note_sample_map` select
  the sample; XM `relative_note`/`finetune` and IT `C5Speed` apply; `global_volume`
  and instrument/sample default pans apply. No envelopes, fadeout, NNA or auto-vibrato —
  say so in the docs and point at M11.
- `note_off` stops the voice through `ChannelTable::stop` (the mixer's 64-frame ramp makes
  it click-free); `set_bend` rewrites `Step` from the note plus cents through the
  linear-frequency table; `control_tick` is a no-op for both today but is called.
- A constructor `instrument_for(module: &Arc<Module>, index: InstrumentId) -> Box<dyn Instrument>`
  that picks by `ModuleFormat`. Trait objects at event rate are fine (§8: "trait objects at
  block level are fine").

### 2. `InstrumentRack`

Sixteen channel slots (`MIDI_CHANNEL_BASE = 48`, engine `ChannelId(48 + n)`), each with
its bound instrument index, program, controller state (CC7 volume, CC10 pan, CC64 sustain
holding note-offs, CC120/123 all sound/notes off, pitch bend with a ±2-semitone range),
and the notes currently held per channel (a fixed-capacity array, no allocation). It
consumes `Event`s: `NoteOn`/`NoteOff` (velocity 0 is off), `Program(InstrumentId)`,
`Controller`, `PitchBend`, `AllNotesOff`, `AllSoundOff`, `Cut`; `Trigger`/`Param` are
reported as unsupported (they are the tracker vocabulary). Built once for a module
(`InstrumentRack::for_module(&Arc<Module>)`, allocating its `Box<[Box<dyn Instrument>]>`
off the audio thread); a module swap rebuilds it with the same retirement path modules use.

### 3. `ExternalEventQueue` and `MidiSource`

- `ExternalEventQueue { consumer: Consumer<TimedEvent>, peeked: Option<TimedEvent> }`
  over `starplayer_rt::spsc`, with `external_event_channel(capacity) -> (ExternalEventProducer, ExternalEventQueue)`;
  `next_event_frame` is the peeked frame, a frame already past dispatches at the
  current frame and increments a `late_events` counter the telemetry warnings expose.
- `MidiSource<Feed: EventFeed>` implementing `EventSource`: `Feed` is the small trait
  both the external queue and E5's SMF sequencer satisfy (`next_frame`, `pop_due`); the
  source owns the `InstrumentRack` and its **own control tick** (master-plan decision 2:
  `ControlClock::interval_frames` at the engine rate, ticked from inside `dispatch`), and
  reports `min(feed.next_frame, next_control_tick)`. With a tracker in the mux the
  `SourceMux` tie-break orders the two sources deterministically; nothing else is needed.
- `Engine::set_source` already takes any `Box<dyn EventSource>`; no engine change unless
  research point 3 finds one.

### 4. Telemetry and tags

`VoiceTag { channel: 48 + n, instrument, sample, note }`; `report_note`-equivalent
updates so the snapshot's channels 48–63 show the MIDI note and instrument (the wasm
header already carries 64 channels); `EngineWarnings` gains `late_events`.

### 5. Proof

- `crates/starplayer-engine/tests/midi_source.rs`: a `ScriptedFeed` of `TimedEvent`s
  (note on/off, program, bend, sustain) against the synthetic MOD and IT fixtures'
  instruments renders byte-identically at block sizes 1, 3, 64, 128, 4096, 8191, on both
  mix paths; a late event dispatches at the current frame and counts a warning; sustain
  holds a note-off until CC64 releases; `AllSoundOff` empties the pool.
- `render_allocation.rs` gains a MIDI-source arm: no allocation in `render()` with the
  rack driving.
- The existing goldens and conformance results are untouched (nothing on the tracker
  path changes).

### 6. Documentation

Architecture §5.3 becomes the committed trait; §5.4 gains the amendment from master-plan
decision 2; §3.3's table lists `MidiSource`/`ExternalEventQueue` as landed; §12 Q4 stays
open with a pointer to M10. `plans/README.md` M4 row.

## Research points

1. **One integer pitch path for `SampleInstrument`.** MOD/S3M/MTM samples carry only
   `reference_rate_hz`; choose between the ST3 period table (what those formats play by)
   and the linear table (what bend uses) so that MIDI 60 is exact and a bend of 0 cents
   changes nothing. Record it.
2. **Bend resolution.** `I1F15` bend × 200 cents through `LINEAR_FREQUENCY_TABLE`
   (768 per octave = 1/64 semitone): document the quantisation.
3. **Does `Engine` need to know about a synthesised control tick?** Decision 2 says no —
   the source schedules its own. Confirm nothing in `render_quantum` or `ControlClock`
   needs to change, or make the minimal change and say why.
4. **Sustain and held notes per channel.** Cap held notes per channel (16?) with a
   documented steal-oldest rule when exceeded.

## Verification

```sh
cargo test -p starplayer-engine
cargo test -p starplayer-engine --test midi_source
cargo test --workspace
cargo xtask goldens --check
cargo xtask conformance --offline          # unchanged
cargo xtask ci --job rt-safety
cargo xtask ci --job no-std-check
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
```

**Do not commit** — the reviewer commits.

## Out of scope

MIDI bytes and SMF (E5), any host input (E6), the mux acceptance (E7), envelopes/NNA
for MIDI instruments (M11), non-sample instruments (M10).

## Research resolution

Recorded at implementation time; each answer is the decision, not a summary of the
question.

### 0. Deviations from the task file — **four, all small, all deliberate**

1. **`InstrumentRack::for_module` and `instrument_for` take a sample rate.** The task
   writes `InstrumentRack::for_module(&Arc<Module>)` and
   `instrument_for(module, index) -> Box<dyn Instrument>`. Neither can work: an instrument
   turns a note into a `Step`, which is *frames per output frame*, so it needs the output
   rate, and `EngineContext` does not carry one (the tracker processors each store it at
   construction for the same reason). Both take `sample_rate_hz` as a third argument, which
   is also what a module swap already has to hand.
2. **The determinism test builds its two modules with `ModuleBuilder`** rather than loading
   `starplayer_offline::fixtures::synthetic_mod()` and `synthetic_it()`. Those fixtures are
   *file bytes*, so using them from `crates/starplayer-engine/tests/` would need
   dev-dependencies on `starplayer-mod` and `starplayer-it`, which depend on the engine —
   a dev-dependency cycle, for fixtures whose value here is their instrument tables rather
   than their bytes. The test builds a MOD-format module (one sample per instrument) and an
   IT-format one (note-sample and note-transpose maps, per-sample tuning, a ping-pong loop)
   directly. The real fixtures are covered where they belong, in
   `crates/starplayer-offline/tests/render_allocation.rs`, whose new MIDI arm plays the
   synthetic IT's instruments through a live `ExternalEventQueue`.
3. **`EventFeed` gained a third method, `refresh`.** See research point 3's answer below:
   `EventSource::next_event_frame` takes `&self`, so a feed that has to *look* for its next
   event — which is exactly what an SPSC ring is — cannot fill its lookahead there. The
   method has a default no-op body, so E5's `SmfSequencer` implements two methods, as the
   task specifies.
4. **The rack's late-event counting lives in `MidiSource`, not in `ExternalEventQueue`.**
   The task puts the `late_events` counter on the queue. It is the *source* that knows the
   frame an event was dispatched at, and putting it there means every feed — E5's SMF
   sequencer included — gets late-event accounting for free instead of each implementing
   it. `MidiSource::late_events()` reports the count and
   `EngineContext::report_late_event` raises the engine's flag.

### 1. One integer pitch path for `SampleInstrument` — **the linear-frequency table, for all five formats**

The ST3 period table cannot meet the task's own two conditions. `period_from_note`
truncates (`PERIOD_TABLE[n] · 8363 · 16 >> octave` **divided by** the reference rate) and
`step_from_period` truncates again (`14317056 / period`), so MIDI 60 on a sample whose
reference rate is 8363 Hz comes back as 8362 or 8364 Hz depending on the sample, and the
error moves with the rate. The linear table is exact at the reference note by construction:
`LINEAR_FREQUENCY_TABLE[0]` is `1 << 24` and the rounding shift undoes it, so
`scale_frequency(rate, 0) == rate` for **every** rate. That is what makes master-plan
decision 4 — "MIDI note 60 plays a sample at its reference rate" — and "a bend of zero
cents changes nothing" true rather than nearly true, and it is the table pitch bend has to
use anyway; the alternative was two tables disagreeing by a few cents at the ends of the
keyboard.

So `SampleInstrument` and `MappedInstrument` share one formula:
`frequency = scale_frequency(sample.reference_rate_hz(), units)` where `units` is
1/64ths of a semitone from note 60, plus the sample's own `relative_note`/`finetune` where
the format has them. **This is not an accuracy deviation**: no tracker pattern reaches this
code. The five format processors keep their own period arithmetic, which is what the
goldens and the conformance corpus pin, and both are byte-identical before and after this
task.

Rather than write a fourth copy of the arithmetic, IT's own `scale_frequency` — which was
already exactly this function — **moved to `starplayer_core::tables::scale_frequency`**,
with `LINEAR_UNITS_PER_SEMITONE` beside it. `starplayer-it` now calls it; its body is
unchanged, so the IT conformance results are unchanged. (The other candidate for lifting,
`sample_region`, is a *projection* of `SampleIndex` into the mixer's vocabulary rather than
arithmetic, and IT's version chooses the sustain loop, which is key-off semantics. The
engine has its own `instrument::sample_region` for the non-sustain case and the format
crates keep theirs.)

### 2. Bend resolution — **1.5625 cents, and it is FT2's and IT's own**

`cents_to_units(cents) = round(cents × 64 / 100)`, and one unit is 1/768 of an octave =
1/64 of a semitone = **1.5625 cents**. The default ±200-cent range therefore resolves to
**256 steps**, not MIDI's 16,384: a wheel moved by one fourteen-bit LSB usually changes
nothing, and two adjacent bend words can produce the same step. That is deliberate — it is
the pitch resolution FastTracker 2 and Impulse Tracker themselves have, and using it keeps
the MIDI path on the same table as the tracker path and therefore bit-identical across x86,
ARM and WASM (design goal 5). A finer bend would mean a second, wider table used by nothing
else.

The rack scales the wheel by its range first — `bend.to_bits() × range_cents / 32767` — so
the quantisation happens once, in the instrument, against the note.

### 3. Does `Engine` need to know about a synthesised control tick? — **no engine change; one addition to `EngineContext`**

Confirmed by construction. `Engine::render_quantum` already asks every source for its next
frame and splits the segment there, and `MidiSource::next_event_frame` reports
`min(feed, own control clock)`, so a control tick lands on an exact frame with no engine
support at all. `ControlClock` needed nothing either: `MidiSource` owns a second instance
of it — `with_interval_micros` at the engine's rate — and the engine's own clock keeps
running unread beside it. That redundancy is the point: a tracker sharing the mux calls
`tick_from_tracker`, which switches the *engine's* clock to `ControlDriver::Tracker` and
stops it synthesising, and a MIDI instrument must not fall silent because somebody else's
module started playing. The cost is one `u64` increment per millisecond.

Two things were added rather than changed, neither on the tracker path:

* **`EngineContext::report_late_event`**, backed by an `Option<&'engine mut EngineWarnings>`
  field the engine attaches the way it already attaches the telemetry publisher.
  `EngineContext::new` keeps its four-argument signature, so the format crates' tests and
  the offline scanner are untouched. `EngineWarnings::late_events` and its telemetry mirror
  `WarningFlags::late_events` follow, plus bit 4 of the wasm snapshot's warning word.
* **`EventFeed::refresh`**, for the reason in deviation 3 above. `ExternalEventQueue`
  refills its one-event lookahead there; the engine calls `advance_to` at the end of
  **every render segment**, so an event in the ring at a segment boundary is visible before
  the next segment is planned and dispatches on its exact frame. The host's two-quantum
  stamping lead (master-plan decision 6) is what keeps that true for an event pushed while
  a segment is being rendered.

### 4. Sustain and held notes per channel — **sixteen, steal the oldest, and the stolen note is forgotten rather than stopped**

`MAX_HELD_NOTES = 16` per channel, in a fixed array with a parallel "note-off arrived while
the pedal was down" flag. Sixteen is past what ten fingers and a sustain pedal produce, and
a fixed array is what keeps the rack allocation-free.

The steal rule falls out of the channel being **monophonic**: a channel drives one
foreground voice, so it sounds its *newest* note and the held list exists for bookkeeping —
whose note-off matters, and what the pedal is holding. A seventeenth note therefore drops
the **oldest** entry, and dropping it is audibly free, because that note has not been
sounding since the note after it arrived. The only observable consequence is that the
stolen note's own `NoteOff` finds nothing to release, which is the correct outcome: the
voice it would have released is already gone.

Sustain: `CC64 ≥ half scale` sets the pedal. A `NoteOff` while it is down marks the held
note `releasing` and stops nothing; raising the pedal walks the held notes newest-first,
removes every marked one and releases whichever of them is the sounding note. `AllNotesOff`
and `AllSoundOff` both clear the list and lift the pedal — a pedal left down through a
panic would swallow the next note-off.
