# M4 — E5: The MIDI codec, the SMF parser and `SmfSequencer`

| Field | Value |
|---|---|
| Milestone | M4-full ([master plan](M4-master-plan.md)) |
| Status | Ready — not yet dispatched |
| Depends on | E4 (`Event` consumers: `InstrumentRack`, `MidiSource`, the `EventFeed` trait) |
| Blocks | E7 |
| Parallel with | E6 |
| Recommended model | Claude Sonnet (a parser and a converter against a public specification) |
| Verified by | agent (`cargo test -p starplayer-midi`, a `.mid` rendered offline byte-identically at every block size, the CLI), then reviewer, then the owner listens |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first.

Architecture §2.3: "MIDI byte parsing lives in `starplayer-midi` as a **codec** that
converts to and from `Event`. MIDI's representation never becomes the internal one."
`crates/starplayer-midi` is an empty `no_std` shell with `smf` as an optional feature and
the facade's `midi`/`smf` features already forwarding to it. Task E4 landed the consumers:
`Event`s go into an `InstrumentRack` through a `MidiSource<Feed>`, and `EventFeed` is the
small trait a feed implements (`next_frame`, `pop_due`). This task makes bytes and files
into that feed.

### The specifications

The MIDI 1.0 detailed specification (channel voice messages, running status, system
real-time bytes interleaving anywhere, system exclusive) and the Standard MIDI File
specification (header chunk, formats 0 and 1, track chunks, variable-length quantities,
delta times in ticks per quarter note or SMPTE frames, meta events — set tempo, end of
track, time signature — and running status inside tracks). Use WebFetch for the text if
needed; do not guess at edge cases.

### Code you must read before changing anything

- `crates/starplayer-core/src/event.rs` (`Event`, `TimedEvent`, `Target`, `Note`,
  `U0F16::from_bits`, `unit_from_midi7`, `bipolar_from_midi_bend`) and `fixed.rs`.
- `crates/starplayer-engine/src/instrument.rs` and `src/midi_source.rs` (E4) — what a
  feed must provide and which `Event`s the rack handles.
- `crates/starplayer-core/src/tempo.rs` and `clock.rs` — `Q32_32` and the exact
  frames-per-tick arithmetic the tempo map should mirror (no floats, no drift).
- `crates/starplayer/src/{lib,sequencer}.rs` — where `probe`/`load` and the `NativeSequencer`
  live; a `.mid` is **not** a `Module`, so it gets its own facade entry points rather than
  an arm in `load`.
- `apps/starplayer-cli/src/{main,play,render,info}.rs`; `crates/starplayer-offline/src/lib.rs`.
- `plans/product/01-technical-architecture.md` §2.3, §3.3.

## Deliverables

### 1. `starplayer_midi::codec`

`MidiDecoder` (a byte-at-a-time state machine: status, running status, data-byte
counting, real-time bytes passed through mid-message, sysex skipped) producing
`(channel: u8, Event)`; `encode(channel, &Event) -> Option<[u8; 3]>` for the messages
that round-trip. Mapping: note on/off with velocity → `U0F16` via `unit_from_midi7`
(velocity 0 note-on is `NoteOff`), CC → `Controller { number, value }`, program →
`Program(InstrumentId)`, pitch bend → `I1F15` via `bipolar_from_midi_bend`, channel and
poly aftertouch, CC120/123 → `AllSoundOff`/`AllNotesOff`. Tests from the specification's
own examples plus a running-status stream with real-time bytes inside a message.

### 2. `starplayer_midi::smf` (feature `smf`)

`parse_smf(bytes) -> Result<Smf, Error>`: header, formats 0 and 1 (format 2 →
`Unsupported`), all tracks merged into one list sorted by absolute tick with a stable
per-track order, the tempo map (`set_tempo` metas, default 120 BPM), PPQN and SMPTE
divisions, end-of-track, malformed-file rejection (a truncated VLQ, a chunk past the
end, a track without end-of-track). `Smf::to_frames(sample_rate_hz) -> Vec<TimedEvent>`
converts ticks to absolute frames through the tempo map with `Q32_32` remainder carry
(no floats), so the same file at the same rate gives the same frames on every target.
Fuzz target `fuzz/fuzz_targets/smf.rs` with synthesised seeds.

### 3. `SmfSequencer`

An `EventFeed` over the sorted `TimedEvent`s with a cursor, plus `seek_frame`, `length_frames`,
and `restart` — enough for `MidiSource<SmfSequencer>` to play a file end to end and for
the CLI to print its length.

### 4. Facade and CLI

`starplayer::midi` re-exports; `starplayer::probe_smf(bytes)`; in the CLI, `info` prints an
SMF's format, tracks, PPQN, tempo changes and length; `render`/`play` accept a `.mid` with
`--instruments <module>` (required; the rack is built from that module, program numbers
index its instruments; a clear error names the flag when it is missing), rendering through
`MidiSource<SmfSequencer>`. The web player is E6/E7's.

### 5. Proof

A hand-assembled format-1 file with two tracks, a tempo change and running status renders
byte-identically at the six block sizes through the synthetic IT fixture's instruments;
the offline allocation hook stays clean with the SMF feed.

## Research points

1. **`midly` or hand-rolled?** `midly` is `no_std`-capable; a hand-rolled parser of the
   subset above is about 400 lines and owns its error model. Prefer hand-rolled unless the
   dependency buys the SMPTE division handling for free; record the choice.
2. **Tempo map arithmetic.** Microseconds per quarter × ticks → frames without floats:
   state the formula and its worst-case rounding.
3. **Program numbers past the module's instrument count.** Clamp, wrap or silence;
   pick the one GM-playing hosts expect least surprise from, and document.

## Verification

```sh
cargo test -p starplayer-midi
cargo test -p starplayer-midi --features smf
cargo test --workspace
cargo run -p starplayer-cli -- info <some.mid>
cargo run -p starplayer-cli -- render <some.mid> --instruments crates/starplayer-s3m/tests/fixtures/NICETUNE.S3M -o /tmp/mid.wav
cargo xtask ci --job no-std-check
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
cargo xtask ci --job rt-safety
```

**Do not commit** — the reviewer commits.

## Out of scope

Live input (E6), jam mode (E7), MIDI output, General MIDI banks (M10).
