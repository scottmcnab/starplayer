# Library playback fixes — review and verification

| Field | Value |
|---|---|
| Date | 2026-10-29 |
| Status | Landed on main; automated verification passed; owner listening/device acceptance pending |
| Integration branch | library-playback-fixes |
| Baseline | main 556ed6a |
| Changes | UI 6ab1904; duration dc8c8a1; MOD compatibility 03e871a |

## Delivered behavior

The six failing archive members are tagless 15-sample Soundtracker MODs, a layout
previously excluded explicitly. They now load through conservative public dispatch,
reader loading, incremental images, the web worklet's panning preferences and native
trace helpers. Native packed MOD cells and distinct Soundtracker/UltimateSoundtracker
dialects are preserved. The accepted subset, byte-unit loop offsets, initial sample
prefix skip, UST timing/effects and valid later-Soundtracker restart positions are
specified in the accuracy policy. This is bounded compatibility support, not a claim
that every signatureless variant or damaged rip is accepted.

HNZ-SAS.XM has 19 orders and restart position 17. The sequencer constructor incorrectly
started the duration scan at that restart position. Host playback explicitly sought
order zero, so it played the omitted introduction and middle. Initial construction
now starts at the first playable order at or after zero; only later wraps use restart
17. No loop-detector or XM effect semantics needed changing. At 44,100 Hz the duration
changes from 371,368 frames / 8.421043084 seconds to 3,353,921 frames / 76.052630385
seconds. Scan and live completion match exactly; the UI shows 1:16.

Library warnings expose their reason directly on an explicit exclamation marker.
Playlist availability errors use a yellow exclamation before the filename, retaining
the full reason in tooltip and accessible text without displaying it inline. Library
selection can be cleared with checkboxes, the heading, Escape or an ordinary second
click on its sole selected row. Lower details require a selected focused row; a native
expanded-by-default disclosure retains its collapsed state across row changes. The
Instruments table places Play immediately before the note-overlap checkbox.

Task specifications and source evidence:
[MOD compatibility](../../engine/complete/LF-task-mod-compatibility.md),
[XM duration](../../engine/complete/LF-task-xm-duration.md),
[web UI](LF-task-library-ui.md).

## Review findings resolved

- The worklet's MOD panning loader bypassed public dispatch and still called the
  tagged loader. Its legacy branch and audible admission/panning regression close
  that second load path. The MOD-only trace helper also accepts legacy input while
  rejecting other formats.
- A valid later-Soundtracker restart must survive both image conversion and public
  NativeSequencer construction. It is stored in private format_extra bits8–14,
  passed into both constructors, and covered by direct/public wrap regressions.
- Incremental image preparation uses header-only preflight and validates/classifies
  legacy cells during its existing bounded scan. An invalid cell beyond the first
  4096 input bytes is reached only on the next scan step, pinned by regression.
- Existing nonzero-restart timeline tests removed their corrective initial seek;
  scanning and playback now use identical settings and preserve multiple wraps.
- UI checks target actual symbol tooltips and native disclosure/selection events,
  with header/body column order and audition behavior checked in packaged Chromium.

## Combined verification

All Cargo checks used CARGO_BUILD_JOBS=2, CARGO_INCREMENTAL=0 and RUST_TEST_THREADS=2.
The coordinator used the checkout's existing target directory for integration and
packaging; workers used a separately coordinated temporary target.

- All eleven `cargo xtask ci` gates passed across the initial run and the host-test
  rerun: host tests, conformance, RT safety, goldens, SIMD, FMA audit, trace zero cost,
  WASM build, no-std builds, Clippy and no-std purity. The initial sandbox run passed
  ten gates; the cast loopback-route test could not access networking. Re-running
  `cargo xtask ci --job host-tests` with local network access passed. It reported
  2,339 passing checks across 160 test binaries, with six existing ignored checks.
- Conformance: 261 cases, 179 pass, zero unexpected failures, 16 accepted deviations,
  66 documented known failures and zero gated cases. All eight canonical golden
  reference files passed without regeneration.
- RT checks passed with the pinned corpus available, including allocator hooks,
  garbage retirement, dense IT playback and telemetry. Required block sizes
  1,3,64,128,4096,8191 remain enforced; a new legacy-MOD regression covers them too.
- Full serialized Node suite: 394 passed, zero failures. The dev-server tests need
  localhost access; the sandbox attempt failed there, and the permitted rerun passed.
  Syntax and `git diff --check` checks passed.
- `cargo xtask wasm` rebuilt the development web bundle in this checkout. Source
  preflight hashes matched packaged assets in every browser run.
- Packaged Chromium checks passed in SAB, forced fallback and plain HTTP modes:
  `--w15-case view`, `--playlist-selection-only`, and `--interactive-only`.
  These cover Library clearing/disclosure/tooltips, Playlist native selection,
  compact warnings, persistence isolation, bulk removal, and Instruments audition,
  touch/native keys, checkbox isolation, held previews and replacement behavior.
- Desktop1280x960 and phone390x844 checks preserve 32px rows and contained scrolling.
  Phone page width is375px within the390px viewport; Playlist table scrolling stays
  inside its329px viewport. The warning is7.234375px wide, yellow, before the filename;
  the Filename column remains240px and contains no inline diagnostic. Desktop/phone
  screenshots were inspected, including Play left of the instrument checkbox.

## Actual-file verification

The original library was read only. No music files were added to the repository.
Independent rebuilt CLI renders produced88,200 mono16-bit frames per file (two seconds
at44.1kHz, fixed path), each with nonzero audio:

| File | Nonzero samples |
|---|---:|
| AXELF.MOD | 88,179 |
| BLOODMNY.MOD | 88,092 |
| IMPACT.MOD | 88,183 |
| JM_JARRE.MOD | 79,416 |
| RSI1.MOD | 88,005 |
| MFC2.MOD | 88,033 |

The rebuilt CLI reports HNZ-SAS.ZIP length1:16.053,3,353,921 frames and natural end.
A temporary browser audit reused the repository's CDP helpers and loaded all six actual
MODs plus the actual HNZ-SAS.ZIP in all three browser modes. Every MOD reached playing
with advancing rows, known duration and positive output peak. XM showed1:16 with
progress maximum3,353,921 at44,100Hz in every mode. Original files and temporary audit
scripts were not committed.

## Local evidence and remaining acceptance

Logs are under `/tmp/starplayer-lf-`: `ci.log`, `host-tests.log`,
`node-unsandboxed.log`, `wasm.log`, `real-cli.log`, `real-browser.log`,
`library-browser.log`, `playlist-browser.log` and `instruments-browser.log`.
Playlist screenshots are `/tmp/starplayer-w18-playlist/playlist-{mode}-{desktop|phone}.png`;
Instruments screenshots are `/tmp/w17-instruments-{mode}-{desktop|phone}.png`.

The integration branch lands on main with a no-fast-forward merge because it contains
several meaningful changes. Task plans move to complete with this landing. Owner
listening and physical-phone acceptance remains; no remote push or deployment was
performed. The original historical trees and owner scratch files are untouched.
