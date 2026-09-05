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

## The task graph (planned 2026-09-05)

Planned at the owner's request while M7 was landing. Task letter **J** (M8 is I). Not
pulled yet: the trigger above still applies, and these files exist so that pulling is a
decision rather than a planning exercise. Decisions taken while planning:

1. **Q5 is answered: `clack`, not `nih-plug`.** Deliverable 3 needs the *host* half of
   CLAP and `nih-plug` has none; `clack` has `clack-plugin` and `clack-host` over one
   `clap-sys`, is `#![forbid(unsafe_code)]`-friendly on the plugin side, and carries no
   licence beyond MIT/Apache. `nih-plug` also brings a framework (parameters, buffers, GUI
   scaffolding) that duplicates `starplayer-host::Player`. J1's first research point
   verifies the crate versions and falls back to a thin `clap-sys` shim confined to one
   crate if `clack` cannot be pinned.
2. **The plugin is a `Player` over a `ClapBackend`.** `starplayer-host::AudioBackend` is a
   push model (the test `ManualBackend` proves it); a CLAP `process()` call is a push. So
   the plugin owns a `Player`, its `process` de-interleaves into the `RenderCallback`, and
   every host surface — load, seek, mixer mode, `midi_only`, `jam`, H7's inserts,
   telemetry — is reused rather than rewritten. Nothing in the engine changes for M9.
3. **Host events are stamped by block offset, not by lead.** A plugin host hands over a
   sorted, sample-accurate event list per block; the plugin stamps each event at
   `block_start_frame + event.time` onto the `ExternalEventQueue`. The live-input lead
   (`EventClock`) exists for keyboards on a jittery thread and is not used here.
4. **State embeds the module bytes.** A path breaks the moment a project folder moves or
   a collaborator opens it; a DAW project is expected to be self-contained. The state
   carries the original file bytes (not the decoded `Module`), an optional origin path as
   a hint, and everything the `Player` exposes (mode, master volume, mutes, mixer mode,
   insert layout). Size is bounded by the module, which the format caps anyway.
5. **Effect hosting is float-first.** A hosted CLAP effect processes `f32`; on the fixed
   path it is wrapped in an `i32 ↔ f32` converting adapter, documented as the one place the
   fixed path stops being bit-exact across targets — an external plugin never was. No
   plugin-delay compensation in M9: the chain reports the summed latency and the master
   plan's "report latency honestly" is met by reporting, not by aligning.
6. **VST3 is a wrapper build, not Rust.** `free-audio/clap-wrapper` turns the CLAP binary
   into a VST3 (and AU) without touching `vst3-sys`, which is GPLv3. The VST3 SDK's own
   dual GPLv3/proprietary licence is flagged for the owner (vision decision 7) and J4 is
   not to be built until the owner says so.

| ID | Task | Depends on | Parallel with | Model |
|---|---|---|---|---|
| J1 | [The CLAP instrument: `starplayer-clap` over a `ClapBackend`](M9-task-J1-clap-instrument.md) | M4 (landed) | J3 | Opus |
| J2 | [State, parameters and the validator](M9-task-J2-state-and-parameters.md) | J1, M7-H7 (insert layout) | — | Sonnet |
| J3 | [Hosting CLAP effects in the insert chains](M9-task-J3-clap-effect-hosting.md) | M7-H7 | J1 | Opus |
| J4 | [VST3 via clap-wrapper — owner-gated](M9-task-J4-vst3-wrapper.md) | J2, owner's licence decision | — | Sonnet |

```
M4 ── J1 ──→ J2 ──→ J4 (gated)
M7-H7 ── J3 (∥ J1)
```
