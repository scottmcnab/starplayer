# StarPlayer — Roadmap, Testing Strategy and Risks

Effort units: **1 unit ≈ one focused week**. ‖ marks work that can proceed in parallel.
Dates assume a start in late August 2026.

## Sequencing principle: prove the hardest host first

The roadmap is ordered around **getting audible output in a browser as early as
possible**, because the WASM AudioWorklet path is simultaneously the owner's chosen
first deliverable *and* the highest-uncertainty infrastructure in the project. Native
hosts (M3) come *after* WASM deliberately: the host abstraction is proven under the
harder constraint first, and a cpal backend after that is nearly free.

The second ordering principle is that **the accuracy machinery arrives with the second
format, not the first**. M1 gets S3M audibly right; M2 adds MOD and MTM *and* the trace
diffing, conformance corpus and golden hashes that prove all three. Building the harness
before there is anything to test is how a project spends a month and produces silence.

## Milestones

| # | Milestone | Exit criterion | Est. |
|---|---|---|---|
| M0 | Foundations + WASM spike | **Sound from a browser tab** — a Rust sine wave through AudioWorklet | 1u |
| M1 | S3M in the browser | **A real `.s3m` plays correctly in a browser** | 2.5u |
| M2 | MOD + MTM native, accuracy machinery | Conformance corpus green for MOD/S3M/MTM | 2u |
| M3 | Native surfaces | cpal host, CLI player, offline WAV renderer, telemetry split | 1.5u |
| M4 | Generalise to a synthesis engine | MIDI in, SMF playback, keyboard triggering, source mux | 1.5u |
| M5 | XM support | Envelopes, key-off/fadeout, linear frequency, multi-sample instruments | 1.5u |
| M6 | IT support | NNA/DCT/DCA, voice stealing policy, resonant filter, compressed samples | 2u |
| M7 | DSP graph | Per-channel inserts, master bus, reverb/chorus/compressor, SIMD, higher-order interpolation | 1.5u |
| M8 | Embedded proof | ESP32-A1S (Xtensa) playing a module from flash through its ES8388 under Embassy, with keys, an optional ST7789 screen and web control; ESP32-C5 proves the RISC-V build | 2.5u |
| M9 | Plugin surfaces | CLAP instrument, then effect hosting; VST3 if warranted | 2u |
| M10 | Alternative synths | FM, wavetable, SID, SoundFont; sample-enhancement plugin API | pull-driven |
| M11 | Instrument library | Tracker-module instruments as a MIDI-playable bank; content-hash dedup | pull-driven |
| A1 | TUI STAR.EXE homage | The original screen, in a modern terminal | 1.5u |
| A2 | Desktop shells | Windows / macOS over the same engine | pull-driven |
| A3 | Mobile shells | iOS / Android over the same engine | pull-driven |

**M0–M1 is the critical path** to something the owner can actually use. M2–M3 make it
provable and portable. M4–M6 complete the engine and format story. **M7 onward is
explicitly pull-driven** — each milestone's master plan states its own trigger.

---

## M0 — Foundations + WASM spike (1u)
*"Nothing musical, everything unblocked."*

No tracker code at all. Deliverables:

- Workspace skeleton with every crate from the architecture document stubbed and wired,
  plus `xtask`.
- `starplayer-core`: fixed-point types (`Q16.16`, `Q32.32`, `U0F16`, `I1F15`), `Frame`,
  `Step`, `Note`, the frame clock, `TempoModel` with all three implementations.
- `starplayer-mixer`: rendering a single voice, enough for a sine.
- **An AudioWorklet playing a Rust-generated sine wave in a browser.**
- CI: host `cargo test`, `wasm32-unknown-unknown` build, `riscv32imc-unknown-none-elf`
  `no_std` check, clippy with `-D warnings`, `forbid(unsafe_code)` verification.
- The **block-size determinism test** (render at 1, 3, 64, 128, 4096, 8191 frames →
  byte-identical).

The AudioWorklet spike is first because it is where the unknowns are: no
`SharedArrayBuffer` without COOP/COEP headers; no `fetch` or dynamic `import` inside a
worklet in some browsers, so the wasm module must arrive as bytes via `postMessage`; a
hard 128-frame quantum; growing wasm memory on the audio thread causes an audible
glitch; `wasm-bindgen` output needs manual massaging for worklet scope. Prove all of it
with a sine wave before anything depends on it.

Toolchain prerequisites not yet present on the dev machine: `rustup target add
wasm32-unknown-unknown`, and `wasm-bindgen-cli` / `wasm-pack`. The riscv32 targets and
Node 24 are already installed.

