# M2-task-C6 — The fixed-point mixer and golden hashes

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Depends on | M1-B5 (float mixer) |
| Blocks | M2 exit; M8 (embedded) |
| Parallel with | C1, C3, C4 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (cross-target hash equality in CI) |

## Context for a fresh agent

The float mixer is the default for desktop and browser. The **fixed-point mixer is the
canonical bit-exact reference** (`plans/product/01-technical-architecture.md` §7.3) and
the embedded path (M8).

Why bit-exactness needs the fixed path: x86 SSE2, ARM NEON and WASM SIMD agree on
`+ - * /` under IEEE-754 round-to-nearest, but they do **not** agree on `sin`, `exp` or
`powf` — different libm — and FMA contraction changes results. Integer arithmetic has no
such freedom. So goldens are hashed on the fixed path and floats carry a tolerance.

Note the corollary that has already shaped the design: **no transcendental functions
anywhere in the RT path**, on either mixer. Tables only.

## Deliverables

1. **The fixed-point voice kernel** — `i16` sample, `i32`/`i64` accumulator, Q32.32 step,
   nearest and linear interpolation. Structurally identical to the float kernel; the
   difference is the accumulator type, so the two should share their shape through
   generics rather than being two hand-written loops that drift apart.

2. **Fixed-point panning, ramping and output conversion**, matching the float path's
   *behaviour* (not its bits). Table-driven pan law, same ramp lengths.

3. **Disabled FMA contraction** for the float path, so the two paths' relationship is
   stable across compilers. Document how (a build flag, or `#[inline(never)]` boundaries)
   and verify it actually took effect.

4. **Golden hashes**: SHA-256 of a fixed-point **i16 mono 44100 Hz linear-interpolation**
   render with DSP bypassed. The filename encodes the configuration:
   ```
   goldens/s3m/<module>__i16_mono_44100_linear.sha256
   ```
   so changing the interpolator produces a *visibly new* golden rather than silently
   breaking every existing one. **Golden WAVs stay out of the repo** — only hashes are
   committed.

5. **`cargo xtask goldens`** to regenerate, with a `--check` mode for CI.

6. **Cross-target hash equality in CI**: render the same module on x86-64, aarch64 and
   wasm32 on the fixed path; assert identical hashes. This is the test that keeps the
   determinism claim honest.

7. **A float-path tolerance comparison** against the fixed path — not bit-exact, but
   within a stated segmental SNR — so a float regression that the goldens cannot see
   still gets caught.

## Research points

1. Whether aarch64 CI is available without hardware (qemu, or a hosted ARM runner). If
   neither is practical, degrade to x86-64 plus wasm32 and record the gap rather than
   quietly dropping the test.
2. The right rounding convention in the fixed accumulator. Truncation is cheapest;
   round-to-nearest is better sounding. Whichever is chosen becomes part of the golden
   contract, so choose deliberately and document it.
3. Whether module *loading* is deterministic enough to hash the `Module` itself as a
   cheaper first-line check. B1 made `Module` hashable for exactly this.

## Research resolution and implementation contract

- **Shared kernel:** M0/M1 had already landed one `accumulate_voice<Path, Interp>` loop
  with real `FloatPath` and `FixedPath` implementations. C6 keeps that shape and changes
  the fixed arithmetic behind the existing implementation; no new trait or duplicate
  voice loop is introduced.
- **Rounding:** every signed fixed-path signal reduction now rounds to nearest, with
  exact ties away from zero. This covers Q0.32 linear interpolation, Q15 voice gain,
  master volume, limiter interpolation, mono fold-down and reduced-depth output. Pan-law
  table selection keeps its pre-existing endpoint flooring so hard pan still makes the
  far channel exactly silent. Ten seconds (441,000 frames) from each fixture form the
  bounded golden segment; each `i16` is hashed in explicit little-endian order.
- **FMA:** `.cargo/config.toml` passes LLVM `-fp-contract=off` for every profile and
  target. `cargo xtask ci --job fma-check` compiles the actual float offline renderer
  with x86 FMA available, then rejects LLVM `contract` flags, `llvm.fma`/`llvm.fmuladd`
  intrinsics and fused machine mnemonics while requiring ordinary float multiplies in
  both outputs.
- **ARM64 CI:** GitHub's hosted-runner reference lists the native
  `ubuntu-24.04-arm` label, so the committed workflow uses it rather than QEMU:
  <https://docs.github.com/en/actions/reference/runners/github-hosted-runners>.
