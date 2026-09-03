# Conformance corpora

`cargo xtask conformance` acquires a checksum-pinned libxmp source snapshot into
`target/conformance/corpora/` and runs the machine-readable MOD, S3M and MTM playback
cases listed in `cases.tsv`. `cargo xtask conformance --offline` refuses corpus
acquisition and never contacts the live corpus host; it is the command used by the CI
test step after the acquisition cache has been prepared. `cargo xtask conformance
--strict` additionally exits non-zero while any known-failure exclusion remains and names
every one of them.

CI runs the informational (non-strict) form. That is deliberate while `C2-S3M-009`
remains; making it strict is one added argument in `job_conformance` in
`xtask/src/main.rs`.

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
divides volume by 16, and adds one to the instrument number.

**Records are paired by time, not by `(row, frame)`.** A libxmp record carries `time_ms`
but no order index, so pairing on the tracker position alone mis-associates a jump
destination's row 0 with the starting order's row 0. The adapter finds the StarPlayer tick
whose end frame carries the record's timestamp, within the same 45-frame bound, and then
compares `row` and `tick_in_row` as ordinary trace fields. A record with no pairable tick
is reported through the differ as a `frame` divergence, never as a harness string error,
so C2's promise of a first-divergence report naming the tick, channel and field holds for
every failure class. MOD's comparison axis is
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

### Impulse Tracker (task G3)

IT is the first format whose oracle records are not one per pattern channel, and the
adapter has four IT-specific projections.

**Background voices are dumped, as virtual channels.** Both `gen_mixer_data.c` and
`compare_mixer_data.c` loop over `p->virt.virt_channels`, which for a format carrying
`QUIRK_VIRTUAL` — IT does — is `num_tracks` plus the whole mixer voice pool. A New Note
Action's detached voice therefore appears under a channel index at or above the module's
channel count. The numbering is reproducible: `libxmp_virt_setpatch` moves the displaced
voice to the **lowest free virtual channel at or above `num_tracks`**
(`src/virtual.c:517-523`) and frees the slot when the voice stops. The adapter's
`background_virtual_channels` replays exactly that rule over trace v2's ` vc=` rows — a
row that was not there last tick takes the lowest free index and holds it until the voice
goes — and folds the result into the same channel axis the ` ch=` rows use, so every
oracle column is enforced, background ones included. Both players walk their channels in
ascending order and both hand out the lowest free slot, so two detachments in one tick are
numbered the same way.

**The period domain is libxmp's whole Amiga period, with the sample rate folded into the
note.** libxmp's IT loader converts each sample's `C5Speed` into a relative note plus a
finetune (`libxmp_c2spd_to_note`, `src/loaders/it_load.c:906`) and then mixes at the fixed
`m->c4rate`, so its period numerator is the constant `C4_PERIOD · 8363` and its `note`
column is the pattern note **plus that relative note**. The C1 trace reports
`428 · 8363 / frequency`; `project_period` divides the oracle's Q12 column by 4096 as MOD
and MTM do, and `project_note` removes the same relative-note offset the loader added, so
the two sit on IT's own note numbering with 60 as the pitch a sample plays at its
`C5Speed`. Accuracy-policy **D64** records what that axis can and cannot see.

**Positions are compared as floored integers, and the two clocks are kept apart.** libxmp's
`pos0` discards the mixer's fraction, so IT joins MOD and MTM in flooring StarPlayer's
Q32.32 position before applying libxmp's one-integer-sample bound — otherwise a voice a
quarter of a frame ahead of the oracle's *floor* reads as a whole frame out whenever the
fraction is high. Separately, libxmp keeps **two clocks that disagree with each other**:
the frames it renders per tick are truncated to a whole frame (`src/mixer.c:440`,
`ticksize = (int)calc`) while the `time` column accumulates the exact value in a `double`
(`src/player.c:2171`). Pairing keys off the timestamps, so `tick_end_frames` models the
*exact* clock for every format including IT; the engine renders IT on the truncated one
(`TempoModelId::ItModern`, accuracy policy §2), which is what makes the positions
comparable at all. Residual position drift from IT's integer-hertz playback rate is
accuracy-policy **D67** and is waived per case.

