# M4 — E1: The XM and IT instrument and sample model

| Field | Value |
|---|---|
| Milestone | M4-lite ([master plan](M4-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | M2 complete |
| Blocks | F1, F2 (XM loader and runtime), G1, G3 (IT loader and runtime); M11's `InstrumentBank` |
| Parallel with | E2, D6, D7 |
| Recommended model | GPT-5.6-sol or Claude Sonnet (model crate only; no RT path) |
| Verified by | agent (`cargo test --workspace`, `cargo xtask goldens --check`, `cargo xtask ci --job no-std-check`, `--job clippy`, `--job conformance --offline`), then reviewer |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there (sample-exact ticks, whole-quantum DSP, buffer-size-independent
output, no allocation/locks/panics in `render()`, format identity) are non-negotiable.

`crates/starplayer-model` is the shared, format-neutral module model: a `Module` is one
PCM blob, one pattern-bytes blob, and index tables (architecture §6). Task M1-B1 declared
the XM/IT instrument concepts — `InstrumentDef`, `Envelope`, `NewNoteAction`,
`DuplicateCheck`, `DuplicateAction`, a 120-entry note→sample map, `fadeout` — as **data
with no behaviour**, so that adding XM (M5) and IT (M6) would be "filling fields in rather
than reshaping the model". This task fills in every field those two formats need, **in one
change designed against both specifications at once**, so that the M5 and M6 streams,
which will run concurrently, never fight over the struct.

Nothing in this task plays anything. The engine, mixer and the three existing format
crates must produce byte-identical output afterwards: the goldens and the conformance
results are the proof.

### The two specifications you are designing against

- **XM** (FastTracker 2): `xm.txt` (the FT2 file-format document) and OpenMPT's
  `Load_xm.cpp`. Instruments own up to 16 samples, a 96-entry note→sample map, volume and
  panning envelopes of up to 12 points with a single *sustain point* and a loop, a
  `volume_fadeout`, and **per-instrument** auto-vibrato (type, sweep, depth, rate) that FT2
  applies to every sample of the instrument. Samples carry `relative_note` (signed
  semitones) and `finetune` (signed, in 1/128 semitone), a default pan (0..255), loop type
  (none / forward / ping-pong) and 8- or 16-bit delta-encoded PCM.
- **IT** (Impulse Tracker 2.14): `ITTECH.TXT` and OpenMPT's `Load_it.cpp` / `ITTools.cpp`.
  Instruments own a 120-entry map of `(note, sample)` pairs — so a note can be *transposed*
  as well as routed to a sample — volume, panning and pitch envelopes of up to 25 points
  with a **sustain loop** and a loop, `carry` flags, fadeout, NNA/DCT/DCA, global volume,
  default pan (with an "enabled" bit), pitch-pan separation and centre, random volume and
  pan variation, initial filter cutoff and resonance (each with an "enabled" bit), and a
  flag making the pitch envelope act as a filter envelope. Samples carry `C5Speed` (a
  reference rate in Hz), default pan (with an "enabled" bit), **per-sample** auto-vibrato,
  a normal loop and a separate **sustain loop** (each forward or ping-pong), 8/16-bit,
  optionally stereo, optionally IT 2.14-compressed PCM.

Everything both formats express must fit the shared model; anything only one format's
effect processor reads stays in that format's crate or in the opaque `format_data` this
task adds. Do not add behaviour, only representation.

### Code you must read before changing anything

- `crates/starplayer-model/src/instrument.rs` — `InstrumentDef`, `Envelope`,
  `EnvelopePoint`, `EnvelopeSpan`, the NNA triple, `NOTE_MAP_LENGTH`, `from_sample`.
- `crates/starplayer-model/src/sample.rs` — `SampleSpec`, `SampleIndex`, `LoopMode`,
  `DEFAULT_REFERENCE_RATE_HZ`; note the doc on `SampleIndex` describing the guard-frame
  layout and the deliberate absence of a `SampleRegion` constructor.
- `crates/starplayer-model/src/builder.rs` — `ModuleBuilder::add_sample` (the stored-frames
  rule and guard-frame fill), `add_instrument`, `set_header`, `build`.
- `crates/starplayer-model/src/module.rs` — `Module`, `validate`, `validate_sample`.
- `crates/starplayer-model/src/header.rs` — `ModuleHeader`, `ModuleFlags`, `format_extra`.
- `crates/starplayer-core/src/sample.rs` — `GUARD_FRAMES` and its rationale table.
- `crates/starplayer-mixer/src/sample.rs` — `append_guarded_sample`, `LoopSpan::ping_pong_frame`,
  `SampleRegion::looping`. **The mixer's guard layout is the contract**; the builder must
  agree with it, not the other way round.
- `crates/starplayer-mixer/src/kernel.rs` — `fold_ping_pong`, `wrap_forward`,
  `normalise_position`: how the kernel reads a loop, so you know which guard frames are
  actually read.
- `crates/starplayer-s3m/src/processor.rs` — `sample_region` and `flush_channel`: how a
  format turns `SampleIndex` into a `SampleRegion` today.
- `crates/starplayer-mod/src/processor.rs` — `waveform_value` and `WAVEFORM_RANDOM_SEED`:
  the fixed-seed xorshift32 you are lifting into core (accuracy policy **D11**).
- Every constructor of `SampleSpec` and `InstrumentDef` in `crates/starplayer-{s3m,mod,mtm}/src/`
  and in `crates/starplayer-offline/src/fixtures.rs` — they must keep compiling, preferably
  untouched, through `..Default::default()` / the existing helper constructors.
- `plans/product/01-technical-architecture.md` §6 (module memory layout) and
  `plans/product/03-accuracy-policy.md` §3 (D7, D11).

## Deliverables

### 1. `SampleSpec` / `SampleIndex` — per-sample fields for XM and IT

Add to `SampleSpec`, with matching accessors on `SampleIndex`:

```rust
/// XM: signed semitone offset added to the note before pitch is derived.
pub relative_note: i8,
/// XM: signed finetune in 1/128 semitone. IT expresses tuning through `reference_rate_hz` instead.
pub finetune: i8,
/// The sample's own default pan, or `None` when the file does not enable one.
pub default_pan: Option<I1F15>,
/// Per-sample auto-vibrato. XM stores it per instrument and applies it to every sample; the XM loader copies it down.
pub auto_vibrato: AutoVibrato,
/// IT's sustain loop, played instead of the normal loop until the note is released.
pub sustain_loop: Option<SustainLoop>,
```

with

```rust
pub enum AutoVibratoWaveform { Sine, RampDown, Square, Random }
pub struct AutoVibrato { pub waveform: AutoVibratoWaveform, pub sweep: u8, pub depth: u8, pub rate: u8 }  // source-format units, like `fadeout`
pub struct SustainLoop { pub mode: LoopMode /* Forward | PingPong only */, pub start: u32, pub end: u32 }
```

Keep `relative_note` and `finetune` **raw**. XM's tuning is applied in linear or Amiga
mode *after* the note is known, so deriving a Hz rate at load would be lossy in Amiga mode;
the XM loader passes `DEFAULT_REFERENCE_RATE_HZ` as the nominal rate. IT's `C5Speed` is a
real rate and maps straight to `reference_rate_hz`. Say so in the field docs.

`SampleSpec::one_shot` and `Default` give every new field its neutral value
(`0`, `0`, `None`, `AutoVibrato::default()` with depth 0, `None`), so no existing loader
changes.

### 2. `ModuleBuilder::add_sample` — sustain loops and ping-pong guard frames

The stored-frames rule becomes:

- no sustain loop and `LoopMode::Forward` → store `0..loop_end`, guard = wrapped loop copy
  (unchanged);
- no sustain loop and `LoopMode::PingPong` → store `0..loop_end`, guard = the reflected
  continuation, computed with the **same** arithmetic as
  `starplayer_mixer::LoopSpan::ping_pong_frame` (reflect about `loop_end - 1`). The mixer
  is the contract; write a test that builds one sample both ways and asserts the two guard
  slices are identical;
- any sample with a sustain loop → store the whole sample (the normal loop and the sustain
  loop may lie anywhere inside it), guard = zeroed. Playback swaps the voice's
  `SampleRegion` from the sustain-loop region to the normal region on key-off, and neither
  region's loop end is at the stored end, so the guard is never a loop continuation. One
  frame of `Linear` interpolation reads real PCM past a loop end instead of the wrapped
  copy; record that as accepted in the field docs and leave it for M7's kernels;
- `LoopMode::None` → unchanged.

Validation (`add_sample` and `Module::validate`): a sustain loop needs
`start < end <= source_frames` and a looping `mode`; the existing "a forward loop stores
exactly `loop_end` frames" rule applies **only when there is no sustain loop**, and
otherwise becomes "stored frames ≥ `max(loop_end, sustain_end)`". Update the `LoopMode`
docs, which still say the mixer implements forward loops only.

### 3. `Envelope` — signed values

`EnvelopePoint.value: i16`. Volume envelopes stay 0..64; IT pan and pitch envelopes are
−32..32. Document that XM's single sustain *point* is an `EnvelopeSpan` with
`start == end`, so `Envelope` needs no new field for it.

### 4. `InstrumentDef` — the IT and XM instrument fields

```rust
/// Sample for each of the 120 notes: a one-based global `SampleId` (0 = none). `u16` because XM allows 128 × 16 samples.
pub note_sample_map: [u16; NOTE_MAP_LENGTH],
/// Note actually played for each of the 120 notes (IT). Identity for every other format.
pub note_transpose_map: [u8; NOTE_MAP_LENGTH],
pub global_volume: U0F16,                 // IT; `U0F16::MAX` elsewhere
pub default_pan: Option<I1F15>,           // IT, with its enable bit
pub pitch_pan_separation: i8,             // IT, -32..32
pub pitch_pan_centre: u8,                 // IT, a note
pub random_volume_variation: u8,          // IT, 0..100 (percent)
pub random_pan_variation: u8,             // IT, 0..64
pub initial_filter_cutoff: Option<u8>,    // IT, 0..127, with its enable bit
pub initial_filter_resonance: Option<u8>, // IT, 0..127, with its enable bit
pub pitch_envelope_is_filter: bool,       // IT
```

`Default` gives the identity transpose map (`0..120`) and neutral values everywhere else;
`from_sample` is unchanged in signature. `Module::validate` rejects a `note_sample_map`
entry naming a sample past `samples.len()` — a fuzzed XM must not get through `build()`
with a dangling map entry.

### 5. `ModuleHeader` — per-channel volume and an opaque format blob

```rust
/// Default volume per channel (IT `ChnVol`). Empty means unity everywhere; otherwise exactly `channel_count` long.
pub default_channel_volume: Box<[U0F16]>,
/// Format-owned bytes the engine never interprets: IT MIDI macro configuration, channel surround/disabled flags, and whatever else a format's processor needs beyond `format_extra`.
pub format_data: Box<[u8]>,
```

`ModuleHeader::new` leaves both empty; `Module::validate` applies the same length rule to
`default_channel_volume` as to `default_pan`. Add `ModuleHeader::channel_volume(channel)`
mirroring `channel_pan`.

### 6. `starplayer_core::random::Xorshift32`

Lift the MOD processor's fixed-seed xorshift32 into `starplayer-core` as
`pub struct Xorshift32 { state: u32 }` with `new(seed)` (a zero seed becomes 1, as the MOD
code does today) and `next_u32()`. The MOD processor uses it, keeping `WAVEFORM_RANDOM_SEED`
where it is and producing the **same bit sequence** — the existing MOD tests and goldens
prove it. IT's random volume and pan variation (G3) will consume it on note-on only, so
the stream stays block-size independent.

### 7. Documentation

Update `plans/product/01-technical-architecture.md` §6 (the `Module` listing now names
`format_data` and the sustain loop) and add a short "sample sustain loops" note to
`crates/starplayer-model/src/lib.rs`'s crate docs. Nothing in the accuracy policy changes:
no behaviour changed.

## Research points

1. **Auto-vibrato units.** XM's `vibrato_sweep` is the number of ticks to reach full depth;
   IT's `VibratoSweep` is the rate at which depth ramps in. Confirm from `xm.txt` and
   `ITTECH.TXT` that a single `u8` per field in "source units" is sufficient and document
   the meaning per format in the field docs, exactly as `fadeout` already does. Do not
   normalise them.
2. **IT `C5Speed` above `u32`?** No — it is a `u32` in the file. Confirm and note it.
3. **Note map width.** Confirm the XM maximum (128 instruments × 16 samples = 2048) exceeds
   `u8` and that IT's global sample numbers (≤ 255 in an `.it`, more in OpenMPT's `.mptm`,
   which is out of scope) fit `u16`.
4. **Ping-pong guard arithmetic.** Read `LoopSpan::ping_pong_frame` and `fold_ping_pong`
   and record which guard frames the `Linear` kernel can actually read for a ping-pong loop
   (the critique of the concurrency plan believes none, with weight zero). Write the
   builder to match the mixer regardless, because M7's `Cubic` and `Sinc` kernels will read
   them.

## Verification

```sh
cargo test --workspace
cargo xtask goldens --check                    # byte-identical: no behaviour changed
cargo xtask conformance --offline              # 33 of 47, identical to before (fetch first if the cache is cold)
cargo xtask ci --job no-std-check
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
cargo test -p starplayer-offline --test fuzz_seeds
```

Add unit tests in `starplayer-model` for: the ping-pong guard equals the mixer's; a sustain
loop past the normal loop stores the whole sample and validates; a dangling
`note_sample_map` entry is rejected; `default_channel_volume` length rule; `Xorshift32`
reproduces the first eight values the MOD processor produced before the move (take them
from a test written *before* you move the code).

Report the exact commands run and their results. **Do not commit** — the reviewer commits.

## Out of scope

Reading any of these fields. No envelope runs, no sustain loop is played, no filter exists.
XM and IT loaders (F1, G1). The linear-frequency table (E2). Voice lifecycle (E3).
