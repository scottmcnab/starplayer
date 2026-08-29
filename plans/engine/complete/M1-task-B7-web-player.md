# M1-task-B7 — The web player

| Field | Value |
|---|---|
| Milestone | M1 ([master plan](M1-master-plan.md)) |
| Depends on | M0-A4 (AudioWorklet spike), B6 (telemetry) |
| Blocks | M1 exit |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (headless browser) + **owner (plays a real module, sounds right)** |

## Context for a fresh agent

M0-A4 proved the AudioWorklet plumbing with a sine wave. This task replaces the sine with
the real engine and puts a usable player around it. **This is the milestone's exit
criterion and the owner's chosen first deliverable.**

Read M0-task-A4's Research points and its recorded answer to architecture open question
Q1 (`plans/product/01-technical-architecture.md` §9) before starting — it says whether
`SharedArrayBuffer` or `postMessage` is the practical telemetry transport, and that
decision shapes this task.

## Deliverables

1. **Engine in the worklet.** Replace A4's sine generator with the real render loop, a
   loaded `Arc<Module>`, and the S3M sequencer. Wasm memory is pre-reserved at init and
   never grown in `process()`.

2. **Module loading.** Drag-and-drop, a file picker, and loading from a URL. The bytes
   reach the worklet as a transferred `ArrayBuffer`; loading itself happens **off** the
   audio thread and the finished `Arc<Module>` is handed in over the command ring, with
   the retired module returned down the garbage channel (architecture §8).

   This is the first real exercise of `ModuleReader`'s zero-copy `&[u8]` path.

3. **Transport controls**: play, stop, seek by order position, next/previous order,
   master volume. All over the SPSC command ring — never a `postMessage` per interaction.

4. **The display**, reading `Snapshot` from B6:
   - order position and count, pattern number, row, speed, BPM;
   - module title, and a sample/instrument list;
   - a per-channel row: instrument name, note, volume, pan, VU bar, and the **effect
     spelled out in English** (the original's signature feature —
     `plans/reference/original-star-ui.md` §2.3);
   - a pattern view showing rows around the current position, via `PatternCell`.

5. **Responsive layout** that works on a phone as well as a desktop browser. The
   per-channel table is the part that needs real thought at narrow widths — decide
   whether it scrolls, collapses or paginates, and say why in a comment.

6. **Graceful degradation**: if `SharedArrayBuffer` is unavailable (no COOP/COEP), the
   scope-oriented transport falls back to `postMessage` and the page still works. Report
   which transport is in use somewhere unobtrusive.

7. **Error handling.** A malformed module produces a readable message, not a broken page
   and not a dead audio context. The audio graph survives a failed load.

## Research points

1. Whether a framework earns its keep here. The page is small; plain modules plus a
   template may be less total work than a build step, and it keeps `xtask wasm` simple.
   Prefer the smaller toolchain unless the pattern view argues otherwise.
2. How to keep the pattern view's redraw cheap. At 50 ticks/second, rebuilding a DOM
   table every tick will be visibly bad — canvas, or a windowed table with cell reuse.
3. Mobile audio-context unlocking rules on iOS Safari; they are stricter than Chromium's
   and have changed. Verify against a real device if one is available.

## Verification

- A real `.s3m` loads and plays, in Chromium and Firefox, and **sounds correct to the
  owner**.
- Transport controls behave: seek lands on the right order, stop silences cleanly with no
  click, play resumes.
- The display tracks playback, and the row shown is the row that is sounding.
- Loading a second module while the first is playing does not glitch and does not leak —
  assert the retired `Arc` reached the garbage channel.
- A truncated or non-module file shows an error and leaves the player usable.
- Audio runs for at least five minutes with no dropouts, no console errors and no wasm
  memory growth.
- The page is usable on a phone-width viewport.
- **Owner check**: the player is pleasant to actually use.

## Out of scope

Oscilloscopes (M3 telemetry). MOD and MTM (M2). Playlists, a file browser, or anything
resembling the original's 64-slot manager — those belong to the TUI homage (A1) if
anywhere.
