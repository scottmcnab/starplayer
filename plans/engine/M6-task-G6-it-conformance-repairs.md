# M6 — G6: IT conformance repairs

| Field | Value |
|---|---|
| Milestone | M6 ([master plan](M6-master-plan.md)); see [the concurrency plan](M3-M6-concurrency-plan.md) |
| Status | Ready — dispatch after F5 (one worker at a time) |
| Depends on | G3 and G2 landed (39 of 121 IT cases pass; 82 recorded as `G3-IT-001`..`G3-IT-008` in `conformance/known-failures.md`) |
| Blocks | M6 exit |
| Parallel with | F5 (different crates) |
| Recommended model | Claude Opus (the most demanding accuracy work in the project) |
| Verified by | agent (`cargo xtask conformance --offline`, the IT column; `--strict` naming only what this task explicitly leaves), then reviewer, then the owner listens to dense ITs |

## Context for a fresh agent

StarPlayer is a `no_std + alloc` Rust tracked-music engine. Read `AGENTS.md` first.

Task G3 landed Impulse Tracker playback in `crates/starplayer-it` and wired 121 IT cases
into the conformance harness: 39 pass (13 fully enforced, 26 waiving `position` under
D67), and **82 are executed every run and recorded as known failures**
`G3-IT-001`..`G3-IT-008`, grouped by the first field that diverges so each group is one
piece of work: the volume chain (24 cases), the filter envelope and cutoff (16), the
sounding voice set (12), note and sample selection (10), pitch (8), row flow (8),
panning (1), position beyond D67 (3). Task G2 landed the resonant filter itself. This
task is to G3 what M2-C9 was to B4: settle each group by fixing the processor, by
recording a deviation with an accuracy-policy entry (D75 onward; D70–D74 are G2's), or
by a `QuirkSet` field for a tracker-specific behaviour (C5's rules) — never by hiding a
case.

The references: OpenMPT's `Snd_fx.cpp`/`Sndmix.cpp`/`Sndfile.h` (`kIT*`) and Schism
Tracker's `player/` sources for what IT does; `ITTECH.TXT`; libxmp's dumps as the
oracle, with the D33–D39 and D64–D69 precedents for representation differences. Read
`plans/engine/complete/M6-task-G3-it-playback.md` (all five research resolutions and
the landing notes: `S2x` deliberately absent, `kITCompatGxxCarryPortaWithIns` and
ping-pong loop shortening not implemented), `plans/engine/complete/M6-task-G2-resonant-filter.md`
(D70–D74, the 514× `FilterParams` encoding, `from_it_scaled`), and
`plans/engine/complete/M2-task-C9-s3m-conformance-repairs.md` (the shape of a repair pass).

## Deliverables

1. **Each `G3-IT-*` group settled**, largest lever first: `G3-IT-001` the volume chain's
   last bit (24 cases — expect one rounding rule in the sample × instrument × channel ×
   global × envelope × fadeout product; OpenMPT's `nRealVolume` arithmetic is the
   reference), `G3-IT-002` filter-envelope interpolation and cutoff (16 — includes the
   `from_it_scaled` path and G2's coefficient law; the oracle's `cutoff`/`resonance`
   fields are exact), `G3-IT-003` the sounding voice set (12 — NNA, fadeout end, voice
   retirement; also decide whether `another life.it` legitimately saturates 256 voices or
   voices should retire sooner), `G3-IT-004` note/sample selection (10), `G3-IT-005` pitch
   (8), `G3-IT-006` row flow (8, including any `ItLoopDialect` refinement), `G3-IT-007`
   panning (1), `G3-IT-008` position beyond D67 (3; ping-pong loop shortening).
2. Every case ends passing, waived with a D75+ entry citing both sources by file and
   line, or left as a sharpened known failure naming the evidence that would settle it.
3. `--strict` names only `C2-S3M-009`, whatever F5 leaves for XM, and this task's
   explicit remainder.
4. No change to the MOD/S3M/MTM/XM results or to any golden except the IT one if a fix
   legitimately changes the synthetic fixture's render (regenerate and say why).
5. `TempoModel::ItModern`: G3 made it truncate and selected it by the IT dialects — the
   accuracy policy §2 row says this is the owner's call. Do not revisit it here; note in the
   report how many cases depend on it.
6. Docs: policy §1/§3, `known-failures.md` pruned, `plans/README.md` M6 row, `M6-master-plan.md`
   exit criteria, architecture §5.2 if the stealing rule changes.

## Research points

1. For `G3-IT-001`, diff one failing case's trace against its dump tick by tick and find
   the first differing tick before touching arithmetic; state the rounding rule found.
2. For `G3-IT-003`, count voices per tick on the three `data/m/*.it` modules before and
   after; record peak counts.
3. For each group that becomes a deviation, say which of OpenMPT and Schism agrees with
   StarPlayer and which with libxmp.

## Verification

```sh
cargo test -p starplayer-it
cargo xtask conformance --offline
cargo xtask conformance --offline --strict      # report exactly what it still names
cargo xtask goldens --check
cargo test --workspace
cargo xtask ci --job rt-safety
cargo xtask ci --job clippy
cargo xtask ci --job host-tests
```

Report the before/after IT table, every group's disposition, peak voice counts, and
every policy entry or quirk field added. **Do not commit** — the reviewer commits.

## Out of scope

XM (F5). `.mptm`. MIDI output. Changing G2's filter law unless a case proves it wrong.
