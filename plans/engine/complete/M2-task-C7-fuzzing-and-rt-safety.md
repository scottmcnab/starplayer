# M2-task-C7 — Loader fuzzing and the RT-safety allocator hook

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Status | Landed |
| Depends on | C3, C4 (three loaders exist) |
| Blocks | M2 exit |
| Parallel with | C5 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (CI green, corpus clean) |

## Context for a fresh agent

Two invariants get automated enforcement here, both of which the codebase has been
*written* to satisfy since M0 but has never been *proven* to satisfy.

**Malformed modules are the number-one crash source in every tracker ever written.** The
files are decades old, written by dozens of trackers, many subtly out of spec, and some
deliberately hostile. The loader invariant is simple and absolute: **never panic, never
OOM, always return `Err` or a clamped-but-valid `Module`.**

**Real-time safety erodes silently.** `no_std` does not prevent it: dropping the last
`Arc<Module>` on the audio thread calls `free()`, and so does a `Vec` growth in a "just
for telemetry" path. The garbage channel (architecture §8) exists to prevent the first;
this task proves it works and catches the second.

## Deliverables

1. **A fuzz target per loader** (`cargo-fuzz`): S3M, MOD, MTM. Each takes arbitrary bytes
   and asserts the loader returns without panicking and without allocating unboundedly.

2. **A memory cap** in the fuzz harness, so an OOM is a *failure* rather than a killed
   process. A loader that reads a length field and allocates it is the classic bug; the
   cap catches it.

3. **A structured fuzz target** as well as the byte-level one: start from a valid module
   and mutate fields, which reaches deeper paths than random bytes usually will.

4. **A seed corpus** from the real modules used in M1/M2 tests, plus the conformance
   corpus.

5. **CI integration**: a bounded fuzz run per commit (minutes, not hours), plus a longer
   scheduled run. Any crash found is committed as a regression test.

6. **The allocator hook**: a test-only global allocator that panics on allocation while a
   thread-local "in render" flag is set. Wrap `render()` and run the **whole corpus**
   through it.

7. **A garbage-channel test**: load a module, start playing, swap in a second module, and
   assert the first `Arc` was dropped on the control thread and never on the audio thread.

## Research points

1. Whether `assert_no_alloc` is a better fit than a hand-rolled allocator hook. It is
   well-tested and small; prefer it unless its `no_std` story blocks us.
2. How to make the fuzzers effective quickly — a dictionary of format magic values
   (`SCRM`, `M.K.`, `MTM`, `SCRS`) usually pays for itself immediately.
3. Whether any *loader* allocation should also be capped in production, not just in fuzz —
   a 2 GB length field in a 4 KB file should be rejected on principle, not merely survived.

## Research point answers (recorded on completion)

**1. `assert_no_alloc`, or a hand-rolled hook? Hand-rolled — about fifty lines, in
`crates/starplayer-offline/tests/render_allocation.rs`.**

`assert_no_alloc` is a thread-local counter plus a `GlobalAlloc` wrapper, which is exactly
the code it would have saved; the crate's last release is 1.1.2 (2022). Two things decided
it, and neither is the `no_std` story — the hook is test-only and the tests are `std`, so
that question never bites:

* **Its violation handler aborts the process.** Deliverable 6 says "run the whole corpus
  through it", and an abort tells you that *something* in seven hundred modules allocated
  without saying which. The hook here records count, bytes and largest request, lets the
  allocation proceed, and the harness asserts and names the module the moment `render()`
  returns.
* **Panicking from inside `GlobalAlloc::alloc` is not safe to do anyway.** The panic
  payload is boxed, so a panicking allocator re-enters itself, and unwinding out of an
  allocation abandons the collection mid-resize. Recording is the only correct shape, and
  once you are recording you are no longer using the crate's interesting part.

The fuzz crate's cap (`fuzz/src/lib.rs`) is a *different* allocator with a different
policy — it returns null, so the standard library aborts and libFuzzer writes an artifact —
which is another reason a single dependency would not have covered both.

**2. Does a dictionary of magic values pay for itself? Immediately — `fuzz/dictionaries/tracker.dict`.**

