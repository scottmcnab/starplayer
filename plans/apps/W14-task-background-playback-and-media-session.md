# W14 — Background playback and media-session controls

| Field | Value |
|---|---|
| Milestone | Web player: mobile listening (follows W13, branch `background-playback`) |
| Status | **Implemented** 2026-10-26 (branch `background-playback`) — owner phone check outstanding |
| Depends on | [W10](complete/W10-task-playback-controller-and-playlist.md), [W13](complete/W13-task-library-remote-source.md) |
| Blocks | — |
| Recommended model | gpt-6.1-sol |
| Verified by | agent (`node --test apps/starplayer-web/test/*.test.mjs` from the repo root, `cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`, `cargo xtask ci`), then the owner on an Android phone (Chrome) against the published GitHub Pages site |

## Context for a fresh agent

Read `CLAUDE.md` first: full variable names, compact formatting, no `cargo fmt`, and do
**not** commit. Then read these:

- **Audio graph:** `apps/starplayer-web/www/app.js`.
  - `startAudio()` builds the `AudioContext` and calls `resume()` synchronously inside
    the user's tap, because iOS Safari's unlock rules require it. It then builds the
    worklet node and connects it with `state.node.connect(state.context.destination)`.
  - `rebuildAudioContext()` makes a new context when the sample rate changes, and
    `rebuildOutputChannels()` makes a new node when the channel count changes. Both
    connect the candidate node to `destination`, then disconnect the old one.
  - The output device: `applyOutputDevice()`, `refreshOutputDevices()`,
    `updateOutputPanel()`, and the saved `sinkId` in `persistPreferences()` and the
    preference loader.
- **Transport:**
  - The Play, Stop, order ◀◀/▶▶ and progress-slider listeners are near the bottom of
    `app.js` (`elements.play`, `elements.stop`, `elements.previous`, `elements.next`,
    `elements.progress`).
  - The playlist's previous and next buttons (`#playlist-previous`, `#playlist-next`) are
    wired in `www/playlist-controls.js`.
- **Playback state:**
  - `www/playback-controller.js`: `play`, `userControl`, `stop`, and the 100 ms `tick()`
    that detects the end of a song and auto-advances.
  - `www/playlist-playback.js`: `next()`, `previous()`, `ended()`.
- **Displayed metadata:** `activateLoadedModule()` sets `state.metadata` (`title`, …) and
  calls `renderMetadata()`. Library rows carry `name`, `title` and `folder`; see
  `playLibraryRow()`. `state.latest` is the latest engine snapshot: `songFrame`,
  `playing`, and so on. The progress slider's maximum is the song length in frames at
  `state.workletSampleRate`.
- **Output panel markup:** `www/index.html`, the `<h2>Output</h2>` panel with
  `#sample-rate`, `#output-device` and the rest.
- **Tests:**
  - `apps/starplayer-web/test/*.test.mjs`, especially `playback-controller.test.mjs` for
    the fake-clock style;
  - `test/headless.mjs`: `run(executable, mode)` for the `sab`, `fallback` and `plain`
    modes, the `Page` helper, and `READ_STATE`.

**Problem.** Rendering happens entirely inside the AudioWorklet (Rust/WASM), and the
worklet node feeds `AudioContext.destination` directly. On Android Chrome the owner hears
playback stop when the screen sleeps. A page whose only sound is Web Audio isn't treated
as a media session: there is no media notification and no lock-screen controls, and the
browser may suspend or freeze it. The standard remedy routes the graph through a
`MediaStreamAudioDestinationNode` into a playing `<audio>` element, and publishes
`navigator.mediaSession` metadata and action handlers. Android then shows a media
notification, with lock-screen controls, and keeps the tab alive as a foreground media
session.

## Deliverables

### 1. Background playback (output routing)

- **New module, `www/background-output.js`.** It is a small, DOM-light module so it can
  be unit-tested with fakes. It owns one hidden `<audio>` element, created once and kept
  across context rebuilds, and the routing decision.
  - **When background playback is on,** for the live context create one
    `MediaStreamAudioDestinationNode`, cached per context and recreated after
    `rebuildAudioContext()`. Set `audio.srcObject = streamDestination.stream`, connect the
    worklet node to the stream destination **instead of** `context.destination` (never
    both, or the audio doubles), and call `audio.play()`.
  - **When it is off,** connect to `context.destination` as today, and pause the element
    and clear its `srcObject`.
  - **A single connect helper.** Expose one function that every connect site uses, for
    example `connectOutput(node, context)`. Replace the three
    `….connect(….destination)` sites in `app.js` with it, so `startAudio`,
    `rebuildAudioContext` and `rebuildOutputChannels` all route correctly.
