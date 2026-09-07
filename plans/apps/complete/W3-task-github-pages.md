# W3 — Publish the web player on GitHub Pages

| Field | Value |
|---|---|
| Milestone | Web player maintenance (follows W2) |
| Status | Landed 2026-09-07; owner: enable Pages source (Settings → Pages → GitHub Actions) and check the live site |
| Depends on | Web player as landed by [M1-B7](../engine/complete/M1-task-B7-web-player.md); `cargo xtask wasm` |
| Blocks | — |
| Recommended model | Claude Opus |
| Verified by | agent (headless harness + `cargo test -p starplayer-web`), then owner on the live site |

## Context for a fresh agent

`apps/starplayer-web` is a framework-free page (`www/index.html`, `www/style.css`,
`www/app.js`, `www/ring.js`, worklet files) built by `cargo xtask wasm` into
`apps/starplayer-web/dist/`; read `apps/starplayer-web/README.md` first. The build is in
`xtask/src/main.rs` (`run_wasm`, ~line 1858; `copy_web_sources` ~2099;
`copy_fixture_modules` ~2130). No wasm-pack, npm, Vite or Trunk is involved: `cargo build`
plus the pinned `wasm-bindgen` CLI (`Cargo.toml` `[workspace.dependencies]`, exact pin,
enforced by `check_bindgen_version`) is the whole job. Every asset path in `www/` is
relative, so `dist/` can be hosted under a subpath such as `https://scottmcnab.github.io/starplayer/`.

The goal is a public GitHub Pages deployment that tracks `main`. Three owner decisions
shape it:

1. **Cross-origin isolation.** GitHub Pages cannot set COOP/COEP, so `crossOriginIsolated`
   would be false and `app.js` would silently use its batched `postMessage` fallback
   (`sharedMemoryAvailable()`, ~line 160). The owner wants the SharedArrayBuffer path on
   Pages, so the Pages build loads the `coi-serviceworker` shim (already vendored at
   `www/coi-serviceworker.js`, v0.1.7, MIT). The shim registers a service worker that adds
   the headers and reloads the page once; it self-skips when the page is already isolated.
   **It must only be active in the Pages build.** `plans/product/01-technical-architecture.md`
   ~line 1291 requires both the isolated and non-isolated paths to stay exercised: the
   headless harness's `plain` mode (`test/headless.mjs`) serves without COOP/COEP and must
   keep seeing `crossOriginIsolated === false`.