Every loader compares four bytes before it parses anything: `SCRM` at `0x2C`, `MTM\x10` at
`0`, and one of eleven tags at `1080`. Mutation has no route to those values, so without a
dictionary a byte-level target essentially never reaches a loader body — it fuzzes
`probe`. The dictionary carries the accepted tags, the tags C5 will accept later, `SCRS` /
`SCRI`, the `0x1A 0x10` type bytes, `defaultpan == 252`, the two `Cwt/v` values whose
dialects matter, and the `0xFE` / `0xFF` order markers. libFuzzer's own
`###### Recommended dictionary` output after a run is the check that it is being used.

The seed corpus matters more than the dictionary, and the structured targets more than
either: starting from a module that already loads is what reaches the sample loop clamps
and the pattern unpacker on the first iteration rather than the millionth.

**3. Should a loader allocation be capped in production? Yes, in exactly one place —
`MINIMUM_PATTERN_BUDGET_BYTES` in `crates/starplayer-s3m/src/loader.rs`.**

An audit of the three loaders found one real amplification vector. MOD requires the pattern
bytes to be present in the file before it allocates them, and clamps sample PCM to what is
there; MTM preflights every declared sample span against the file length (C4 already fixed
that one) and its pattern count is a `u8`. S3M is different: `Patnum` is a `u16` and a
pattern costs **two bytes** of parapointer, while each pattern unpacks to a fixed
`64 × channels × 5` bytes. A 132 KB file may therefore declare 65,535 patterns and expand
to 670 MB — with every parapointer zero, which is not even malformed, because Scream
Tracker 3 spells an empty pattern that way.

The rule adopted is the one the research point states: a 2 GB length field in a 4 KB file
is rejected on principle, not merely survived. The budget is `max(4 MiB, 64 × file size)`
of decoded pattern data, which is 819 patterns of 32 channels at the floor, against Scream
Tracker 3's own limit of 100 and OpenMPT's 256. Three unit tests pin it: the attack is
refused, 256 wide patterns still load, and the budget function is a floor rather than a
ratio for small files.

## Verification

- Each fuzz target runs for the CI budget with zero crashes and zero OOMs.
- A deliberately-introduced unchecked index in a loader is found by the fuzzer within the
  CI budget. Revert it. (This proves the fuzzer is actually reaching the code.)
- The allocator hook runs the whole corpus with no allocation inside `render()`.
- A deliberately-introduced `Vec::push` inside `render()` fails the hook. Revert.
- The garbage-channel test passes, and fails if the return path is removed.

## Out of scope

Fuzzing the sequencer or mixer directly (the loaders are where untrusted input enters).
XM and IT fuzz targets — they arrive with their formats.

## Post-landing notes (2026-09-03)

All seven deliverables landed on branch `m2-c7`. Where the outcome differed from the text
above:

- **Deliverable 6 records rather than panics.** Panicking inside a global allocator is
  unsound (the boxed payload re-enters the allocator), so the hook counts allocations on
  the render thread and the harness asserts immediately after `render()` returns, naming
  the module. It runs as `cargo xtask ci --job rt-safety` over every module the repository
  can reach, at three host block sizes and across a `LoadModule` swap, with and without
  `telemetry`. It is `cfg(not(feature = "trace"))`: the trace recorder allocates inside
  `render()` by design and is guarded by `trace-zero-cost` instead.
- **Deliverable 3 is three structured targets**, one per format, each starting from four
  compiled-in valid bases (synthesised seeds and one owner S3M).
- **Deliverable 5**: `fuzz-smoke` is the only CI job on nightly (`cargo fuzz` needs
  sanitizer coverage that stable rejects) and is opt-in for the local `cargo xtask ci`
  sweep; a nightly workflow runs 15 minutes per target with a corpus cache. Every seed and
  every future crash regression is replayed on the pinned toolchain inside `host-tests`.
- **Research point 3**: one production cap, in the S3M loader — a decoded-pattern budget
  of `max(4 MiB, 64 × file size)`, since a 132 KB file may legally declare 65,535 empty
  patterns that unpack to 670 MB. MOD and MTM were audited and need nothing.
- The three negative checks passed: an unchecked MOD finetune index was found by the
  fuzzer in under a second, a `Vec::push` in `render_quantum` failed the hook, and an
  inline drop in place of `garbage.retire` failed the garbage-channel test. All reverted.
- No real crash, OOM or render-path allocation was found in existing code.