- **The user-gesture rule.** `audio.play()` must be first called synchronously inside the
  same tap that starts audio, next to the existing `state.context.resume()` in
  `startAudio()`, before any `await`. Otherwise mobile autoplay policy rejects it. Before
  the stream exists, it may play an element with a placeholder `srcObject` (an empty
  `MediaStream`), and get its `srcObject` once the stream destination is ready. A rejected
  `play()` must not break audio start: fall back to direct output and show a short
  message in the Output panel.
- **The option.**
  - **The control.** A **Background playback** checkbox in the Output panel
    (`#background-playback`), with a one-line note under it: "Keeps playing with the
    screen off and adds lock-screen controls; adds a little output latency".
  - **The default.** It defaults **on** for phones and tablets and **off** elsewhere.
    Detect that with `navigator.userAgentData?.mobile`, falling back to
    `matchMedia('(pointer: coarse)').matches`.
  - **Saving and toggling.** Save the choice with the existing preferences in
    `persistPreferences()`. Toggling while audio runs re-routes the live node at once:
    disconnect it, connect it the other way, and play or pause the element. No context
    or node rebuild is needed.
- **The output device.** When background playback is on, `setSinkId` goes to the
  `<audio>` element (`HTMLMediaElement.setSinkId`, if present) instead of the
  `AudioContext`.
  - **The device list.** `updateOutputPanel()` reports the element's `sinkId` in that
    mode.
  - **Switching modes.** Switching modes carries the chosen device across.
  - **No `setSinkId`.** Where the element has no `setSinkId`, keep the existing "system
    default output is used" note.
- **The latency readout.** The Output panel's latency line should still read sensibly.
  The element adds latency the context can't see. Add " + media element" to the latency
  text when routed through it; don't try to measure it.

### 2. Media Session integration

- **New module, `www/media-session.js`.** It takes an injectable `mediaSession` object,
  so tests pass a fake, and does nothing when `navigator.mediaSession` is absent.
- **Metadata,** set on every successful activation (`activateLoadedModule` success path):
  - **`title`** is the module title, or the file label when the title is empty.
  - **`artist`** is the library row's `folder` for library and playlist-library plays,
    or the archive label for archive members. Otherwise use `'StarPlayer'`.
  - **`album`** is `'StarPlayer'`.
  - **`artwork`**, if `www/` already has an icon; otherwise omit it. Don't add binary
    assets for this.
- **Playback state.** Set `playbackState` to `'playing'` or `'paused'` from the
  snapshot's `playing` flag. Update it only when it changes, never every frame.
- **Position.** Call `setPositionState({ duration, position, playbackRate: 1 })`, in
  seconds from frames ÷ `state.workletSampleRate`, when the song length is known and
  finite.
  - **When to call it.** Call it on activation, on seek, on Play and on Stop. Don't call
    it per frame.
  - **When to clear it.** Clear it with `setPositionState()` when the length is unknown.
  - **Errors.** Guard against the `TypeError` thrown when the position exceeds the
    duration.
- **Action handlers.** Each one does exactly what the on-page control does, by calling the
  same function or clicking the same element, so the two can't diverge.
  - `play` → the Play button.
  - `pause` and `stop` → the Stop button. Stop is the player's pause; Play continues from
    the stop position.
  - `nexttrack` / `previoustrack` → the playlist's next and previous track buttons when
    the playlist has entries and those buttons are enabled. Otherwise, set them to
    `null` so the OS hides them.
  - `seekto` → the progress slider's seek (`userSeek(Ring.OPCODE_SEEK_FRAME, frame)`),
    only when the length is known.
  - `seekforward` / `seekbackward` → ±10 s, by the same seek path, clamped. These are
    optional; leave them out if they complicate the code.
