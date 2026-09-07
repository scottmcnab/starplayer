# StarPlayer

A reusable Rust tracked-music engine: a modern reimplementation of StarPlayer, the
1990s DOS MOD/S3M/MTM player, generalised into a real-time, event-driven synthesis
engine. It plays MOD, S3M, MTM, XM and IT; runs in the browser via WebAssembly, on
native desktop, and on `no_std` embedded targets; and is built to be embedded in other
projects rather than to be one application.

## The original

StarPlayer was written by Scott McNab ("Jedi" of Oxygen) in 80386 assembly between 1994
and 1996. Both of the original public releases are still on the Hornet Archive:

* **StarPlay 2.25** (February 1996), the binary release —
  [hornet.org](https://www.hornet.org/music/programs/players/starp225.zip) ·
  [scene.org mirror](https://files.scene.org/view/mirrors/hornet/music/programs/players/starp225.zip)
* **StarPlayer 2.25s source** (December 1996), the complete 32-bit pmode assembly —
  [hornet.org](https://www.hornet.org/code/audio/players/sp-code.zip) ·
  [scene.org mirror](https://files.scene.org/view/mirrors/hornet/code/audio/players/sp-code.zip)
* Pouët: [StarPlayer 2.25 by Oxygen](https://www.pouet.net/prod.php?which=73142)

The 2.25s source is checked in unchanged as `STARPLAY-2.25s/`, alongside `STARPLAY/`, a
later unfinished rewrite. Both are read-only references and the specification for the
MOD/S3M/MTM effect semantics this engine reproduces.

## Layout

```
crates/     the engine, one crate per format loader, hosts, telemetry, test kit
apps/       starplayer-web (browser player), starplayer-cli, starplayer-tui
xtask/      build orchestration: CI jobs, wasm packaging, golden regeneration
fuzz/       cargo-fuzz loader targets (nightly)
plans/      design documents and implementation plans — start at plans/README.md
```

## Building

The toolchain is pinned by `rust-toolchain.toml`. Native tests need the ALSA headers
(`libasound2-dev` on Debian/Ubuntu).

```text
cargo test --workspace          # unit, golden and conformance tests
cargo xtask ci                  # every CI job, as GitHub Actions runs them
cargo xtask wasm && cargo xtask serve   # the web player at http://localhost:8080/
```

See `apps/starplayer-web/README.md` for the browser player and its GitHub Pages deploy.

## Licence

The code in this repository is licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
* MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

Two things are **not** under that licence:

* **The original DOS sources** in `STARPLAY/` and `STARPLAY-2.25s/` are Scott McNab's
  1994–96 work and stay under the terms in `STARPLAY-2.25s/SP-CODE.DOC`: free to use in
  your own productions, with a word in the credits.
* **The S3M test fixtures** in `crates/starplayer-s3m/tests/fixtures/` are music, not
  code, and are licensed under
  [CC BY-NC-ND 4.0](https://creativecommons.org/licenses/by-nc-nd/4.0/). See the README
  in that directory.
