# M9 — J4: VST3 via clap-wrapper — **owner-gated**

| Field | Value |
|---|---|
| Milestone | M9 ([master plan](M9-master-plan.md), "The task graph" section) |
| Status | Planned; **do not start until the owner has decided the licensing question below** |
| Depends on | J2 (a plugin with state), the owner's decision |
| Blocks | — |
| Parallel with | — |
| Recommended model | Claude Sonnet (a build recipe and CI job around an external C++ project; no engine code) |
| Verified by | agent (the wrapper builds and its VST3 validator passes), then owner in a VST3 host |

## The decision the owner has to make first

VST3 has two licences: **GPLv3**, or Steinberg's proprietary VST3 licence agreement. A
binary that links the VST3 SDK ships under one of them. StarPlayer's own licence is
deferred (`plans/product/00-vision.md` decision 7, "revisit before going public"), and this
is the first task whose output constrains that decision: a GPL VST3 build of a
not-yet-licensed private engine is fine for the owner's own DAW, but not something to
publish without settling the engine's licence. The Rust route (`vst3-sys`) is GPLv3 as
well, so there is no licence-neutral way to a VST3.

Master-plan decision 6 chooses the route that touches the SDK least: **`free-audio/clap-wrapper`**,
a C++ project that wraps an existing CLAP binary as VST3 (and AUv2), MIT-licensed itself,
building against the VST3 SDK it fetches. No Rust code links the SDK; the wrapper does.

## Context for a fresh agent

J1/J2 produced `starplayer.clap`. clap-wrapper takes that binary and produces
`starplayer.vst3` whose `process` forwards to the CLAP plugin — every feature, parameter
and state blob is the CLAP one. There is nothing to reimplement; the work is a
reproducible build and a place for it in `xtask` and CI.

### Read first

- `crates/starplayer-clap/README.md`, `xtask/src/main.rs` (`xtask clap`, the `openmpt` build
  recipe in `xtask perceptual` is the precedent for building a pinned external source tree
  into `target/`).
- clap-wrapper's README and its CMake options (`CLAP_WRAPPER_OUTPUT_NAME`,
  `CLAP_WRAPPER_DOWNLOAD_DEPENDENCIES`, the VST3 SDK pin).

## Deliverables

1. **`xtask vst3`**: fetches a checksum-pinned clap-wrapper source tarball into
   `target/downloads/`, builds it with CMake (`cmake` and a C++17 compiler are required;
   the task states the versions; no root), pointing it at `target/clap/starplayer.clap`,
   and produces `target/vst3/starplayer.vst3`. `--install` copies it to `~/.vst3/` on
   Linux. Idempotent, like `xtask perceptual`'s openmpt step.
2. **CI**: an **opt-in** job `vst3-build` (not in the default `xtask ci` list, like
   `perceptual`), because it needs CMake and a network fetch. It runs Steinberg's
   `validator` from the SDK the wrapper fetched, and fails on any error.
3. **Licence flag**: `plans/product/00-vision.md` decision 7 gains a sentence recording
   that a VST3 build exists, its licence consequence, and that the built artefact is
   never committed. `plans/README.md` M9 row says "VST3 build available, owner-gated".
4. **AUv2** comes with the same wrapper on macOS; note in the resolution whether it was
   tried (it cannot be on the WSL2 box) and leave it there.

## Research points

1. The clap-wrapper release to pin, the VST3 SDK version it pulls, and whether its fetch
   can be redirected to a pinned tarball for reproducibility (it can; say how).
2. Whether the wrapper needs the CLAP's `clap.state` (yes: VST3 state is delegated) and
   `clap.params` (yes) — J2 must have landed.
3. What the VST3 validator complains about with a CLAP-wrapped plugin in practice
   (bus arrangements are the usual one); fix on the CLAP side if it is ours.

## Verification

```
cargo xtask vst3
<sdk>/validator target/vst3/starplayer.vst3
cargo xtask ci --job vst3-build
```

## Out of scope

Any Rust VST3 binding; AU on this machine; LV2; changing the engine's licence — that is
the owner's decision, recorded elsewhere.
