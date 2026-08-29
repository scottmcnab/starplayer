# M1-task-B6 — Telemetry v1: the coherent scalar snapshot

| Field | Value |
|---|---|
| Milestone | M1 ([master plan](M1-master-plan.md)) |
| Depends on | B3 (render loop) |
| Blocks | B7 (web player) |
| Parallel with | B4 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests + the web UI rendering it) |

## Context for a fresh agent

Telemetry is a **first-class API**, not a debug hook. It is what the web player, the TUI
homage and any future tracker editor render from. The original understood this: its
`ChannelData` carried `_CMDVal`, `_CMDData`, `_VUBarLevel` and `_ActiveFlag` explicitly
marked "for host program" / "info only"
(`plans/reference/original-s3mlib-analysis.md` §2).

Read `plans/product/01-technical-architecture.md` §9. Telemetry is deliberately split in
two, and **this task delivers only the first half**:

- **(a) Coherent scalar state** — order, row, tick, pattern, speed, BPM, global volume,
  and per-channel note, instrument, volume, pan and effect. About 1 KB. Published through
  a triple buffer or seqlock, because the UI needs it internally consistent: a row number
  from one tick paired with a note from another renders wrong.
- **(b) Lossy audio taps** — scope waveforms and VU peaks. Per-channel rings, `Relaxed`
  write index, tearing acceptable because it is invisible on a scope. **That is M3.**

The one exception: VU peak levels are cheap scalars and the UI wants them in M1, so they
ride in (a) for now and move to (b) at M3 when the scopes arrive. Note that in the file.

## Deliverables

1. **`starplayer-telemetry`** snapshot types, shared by every UI:
   ```rust
   pub struct Snapshot {
       pub transport: TransportState,   // order, pattern, row, tick, speed, bpm, global volume
       pub channels: [ChannelState; MAX_CHANNELS],
       pub warnings: WarningFlags,      // includes the zero-advance guard from B3
       pub voices_active: u16,
   }
   pub struct ChannelState {
       pub note: Option<Note>, pub instrument: u8,
       pub volume: U0F16, pub pan: I1F15,
       pub effect: EffectDisplay,       // raw code + human-readable name
       pub vu_level: U0F16,
       pub active: bool, pub muted: bool,
   }
   ```

2. **The `_MActual*` snapshot discipline.** The original took a snapshot of order, row,
   pattern and tick at the top of each row specifically so the display showed the row
   that was **currently sounding** rather than the one being parsed
   (`plans/reference/original-star-ui.md` §2.1). Reproduce that — it is the difference
   between a display that looks right and one that runs a row ahead.

3. **Publication** through a triple buffer or seqlock in `starplayer-rt`. The audio thread
   must never block on the reader, and the reader must never observe a torn snapshot.
   `no_std`-compatible, using `portable-atomic` where CAS is unavailable.

4. **VU behaviour**, matching the original: peak-hold set to the channel volume on a new
   note or a volume-column write, decayed by a fixed amount per tick, clamped at 0. The
   original decayed `_VUBarLevel` by 2 per tick on a 0–64 scale; scale that to `U0F16`
   and keep the ratio.

5. **`EffectDisplay`** — the raw command code and parameter, plus a human-readable name.
   The name table is in `plans/reference/original-star-ui.md` §2.3. **This is the
   original's single most charming feature** — instead of raw `A06` / `S8F` hex, each
   channel says `change speed`, `vibrato & vol. slide`, `pattern loop`. Both UIs want it.

6. **A `PatternCell` view** so a UI can render rows around the current position without
   understanding any format's native pattern encoding. Read-only, derived from
   `Module::blob` on demand rather than published in the snapshot.

## Research points

1. Triple buffer versus seqlock for a ~1 KB payload in `no_std`. A seqlock needs the
   reader to retry; a triple buffer needs three copies. Measure nothing yet — pick the
   simpler one and note that M3 revisits it when the scope rings arrive.
2. Whether `MAX_CHANNELS` should be a const generic on `Snapshot` or a fixed 64. Fixed is
   simpler and 64 covers every format in scope; const generics leak into every UI
   signature. Recommendation: fixed, documented.

## Verification

- A reader polling from another thread never observes a snapshot mixing state from two
  different ticks — assert with a monotonically-increasing sequence field checked for
  internal consistency across 100,000 reads under contention.
- The published row matches the row that is *sounding*, not the one being parsed: play a
  known module, sample the snapshot mid-row, assert against the expected row.
- VU decay: a note at full volume decays to 0 in the expected number of ticks and clamps
  there.
- `EffectDisplay` renders `A06` as `change speed` and `S82` as `channel pan`.
- The publisher performs no allocation.

## Out of scope

Oscilloscope rings and per-channel audio taps — M3. Any UI — B7.
