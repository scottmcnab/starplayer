# M4 — E5: The MIDI codec, the SMF parser and `SmfSequencer`

| Field | Value |
|---|---|
| Milestone | M4-full ([master plan](M4-master-plan.md)) |
| Status | Landed 2026-09-04 |
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

## Research resolution

Recorded at implementation time; each answer is the decision, not a summary of the
question.

### 0. Deviations from the task file — three, all small, all deliberate

1. **`starplayer-midi`'s `smf` feature now depends on `starplayer-engine`.** The task's
   "Code you must read" list and architecture §11's crate-layout table both drew this
   crate as `→ core` alone. That was written before E4 landed
   [`EventFeed`](../../../crates/starplayer-engine/src/instrument.rs), and architecture
   §3.3 itself calls `SmfSequencer` "the other `EventFeed`" — a feed can only exist where
   the trait it implements is in scope, and `EventFeed` lives in `starplayer-engine`,
   which must not depend back on this crate. So the edge has to run this way: `smf` (not
   the base codec, which needs nothing beyond `starplayer-core` and has no features at
   all) pulls in `starplayer-engine`, optional, for `EventFeed` and the
   `MIDI_CHANNEL_BASE` channel mapping ([`midi_channel`](../../../crates/starplayer-engine/src/instrument.rs))
   architecture §2.3's own SmfSequencer diagram already assumed. `starplayer-engine` is
   itself `no_std + alloc` with an empty default feature set, so design goal 4 is
   unaffected; `xtask/src/main.rs`'s `FEATURE_ENABLED_NO_STD_CHECKS` gained
   `("starplayer-midi", "smf")` alongside the existing `("starplayer-engine",
   "telemetry")` entry so the bare-metal build is checked with the feature on, not just
   off. `starplayer-midi/src/lib.rs`'s module doc and architecture §11's crate-layout
   line both record the edge; `crates/starplayer-midi/Cargo.toml` makes it optional and
   feature-gated.
2. **`apps/starplayer-cli/src/archive.rs` is untouched.** A `.mid` outside a ZIP archive
   already works with no change: `resolve_entry` returns non-ZIP bytes unchanged, so a
   bare `.mid` passes through to `info`/`render` exactly like a module does. A `.mid`
   *inside* a ZIP archive does not, and cannot without a change well outside this task's
   walls: `starplayer_archive::ArchiveEntry::format` is a
   `starplayer_model::ModuleFormat`, which has no SMF variant and must not gain one — a
   `.mid` is not a `Module` (this task's own Context section), and giving `ModuleFormat`
   a case with no sample data, no orders and no patterns would be a lowering the format
   crates' own invariants forbid. Widening `starplayer-archive` to recognise a `.mid`
   entry through some other type is real work belonging to whichever task next touches
   that crate, not a one-line fix here. Documented as a limitation rather than attempted.
3. **`play --instruments` parses and validates but does not play.** The coordinating
   task narrowed this task's touch of `play.rs` to "a new `--instruments` flag... in a
   small, clearly separated block", because `Player` (`starplayer-host`, E6's crate) has
   no entry point for a `Box<dyn EventSource>` or a `MidiSource<SmfSequencer>` today —
   only `Player::load`/`load_module`, both tracker-only. So `play`'s new block (marked
   `// ── task E5 ──` / `// ── end task E5 ──`) adds the flag, and a `.mid` fails fast
   with one of two clear messages: naming `--instruments` when it is missing, or naming
   `render` as what to use instead when it is given (`play cannot drive a .mid through a
   device yet`). Live `.mid` playback through a device is E6/E7's, once `Player` gains
   the surface for it.

### 1. `midly` or hand-rolled? — **hand-rolled**

