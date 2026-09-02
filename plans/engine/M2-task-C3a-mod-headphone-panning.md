# M2-task-C3a — Headphone-friendly MOD panning in the web player

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Depends on | C3 (native MOD), C4 (three-format web host) |
| Blocks | — |
| Parallel with | C5, C7 |
| Recommended model | GPT-5.6-sol |
| Verified by | coordinator (Rust, WASM packaging, headless browser) |

## Context for a fresh agent

Native MOD loading already exposes `starplayer_mod::LoadOptions` and
`StereoSeparation`. Authentic MOD playback defaults to the Amiga L-R-R-L map at hard
left/right, but this is tiring on headphones. S3M's ordinary stereo defaults use pan
nibbles 3 and 12, which are exactly symmetric at -3/5 and +3/5. The web player needs a
binary preference that applies that 60% magnitude to MOD's existing L-R-R-L assignment.

The page validates and displays a module through its page-side WASM instance, then sends
the retained bytes to a separate AudioWorklet WASM host. The option must reach every
audio-side module activation, including graph rebuilds, without changing S3M or MTM.
Module decoding and replacement remain outside `process()`; retired `Arc<Module>` values
must continue to return through the existing garbage path.

The owner chose immediate application: changing the checkbox while a MOD is active
reloads it at the currently sounding order. As with existing graph rebuilds, the order
restarts at row zero. The playing/stopped state, master volume, and channel mute flags
must survive.

## Deliverables

1. Add a persistent, initially unchecked checkbox near the module-loading controls,
   labelled **Headphone-friendly MOD panning**, with concise text explaining that it uses
   S3M-style 60% spacing. Extend the existing v1 preference object compatibly; an older
   stored object without the field means unchecked.
2. Thread a boolean option through the page-to-worklet load message and an option-aware
   WASM loading export. Retain the existing load export as the authentic/default wrapper.
   In the host, only `ModuleFormat::Mod` uses `starplayer_mod::load_with_options` with
   60% or 100% separation; S3M and MTM keep their native loading paths unchanged.
3. Expose the inspected module's format (or an equivalent `is_mod` query) to page JS so
   the checkbox reloads only an active MOD. New direct loads, MODs extracted from ZIPs,
   sample-rate rebuilds, and output-channel rebuilds all use the saved choice.
4. When toggled during MOD playback, keep the old module playing until replacement is
   ready, disable the checkbox during the reload, restore the sounding order and prior
   playing/stopped state, schedule off-thread garbage collection, and report success.
   On failure, retain playback, restore and persist the previous checkbox value, re-enable
   the control, and show a readable error. With no module or an S3M/MTM active, merely save
   the preference for the next MOD load.
5. MOD `8xx`/`E8x` panning effects remain native and may override the initial positions.
   Do not change loader defaults, the canonical MOD conformance path, S3M panning, or MTM
   panning.

## Research points

1. Reuse the existing retained byte buffer and activation request/response path rather
   than inventing a render-thread command or mutating a loaded `Module`.
2. The page-side loaded module is display metadata only; decide whether it needs the
   panning option or only an `is_mod` export. Avoid a public facade API unless two real
   non-web consumers require it.
3. Ensure rapid toggle/load interactions cannot leave the checkbox disabled or persist a
   value different from the module that is actually active.

## Verification

- Unit-test that 60% MOD separation produces the L-R-R-L signs at exactly +/-3/5, while
  the default remains hard panned.
- Host-WASM tests load a synthetic MOD at both settings and inspect its retained header;
  S3M and MTM still load through their native paths.
- Page tests cover format identification, backward-compatible preference restoration,
  and the option's presence and wording.
- The packaged headless player loads a synthetic MOD, observes hard pan, checks the
  option and observes S3M-style spacing, confirms transport/order survival, then unchecks
  and observes hard pan again. Cover the existing SAB, fallback, and plain modes.
- Run `cargo test -p starplayer-mod -p starplayer-host-wasm -p starplayer-web`, WASM target
  checks for both web crates, `cargo xtask wasm`, and the headless harness. Do not run
  `cargo fmt`.

## Out of scope

- A continuous stereo-width slider.
- Changing S3M or MTM panning, or changing the MOD L-R-R-L side assignment.
- Preserving an exact row/tick across the reload; the sounding order is the restore unit.
- Adding support for tagless 15-sample MOD files.
