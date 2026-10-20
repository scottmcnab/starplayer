# W10 — Playback controller, playlist UI and automatic advance

| Field | Value |
|---|---|
| Milestone | Web player: music library and playlist (W7–W11, branch `library-playlist`) |
| Status | Specified 2026-10-21; not started |
| Depends on | [W9](W9-task-library-view.md) |
| Blocks | [W11](W11-task-playlist-uploads-and-persistence.md) |
| Recommended model | gpt-6-astra |
| Verified by | agent (`node --test apps/starplayer-web/test/*.test.mjs`, `cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`, `cargo xtask ci`), then the owner in a browser |

## Context for a fresh agent

Read `CLAUDE.md` (do **not** commit), [W7](W7-task-library-index-and-member-lookup.md)
(Interfaces), [W8](W8-task-library-sources-and-resolver.md),
[W9](W9-task-library-view.md), `apps/starplayer-web/README.md` (Controls: the Repeat
semantics and the song-end behaviour), `apps/starplayer-web/www/app.js` in full,
`www/ring.js` (snapshot layout: `playing`, `songFlags`, `moduleGeneration`),
`www/worklet-processor.js` (`loadModule` → `moduleLoaded` with `generation`) and
`crates/starplayer-engine/src/sequencer.rs` around `end_reached` (~line 905).

The owner wants a playlist: "when playing, starplayer can automatically start the next
track once one finishes". W9 left an in-memory `playlist.js` model and a
`playLibraryRow` seam. This task makes playback ownership explicit, builds the playlist
UI and implements automatic advance correctly.

### Facts this design rests on (checked 2026-10-21 at `6a2d987`)

* `activateLoadedModule` (`app.js` ~629) claims `state.moduleRevision` only **after** the
  bytes have arrived, been inspected and audio is ready. A slow fetch for an automatic
  advance could therefore claim a newer revision than a manual load the user started in
  the meantime and replace it.
* The Repeat checkbox is read directly in four places: activation (`queueRepeatCommand`
  ~659), `queueRepeatCommand` itself (~1191), the playback-restore commands for graph
  rebuilds (~1241) and the progress display's fade length (~1986).
* `SONG_FLAG_END_REACHED` is sticky until a seek under a non-repeat at-end policy, so
  `END_REACHED && !playing` is also true after the user presses **Stop** during a looping
  song's 5-second fade, and stays true while a stopped song sits there. It is not a
  completion event by itself.
* Snapshots are read by `refresh()` on `requestAnimationFrame` (~2043), which browsers
  pause in background tabs; the `postMessage`-fallback command flush also rides that
  loop. Auto-advance must not depend on painting.
* The worklet tags each activation with a `generation`, and every snapshot carries
  `moduleGeneration`.

## Deliverables

### 1. `apps/starplayer-web/www/playback-controller.js`

The single owner of what plays, per W7's interface. Inject its dependencies (resolver,
`activate(buffer, label, origin)` callback into `app.js`, a clock/timer, a snapshot
reader) so it is unit-testable in Node without a DOM.

* `play(locator, {origin, entryId})`: increments an **intent token before any I/O**;
  after every await (resolve, inspect, audio ready, activation) it checks the token and
  quietly abandons if superseded. Activation's returned `generation` is recorded as the
  current `{graph, generation, entryId, origin}`.
* Every other load route — file picker / drop, URL, bundled fixture, archive modal and
  retained-archive dropdown, library double-click — goes through `play` too (bytes
  already in hand are a locator of kind `bytes` internal to the controller). Any origin
  other than `playlist` **leaves playlist mode**.
* `effectiveAtEnd()`: in playlist mode songs end (the existing non-repeat policy: a
  looping song fades over `SONG_FADE_SECONDS` then stops; an ending song stops); outside
  it, the user's Repeat preference. Replace **all four** direct reads of
  `elements.repeat.checked` with it. In playlist mode the Repeat checkbox is disabled with
  a title explaining why, and **keeps** its stored value and `localStorage` preference.
* **Natural completion.** Track per current module: whether a user Stop or a seek
  (progress slider, order seek, previous/next order buttons) has been issued since the
  last Play. A completion is the edge from a snapshot with `playing` to one with
  `!playing && END_REACHED`, whose `moduleGeneration` and graph match the current
  activation, with no user Stop/seek recorded. Emit `onTrackEnded(entryId)` exactly once
  per activation.
* **Control tick.** Run completion detection from a `setInterval` (~100 ms) that also
  flushes the fallback command batch, independent of `requestAnimationFrame`, so a
  background tab advances. Keep the rAF loop for drawing. (Chrome does not throttle
  timers in audible tabs; note in the README that a silent background tab may be
  throttled to 1 Hz, which only delays the advance.)

### 2. Playlist UI (`playlist.js` view on W9's model)

A **Playlist** panel: ordered entries showing name, title, format, folder/source and a
state marker (current ▶, failed ⚠, unavailable); remove (× button, Delete key), reorder
(drag and drop, and Alt+↑/↓ for keyboard users), **Clear**, **Loop playlist** checkbox,
**Previous track** / **Next track** buttons (distinct from the existing order-seek
◀/▶ buttons; label them clearly). Double-click or Enter on an entry plays it in playlist
mode. Removing the current entry stops nothing but invalidates any pending advance.

### 3. Automatic advance

* On `onTrackEnded`: play the next entry with origin `playlist`; at the end, wrap if
  **Loop playlist** is on, else stop and leave the last entry current.
* When a playlist track starts, `resolver.prefetch(next.locator)` — cache warming only;
  prefetch never claims the intent token.
* A failed entry (resolve or activation error) is marked ⚠ with the message as its
  tooltip and skipped after the error is shown; three consecutive **real** failures stop
  advancing with a message. An abandoned (superseded or aborted) load is not a failure.

### 4. Tests

* `test/playback-controller.test.mjs` (`node --test`) with fake timers, a fake resolver
  whose promises the test resolves out of order, and scripted snapshots:
  * a slow auto-advance resolve that finishes after a manual play is discarded;
  * natural completion of an ending song and of a looping song (after its fade) emits
    once; a repeated stopped snapshot does not emit again;
  * Stop during a fade, a seek after the end, and a module replacement do not emit;
  * a stale snapshot carrying the previous `moduleGeneration` does not emit;
  * `effectiveAtEnd()` in and out of playlist mode; Repeat preference preserved;
  * three consecutive failures stop; an aborted load does not count.
* `test/headless.mjs` library scenario extended: add two tracks to the playlist, tick
  Repeat, play the first from the playlist, seek near its end, and assert the second
  starts (its title appears) and the first entry is no longer current; press Stop during
  the next track's final fade and assert nothing advances. Also rebuild the sample rate
  during playlist playback and assert the at-end behaviour survives (the restore commands
  use `effectiveAtEnd()`).

## Research points

1. Confirm by test what `songFlags`/`playing` look like across a looping song's fade and
   stop, and across the host's rewind to frame 0 after a stop (README "Controls"), so the
   edge detector does not fire on the rewind. Record the observed sequence here.

## Verification

```text
node --test apps/starplayer-web/test/*.test.mjs
cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs
cargo xtask ci
```

Owner: queue a music disk from the Hornet mirror, play it through with the tab in the
background, try Stop during a fade, reorder and remove entries while it plays.

## Out of scope

Uploads, persistence, CSV import/export and library rebinding (W11); gapless or crossfade
transitions.
