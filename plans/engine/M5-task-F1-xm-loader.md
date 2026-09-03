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

## Research resolution

Recorded at implementation time; each answer is the decision, not a summary of the
question. Sources: FastTracker 2's own `xm.txt` (Fredrik Huss / Mr.H of Triton, 1994,
mirrored at `ftp.modland.com/pub/documents/format_documentation/`), OpenMPT's
`soundlib/Load_xm.cpp`, `soundlib/XMTools.{h,cpp}` and `soundlib/SampleIO.cpp` on GitHub
(`OpenMPT/openmpt`, `master`), and libxmp's `src/loaders/xm_load.c` — all fetched, none
recalled. Every claim about the corpus below was measured against the pinned
`libxmp-6ec0ba21b1b28f91e22b68a51d59207c6bbf6139` tree.

### 1. Versions `0x0102` / `0x0103` — **supported; the layout difference is two lines**

Not optional: `data/m/dontyou.xm` is version `0x0102` and lives in a directory deliverable
5 requires to load `Ok`. It is the only pre-`0x0104` file in the corpus (160 of the 161
XM-signature files are `0x0104`).

The difference is exactly two things, both isolated behind predicates on
[`XmHeader`](../../crates/starplayer-xm/src/header.rs):

* `patterns_precede_instruments()` — from `0x0104` the patterns come first and each
  instrument's PCM follows that instrument's sample headers; before it, the instrument and
  sample *headers* come first, then the patterns, then every sample's PCM in one run at the
  very end (`Load_xm.cpp`, the `if(fileHeader.version >= 0x0104)` pair around lines 722 and
  940).
* `rows_are_a_biased_byte()` — `0x0102` alone stores a pattern's row count as `u8 + 1`, so
  its pattern header is eight bytes rather than nine and its packed-size field sits two
  bytes earlier. `0x0103` already uses the `u16` (`Load_xm.cpp`,
  `if(fileHeader.version == 0x0102) numRows = file.ReadUint8() + 1;`).

The loader collects every instrument's headers into a pending list either way and assigns
the PCM offsets in a second pass, so the two layouts share all of the decoding. No version
is rejected: a version below `0x0102` simply takes the older path, which is what OpenMPT
does.

### 2. Orders past the pattern count — **FastTracker 2's reading: one shared empty pattern**

FastTracker 2 holds 256 pattern slots in memory at all times and the slots a file does not
store are empty 64-row patterns, so an order naming one plays four bars of silence rather
than being skipped. The loader appends **one** empty 64-row pattern after the file's own
when — and only when — some order needs it, and points every such order at it.

Note that deliverable 1 of this task says "an order past the pattern count maps to
`ORDER_MARKER`-style skipping per research point 2" while research point 2 says to choose
FastTracker 2's reading. They contradict each other; research point 2 is the explicit
instruction and is what is implemented. The corpus does not distinguish the two readings —
no file in it has an out-of-range order — so this is a fidelity choice, not a
corpus-forced one, and `an_order_past_the_pattern_count_plays_one_shared_empty_pattern`
pins it.

One related FastTracker 2 behaviour is implemented alongside it, from `Load_xm.cpp`'s
`verEmptyOrders` handling: a `song_length` of zero becomes a one-entry order list naming
pattern 0 — the fix for `lamb_-_dark_lighthouse.xm` — **except** in a file whose tracker
name says OpenMPT, which means an empty order list literally.

### 3. Stereo XM samples — **decoded and downmixed; the corpus has two of them**

The research point says "reject unless the corpus contains one". It does: `data/stereo.xm`
and `data/test.xm`, both directly under `data/` and therefore both required to load `Ok`,
carry two stereo samples each — one with `type` `0x21` (loop + stereo) and one with `0x31`
(loop + 16-bit + stereo). Rejecting the flag would fail deliverable 5.