The dependency does not buy the SMPTE division handling for free — nothing in the crate
graph reachable from this repository was already pulling in `midly`, and this environment's
crate registry access is restricted to the vendored/workspace graph (`cargo` cannot reach
`crates.io` here to add a new external dependency at all, confirmed by a `curl` to it
returning 403). Even setting that aside, `midly`'s `no_std` support is a Cargo feature this
crate would have to carry correctly through its own `smf` feature forever, for a parser
whose full surface (header, format 0/1, VLQs, running status, the three meta events this
task needs, sysex skipping) is under 400 lines against a specification with no ambiguity
left once read closely (§ "The specifications" below). Hand-rolled also means the error
model is `starplayer_core::Error` — the same enum every format loader in this repository
already returns — rather than a second error type a caller has to convert at the boundary.
`crates/starplayer-midi/src/smf.rs`'s `Cursor` mirrors the bounds-checked-read style
`starplayer-mtm/src/loader.rs`'s `Source` already established, so the parser reads like
every other loader in the tree rather than like a wrapped dependency.

### 2. Tempo map arithmetic — **the same Q32.32 remainder-carry shape `FrameClock` uses, generalised from one tick at a time to a monotonic sequence of ticks**

For a PPQN-divided file, one tick's length in Q32.32 is

```text
frames_per_tick_bits = (sample_rate_hz * micros_per_quarter_note * 2^32) / (ppqn * 1_000_000)
```

— a `u128`-widened single division, saturating at `u64::MAX`, exactly
`starplayer_core::tempo::exact_frames_per_tick`'s own shape. For an SMPTE-divided file
(which never consults the tempo map — SMPTE delta-times are already real time) it is

```text
frames_per_tick_bits = (sample_rate_hz * 2^32) / (|frames_per_second| * ticks_per_frame)
```

`Smf::to_frames` and `Smf::length_frames` both walk `TickToFrameConverter::advance_to`,
which — like `FrameClock::advance_tick` — adds `delta_ticks * frames_per_tick_bits` (widened
through `u128`, saturating) to a carried `Q32_32` remainder and takes only the whole frames
out, once per boundary (every tempo change at or before the target tick, then the target
tick itself). The generalisation from `FrameClock`'s "one tick at a time" is that a segment
here can span many ticks between two tempo changes: `Q32_32::take_whole()` after an
`N`-tick addition is the exact integer identity of taking it after `N` separate one-tick
additions (both compute `⌊total_bits / 2^32⌋` off the same running remainder), so nothing is
lost by batching. **Worst-case rounding** is therefore under one output frame at any given
tick, the same bound `FrameClock` carries, and it never accumulates across the file: two
`Smf` tests pin exact results at rates chosen so the per-tick division has no remainder at
all (`a_tempo_change_is_recorded_and_frames_reflect_it`: 96 PPQN, 500,000 µs/quarter and
44,100 Hz reduces to an exact 229.6875 frames/tick;
`smpte_division_ignores_the_tempo_map_and_uses_a_fixed_rate`: 44,100 Hz / 25 fps is exactly
1,764), so the same file at the same rate gives the same frames on every target with no
floating point anywhere in the path.

### 3. Program numbers past the module's instrument count — **stored, not clamped or wrapped: the channel sounds nothing until a valid program arrives**

Already the rack's own answer from E4 (`InstrumentRack::set_program`,
`crates/starplayer-engine/src/instrument.rs`): *"A program past the module's instrument
count is stored rather than clamped: it simply sounds nothing until a program that exists
arrives. What a General MIDI file should get instead is E5's research point 3."* That
comment is this task's own forward reference, so this is confirmation rather than a new
decision. The alternative a General MIDI-playing host might expect — wrapping or clamping
to instrument 0 — was rejected because it would make an out-of-range program *sound
something*, silently substituting whichever instrument happens to sit at index 0 (or
`count - 1`) for a program the loaded module never claimed to have; a channel that goes
silent until its own program arrives is a legible, debuggable failure (`InstrumentRack`'s
`unsupported_events`/`held_notes` surface still reports what the channel is doing), where a
substituted instrument would sound plausible and be wrong. There is no General MIDI bank
until M10 (out of scope, master-plan decision 5), so "least surprise" here means matching
what every host in this repository already does for a program with no instrument behind
it, not matching a GM synthesiser's own fallback behaviour, which this build has nothing
to fall back to.
