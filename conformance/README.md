# Conformance corpora

`cargo xtask conformance` acquires a checksum-pinned libxmp source snapshot into
`target/conformance/corpora/` and runs the machine-readable MOD, S3M and MTM playback
cases listed in `cases.tsv`. `cargo xtask conformance --offline` refuses corpus
acquisition and never contacts the live corpus host; it is the command used by the CI
test step after the acquisition cache has been prepared.

The snapshot is libxmp commit
`6ec0ba21b1b28f91e22b68a51d59207c6bbf6139`. The archive URL and SHA-256 are constants
in `xtask/src/main.rs`; the extracted revision marker is checked before every run. The
OpenMPT-origin rows use libxmp's checked-in copies of OpenMPT's player-test modules and
the frame dumps generated for them. This gives those otherwise qualitative cases a
machine-readable oracle while keeping the bytes under the same immutable archive pin.
The OpenMPT wiki snapshots embedded there identify MOD page revision 4572 and S3M page
revision 4657.

## Oracle semantics

libxmp's `test-dev/gen_mixer_data.c` and `compare_mixer_data.c` are the primary source
for the adapter. At 44.1 kHz and 100% stereo separation, each active mapped channel is
written after `xmp_play_frame` as:

```text
time-ms row frame channel period-q12 note instrument-zero-based volume-x16 pan-signed position [cutoff [resonance]]
```

Playback stops before the second loop. Unmapped and sample-ended voices are omitted.
libxmp permits one millisecond of time error and one integer sample of position error;
its period is a floating-point-derived Q12 value. StarPlayer's adapter compares libxmp's
end-of-frame time with the exact C1 tick-end frame (45 output frames of tolerance),
divides volume by 16, and adds one to the instrument number. MOD's comparison axis is
the ProTracker display octave: the adapter subtracts two octaves from libxmp's mixer
note and one octave from C1's reference-rate note, and divides Q12 period by 4096 into
native Amiga periods. Both sides advance MOD samples on the PAL clock — libxmp keeps
`C4_PAL_RATE` for the ProTracker and OpenMPT tracker ids and picks NTSC only for the
ScreamTracker3/FastTracker/TakeTracker/ModsGrave ids or more than four channels — so the
residual MOD position differences are libxmp's continuous finetuned periods and its
rounded integer C-4 rate, recorded as accuracy-policy D14, not a PAL/NTSC clock split.
S3M and MTM retain C2's one-octave subtraction and division by 1024 into their native
quarter-period scale. MOD pan preserves the full unsigned byte so `8xx` low bits remain
observable; S3M and MTM pan decode libxmp's high-nibble representation onto their native
4-bit grid. Period is allowed one native unit. libxmp's `pos0` is the integer source
position before mixing, so for the two formats compared in the MOD-period mixer domain —
MOD and MTM — the adapter floors StarPlayer's Q32.32 position into that same integer
domain; S3M compares the unfloored Q32.32 value. All three then apply libxmp's own
one-integer-sample comparison bound, so a two-frame playback-step error remains visible.
Cutoff zero is libxmp's disabled-filter sentinel and maps to C1's equivalent fully-open
255; like libxmp, the two fully-open cutoff values 254 and
255 are equivalent. Other cutoff and resonance values compare directly. Only sample
number and dirty flags lack upstream columns and are projected from
the actual C1 trace. The active-channel set is a union, so extra StarPlayer voices cannot
disappear during projection.

## Scope accounting

At the pinned revision, `test-dev/` contains 138 files with a MOD, S3M or MTM extension:
91 MOD, 42 S3M and 5 MTM. Exactly 47 calls to libxmp's `compare_mixer_data*` functions
target those formats (27 MOD, 17 S3M and 3 MTM), covering 46 unique modules. The manifest
contains all 47, not a smoke subset. On every run the harness parses the pinned upstream
`test_*.c` call sites and fails if `cases.tsv` is missing or inventing a pair.

The other 92 binaries are loader, depacker, fuzzer, module-length, full-song, or
documentation inputs without a frame-state dump. They are reported as seed/documentation
inventory, never as conformance passes. The pinned OpenMPT snapshot contains 45 MOD/S3M
modules: 22 unique modules have 23 frame-state oracle cases, while 23 remain
documented/audio-only. The latter are visible inventory for future purpose-built oracles;
perceptual/audio comparison remains M3 and is not fabricated here.

## Exclusions and gates

Every exclusion must name a manifest case and contain both a non-empty reason and an
accuracy-policy or tracking reference. Unknown, duplicate, or incomplete entries make
the command fail. Excluded cases are still executed, and an unexpected pass makes the
exclusion stale and fails the command. Accepted format boundaries and deliberate
secondary-oracle disagreements link to the accuracy policy. Everything else links to the
task file that owns it — C2a for harness bugs, C3b for ProTracker fidelity and the
voice-boundary swap, C5 for tracker dialects, C9 for the S3M records catalogued in
`known-failures.md` — and each reason records the first observed mismatch from the pinned
run. Those remain M2 exit blockers.

All three formats are now registered through their own native trace paths — MOD by C3,
MTM by C4, S3M from M1 — so the runner's pending-integration table is empty and its
`gated` column is always zero. Every pinned case therefore executes and reports a pass or
a named exclusion; a missing loader would be a gate, never an exclusion, but no format is
in that state.

## Current standing

3 of the 47 pinned cases pass: MOD 0 of 27, S3M 2 of 17, MTM 1 of 3. The remaining 44 are
exclusions, and they are not one kind of thing:

- **Accepted accuracy-policy deviations** (D12 is explicitly *not* one of these): the
  oracle-representation entries D14–D19 and the §4 Startrekker AM-synth cases.
- **Tracker dialects** awaiting `plans/engine/M2-task-C5-quirks-and-tempo-models.md`: five
  Octalyser / Digital Tracker MOD cases and six S3M `cwtv` pattern-loop modes.
- **Tracked repairs**: harness bugs in
  `plans/engine/M2-task-C2a-conformance-harness-repairs.md`, ProTracker fidelity and the
  voice-boundary sample swap in
  `plans/engine/M2-task-C3b-protracker-fidelity-repairs.md`, and the S3M records in
  `plans/engine/M2-task-C9-s3m-conformance-repairs.md`.

The pass count is the honest measure of M2's third exit criterion; the harness exiting
zero on 44 exclusions is not.

## Deviation from the C2 task file

C2's deliverable named an OpenMPT corpus acquisition step. That was substituted: the
`openmpt-*` rows use libxmp's own checked-in copies of OpenMPT's player-test modules and
the frame dumps generated for them (23 oracle cases; 23 of the pinned OpenMPT binaries
remain documented-only). This keeps every byte under one immutable archive pin and avoids
a second, unlicensed download, but it is a scoped deviation from the task as written, not
a completed deliverable.

## Licensing finding (decision deferred)

- libxmp source and its associated documentation declare the MIT licence in
  `docs/COPYING`.
- OpenMPT's source tree declares BSD-3-Clause. Its wiki page text is CC BY-SA 3.0; the
  pinned `00_README` snapshots in libxmp carry that notice.
- No explicit per-file licence grant was found for the binary player-test modules served
  from `resources.openmpt.org`. Their redistribution status is therefore unresolved.

For that reason the corpus is fetched into an ignored build cache and is not vendored.
Before a public release, decision 7 in `plans/product/00-vision.md` must resolve whether
downloading these files for tests is acceptable and what notices or replacement corpus
are required. This task adds no repository licence or SPDX declaration.
