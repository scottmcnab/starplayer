# A2 — Desktop shells (Windows, macOS)

| Field | Value |
|---|---|
| Goal | Native desktop applications over the same engine |
| Estimate | open-ended |
| Depends on | M3 (cpal host) |
| Trigger | **Pull-driven.** Only when the owner or a user actually wants a desktop app rather than the browser or the terminal |

## Context

cpal already gives WASAPI and CoreAudio, so the *audio* half is expected to be free once
M3 lands. What is not free is the UI shell, and that is the whole cost of this milestone.

Before starting, ask whether it is needed at all. The web player (M1) runs on every
desktop; the TUI (A1) runs in every terminal. A native shell earns its place only if it
does something neither can — file-system integration, OS media keys, a plugin-host-like
workflow, or simply being a real installable app.

## Deliverables when pulled

1. A UI toolkit decision, made once and recorded — the candidates change fast enough that
   researching them at pull time beats choosing now.
2. A shell over the facade crate, reusing `starplayer-telemetry` exactly as the web and
   TUI players do. **No engine logic in the app.**
3. Packaging and signing for each platform.
4. Verification that cpal's Windows and macOS backends behave as expected under real
   load, which M3 does not test.

## Out of scope

Reimplementing anything the engine already does. If the shell needs an engine change,
that is an engine milestone, not this one.