## M1 — S3M in the browser (2.5u)
*"A real module plays, correctly, in a browser tab."*

- `starplayer-model`: the `Module` blob-and-offsets layout, `SampleIndex`,
  `PatternIndex`, `InstrumentDef`, guard frames, display-only `PatternCell`.
- `starplayer-s3m` loader: `SCRM` validation, parapointers, packed pattern unpacking,
  unsigned 8-bit samples, default panning from the channel-settings array and the
  optional 32-byte pan block.
- `starplayer-s3m` effect processor: the full `Axx`–`Xxx` set with tick-0 vs per-tick
  semantics, shared D/E/F and H/R/U parameter memories, decimal `Cxx`, `SDx` note delay
  via the saved dirty-flag byte, `SBx` pattern loop, `SEx` pattern delay, glissando and
  Amiga-limit clipping.
- `starplayer-engine`: the render loop, `RowClock`, the `RENDER_QUANTUM` adapter, the
  voice pool, the command queue.
- `starplayer-mixer`: linear-interpolated float stereo mixing and output conversion.
- Telemetry v1 (coherent scalar state only).
- `apps/starplayer-web`: transport controls, order/pattern/row display, per-channel note
  and effect display, VU meters.

The effect task spec is written **effect by effect from the assembly**, each stating its
tick-0 behaviour, its per-tick behaviour, its parameter memory, and any canonical
deviation being taken per `03-accuracy-policy.md`.

## M2 — MOD + MTM native, plus accuracy machinery (2u)
*"Three formats, and proof that they are right."*

- `starplayer-mod` and `starplayer-mtm`: native loaders and effect processors, *not* an
  S3M conversion. The original's conversion tables are reproduced as MOD/MTM semantics:
  the two finetune → C2SPD tables, the LRRL channel panning map, the loop-length > 4
  gate, the Amiga-limits derivation from the song's octave range, and the sign
  conventions.
- `QuirkSet` and the `TempoModel` policies wired end to end.
- The fixed-point mixer path — the canonical bit-exact reference.
- The per-tick trace format and the trace differ in `starplayer-testkit`.
- The libxmp `test-dev/` and OpenMPT `test_*` corpora in CI.
- Golden hashes and `cargo xtask goldens`.
- Loader fuzzing.

## M3 — Native surfaces (1.5u)

Audio host abstraction and the cpal backend; `apps/starplayer-cli` (play, render to WAV,
dump trace); `starplayer-offline`; the telemetry split (coherent scalars + lossy scope
taps).

## M4 — Generalise to a synthesis engine (1.5u)

This is where `Instrument` is extracted properly and the musical event vocabulary is
finalised — informed by two real format implementations rather than by speculation in
M0. Plus `starplayer-midi` (byte codec + SMF parser), `ExternalEventQueue`, live MIDI
input, computer-keyboard triggering, and `SourceMux`.

## M5 — XM support (1.5u)

Volume and panning envelopes, key-off and fadeout, auto-vibrato, the linear frequency
table, multi-sample instruments with a note→sample map, 16-bit and delta-encoded
samples, and the XM effect set. This is what forces `Instrument` to be honest.

## M6 — IT support (2u)

New Note Actions, Duplicate Check Types and Actions, the global virtual-channel pool
with a voice-stealing policy matched to libopenmpt, the resonant filter with its
envelope, IT's extended effect set, compressed sample decoding, and instrument-mode
note maps. Budget generously — this is the hardest format, and the filter plus NNA
accuracy are worth two weeks on their own.

## M7 — DSP graph (1.5u) — pull-driven

Per-channel insert chains, master bus, reverb, chorus, delay, compressor, EQ; SIMD
kernels with a scalar-equivalence gate; cubic and windowed-sinc interpolation.

## M8 — Embedded proof (2.5u) — pulled 2026-09-11

The `no_std` CI check proves it compiles; this proves it *plays*. Re-planned for the
owner's hardware: an **ESP32-A1S Audio Kit** (classic Xtensa ESP32, ES8388 codec) renders
a module borrowed from memory-mapped flash through I2S under Embassy on the fixed-point
mixer, with its six keys as controls, an ST7789 now-playing screen behind an `lcd`
feature, and captive-portal web control; an **ESP32-C5** renders the goldens on RISC-V.
The firmware is its own workspace at `embedded/`; the board-independent host is a
`no_std` crate in the main workspace. Validates the portability claims before they
calcify. Settles architecture open question Q2.

## M9 — Plugin surfaces (2u) — pull-driven

