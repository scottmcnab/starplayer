# M10 — K4: SoundFont 2 → `InstrumentBank`

| Field | Value |
|---|---|
| Milestone | M10 ([master plan](M10-master-plan.md), "The task graph" section); shares `InstrumentBank` with [M11](M11-master-plan.md) |
| Status | Planned; pull-driven (not scheduled) |
| Depends on | K1 (the `&mut self` trait, `Adsr`, `starplayer-synth`); M4-E5 (SMF playback, which this makes useful) |
| Blocks | — |
| Parallel with | K3, K6 |
| Recommended model | Claude Opus (a spec-heavy loader whose output has to sound like every other SF2 player's, plus the first `InstrumentBank`) |
| Verified by | agent (parser against the SF2.04 spec and a permissively licensed test bank, generator mapping tests, an SMF rendered through a bank at every block size), then owner listening |

## Context for a fresh agent

A SoundFont is samples plus instruments plus presets: exactly a tracker module's
instrument half with MIDI addressing on top. M4 gave StarPlayer `SmfSequencer` and an
`InstrumentRack` that plays a **module's** instruments from MIDI; today an SMF needs
`--instruments <module>` to sound at all. This task makes an `.sf2` a source of
instruments, and in doing so introduces **`InstrumentBank`** — master-plan decision 5: a
`Module` with no patterns, wrapped, plus a manifest — which M11's library scan will also
produce, so the rack has *one* notion of where instruments come from.

What SF2 needs beyond `MappedInstrument`: velocity layers (zones select by key **and**
velocity), the DAHDSR volume envelope with its own units (timecents, centibels), per-zone
tuning (root key, coarse/fine tune, scale tuning), stereo sample pairs, loop modes
(none / continuous / until release), and a modulator subset. So the SF2 crate has its own
`Instrument` implementation, `Sf2Instrument`, the way each tracker format has its own
processor: the bank carries the zones in `format_data` and the instrument interprets them.

### Code you must read before changing anything

- `crates/starplayer-model/src/{module,builder,instrument,sample,header}.rs`;
  `crates/starplayer-engine/src/instrument.rs` (`MappedInstrument`, `InstrumentRack::for_module`,
  `Program`, `MIDI_CHANNEL_BASE`); `crates/starplayer-synth/src/adsr.rs` (K1).
- `crates/starplayer-xm/src/loader.rs` or `crates/starplayer-it/src/loader.rs` — a loader
  that builds instruments with note maps, envelopes and per-sample tuning; the shape to
  follow.
- `crates/starplayer-midi/src/{smf,sequencer}.rs`; `crates/starplayer-offline/src/lib.rs`
  `render_smf_song`; `crates/starplayer-host/src/player.rs` `load_smf`.
- The SoundFont 2.04 specification (chapters 7–9: the RIFF chunks, generators, modulators,
  the default modulators); the `fuzz/` crate's loader targets (add one).
- `plans/product/01-technical-architecture.md` §5.3, §6, §11; `plans/engine/M11-master-plan.md`.

## Deliverables

### 1. `InstrumentBank` in `starplayer-model`

`pub struct InstrumentBank(Module)` with `samples()`, `instruments()`, `manifest()`;
`BankManifest { source: Box<str>, entries: Box<[BankEntry]> }`,
`BankEntry { name, bank: u16, program: u8, instrument: InstrumentId, content_hash: [u8; 32], articulation: ArticulationKind }`
where `ArticulationKind` names which `Instrument` implementation runs it
(`Sample | Mapped | Sf2 | Xm | It | …`). `ModuleBuilder::build_bank(manifest)`. The
content hash is over the instrument's referenced PCM and its `InstrumentDef` bytes —
M11's identity rule, stated once here.

`InstrumentRack::for_bank(&Arc<InstrumentBank>, rate)` beside `for_module`, dispatching
each entry to the constructor its `ArticulationKind` names; **bank select** (CC 0/32)
lands in the rack as a per-channel `bank: u16` so `Program` resolves `(bank, program)`
against the manifest — the minimal M11 deliverable 4, done here.

### 2. `crates/starplayer-sf2` (`no_std + alloc`, feature `sf2` in the facade)

- **Parser**: RIFF `sfbk` → `INFO`, `sdta` (16-bit samples; the optional 24-bit `sm24`
  is read and folded to 16 with rounding — recorded as a deviation), `pdta` (phdr, pbag,
  pmod, pgen, inst, ibag, imod, igen, shdr). Every index through `get`; every length
  checked; the harness memory cap in `fuzz/` respected with a decoded-size budget like
  S3M's. Fuzz targets `sf2_loader` and `sf2_structured` with a small seed bank.
- **Bank building**: each sample becomes a `SampleSpec` (reference rate from `dwSampleRate`
  adjusted by `byOriginalPitch`/`chPitchCorrection` so the linear-frequency pitch path
  plays root key at the right Hz; loop from `dwStartloop/dwEndloop`; stereo pairs kept as
  two samples with `format_data` linking them); each *preset* becomes one `InstrumentDef`
  with `note_sample_map` filled by the **highest-velocity** zone per key (so `MappedInstrument`
  can play the bank with no velocity layers — the fallback), and the full zone table
  (key range, velocity range, sample, generators, modulators) serialised into
  `ModuleHeader::format_data` for `Sf2Instrument`. Generator inheritance (global zone →
  instrument zone, preset generators *add* to instrument generators) implemented per spec
  §9.4. Default modulators (velocity → initial attenuation, velocity → filter cutoff,
  channel pressure/CC1 → vibrato LFO depth, CC7/CC10/CC11 handled by the rack) implemented
  as a table; user modulators parsed and applied for the source/destination pairs the
  instrument supports; unsupported ones counted.
- **`Sf2Instrument: Instrument`** (K1 pattern): `note_on` picks every zone matching key
  and velocity (layers → one voice each, stereo pairs → two voices with the pair's pan),
  applies tuning generators, initial attenuation (centibels → `U0F16` via
  `db_to_gain_q15`), starts a volume `Adsr` per voice from the DAHDSR generators
  (timecents → control ticks; hold and delay stages added to K1's `Adsr` — a small
  extension, kept generic), sets the IT filter from `initialFilterFc`/`initialFilterQ`
  (SF2's absolute-cents cutoff → the 7-bit IT cutoff through the linear table; record the
  mapping), and honours `sampleModes` (loop until release → `Voice::queue_region` to the
  one-shot tail on key-off, exactly as IT's sustain loop does). `control_tick` advances
  envelopes and the vibrato LFO (`vibLfoToPitch`, delay/frequency in K1's `Lfo` at control
  rate); `note_off` releases; `Done` stops. Pitch and modulation envelopes: research
  point 2.

### 3. Hosts

- `starplayer render <song.mid> --soundfont <bank.sf2>` and `play` likewise; the CLI's
  `--instruments <module>` stays as the other bank source. `Player::load_smf_with_bank`.
- The web page's MIDI panel accepts an `.sf2` alongside a module (it is a file like any
  other; the archive path handles size).
- `starplayer-offline`: `render_smf_song_with_bank` for tests and goldens.

### 4. Proof

- Parser tests on a small permissively licensed bank committed under
  `crates/starplayer-sf2/tests/fixtures/` (research point 1) and on hand-built RIFF
  fixtures for each error path; fuzz-smoke clean.
- Generator mapping: a preset with `coarseTune 12` plays an octave up (spectral peak);
  `initialAttenuation 200` is −20 dB ± 0.1; a two-layer preset picks the right layer at
  velocity 40 and 100; a stereo pair is panned ±1.
- Envelope: DAHDSR with `attackVolEnv −7973` (10 ms) reaches −0.5 dB within 10 ms ± 1 ms;
  release from sustain follows `releaseVolEnv` within 5 %.
- An SMF rendered through the bank at block sizes 1, 3, 64, 128, 4096, 8191 is
  byte-identical on both paths; RT-safety with the bank driving 64 voices.
- `InstrumentBank` round trip: build → manifest → `for_bank` → `Program` with bank select
  resolves to the right entry; a renamed source file keeps every content hash.
- `no-std-purity` with `starplayer-sf2` listed; `wasm-build`; `clippy`.

### 5. Documentation

Architecture §5.3 (`InstrumentBank`, `Sf2Instrument`, bank select in the rack), §11
(`starplayer-sf2`), §6 (banks are modules without patterns); accuracy policy: a short
"SoundFont" subsection for the 24-bit fold and any generator not implemented; M11 master
plan: note that deliverables 2 and 4 landed here. Append `## Research resolution`.

## Research points

1. **A test bank**: FluidSynth's `VintageDreamsWaves-v2.sf2` is small and freely
   redistributable (check its licence text); GeneralUser GS is large and has its own
   terms. Pick one for the repo and say why; never commit a bank whose licence is unclear.
2. **Pitch and modulation envelopes / LFOs**: `modEnvToPitch`, `modEnvToFilterFc`,
   `modLfoToPitch/FilterFc/Volume`. All are control-rate writes of `Step`/filter/volume
   the instrument can already make; decide how many land in K4 and which are deferred
   with a counter, based on what the test bank actually uses.
3. **Velocity layers vs. the pool**: a piano preset with four layers × stereo = eight
   voices per note if layers are summed; SF2 says layers *are* summed (they are separate
   zones), but real banks use velocity ranges to *select*. Implement selection by range
   and summation only for overlapping ranges, and cap voices per note (research the cap).
4. **Exclusive class** (`exclusiveClass`, hi-hats): a note in a class chokes sounding
   notes of the same class on the same preset — implement it in `note_on` through the
   instrument's own state; it is cheap and audible.

## Verification

```
cargo test -p starplayer-sf2
cargo test -p starplayer-model
cargo test -p starplayer-engine
cargo test --workspace
cargo run -p starplayer-cli -- render <song.mid> --soundfont <bank.sf2> -o /tmp/sf2.wav
cargo xtask goldens --check
cargo xtask ci --job host-tests
cargo xtask ci --job rt-safety
cargo xtask ci --job fuzz-smoke
cargo xtask ci --job no-std-purity
cargo xtask ci --job wasm-build
cargo xtask ci --job clippy
```

## Out of scope

SF3 (Vorbis-compressed samples); DLS; the reverb/chorus send generators (they map onto M7
inserts later — note the mapping); the library scan (M11); editing banks.
