# W5 — A rendering-options disclosure and the experimental speed adjustment

| Field | Value |
|---|---|
| Milestone | Web player maintenance (follows [W4](complete/W4-task-enhancement-checkboxes.md)) |
| Status | Implemented 2026-09-19; owner browser check outstanding — move to `complete/` once landed |
| Depends on | [W4](complete/W4-task-enhancement-checkboxes.md) (the load-option reload dance and `enhancements_json()`) |
| Blocks | — |
| Recommended model | Claude Sonnet (one sequencer field, one wasm argument, one disclosure) |
| Verified by | `cargo test -p starplayer-engine -p starplayer-host-wasm`, `cargo xtask wasm && node apps/starplayer-web/test/headless.mjs`, then the owner in a browser on a Cubulus rip |

## Why

Two requests from the owner, in one change because they land on the same panel.

1. **The load options are noise on a first visit.** Headphone panning and four
   sample-enhancement checkboxes sit directly under the file controls, which is where
   someone who only wants to play a file is looking. They belong behind a disclosure.
2. **Some game modules play too fast.** The owner's rips from the Amiga game *Cubulus*
   run fast in every canonical player. The likely cause is the game's own replayer adding
   a constant to every speed command, so the module's `Fxx` values are one short of what
   the music was composed against. An **Adjust speed** control supplies that constant.

## What landed

### Engine — `crates/starplayer-engine/src/sequencer.rs`

`PatternSequencer` gained `speed_adjust: i8` (default 0) and `set_speed_adjust`, plus a
shadow `requested_speed`: the ticks-per-row the module last *asked for*, before the bias.
`TickContext::outcome()` now seeds its speed from that shadow rather than from the row
clock, so a module that re-requests the speed the clock is already running at still moves
it — the case a "did the effective speed change?" comparison silently misses. `commit`,
the one place a speed request reaches the row clock, applies the bias and clamps it to at
least 1. A seek restores the timing the scan recorded, which is already biased, so
`begin_seeked_row` takes the request back out of the mark it restores.

The header's initial speed is **not** biased: the bias is on requests, which is what the
game replayer did, and what makes the option observable at all on a MOD (a MOD has no
initial-speed field — it always starts at 6).

### Facade and host

* `starplayer::scan_song_with_speed_adjust` — `scan_song` is that function at 0. The scan
  *is* the song's length and every seek target, so it has to run at the adjustment
  playback runs at. For a MOD this also feeds the CIA-against-VBlank length comparison,
  which is correct: both candidates are measured under the same bias.
* `NativeSequencer::set_speed_adjust`, forwarded across the format arms.
* `starplayer_host::scan_module_with_speed_adjust`; `build_source` takes the adjustment
  and applies it before the timeline and before any seek. `Player::set_speed_adjust`
  records it and **drops the cached scan**, so the next install measures the song again.

### wasm host and the page

`load_module_with_options` gained a fourth argument, `speed_adjust: i32`, clamped to `i8`
on the way in and set on the player before the load — because the load is where the scan
happens. The worklet's `loadModule` message carries `speedAdjust`.

In `index.html` every load option moved inside
`<details id="load-options-disclosure">`, whose `<summary>` carries a one-line summary of
whatever is not at its default (`renderLoadOptionsSummary`), so an option that is on can
never be invisible. The open/closed state is remembered with the rest of the preferences.
The new **Adjust speed** number input (−8…+8) is a load option like the others: it goes
through `loadOptionsFromControls`, `applyLoadOptions`, the availability lock and the
`starplayer.output-and-mixer.v1` record. `applyLoadOptions`'s three messages moved into
one `describeLoadOptionChange` table rather than growing a third ternary each.

It is a **load** option rather than a live command for one reason: the timeline is
measured by the scan that runs during the load, and biasing playback without rescanning
would leave the progress slider describing a song nobody is playing.

## Accuracy

Recorded in [`03-accuracy-policy.md`](../product/03-accuracy-policy.md) §2 under
*Deliberately non-canonical playback options*. Zero is canonical playback; goldens,
conformance traces and offline renders all run at zero and have no way to set anything
else.

## Verification

* `cargo test -p starplayer-engine` — three sequencer cases: the bias applies to requests
  and not to the initial speed, it clamps at 1, and setting it mid-song waits for the
  next request.
* `cargo test -p starplayer-host-wasm` —
  `the_speed_adjustment_biases_what_the_module_asks_for` drives a synthetic MOD asking for
  speed 5 through the real engine at adjustments 0, +1, +3 and −4 and reads the telemetry.
* `cargo xtask wasm && node apps/starplayer-web/test/headless.mjs` — the page loads a
  synthetic MOD with `F05` on every row 0, reads Speed 5, sets the control to +2, and
  reads Speed 7 (deliberately neither 5 nor the MOD's own initial 6), then checks the
  persisted record and the disclosure summary before resetting to 0.

## Owner acceptance

Load a Cubulus rip, open **Rendering options**, and try **Adjust speed** at 1. If 1 is
right the track should sit at the tempo it has on the Amiga. If a different constant fits
better, that is the interesting finding — the control is deliberately a number, not a
checkbox, because the constant is a guess.

## Out of scope

* A per-module memory of the adjustment. It is a session preference like every other load
  option.
* Exposing it on the CLI or in the offline renderer. The engine surface is there
  (`Player::set_speed_adjust`, `scan_song_with_speed_adjust`) if a use appears.
* Detecting the bias automatically. Nothing in a MOD says which replayer it was written
  for, which is exactly why this is a chosen option and not a `FormatDialect`.
