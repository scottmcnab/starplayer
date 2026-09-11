# M8 — I2: Flash-resident modules — borrowed PCM and the module image

| Field | Value |
|---|---|
| Milestone | M8 ([master plan](M8-master-plan.md), decision 3; deliverable 2) |
| Status | Implemented 2026-09-11; awaiting review |
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

---

## Research resolution

### 1. Alignment of `include_bytes!`, and how many constructors

`include_bytes!` yields a `&'static [u8]` of alignment 1, so a firmware must place the
image behind a `#[repr(C, align(4))]` wrapper or in an aligned flash partition. The
recommendation in the task — "provide both, name them honestly" — landed as **three**
constructors, because fuzzing and I6's upload path both need a fourth behaviour the other
two cannot express (a non-`'static` slice):

| Constructor | Blob | PCM | Wants | Who calls it |
|---|---|---|---|---|
| `Module::from_image` | borrowed | borrowed, or `Err` | a 4-byte-aligned `&'static [u8]` | I3's firmware; it is the constructor that *proves* the alignment wrapper is there |
| `Module::from_image_or_copy` | borrowed | borrowed when it can be, else copied | any `&'static [u8]` | a flash partition whose alignment the caller does not control (I6) |
| `Module::from_image_copied` | copied | copied | any `&[u8]`, any alignment, any endianness | the `image` fuzz target, host tests, an uploaded image |

Alignment is checked twice over: the writer pads so the PCM's *offset within the image* is
a multiple of 4, and `from_image` checks the mapped *address* is too (`bytemuck`'s
`try_cast_slice` then re-checks `align_of::<i16>()` and the length). A misaligned image is
`Err(Invalid("a module image must be 4-byte aligned in memory to borrow its PCM"))` — never
a silent copy, which is the whole point of having the strict constructor at all.

Big-endian hosts are refused on the two borrowing constructors and supported on
`from_image_copied`, which decodes each frame from explicit little-endian bytes. Both ESP32
targets are little-endian, so nothing on the milestone's path takes that arm.

`starplayer-model` is `#![forbid(unsafe_code)]`, so the `&[u8]` → `&[i16]` borrow goes
through **bytemuck** (`try_cast_slice`), added as a workspace dependency. It is `no_std`
with no default features, is already in the lock file below `fixed`, and its checked cast
*is* the alignment test rather than an extra one.

### 2. 8-bit storage — what the PCM actually costs

Measured from `cargo xtask module-images` (the number each line reports):

| Fixture | Source module | Image | PCM in the image | PCM share | An `i8` storage would save |
|---|---|---|---|---|---|
| `synthetic-mod` (31 samples, 2 patterns) | 3 324 B | 6 072 B | 1 376 B | 22.7 % | 688 B (11 % of the image) |
| `synthetic-mtm` (2, 2) | 1 548 B | 2 312 B | 448 B | 19.4 % | 224 B (10 %) |
| `synthetic-xm` (2, 2) | 3 449 B | 3 564 B | 448 B | 12.6 % | 224 B (6 %) |
| `synthetic-it` (2, 1) | 1 797 B | 2 072 B | 448 B | 21.6 % | 224 B (11 %) |
| `petri-s3m` (5, 9) | 35 966 B | 88 036 B | 64 156 B | **72.9 %** | 32 078 B (36 %) |
| `reflex-s3m` (4, 10) | 9 634 B | 14 984 B | 4 480 B | 29.9 % | 2 240 B (15 %) |

The conclusion is that the share is a function of how much *music* a module has relative to
its *samples*, and that only a sample-heavy module makes an `i8` storage worth a second
mixer inner-loop instantiation. `PETRI.S3M` — the exit criterion's module — is the
sample-heavy case: 5 samples, 32 078 stored frames, and an image where nearly three
quarters is PCM. There an `i8` storage is a real 32 KB, or 0.8 % of a 4 MB flash. On the
synthetic fixtures it is noise. Not implemented, as the task says; the number is recorded
so I3's budget document can cite it.

Two measurements that came out of the same exercise and were *not* anticipated:

* **The pre-roll and the guard are free.** 16 frames per sample, so `PETRI`'s five samples
  cost 160 bytes of the 64 156.
