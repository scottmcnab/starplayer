# A1 — The TUI: a STAR.EXE homage

| Field | Value |
|---|---|
| Goal | The original's screen, in a modern terminal |
| Estimate | 1.5u |
| Depends on | M3 (native host, CLI, telemetry split with scopes) |
| Trigger | **Pull-driven.** The web player (M1) is the primary UI; this is the nostalgic one |
| Reference | [original-star-ui](../reference/original-star-ui.md) — the full archaeology, with every screen decoded |

## Why this exists

Because it would be a shame not to. `plans/reference/original-star-ui.md` reconstructs
the entire 1996 interface from the surviving data segment — the layout, the exact field
positions, the CGA colour scheme, every popup panel, all the hotkeys — and recreating it
in a modern terminal is both a pleasant deliverable and a genuinely good stress test of
the telemetry API.

It is also the second consumer of `starplayer-telemetry`, which is the point at which
that API stops being designed for one UI.

## What to take from the original

Per the reference document's §12:

1. **The layout** — banner, three-row status header, one row per channel (sample name,
   number, volume, VU, pan, effect), bottom help bar.
2. **The 16-cell VU meter**, green → yellow → red, with peak-hold and a fixed decay per
   tick. The original decayed by 2 per tick on a 0–64 scale.
3. **Effects spelled out in English** — `change speed`, `vibrato & vol. slide`,
   `pattern loop` — instead of raw `A06` / `S8F` hex. This is the original's single most
   charming idea and `EffectDisplay` (M1-B6) already carries it.
4. **The `_MActual*` snapshot discipline** — display the row that is *sounding*, not the
   one being parsed. Already handled engine-side in M1-B6; the UI must not undo it.
5. **The panel aesthetic** — double-line outer frames, single-line field boxes, cyan
   panels with yellow headings.
6. **The star logo** on exit.

## What to add, because the engine now supports it

- Per-channel **mute and solo**. The original had none — `_ActiveFlag` was display-only.
- **Oscilloscopes** per channel, from M3's lossy audio taps.
- A real **pattern view**, not a single effect column.

## What to leave behind

- The 64-slot module manager, the DOS shell, CD audio mode, the playlist writer. They
  solved 1996 problems.
- The personal contact details on the exit screen.
- `DeCrunch`, the TheDraw bytecode screen format — *as an implementation*. It is the most
  interesting primitive in the original (§10 of the reference) and worth reading, but a
  modern TUI has better tools.

## Deliverables

1. `apps/starplayer-tui` on `ratatui`, over `starplayer-host-cpal`.
2. The main screen, faithful to the reference document's field positions where a modern
   terminal allows, and honest about where it does not.
3. A colour scheme derived from the original's CGA attributes, with a 256-colour or
   truecolour variant and a plain fallback.
4. File loading, transport, seek, mute/solo, and the key map — the original's where it
   still makes sense, documented where it diverges.
5. Scopes and a pattern view.

## Exit criteria

It looks like StarPlayer, it plays modules, and the owner smiles.

## Out of scope

Everything in "What to leave behind". Terminal-specific hacks for one emulator.
