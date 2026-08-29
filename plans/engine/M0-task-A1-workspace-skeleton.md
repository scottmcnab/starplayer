# M0-task-A1 — Workspace skeleton, xtask and CI

| Field | Value |
|---|---|
| Milestone | M0 ([master plan](M0-master-plan.md)) |
| Depends on | — |
| Blocks | every other task in the project |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (CI matrix green) |

## Context for a fresh agent

This repository currently contains only `STARPLAY/` (read-only 1990s DOS assembly
sources), `AGENTS.md` and `plans/`. There is no Rust code yet. This task creates the
workspace that everything else is built in.

Read `plans/product/01-technical-architecture.md` §11 for the crate layout and feature
flags — it is authoritative and this task implements it literally. Read `AGENTS.md` for
the non-negotiable design goals; several of them are CI checks created here.

Installed on the dev machine: Rust 1.97, `riscv32imc-unknown-none-elf` (and other
riscv32 targets), Node 24. **Not** installed: `wasm32-unknown-unknown`,
`wasm-bindgen-cli`, `wasm-pack`. Adding the wasm target is part of this task.

## Deliverables

1. **Virtual workspace** at the repo root: `Cargo.toml` with `resolver = "2"`, members
   covering `crates/*`, `apps/*` and `xtask`. Shared `[workspace.package]` metadata
   (edition, rust-version) and a `[workspace.dependencies]` table so versions are pinned
   once. **No `license` field and no SPDX headers** — licensing is deliberately deferred
   (`plans/product/00-vision.md` decision 7).

2. **Crate stubs**, exactly the set in architecture §11. Each `no_std` crate starts with:
   ```rust
   #![no_std]
   #![forbid(unsafe_code)]
   extern crate alloc;
   ```
   `starplayer-engine` and `starplayer-mixer` additionally carry
   `#![deny(clippy::indexing_slicing, clippy::unwrap_used, clippy::panic)]`.
   A stub is a `lib.rs` with the crate's doc comment stating its responsibility and its
   allowed dependency edges — not empty, but no logic.

3. **Feature flags** per architecture §11.1, wired through the dependency graph. The hard
   invariant: **no default feature transitively enables `std`.** Add a CI check that
   proves it (see deliverable 5).

4. **`xtask`** with subcommands stubbed and `ci` implemented: `xtask ci` runs the whole
   matrix locally so an agent can verify without pushing. Later milestones add
   `xtask goldens` and `xtask wasm`.

5. **CI** (GitHub Actions) with these jobs, all required:
   | Job | Command |
   |---|---|
   | host tests | `cargo test --workspace` |
   | wasm build | `cargo build --target wasm32-unknown-unknown -p starplayer` |
   | no_std check | `cargo check --target riscv32imc-unknown-none-elf --no-default-features -p <each no_std crate>` |
   | clippy | `cargo clippy --workspace --all-targets -- -D warnings` |
   | no-std purity | build each `no_std` crate with default features and assert `std` is absent from the dependency graph (`cargo tree -e features` grep, or a `build.rs`-free compile against the bare-metal target, which fails if `std` is pulled in) |

6. **`rust-toolchain.toml`** pinning the channel and listing
   `wasm32-unknown-unknown` and `riscv32imc-unknown-none-elf` in `targets`, so the
   toolchain installs them automatically.

7. **`.gitignore`** additions for `/target` and wasm build output. Note the repo-root
   `.gitignore` comment convention: component-specific ignores live in each component's
   own `.gitignore`.

## Research points

1. The cleanest way to assert "no `std` in the graph" in CI. Compiling for a bare-metal
   target is the strongest signal, since `std` is unavailable there; confirm it catches
   a deliberately-introduced `std` dependency before relying on it.
2. Whether `cargo clippy` on the bare-metal target is worth adding, or whether host
   clippy plus the bare-metal `check` is sufficient coverage.

## Verification

- `cargo xtask ci` passes locally.
- Deliberately add `use std::vec::Vec;` to `starplayer-core`, confirm the no_std check
  **fails**, then revert. This proves the guard works rather than merely existing.
- `cargo tree -p starplayer --no-default-features` shows no `std`-only dependency.

## Out of scope

Any real logic in any crate. Any app content beyond a stub `main`. The AudioWorklet
plumbing (that is A4).