* **The 120-entry note maps were not.** As first written, the image stored
  `note_sample_map` (240 B) and `note_transpose_map` (120 B) verbatim for every instrument,
  which made `synthetic-mod` — 31 instruments, 1 376 bytes of PCM — a **17 168-byte** image,
  65 % of it zeroes and an identity map. The format now writes one presence byte for each
  map and the array only when it differs from the default every MOD/S3M/MTM instrument
  carries. `synthetic-mod` fell to 6 072 bytes and `petri-s3m` to 88 036. That is a layout
  decision taken during implementation rather than in the task file, and it is the reason
  the deliverable's format table mentions the note maps at all.

### 3. `ModuleBuilder::reserve_pcm`

Added, since the builder was already being touched. `reserve_pcm(frames)` is an **exact**
reservation (`Vec::reserve_exact`) for a caller that knows the total before the first
`add_sample`, which is what keeps the peak of a load near the finished blob rather than
near twice it during the last doubling — K5a's point, and a real one on a heap-constrained
target.

`Module::enhanced` now calls it with the source module's PCM length. That is exact for an
identity rebuild and a **floor** for any other, because an enhancer may only raise a
sample's rate. The total-up-front version K5a imagined is still not possible: `enhanced`
cannot know what the enhancer will produce without running it, and running it twice is
worse than one reallocation.

### 4. Where `Hash` / `PartialEq` on `Module` are used

Confirmed, and all four sites stayed green with no change:

* `starplayer-model/src/module.rs` — the hash and equality tests, including
  `a_module_compares_equal_to_its_clone`;
* `starplayer-model/src/enhance.rs` — `an_identity_enhancer_rebuilds_an_equal_module`,
  K5a's load-bearing test;
* `starplayer-enhance/tests/module_rebuild.rs` — the same claim across all five formats,
  field by field (`pcm()`, `blob()`, `samples()`, then the whole module);
* `starplayer-s3m/tests/fixtures.rs` — fixture equality.

Nothing in the workspace puts a `Module` in a `HashMap` or a `HashSet`; `Hash` exists for
fingerprinting. The manual impls on the two storage enums are defined over the **slice
contents**, so an owned module and the borrowed module read back from its image compare
equal and hash alike — `crates/starplayer-offline/tests/module_image.rs` asserts exactly
that for all six fixtures, and asserts the identity rebuild of a *borrowed* module is an
equal *owned* one.

### Done differently from the task file, and why

1. **Three constructors rather than two** — research point 1 above.
2. **`to_image` is not feature-gated.** The task offered `image-write` "or unconditional if
   it costs nothing". It costs nothing: it is a plain `alloc` function with no new
   dependency, and a firmware that never calls it never links it. A feature would have cost
   every test and every tool a flag, and would have made `cargo test -p starplayer-model`
   skip the round-trip tests by default — which is the one thing the reader must not be
   allowed to do.
3. **The note maps are written compactly** — research point 2 above.
4. **The six images are the fuzz seeds, but generated rather than committed.**
   `embedded/assets/*.spmi` is a git-ignored build product, so `cargo xtask fuzz --seed`
   runs `cargo xtask module-images` and copies the result into the working corpus, exactly
   the way the pinned libxmp corpus is handled. Committing 130 KB of binaries that go stale
   the moment `IMAGE_VERSION` moves buys nothing; the pinned-stable half of the coverage is
   `crates/starplayer-offline/tests/module_image.rs`, which replays all six images on every
   commit.
5. **`golden_fixtures()` moved into `starplayer-offline`.** The six fixtures were a private
   list inside `starplayer-goldens`; the image writer needs the same six, and two copies of
   that list would drift. The goldens binary keeps its own per-fixture *kernel* mapping,
   which is its business alone.
6. **`render_loaded_fixed_mono` / `canonical_sha256_of_loaded`.** The golden render path
   took `(format, bytes)` and loaded internally, so there was no way to hash a module that
   arrived by another route. Both now delegate to one `render_golden` that takes a
   `Module`; nothing about the render changed, and the committed goldens are byte-identical
   across the refactor.
7. **`cargo xtask module-image` shares the golden driver's target directory.** Same package,
   same features, so a dedicated one would have meant a second several-hundred-megabyte
   build tree for no isolation that matters.
