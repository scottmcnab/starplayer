# W5a — A bounded speed-adjustment chooser

| Field | Value |
|---|---|
| Milestone | Web player maintenance; follow-up to [W5](../W5-task-speed-adjust-and-options-disclosure.md) |
| Status | Implemented and verified 2026-10-27; archived with the main landing |
| Depends on | W5's speed adjustment and load-option reload behavior |
| Recommended model | GPT-6.1-sol |
| Verified by | Existing engine clamp test; `cargo xtask wasm`; packaged browser checks in SAB, fallback and plain modes |

## Context for a fresh agent

The web player's experimental **Adjust speed** field is currently a number input with
an allowed range of -8 through +8. The owner wants a native combobox/dropdown offering
every integer from -6 through +6, with 0 the default. Keep restoring the saved choice:
the owner explicitly selected this behavior over resetting to 0 on every visit.

This is a ticks-per-row bias, not a percentage or BPM change. Negative adjustments make
rows shorter and playback faster; positive adjustments make rows longer and playback
slower. The engine already clamps biased requests to 1..255 and has a regression test
that confirms a negative adjustment cannot stall the row clock. It intentionally biases
speed requests only, leaving the initial/header speed and format-specific handling of
zero commands alone. Preserve those semantics and the existing load/rescan pathway.

Read the repository's `AGENTS.md`. The original assembly directories are read-only.
Do not format unrelated code or run `cargo fmt`. No attribution in commits or PR text.

## Deliverables

1. Replace `#speed-adjust` in `apps/starplayer-web/www/index.html` with a native,
   single-selection `<select>` retaining its ID, label and accessible name. Provide
   exactly 13 numeric values from -6 through 6, show explicit plus signs on positive
   labels, and label the selected default `0 (default)`.
2. Explain in the nearby helper text that negative values play faster, positive values
   play slower, and the minimum is 1 tick per row. Retain the explanation that 0 is
   canonical playback and the control adjusts the module's speed requests.
3. Update the existing control-specific CSS for the select, giving the default label
   and dropdown arrow enough width without causing mobile overflow. Ensure this select
   has the existing disabled appearance while a load is pending.
4. Change the web `SPEED_ADJUST_LIMIT` to 6 and update comments referring to a number
   input/min/max. Retain integer normalization, the storage record/key, choice restore,
   summary, change handler and reload/rollback behavior. Old saved -8/-7 and +7/+8
   values must restore to -6 and +6, respectively; absent or invalid values use 0.
5. Extend the existing speed-adjustment scenario in
   `apps/starplayer-web/test/headless.mjs`: verify the native select, exact values and
   visible positive signs, fresh default, saved choice restore, both legacy endpoint
   clamps, and the existing +2 playback case. Apply -6 to the synthetic speed-5 MOD,
   observe speed 1 and continued playback, verify the negative summary/storage, then
   reset to 0 and observe speed 5. Use observable transport progress rather than a
   fixed sleep to prove playback continues. Reuse the existing harness and setup.
6. Update W5's current-control description/verification with a link to this follow-up;
   update the plans index to identify the follow-up. The parent agent will record final
   verification and archive this task under `plans/apps/complete/` when it is landed.

## Research points

- Confirm the existing preference setup/reload checks near the start of the browser
  scenario; add chooser restore checks before audio starts so no extra audio graph
  lifecycle or fallback-mode setup is needed.
- Confirm current engine behavior in `PatternSequencer::adjusted_speed` and
  `a_speed_adjustment_can_never_stall_the_row_clock`. This should require no engine or
  public API changes.
- Keep the helper concise and consistent with the player's existing rendering options.

## Verification

- `git diff --check` and JS syntax checks for edited scripts.
- `cargo test -p starplayer-engine a_speed_adjustment_can_never_stall_the_row_clock`.
- `cargo xtask wasm` followed by
  `STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --seconds 5`
  (all default modes: SAB, forced fallback and plain). The normal browser scenario also
  verifies the phone viewport does not overflow.
- Inspect the diff for preserved default/canonical playback, unchanged Rust/WASM
  interfaces, and no unrelated changes.

## Out of scope

Playback-rate or percentage controls, BPM changes, biasing the initial speed, per-module
settings, removing persistence, custom combobox widgets, new engine semantics, changes
to the historical assembly, deployment, and unrelated formatting.

## Verification results — 2026-10-27

- Parent review confirmed the native chooser, signed labels, default 0, existing storage
  key and reload/rescan behavior; no Rust, WASM API or historical-source changes.
- `git diff --check`, `node --check apps/starplayer-web/www/app.js` and
  `node --check apps/starplayer-web/test/headless.mjs` passed.
- `cargo test -p starplayer-engine a_speed_adjustment_can_never_stall_the_row_clock`
  passed (the Rust implementation is unchanged).
- `cargo xtask wasm` passed in the isolated task worktree.
- `STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs --seconds 5`
  passed all requested SAB, forced-fallback and plain scenarios in Chrome 156.0.8078.4,
  including the existing library and playlist checks.
- Each mode reported `5 → 7 → 1 (advancing) → 5`; the chooser/default and saved-value
  restore checks passed, and the open rendering-options panel fit at 390 px (`390 <= 390`).
