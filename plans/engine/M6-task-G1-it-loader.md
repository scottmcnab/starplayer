# M6 — G1: The IT loader

| Field | Value |
|---|---|
| Milestone | M6 ([master plan](M6-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready |
| Depends on | M4-lite landed (E1–E3) |
| Blocks | G3 (IT instrument runtime), G4 (effects), G5 (wiring) |
| Parallel with | D3, D6, F1 |
| Recommended model | Claude Opus (two decompressors, the packed pattern format, and 232 corpus files to survive) |
| Verified by | agent (`cargo test -p starplayer-it`, the corpus load test, fuzz seeds, `cargo xtask ci --job no-std-check`, `--job clippy`), then reviewer |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first — the
design invariants there are non-negotiable, and two matter most here: **format identity**
(an IT keeps its native pattern data and gets its own effect processor) and **a loader
either produces a valid `Module` or an `Err`, never a panic**.

`crates/starplayer-it` is an empty shell with its manifest, feature table and `no_std`
enforcement in place. This task makes it a loader. The instrument runtime with NNA (G3),
the effect processor (G4) and the facade/harness wiring (G5) are separate tasks; **do not
touch `crates/starplayer/src/lib.rs`** — task D3 is refactoring it concurrently.

The model already has every field an IT needs (task E1): sample sustain loops, `C5Speed`
as `reference_rate_hz`, per-sample auto-vibrato and default pan, the `u16` note→sample map
plus the note transpose map, signed envelope points, NNA/DCT/DCA, instrument global
volume, pan, pitch-pan, random variation, initial filter and the filter-envelope flag,
`ModuleHeader::default_channel_volume` and the opaque `format_data`;
`FormatDialect::{ImpulseTracker, OpenMptIt, SchismTracker, ModPlugIt}` (task E2).

### The specification

`ITTECH.TXT` (Impulse Tracker 2.14's own format document, mirrored widely — for example
under `ftp.modland.com/pub/documents/format_documentation/`), read together with
OpenMPT's `soundlib/Load_it.cpp`, `soundlib/ITTools.{h,cpp}` and
`soundlib/ITCompression.cpp` on GitHub (`OpenMPT/openmpt`). Use WebFetch to read them; do
not work from memory. `ITTECH.TXT` documents the header, instruments (new and old
format), samples, patterns and the embedded MIDI configuration; OpenMPT documents the
real-world deviations and the decompressor.

### The template

`crates/starplayer-s3m` is the shape to copy: `header.rs`, `pattern.rs`, `sample.rs`,
`loader.rs` (`probe`, `probe_reader`, `load`, `load_from` over `ModuleReader`, with a
decoded pattern budget), `lib.rs`. Read its `loader.rs` docs on clamp-or-reject decisions
and copy that discipline.

### Code you must read before changing anything

- `crates/starplayer-s3m/src/{loader,header,pattern,sample,lib}.rs` — the template.
- `crates/starplayer-model/src/{builder,sample,instrument,header,pattern,reader}.rs`.
- `crates/starplayer-engine/src/sequencer.rs` — `PatternData` (`row_bytes` returns one
  fixed-stride slice per row).
- `crates/starplayer-core/src/quirks.rs` — the IT `FormatDialect` variants and their
  `Cwt/v` / `Cmwt` detection docs.
- `fuzz/` (as for F1) and `crates/starplayer-offline/tests/{fuzz_seeds,render_allocation}.rs`.
- `xtask/src/main.rs` — `NO_STD_CRATES` (already lists `starplayer-it`), the fuzz job.

## Deliverables

### 1. `crates/starplayer-it` loader

- **`probe`**: `IMPM` at offset 0.
- **Header**: song name, `PHiligt`, `OrdNum`, `InsNum`, `SmpNum`, `PatNum`, `Cwt/v`,
  `Cmwt` (`Cmwt < 0x200` → old-format instruments, research point 1), `Flags` (bit 0
  stereo → `ModuleFlags::stereo`; bit 2 instrument mode; bit 3 linear slides →
  `ModuleFlags::linear_slides`; bit 4 old effects; bit 5 compatible `Gxx`; bit 6 MIDI
  pitch controller; bit 7 embedded MIDI macros requested), `Special` (bit 0 message; bit
  3 MIDI configuration embedded), global volume, mix volume, initial speed, initial
  tempo, stereo separation, pitch-wheel depth, message length/offset, `ChnPan[64]` (0..64
  → `I1F15`; 100 = surround; +128 = disabled) and `ChnVol[64]` (→
  `default_channel_volume`), the order list (254 → `ORDER_MARKER`, 255 → `ORDER_END`),
  then the instrument, sample and pattern parapointers. Everything the engine does not
  read but the processor will — instrument mode, old effects, compatible `Gxx`, the MIDI
  pitch controller flag — goes in `format_extra` as documented bits; surround and
  disabled channel flags and the embedded MIDI macro block (9×32 global + 16×32 `SFx` +
  128×32 `Zxx` bytes) go in `format_data` with a documented layout, because `Zxx` and
  `Sxx` need them (G4). `dialect` from `Cwt/v`/`Cmwt` per E2.
- **Instruments** (`IMPI`, new format): NNA, DCT, DCA, fadeout (raw), PPS, PPC, global
  volume, default pan (bit 7 clear = enabled → `Some`), random volume and pan, tracker
  version, sample count, name, initial filter cutoff and resonance (bit 7 = enabled),
  MIDI channel/program/bank (ignore), the 120 `(note, sample)` pairs →
  `note_transpose_map` and `note_sample_map` (global ids; sample 0 = none), three
  envelopes (flags: bit 0 on, bit 1 loop, bit 2 sustain loop, bit 7 filter on the pitch
  envelope → `pitch_envelope_is_filter`; node count; loop begin/end; sustain begin/end;
  25 × `(value: i8, tick: u16)`), with volume envelope values `0..64` and pan/pitch
  `−32..32`. Old-format instruments: research point 1.
- **Samples** (`IMPS`): name, global volume, flags (bit 0 has data; bit 1 16-bit; bit 2
  stereo; bit 3 compressed; bit 4 loop; bit 5 sustain loop; bit 6 ping-pong loop; bit 7
  ping-pong sustain), volume, default pan (bit 7 = enabled), length, loop begin/end,
  `C5Speed` → `reference_rate_hz`, sustain loop begin/end → `SustainLoop`, sample
  pointer, vibrato speed/depth/rate/type (0 sine, 1 ramp down, 2 square, 3 random),
  convert flags (bit 0 signed, bit 2 delta for IT 2.15 compressed data). PCM: raw 8/16-bit
  signed or unsigned per the convert flag; **IT 2.14 compressed** 8- and 16-bit blocks
  (`ITCompression.cpp` is the reference: 0x8000-byte / 0x4000-sample blocks, the
  bit-width state machine, the IT 2.15 delta variant), decoded into `i16`; a stereo sample
  (OpenMPT's extension: left block then right block) is **downmixed** `(l + r) / 2` at
  load — record it as accuracy-policy entry **D60** (IT entries start at D60). A truncated
  or malformed compressed block must produce `Err` or a shorter sample, never a panic or a
  huge allocation (`data/f/load_it_invalid_compressed*.it` and `play_it_truncated_sample.it`
  are the regression files).
- **Patterns**: length, rows (`1..=200` per spec; accept up to 256), packed data with
  the channel-variable byte (`0` ends the row; bit 7 = a new mask follows; low 6 bits +1
  = channel), the per-channel mask memory and the per-channel note / instrument /
  volume / command / parameter memories. Unpack into fixed **5-byte cells** per channel
  per row, `[note, instrument, volpan, command, parameter]`, in IT's own byte values
  (`note` 0..=119, 254 note cut, 255 note off, 253 note fade; document the byte you use for
  "no note", which must not collide with any of those). A pattern parapointer of zero is
  an empty 64-row pattern. Budget the decoded size.
- **Clamp-or-reject table** in the crate docs, as S3M's.

### 2. `pattern.rs` — the cell and the display view

`ItCell` with `from_bytes`, tracker notation, volume-column decoding helpers, and
`PatternView` producing display-only `PatternCell`s. Add `EffectNames::IT` to
`crates/starplayer-model/src/pattern.rs` — the English names of `Axx`..`Zxx` and the
volume-column effects — a table only, and the only model change in this task.

### 3. `ItPatternData(pub Arc<Module>)` implementing `PatternData`

`row_bytes` returns the `channel_count × 5` slice of a row. IT pattern channel count is
always 64 in the file; the header's used-channel count comes from `ChnPan` (a channel is
in use if not disabled and any pattern writes to it — follow OpenMPT's `GetNumChannels`
rule and record it). Nothing else of the engine seam is implemented here (G3/G4).

### 4. Fuzzing

`fuzz/fuzz_targets/it_loader.rs` and `it_structured.rs`; synthesised seeds under
`fuzz/seeds/it/` (minimal sample-mode file, instrument-mode file with envelopes and NNA,
a compressed 8-bit and a compressed 16-bit sample, a sustain ping-pong loop, embedded MIDI
macros, a header with maximum counts); the `Mutation` walker gains IT cells;
`fuzz/regressions/it/README.md`; `crates/starplayer-offline/tests/fuzz_seeds.rs` replays
the IT seeds; `xtask`'s fuzz job lists the targets. No corpus binaries are committed.

### 5. The corpus load test

`crates/starplayer-it/tests/corpus.rs`: walk every `*.it` under the pinned libxmp
`test-dev` (skip with a message if `target/conformance` is absent). Every file under
`data/`, `data/m/` and `openmpt/it/` must load `Ok`; every `data/f/load_it_*` and
`data/f/play_it_*` must return `Err` or `Ok` without panicking; print a table of counts,
the dialect distribution, and how many samples were compressed, stereo, or sustain-looped.
`data/format_it_schism.it` must report `FormatDialect::SchismTracker`.

### 6. Documentation

Crate docs: layout, the cell format, the `format_extra` bits, the `format_data` layout,
the clamp-or-reject table. `plans/README.md`'s M6 row notes G1. Accuracy policy: D60 for
the stereo downmix and any other reading you had to choose.

## Research points

1. **Old-format instruments (`Cmwt < 0x200`).** IT 1.x stores a 200-byte volume envelope
   table instead of nodes. OpenMPT converts it. Support if the conversion is under ~60
   lines; else `Unsupported` naming the version. Record which and whether the corpus has
   any.
2. **IT 2.15 delta compression** (`Cvt` bit 2 with the compressed flag): confirm from
   `ITCompression.cpp` and test with a synthesised sample.
3. **Used channel count.** Confirm OpenMPT's rule and libxmp's (`it_load.c`), pick one,
   and note the difference, because the trace and telemetry channel counts come from it.
4. **`Cwt/v` beyond IT**: MPT/OpenMPT files carry extra chunks after the pattern data
   (`STPM`, `XTPM`, plugin data). Ignore them; confirm nothing in the corpus needs them to
   *load*.
5. **Pattern rows above 200.** Accept up to 256 (OpenMPT writes more in `.mptm`, not
   `.it`); confirm and clamp.

## Verification

```sh
cargo test -p starplayer-it
cargo test -p starplayer-it --test corpus -- --nocapture       # after `cargo xtask conformance --fetch-only`
cargo test -p starplayer-model
cargo test -p starplayer-offline --test fuzz_seeds
cargo test --workspace
cargo xtask ci --job no-std-check
cargo xtask ci --job no-std-purity
cargo xtask ci --job clippy
cargo xtask goldens --check
(cd fuzz && cargo +nightly fuzz build it_loader && cargo +nightly fuzz build it_structured)   # if nightly is installed; else report
```

Report the exact commands run and their results, plus the corpus table. **Do not commit**
— the reviewer commits.

## Out of scope

NNA, envelopes, the filter, any playback (G2, G3, G4). Facade arms and the conformance
harness (G5). `TempoModel::ItModern` (G4).

## Research resolution

Recorded at implementation time; each answer is the decision, not a summary of the
question. Corpus figures are from the pinned libxmp `test-dev` tree
(`6ec0ba21b1b28f91e22b68a51d59207c6bbf6139`): 85 files under `data/`, 3 under `data/m/`,
119 under `openmpt/it/` and 25 under `data/f/`.

### 1. Old-format instruments (`Cmwt < 0x200`) — **supported, and the corpus needs it**

Supported, in `instrument::ItInstrument::parse_old`, at 46 lines of body. The conversion is
small because ITTECH.TXT's 200-byte `volenv[200]` table at `0x130` **is not read**: it is a
pre-interpolated copy of the envelope whose 25 `(tick, value)` node pairs sit right behind
it at `0x1F8`, and OpenMPT's `ITOldInstrument::ConvertToMPT` ignores it for exactly that
reason. So the whole difference from the IT 2.xx layout is: one envelope instead of three;
its flag byte, loop points and sustain points come from the *instrument* header (`0x11`
through `0x15`) rather than from an envelope structure; its nodes are two-byte
`(tick, value)` pairs terminated by a tick of `0xFF` rather than three-byte
`(value, tick)` triples with a count; `NNA` and the duplicate-check flag sit at `0x1A`/`0x1B`
rather than `0x11`/`0x12`; and there is no panning envelope, pitch envelope, filter,
pitch/pan separation, instrument global volume, random variation or duplicate-check
*action*. Those absent fields take the values `ITOldInstrument::ConvertToMPT` gives them —
full global volume, no instrument pan, `Cut` on a duplicate.

The one thing that is *not* a straight copy is the fadeout. ITTECH.TXT gives the old
format a 0..64 fadeout against a count of 512 and the new one a 0..128 fadeout against a
count of 1024, so the same raw value fades twice as fast in the old layout; OpenMPT spells
the same ratio as `fadeout << 6` against `fadeout << 5`. The model stores one fadeout in
"the source format's own units", so the loader multiplies an old fadeout by 2 and G3 has a
single scale to interpret. `instrument::OLD_FADEOUT_SCALE` is that constant.

**The corpus has seven files with `Cmwt < 0x200`**: `data/pattern_loop_it100.it`,
`pattern_loop_it100_breakjump.it`, `pattern_loop_it104.it`, `pattern_loop_it200_breakjump.it`,
`pattern_loop_it210.it`, `data/m/Fight2.it` and `data/f/play_it_reverse_past_end.it`. Six of
those declare no instruments at all (they are sample-mode pattern-flow fixtures), but
**`data/m/Fight2.it` is instrument mode with ten old-layout instruments**, so returning
`Unsupported` would have cost a real module. `fuzz/seeds/it/old-instruments.it` is the
synthesised seed and `crates/starplayer-it/tests/seeds.rs` pins the conversion.

### 2. IT 2.15 delta compression — **confirmed: `Cvt` bit 2 *with* the compressed flag**

Confirmed from `ITSample::GetSampleFormat`, which reads
`sampleIO |= (cvt & ITSample::cvtDelta) ? SampleIO::IT215 : SampleIO::IT214` **inside** the
`flags & sampleCompressed` arm. So `Cvt` bit 2 means two different things depending on the
compressed flag: with it, the double-integrator variant; without it, ordinary delta-coded
PCM. `sample::decode_frames` handles the second and `compression::decompress`'s `is_215`
argument the first, and `sample.rs`'s `CONVERT_DELTA` documents the split.

The difference in the decoder is one line — `ITDecompression::Write` stores `mem2` rather
than `mem1`, where `mem2 += mem1` on every sample — but it is the whole waveform, so it is
tested three ways: `compression::tests::the_it_215_variant_integrates_twice` drives the same
residuals through both variants and asserts the two running sums (`1, 2, 0` against
`1, 3, 3`); `fuzz/seeds/it/compressed-16bit.it` is a synthesised 16-bit IT 2.15 sample; and
`crates/starplayer-it/tests/seeds.rs` asserts it decodes to its declared 64 frames of
non-silence. The corpus has only two IT 2.15 samples and both are in `data/f/`
(`load_it_invalid_compressed.it`, `play_it_extreme_filter.it`), which is why the synthesised
seed matters.

One further detail worth recording, because it is not in ITTECH.TXT at all: the compressed
stream's **block size is in bytes, not samples**. `ITCompression::blockSize` is `0x8000` and
`Uncompress` takes `blockSize / sizeof(sample_t)`, so a block holds up to 0x8000 samples of
8-bit data and 0x4000 of 16-bit, and the two integrators reset at every block boundary.

### 3. Used channel count — **OpenMPT's rule, and this is accuracy-policy entry D61**

OpenMPT (`Load_it.cpp`'s pre-scan) counts a channel as used only when an event's mask
**low nybble** is non-zero — that is, when the stream really writes a note, an instrument,
a volume or a command there — and it does *not* mask the channel byte to six bits.
libxmp (`it_load.c`'s `max_ch` scan) counts any channel an event names, whether or not the
event writes anything, and masks the channel with `& 63`.

Picked: **OpenMPT's**, clamped to the format's 64 columns and floored at 1, in
`pattern::used_channels`. Two consequences, both recorded as **D61** in
`plans/product/03-accuracy-policy.md`:

* a pattern that names a channel with an all-memory mask (low nybble zero) does not widen
  the song, where libxmp's rule would;
* a channel byte above 64 is parsed against its own memories and then discarded, where
  libxmp wraps it into `0..63`. This can only arise in a file no Impulse Tracker wrote,
  since IT's own unpacker masks; the per-channel memory arrays are sized for OpenMPT's full
  `0..126` range so such an event cannot alias channel 0's memory on its way past.

A channel the header **disables** (`ChnPan` bit 7) still counts, because ITTECH.TXT is
explicit that "effects in muted channels are still processed"; the disabled and surround
flags are carried verbatim in `format_data` so G4 can mute the notes without the trace and
telemetry channel counts changing under it.

Loading is a two-pass walk because of this: `loader::scan_patterns` counts without
allocating, and only then does `pattern::unpack` run at the fixed stride the scan produced.
That is also what makes the decoded-pattern budget exact before a byte of it is allocated.

### 4. `Cwt/v` beyond IT and the trailing extension chunks — **ignored, and nothing needs them**

Ignored. The loader stops at the last thing the offset tables point at and never looks for
`STPM`, `XTPM`, `PNAM`, `CNAM` or a plugin chunk, all of which OpenMPT writes *after* the
pattern data and reads only for editor state (pattern names, channel names, plugin
assignments, extended instrument/song properties).

Confirmed against the corpus by scanning for the two chunk tags: they are present in 40
files — 27 `STPM` and 9 `XTPM` under `data/`, one of each under `openmpt/it/`, and 12
`STPM` / 3 `XTPM` under `data/f/` — and **every one of the 207 files in the three required
directories loads `Ok` without them**. Nothing in a chunk is needed to *load* a file; what
lives there that playback would eventually want (OpenMPT's extended song properties, its
per-channel settings above 64 channels) belongs to a later task, not to G1.

The `Cwt/v` values themselves are read, for the dialect: the pinned corpus's 207 loadable
files come out as 118 `ImpulseTracker`, 39 `SchismTracker`, 27 `OpenMptIt` and 23
`ModPlugIt`, and `data/format_it_schism.it` reports `SchismTracker` as the deliverable asks.

### 5. Pattern rows above 200 — **256 accepted, anything else becomes an empty 64-row pattern**

Confirmed and implemented as `pattern::MAX_ROWS = 256`. ITTECH.TXT says 32..200; libxmp's
own comment records the three real limits — "Impulse Tracker and Schism Tracker allow up to
200 rows, ModPlug Tracker 1.16 allows 256 rows, OpenMPT allows 1024 rows" — and its
`num_rows > 1024` guard is for `.mptm`, not `.it`.

**Clamping is the wrong word for what happens**, which is worth recording because the task
file says "clamp": a pattern whose row count is outside `1..=256` is *skipped* and becomes an
empty 64-row pattern, exactly as OpenMPT (`numRows < 1 || numRows > MAX_PATTERN_ROWS →
continue`) and libxmp (`pp_pat[i] = 0`) both do. Truncating a 1024-row pattern to 256 would
silently reinterpret its packed stream, since the row terminators past row 255 would be read
as channel bytes; dropping it keeps the pattern numbering and plays silence.

The corpus needs the 256 acceptance: `data/bidi_sync.it` and `data/m/another life.it` each
contain a 256-row pattern, and both are in the must-load set. At the other end
`data/f/load_it_dca_3.it` declares row counts of 65281, 43028, 36607, 35721, 19467 and 6230,
which is exactly the population the skip is for.
