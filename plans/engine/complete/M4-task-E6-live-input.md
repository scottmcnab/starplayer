# M4 — E6: Live input — MIDI ports, Web MIDI and the keyboard map

| Field | Value |
|---|---|
| Milestone | M4-full ([master plan](M4-master-plan.md)) |
| Status | Landed 2026-09-04; owner keyboard check outstanding |
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

## Research resolution

Recorded at implementation time; each answer is the decision, not a summary of the
question.

### 0. Deviations from the task file — **six, all deliberate**

1. **The stamp is taken from the *musical* clock, not the output clock.** The task and
   master-plan decision 6 both write `output_frame + lead`. That is only correct for an
   engine that has never been paused, and a `Player` is always paused at least once:
   `Player::open` freezes the musical clock before the stream starts, so by the time
   anybody presses Play the output clock is already ahead by however long they took. The
   engine dispatches every `EventSource` against `Engine::source_frame` — the clock that
   *stops with the transport* — so an event stamped in the output clock would arrive late
   by exactly the time the transport had spent stopped, and a keyboard would fall silent
   after a long pause. `Player::send_event` therefore stamps `source_frame + lead`.
   `crates/starplayer-host/src/events.rs`'s preamble says so at the code.
2. **`midir` lives in its own crate, `starplayer-midi-native`.** Research point 1 below.
3. **The web player needs one thing the task does not name: a way to *install* the
   live-input source.** Opcode 10 carries the events, but building the `InstrumentRack`
   allocates, and design goal 5 forbids that on the render path. So there is a
   `set_midi_input(enabled)` wasm export driven by a `{ type: 'midiInput', enabled }` port
   message, handled in a worklet message task exactly as `set_mixer_mode` already is.
4. **A private MIDI decoder rides `starplayer-host` until E5 lands.** E5 is running
   concurrently and `crates/starplayer-midi` is still an empty shell in this tree, so
   `crates/starplayer-host/src/midi_decode.rs` is the subset both live paths need — the
   eight channel voice messages, running status, real-time bytes mid-message, sysex
   skipping — written against the API E5's task file states (fed a byte at a time, yielding
   `(u8, Event)`). Its module doc names the two-line change that removes it at merge, and
   both its `mod` and its `pub use` in `lib.rs` carry an `E5 supersedes this` comment.
   `starplayer-host` is the right home for the stand-in because it is the one crate both
   `starplayer-midi-native` and `starplayer-host-wasm` already depend on.
5. **The queue is created by `Player::midi_only`, not by every module load.** The task
   writes "the queue is created when a module is loaded (its rack needs the module)". The
   *rack* needs the module, which is the real constraint, and `midi_only` is the first
   moment both the module and the intention to jam exist. Creating one on every load would
   allocate a 256-event ring and sixteen bound instruments for every module anybody ever
   plays, almost all of which never see a note. E7's jam mode has the same shape available
   to it: build the rack, build the queue, put a `MidiSource` in the mux.
6. **The live-input controls are their own panel, beside the Engine one rather than inside
   it.** The task says "an input selector in the Engine panel". The Engine panel is a
   `<dl>` of read-only diagnostics — transports, quantum, memory, dropped — and it has no
   controls at all; the panels that *do* take input (Output, Mixer) are its siblings in the
   same `.lower-grid`. A "Live input" panel there matches the page's existing grammar, and
   it has four things to show (the toggle, the lead, the port selector, the key map hint)
   rather than one.

### 1. Where `midir` lives — **its own crate, `starplayer-midi-native`; the ALSA headers already there suffice**

`starplayer-host-cpal` is the crate that turns an output *device* into an `AudioBackend`,
and its own module doc scopes it to "what is genuinely platform" for output: device
enumeration and an `i16` conversion. MIDI input shares none of that machinery — no
`AudioSpec`, no `Stream`, no negotiation, no render callback — and putting it there would
mean a host on any other audio backend (a JACK one, M9's plugin host, a future embedded
host) had to depend on cpal to read a keyboard. The new crate depends on `starplayer-host`,
not on `starplayer-host-cpal`, so `crates/starplayer-host-cpal` takes it as a
**dev-dependency** for `examples/play.rs`'s `--midi` with no cycle.

**The headers.** `midir` binds `alsa-lib`'s *sequencer* interface (`snd_seq_*`); cpal binds
the same library's PCM interface. It is one `pkg-config --exists alsa`, so the
`PKG_CONFIG_PATH` this repository's `~/.cargo/config.toml` points at a user-built
`alsa-lib` prefix satisfies both, and so does `sudo apt install libasound2-dev` on CI.
Verified here: `cargo build -p starplayer-midi-native` compiles with no change to the
environment. `midir`'s own default feature set is **empty**; the manifest pins it
`default-features = false` only to say out loud that `jack` — which binds `libjack`, a real
system dependency — stays off.

**Runtime, as opposed to build time, is a different question**, and this machine answers it
the hard way: WSL2 has no `/dev/snd` at all, so `MidiInput::new` itself fails with
`MIDI support could not be initialized`. That is why `MidiError::Unavailable` is a distinct
variant from `MidiError::NoPorts`: "there is no MIDI stack here" and "there is one and
nothing is plugged in" are different sentences for `--list-midi-ports` to print, and
neither is a reason to fail a command that was not asked to play anything.

