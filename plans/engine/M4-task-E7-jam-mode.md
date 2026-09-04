# M4 — E7: Jam mode — a module and live input through one `SourceMux`

| Field | Value |
|---|---|
| Milestone | M4-full ([master plan](M4-master-plan.md)) |
| Status | Implemented 2026-09-04 — review and owner hardware jam check pending |
| Depends on | E4, E5, E6 |
| Blocks | M4 exit; M9 (the plugin edge reuses `ExternalEventQueue`) |
| Recommended model | Claude Sonnet (assembly of landed parts plus the acceptance test) |
| Verified by | agent (the two-source determinism test, both hosts), then reviewer, then the owner jams over a module |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first.

M4's exit criterion: "A MIDI file plays; a connected MIDI keyboard triggers notes from a
loaded module's instruments; **both at once, over a playing module, with deterministic
ordering**." `SourceMux` (`crates/starplayer-engine/src/source.rs`) has existed since
M1-B3 with the architecture §3.1 rule-3 tie-break — `(frame, source_slot, sequence)` —
and a unit test, but no host has ever put two real sources in it. E4 landed `MidiSource`,
E5 `SmfSequencer`, E6 the live feeds. This task puts the module's `NativeSequencer` and a
`MidiSource` in one mux on both hosts and proves the ordering is deterministic.

### Code you must read before changing anything

- `crates/starplayer-engine/src/source.rs` (`SourceMux`, `SourceSlot`, the tie-break and
  its test), `engine.rs` (`set_source`, `replace_source`).
- `crates/starplayer-host/src/player.rs` — how a source is built on load and swapped; the
  retirement path for a replaced source (D4).
- `crates/starplayer/src/sequencer.rs`, `crates/starplayer-engine/src/midi_source.rs`.
- `crates/starplayer-offline/src/lib.rs` and `tests/` — the block-size determinism tests.
- `apps/starplayer-cli/src/play.rs`, `apps/starplayer-web/www/app.js`,
  `crates/starplayer-host-wasm/src/lib.rs`.
- `plans/product/01-technical-architecture.md` §3.1 rule 3, §3.3.

## Deliverables

1. **`Player::jam(enabled: bool)`**: with a module loaded, installs a `SourceMux` holding
   the module's `NativeSequencer` (slot 0) and a `MidiSource<ExternalEventQueue>` over the
   module's instruments (slot 1), swapping through `replace_source` so the retired source
   goes down the retirement ring; disabling restores the sequencer alone without losing
   the song position. A module swap rebuilds both. Seeks and transport keep working in
   jam mode (they address slot 0).
2. **`starplayer play <module> --midi <port>`** enables jam mode; the web player's
   Keyboard/MIDI toggle (E6) enables it when a module is playing.
3. **The acceptance test** in `starplayer-offline`: a module (the synthetic IT fixture)
   plus a scripted external feed whose events land exactly on tracker tick frames — the
   collision the tie-break exists for — rendered at block sizes 1, 3, 64, 128, 4096 and
   8191 on both mix paths, byte-identical; and the same render with the two sources
   inserted in the opposite slot order documented as *different* (the rule is stable, not
   order-free). The allocator hook stays clean with the mux driving.
4. **Telemetry**: the snapshot shows the module's channels and the MIDI channels 48–63
   at once; the web player's channel table draws both.
5. **Docs**: architecture §3.3's `SourceMux` row says "acceptance case landed E7";
   `plans/engine/M4-master-plan.md` exit criteria marked; `plans/README.md`.

## Research points

1. **Seek while jamming.** A seek rebases the song clock; confirm the MIDI source's own
   control tick and the queue's absolute frames need no adjustment (they are output-clock
   frames, not song frames), and say so.
2. **Voice budget.** A dense IT at 256 voices plus sixteen MIDI channels: confirm
   `recommended_voice_capacity` leaves headroom or raise it for jam mode.

## Verification

```sh
cargo test -p starplayer-offline --test jam_determinism
cargo test --workspace
cargo xtask goldens --check
cargo xtask conformance --offline          # unchanged
cargo xtask ci --job rt-safety
cargo run -p starplayer-cli -- play crates/starplayer-s3m/tests/fixtures/NICETUNE.S3M --midi <port>   # if a port exists here
cargo xtask wasm && node apps/starplayer-web/test/worklet-harness.mjs && node apps/starplayer-web/test/headless.mjs
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
```