CLAP instrument first (`ExternalEventQueue` already has the right shape), then effect
hosting, then VST3 via a wrapper if it is warranted.

## M10 — Alternative synths — pull-driven

FM, wavetable, SID emulation, SoundFont; and the sample-enhancement plugin API for
upscaling or AI-generated high-resolution harmonic content from 8-bit sources.

## M11 — Instrument library — pull-driven

Scan a library of MOD/S3M/MTM/XM/IT files and expose every instrument as a MIDI-playable
bank, each running its own format's envelopes and articulation. Little new synthesis;
the work is instrument extraction, tuning conventions, content-hash identity and dedup.
Shares its `InstrumentBank` abstraction with M10's SoundFont support, and is the right
place to run M10's sample enhancement — once per unique sample rather than per module
load.

---

## Testing strategy

Ordered by value per unit of effort. Detail and rationale in `03-accuracy-policy.md` §5.

| # | Test | Lands |
|---|---|---|
| T1 | **Block-size determinism** — 1, 3, 64, 128, 4096, 8191 frames → byte-identical | M0 |
| T2 | **Per-tick state trace diffing** — the highest-value tool in the project | M2 |
| T3 | **libxmp `test-dev/` suite** — purpose-built modules *with* expected per-frame channel state. Both corpus and oracle. Mine it before writing effect code | M2 |
| T4 | **OpenMPT `test_*.{mod,s3m,xm,it}`** — one compatibility quirk each, documented on the wiki | M2, extended at M5/M6 |
| T5 | **Golden hashes** — SHA-256 of a fixed-point i16 mono 44100 Hz linear render, DSP bypassed; config encoded in the filename | M2 |
| T6 | **Cross-target hash equality** — x86-64, aarch64, wasm32 agree on the fixed path | M2 |
| T7 | **Loader fuzzing** (`cargo-fuzz`) — never panic, never OOM, always `Err` | M2, per format thereafter |
| T8 | **RT-safety** — allocator hook panicking on any allocation inside `render()` | M2 |
| T9 | **Properties** — no NaN/Inf; voice pool returns to zero active after song end; `next_event_frame()` never returns the past; the zero-advance guard never trips on the corpus | M2 |
| T10 | **Perceptual comparison vs libopenmpt** on the float path — spectral distance / segmental SNR with tolerance. Nightly, not a gate. `cargo xtask perceptual`: `openmpt123` built from a checksum-pinned libopenmpt source tarball into `target/openmpt/`, both renders RMS-normalised, segmental SNR and log-spectral distance per fixture into `target/perceptual/report.tsv`; `.github/workflows/perceptual.yml` runs it nightly and uploads the table | M3 |

Golden WAVs stay **out of the repo**; only their hashes are committed, and
`cargo xtask goldens` regenerates them.

---

## Risks

**R1 — Effect-semantics fidelity.** The 6,317-line assembly is the only specification
for MOD/S3M/MTM, and the subtleties (tick-0 vs per-tick, shared parameter memories,
slide clamping, retrigger and tremor counters, glissando) are exactly where players
differ from each other.
*Mitigation:* effect-by-effect task specs written from the source; T2 trace diffing
makes deviations visible rather than merely audible; T3's libxmp oracle mined **before**
effect code is written.

**R2 — DSP block granularity silently breaking determinism.** Highest-probability silent
failure in the design (architecture §1.4).
*Mitigation:* the quantise-for-DSP rule, plus T1 present from M0 so the bug class can
never land.

**R3 — Real-time safety eroding silently.** `no_std` does not prevent it: dropping the
last `Arc<Module>` on the audio thread calls `free()`, and so does a `Vec` growth in a
"just for telemetry" path.
*Mitigation:* the garbage channel for retired `Arc`s, the T8 allocator hook in CI, and
clippy denials on the engine and mixer crates.

**R4 — AudioWorklet plumbing.** COOP/COEP, worklet-scope module instantiation, the hard
128-frame quantum, wasm memory growth glitching audio.
*Mitigation:* it is M0, proven with a sine wave before anything depends on it. Settles
architecture open question Q1.

**R5 — Abstraction paralysis.** Designing traits for SID + FM + physical modelling + VST
+ embassy before one note plays is exactly how this project reaches 80% design and 0%
audio.
*Mitigation:* the "no trait until its second implementation" rule (architecture §10.1),
and a roadmap that is explicitly pull-driven past M7.

**Honourable mentions.** IT filter and NNA accuracy are worth two weeks on their own.
A change of interpolator silently invalidates every golden — hence the config in the
filename. `alloc::sync::Arc` needs `portable-atomic` on no-CAS targets.
