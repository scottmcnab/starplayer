# M4 — E4: The `Instrument` trait, the instrument rack and the MIDI event source

| Field | Value |
|---|---|
| Milestone | M4-full ([master plan](M4-master-plan.md), "M4-full — the task graph") |
| Status | Ready — not yet dispatched |
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