ModPlug's stereo layout is `stereoSplit`: the left channel's **whole** delta stream, then
the right channel's, each decoded from its own running accumulator, with `length`,
`loopStart` and `loopLength` all counting both channels' bytes (OpenMPT
`XMSample::ConvertToMPT` halves all three again for stereo, on top of the halving for
16-bit). The engine's sample blob is mono and pan comes from the channel, so the two
channels are averaged: `(left + right) / 2` per frame. The alternative — keeping the left
channel only, as `starplayer-s3m` does for S3M's own stereo flag — loses half the signal
of a genuinely stereo sample, and unlike S3M's case these files really exist.

### 4. `ADPCM4` — **decoded, not rejected; the task's detection description was wrong**

Two findings, and both change the deliverable.

**The detection is not a marker in the data.** OpenMPT's `XMSample::GetSampleFormat`
(`XMTools.cpp`) reads the sample header's `reserved` byte at offset `0x11`: the sample is
ADPCM when `reserved == 0xAD` **and** neither the 16-bit nor the stereo flag is set. There
is no `ADPCM4` string where the PCM would start. libxmp agrees (`xm_load.c` line 704,
`if (xsh[j].reserved == 0xad) flags = SAMPLE_FLAG_ADPCM;`). FastTracker 2 itself writes the
sample name's *length* in that byte, which is why the 16-bit/stereo guard is needed.

**Rejecting the file is not available.** `data/m/MRHPx-HBTN LUCiFER.xm` has fourteen ADPCM
samples and lives under `data/m/`, which deliverable 5 requires to load `Ok`. So the
`Error::Unsupported("ADPCM-compressed XM sample")` the deliverable asks for would fail the
corpus test on that file, and there is no exclusion mechanism in a loader-level test.

The decoder is twenty lines and is transcribed from OpenMPT's `SampleIO::ReadSample`: a
16-byte signed compression table, then two 4-bit table indices per byte, **low nybble
first**, each added to a running `i8`; the encoded size is `16 + (length + 1) / 2` bytes
and `length` is a *frame* count rather than a byte count for this encoding alone. This is
the one place the implementation is deliberately larger in scope than the task file asked
for; a reviewer who prefers the rejection would also have to add a corpus exclusion for
that file.

### 5. Instrument count above 128 / sample count above 16 — **both tolerated, with a ceiling on the first**

*Instruments.* `xm.txt` says 128. OpenMPT clamps instead of rejecting —
`m_nInstruments = std::min(fileHeader.instruments, MAX_INSTRUMENTS - 1)`, i.e. 255. The
corpus's largest declared instrument count is exactly 128, so nothing forces the question.
The loader takes OpenMPT's ceiling: 129..=255 loads normally (with a doc note that it is
out of specification), and above 255 is `Error::TooLarge("more than 255 XM instruments")`.
A ceiling is needed rather than nothing, because an `InstrumentDef` is around 370 bytes of
note maps and a `u16` count would let an eighty-byte header ask for 24 MB.

*Samples per instrument.* `xm.txt` implies 16 and OpenMPT's `AllocateXMSamples` caps its
*slot allocation* at 32 while still reading every declared header; libxmp is stricter and
**rejects** the whole file above 32 (`XM_MAX_SAMPLES_PER_INST`). The corpus goes past 16:
`data/m/grass near the house.xm` has an instrument with 23 samples. This loader imposes no
count limit at all — it reads sample headers until one would run past the end of the file
and then stops, so the real bound is the file's own size, and there is no cliff at 16 or
32. A note whose map entry names a local sample the instrument does not have maps to no
sample rather than to a wrong one.

*Patterns and channels.* Both are refused rather than clamped, because both are engine
limits rather than tolerances: above 256 patterns is `Error::TooLarge` (the format's own
maximum, and the order byte cannot name more), and above 64 channels is `Error::TooLarge`
(`ChannelTable::MAX_CHANNELS`). The corpus's widest file is `data/m/xyce-dans_la_rue.xm`
at 22 channels — note the odd count, which OpenMPT rounds up to even and this loader does
not.

### 6. Tracker-name variants — **E2's strings confirmed, with one clause dropped**

Checked against `Load_xm.cpp` lines 618–692 and against every file in the corpus.

