# LF-M — Legacy library MOD compatibility

| Field | Value |
|---|---|
| Status | Landed on main; automated verification passed; owner acceptance pending |
| Baseline | main 556ed6a |
| Least-capable safe worker | GPT-6.1-sol (high) |
| Coordinator | library-playback-fixes; workers leave changes uncommitted |

## Context for a fresh agent

Read AGENTS.md first. Work only in the assigned sibling worktree. Historical
STARPLAY/ and STARPLAY-2.25s/ are read-only. No cargo fmt, unrelated edits, broad
staging, commits, merge, push or deployment. Leave changes uncommitted for review.
Preserve no_std, allocation-free render, sample-exact events, native formats and
block-size determinism. Library input is read-only under
/mnt/c/Users/scott/Documents/Projects/music. Never commit music files; use synthetic
regressions. Coordinate Cargo access with parent, using CARGO_BUILD_JOBS=2,
CARGO_INCREMENTAL=0, RUST_TEST_THREADS=2, CARGO_TARGET_DIR=/tmp/starplayer-lf-target.

The owner reports AXELF.MOD, BLOODMNY.MOD, IMPACT.MOD, JM_JARRE.MOD, RSI1.MOD and
MFC2.MOD in old/mods.zip fail although original StarPlayer played them. Own
crates/starplayer-mod and format dispatch in crates/starplayer/src if needed.
Coordinate other crate/shared document edits with parent.

## Deliverables

- Reproduce and explain exact rejection and layout for all six archive members.
- Add narrowly justified support that loads and plays all six, retaining bounds checks,
  byte-slice and ModuleReader loading, and conservative format probing/dispatch.
- Add synthetic regressions for observed formats and malformed/truncated false positives.
- Record settled behavior, actual-file evidence and verification below. Send parent any
  accuracy-policy amendments needed for new supported format contracts/deviations.

## Research points

Inspect archive extraction vs dispatch errors, header signatures, order/sample layouts,
existing legacy support and original assembly loader. Do not mistake decode failures
for format errors. Inspect existing conformance and sample-truncation policy.

## Verification

Run complete starplayer-mod tests, relevant public-dispatch tests and no-default-feature
checks. Reproduce all six actual files via public dispatch and offline render enough
frames to show playback. Coordinate Cargo access; parent owns CI/WASM/browser checks.
Do not regenerate canonical goldens.

## Out of scope

UI tooltips, XM timeline, broad loader redesign.

## Settled behavior and implementation evidence (2026-10-29)

The six ZIP members extract with valid CRCs. Their rejection was format dispatch:
all use the tagless 15-sample Soundtracker layout (600-byte header, length at470,
tempo/restart at471, orders472..599, four-channel 1024-byte patterns). None has a
recognized MOD tag at1080; baseline public `probe` returned None and `load` returned
BadMagic. This was not an archive decode failure or missing sample body.

| Member | Bytes | Orders | Stored patterns | PCM file offset | Declared PCM bytes | Extra trailing bytes | Header471 | Chosen dialect / initial BPM |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| AXELF.MOD |113192|83|33|34392|78800|0|120|UltimateSoundtracker /120|
| BLOODMNY.MOD |171904|31|15|15960|155920|24|127|Soundtracker /125|
| IMPACT.MOD |90964|27|11|11864|79100|0|120|Soundtracker /125|
| JM_JARRE.MOD |79112|27|13|13912|65200|0|120|Soundtracker /125|
| MFC2.MOD |95294|21|10|10840|84454|0|120|Soundtracker /125|
| RSI1.MOD |45544|7|6|6744|38800|0|112|Soundtracker /125|

Source archive SHA256:
`f24d6b2f2bc8dedec211474109f40ad81c0bdfbecd01dec9b8ac73614a096eaa`.
Member SHA256, respectively:

- AXELF: `6a3270a492a8da932cfc45debb9d992be2ce9aa8d2662fb5b78ca17e6e09cb64`
- BLOODMNY: `1749455dc393527e91ffee027baab16f5bdaf51ea7afe748ee4646165eccedeb`
- IMPACT: `ee350034b25fa469a5ce84d59501f30302bbaf584294c74652c050c2d745ece9`
- JM_JARRE: `afb8b807901a6a5d88767d74bae629f8ff4e54793555f6f61d8c7d155343008d`
- MFC2: `60046c57e3035cb9dadd0734de73bb882eb64944d42eb8064866ed8ae7d10e1c`
- RSI1: `41c6cf0df84e561be59ff2316560a43eb7dce4eac6728633021a543168590c5e`

`mod_file::soundtracker` provides explicit probe/load and ModuleReader entry points;
tagged MOD entry points retain their existing contract. The public facade tries the
structural legacy probe last, after all strong signatures and tagged MOD. Worklet
loading preserves its stereo-separation option on the legacy path; the MOD-specific
trace helper now accepts these files while still rejecting another module format.
The reader does not allocate during probing. Recognition reads at most128 patterns
(128KiB) synchronously on the control plane; image decoding checks the600-byte header
in its header phase and validates/classifies pattern cells within the existing4096-byte
step budget. Native cells are retained verbatim.

