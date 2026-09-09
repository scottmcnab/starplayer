# W4 — Sample-enhancement checkboxes in the web player

| Field | Value |
|---|---|
| Milestone | Web player maintenance (follows W3); the web half of M10-[K5](../engine/M10-task-K5-sample-enhancement.md) |
| Status | Planned 2026-09-09 |
| Depends on | [K5a](../engine/M10-task-K5a-enhancer-core.md) landed (`starplayer::enhance`, `CATALOGUE`, `Module::enhanced`) |
| Blocks | — |
| Parallel with | [K5b](../engine/M10-task-K5b-enhance-offline-and-cli.md) |
| Recommended model | Claude Sonnet (one wasm export, one JSON descriptor, and generalising an existing checkbox's reload dance) |
| Verified by | agent (headless harness + `cargo test -p starplayer-host-wasm -p starplayer-web`), then owner in a browser on an 8-bit MOD |

## Context for a fresh agent

Read `CLAUDE.md`, `apps/starplayer-web/README.md`, and [K5a](../engine/M10-task-K5a-enhancer-core.md).
The owner wants the K5 sample enhancers **exposed as checkboxes in the web player so they
take effect when a track is loaded**. Enhancement is a load-time module rebuild
(`Module::enhanced`), so in the web player it belongs exactly where the module is decoded:
the worklet's `loadModule` message task in `crates/starplayer-host-wasm/src/lib.rs`
(`decode`, `:175`; `Host::load_module_with_options`, `:284`; exports `:596-607`). That
task already runs off `process()`; a load-time dropout is accepted as today's decode is.

The one existing load option — **Headphone-friendly MOD panning** — is the precedent to
generalise, not to copy: `index.html:59-62`, `style.css:65-69`, and in `app.js` the reads
at `:475`, `:647`, `:1247`, `state.activeModHeadphonePanning` (`:99`, `:480`, `:1100`,
`:1164`, `:1208`), `updateModPanningAvailability` (`:657-662`), `applyModPanningPreference`
(`:1067-1122`: re-entrancy guard, persist, MOD-only early-out, claim `++state.moduleRevision`,
`playbackRestoreCommands()`, activate, stale check, success/failure/finally), the
`localStorage` record `starplayer.output-and-mixer.v1` (`restorePreferences` `:198`,
`persistPreferences` `:215`), the worklet's `loadModule` handler
(`www/worklet-processor.js:115-137`), and the headless test block (`test/headless.mjs:429-533`,
`READ_STATE` `:356`). The Effects panel's rule applies (`app.js:1232-1247`): nothing about
a specific enhancer is transcribed in JS; the page builds its controls from a
wasm-exported descriptor (`effects_json`, `lib.rs:724-749`).

## Deliverables

### 1. wasm host

- `load_module_with_options(bytes: &[u8], headphone_friendly_mod_panning: bool, enhancement_flags: u32) -> Result<u32, JsValue>`;
  `load_module` stays the no-options alias. `decode()` applies
  `starplayer::enhance::from_flags(flags, Some(2 × negotiated rate))` — the negotiated
  rate is `host.player.spec().sample_rate_hz` (`:1045`), store it on `Host` — via
  `Module::enhanced` before `Player::load_module`.
- **Frame budget**: the IT loader's `decoded_pcm_budget` is load-time only; a rebuild
  bypasses it. If `module.pcm().len() × factor` would exceed 16 M frames, drop 4× to 2×,
  then to identity, and report the applied flags in the `moduleLoaded` reply so the page
  can say so. `HEAP_RESERVE_BYTES` (`:75`) is for the host and stays.
- `enhancements_json() -> String`: `[{ "bit": 0, "id": "sinc4x", "label": …, "description": … }, …]`
  from `CATALOGUE` (entries with a flag bit only).
- `starplayer-host-wasm/Cargo.toml`: facade feature `enhance`. Unit test beside
  `mod_headphone_option_changes_only_mod_initial_panning` (`:1011`): flags 0 equals the
  plain load; flag `sinc4x` on REFLEX.S3M quadruples every sample's `length_frames` and
  sets `rate_scale_log2 == 2`.

### 2. Page

- The worklet's first `ready` message carries `enhancements: this.wasm.enhancements_json()`
  beside `effects`; `loadModule` forwards `enhancementFlags` (default 0) to the export.
- `index.html`: a load-options group in the load panel holding the existing panning
  checkbox and the enhancement checkboxes the page builds from the descriptor (same
  `.mod-panning-option` styling, label + small description). Default off.
- `app.js`: a load-options record `{ headphoneFriendlyModPanning, enhancementFlags }`
  replaces `state.activeModHeadphonePanning` at every read (`:475`, `:647`, `:1150`, `:1201`);
  `applyModPanningPreference` becomes `applyLoadOptions(changedKey)` whose MOD-only
  early-out applies only when the panning flag changed; `updateModPanningAvailability`
  disables every load-option input during a reload; the reload dance (revision claim,
  transport/order/mute restore, revert-on-failure) is kept as is. Persist `enhancementFlags`
  in the same record, **keeping the existing `headphoneFriendlyModPanning` key** so the
  harness's v1-record test (`headless.mjs:429-433`, `:476`) stays valid. When the host
  reports a reduced factor, show it in the status line.
- `apps/starplayer-web/src/lib.rs` (page-side metadata instance) stays unenhanced; relabel
  its instrument length column "source frames" (`instrument_sample_length`, `:253`).
- README: a "Load options" section documenting all three checkboxes, the budget fallback,
  and that the metadata panel shows source lengths.

### 3. Tests

`test/headless.mjs`: extend the panning block into a load-options block — each enhancement
checkbox toggles on and off; inputs are disabled during the reload; the memory line
changes across the reload and is stable during playback; the choice persists in
`localStorage` and restores; the race against a concurrent load still resolves to the
newer request. `READ_STATE` gains `enhancementFlags`. `cargo test -p starplayer-host-wasm -p starplayer-web`.

## Research points

1. Load time: measure the worklet-thread time of a `sinc4x` rebuild of the largest fixture
   (and of a ~2 MB IT if one is at hand) and record it; if it is over ~1 s, note the option
   of moving the rebuild to a Worker that transfers the rebuilt module as bytes (needs a
   module serialisation — out of scope).
2. Whether the checkboxes should also be offered per track rather than as a global
   preference; keep global (the owner asked for checkboxes that apply at load).

## Verification

```
cargo test -p starplayer-host-wasm -p starplayer-web
cargo xtask ci --job wasm-build
cargo xtask ci --job clippy
cargo xtask wasm
node apps/starplayer-web/test/headless.mjs
node apps/starplayer-web/test/worklet-harness.mjs
```

## Out of scope

CLI/offline (K5b); serialising a rebuilt module across threads; enhancing in the
page-side metadata instance; a per-track (non-persistent) setting.
