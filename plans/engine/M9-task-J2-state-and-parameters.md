# M9 — J2: Plugin state, parameters and the validator

| Field | Value |
|---|---|
| Milestone | M9 ([master plan](M9-master-plan.md), "The task graph" section) |
| Status | Planned; pull-driven (not scheduled) |
| Depends on | J1 (the plugin), M7-H7 (`Player::inserts` / `InsertLayout`) |
| Blocks | J4; the M9 exit criterion ("responds to host transport and MIDI" needs a module in the project) |
| Parallel with | — |
| Recommended model | Claude Sonnet (serialisation and a parameter table over an existing `Player` surface) |
| Verified by | agent (state round-trip tests, `clap-validator` state and param checks), then owner in a DAW |

## Context for a fresh agent

J1 landed `starplayer-clap`: a `Player` over a `ClapBackend`, loading its module from an
environment variable. A plugin that forgets its module when the project is reopened is a
demo, not an instrument. This task gives it state and parameters.

Master-plan decision 4 settles the central question: **state embeds the module's original
bytes**. Read the decision; the short version is that a DAW project must be self-contained,
and the original file (not the decoded `Module`) is the smallest faithful thing to embed —
the loader is deterministic, so decoding again on load yields the same `Module`.

### Code you must read before changing anything

- `crates/starplayer-clap/src/*` (J1) and its `README.md`.
- `crates/starplayer-host/src/player.rs` — everything a `Player` exposes that a project
  should remember: `mixer_mode`, `at_end`, `set_master_volume`, `mute`, `midi_only`/`jam`,
  `inserts` (H7), `set_insert_param`.
- `crates/starplayer-engine/src/mixer_mode.rs` — `MixerMode::to_wire`/`from_wire`, the
  stable `u32` encoding to reuse rather than reinvent.
- `crates/starplayer-dsp/src/insert.rs` — `InsertDescriptor`/`ParamSpec`, the source of the
  per-insert parameter list.
- The `clack-extensions` `state` and `params` modules and CLAP's `state`, `params`,
  `preset-load` extension docs.

## Deliverables

### 1. State (`clap.state`)

A versioned little-endian binary blob (write a tiny `StateWriter`/`StateReader`; no
`serde`, no new dependency):

```
magic "SPCL", version u16
mode u8, master_volume u16 (U0F16 bits), at_end u8, fade_frames u32
mixer_mode u32 (MixerMode::to_wire)
mutes: u64 bitmask over 64 channels
inserts: count u8, then per insert { target u8, slot u8, kind u8, param_count u8, params: [i32] }
module: origin_path (u16 len + utf8, may be empty), bytes (u32 len + bytes)
```

`save` writes what the `Player` reports; `load` (main thread) parses, `Player::load`s the
bytes, applies mode, mutes, volume, mixer mode and inserts in that order, and stores the
origin path as a hint only. A blob with no module is valid (a plugin instance that never
loaded one). Unknown future versions are refused with a clear host-visible error; the
same version with trailing bytes is accepted (forward-compatible appends).

### 2. Parameters (`clap.params`)

A fixed table, ids stable forever (append-only):

| id | name | range | maps to |
|---|---|---|---|
| 0 | Mode | 0 Module / 1 Instrument / 2 Jam (stepped) | `midi_only` / `jam` |
| 1 | Master volume | 0.0–1.0 | `set_master_volume` |
| 2 | Repeat | 0 Fade / 1 Continue / 2 Stop (stepped) | `set_at_end` |
| 3 | Interpolator | 0 nearest / 1 linear / 2 cubic / 3 sinc (stepped) | `set_mixer_mode` (a rebuild — flagged `CLAP_PARAM_REQUIRES_PROCESS` off and applied on the main thread) |
| 4 | Bend range | 0–2400 cents | rack bend range |
| 100–163 | Mute ch 1–64 | 0/1 (stepped) | `mute` |
| 1000 + 256·t + 16·s + p | Insert `t`/slot `s`/param `p` | the `ParamSpec` range | `set_insert_param` |

Parameter events arriving in `process()` are applied on the audio thread only where the
`Player` call is RT-safe (volume, mutes, insert params, bend range); mode, repeat and
interpolator are deferred to `on_main_thread`. Names come from the descriptors, so a host
shows "Reverb: Room size" once an insert is installed; before that the slot's parameters
are hidden (`CLAP_PARAM_IS_HIDDEN`) and the plugin calls `params.rescan(CLAP_PARAM_RESCAN_ALL)`
when the layout changes.

### 3. Loading a module in a host

`preset-load` (CLAP 1.2): a preset *is* a module file — `load_from_location(file, …)`
reads the bytes and `Player::load`s them. This replaces J1's environment variable, which
stays as a fallback for headless testing. A module that fails to load reports through the
host log and leaves the previous one playing (`a_bad_load_leaves_the_previous_module_playing`
already proves the `Player` side).

### 4. Proof

- State round-trip test: save → load into a fresh instance → save again is byte-identical,
  for an instance with a module, inserts and mutes, and for an empty one.
- A block rendered after `load` equals a block rendered by a `Player` configured the same
  way by hand.
- `clap-validator` passes its state and params suites (it saves, reloads and compares).
- Parameter → `Player` mapping test for every row of the table.

### 5. Documentation

Crate README: the state format, the parameter ids and what is main-thread-only.
Architecture §11 row updated. Append `## Research resolution`.

## Research points

1. `preset-load` availability in `clack-extensions` at the pinned version; if absent,
   implement the raw extension in the FFI module or fall back to a file-path parameter
   (a string parameter does not exist in CLAP — say what you did).
2. Whether a 64-entry mute table as parameters is what hosts want or noise; consider
   exposing only the loaded module's channel count via `CLAP_PARAM_IS_HIDDEN` on the rest
   and a rescan on load.
3. State size: the largest module in the corpus, and whether hosts balk at multi-megabyte
   state (most do not; REAPER stores it in the project file — say what you found).

## Verification

```
cargo test -p starplayer-clap
cargo xtask clap
clap-validator validate target/clap/starplayer.clap
cargo xtask ci --job clap-build
cargo xtask ci --job clippy
```

## Out of scope

A GUI; tempo-follow; per-note expressions; VST3 (J4).
