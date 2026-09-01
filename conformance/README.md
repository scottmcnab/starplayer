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
subtracts libxmp's one-octave note-number bias, rounds period to the native quarter-period
scale, divides volume by 16, and adds one to the instrument number. MOD pan preserves the
full unsigned byte so `8xx` low bits remain observable; S3M and MTM pan decode libxmp's
high-nibble representation onto their native 4-bit grid. Period is
allowed one native unit and sample position one whole source frame. Cutoff zero is
libxmp's disabled-filter sentinel and maps
to C1's equivalent fully-open 255; like libxmp, the two fully-open cutoff values 254 and
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
exclusion stale and fails the command.

Current engine failures link to `known-failures.md`. They are M2 exit blockers rather
than accepted deviations, and the reasons record the first observed mismatch from the
pinned run.

MOD and MTM cases are not exclusions while their native loaders/processors are absent.
They are reported as explicit C3/C4 gates and kept in the total. When those format
capture paths land, their integration must register the corresponding function in the
runner's capability table; the gate does not claim automatic discovery. A missing
implemented-format loader is never converted into an exclusion.

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
