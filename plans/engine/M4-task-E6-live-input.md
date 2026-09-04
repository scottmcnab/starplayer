# M4 — E6: Live input — MIDI ports, Web MIDI and the keyboard map

| Field | Value |
|---|---|
| Milestone | M4-full ([master plan](M4-master-plan.md)) |
| Status | Ready — not yet dispatched |
| Depends on | E4 (`ExternalEventQueue`, `MidiSource`, `InstrumentRack`) |
| Blocks | E7 |
| Parallel with | E5 |
| Recommended model | Claude Opus (both hosts' live paths; the audio-thread boundary) |
| Verified by | agent (host tests, the Node harnesses, a `midir` virtual port test), then reviewer, then the owner plays a keyboard |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine; hosts are `std`. Read
`AGENTS.md` first — no allocation, locks or panics on the audio thread, and the worklet's
`process()` is that thread.

Task E4 landed the engine side: `ExternalEventQueue` (an SPSC of `TimedEvent` with
absolute frames), `MidiSource` and the `InstrumentRack` bound to a module's instruments.
This task feeds the queue from the world: a MIDI port on native hosts, Web MIDI in the
browser, and a tracker-style computer keyboard in the web player. Architecture §3.2:
"pre-materialised timestamped event lists are exactly the right shape *at the edge*";
§9.1's Q1 resolution: "the ring is where MIDI input … [has] to arrive; the fallback is not
a place they can live" — on the `postMessage` fallback, events still cross, batched per
animation frame, with the added latency reported.

### Code you must read before changing anything

- `crates/starplayer-host/src/{player,engine,source,transport,backend}.rs` — `Player`,
  its command queue, `output_frame()`, the seek mailbox, how the render callback reaches
  the engine; `crates/starplayer-host-cpal/src/lib.rs` and `examples/play.rs`;
  `crates/starplayer-host-wasm/src/{lib,backend,command}.rs` — the wire opcodes 1–9, the
  `SharedArrayBuffer` command ring, `drain_commands`, the fallback batching.
- `apps/starplayer-web/www/{ring.js,worklet-processor.js,app.js,index.html}` and
  `apps/starplayer-web/test/*.mjs`.
- `apps/starplayer-cli/src/{play,main}.rs`.
- `crates/starplayer-engine/src/midi_source.rs`, `instrument.rs` (E4).
- `plans/reference/original-star-ui.md` (the original's key map, for the octave layout
  the tracker keyboard should echo); `plans/apps/A1-master-plan.md` (the TUI reuses this
  map later).

## Deliverables

### 1. `Player::send_event`

`Player::send_event(&mut self, channel: u8, event: Event) -> Result<(), HostError>` stamps
`TimedEvent { frame: output_frame + lead, target: Channel(48 + channel), event }` with a
configurable `lead` (default two `RENDER_QUANTUM`s) and pushes it on the
`ExternalEventQueue` producer; `Player::set_event_lead`; the queue is created when a module
is loaded (its rack needs the module) and installed as the engine's source in E7's jam mode
— here, `Player::midi_only()` installs a `MidiSource` alone so a keyboard can play a
module's instruments without the module playing. A full queue rejects with a counted
error, never blocks.

### 2. Native MIDI ports

`midir` pinned in `[workspace.dependencies]`; `starplayer-host-cpal` (or a sibling
`starplayer-midi-native` if `midir` should not ride the audio crate — research point 1)
lists input ports and opens one, decoding bytes with `starplayer_midi::MidiDecoder` (E5
lands the codec concurrently; if it has not landed, write the decoder call against the
E5 task file's stated API and note it) on `midir`'s callback thread and calling
`send_event`. `starplayer play <module> --midi <port>` and `--list-midi-ports` in the CLI
and the cpal example.

### 3. Web MIDI and the keyboard map

- A new ring opcode `OPCODE_MIDI_EVENT = 10`: `argument` packs status, data1, data2;
  `extra` is unused. The worklet decodes it into an `Event` and calls the same
  `send_event` path. On the fallback transport the page batches events per animation
  frame like other commands.
- `navigator.requestMIDIAccess` on the page, an input selector in the Engine panel,
  messages forwarded as-is.
- A tracker-style keyboard: two octaves (`Z`–`M` and `Q`–`P` rows with the sharps on the
  rows above, as the original's map), octave up/down, instrument next/previous, velocity
  fixed at 100; key repeat ignored; focus rules so typing in inputs does not play notes.
  A "Keyboard" toggle in the Engine panel and an on-screen hint.
- Latency reported: the lead in frames and milliseconds shown next to the toggle.

### 4. Proof

`starplayer-host` tests: `send_event` stamps `output_frame + lead`, a full queue reports
rather than blocks. A `midir` virtual-port test (skipped when the platform cannot create
one) round-trips a note. The Node worklet harness pushes opcode 10 records on both
transports and asserts the note sounds (a non-zero peak from a silent module) and that
`process()` still allocates nothing on the shared-memory path.

## Research points

1. **Where `midir` lives.** It is a std dependency with platform backends (ALSA on
   Linux — the same header story as cpal); decide whether it rides `starplayer-host-cpal`
   or its own crate, and whether CI's ALSA headers suffice.
2. **Event lead.** Measure the worklet's and cpal's actual scheduling slack; choose the
   default lead so a keyboard feels immediate without late events.
3. **Keyboard layout.** Confirm the original's map from `original-star-ui.md` and note
   any divergence.

## Verification

```sh
cargo test -p starplayer-host -p starplayer-host-cpal -p starplayer-host-wasm
cargo run -p starplayer-cli -- play --list-midi-ports
cargo run -p starplayer-cli -- play crates/starplayer-s3m/tests/fixtures/NICETUNE.S3M --midi <port>   # if a port exists here; else report
cargo xtask wasm && node apps/starplayer-web/test/worklet-harness.mjs && node apps/starplayer-web/test/ring-harness.mjs && node apps/starplayer-web/test/headless.mjs
cargo test --workspace
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
cargo xtask ci --job wasm-build
```

**Do not commit** — the reviewer commits.

## Out of scope

Playing a module and jamming at once (E7). MIDI output. The TUI's keyboard (A1).