2. **No fixtures on the public page.** `copy_fixture_modules` ships five S3M files from
   `crates/starplayer-s3m/tests/fixtures/` whose licensing is deferred ("testing only",
   see that directory's README). The Pages build must not package them, and the page must
   cope with their absence instead of showing a picker that 404s.
3. **Deploy only after CI passes on `main`**, plus manual `workflow_dispatch`.

## Deliverables

### 1. `cargo xtask wasm --pages`

Add a `--pages` flag to `run_wasm`'s argument loop. With it:

- `copy_fixture_modules` is skipped entirely (no `dist/modules/` directory).
- `copy_web_sources` post-processes `index.html`: replace the marker line
  `<!-- xtask:coi -->` with `<script src="coi-serviceworker.js"></script>`. If the marker
  is absent, fail the build with a clear message (the same loud-failure stance
  `build_worklet_bundle` takes when the glue shape changes). Without `--pages` the marker
  is copied through untouched.

Without `--pages`, `copy_fixture_modules` additionally writes `dist/modules/index.json`:
a JSON array of the packaged file names (the existing `names` array). Update the usage text
(~line 276) and the `run_wasm` doc comment. Keep the repo's compact Rust style
(`AGENTS.md`/`CLAUDE.md`); no `cargo fmt`.

### 2. Web sources

- `www/index.html`: add the `<!-- xtask:coi -->` marker line in `<head>` **before** the
  stylesheet link (the shim must run before anything else). Remove the five hardcoded
  `<option>` entries from `#fixture-picker`, keeping only the placeholder option.
- `www/app.js`: during initialisation `fetch('modules/index.json')`. On a `2xx` response
  parse the array and append one `<option>` per name to `#fixture-picker`. On any failure
  (404 on Pages, network error, bad JSON) hide both `#fixture-picker` and `#load-fixture`
  (set `hidden = true`, the same mechanism `#archive-tracks` uses) and do not report an
  error — absence is the normal state on Pages. Make this part of the page's existing
  startup sequence so it has completed before the UI is usable: the headless harness sets
  `#fixture-picker.value` then clicks `#load-fixture` (`test/headless.mjs` ~line 380) and
  must not race the population. The fetch at `loadFixture` (~line 1850) is unchanged.
- Leave `www/coi-serviceworker.js` byte-for-byte as vendored (header comment carries the
  version and licence). It is copied to `dist/` by the existing flat copy.

### 3. `.github/workflows/pages.yml`

```yaml
name: Pages
on:
  workflow_run:
    workflows: ["CI"]
    types: [completed]
    branches: ["main"]
  workflow_dispatch:
permissions:
  contents: read
  pages: write
  id-token: write
concurrency:
  group: pages
  cancel-in-progress: true
jobs:
  build:
    if: github.event_name == 'workflow_dispatch' || github.event.workflow_run.conclusion == 'success'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
        with:
          ref: ${{ github.event.workflow_run.head_sha || github.sha }}
      - name: Install pinned toolchain
        run: rustup show
      - name: Install ALSA headers (parity with ci.yml)
        run: sudo apt-get update && sudo apt-get install -y libasound2-dev
      - uses: Swatinem/rust-cache@v2
      - uses: taiki-e/install-action@v2
        with:
          tool: wasm-bindgen@0.2.127   # must match the exact pin in Cargo.toml
      - run: cargo xtask wasm --pages
      - uses: actions/configure-pages@v5
      - uses: actions/upload-pages-artifact@v3
        with:
          path: apps/starplayer-web/dist
  deploy:
    needs: build
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - id: deployment
        uses: actions/deploy-pages@v4
```

Follow `ci.yml`'s step style. Read the `wasm-bindgen` pin from `Cargo.toml` and use that
exact value.

### 4. Docs

- `apps/starplayer-web/README.md`: a **Deploying** section covering `cargo xtask wasm --pages`
  and what it changes (no fixtures, isolation shim, one-off reload on first visit), the
  `Pages` workflow and its CI-gated trigger, the site URL
  `https://scottmcnab.github.io/starplayer/`, and the one-time repository setting
  (Settings → Pages → Source: **GitHub Actions**). Amend the existing fixture paragraph to
  say the menu is populated from `modules/index.json` and hidden when absent.
- `plans/README.md`: add a "Web player on GitHub Pages" row to the apps table pointing at
  this file.

## Research points

- Confirm from `www/coi-serviceworker.js` how it decides whether to register (it checks
  `window.crossOriginIsolated`) and that it registers against its own script URL, so the
  service-worker scope is the Pages subpath.
- Check whether `#fixture-picker`/`#load-fixture` are referenced in `setControlsEnabled()`
  or similar and keep them hidden if so.

## Verification

1. `cargo xtask wasm`: `dist/modules/` holds the five fixtures and `index.json`;
   `grep -c 'xtask:coi' dist/index.html` is 1 and `coi-serviceworker` is not referenced
   from `dist/index.html`. `node apps/starplayer-web/test/headless.mjs` passes in all
   three modes.
2. `cargo xtask wasm --pages`: no `dist/modules/`; `dist/index.html` contains the shim
   script tag and no marker; `dist/coi-serviceworker.js` exists.
3. Serve the `--pages` build without isolation:
   `node apps/starplayer-web/dev-server.mjs --no-isolation --root apps/starplayer-web/dist --port 8091`
   and drive it with headless Chrome (reuse the harness's launch code, or a short script):
   after the shim's reload, `crossOriginIsolated` is true and the fixture picker is hidden.
4. `cargo test -p starplayer-web`, `cargo xtask ci --job wasm-build`,
   `cargo xtask ci --job clippy` green. Rebuild with plain `cargo xtask wasm` at the end so
   `dist/` is left in its development shape.

## Out of scope

- Custom domain; shipping any demo modules publicly; changing the fallback transport or
  the dev server; adding a `pages` mode to the headless harness.
