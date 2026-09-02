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
switched the fixed path from truncation to round-to-nearest with ties away from zero. Every
signed reduction on that path goes through `starplayer_dsp::round_shift_nearest`: Q0.32
linear interpolation (`crates/starplayer-dsp/src/interpolate.rs`, `Linear::sample_fixed`),
Q15 voice gain and voice accumulation (`crates/starplayer-mixer/src/path.rs`,
`FixedPath::gain` and `FixedPath::mix`), limiter interpolation and master volume
(`crates/starplayer-mixer/src/master.rs`), and reduced-depth output and mono fold-down
(`crates/starplayer-mixer/src/output.rs`). Confirmed against the code by C6a. That was the
deliberate outcome of
research point 2 — but **fixed-path S3M output now differs from M1's at the least
significant bit**, and because the S3M goldens were generated *after* the change, no
committed artefact records the break. Stated here so a future comparison against an M1
render is not mistaken for a regression.

**`RUSTFLAGS` discards `[build] rustflags`.** `.cargo/config.toml` carries
`rustflags = ["-C", "llvm-args=-fp-contract=off"]`. Cargo **replaces** rather than merges
when the `RUSTFLAGS` environment variable is set, so any CI job or developer shell that
exports `RUSTFLAGS` silently loses the FMA policy — an empty `RUSTFLAGS` strips it just as
completely as a populated one. Nothing in CI sets it. C6a made that loud: every
`cargo xtask ci` invocation now fails up front unless `RUSTFLAGS` is either unset or
carries `-fp-contract=off` itself.

**Three gaps in what the deliverables claim, all tracked by C6a:**

- Deliverable 3's "verify it actually took effect": `cargo xtask ci --job fma-check`
  compiled and scanned only `starplayer-offline`'s own codegen — trailing `rustc` arguments
  and `--emit` apply to the final crate only — so the non-generic float master bus in
  `starplayer-mixer` and `starplayer-dsp` was never compiled with `+fma` and never
  inspected. It also had no negative control, and Rust emits no `contract` flag by default,
  so the absence of fused operations proved nothing about the flag. C6a deliverable 7.
- Deliverable 4: goldens exist for **S3M only**. MOD and MTM, the two formats M2 adds, have
  no cross-target hash check. C6a deliverable 5.
- `GOLDEN_INTERPOLATOR` named the golden file but did not select the kernel — the render
  hard-coded `Linear` — so the filename contract this deliverable exists to enforce could
  be broken in either direction without a failure. C6a deliverable 9.

Also noted: `round_shift_nearest` existed in two independent copies, in
`crates/starplayer-mixer/src/path.rs` and `crates/starplayer-dsp/src/interpolate.rs`, both
commented as "the canonical rule". They agreed; C6a deliverable 10 exports one.

## Post-landing notes — C6a, 2026-09-02

**Deliverable 3's claim is weaker than C6 stated, and this is the correction.** C6a's
research point 3 measured whether anything in `starplayer-mixer` or `starplayer-dsp`
actually contracts when the flag is removed. Nothing does. Building either crate at
`--release --lib -C target-feature=+fma` with `RUSTFLAGS` set to something that drops
`-fp-contract=off` produces byte-for-byte the same count of fused mnemonics (zero), float
multiplies and `contract` markers as the shipped configuration: rustc never emits `contract`
fast-math flags, and LLVM's default fusion policy already refuses to fuse operations that
do not carry them. **Stripping the flag therefore cannot be detected, so no control built
on stripping it can exist**, and C6's "proves it took effect" is not a claim the audit can
support.

What the audit does now support, and what `cargo xtask ci --job fma-check` asserts:

1. **Three positive passes**, one `cargo rustc` invocation each so the trailing `+fma` and
   `--emit` actually reach the crate being scanned: `starplayer-offline` (which
   monomorphises the whole float voice path), `starplayer-mixer` (the non-generic master
   bus: `process_float`, `soft_knee_f32`, `bound_f32`) and `starplayer-dsp`. Each must
   contain no fused mnemonic, no `llvm.fma`/`llvm.fmuladd` intrinsic and no
   contract-marked float operation. The first two must also contain an ordinary `f32`
   multiply, or the pass is reported as inconclusive. `starplayer-dsp` is exempt from that
   last requirement and is documented as such in `xtask/src/main.rs`: every float
   expression it owns lives in a generic or trait-impl method, so its own rlib codegens
   none of them and its optimized output is empty. The pass runs regardless, so the first
   non-generic float helper added there is covered from that commit.
2. **A negative control that does exist.** The audit re-runs the `starplayer-mixer` pass
   with `RUSTFLAGS="-C llvm-args=-fp-contract=fast"`, which fuses regardless of what the
   IR asks for, and **requires** a fused mnemonic. It finds one. That is what separates
   "no fusion happened" from "the scan is looking in the wrong file", and it also shows
   this code would fuse if the policy allowed it.

So `-fp-contract=off` is defence in depth against a future rustc or LLVM default, not a
setting whose effect is observable today — and that is now what the tree says.

**Goldens cover all three M2 formats.** MOD and MTM have no licence-safe module to commit,
so C6a synthesises one fixture each from a committed generator
(`crates/starplayer-offline/src/fixtures.rs`) rather than hashing renders of the pinned
libxmp corpus. The trade-off is recorded in that file's module comment: the corpus route is
cheaper but binds the golden job to a cached download and to a corpus revision, while the
generator keeps the goldens reproducible on the ARM64 runner and under Wasmtime with no
inputs at all, and survives a corpus repin. Its cost is that the fixtures were never played
by a real tracker, so they are a mixer-regression contract and not an accuracy one —
accuracy stays the conformance harness's job. The five S3M hashes are unchanged by
everything in C6a.

**`GOLDEN_INTERPOLATOR` now selects the kernel.** `render_golden` matches on the constant
and instantiates `Nearest` or `Linear` from it, so the filename and the audio move
together; a kernel the build has no implementation for is `RenderError::UnimplementedInterpolator`
rather than a silently mislabelled render.

**`round_shift_nearest` has one definition**, `starplayer_dsp::round_shift_nearest`;
`starplayer-mixer`'s `path`, `master`, `output` and `gain` all import it, and the existing
rounding assertions are unchanged.

## Out of scope

SIMD (M7). Cubic and sinc interpolation (M7). The embedded build itself (M8).
