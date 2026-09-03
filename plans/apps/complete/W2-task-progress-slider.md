# W2 — Progress slider, elapsed/total time and Repeat in the web player

| Field | Value |
|---|---|
| Milestone | Web player maintenance (follows W1) |
| Status | Landed 2026-09-03; owner browser check outstanding |
| Depends on | [M3-D1](../../engine/complete/M3-task-D1-song-timeline-and-loop-detection.md) (song timeline, `OPCODE_SEEK_FRAME`, `OPCODE_AT_END`, telemetry words 19–21) |
| Blocks | — |
| Recommended model | Claude Sonnet |
| Verified by | agent (headless harness + `cargo test -p starplayer-web`), then owner in a browser |

## Context for a fresh agent

`apps/starplayer-web` is a framework-free page (`www/index.html`, `www/style.css`,
`www/app.js`, `www/ring.js`, worklet files) built by `cargo xtask wasm`; read
`apps/starplayer-web/README.md` first. Transport today is order-based: `#previous`, `#play`,
`#stop`, `#next`, the `#seek-order` number box and the `#volume` range input in the
`.controls` row (`index.html` ~lines 89–101; handlers at the bottom of `app.js`;
`queueCommand(opcode, argument, extra)`; `playbackRestoreCommands()`; `setControlsEnabled()`;
`updateSnapshot()` driven by `requestAnimationFrame` in `refresh()`).

Task D1 added a song timeline in the engine and exposes it on the wire:
- telemetry header words (decoded by `Ring.decodeSnapshot` in `ring.js`): `songFrame`,
  `songLengthFrames`, `songFlags` (bit0 length known, bit1 ends by loop, bit2 end reached,
  bit3 fading);
- `Ring.OPCODE_SEEK_FRAME` (argument = song frame) and `Ring.OPCODE_AT_END` (argument
  `0` = fade out and stop at the loop point, `1` = wrap and continue; `extra` = fade length
  in frames).
`state.workletSampleRate` holds the context rate for frame ↔ seconds. Elapsed is the
engine's position, not compensated for `context.outputLatency` (neither is the row display).

Owner decisions: a **Repeat** checkbox, **checked by default** (song wraps at the loop point
and the slider wraps with it); unchecked, the song fades out over `SONG_FADE_SECONDS = 5`
into its second pass and stops, after which Play restarts from the top.

## Deliverables

1. **Markup** (`index.html`, inside `.controls`, after the transport buttons and before the
   Order box): a progress row — `<output id="elapsed">0:00</output>`,
   `<input id="progress" type="range" min="0" max="0" step="1" value="0" disabled
   aria-label="Playback position">` (value in frames), `<output id="duration">0:00</output>`,
   and `<label class="repeat-label"><input id="repeat" type="checkbox" checked> Repeat</label>`.
   Keep prev/next and the Order box.
2. **Style** (`style.css`): the row takes the full width (`flex-basis: 100%`) and sits after
   the buttons; the range uses `accent-color: var(--accent)` like `#volume`; the two outputs
   use the monospace numerals of `.position-grid strong` at control size and a fixed width
   so the slider does not jitter. Must survive the ≤820 px sticky controls and ≤520 px
   ordering rules; nothing overflows at 390×844 (the headless test asserts this).
3. **Behaviour** (`app.js`):
   - `formatTime(frames, rate)` → `m:ss`, `h:mm:ss` from one hour; `--:--` when the length
     is unknown (flag bit0 clear) and the slider disabled.
   - `updateSnapshot`: set `progress.max = songLengthFrames` and `#duration`; unless
     `state.scrubbing`, set `progress.value` and `#elapsed` from `songFrame` (clamped to
     `max` while fading). After a seek keep showing `state.pendingSeekFrame` until a snapshot
     with a newer `sequence` than the one at seek time arrives. When bit2 (end reached) is
     set and `playing` is false (post-fade stop), show `0:00` and slider 0.
   - Slider `input` → `state.scrubbing = true`, update `#elapsed` only; `change` →
     `queueCommand(Ring.OPCODE_SEEK_FRAME, value)`, record `pendingSeekFrame`, clear scrubbing.
   - `#repeat` `change` → `queueCommand(Ring.OPCODE_AT_END, checked ? 1 : 0,
     Math.round(SONG_FADE_SECONDS * rate))`. Send the same command when a module is
     activated and from `playbackRestoreCommands()` (graph rebuilds).
   - `setControlsEnabled` covers `#progress` and `#repeat`.
4. **Tests.**
   - `src/lib.rs` copy tests: assert the new ids (`elapsed`, `progress`, `duration`, `repeat`)
     and the label text `Repeat` exist in `index.html`.
   - `test/headless.mjs` transport section: after load, `#duration` is not `0:00`/`--:--`
     and `#progress.max > 0`; play, wait, assert `#elapsed` advanced and `#progress.value`
     > 0; set `#progress.value` to half of `max`, dispatch `change`, wait, and assert the
     snapshot's `order` moved to the order the timeline puts there (compare with the value
     the page shows in `#order`); untick `#repeat`, seek near the end, wait past the fade,
     assert the transport chip reads `stopped` and `#elapsed` reads `0:00`. Extend
     `READ_STATE` accordingly.
   - `test/ring-harness.mjs` already decodes the new words (D1); assert them if not.
5. **Docs.** README "Controls" (or nearest) paragraph: the slider, `m:ss` display, Repeat
   semantics and the fade, and the note that elapsed is engine position.

## Research points

- Continuous `input` events while dragging must not flood the command ring (capacity 64,
  drained per quantum): send only on `change`.
- `snapshot.sequence` is the seqlock counter; on the postMessage fallback path snapshots
  arrive every 8 quanta — `pendingSeekFrame` must work in both modes.
- `songFrame` is an `Int32` word; do not format negative values (treat as 0).

## Verification

```
cargo xtask wasm
cd apps/starplayer-web
cargo test -p starplayer-web
node test/ring-harness.mjs
node test/headless.mjs --mode sab --seconds 8      # skips cleanly without Chromium
node test/headless.mjs --mode fallback --seconds 8
```
Report the exact commands and whether the harness skipped. **Do not commit.**

## Out of scope

- Keyboard shortcuts for seeking; a time-remaining display; output-latency compensation;
  changes to Rust crates other than `apps/starplayer-web/src/lib.rs` tests.
