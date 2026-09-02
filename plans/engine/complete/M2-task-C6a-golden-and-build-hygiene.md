# M2-task-C6a — Build hygiene, goldens coverage, and the web panning race

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Status | Landed |
| Depends on | C6 (fixed-point mixer, goldens), C1 (trace feature), C3a (web panning toggle) |
| Blocks | M2 exit (criteria 2 and 4) |
| Parallel with | C2a, C3b, C9 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (CI green on every job, plus the negative controls named below) |

## Context for a fresh agent

C1 (trace format), C6 (fixed-point mixer and golden hashes) and C3a (the web MOD panning
toggle) all landed on `m2`. A branch review found that several of their claims are not
actually enforced by the build: the `trace` feature leaks into every workspace build, the
FMA audit inspects the wrong crate and has no negative control, `GOLDEN_INTERPOLATOR`
names the golden file without selecting the kernel, goldens exist for S3M only, and the
web panning toggle can race a concurrent module load.

None of this is effect-accuracy work. It is the machinery that makes the accuracy claims
**true**, and each item is small. Do them all; none depends on the others except where
stated.

## Deliverables

1. **Stop the `trace` feature leaking into every workspace build.**
   `crates/starplayer-offline/Cargo.toml:8` and `crates/starplayer-testkit/Cargo.toml:8`
   both declare `starplayer = { workspace = true, features = ["std", "trace"] }` as an
   ordinary dependency feature. Under resolver-2 feature unification,
   `cargo test --workspace` therefore builds the engine and the wasm host **with tracing
   compiled in** — a `Vec` per tick inside `render()`. Two consequences: the
   `cfg(not(feature = "trace"))` arms are never test-compiled at all, and the golden hashes
   are produced by a binary that has the recorder linked in. Shipping builds happen to be
   safe only because `cargo xtask wasm` builds those packages in a separate invocation.

   Fix: make `trace` an **optional** feature of both crates
   (`trace = ["starplayer/trace"]`, with the base dependency carrying only `std`), and
   enable it through `required-features` on the `starplayer-trace` and
   `starplayer-conformance` binaries only. The goldens binary must build **without** it.
   *Verification:* `cargo tree -e features -p starplayer --workspace` shows `trace` off for
   an ordinary `cargo test --workspace`, and the two trace binaries still build and run.

2. **Meet C1 deliverable 2: prove the trace hook is zero-cost when disabled.**
   No benchmark exists, and the `report_trace_channels` loops run **unconditionally** in
   S3M (`crates/starplayer-s3m/src/processor.rs:610`, which also does a per-channel
   instrument lookup) and in MOD (`crates/starplayer-mod/src/processor.rs:733`).

   Fix: `#[cfg(feature = "trace")]` those call sites (and any sibling in MTM), then add
   either a small dependency-free timing test or a recorded `--emit=asm` inspection
   showing the loop is absent from the release build. Record the result in the C1 task
   file's post-landing note (C6a amends it; see deliverable 9 of the documentation pass).
   *Verification:* the release build with `trace` off contains no call to
   `report_trace_channels`, demonstrated by the check you add — not by assertion in prose.

3. **Trace correctness nits.**
   - `crates/starplayer-engine/src/source.rs:116` and
     `crates/starplayer-engine/src/trace.rs:291` both map `VoiceParam::Filter(_)` onto the
     `PITCH` dirty bit, so a filter write is reported as a pitch write. There is no filter
     flag; add one, or report no flag — do not report a false one.
   - `crates/starplayer-engine/src/trace.rs:330` unions the mixer-lifetime `params.dirty`
     into the per-tick flags (`entry.state.flags = voice.params.dirty | entry.writes`),
     so flags accumulate state that did not change this tick. Use `entry.writes`.
   *Verification:* a trace of a module with no filter effect contains no `P` flag on a
   tick whose only write was a filter parameter; a tick with no writes reports no flags.