- **Refreshing the handlers.** Re-evaluate which handlers are set when the playlist or
  the enabled state changes. Keep the cost small; a cheap check in an existing update
  path is fine.

### 3. Background robustness

The end-of-song detection and auto-advance run on the main thread
(`playback-controller.js`'s 100 ms `setInterval`; the fallback transport flushes commands
from `tick()`). Hidden pages get their timers throttled. Chrome exempts audible pages from
intensive throttling, but not from the 1 s minimum.

- **Check the auto-advance path under throttling.** Confirm by reading the code that the
  auto-advance path (`tick()` → `ended` → `playlistPlayback.ended` → resolve → activate)
  doesn't depend on `requestAnimationFrame`. rAF doesn't run while the page is hidden.
  - **If anything on that path does depend on rAF,** move it off: for example a fallback
    command flush, or a UI step the activation awaits.
  - **What to record.** Note the result under "Research resolution" below.
- **`visibilitychange`.** Add one listener. When the page becomes visible again, call
  `controller.tick()` once and refresh the media-session position.
- **When the page is hidden,** don't start the audio element or resume the context if
  the user had stopped playback.

### 4. Docs

- **Player README.** In `apps/starplayer-web/README.md`, add a short "Phones: background
  playback and lock-screen controls" section. It says what the option does, the latency
  trade-off, that Android battery saver can still stop it, and that iOS Safari support is
  best effort.
- **Plans index.** Update this file's Status and Research resolution, and the
  `plans/README.md` row.

## Research points

1. **Ending the route without dropouts.** Does Chromium keep a context running when
   nothing is connected to `context.destination` but a `MediaStreamAudioDestinationNode`
   is connected? (It does, per spec. Confirm in the headless run that `currentTime`
   advances and the master peak telemetry is non-zero in background mode.)
2. **iOS Safari.** It needs `audio.play()` inside the gesture, and an `<audio>` element
   fed by a `MediaStream` may need `playsinline`. Set `playsinline` and `autoplay` false,
   and document anything you can't verify.
3. **Silence.** Should the element keep playing (silence) while the player is stopped,
   so the media notification stays? Recommended: pause the element on Stop, set
   `playbackState = 'paused'`, and call `play()` again on Play. The lock-screen Play
   action counts as a user activation for media, so `play()` from that handler is
   allowed. Record what you chose.

## Verification

- **`node --test apps/starplayer-web/test/*.test.mjs`**, run from the repo root. New
  `background-output.test.mjs` and `media-session.test.mjs`, using fake context, node,
  element and media-session objects, cover:
  - routing: the node is connected to exactly one of `destination` and the stream
    destination, re-routing on toggle disconnects first, the stream destination is
    recreated for a new context, and a `play()` rejection falls back to direct output;
  - the sink choice goes to the element in background mode and to the context otherwise;
  - metadata from module, library-row and archive inputs;
  - `playbackState` changes only on transitions;
  - `setPositionState` arguments and the unknown-length clear;
  - `nexttrack`/`previoustrack` are set or `null` by playlist state;
  - each action handler invokes the injected transport callback.
- **`cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`**,
  in all three modes. The existing scenarios must pass unchanged with the option at its
  desktop default (off). Add a scenario that turns **Background playback** on before
  starting audio, loads a fixture module and asserts:
  - a hidden `<audio>` element exists, with `srcObject instanceof MediaStream` and
    `paused === false`;
  - the context's `currentTime` advances, and the master peak telemetry is non-zero;
  - `navigator.mediaSession.metadata.title` equals the displayed title, and
    `playbackState === 'playing'`.

  To exercise the handlers, capture them with an init script that wraps
  `navigator.mediaSession.setActionHandler` before the app loads. Add a `Page` helper if
  needed (CDP `Page.addScriptToEvaluateOnNewDocument`). Then:
  - invoke `pause`, and check `playbackState === 'paused'` and that the engine stopped;
  - invoke `play`, and check that it resumes;
  - toggle the option off while playing, and check that audio continues (peak non-zero)
    with the element paused.
- **`cargo xtask ci`** passes.
- **Owner check.** On Android Chrome, against the Pages build:
  - start audio, play a playlist of two or more library tracks, and lock the phone;
  - music continues through a track change;
  - the lock screen shows the title and folder, and play/pause/next/previous work there.

## Out of scope

