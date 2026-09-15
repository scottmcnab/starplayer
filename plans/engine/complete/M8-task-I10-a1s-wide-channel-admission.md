# M8 — I10: Admit wide A1S modules with a measured-limit warning

| Field | Value |
|---|---|
| Milestone | M8 |
| Status | Complete; release builds verified; owner listening acceptance pending |
| Recommended model | GPT-5.6-sol, high |
| Depends on | I8d PSRAM module decoding; I9 compact layout and capacity benchmark |

## Context for a fresh agent

The normal A1S web firmware constructs a persistent eight-channel/eight-voice engine and
rejects uploaded or stored modules whose native header declares more than eight channels.
I9 added and verified `EngineLayout::MasterOnly`: it removes per-channel insert buffers,
scope rings and unused routing storage while preserving byte-identical output when channel
insert chains are empty. The A1S product has no channel insert effects. Its engine supports
64 native tracker channels, and the owner wants wider modules admitted rather than refused.

The I9 hardware workload certified 11 unfiltered mixer voices in audio-only operation.
That is a voice result rather than a direct S3M channel benchmark, but the owner explicitly
wants 11 used as the warning threshold for wide traditional modules. Modules above that
threshold must remain playable and clearly warn the user. The owner subsequently chose a
64-voice pool as well as 64-channel admission: this covers traditional S3M and one
foreground voice per IT channel, while additional IT NNA activity uses the engine's
established voice-stealing policy. Every audio path remains fixed point, stereo and 48 kHz.

Follow `AGENTS.md`. Historical source trees are read-only. Work in the assigned sibling
worktree; do not run `cargo fmt`; stage explicit paths; use no attribution.

## Deliverables

1. Change the normal A1S web player to `EngineLayout::MasterOnly`, with capacity for all
   64 native tracker channels and 64 voices. Remove the board-specific
   eight-channel rejection in the PSRAM image adoption path. Retain the generic host
   capacity guard as an invariant against corrupt or unsupported widths.
2. Define one named measured channel-warning threshold of 11. When the current module has
   more than 11 declared channels, expose a persistent warning through the status API and
   render it visibly in the embedded web UI. The warning must include the declared count
   and make clear that playback continues but exceeds the tested limit. Hide it at 11 or
   fewer channels and update it correctly after upload, stored-module selection and module
   replacement.
3. Print the same warning once when a wide module is adopted so headless UART users see it.
   Do not log from the render path.
4. Update A1S documentation and the embedded budget for the 64-channel/64-voice compact
   web engine. State that channel admission and simultaneous-voice capacity are different:
   all native-width files load, while IT NNA activity can still need voice stealing above
   64 concurrent voices.

## Verification

- Unit-test 11-channel and 12-channel warning boundaries in firmware-common/API code.
- Test that the A1S adoption path no longer contains or returns the eight-channel refusal.
- Test status JSON and UI warning presentation for narrow and wide current modules.
- Run relevant firmware-common, embedded-host and web/board host tests.
- Build release A1S `web` and `web,lcd`; inspect the 32 KiB stack gate.
- Run `git diff --check` and review explicit status. Do not commit generated images, logs,
  owner modules or `__pycache__`.

## Out of scope

Increasing the pool beyond 64 voices, claiming that 11 S3M channels were independently
certified, adding channel insert effects, changing the 48 kHz DMA/audio path, or modifying
module format semantics.
