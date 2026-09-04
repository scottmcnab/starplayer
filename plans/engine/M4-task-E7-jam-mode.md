# M4 — E7: Jam mode — a module and live input through one `SourceMux`

| Field | Value |
|---|---|
| Milestone | M4-full ([master plan](M4-master-plan.md)) |
| Status | Ready — not yet dispatched |
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
