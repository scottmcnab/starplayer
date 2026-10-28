# M11 — The instrument library: tracker samples as MIDI instruments

| Field | Value |
|---|---|
| Goal | Every instrument in a library of MOD/S3M/MTM/XM/IT files, playable from MIDI |
| Estimate | open-ended |
| Depends on | M4 (`Instrument` trait, MIDI in, SMF), and much better with M5/M6 (XM/IT envelopes) |
| Trigger | **Pull-driven.** Owner idea, recorded 2026-08-29. Start when the MIDI path from M4 is in real use |
| Related | [M10](M10-master-plan.md) — SoundFont support should share this milestone's `InstrumentBank` abstraction |

## The idea

Scan a library of tracker modules, extract every instrument, and expose the whole
collection as a bank of MIDI-playable instruments. Decades of demoscene sample work
becomes a playable instrument library.

It works best on XM and IT, and that is not a coincidence: those formats already carry
volume and panning envelopes, key-off and fadeout, auto-vibrato, and a note→sample map.
An XM or IT instrument **is** a synthesiser patch in everything but name.

## Current engine support and the remaining articulation work

The engine already supplies sample mapping and a shared voice pool, but MIDI mapping
alone does not supply native instrument articulation.

- **Legacy live constructors remain sample players.** M4's `SampleInstrument` and
  `MappedInstrument` map samples and tuning without native XM/IT articulation.
- **The NO feature branch implements native loaded-module articulation and audition.**
  NO-02 supplies `XmArticulationRuntime`/`ItArticulationRuntime`; NO-03 supplies the shared
  exact tracker/held cadence; NO-06 installs one mutable `RackRuntime` per rack with both
  concrete format adapters and a separate live RNG. The facade's explicitly prepared
  Song and Live factories run native maps, envelopes, release, fadeout, auto-vibrato,
  IT filter and note actions. NO-W1 exposes loaded-module controls in the browser.
  NO-07 verifies the combined path, including real browser and firmware compile checks;
  main landing and owner listening/device execution remain separate.
  Optional features and runtime preparation preserve legacy/embedded defaults.
- **M11 extraction, library and bank design remain outstanding.** Loaded-module audition
  does not extract self-contained instruments or implement stable bank addresses,
  synthetic release envelopes, broad controllers or a library browser.
- **The event vocabulary is already a MIDI superset.** `Event::NoteOn`/`NoteOff`/
  `Program`/`Controller` exist, MIDI parsing is a codec rather than the internal
  representation, and velocity is `U0F16` rather than 7 bits
  (`plans/product/01-technical-architecture.md` §2.3).
- **Voices are format-agnostic.** One global pool, generational handles, background
  voices for NNA. Native tracker IT and explicitly prepared native live racks both use it.
- **`Module` is already position-independent** — blob plus u32 offsets — so extracting
  and re-packing sample data is a mechanical transform, not a redesign.

## The five real problems

Beyond the loaded-module articulation subset in NO, the library still needs the following work.

### 1. An instrument is not self-contained

`InstrumentDef` references samples by index into **its own module's** `pcm` array, and
`Module` is one allocation. Playing a single instrument from a 4 MB module means keeping
all 4 MB resident, and a library of hundreds of modules is then untenable.

So the milestone's central deliverable is an **extraction step**: an `InstrumentBank` that
owns only the PCM its instruments actually reference, rebuilt with fresh offsets. Same
blob-plus-offsets shape as `Module`, same `Arc`-shared, hashable, flash-friendly
properties.

### 2. Tuning — MIDI note numbers versus tracker periods

Tracker instruments are tuned by C2SPD or finetune relative to a period table, not to
A440. There is no native notion of "middle C = MIDI 60".

- **XM and IT map cleanly.** Per-sample `relative_note` and `finetune`, plus the 96-entry
  note→sample map, give a real, unambiguous answer.
- **MOD, S3M and MTM need a convention.** The only sane one is "C-4 at the sample's
  reference rate", plus a per-instrument transpose and fine-tune offset stored in the
  bank so the user can correct it. State the convention explicitly rather than letting it
  emerge from the arithmetic.

### 3. Note-off has nothing to release into, for the simple formats

MOD/S3M/MTM samples are one-shot or forward-looping with no envelope. A MIDI `NoteOff` on
such an instrument has no natural meaning: the note either rings until the loop ends or
gets cut abruptly.

Offer a **synthesised release** — a short configurable fade — as bank metadata, defaulting
to a few tens of milliseconds. XM and IT need none of this; they already have a release
envelope and fadeout.

### 4. Velocity, and what else MIDI should reach

Tracker instruments have no velocity layers, so velocity maps to volume through a curve
that has to be chosen. NO loaded-module audition scales the native base note gain from velocity before the
envelope and records that identity before separate CC7 gain. A future extracted bank
still needs its own velocity curve and metadata contract.

Worth doing while in the area: map IT's filter cutoff and resonance to MIDI CC 74/71.
That turns an IT instrument into something genuinely expressive from a keyboard, and IT's
filter is already built (M6).

### 5. Addressing and identity

MIDI program change plus 14-bit bank select gives about two million addresses, which is
ample. But IDs must be **stable across sessions and across file renames**, so identity is
a **content hash of the instrument's sample data plus its parameters**, not a file path.

That choice pays for itself immediately: content hashing gives **free deduplication**, and
the duplication in a real MOD library is enormous — the Amiga ST-01/ST-02 sample disks
appear in a very large fraction of all MODs ever written. A library scan of a few thousand
modules should collapse to a far smaller unique instrument count.

## Composition with M10

Two connections worth building deliberately rather than discovering later:

- **SoundFont shares the abstraction.** M10 lists SF2 support. An SF2 file and a library
  scan should both produce an `InstrumentBank`, so the MIDI path has *one* concept of
  "where instruments come from" rather than two parallel ones.
- **Sample enhancement belongs at extraction time.** M10's enhancement API (upscaling,
  spectral band replication, or a learned model for 8-bit Amiga samples) is defined as a
  load-time transform. Running it **once, during a library scan**, and storing the result
  in the bank is strictly better than running it per module load: the cost is paid once
  across every module that shares the sample, and after dedup that is a large multiplier.

## Deliverables when pulled

1. **`starplayer-library`** (std-only): directory scan, load via the existing format
   loaders, instrument extraction, content-hash dedup, and a manifest.
2. **`InstrumentBank`** — blob plus offsets, `Arc`-shareable, consumable by the `no_std`
   engine, produced by either a library scan or an SF2 import.
3. **A bank manifest format** carrying, per instrument: source module and its title, the
   original instrument name, the content hash, the tuning convention and any transpose
   offset, the synthesised-release setting, and the format whose `Instrument`
   implementation it runs under.
4. **MIDI addressing** — bank select plus program change onto stable content-derived IDs.
5. **Velocity and CC mapping**, with the IT decision from §4 documented.
6. **A browser/search surface** in whichever UI wants it — searching several thousand
   instruments by name, source module or format is the difference between a library and a
   pile.

## Exit criteria

A directory of tracker modules is scanned into a bank; a MIDI keyboard plays any
instrument in it with its own format's envelopes and articulation; the same instrument
keeps its ID after the source file is renamed.

## Out of scope

Editing instruments. Rendering the *songs* from the scanned library (that is just normal
playback). Any new synthesis method — this milestone adds no oscillators, only reach.