**Filter and pan.** libxmp stores IT's 0..127 cutoff and resonance doubled
(`xc->filter.cutoff = val << 1`) and scales the cutoff by the filter envelope before
dumping it, so the columns are a 0..255 domain. `FilterParams::from_it` encodes
`value · 2 · 257` — `255 · 257 == 65535` exactly — which makes the C1 trace's
`unit_to_scale(bits, 255)` read back libxmp's column with no rounding and
`FilterParams::to_it` read back IT's 0..127 pair for the mixer's coefficients. As libxmp's
own comparator does, two cutoff values at or above 254 compare equal. The pan column
carries libxmp's `PAN_SURROUND` sentinel (`0x8000`) verbatim for a channel `S91` put into
surround; the adapter projects it onto centre, which is what StarPlayer renders
(accuracy-policy **D66**), and every other IT pan compares as a full byte because IT's
0..64 pan is scaled by four inside the replayer rather than shifted into a nibble.

**The manifest covers 121 IT cases, not 59.** `audit_pinned_corpus` insists the manifest
is exactly the set of `compare_mixer_data*` pairs in the pinned tree for every target
extension, so wiring `it` brings in the 59 `openmpt/it` fixtures **and** the 62 `data/*.it`
ones from `test_effect_it_*`, `test_player_it_*`, `test_storlek_*` and two fuzzer cases.

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
accuracy-policy or tracking reference, plus an optional fourth `waive=` column. Unknown,
duplicate, or incomplete entries make the command fail, as does a waiver naming a field
the trace contract does not have. Excluded cases are still executed, and an unexpected
pass makes the exclusion stale and fails the command. The summary splits exclusions into
**accepted deviations**, whose reference resolves into `plans/product/03-accuracy-policy.md`,
and **known failures**, whose reference resolves into a task file or
`conformance/known-failures.md`. Only the second kind blocks the M2 exit, and `--strict`
is what refuses it. Accepted format boundaries and deliberate
secondary-oracle disagreements link to the accuracy policy. Everything else links to the
task file that owns it — C2a for harness bugs, C3b for ProTracker fidelity and the
voice-boundary swap, C9 for the S3M records catalogued in
`known-failures.md` — and each reason records the first observed mismatch from the pinned
run. Those remain M2 exit blockers.

### Adapter projections and the tick budget

Three comparisons are projections rather than plain field equality, and each is named in
the adapter's comments:

- **Loop wrap.** libxmp's own comparator accepts start/end equivalence at a loop boundary
  (`test-dev/compare_mixer_data.c:78-82`). The adapter reads the loop span out of the
  loaded `Module` — nothing is added to the committed C1 trace format for a
  comparison-only concern — and compares the two positions circularly inside that span, so
  a voice one frame past the wrap is not reported 63 frames away from itself.
- **Accuracy policy D18.** libxmp inspects `NOTE_SAMPLE_END` after `xmp_play_frame` has
  rendered the interval and omits the voice; C1 snapshots the channel at the event
  boundary before it. When a StarPlayer voice is active, libxmp has no record for that
  channel on that tick, the region is one-shot, and the voice cannot survive the interval,
  the pair is projected as matching. Every other absence stays an active-set mismatch.
- **Per-field waivers.** A fourth `waive=field[,field]` column in `exclusions.tsv` makes
  the differ ignore exactly those fields for exactly that case and enforce every other
  one, so a fixture excluded for one documented representation difference still verifies
  the effect it exists to test. An unknown or structural field name is a hard error. A
  case that waives `frame` has declared its two timelines incomparable, so its records are
  anchored on the first matching `(row, frame)` and then tracked by residual offset.

The capture tick budget is derived from the oracle's **last timestamp**, not its record
count: libxmp writes no line for a tick with no mapped active voice, so the record count
is only a lower bound. Exhausting the budget is reported as a harness error that fails the
run, never as a case result.