4. **The web MOD-panning toggle races a concurrent load.**
   `apps/starplayer-web/www/app.js:336` claims a revision for a load
   (`const revision = ++state.moduleRevision`), but `:579` in `applyModPanningPreference`
   **reuses** `state.moduleRevision` without claiming a new one. Toggling the checkbox
   while a load is in flight posts the old module's bytes after the new ones (the worklet
   port is a FIFO), so the UI shows module B while the audio plays module A. Separately,
   `:571` persists the preference **before** the reload, and the failure path at
   `:600-605` only restores it when the revision still matches — so during a concurrent
   load a failed toggle leaves the persisted preference wrong forever.

   Fix, all three parts:
   - Claim a revision in `applyModPanningPreference` (`++state.moduleRevision`) exactly as
     the load path does, and compare against it on resolution.
   - Disable `elements.modHeadphonePanning` while `state.pendingLoads.size` is non-zero,
     and re-enable it when the map empties (including on the error path).
   - Restore the persisted preference on failure unconditionally, not only when the
     revision still matches.
   *Verification:* a headless test that starts a module load, toggles the panning checkbox
   before the load resolves, and asserts the **active module matches the UI** — both the
   displayed metadata and the panning state. It must fail against the current code.

5. **Goldens cover S3M only.**
   `goldens/s3m/` holds five owner fixtures. M2's exit criterion 4 is cross-target hash
   equality on the fixed path for the milestone's formats, and **MOD and MTM have no hash
   check at all** — the two formats this milestone exists to add.

   The obstacle is licence-safe input. Two acceptable routes, pick one and say why:
   - Hash renders of the **pinned corpus modules** inside the conformance job, where the
     bytes are already present and never committed to this repo. The goldens then live
     beside the pinned corpus commit hash so they are reproducible.
   - **Synthesise** small MOD and MTM fixtures in the testkit (a generator that writes a
     module exercising loops, finetune, vibrato, pan and a tempo change), commit the
     generator rather than the module, and hash the render.
   Either way, keep C6's filename contract:
   `goldens/<format>/<module>__i16_mono_44100_linear.sha256`.
   *Verification:* `cargo xtask goldens --check` covers all three formats, and the
   cross-target job (x86-64, aarch64, wasm32-wasip1) verifies the new hashes too.

6. **Record the M1 bit-compatibility break.** C6 switched truncation to round-half-away
   from zero (`crates/starplayer-mixer/src/path.rs:150-155`,
   `crates/starplayer-dsp/src/interpolate.rs:94-99`,
   `crates/starplayer-mixer/src/master.rs:143` and `:190`,
   `crates/starplayer-mixer/src/output.rs:179` and `:311`). That was intended, but the S3M
   goldens were **generated after** the change, so nothing in the tree marks that
   fixed-path S3M output now differs from M1 at the least significant bit. This is a
   documentation edit to `plans/engine/complete/M2-task-C6-fixed-point-mixer.md`; the
   documentation pass adds it, and your job is only to confirm the statement is accurate
   against the code before it lands.

7. **`fma-check` audits the wrong crate and has no negative control.**
   `xtask/src/main.rs:441-506` runs
   `cargo rustc -p starplayer-offline --release --lib -- -C target-feature=+fma --emit=asm,llvm-ir`.
   Trailing `rustc` arguments and `--emit` apply to the **final crate only**, so the
   non-generic float master bus — `soft_knee_f32`'s `from + (to - from) * fraction`,
   `bound_f32`, `process_float` in `starplayer-mixer` and `starplayer-dsp` — is never
   compiled with `+fma` and never scanned. And because Rust never emits `contract` flags by
   default, the **absence** of fused operations proves nothing about whether
   `-fp-contract=off` took effect.

   Fix: add `-p starplayer-mixer` and `-p starplayer-dsp` passes with the same inspection,
   and add a **negative control**: a second pass with the `-fp-contract=off` flag stripped
   from `RUSTFLAGS`/`.cargo/config.toml` that **must** produce a fused mnemonic or a
   contract flag. If it does not, the check proves nothing and you must either find a
   construct that does contract or drop C6's "proves it took effect" claim from the task
   file. Do not leave the claim standing on an inconclusive test.
   *Verification:* the negative control fails the audit (that is its success condition) and
   the positive pass covers all three crates.

