# M2-task-C8 — DOS reference harness (DEFERRED, pull-driven)

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) — **deferred; not scheduled** |
| Trigger | M1/M2 hits an effect-semantics ambiguity that reading `S3MLIB.ASM` cannot settle, and that libxmp's `test-dev/` and the OpenMPT corpus do not resolve either |
| Depends on | C1 (trace format) |
| Blocks | — |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (reference trace produced and diffed) |

## Context for a fresh agent

**Do not start this task speculatively.** It is deferred by owner decision
(`plans/product/00-vision.md` decision 5): read the assembly as the specification, and
pull this in only if the port hits an ambiguity the source cannot settle.

If the trigger does fire, this is how to get ground truth from the original player
itself.

### Why a state trace, not an audio capture

The tempting approach — run `STAR.EXE` in DOSBox and diff the audio — is the wrong one.
DOSBox's SoundBlaster emulation resamples, and the original mixed 8-bit unsigned mono at
whatever rate `SetSBMixingRate` happened to pick. Diffing that means diffing emulator
artefacts. It would also compare the *mixer*, which we deliberately do not reproduce
(`plans/product/03-accuracy-policy.md` §4), rather than the *sequencer*, which is the
thing in question.

Instead, dump the original's own per-tick `ChannelData` and compare it against our C1
trace. That is an apples-to-apples comparison of exactly the state we care about.

### Why this is even possible — updated 2026-08-31

**No reconstruction is needed any more.** `STARPLAY-2.25s/` (imported from Hornet's
`sp-code.zip`) is the complete, assemblable 2.25s source: `MAKESP.BAT` builds it with
TASM 4.0 + DOS `link` on the pmode 2.51 extender, and its tracker core is byte-identical
to the text inside `STARPLAY/`'s `comment %` blocks
(`plans/reference/original-s3mlib-analysis.md` §0.a). If this task's trigger fires,
start from `STARPLAY-2.25s/` (copied to `tools/dos-reference/`, since both original
trees are read-only) and replace `STARPLAY.ASM` with the harness — deliverable 1's
comment-stripping step is obsolete. The `P.BAT` PMODE/W chain in `STARPLAY/` remains a
fallback only.

## Deliverables

1. **A reconstructed copy** of the sources with the `comment %` wrappers stripped, in a
   **separate directory** (e.g. `tools/dos-reference/`).
   **`STARPLAY/` is read-only and must not be modified** — this is a repo-wide rule in
   `AGENTS.md`, and the reconstruction is a derived artefact, not an edit.

2. **A minimal DOS harness** in the original's own style, replacing `STAR.ASM`: call
   `PM_LoadModule`, then drive `__UpdateTracker` N times, dumping the 32-entry
   `ChannelData` array to a file after each tick. It needs **no sound hardware at all** —
   which is the point, since it also sidesteps DOSBox's audio emulation entirely.

3. **A DOSBox invocation** that runs it headlessly and retrieves the dump.

4. **A converter** from the dumped `ChannelData` layout into the C1 trace format, so the
   existing differ works unchanged. The field mapping is in
   `plans/reference/original-s3mlib-analysis.md` §2.

5. **A findings document** — `plans/reference/dos-reference-findings.md` — recording what
   ambiguity prompted the run, what the original actually does, and what was decided. Any
   resulting behaviour change is an accuracy-policy amendment.

## Research points

1. Whether TASM and Watcom `wlink` are practically obtainable and runnable under DOSBox
   today. Neither DOSBox nor TASM is installed on the dev machine. If the toolchain
   proves impractical, the fallback is careful re-reading plus the OpenMPT wiki, and this
   task closes as "not feasible" with that recorded.
2. Whether the `comment %` blocks strip cleanly, or whether some code inside them was
   left half-edited and no longer assembles. Time-box this: if it does not build within a
   day, the answer to research point 1 is effectively "no".
3. Whether `PM_InitSystem` can be stubbed out entirely so no sound device is needed.

## Verification

- The harness builds and runs under DOSBox and produces a per-tick dump for a known
  module.
- The dump converts into the C1 format and the differ runs against our own trace.
- The specific ambiguity that triggered this task is **answered**, and the answer is
  written into `plans/product/03-accuracy-policy.md`.

## Out of scope

Reviving a *playable* `STAR.EXE`. That would be a pleasant side effect and is explicitly
not the goal — the deliverable is a state trace. Any audio capture or comparison.
