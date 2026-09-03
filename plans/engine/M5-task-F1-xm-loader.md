# M5 — F1: The XM loader

| Field | Value |
|---|---|
| Milestone | M5 ([master plan](M5-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | M4-lite landed (E1–E3) |
| Blocks | F2 (XM instrument runtime), F3 (effects), F4 (wiring) |
| Parallel with | D3, D6, G1 |
| Recommended model | Claude Opus (a loader that must survive fuzzing and 141 corpus files) |
| Verified by | agent (`cargo test -p starplayer-xm`, the corpus load test, fuzz seeds, `cargo xtask ci --job no-std-check`, `--job clippy`), then reviewer |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there are non-negotiable, and two matter most here: **format identity**
(an XM keeps its native pattern data and gets its own effect processor; nothing is lowered
into S3M) and **a loader either produces a valid `Module` or an `Err`, never a panic**.

`crates/starplayer-xm` is an empty shell with its manifest, feature table and `no_std`
enforcement already in place. This task makes it a loader. The effect processor (F3), the
instrument runtime (F2) and the facade/harness wiring (F4) are separate tasks; **do not
touch `crates/starplayer/src/lib.rs`** — task D3 is refactoring it concurrently.

The model already has every field an XM needs (task E1): `SampleSpec` with
`relative_note`, `finetune`, `default_pan`, `auto_vibrato`, ping-pong loops with correct
guard frames; `InstrumentDef` with a `u16` global-id `note_sample_map`, volume and panning
`Envelope`s, `fadeout`; `ModuleFlags::linear_slides`; `FormatDialect::{FastTracker2,
MilkyTracker, ModPlugXm, OpenMptXm}` (task E2).

### The specification

FastTracker 2's `xm.txt` (the format document shipped with FT2, mirrored widely — for
example under `ftp.modland.com/pub/documents/format_documentation/`), read together with
OpenMPT's `soundlib/Load_xm.cpp` and `soundlib/XMTools.{h,cpp}` on GitHub
(`OpenMPT/openmpt`), which document every real-world deviation from the spec. Use WebFetch
to read them; do not work from memory.

### The template

`crates/starplayer-s3m` is the shape to copy: `header.rs` (parse + validate),
`pattern.rs` (the cell type and a display-only `PatternView`), `sample.rs`, `loader.rs`
(`probe`, `probe_reader`, `load`, `load_from`, all over `ModuleReader`, with a decoded
pattern budget so a small file cannot declare gigabytes), `lib.rs` re-exports. Read its
`loader.rs` docs on clamp-or-reject decisions and copy that discipline.

### Code you must read before changing anything

- `crates/starplayer-s3m/src/{loader,header,pattern,sample,lib}.rs` — the template.
- `crates/starplayer-model/src/{builder,sample,instrument,header,pattern,reader}.rs` —
  `ModuleBuilder`, `SampleSpec`, `InstrumentDef`, `ModuleHeader`, `EffectNames`,
  `ModuleReader`.
- `crates/starplayer-engine/src/sequencer.rs` — `PatternData` (`row_bytes` must return one
  fixed-stride slice per row; F3's processor reads it).
- `crates/starplayer-core/src/quirks.rs` — the XM `FormatDialect` variants and their
  detection docs.
- `fuzz/` — `Cargo.toml`, `src/lib.rs`, `fuzz_targets/s3m_loader.rs`,
  `fuzz_targets/s3m_structured.rs`, `seeds/s3m/`, `dictionaries/tracker.dict`;
  `crates/starplayer-offline/tests/fuzz_seeds.rs` (replays committed seeds on stable).
- `crates/starplayer-offline/tests/render_allocation.rs` — how a test walks the pinned
  corpus at `target/conformance/corpora/libxmp-*/test-dev` and skips when it is absent.
- `xtask/src/main.rs` — `NO_STD_CRATES` (already lists `starplayer-xm`), the fuzz job.

## Deliverables

### 1. `crates/starplayer-xm` loader

- **`probe`**: the 17-byte `Extended Module: ` magic and `0x1A` at offset 37.
- **Header**: tracker name (20 bytes → `FormatDialect` per E2's rules), version (`0x0104`
  and later; `0x0102`/`0x0103` see research point 1), header size (honoured: skip what
  you do not read), song length, restart position, channel count (1..=64 — FT2 writes
  2..=32, OpenMPT up to 64; reject above `ChannelTable::MAX_CHANNELS`), pattern count
  (≤256), instrument count (≤128), flags bit 0 → `ModuleFlags::linear_slides`, default
  speed and BPM, the 256-byte order table (only `song_length` entries used; an order past
  the pattern count maps to `ORDER_MARKER`-style skipping per research point 2).
  `title`, `initial_speed`, `initial_tempo`, `format = ModuleFormat::Xm`, `dialect`.
  `default_pan` empty (XM channels start centred; instrument/sample pans apply at
  note-on).
- **Patterns**: per pattern a header (length, packing type, rows 1..=256, packed size),
  then packed cells: a byte with bit 7 set is a mask (`note`, `instrument`, `volume`,
  `effect`, `parameter` present bits), otherwise the byte is the note and all five fields
  follow. Unpack every pattern into the blob as **fixed 5-byte cells** per channel per
  row, `[note, instrument, volume, effect, parameter]`, raw XM byte values (`note` 0 =
  none, 1..=96 = C-0..B-7, 97 = key off). A packed size of zero is an all-empty pattern
  of `rows` rows. A pattern named by no order still loads. Budget the decoded size as
  S3M does.
- **Instruments**: header (size, name, type, sample count); when the count is non-zero
  the extended header (sample-header size, 96-entry note→sample map, 12 volume and 12
  panning envelope points, counts, sustain/loop points, types, vibrato type/sweep/depth/
  rate, fadeout). Envelope `type` bit 0 = on, bit 1 = sustain, bit 2 = loop; sustain is
  an `EnvelopeSpan { start: s, end: s }`. Points are `(tick, value)`; XM panning envelope
  values are `0..64` (unsigned; centre 32). `fadeout` raw. Then `sample_count` sample
  headers (length, loop start, loop length, volume, finetune `i8`, type — bits 0–1 loop
  kind, bit 4 16-bit, bit 5 stereo (research point 3) — pan, relative note `i8`, name),
  then the delta-encoded PCM for each in order. Map XM sample `k` of instrument `i` to the
  global `SampleId` the builder returns; `note_sample_map[n] = id + 1` for `n` in
  `0..96`, zero above. `InstrumentDef::sample = None`; an instrument with zero samples is
  a valid, silent instrument (`ft2_*.xm` fixtures exercise it). The instrument's vibrato
  quartet is copied to **each of its samples'** `auto_vibrato`; XM vibrato types are
  0 sine, 1 square, 2 ramp down, 3 ramp up — add `AutoVibratoWaveform::RampUp` to the model
  (the one model change in this task).
- **Samples**: 8-bit delta → `i16 << 8`; 16-bit delta → `i16`; lengths and loop points
  in bytes halved for 16-bit; loop kind 0 none / 1 forward / 2 ping-pong; a loop of zero
  length is no loop; `volume` 0..64 → `U0F16`; `pan` 0..255 → `Some(I1F15)`;
  `reference_rate_hz = DEFAULT_REFERENCE_RATE_HZ` with `relative_note` and `finetune`
  raw (E1 decided XM never round-trips pitch through Hz). Instrument and sample header
  sizes are honoured, not assumed: OpenMPT writes 263-byte instrument headers, other
  trackers other sizes.
- **Trailing data** (OpenMPT's `text`, `MIDI`, `XTPM` chunks and the ModPlug
  `ADPCM4`-compressed samples): everything after the last sample is ignored; an `ADPCM4`
  sample is `Error::Unsupported("ADPCM-compressed XM sample")` (research point 4).
- **Clamp-or-reject table** in the crate docs, as S3M's, listing every field and what an
  out-of-range value does.

### 2. `pattern.rs` — the cell and the display view

`XmCell { note, instrument, volume, effect, parameter }` with `from_bytes`, tracker
notation helpers, and `PatternView` returning display-only `PatternCell`s. Add
`EffectNames::XM` to `crates/starplayer-model/src/pattern.rs` (the English names of
`0xy`..`Xxy` plus the volume-column effects) — the second model change, a table only.

### 3. `XmPatternData(pub Arc<Module>)` implementing `PatternData`

`row_bytes` returns the `channel_count × 5` slice of a row. Nothing else of the engine
seam is implemented here (F3 does the processor).

### 4. Fuzzing

`fuzz/fuzz_targets/xm_loader.rs` and `xm_structured.rs` in the shape of the S3M ones;
synthesised seeds under `fuzz/seeds/xm/` (a minimal one-instrument file, a 16-bit
ping-pong one, a zero-sample instrument, an empty pattern, a header with the maximum
counts); the `Mutation` walker gains XM cells; `fuzz/regressions/xm/README.md`;
`crates/starplayer-offline/tests/fuzz_seeds.rs` replays the XM seeds; `xtask`'s fuzz
job lists the two new targets. No corpus binaries are committed.

### 5. The corpus load test

`crates/starplayer-xm/tests/corpus.rs`: walk every `*.xm` under the pinned libxmp
`test-dev` (skip with a message if `target/conformance` is absent, as
`render_allocation.rs` does). Every file under `data/`, `data/m/`, `data/p/` and
`openmpt/xm/` must load `Ok`; every `data/f/load_xm_*` must return `Err` **or** `Ok`
without panicking (they are libxmp's fuzz regressions); print a table of counts and the
`FormatDialect` distribution. Fetch the corpus with `cargo xtask conformance
--fetch-only` if it is absent in your worktree.

### 6. Documentation

Crate docs: layout, the cell format, the clamp-or-reject table. `plans/README.md`'s M5
row notes F1. No accuracy-policy entry unless you had to choose between two readings of
the spec, in which case add one (XM entries start at **D42**).

## Research points

1. **Versions `0x0102`/`0x0103`.** OpenMPT loads them (older instrument layout). Decide:
   support if the layout difference is small, else `Unsupported` with the version in the
   message. Record which.
2. **Orders past the pattern count.** FT2 plays an empty pattern; OpenMPT skips. Choose
   FT2's reading (an all-empty pattern) and say so.
3. **Stereo XM samples** (type bit 5, ModPlug extension). Downmix or reject? Reject
   unless the corpus contains one; record.
4. **`ADPCM4`.** Confirm OpenMPT's detection (`ADPCM4` marker where PCM would start) and
   reject with a clear error.
5. **Instrument count above 128 / sample count above 16.** OpenMPT allows more in
   `.xm` written by itself? Confirm and clamp or reject per the table.
6. **The MilkyTracker / OpenMPT tracker-name variants.** Confirm E2's detection strings
   against `Load_xm.cpp` and list what the corpus files report.

## Verification

```sh
cargo test -p starplayer-xm
cargo test -p starplayer-xm --test corpus -- --nocapture     # after `cargo xtask conformance --fetch-only`
cargo test -p starplayer-model
cargo test -p starplayer-offline --test fuzz_seeds
cargo test --workspace
cargo xtask ci --job no-std-check
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
cargo xtask goldens --check
(cd fuzz && cargo +nightly fuzz build xm_loader && cargo +nightly fuzz build xm_structured)   # if nightly is installed; else report
```

Report the exact commands run and their results, plus the corpus table. **Do not commit**
— the reviewer commits.

## Out of scope

The effect processor, envelopes, any playback (F2, F3). Facade `probe`/`load` arms and
the conformance harness (F4). Golden fixtures (F4).
