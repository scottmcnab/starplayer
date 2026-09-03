# M3 — Native surfaces

| Field | Value |
|---|---|
| Goal | cpal host, CLI player, offline WAV renderer, and the telemetry split |
| Estimate | 1.5u |
| Depends on | M2 |
| Blocks | A1 (TUI) |

## Why after WASM, not before

Native audio output is the *easier* host. Doing it second means the host abstraction was
designed under the harder constraint (a 128-frame quantum, a worklet-scoped module, no
threads by default) and a cpal backend then falls out nearly free. Doing it first would
have produced an abstraction shaped around a comfortable API that the browser then did
not fit.

## Deliverables

0. **`starplayer::NativeSequencer`** (task D3) — one format dispatch in the facade,
   lifted from the wasm host's private enum, so the cpal host, the CLI, the offline
   renderer, the trace path and the conformance harness all turn a `Module` into a playing
   sequencer through the same code. A prerequisite for D4 and D5, and for M5's and M6's
   wiring tasks; see [the concurrency plan](M3-M6-concurrency-plan.md).
1. **`starplayer-host` abstraction** (task D4 — **landed**; named `starplayer-host`
   rather than `starplayer-audio` to match the two backend crates) — `AudioBackend`, the
   trait a backend implements, plus device enumeration, sample-rate and buffer-size
   negotiation, and stream lifecycle; and `Player`, the backend-neutral controller that
   owns the engine, the transport and the seek mailbox. The WASM backend is **not** yet
   retrofitted behind `AudioBackend` — the owner deferred that (2026-09-03) so cpal, the
   CLI and the TUI were not blocked on the riskier half — but it already shares the seek
   mailbox, the repeat slot and the output-depth post-stage, which is the part that had to
   move when `EventSource` became `Send`. The retrofit, which is the real test of whether
   the abstraction is honest, is a follow-up task.
2. **`starplayer-host-cpal`** (task D4 — **landed**) — ALSA and PulseAudio are both
   present on the dev machine; Windows and macOS come free via cpal.
3. **`apps/starplayer-cli`** — play a file, render to WAV, dump a trace, print module
   info. Argument surface deliberately echoes the original's where it still makes sense
   (`plans/reference/original-star-ui.md` §7): a mixing rate, a buffer size, a device
   selection.

   `render` has no natural length, so it inherits `RenderLength::default_for(rate)` from
   task D1 — play the song once, then fade over ten seconds into the second pass, capped at
   an hour — and exposes the three knobs that change it: `--repeat N` (extra passes through
   the repeating section), `--fade S` (fade length in seconds) and `--at-end cut|fade`
   (`cut` stops dead on the loop point with no fade). A song that *ends* of its own accord
   — a stop marker, or an order list that simply runs out — gets no fade whatever is asked
   for, because there is nothing to fade away from (task D2).

   `info`, `render` and `trace` landed with task D5, along with the WAV writer
   (`starplayer_offline::wav`). `render` also gained a `--golden` switch beyond the
   original argument surface: the general length knobs cannot always reproduce a
   committed golden bit-for-bit (a song shorter than the ten-second golden window is a
   counterexample — `MOVEMENT.S3M`), so `--golden` calls the golden-generation functions
   directly instead; see task D5's research resolution. `play` is still a stub naming
   task D4, which this task depends on.
4. **`starplayer-offline`** — deterministic rendering to WAV at any rate and depth, with
   the higher-quality interpolators when M7 lands. `song_timeline` and `render_song`
   landed early with task D1, because the web player's progress slider needed the scan.
5. **The telemetry split** (architecture §9): the coherent scalar snapshot from M1-B6
   gains its second half — per-channel lossy audio taps for oscilloscopes and VU peaks.
   Per-channel fixed rings, `Relaxed` write index, downsampled in the audio thread
   (peak-per-16-frames) so it ships 32 frames per quantum rather than 4096. Tearing is
   acceptable and invisible on a scope.
6. **Perceptual comparison vs libopenmpt** (T10) as a nightly job — spectral distance or
   segmental SNR with a tolerance. It catches "sounds wrong" that hashes cannot express.

## Exit criteria

`starplayer play foo.s3m` works on Linux; `starplayer render foo.mod -o out.wav`
produces a file byte-identical to the browser's output at the same settings; scope data
reaches a consumer without blocking the audio thread.

## Out of scope

The TUI (that is A1). Windows and macOS *testing* — cpal should give them for free, but
verifying them is A2's problem.
