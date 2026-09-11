# M8 — I2: Flash-resident modules — borrowed PCM and the module image

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md), decision 3; deliverable 2) |
| Status | Planned 2026-09-11; pulled |
| Depends on | M1-B1 (landed): the `Module` blob-and-offsets layout; M10-K5a (landed): `Module::enhanced` rebuild, which must keep working |
| Blocks | I3 |
| Parallel with | I1 |
| Recommended model | Claude Opus (a change to the model crate's one load-bearing struct, a new on-disk format with a validating reader, and a determinism proof across it) |
| Verified by | agent (round-trip tests, goldens byte-identical through an image, fuzz seed for the reader, `no-std-check`), then I3 on hardware |

## Context for a fresh agent

Architecture §6 chose **offsets, not references** for `Module` — two owned blobs plus
`u32` index tables — and listed "mmap- and flash-friendly on ESP32, where sample data may
be borrowed rather than owned" among the reasons. Nothing cashes that in yet:
`crates/starplayer-model/src/module.rs:52` holds `pcm: Box<[i16]>`, and every loader
widens 8-bit file samples to `i16` through `ModuleBuilder::add_sample`
(`crates/starplayer-model/src/builder.rs:83`), appending `PRE_ROLL_FRAMES ‖ body ‖
GUARD_FRAMES`. On a desktop that is the right shape. On a 4 MB-flash ESP32 with a few
hundred KB of DRAM it means every module is copied — and doubled — into RAM.

The fix is not to make the *loaders* run from flash; it is to make the **built module**
live there. The loader runs on the development machine, the finished `Module` is written
out as an image whose PCM is 4-byte aligned, the image goes into the firmware (or a flash
partition), and the device constructs a `Module` that **borrows** the PCM and the pattern
blob from the memory-mapped image. That keeps the loaders untouched, keeps `Module`'s
index tables exactly as they are, and gives the device a load that is a validation pass
and four small allocations.

Modules uploaded at run time (I6) still go through the ordinary loader into the PSRAM
heap. Both paths produce the same `Module`; only the storage differs.

### Code you must read before changing anything

- `crates/starplayer-model/src/module.rs` — `Module`, `from_parts`, `pcm`, `sample_pcm`,
  the `assert_send_sync`, and the "Nothing here panics" rule.
- `crates/starplayer-model/src/builder.rs` — `ModuleBuilder::build` and its validation;
  `add_sample`'s pre-roll and guard layout. The image reader must validate to the same
  standard, because the engine reads a `Module` inside `render()`.
- `crates/starplayer-model/src/{sample,pattern,instrument,header}.rs` — the index types
  the image serialises. Note which contain `Box`/`Vec` (`ModuleHeader::format_data`,
  instrument envelopes) — those are small and stay owned.
- `crates/starplayer-model/src/enhance.rs` and `Module::enhanced` — a rebuild that must
  still produce an **equal** module for an identity enhancer, whatever the storage.
- `crates/starplayer-model/src/reader.rs` — `ModuleReader`, `slice_at` (the zero-copy
  fast path over file bytes; the image reader uses the same discipline).
- `crates/starplayer-offline/src/lib.rs` — `canonical_sha256_with`, `render_with_kernel`;
  `crates/starplayer-offline/src/bin/starplayer-goldens.rs` — the six fixtures.
- `fuzz/` — the loader targets and seed layout; the image reader gets one.
- `xtask/src/main.rs` — command dispatch, to add `module-image`.
- `plans/product/01-technical-architecture.md` §6 and §10.

## Deliverables

### 1. `PcmStorage` in `starplayer-model`

```rust
pub enum PcmStorage {
    Owned(Box<[i16]>),
    Borrowed(&'static [i16]),
}
pub enum BlobStorage { Owned(Box<[u8]>), Borrowed(&'static [u8]) }
```

`Module.pcm: PcmStorage`, `Module.blob: BlobStorage`. `Deref` to the slice; manual
`Clone` (a clone of a borrowed module stays borrowed), `PartialEq`/`Eq`/`Hash` over the
slice contents so `Module` keeps deriving them and the identity-enhancer equality test in
K5a still holds. `Send + Sync` stays a compile-time assertion. `Module::pcm()` and every
accessor keep their signatures. `from_parts` takes the storage enums; `ModuleBuilder`
always builds `Owned`.

`'static` is the right lifetime: a flash-mapped image lives for the program, and a
non-static borrow would put a lifetime on `Module`, on `Arc<Module>` and on every engine
type. Say so in the doc comment.

### 2. The module image (`starplayer-model/src/image.rs`, no_std)

A flat little-endian format:

```text
magic "SPMI", format version u16, flags u16,
header (ModuleHeader serialised; format_data length-prefixed),
counts: samples, patterns, orders, instruments,
index tables (SampleIndex / PatternIndex / u16 orders / InstrumentDef with envelopes),
blob length, blob bytes,
padding to a 4-byte boundary,
pcm length (frames), pcm i16 little-endian
```

- `Module::from_image(image: &'static [u8]) -> Result<Module, Error>` — validates magic,
  version, every length and offset against the slice, then runs the **same** invariant
  checks `ModuleBuilder::build` runs (factor them into a shared `validate` if they are
  not already), and borrows `blob` and `pcm` in place. It must reject an image whose PCM
  is not 4-byte aligned in memory rather than copy it (research point 1). Nothing panics.
- `Module::to_image(&self) -> Vec<u8>` behind a new `alloc`-only feature `image-write` (or
  unconditional if it costs nothing — the reader must stay in the default build).
- On-disk endianness is little-endian on every host; the Xtensa and RISC-V targets are
  little-endian, so the borrowed `i16` slice is read directly. A big-endian host would
  need a copy; state that this is deliberately unsupported.

### 3. `cargo xtask module-image <module> <out.spmi>`

Loads with `starplayer::load`, writes `to_image`. Plus `cargo xtask module-images` that
produces images of the six golden fixtures (the four synthetic ones via
`starplayer_offline::fixtures`, plus `PETRI.S3M` and `REFLEX.S3M`) into
`embedded/assets/` — a **build product**, `.gitignore`d, that I3's firmware includes with
`include_bytes!`. The two committed S3M fixtures are the owner's own compositions and are
fine to embed.

### 4. Proof

- Round trip: for every fixture, `load(bytes)` equals `from_image(leak(to_image(load(bytes))))`
  (leak in the test to obtain `'static`), and the borrowed module's golden digest equals
  the owned one's — that is, `goldens/` is byte-identical through an image.
- `Module::enhanced` over a borrowed module produces an owned, equal module for the
  identity enhancer.
- A fuzz target `image` under `fuzz/` with the six images as seeds; the reader never
  panics on arbitrary bytes (the fuzz smoke job covers it).
- Misalignment is an `Err`, not a copy.
- `cargo xtask ci --job no-std-check` with the new code in the default build.

### 5. Documentation

Architecture §6: a paragraph on `PcmStorage` and the image, replacing "may be borrowed"
with what is now true. The crate doc for `image.rs` carries the format table above and
the version-bump rule (any layout change bumps the version; the reader rejects unknown
versions).

## Research points

1. **Alignment of `include_bytes!` on Xtensa and RISC-V.** A `&'static [u8]` from
   `include_bytes!` has alignment 1; the firmware must wrap it in a `#[repr(C, align(4))]`
   struct (the standard trick) or place the image in an aligned flash partition. Decide
   whether `from_image` should also accept an unaligned image by *copying only the PCM*
   into the heap as a documented fallback (`from_image_or_copy`), which I6's flash-partition
   path may want. Recommend: provide both, name them honestly.
2. **8-bit storage.** An `i8` PCM storage would halve the flash cost of MOD/S3M samples
   but needs a second mixer inner-loop instantiation. Measure how much of each fixture's
   image is PCM and record it; do not implement.
3. **`ModuleBuilder::reserve_pcm`** (K5a's Research resolution asked for it for embedded
   hosts). If it falls out naturally while touching the builder, add it; otherwise leave
   the note.
4. **Where `Hash`/`PartialEq` are used** on `Module` (goldens fingerprinting, K5a's
   equality test) — confirm the manual impls keep those tests green without changes.

## Verification

```text
cargo test -p starplayer-model
cargo test -p starplayer-offline
cargo xtask module-images && ls embedded/assets/*.spmi
cargo xtask ci --job goldens
cargo xtask ci --job no-std-check
cargo xtask ci --job no-std-purity
cargo xtask ci --job fuzz-smoke        # nightly; the new target's seeds
cargo clippy --workspace --all-targets -- -D warnings
```

## Out of scope

Running any loader on the device (I6 does that through the ordinary `starplayer::load`,
into the heap). An `i8` storage. Compression of the image. Changing the loaders.