### 2. Event lead — **256 frames by default, floored at the device's own block plus a quantum**

The measurable quantity is not "scheduling jitter" but **how stale the control thread's
view of the clock is**, and it is exactly one device block by construction:
`RenderState::publish` runs once, at the end of `RenderState::render`, so while a callback
is running the value a sender reads is the frame the *previous* callback ended on. An event
stamped less than a block ahead is therefore stamped into audio that has already been
rendered. The engine tolerates that — it dispatches at the current frame and counts the
event late (E4) — but the timing is then quantised to the block instead of to the sample,
which is the whole property design goal 1 exists for.

`crates/starplayer-host/tests/live_input.rs` measures the staleness directly —
`before.frames_until(player.source_frame())` across one 4096-frame callback is exactly
4096, one publication per callback whatever its length — and reads the lead back at 128 and
at 4096. The other rows are that rule applied to each host's own block:

| host | block | staleness | effective lead | at 48 kHz | |
|---|---|---|---|---|---|
| worklet | 128 (a promise, not a preference) | 128 | **256** | **5.3 ms** | measured |
| cpal, `--buffer 1024` (the CLI default) | 1024 | 1024 | 1152 | 24 ms | by the rule |
| cpal, `--buffer 4096` | 4096 | 4096 | 4224 | 88 ms | measured |
| cpal, device default on WSLg PulseAudio | up to 96 000 | 96 000 | 96 128 | 2.0 s | by the rule, on the block size `starplayer-host`'s own `RenderState::render` documents |

So the default is master-plan decision 6's two quanta — `DEFAULT_EVENT_LEAD_FRAMES = 256` —
and `EventClock::lead_frames` returns
`max(requested, largest block seen + RENDER_QUANTUM)`. The block is **observed** through a
`fetch_max` from the callback rather than read from `AudioSpec::preferred_block_frames`,
because that field is `None` for "the device's own", which is precisely the case that can
be two seconds long. `Player::set_event_lead` raises the floor and never lowers it below
it, and `Player::event_lead()`/`event_lead_millis()` report what is actually applied — which
is what the web player prints beside its Keyboard toggle, and what `play --midi` prints on
its second line.

Two consequences worth stating plainly. On the worklet, which is the only place a keyboard
is played in this task, the lead is 5.3 ms — below the ~10 ms at which a player begins to
feel a key as late, and an order of magnitude under the browser's own output latency
(96–136 ms measured in the headless runs). On the `postMessage` fallback the page batches
commands per animation frame, so a note waits up to one frame *on top* of the lead;
architecture §9.1's Q1 resolution asks for that to be **reported** rather than hidden, and
the Live input panel appends "+ one frame batched" when the shared ring is not in use.

### 3. Keyboard layout — **the original has no note keyboard at all; the map is FT2's and IT's, and the divergence is deliberate**

`plans/reference/original-star-ui.md` §6 is the whole of StarPlayer 2.25s's keyboard, and
there is no note entry in it: `F1`–`F8` start songs, `F9` terminates, `F10` opens the popup,
`F11`/`F12` step the pattern, and the letters are mode switches (`C` cd, `D` dos shell, `Z`
video, `?` help). The original is a *player*; it never had an instrument keyboard to echo.

So the map is the one every tracker has used since Ultimate Soundtracker, and the one a
tracker musician's fingers already know — FastTracker 2's and Impulse Tracker's, which the
task file's own wording ("`Z`–`M` and `Q`–`P` rows with the sharps on the rows above")
describes exactly:

```
lower octave   sharps   S D _ G H J _ L ;        upper octave   sharps   2 3 _ 5 6 7 _ 9 0
               naturals Z X C V B N M , . /                     naturals Q W E R T Y U I O P
```

Three details, each a decision:

* **Keys are named by `KeyboardEvent.code`, not `.key`.** A tracker map is *physical* — an
  AZERTY or Dvorak player reaches for the same key positions — and `code` is the only way to
  say that.
* **Both rows run five notes past their octave** (`, L . ; /` and `I 9 O 0 P`), exactly as a
  tracker's do, so the two hands cover thirty-one semitones rather than twenty-four.
* **Octave 4 puts `Z` on MIDI note 60**, which is master-plan decision 4's reference note: a
  sample plays at its own rate there. `[` and `]` shift the octave and `-` and `=` step the
  instrument (a program change on MIDI channel 0), which is Impulse Tracker's pairing of
  those two functions onto adjacent keys.

Velocity is fixed at 100, as the deliverable specifies — a computer keyboard has none to
read. `event.repeat` is dropped, because a typematic retrigger every 30 ms is not what
holding a note means. The note-off is looked up from a `Map` keyed on the physical key, so
a key held across an octave change releases the note it *started*; a `blur` releases
everything, so a page that loses focus mid-chord does not leave notes sounding. And
`keyboardIsListening` refuses any event whose target is an `input`, `select`, `textarea` or
`contentEditable`, or that carries Ctrl/Alt/Meta — the headless harness types `Z` into the
URL field and asserts that nothing sounded.

Plan A1's TUI reuses this table.