The supported subset intentionally requires printable-ASCII/NUL names, nonempty order
length<=128, all128 order entries<128, complete declared sample bodies totaling>=8 bytes,
valid sample metadata, at least one note, exact zero-finetune-table Amiga periods and
instrument<=15. Sample lengths up to32768 words are accepted (Blood Money needs a32768-byte
sample). The command subset is0/1/2/C/E00/E01/F, exactly covering the six reports;
unsupported extensions and damaged/truncated tagless rips remain rejected. Tagged MOD's
existing permissive missing-sample-tail policy is unchanged. Trailing bytes are accepted.

Primary compatibility evidence is pinned libxmp
[`src/loaders/st_load.c`](https://github.com/libxmp/libxmp/blob/6ec0ba21b1b28f91e22b68a51d59207c6bbf6139/src/loaders/st_load.c),
available in `/tmp/libxmp-6ec0ba21.tar.gz`. Its loader uses byte-unit loop starts,
word-unit sample/repeat lengths and skips the declared byte prefix on initial playback
as well as repeat playback. File offsets still advance by each full declared sample
length. The builder trims looping PCM after the loop end as usual. Non-UST files start
at125BPM and use VBlank Fxx (every nonzero value is speed). Valid later-Soundtracker
restarts are stored in format_extra bits8..14; invalid restarts normalize to0. UST
classification follows the pinned source's heuristic: header byte>=64, zero finetune,
sample/repeat lengths<=0x1387 words, loop start<=9999, only effect1 with nonzero parameter
or effect2 with only one nonzero nibble (zero000 cells allowed). UST uses header471 as
BPM, has restart0 and interprets1 as arpeggio,2-low-nibble as pitch-up and2-high-nibble as
pitch-down. AXELF alone meets it; no new RT allocation or transcendental math was added.

The historical released loader does not actually offer this family:
`STARPLAY-2.25s/S3MLIB.ASM:GetNumChans`2057–2096 recognizes tags and always sets
NumSamps31, otherwise aborting. This support is an intentional extension, not evidence
that the released2.25s binary played these six bytes. Foundation policy amendments are
owned by the coordinator: replace the blanket tagless exclusion with this explicit,
conservatively detected subset and preserve the unsupported-family limits above.

## Verification evidence

All Cargo commands used CARGO_BUILD_JOBS=2, CARGO_INCREMENTAL=0,
RUST_TEST_THREADS=2 and CARGO_TARGET_DIR=/tmp/starplayer-lf-target.

- `cargo test -p starplayer-mod -p starplayer --lib --tests`: PASS, including85 MOD
  tests (72 unit,3 quirks,2 existing determinism,8 new legacy regressions), public dispatch
  and image parity tests. The valid-restart regression depends on the coordinator's
  already-integrated shared PatternSequencer constructor fix (dc8c8a1). Its exact
  one-line engine fix was temporarily mirrored for verification and restored afterward;
  there is no engine diff in this worktree.
- `cargo test -p starplayer-host-wasm -p starplayer-offline --features starplayer-offline/trace soundtracker -- --nocapture`:
  PASS, both focused tests (worklet admission/panning/audible output; legacy trace and
  rejection of another format).
- `cargo check -p starplayer-mod -p starplayer --no-default-features --features starplayer/mod --target riscv32imac-unknown-none-elf`:
  PASS. First attempt encountered a stale sibling-worktree core fingerprint in the
  shared target; touching the edited core/model sources invalidated it and the clean
  rebuild passed.
- `cargo clippy -p starplayer-mod -p starplayer --tests -- -D warnings`: PASS.
- `git diff --check`: PASS. No cargo fmt or canonical golden regeneration.

A temporary external harness `/tmp/starplayer-lf-mod-audit` (not part of the repository)
loaded all six via public `starplayer::probe/load`, checked public incremental-image
output byte-for-byte against `Module::to_image`, and rendered88200 mono-i16 frames
(two seconds,44.1kHz,128-frame blocks) via public offline `render_song_fixed_mono`.
Every file passed with audible output. FNV-1a hashes below identify this local evidence;
they are not canonical golden claims or comparisons with DOS output.

| Member | Nonzero frames /88200 | Absolute peak | FNV1a64 over little-endian PCM | Image bytes | Incremental steps |
|---|---:|---:|---|---:|---:|
| AXELF.MOD |88179|32767|295056a047dcdd39|193904|272|
| BLOODMNY.MOD |88092|27829|1b464a22f034838b|329484|268|
| IMPACT.MOD |88183|32767|0eaca7de7f1e0380|157456|177|
| JM_JARRE.MOD |79416|16255|b402ec4a1be269ad|143548|174|
| MFC2.MOD |88033|15886|f6070276e221043c|175160|180|
| RSI1.MOD |88005|32767|b7f520d15e490874|85780|120|

New synthetic tests cover copying ModuleReader, attack/loop units and sample alignment,
UST tempo and both pitchbend directions at actual ticks, VBlank high Fxx, large samples,
all truncation prefixes, malformed headers/cells, tagged collisions,24-byte trailer,
owned/incremental image equality for both dialects (also pins append-only image codes26/27),
4096-byte pattern scanning, valid restart first-start0/wrap1 and byte-identical rendering
at host blocks1,3,64,128,4096,8191. Coordinator owns combined CI/WASM/browser checks
and review before committing. No music files, scratch harness or generated audio are
part of this change.

## Coordinator acceptance

Reviewed and verified in the combined integration branch. See [combined review and
verification](../../apps/complete/LF-review-and-verification.md) for CI, real-file, browser and landing evidence. Only owner
listening and device acceptance remains.