**Do not commit** — the reviewer commits.

## Out of scope

Plugin hosting (M9), envelopes for MIDI-driven XM/IT instruments (M11), the TUI (A1).

## Research resolution

Recorded at implementation time; each answer is the decision, not a summary of the
question.

### 0. Deviations from the task file — **four**

1. **Live events remain absolute on the musical/source clock, not the output clock.** E6
   deliberately corrected the earlier `output_frame + lead` design: every `EventSource`
   dispatches against `Engine::source_frame`, which pauses with transport, so stamping on
   the always-running output clock would make notes late or unreachable after a pause. E7
   does not undo that landed correction. The seek result below is unchanged because a seek
   rebases only the sequencer's song clock, not the engine's monotonic source clock.
2. **The channel table expands to 64 while a MIDI lane is sounding, rather than merely
   because jam mode is enabled.** `EventSource` intentionally has no mode or downcast API,
   so the tracker sequencer cannot ask whether it is inside a mux. It can observe the
   actual engine state: while any live lane 48–63 owns a voice it publishes all 64 lanes;
   otherwise it publishes the module's native count. This shows both sources together
   without making an idle jam mode turn every module's display into sixty-four blank rows.
3. **`play song.mid --instruments module.it` now drives a device**, through a new
   `Player::load_smf` and `SmfSequencer::new_at`. E5 landed the `.mid` arm of `render` and
   left `play` returning an error that named E6/E7 as the missing piece; M4's exit sentence
   opens with "a MIDI file plays", so E7 is where that error stops being true. The file's
   frame zero is rebased onto the engine's already-running monotonic source clock, exactly
   as a module install rebases a sequencer's clock, so its first event is not dispatched
   late.
4. **Toggling jam mode is a seek, not a crossfade.** Neither `SourceMux` nor `Engine` has a
   per-slot command, so installing or removing the live slot means building a new mux and
   handing it over whole — which means a new `NativeSequencer`. It is rebuilt at the
   sounding song frame, so the position is kept, but the voices that were ringing are
   dropped and the module is audible again from its next *note*: on `REFLEX.S3M` that is
   about half a second. This is the behaviour a mid-song seek already has, and the worklet
   harness asserts it honestly rather than asserting the toggle is gapless. Making the
   toggle seamless needs an engine command that reaches one mux slot, which is a change to
   the source plumbing and not part of this task.

### 1. Seek while jamming — **no MIDI rebase; the seek mailbox remains in slot 0**

`Player::jam` constructs slot 0 with the existing `SeekableModuleSource`, so order, row
and elapsed-frame seeks reach exactly the same mailbox wrapper they do outside jam mode.
A seek calls `NativeSequencer::{seek_order_at,seek_frame,restart_clock_at}` and rebases the
sequencer's *song* clock at the current engine source frame. It does not rewind or jump
`Engine::source_frame`, so the `ExternalEventQueue`'s absolute musical-clock stamps and
`MidiSource`'s own synthesised control tick need no adjustment. Toggling jam mode rebuilds
the sequencer at the telemetry snapshot's sounding `song_frame` and starts both sources at
the current monotonic `source_frame`; module loads rebuild both sources, and mixer/rate
rebuilds carry the requested starting position into the single jam-mode installation.

### 2. Voice budget — **272 host slots, with IT still limited to 256 owned voices**

The previous persistent-host capacity of 256 exactly equalled IT's virtual-channel limit
and left no guaranteed slot for a live MIDI voice. Jam mode raises
`MAX_VOICE_CAPACITY` to **272**: 256 IT-owned voices plus one foreground voice for each of
the sixteen MIDI channels. Merely enlarging the pool is insufficient because MIDI may
take low IDs first and IT's parallel articulation state used to have only 256 entries.
`ItProcessor` therefore sizes its ID-indexed state for all 272 global slots while retaining
the format's 256-owned-voice limit; once 256 IT voices exist it steals according to the
existing IT policy even if the global reserve contains free slots. Thus MIDI cannot make
an IT voice untracked, and IT cannot consume the sixteen-slot jam reserve. Module-specific
offline jam engines likewise request `recommended_voice_capacity(module) + 16`; ordinary
module-only renderers keep the format's unchanged recommendation and goldens.
