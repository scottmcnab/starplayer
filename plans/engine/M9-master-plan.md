# M9 — Plugin surfaces

| Field | Value |
|---|---|
| Goal | StarPlayer as a CLAP instrument, and as a host for third-party effects |
| Estimate | 2u |
| Depends on | M4, M7 |
| Trigger | **Pull-driven.** Start when the owner wants StarPlayer in a DAW, or when a tracker-workstation use case makes it worthwhile |

## Context

Much of this is already paid for. A plugin host hands you a **pre-sorted, timestamped
event list per block** — which is exactly the shape of `ExternalEventQueue`
(architecture §3.2), built in M4 for live MIDI. The plugin path was deliberately admitted
at the edge so it would not contaminate the tracker path, and this milestone collects on
that.

CLAP first: it is the cleaner API, it is what modern Rust tooling targets well, and a
VST3 wrapper over it is a smaller step than the reverse. Architecture open question **Q5**
asks whether `clack` or `nih-plug` is the better base — settle it here.

## Deliverables

1. **CLAP instrument** — StarPlayer as a playable instrument: load a module, trigger its
   instruments from host MIDI, expose parameters, report latency honestly.
2. **State save/restore**, including the loaded module. Decide whether the module is
   embedded in the plugin state or referenced by path, and say why.
3. **CLAP effect hosting** — third-party effects in the per-channel insert chains and on
   the master bus built in M7.
4. **VST3** via a wrapper, if the owner wants it. Note that VST3 licensing has
   implications for the eventual public release, which is a deferred decision
   (`plans/product/00-vision.md` decision 7) — flag it rather than deciding it here.
5. **Answer Q5** in the architecture document.

## Exit criteria

StarPlayer loads in a CLAP host, plays a module, responds to host transport and MIDI, and
hosts at least one third-party effect on a channel.

## Out of scope

AU. LV2. Being a *host* for instrument plugins (only effects).
