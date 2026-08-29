# A3 — Mobile shells (iOS, Android)

| Field | Value |
|---|---|
| Goal | Mobile applications over the same engine |
| Estimate | open-ended |
| Depends on | M3 |
| Trigger | **Pull-driven**, and the lowest priority of the app milestones |

## Context

The M1 web player is already responsive and works in mobile browsers, which covers most
of what a mobile app would offer. This milestone is only worth pulling if native mobile
gives something the browser genuinely cannot: background audio, lock-screen transport
controls, file-system access, or offline install as a real app.

The engine side is expected to be straightforward — the core is `no_std`, so an iOS or
Android build is a less demanding target than the ESP32 work in M8. The cost is entirely
in the platform shells and their release processes.

## Deliverables when pulled

1. An audio backend per platform (AAudio/Oboe on Android, AVAudioEngine or AudioUnit on
   iOS) behind the M3 host abstraction.
2. A shell per platform, or one cross-platform shell — decide at pull time.
3. Background audio and lock-screen transport integration, since that is the main reason
   to do this at all.
4. Store packaging.

## A note on licensing

Store distribution will force the licence question that
`plans/product/00-vision.md` decision 7 deliberately defers. Settle it before this
milestone, not during it.

## Out of scope

Anything the responsive web player already does adequately.