- **WASM CI:** Rust documents `wasm32-wasip1` as a Tier-2 target with a self-contained
  sysroot and the same default WebAssembly features as `wasm32-unknown-unknown`:
  <https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip1.html>. The official
  Bytecode Alliance setup action installs Wasmtime, whose documented `--dir=.` capability
  exposes the committed hashes to `--check`:
  <https://github.com/bytecodealliance/actions>,
  <https://docs.wasmtime.dev/cli-options.html>. The workflow pins the patched 46.0.2
  release: <https://github.com/bytecodealliance/wasmtime/releases/tag/v46.0.2>. This
  executes the integer renderer as WebAssembly; it is not a browser/JS ABI test. The
  existing `wasm32-unknown-unknown` build job remains the browser compile check.
- **Float tolerance:** average segmental SNR uses 1,024-frame windows, excludes segments
  below −80 dBFS RMS, caps exact matches at 120 dB, and gates at 60 dB. The five-fixture
  corpus measures 71.03–85.81 dB with the C6 rounding contract.
- **Module hashing:** `Module` remains deterministically `Hash`, but an additional module
  digest would only repeat loader coverage and cannot detect mixer regressions. The audio
  SHA-256 is retained as the required first-line golden.
- **Honest local gap:** the development host is x86-64 and has no native ARM64 hardware
  or Wasmtime. Native x86 results are locally verified, and the WASIp1 checker was also
  executed locally under Node's WASI runtime. The checked-in native ARM64 runner and
  pinned Wasmtime job remain the authoritative executable checks for those environments,
  rather than treating cross-compilation as proof of execution.

## Verification

- The same module renders to the same hash on two runs, at six different host block sizes.
- The same module renders to the same hash on x86-64 and wasm32 (and aarch64 if
  available).
- The float path stays within the stated SNR of the fixed path on the whole corpus.
- Changing the interpolator produces a differently-named golden, and `--check` reports a
  missing golden rather than a mismatch.
- The fixed path allocates nothing and produces no NaN/Inf (trivially, but assert it).

## Post-landing notes — 2026-09-02 branch review

**The rounding contract breaks bit-compatibility with M1, and nothing marked it.** C6
switched the fixed path from truncation to round-to-nearest with ties away from zero
(`crates/starplayer-mixer/src/path.rs:150-155`,
`crates/starplayer-dsp/src/interpolate.rs:94-99`,
`crates/starplayer-mixer/src/master.rs:143` and `:190`,
`crates/starplayer-mixer/src/output.rs:179` and `:311`). That was the deliberate outcome of
research point 2 — but **fixed-path S3M output now differs from M1's at the least
significant bit**, and because the S3M goldens were generated *after* the change, no
committed artefact records the break. Stated here so a future comparison against an M1
render is not mistaken for a regression.

**`RUSTFLAGS` discards `[build] rustflags`.** `.cargo/config.toml` carries
`rustflags = ["-C", "llvm-args=-fp-contract=off"]`. Cargo **replaces** rather than merges
when the `RUSTFLAGS` environment variable is set, so any CI job or developer shell that
exports `RUSTFLAGS` silently loses the FMA policy. Nothing in CI sets it today; an
assertion that it is unset is tracked by
[M2-task-C6a](../M2-task-C6a-golden-and-build-hygiene.md) deliverable 8.

**Three gaps in what the deliverables claim, all tracked by C6a:**

- Deliverable 3's "verify it actually took effect": `cargo xtask ci --job fma-check`
  (`xtask/src/main.rs:441-506`) compiles and scans only `starplayer-offline`'s own
  codegen — trailing `rustc` arguments and `--emit` apply to the final crate only — so the
  non-generic float master bus in `starplayer-mixer` and `starplayer-dsp` is never compiled
  with `+fma` and never inspected. It also has no negative control, and Rust emits no
  `contract` flag by default, so the absence of fused operations proves nothing about the
  flag. C6a deliverable 7.
- Deliverable 4: goldens exist for **S3M only**. MOD and MTM, the two formats M2 adds, have
  no cross-target hash check. C6a deliverable 5.
- `GOLDEN_INTERPOLATOR` (`crates/starplayer-offline/src/lib.rs:39`) names the golden file
  but does not select the kernel (`:233` hard-codes `Linear`), so the filename contract
  this deliverable exists to enforce can be broken in either direction without a failure.
  C6a deliverable 9.

Also noted: `round_shift_nearest` exists in two independent copies
(`crates/starplayer-mixer/src/path.rs:170-180` and
`crates/starplayer-dsp/src/interpolate.rs:105-115`), both commented as "the canonical
rule". They agree today; C6a deliverable 10 exports one.

## Out of scope

SIMD (M7). Cubic and sinc interpolation (M7). The embedded build itself (M8).