8. **`RUSTFLAGS` silently discards `[build] rustflags`.**
   `.cargo/config.toml` sets `rustflags = ["-C", "llvm-args=-fp-contract=off"]`. Cargo
   **replaces** rather than merges when the `RUSTFLAGS` environment variable is set, so any
   job or developer shell that exports `RUSTFLAGS` loses the FMA policy without warning.
   Nothing in CI sets it today.

   Fix: add a CI assertion that `RUSTFLAGS` is unset (or that it contains the flag), and
   document the trap in `plans/engine/complete/M2-task-C6-fixed-point-mixer.md`.
   *Verification:* the assertion fails when `RUSTFLAGS=-C opt-level=2` is exported.

9. **`GOLDEN_INTERPOLATOR` names the file but does not select the kernel.**
   `crates/starplayer-offline/src/lib.rs:39` declares
   `pub const GOLDEN_INTERPOLATOR: Interpolator = Interpolator::Linear;` while `:233`
   hard-codes `Engine<Path, Linear, Out, Arc<Module>>`. Changing the constant renames every
   golden without changing a single rendered sample; changing the type changes the audio
   under the same filename. Both directions are silent, and C6's whole point was that the
   filename encodes the configuration.

   Fix: select the engine instantiation from the constant in one `match`, so the two can
   never disagree.
   *Verification:* switching the constant to `Nearest` produces differently-named goldens
   **and** different bytes; `--check` reports them missing rather than mismatched. Revert.

10. **`round_shift_nearest` exists in two copies.**
    `crates/starplayer-mixer/src/path.rs:170-180` and
    `crates/starplayer-dsp/src/interpolate.rs:105-115` are two independent definitions, and
    both are commented as "the canonical fixed-mixer rounding rule". They agree today.

    Fix: export it once from `starplayer-dsp` (the lower crate) and have the mixer use it.
    Keep the existing unit tests, wherever they end up.
    *Verification:* one definition exists; `grep -rn "fn round_shift_nearest" crates` finds
    exactly one hit, and the existing rounding assertions still pass unchanged.

## Research points

1. Whether `required-features` on a binary is enough to keep `trace` off the default
   workspace test build, or whether the testkit's library also needs a `cfg` split. Verify
   with `cargo tree -e features`, not by reading the manifest.
2. Which of the two golden routes in deliverable 5 the project prefers. The pinned-corpus
   route is cheaper and reproducible; the synthesised route is self-contained and survives
   a corpus repin. State the trade-off and pick one.
3. Whether any construct in `starplayer-mixer`/`starplayer-dsp` actually contracts under
   `+fma` with the flag removed. If none does, deliverable 7's negative control cannot
   exist and the C6 claim must be weakened instead — that is an acceptable outcome, but it
   must be written down.
4. Whether the headless web test in deliverable 4 fits the existing web test harness or
   needs a new one. Prefer the existing one.

## Research resolution and implementation contract

1. **`required-features` alone is not enough.** `cargo tree --edges features --workspace`
   is the check that decides it, and it says so: with `trace` declared as an ordinary
   dependency feature the default workspace resolution carries nine `feature "trace"`
   edges; with it optional, zero. But `starplayer-testkit`'s **library** is the trace
   differ — every item in it names `starplayer::engine::Trace` — so a `required-features`
   binary would leave the library edge unconditional. The library therefore carries
   `#![cfg(feature = "trace")]` and compiles to nothing without it.
   `starplayer-offline` keeps a real trace-free half (the golden renderer), so there the
   trace items are gated individually. `cargo xtask ci --job host-tests` now runs the
   `cargo tree` check itself, and runs `starplayer-{engine,offline,testkit}` a second time
   with `trace` on so the recorder keeps its test coverage.
2. **Synthesised fixtures, not the pinned corpus.** The corpus route is cheaper, but it
   binds the golden job to a cached download and to a corpus revision, and the goldens
   have to run on the ARM64 runner and under Wasmtime where that cache is the awkward part.
   Committing a generator instead keeps the golden job input-free on all three targets and
   survives a repin. The cost — the fixtures were never played by a real tracker, so the
   hashes are a mixer-regression contract rather than an accuracy one — is acceptable
   because accuracy is the conformance harness's job, and it is recorded in the generator's
   module comment.
