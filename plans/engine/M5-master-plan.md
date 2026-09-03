# M5 — XM support

| Field | Value |
|---|---|
| Goal | FastTracker 2 modules play correctly |
| Estimate | 1.5u |
| Depends on | M4 |
| Blocks | M6 |

## Why here

XM is the first format with **instruments** as distinct from samples: a note→sample map,
volume and panning envelopes, key-off and fadeout, and auto-vibrato. It is what forces
the `Instrument` trait extracted in M4 to be honest, and it is the natural stepping stone
to IT's much larger version of the same ideas.

The reference is the XM specification plus OpenMPT's documented compatibility behaviour —
the original never supported XM (its `S3MLIB.INC` carries commented-out `TYPE_XM`
scaffolding that never landed).

## Deliverables

1. **`starplayer-xm`** loader: the header, pattern data with its packing scheme,
   instruments with their 96-entry note→sample maps, up to 16 samples per instrument,
   delta-encoded 8- and 16-bit sample data.
2. **Envelopes** — volume and panning, with sustain and loop points, advancing on the
   control tick (architecture §5.4).
3. **Key-off and fadeout**, and the volume-envelope release path.
4. **Auto-vibrato** with its sweep.
5. **Linear frequency mode** as well as Amiga periods, selected by the header flag.
6. **The XM effect set**, including the volume column's own effect vocabulary, and the
   well-known FT2 quirks — the ones the OpenMPT wiki documents. Expect these to be the
   bulk of the work, exactly as with S3M.
7. **Conformance**: the libxmp `test-dev/` and OpenMPT `test_*.xm` cases run through the
   M2 harness with no new machinery.

## What has landed

* **F1** — the loader (deliverable 1), archived under `complete/`.
* **F2** — deliverables 2 to 7. `XmProcessor` is FastTracker 2's replayer ported from
  `ft2-clone`'s `src/ft2_replayer.c` function by function: `getNewNote`, `triggerNote`,
  `triggerInstrument`, `keyOff`, `updateVolPanAutoVib`, both effect jump tables and both
  volume-column ones. Linear **and** Amiga periods, both envelopes with sustain and loop,
  key-off and fadeout, auto-vibrato with its sweep, the whole effect set and the whole
  volume column. The facade, the wasm host, the web player and the CLI all accept `.xm`,
  and `goldens/xm/synthetic__i16_mono_44100_linear.sha256` is the format's first golden.

  Conformance: the manifest grew from 47 cases to 140 — every `compare_mixer_data*` call
  in the pinned tree whose module is an `.xm`, which is what `audit_pinned_corpus`
  requires now that `matches_target_extension` covers XM. **72 of the 93 XM cases pass**,
  one is an accepted deviation and twenty are recorded under `F2-XM-001`..`F2-XM-012` in
  `conformance/known-failures.md`. The MOD, S3M and MTM results are unchanged. Six new
  accuracy-policy entries, D42–D47, record where libxmp represents FastTracker 2's
  behaviour differently.

  F2 also folded in the wiring F4 was to have done, because the conformance loop cannot
  run without it: the facade's `xm` arms, `ConformanceFormat::Xm` and `GoldenFormat::Xm`
  all landed here.

## Exit criteria

The XM conformance corpus passes with exclusions justified; several well-known `.xm`
files sound right to the owner.

## Out of scope

NNA — XM has key-off and fadeout but not Impulse Tracker's New Note Actions. That is M6.