All three formats are now registered through their own native trace paths — MOD by C3,
MTM by C4, S3M from M1 — so the runner's pending-integration table is empty and its
`gated` column is always zero. Every pinned case therefore executes and reports a pass or
a named exclusion; a missing loader would be a gate, never an exclusion, but no format is
in that state.

## Current standing

Task G3 wired Impulse Tracker in, which took the manifest from 47 cases to **168**: the 121
IT `compare_mixer_data` pairs join the MOD, S3M and MTM ones. **72 of the 168** pass —
MOD 16 of 27, S3M 16 of 17, MTM 1 of 3, IT 39 of 121 — with 13 accepted deviations and 83
known failures, one of them the pre-existing S3M record and 82 of them G3's own
(`G3-IT-001` … `G3-IT-008` in `conformance/known-failures.md`, grouped by the first field
that diverges so each group is one piece of work). Thirteen IT cases pass with every field
enforced and twenty-six more waive `position` under accuracy-policy **D67**. The M2 figures
below are unchanged by G3 and are the MOD/S3M/MTM half of that total.

After the C2a harness repairs, C3b's ProTracker fidelity repairs, the C9 S3M repairs and
C5's tracker dialects, **33 of the 47** M2 cases pass: MOD 16 of 27, S3M 16 of 17,
MTM 1 of 3. Thirteen of the MOD passes and ten of the S3M passes waive one or more fields
under an accuracy-policy entry and enforce every other field. The remaining 14 split into
13 accepted deviations and **1** known failure:

- **Accepted accuracy-policy deviations**: the oracle-representation entries D14–D39 and
  the §4 Startrekker AM-synth cases. D12 is no longer among them — the queued sample swap
  is implemented, and the two cases that still fail around it (`openmpt-mod-swap-no-loop`,
  `openmpt-mod-portamento-sample-change-pt`) fail on D18's post-mix omission rather than
  on the swap: a queued stop that falls due inside a tick interval makes libxmp omit the
  channel for that whole tick, and the adapter's D18 projection covers only a one-shot
  running out.
- **Tracker dialects**, delivered by
  `plans/engine/complete/M2-task-C5-quirks-and-tempo-models.md`: the five Octalyser / Digital
  Tracker MOD cases and the five S3M `cwtv` pattern-loop modes now select a `FormatDialect`
  detected from the file header. All ten pass, two of them
  (`libxmp-s3m-pattern-loop-imf-breakjump`, `libxmp-s3m-pattern-loop-st301-breakjump`) with
  no waiver at all and no exclusion row. `libxmp-s3m-pattern-loop-mpt-breakjump` had
  already started passing on the shared ST3.21 flow path C9 corrected.