3. **Nothing contracts, so the flag's effect is not observable — the claim is weakened.**
   Building `starplayer-mixer` or `starplayer-dsp` at `--release --lib -C target-feature=+fma`
   with `-fp-contract=off` stripped from `RUSTFLAGS` produces exactly the same output as
   with it: zero fused mnemonics, the same float multiplies, no `contract` marker. rustc
   emits no `contract` fast-math flags and LLVM's default policy will not fuse without
   them. C6's "proves it took effect" has been corrected in
   `complete/M2-task-C6-fixed-point-mixer.md`. A control that *does* exist replaces it:
   re-running the audit with `-fp-contract=fast`, which fuses regardless of the IR, must
   find a fused mnemonic — and does. That separates "no fusion happened" from "the scan is
   looking in the wrong file".
4. **The existing headless harness fits.** The new race check is a section of
   `apps/starplayer-web/test/headless.mjs` and runs in all three of its modes. Making it
   deterministic needed no new harness, only a different hook: the toggle is dispatched
   from a `MutationObserver` callback on the status line, which is a microtask at the end
   of the same task that posts the load to the worklet, so it always lands after the post
   and before the reply.

## Verification

- `cargo test --workspace` passes with `trace` **off**, proven by `cargo tree -e features`.
- `cargo xtask trace <module>` and `cargo xtask conformance` still work (their binaries
  carry `required-features`).
- `cargo xtask ci --job fma-check` passes across `starplayer-offline`, `starplayer-mixer`
  and `starplayer-dsp`, and its negative control fails as designed.
- `cargo xtask goldens --check` passes for MOD, S3M and MTM on x86-64, aarch64 and
  wasm32-wasip1.
- Switching `GOLDEN_INTERPOLATOR` changes both the golden filename and the rendered bytes.
- `grep -rn "fn round_shift_nearest" crates` returns exactly one definition.
- The headless web test fails on the pre-fix `app.js` and passes after.
- `cargo xtask ci --job clippy`, `--job no-std-check`, `--job wasm-build` all green.
- The buffer-size-independence test still holds at block sizes 1, 3, 64, 128, 4096, 8191.

## Out of scope

Any effect-accuracy change in any format processor (C3b, C9). Harness comparator repairs
(C2a). SIMD or new interpolators (M7). Adding new CI targets beyond the three C6 already
names.

## Post-landing notes (2026-09-02)

All ten deliverables landed on branch `m2-c6a`; `cargo xtask ci` passes every job,
including the new `trace-zero-cost` job. Where the outcome differed from the text above:

- **Deliverable 7's negative control cannot strip the flag.** Measured on the pinned
  toolchain, removing `-fp-contract=off` changes nothing: rustc emits no `contract`
  fast-math flags and LLVM's default policy does not fuse without them. The control
  instead forces `-fp-contract=fast` on `starplayer-mixer` and requires the scan to find a
  fused mnemonic, which proves the scan works. C6's "proves the flag took effect" claim
  is withdrawn in its task file; the flag is defence in depth. `starplayer-dsp` alone
  codegens no float (all generic), so its pass is scanned but not required to see a
  multiply.
- **Deliverable 3**: a filter write reports no flag rather than a new bit, because the
  v1 text format has no letter for one and adding a bit would bump the format version.
- **Deliverable 5** took the synthesised-fixture route; the generator is
  `crates/starplayer-offline/src/fixtures.rs`. The MOD and MTM hashes are regression
  contracts, not accuracy ones, and will legitimately move when C3b changes ProTracker
  behaviour those fixtures exercise (vibrato rounding, the loop gate, the Paula floor).
- **Deliverable 4**: when a file load and a panning toggle genuinely race, the toggle
  claims the later revision, so the file load is the one discarded. The new checkbox
  disable makes that unreachable through the UI.
- The aarch64 golden leg was not run locally (no cross toolchain); the `ubuntu-24.04-arm`
  CI job remains the authority.
