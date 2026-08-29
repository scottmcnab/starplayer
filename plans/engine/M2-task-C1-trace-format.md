# M2-task-C1 — Per-tick state trace format and differ

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Depends on | M1 |
| Blocks | C2 |
| Parallel with | C6 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (round-trip + differ tests) |

## Context for a fresh agent

**This is the highest-value tool in the project.** Audio diffs tell you *that* something
is wrong; a per-tick state trace tells you *which effect, on which channel, at which
tick*. Every subsequent accuracy task depends on it.

Read `plans/product/01-technical-architecture.md` §2.2. The trace is captured through a
`#[cfg(feature = "trace")]` hook on parameter writes — **zero cost in release builds**,
full detail in test builds. That is why voice parameters are a struct with dirty bits
rather than a queued event stream: we get the diffable stream where it is useful without
paying for it in the audio path.

The format must be comparable against libxmp's `test-dev/` expected-state dumps (C2), so
survey those first and align the fields where it is sensible to.

## Deliverables

1. **A stable line-oriented text format**, one block per tick:
   ```
   t=00042 ord=03 pat=07 row=12 spd=06 bpm=125 gv=64
    ch=00 note=C-5 ins=01 vol=64 per=1712 pan=48 pos=00001234.5678 fl=VP
    ch=01 ...
   ```
   Stability matters more than elegance — this format is checked into tests and diffed by
   humans. Version it with a header line so a format change is visible rather than
   silently invalidating every expectation.

2. **The trace hook** on every `VoiceParams` write and every sequencer state change, gated
   behind `feature = "trace"`. Assert by inspection of the generated assembly, or at least
   by a benchmark, that the release build is unaffected.

3. **The differ** in `starplayer-testkit`: compares two traces and reports the **first**
   divergence with surrounding context, plus a summary of how many ticks and channels
   diverged. First-divergence reporting is what makes it usable; a wall of differences
   is not.

4. **Tolerance options.** Some comparisons want exactness; comparing against libxmp will
   need tolerance on fields where representation differs (period versus frequency, volume
   scale). Make tolerance explicit and per-field, never global and implicit.

5. **`cargo xtask trace <module> [--ticks N]`** to produce a trace, so an agent debugging
   an effect can get one in a single command.

## Research points

1. The exact shape of libxmp `test-dev/`'s expected-state dumps. Aligning field names and
   ordering where possible makes C2 much cheaper. Where they cannot align, write the
   mapping down once, in the harness.
2. Whether a compact binary trace is worth having alongside the text one for long
   modules. Text first; add binary only if trace files become unwieldy.

## Verification

- A trace of the same module is byte-identical across two runs, and across two different
  host block sizes.
- The differ finds a deliberately-injected one-tick, one-channel, one-field difference and
  reports it as the first divergence with the right tick and channel.
- The differ reports "identical" for two identical traces.
- A release build with `trace` disabled shows no measurable difference in render
  throughput.

## Out of scope

The conformance corpus itself (C2). Golden WAV hashes (C6). Comparing against the
original DOS player (C8, deferred).
