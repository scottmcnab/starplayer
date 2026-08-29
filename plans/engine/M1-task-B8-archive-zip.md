# M1-task-B8 — Zip archives in the web player (`starplayer-archive`)

| Field | Value |
|---|---|
| Milestone | M1 follow-up ([master plan](M1-master-plan.md)) |
| Depends on | B7 (web player) |
| Blocks | — (M3 CLI/TUI reuse the crate) |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests + headless browser) + **owner (drops a real zip)** |

## Context for a fresh agent

Read `AGENTS.md` first (working agreements: full variable names, compact formatting,
one-line asserts, no hand-written `unsafe`, no `cargo fmt`, never `git add -A`). The web
player landed in [B7](complete/M1-task-B7-web-player.md); read
`apps/starplayer-web/README.md`, `apps/starplayer-web/src/lib.rs`,
`apps/starplayer-web/www/app.js` (`loadBuffer`, `activateModule`, `readMetadata`) and
`apps/starplayer-web/test/headless.mjs` before starting. `cargo xtask ci` is green and
must stay green; `cargo xtask wasm` builds both wasm instances.

## Context

Modules are almost always distributed zipped (The Mod Archive, competition packs — the
owner's own `Documents/Projects/music/MC4/m4v-*.zip` are exactly this). The web player
currently accepts only a bare `.s3m` from the file picker, drag-and-drop or a URL. The
owner asked for `.zip` support and for a public Rust crate to do it, so the CLI/TUI in
M3 get it for free.

Decisions taken with the owner:
- **Multiple modules in one archive → a picker**; a single module loads straight away;
  non-module entries are ignored.
- **A reusable Rust crate**, not JavaScript: new std crate `starplayer-archive` wrapping
  the `zip` crate, compiled into the *page-side* wasm instance (`starplayer-web`).

The worklet never sees a zip: extraction happens on the page thread in the instance that
already validates untrusted bytes (`apps/starplayer-web/src/lib.rs` `inspect`), and the
extracted module bytes then follow the existing path unchanged (`activateModule` →
transferred `ArrayBuffer` → worklet `load_module`). A bad archive therefore cannot
disturb the audio graph, same as a bad module today.

## Design

### 1. `crates/starplayer-archive` (std, new)

```
crates/starplayer-archive/
  Cargo.toml     starplayer-model (for probes later), zip, flate2
  src/lib.rs     #![forbid(unsafe_code)]; the API below
  tests/         a tiny zip built in-test with the zip crate's writer (no binary fixtures)
```

Dependencies, pinned in the workspace `[workspace.dependencies]`:
- `zip = { version = "6", default-features = false, features = ["deflate-flate2"] }`
- `flate2 = { version = "1", default-features = false, features = ["rust_backend"] }`
  (selects miniz_oxide — pure Rust, no C, builds for `wasm32-unknown-unknown`).
  `zip`'s own `deflate` feature pulls in zopfli (a *compressor*) and zlib-rs; neither is
  wanted. Stored (method 0) and Deflate (method 8) are the only methods a module zip
  ever uses; anything else is reported as unsupported, not silently skipped.
  **Verify at implementation** that this feature pair builds for wasm32 with no
  `getrandom`/`time` in the graph (`cargo tree -p starplayer-archive --target
  wasm32-unknown-unknown`); if `zip` 6 needs a different spelling, use it and record why.

API (small, format-agnostic, extension-driven now, probe-driven when MOD/MTM land):

```rust
pub const MAX_ENTRY_BYTES: u64 = 64 << 20;     // zip-bomb guard; a module is never this big

pub fn is_zip(bytes: &[u8]) -> bool;           // "PK\x03\x04" (or empty-archive "PK\x05\x06")

pub struct ArchiveEntry { pub index: usize, pub name: String, pub size: u64, pub format: ModuleFormat }

/// Every entry that looks like a playable module, in archive order.
/// Directories, resource forks (`__MACOSX/`), and unknown extensions are skipped.
pub fn list_modules(bytes: &[u8]) -> Result<Vec<ArchiveEntry>, ArchiveError>;

/// Inflate one entry, refusing anything above MAX_ENTRY_BYTES *before* inflating
/// (from the central directory's uncompressed size) and again if the stream lies.
pub fn extract(bytes: &[u8], index: usize) -> Result<Vec<u8>, ArchiveError>;

pub enum ArchiveError { NotAnArchive, Corrupt(String), UnsupportedMethod(String), TooLarge { name, size }, NoModules, NoSuchEntry }
```

`ModuleFormat` is `starplayer_model::ModuleFormat`; the extension → format table lives
here (`.s3m` → `S3m`, `.mod`/`.mtm`/`.xm`/`.it` declared now so the CLI's autodetect can
use the same list, but the web player only *offers* `.s3m` until M2 lands — filter on
the JS side by what `starplayer::s3m::probe` accepts).

Add the crate to the std group in `plans/product/01-technical-architecture.md` §11
(`starplayer-archive  zip (later: lha/rar?) container support → model, zip`) and to
`xtask`'s wasm-build job? Not needed: `starplayer-web` already builds for wasm and pulls
it in. It is *not* a `no_std` crate and must not be added to `NO_STD_CRATES`.

### 2. Page-side wasm (`apps/starplayer-web/src/lib.rs`)

New exports next to `inspect_s3m`:

```rust
pub fn is_archive(bytes: &[u8]) -> bool
pub fn archive_modules(bytes: &[u8]) -> Result<String, JsValue>   // "index\tname\tsize\n" per module entry
pub fn archive_extract(bytes: &[u8], index: u32) -> Result<Vec<u8>, JsValue>
```

Line-record strings, like `effect_names()` already does — no serde, no JSON dependency.
`Cargo.toml` gains `starplayer-archive = { workspace = true }`.

### 3. The page (`apps/starplayer-web/www/app.js`, `index.html`, `style.css`)

`loadBuffer(buffer, label)` grows a front half:

```
if Loader.is_archive(bytes):
    entries = Loader.archive_modules(bytes)         // error → readable message, nothing changes
    if entries.length == 0  → error "no S3M inside <zip>"
    if entries.length == 1  → buffer = Loader.archive_extract(bytes, entries[0].index); label = "<entry> (from <zip>)"
    else                    → show the picker; on choice, extract and continue; on dismiss, nothing changes
```

then the existing `inspect_s3m` → metadata → `activateModule` path, untouched.

- The picker: a `<dialog>`-free inline panel (keep the page framework-free) listing
  `name — size`, with Load/Cancel; keyboard-navigable; reuses the notice styling.
  While it is open the current module keeps playing.
- File picker `accept=".s3m,.zip,audio/s3m,application/zip"`; drop zone text and the
  status message mention zips; URL loading works unchanged (bytes are bytes).
- The `README.md` architecture section gets a paragraph: archives are opened on the
  page thread, the worklet only ever receives module bytes.

### 4. Tests

- `starplayer-archive` unit tests: build archives in-memory with `zip::ZipWriter`
  (stored and deflated), then assert: listing finds `.s3m` entries case-insensitively and
  skips `__MACOSX/._x.s3m`, directories and `readme.txt`; `extract` round-trips the bytes;
  an entry whose declared size exceeds `MAX_ENTRY_BYTES` is refused *without* inflating;
  a truncated archive is `Corrupt`, not a panic; `is_zip` on an S3M is false.
- `starplayer-web` unit test: `archive_modules` record format on an in-memory zip of
  `REFLEX.S3M` + `readme.txt`.
- `apps/starplayer-web/test/headless.mjs`: add a scenario that zips `REFLEX.S3M` in the
  harness (Node's `zlib.deflateRawSync` + a hand-built local header/central directory,
  ~40 lines, or reuse the archive crate via a tiny `xtask`-built fixture — prefer the
  Node builder so the test is independent of the crate under test), loads it through the
  file path, asserts it plays and the label reads `REFLEX.S3M (from reflex.zip)`; and a
  two-module zip that shows the picker, picks the second, and plays it.
- `cargo xtask ci` stays green: `starplayer-web` is already in the wasm-build job.

## Files

- new `crates/starplayer-archive/{Cargo.toml,src/lib.rs,tests/archive.rs}`
- `Cargo.toml` (workspace deps: `zip`, `flate2`, `starplayer-archive`)
- `apps/starplayer-web/{Cargo.toml,src/lib.rs,www/app.js,www/index.html,www/style.css,README.md,test/headless.mjs}`
- `plans/product/01-technical-architecture.md` §11 (one line in the std group)

## Verification

1. `cargo xtask ci` — 5/5.
2. `cargo tree -p starplayer-archive --target wasm32-unknown-unknown --edges normal` shows
   `zip`, `flate2`, `miniz_oxide`, `crc32fast` and nothing platform-bound.
3. `cargo xtask wasm && cargo xtask serve --host 0.0.0.0 --tls --tls-san 192.168.0.202,192.168.0.210,172.31.34.119`;
   in the browser drop one of the owner's `MC4/m4v-*.zip` files (they hold single modules)
   → plays with the "(from …zip)" label; a zip with several `.s3m` → picker → chosen one
   plays; a zip with no modules → readable error, current song keeps playing.
4. `node apps/starplayer-web/test/headless.mjs` — new zip scenarios pass.

## Out of scope

Nested archives; LHA/RAR/7z (the 1990s' other containers — a follow-up if wanted, the
crate name is deliberately not `-zip`); loading from the worklet instance; MOD/MTM entries
(offered automatically once M2's loaders exist and `probe` accepts them).