- Screen Wake Lock (keeping the screen on).
- A service worker for offline audio.
- Any change to the worklet, ring ABI or Rust code.
- Measuring the media element's latency.
- Artwork generation.
- iOS-specific workarounds beyond `playsinline` and gesture ordering.

## Research resolution

- **rAF audit:** `tick()` flushes fallback commands before reading SAB/fallback
  snapshots. Completion calls `playlistPlayback.ended()` → `playEntry()` →
  `controller.play()` → resolver → `preparePlayback()` → `activateLoadedModule()`.
  Activation waits for loader/audio promises and the worklet port's load reply, never
  an animation frame. No step needed moving off rAF. Media Session state/handler
  refreshes use this same 100 ms controller path; visibility restoration ticks once
  and refreshes position without starting audio or resuming a stopped context.
- **1 — Stream destination:** The [Web Audio specification](https://www.w3.org/TR/webaudio-1.1/#MediaStreamAudioDestinationNode)
  defines a stream destination independent of the hardware destination. Chromium's
  headless scenario checks advancing `currentTime` and non-zero master peaks with
  only the stream destination connected, plus a playing hidden element. Every
  graph creation/rebuild uses the same routing helper; streams are cached per
  context and the element survives rebuilds.
- **2 — iOS:** `playsinline` is set and `autoplay` is false. The real stream is created
  synchronously beside `context.resume()`, before the first await, and `audio.play()`
  is called in that gesture. No empty placeholder stream is needed. Rejected play
  requests fall back to direct output with an Output-panel message. This follows
  [WebKit's MediaStream gesture guidance](https://webkit.org/blog/7763/a-closer-look-into-webrtc/).
  Physical iOS Safari gesture policy, screen-lock survival and latency were not
  verified; Chromium automation explicitly permits autoplay.
- **3 — Silence:** Stop pauses the element and snapshot transitions publish
  `playbackState = 'paused'`. Play starts the same element synchronously and resumes
  the context; the engine continues from the stopped position. Natural completion
  pauses the element only outside playlist mode; a playlist advance keeps it playing
  (silence) between tracks, because pausing it would let a phone drop the media session
  and freeze the tab before the next track loads. Changing the route or rebuilding
  channels while stopped preserves that pause. No silent keep-alive is used.
- **Metadata and position:** Library and playlist-library plays carry their folder;
  archive members carry their archive label; other inputs use StarPlayer. There is
  no existing icon under `www/`, so artwork is omitted. Position updates happen on
  activation (cleared until the new generation's snapshot arrives), transport controls,
  seeks and visibility restoration. Optional relative-seek actions are omitted.
- **Outstanding:** Owner Android Chrome check against the published Pages build:
  screen-off continuation through playlist auto-advance and actual notification /
  lock-screen controls. Physical output-device selection and iOS Safari remain
  unverified; routing/device behaviour is covered with fakes.

## Revision after review (2026-10-26): silent keep-alive element instead of a MediaStream route

The first implementation sends the worklet node through a `MediaStreamAudioDestinationNode`
into the hidden `<audio>` element. A review showed, and Chromium's source confirms, that
this can't produce Android lock-screen controls.

**Why the route fails.**
- **Chromium registers a `MediaStream` element as one-shot.** `WebMediaPlayerMS` registers
  every `MediaStream` player as `MediaContentType::kOneShot`
  (`third_party/blink/renderer/modules/mediastream/web_media_player_ms.cc`, "We use
  OneShot focus type here…").
- **A one-shot session isn't controllable.** `MediaSessionImpl::IsControllable()` returns
  false when a session has only one-shot players
  (`content/browser/media/session/media_session_impl.cc`).
- **The result.** No media notification or lock-screen controls appear, whatever
  metadata and handlers the page sets.
- **What a controllable player needs.** Only a *persistent* player takes `kGain` focus
  and is controllable. `media::DurationToMediaContentType`
  (`media/base/media_content_type.cc`) classes as persistent only a duration that is
  unknown (zero) or **longer than 5 s**. A shorter clip is transient
  (`kGainTransientMayDuck`), and not controllable.

**Revised design.** Replace the deliverables in section 1 as follows. Sections 2–4 stand,
except where noted here.

1. **Output is always direct.**
   - **Routing.** The worklet node always connects to `context.destination`, exactly as
     before W14.
   - **Remove the stream route.** Remove `MediaStreamAudioDestinationNode`, the
     per-context stream cache, and the `connectOutput` re-routing.
   - **Output device.** Output-device selection goes back to `AudioContext.setSinkId`
     only. Restore the pre-W14 sink code paths in `app.js`: `persistPreferences`,
     `updateOutputPanel`, `startAudio`, `rebuildAudioContext`, `refreshOutputDevices`,
     `applyOutputDevice`, and the "does not expose AudioContext.setSinkId()" note.
   - **No latency cost.** Drop the " + media element" latency suffix, and the latency
     wording in the option note and the README.
2. **Keep-alive element.** `www/background-output.js` becomes a keep-alive for the
   media session.
   - **The element and its source.** It owns one hidden `<audio>` element (with
     `playsinline` and `loop`). Its source is a **silent PCM WAV generated in JS** as a
     `Blob`, with an object URL created once:
     - 8-bit unsigned mono, so silence is `0x80`;
     - a low sample rate such as 8000 Hz;
     - **10 s** long, safely over Chromium's 5 s persistent threshold.

     It is never a `MediaStream`, and the repo gets no binary asset.
   - **`start()`** calls `audio.play()`. It is first called synchronously inside the
     gesture that starts audio, beside `context.resume()` in `startAudio()`, as now.
   - **`pause()`** pauses the element.
   - **Rejection.** A rejected `play()` reports a short note in the Output panel. Audio
     carries on regardless, because output never depended on the element.
   - **The toggle.** It now only starts or stops the keep-alive element: start it if the
     engine is playing, pause it otherwise. There is nothing to re-route.
   - **Option note.** The Output-panel note becomes "Keeps playing with the screen off
     and adds lock-screen controls".
3. **When the element pauses** (this resolves review finding 2).
   - **Running.** It runs while the engine plays, and **through a playlist advance**,
     including resolving and fetching the next track. Pausing it between tracks would let
     a phone drop the media session and freeze the tab.
   - **When it pauses:**
     - on Stop, or the media-session `pause`/`stop` actions;
     - when a song ends naturally outside playlist mode;
     - when a playlist advance **finishes without starting a track**: there is no
       successor (the last entry without Loop), or the three-failures stop.
   - **Detecting a dead advance.** In app.js, wrap `onTrackEnded` so that, after
     `playlistPlayback.ended(id)` settles, it checks whether a newer activation began
     (for example, by comparing a counter that `activateLoadedModule` bumps). If none
     did, and the engine isn't playing, pause the element and set
     `playbackState = 'paused'`.
4. **Media position freshness** (review findings 3 and 4).
   - **Playback edges.** Refresh `setPositionState` from the snapshot on every
     playing-state transition. That makes Play after a natural end publish the rewound
     position, not the old end.
   - **Rebuilds and reloads.** Set `state.mediaPositionPending = true` wherever
     `playback.rebind(...)` is called: the load-option / speed-adjust reload,
     `rebuildAudioContext` and `rebuildOutputChannels`. The next snapshot of the new
     generation then republishes duration and position.
   - **Frame seeks.** These may keep publishing the requested frame at once, then also
     set the pending flag so the engine's row-aligned frame replaces it.
5. **Tests.**
   - **Unit tests.** Rewrite `test/background-output.test.mjs` for the keep-alive:
     - the WAV header and size, a duration over 5 s, and silent samples;
     - `loop` and `playsinline` set;
     - start and pause;
     - the rejection note;
     - no `MediaStream` or `createMediaStreamDestination` use;
     - and that the module never touches the audio graph.
   - **Headless.** The scenario asserts that:
     - the hidden element plays the blob URL with `duration > 5` and is not paused;
     - output peak telemetry is non-zero;
     - metadata, `pause`/`play` handlers and `playbackState` behave as before;
     - toggling off pauses the element while audio continues;
     - a playlist of one or two short fixtures running to its natural end leaves the
       element paused, with `playbackState === 'paused'`. If the fixtures are too long
       for a test, drive it by seeking near the end.
   - **What only the phone can prove.** The owner's Android check is the only evidence
     for real lock-screen controls and screen-off survival. Say so in the Research
     resolution.