- **Tracked repairs**: one record in `conformance/known-failures.md` — `C2-S3M-009`, the
  `Rxy` tremolo phase question left by C9. (`C2-MOD-001`, the pairer's residual-offset
  tracking across a 1300-frame tempo flip, is resolved: `pair_by_time` re-anchors a
  waived timeline on `(row, tick_in_row)`, and `libxmp-mod-pattern-loop-dt` passes
  across the fixture's alternating 255 and 63 BPM sections while `frame` is waived for D15.)

C5's own before/after, on the pinned corpus: MOD went from 11 of 27 to 15 of 27 (16 after the harness re-anchor) and S3M
from 11 of 17 to 16 of 17, and no case outside the ten dialect targets changed state. The
`QuirkSet` fields it added that no corpus case can see — the Paula clock (D14) and the
ProTracker tremolo-ramp phase (D20) — are observed by unit tests instead.

C3b's own before/after, on the pinned corpus: MOD went from 7 of 27 to 11 of 27.
`openmpt-mod-vibrato-reset` was fixed by the LFO magnitude rounding;
`openmpt-mod-instrument-swap`, `openmpt-mod-swap-empty` and `openmpt-mod-stopped-swap`
were fixed by the voice-boundary swap. Four MOD cases that C3b's task file listed as
targets were re-diagnosed rather than fixed and now cite the accuracy policy:
`openmpt-mod-delay-break` (D21, PT's tick-zero `E9x` retrigger),
`openmpt-mod-portamento-swap-pt` (D22, PT's tick-zero instrument latch under `EDx`), and
the two D18 cases above. C9's: S3M went from 2 of 17 to 11 of 17, with D24–D39 recording
what it found.

Because the old aligner returned a string error before the differ ever ran, most of the
pre-C2a reasons named an alignment position rather than a state difference. Every reason
in `exclusions.tsv` now records the first divergence the repaired harness actually
observed, and several of them are in a different place — and of a different kind — from
what was recorded before.

The pass count is the honest measure of M2's third exit criterion; the harness exiting
zero on 33 exclusions is not, which is what `--strict` exists to say.

## Deviation from the C2 task file

C2's deliverable named an OpenMPT corpus acquisition step. That was substituted: the
`openmpt-*` rows use libxmp's own checked-in copies of OpenMPT's player-test modules and
the frame dumps generated for them (23 oracle cases; 23 of the pinned OpenMPT binaries
remain documented-only). This keeps every byte under one immutable archive pin and avoids
a second, unlicensed download, but it is a scoped deviation from the task as written, not
a completed deliverable.

## Perceptual comparison against libopenmpt

`cargo xtask perceptual` is roadmap T10: the check that hashes and per-tick traces cannot
express, "does it sound like libopenmpt". It is a **nightly report with a tolerance, not a
gate** — `.github/workflows/perceptual.yml` runs it, `ci.yml` does not, and no score can
fail a build. `--threshold-snr DB` adds an advisory line naming the fixtures below it.

The oracle is `openmpt123`, the renderer inside libopenmpt's own source tree.
`cargo xtask openmpt` downloads the pinned tarball, verifies its SHA-256 and builds it
with `make` and every optional backend switched off (`NO_ZLIB`, `NO_MPG123`, `NO_OGG`,
`NO_VORBIS`, `NO_VORBISFILE`, `NO_FLAC`, `NO_SNDFILE`, `NO_PORTAUDIO`, `NO_PORTAUDIOCPP`,
`NO_PULSEAUDIO`, `NO_SDL2`, plus `EXAMPLES=0 TEST=0 SHARED_LIB=0 DYNLINK=0`), so it needs
nothing but a C++17 compiler. The version, URL and checksum are constants in
`xtask/src/main.rs`; a version marker in `target/openmpt/` skips a current build.
`--fetch-only` prepares the source cache, `--offline` refuses to download.

Both renders are 44 100 Hz stereo 32-bit float with linear interpolation
(`openmpt123 --filter 2`), unity gain, no dithering and no repeat; StarPlayer's side is
`render_song` with `RenderLength::default_for(44_100)`. They are aligned at frame zero,
trimmed to the shorter, normalised to equal RMS, and scored with a segmental SNR over
4096-frame segments and a log-spectral distance (Hann, 4096-frame frames, 50 % overlap).
The RMS ratio before normalisation is reported too, because a level mismatch is itself a
finding. The table goes to `target/perceptual/report.tsv`, and both renders are kept
beside it as headerless interleaved float — `openmpt123`'s WAV writer needs libsndfile,
which the dependency-free build deliberately does not have.

Read the research resolution in
`plans/engine/complete/M3-task-D7-perceptual-comparison.md` before reacting to a number.
The measured baseline is low by design: StarPlayer's canonical exact tick length and
libopenmpt's ST3-style truncated one diverge by a fraction of a frame per tick, which a
sample-domain SNR punishes and a spectral distance does not.

### Licensing

libopenmpt is BSD-3-Clause. Its source is downloaded into the ignored build cache
`target/openmpt/`, exactly like the libxmp corpus, and **nothing of it is committed** —
no source, no binary, and none of the WAV or raw renders under `target/perceptual/`.

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