| Evidence | Dialect | Corpus files |
|---|---|---|
| `OpenMPT ` prefix (8 bytes) | `OpenMptXm` | 70 |
| `MilkyTracker` prefix (12 bytes — OpenMPT's `memcmp(…, "MilkyTracker ", 12)` compares twelve, so the trailing space is not part of the test) | `MilkyTracker` | 2 |
| `FastTracker v 2.00  ` exactly, note the extra mid-string space | `ModPlugXm` | 0 |
| `FastTracker v2.00   ` exactly, with `header_size == 276` | `FastTracker2` | 59 |
| `Fasttracker II clone` exactly (8bitbubsy's clone, which OpenMPT treats as FT2) | `FastTracker2` | 2 |
| anything else | `Unknown` | 7 |

The seven `Unknown` files are `MadTracker 2.0`, `Skale Tracker`, `XMLiTE`,
`rst's SoundTracker`, `*MRHPx produktions*` (×2) and `Fasttracker Too!!11`. OpenMPT
recognises the first two and adjusts two play behaviours for each; StarPlayer has no
`QuirkSet` field for either yet, and task C5's rule is that a dialect earns a field only
when a corpus case justifies one, so they stay `Unknown` — which resolves to the profile
default, exactly as E2's `quirks.rs` documents for every XM dialect.

Two departures from E2's doc comment, both deliberate:

* **The `version >= 0x0104` clause is dropped from the FastTracker 2 test.** `quirks.rs`
  describes the tag as "header size 276 and format version `0x0104` or later", but
  `Load_xm.cpp` sets `verFT2Generic | verConfirmed` for a version *below* `0x0104` with
  that tag and header size — an old FastTracker 2 file is still a FastTracker 2 file. The
  loader therefore tests the name and the header size only. `data/m/dontyou.xm`
  (version `0x0102`) is classified `FastTracker2` because of this; with the version clause
  it would have been `Unknown`. Since every XM dialect maps to
  `QuirkSet::profile_default()` today, this changes evidence and not behaviour.
* **`Fasttracker II clone` is added**, which E2's comment does not mention. It is a
  distinct 20-byte tag, not one of the null-padding heuristics E2 says StarPlayer folds
  together, and OpenMPT maps it to FT2 outright. Two corpus files carry it.

`quirks.rs` was not edited: it is outside this task's allowed file list, and its comment is
prose about detection evidence rather than an executable rule. A reviewer may want to
reword the `FastTracker2` doc comment when F4 lands.

### 7. Not a research point, but a decision a reviewer will want: `sample_header_size` is not a stride

The deliverable says "Instrument and sample header sizes are honoured, not assumed". The
instrument header's `size` **is** honoured — trackers genuinely disagree about it (263 from
FastTracker 2 and OpenMPT, 245 from ModPlug Tracker 1.0 alpha, 33 for an empty FastTracker 2
instrument, 29 in `4-mat`'s `eternity.xm`), and fields past it read as zero, matching
OpenMPT's `ReadStructPartial`.

`sample_header_size` is **not** used as a stride. OpenMPT reads a fixed
`sizeof(XMSample)` = 40 bytes per sample header and only asserts that the field is 0 or 40;
libxmp likewise seeks by `40 * xih.samples`. Both do so because FastTracker 2 does: early
Sk@le Tracker writes `0` (`IFULOVE.XM`) and `cybernostra weekend` writes `0x12`, which
would cut the sample name off, and FastTracker 2 reads the full 40 bytes in both cases.
Honouring the field would break exactly the files the two reference loaders added the
workaround for. The field is parsed and exposed on `XmInstrumentHeader` (with 0 and
anything above 263 normalised to 40) so a later task can use it as evidence; it does not
move the cursor.

### 8. Not a research point either: one corpus `*.xm` is not an XM

`data/ice21_ambiguous.xm` is an **Ice Tracker** module — `Ice!` at offset 0 — that libxmp
ships to test format disambiguation, and whose song title happens to begin
`Extended Modu: Necromanc…`. It has an `.xm` extension and must *not* probe or load as an
XM. `crates/starplayer-xm/tests/corpus.rs` names it in a `NOT_XM` list and asserts both
`probe` and `load` refuse it, so the file is checked rather than merely skipped. This is
why the corpus table below reports 140 modules loaded from 141 `*.xm` files rather than
141.
