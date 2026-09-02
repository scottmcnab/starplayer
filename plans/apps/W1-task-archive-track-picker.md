# W1 — Keep a dropped ZIP's other tracks selectable in the web player

| Field | Value |
|---|---|
| Milestone | Web player maintenance (follows M1-B8 archive support) |
| Status | In progress |
| Depends on | M1-B8 (ZIP archive support, landed) |
| Blocks | — |
| Recommended model | Claude Opus |
| Verified by | agent (headless harness + `cargo test -p starplayer-web`), then owner in a browser |

## Context for a fresh agent

`apps/starplayer-web/www/app.js` is the browser player page. When a file is dropped,
chosen, or fetched, `loadBuffer(buffer, label)` inspects it. If `Loader.is_archive` says
it is a ZIP, the page lists its modules (`Loader.archive_modules`), opens the modal
`#archive-picker` section (`chooseArchiveEntry`) so the user can pick one, extracts that
entry (`Loader.archive_extract(inputBytes, index)`) and plays it. The ZIP bytes are then
forgotten: to play another track from the same ZIP the user has to drop the file again.

The page already has a persistent "Bundled fixture…" `<select id="fixture-picker">` with a
"Load fixture" button in the `.load-actions` row of `index.html`. The owner wants the same
affordance for the most recently loaded ZIP: a dropdown listing the archive's modules,
plus a load button, that stays on the page after the first track is chosen.

Read `apps/starplayer-web/README.md`, `www/app.js` (`loadBuffer`, `chooseArchiveEntry`,
`finishArchivePicker`, `loadFixture` and the listener block at the bottom), `www/index.html`,
`www/style.css`, `src/lib.rs` (the `#[cfg(test)]` module asserts on page copy) and
`test/headless.mjs` (the archive section around the "a single-module ZIP does not open the
picker" assertion) before changing anything.

## Deliverables

1. **Retain the archive.** After a ZIP is inspected and it contains at least one supported
   module, keep `{ label, bytes: Uint8Array, entries }` in `state` (for example
   `state.retainedArchive`). Retain it as soon as the entry list is known, before the modal
   pick, so cancelling the modal still leaves the ZIP available in the dropdown. Loading a
   new ZIP replaces the retained one. Loading a plain module (fixture, URL, single file)
   leaves the retained ZIP in place. A ZIP with no supported modules is not retained and
   the existing error is unchanged.
2. **Persistent track dropdown.** Add to `index.html`, in the `.load-actions` row after the
   fixture controls, a `<select id="archive-tracks">` with an accessible label (for example
   `aria-label="Track from the last ZIP"`) and a `<button id="load-archive-track">Load
   from ZIP</button>`. Both are `hidden` until an archive is retained. Options show
   `name — size` exactly as the modal does (reuse the option-building code; do not
   duplicate `formatByteSize` logic). The placeholder option text should name the archive,
   e.g. `Track from ARCHIVE.ZIP…`, and the currently playing entry should be selected in
   the dropdown after a load so the user can see where they are.
3. **Loading from the dropdown** extracts from the retained bytes and goes through the
   same activation path as the modal pick, producing the same `${name} (from ${label})`
   label and the same messages. Factor the post-choice half of `loadBuffer` (extract →
   `inspect_s3m` → `loadEffectNames` → metadata → activate → play) into a function both
   routes call rather than copying it. Keep the ordering that `src/lib.rs`'s
   `browser_refreshes_format_specific_effect_names_before_rendering_metadata` test asserts
   on (inspect, then `loadEffectNames()`, then `readMetadata`) inside `loadBuffer`'s body
   text, or update that test so it checks the shared function instead.
4. **Modal picker unchanged in behaviour** for the first pick (multi-entry ZIP opens it,
   single-entry ZIP skips it, cancel restores the previous message). Double-click and
   Enter still load.
5. **Mobile layout.** The `.load-actions > *` flex rule at the bottom of `style.css` must
   still lay the new controls out sensibly at narrow widths; hidden controls take no
   space.
6. **Tests.**
   - Extend the archive section of `test/headless.mjs`: after the multi-entry ZIP load,
     assert the dropdown is visible, lists the same entries as the modal did, has the loaded
     entry selected; then select a different entry, click the load button, and assert the
     metadata label changes to that entry without the modal opening. Then load a bundled
     fixture and assert the dropdown is still visible. Then load the single-entry ZIP and
     assert the dropdown now lists only that entry.
   - Extend `src/lib.rs`'s copy tests to assert the new element ids and the button text
     exist in `index.html` (they are the contract the headless test drives).
7. **Docs.** Add a short paragraph to `apps/starplayer-web/README.md`'s ZIP section
   describing the retained-archive dropdown.

## Research points

- `chooseArchiveEntry` resolves a promise; the dropdown path must not touch
  `state.archivePickerResolve`. If the modal is open when the user loads from the
  dropdown, cancel the modal first (`finishArchivePicker(null)`).
- `loadBuffer` bumps `state.moduleRevision` to discard stale activations; the shared
  function must keep that guard.
- `Loader.archive_extract` returns a fresh `Vec<u8>` copy each time, so retaining the
  ZIP bytes is the only memory held. Do not retain the extracted module separately;
  `state.currentModuleBytes` already does that.

## Verification

```
cd apps/starplayer-web
cargo test -p starplayer-web
node test/headless.mjs --mode sab --seconds 5      # skips cleanly without Chromium
```

Build the distribution the way `README.md` says (`cargo xtask web` or the documented
command) so `headless.mjs` exercises the new `app.js`. Report the exact commands run and
their results; if the headless harness skipped, say so.

## Out of scope

- Remembering archives across page reloads (IndexedDB / localStorage).
- Retaining more than one archive at a time.
- Nested ZIPs, non-ZIP archives.
- Changes to `starplayer-archive` or `src/lib.rs` exports.
