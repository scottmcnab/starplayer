# M2-task-C7 — Loader fuzzing and the RT-safety allocator hook

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
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
