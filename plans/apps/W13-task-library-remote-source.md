# W13 — Stream library tracks from `source_url`, through a CORS proxy when needed

| Field | Value |
|---|---|
| Milestone | Web player: music library (follows W7–W12, branch `library-remote`) |
| Status | Specified 2026-10-23 |
| Depends on | [W8](complete/W8-task-library-sources-and-resolver.md), [W11](complete/W11-task-playlist-uploads-and-persistence.md), [W12](complete/W12-task-library-comment-search.md) |
| Blocks | — |
| Recommended model | gpt-6.1-sol |
| Verified by | agent (`node --test apps/starplayer-web/test/*.test.mjs` from the repo root, `cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`, `cargo xtask ci`), then the owner on the published GitHub Pages site with the deployed Worker |

## Context for a fresh agent

Read `CLAUDE.md` first: full variable names, compact formatting, no `cargo fmt`, and do
**not** commit. Then read these:

- **The CSV contract:** `apps/starplayer-web/LIBRARY-CSV.md`.
- **Sources:**
  - `apps/starplayer-web/www/library-source.js`: `createHttpSource`, the directory and
    `FileList` sources, `discoverIndex`, and `storedDirectoryHandle`, which is IndexedDB
    `starplayer-library` v1, store `handles`.
  - `apps/starplayer-web/www/library-controls.js`: how a source and an index CSV are chosen
    and loaded, including `pendingIndex` from **Open index CSV…** and the revision counters.
- **Data and loading:**
  - `apps/starplayer-web/www/library-index.js`: `readRows`, `parseLibraryCsv`,
    `libraryIdentity`.
  - `apps/starplayer-web/www/track-resolver.js`: `source.size` then `source.readFile`; a
    `null` size skips the pre-check, and the post-read cap already exists.
- **Tests:**
  - `test/library-source.test.mjs`, `test/library-controls.test.mjs`,
    `test/dev-server-library.test.mjs`;
  - in `test/headless.mjs`: `runLibrary`, `startDevServer`, the `DataTransfer` upload
    pattern around `playlist-files`, and how scenarios assert on displayed text.
- **The server:** `apps/starplayer-web/dev-server.mjs`, especially `confinedPath`, its
  segment-wise decoding and path rules.

The web library plays tracks from a local mirror today, through the folder picker or
`cargo xtask serve --library`. It is also published on GitHub Pages; `.github/workflows/pages.yml`
builds `cargo xtask wasm --pages`, and the page runs `www/coi-serviceworker.js` to become
cross-origin isolated. The owner wants the published player to open
`hornet-modules.csv` (from ModDNA) and play each track by fetching it over the network.

Facts the coordinator checked:

- **CSV.** Every row of `hornet-modules.csv` (9,585 rows, 8,511 distinct files) has
  `source_url` = `https://hornet.org/music/` + `source_path`.
- **hornet.org.** It serves the files over HTTPS with `Content-Length`, but sends **no**
  `Access-Control-Allow-Origin` or `Cross-Origin-Resource-Policy` header, and answers a
  preflight `OPTIONS` with 405.
- **Consequence.** A page on another origin cannot read those bytes. The player is also
  cross-origin isolated (COEP `require-corp`, natively or via the shim), and a cross-origin
  `fetch` then has to pass CORS.
- **Pages shim.** It passes CORS-mode requests through.

The owner's decision is a small **Cloudflare Worker** proxy, kept in this repository and
deployed by the owner. The player must also work **without** a proxy against any server
that does send CORS headers ("direct mode"), so it keeps working unchanged if hornet.org ever
adds CORS.

Rules:

- Never write into `/home/scott/projects/moddna` or `/mnt/d/hornet-music`, and never copy
  ModDNA CSV rows into the repository. Fixtures are synthetic.
- Never use `ARMANI.S3M` as a fixture; REFLEX and PETRI are fine.
- Tests make **no requests to the internet**. Every network peer is a local test server.
- Do not deploy anything, and do not add npm dependencies; the Worker is plain JavaScript.
- The library identity stays the SHA-256 of the index CSV bytes plus its name. Saved
  playlists depend on it.

## Deliverables

### 1. The Worker: `apps/starplayer-web/remote-proxy/`

- **`worker.js`.** An ES module: `export default { async fetch(request, env) { … } }`.
  Export the pure helpers too, for tests. It must import and run under Node 24, which has
  `Request`, `Response` and `fetch`, so it can't depend on `caches` or other Workers-only
  globals.
  - **Configuration.** `env.UPSTREAM` is the upstream base and must end in `/`; it defaults
    to `https://hornet.org/music/`. `env.ALLOWED_ORIGINS` is a comma-separated list,
    defaulting to `https://scottmcnab.github.io,http://localhost:8080,http://127.0.0.1:8080`.
  - **Path mapping.** `https://<worker>/<p1>/<p2>/…` fetches `UPSTREAM + p1/p2/…`.
    - **Validation.** Each segment is decoded on its own. Malformed percent-encoding, empty
      segments, `.`, `..`, `\` or NUL in a decoded segment, and an empty path all return
      **400**.
    - **Rebuilding.** The upstream URL is rebuilt with `encodeURIComponent` per segment, and
      the query string is ignored.
    - **No other host.** It never fetches anything that isn't under `UPSTREAM`, and it is
      not an open proxy.
  - **Methods.**
    - **GET and HEAD** are proxied; HEAD returns the headers without a body.
    - **OPTIONS** from an allowed origin returns 204 with `Access-Control-Allow-Origin`,
      `Access-Control-Allow-Methods: GET, HEAD, OPTIONS`, `Access-Control-Max-Age: 86400`,
      and `Vary: Origin`.
    - **Anything else** returns **405**.
  - **Origins.**
    - **Allowed `Origin`:** the response carries `Access-Control-Allow-Origin: <origin>`,
      `Vary: Origin`, and `Access-Control-Expose-Headers: Content-Length`.
    - **Any other `Origin`:** **403**, with no upstream fetch.
    - **No `Origin`** (for example curl): served without CORS headers.
  - **Response headers.**
    - **Always:** `Cross-Origin-Resource-Policy: cross-origin`.
    - **Passed through from upstream when present:** `Content-Type`, `Content-Length`,
      `ETag`, `Last-Modified`.
    - **On a 200:** `Cache-Control: public, max-age=31536000, immutable`.
  - **Upstream fetch.**
    - **Edge caching:** `fetch(upstreamUrl, { method, headers: { 'User-Agent':
      'StarPlayer-proxy (+https://github.com/scottmcnab/starplayer)' }, cf: { cacheEverything:
      true, cacheTtl: 2592000 } })`. Node ignores the `cf` option.
    - **Errors:** a non-2xx upstream status passes through with a short text body.
    - **Size cap:** an upstream `Content-Length` over 128 MiB returns **413**, and the body
      is not streamed.
- **`wrangler.toml`.** Worker name `starplayer-hornet-proxy`, `main = "worker.js"`, a recent
  `compatibility_date`, and `[vars]` holding `UPSTREAM` and `ALLOWED_ORIGINS` with the
  defaults above.
- **`README.md`.**
  - **What it is.** One upstream per Worker, origin-allowlisted, edge-cached, and not an
    open proxy.
  - **Deploying.** `npx wrangler login`, then `npx wrangler deploy` from this directory,
    which prints the `https://starplayer-hornet-proxy.<subdomain>.workers.dev` URL.
  - **Changing it.** How to edit `ALLOWED_ORIGINS` and `UPSTREAM`.
  - **Checking it.** A `curl` example.
  - **Cost.** The free tier allows 100k requests a day, and Hornet files are cached at the
    edge.
  - **Wiring it up.** Put the URL into `www/remote-sources.js`.

### 2. Player: a remote source

- **`library-index.js`.** `readRows` keeps `row.sourceUrl`, the trimmed `source_url`, or
  `''`.
- **`library-source.js`.**
  - **Generalised HTTP source.** `createHttpSource({ base = '/library/', kind = 'http',
    label = 'server library', indexPath = '/library-index.csv', fetch })` builds
    `base + segments.map(encodeURIComponent).join('/')`. The current behaviour and tests
    stay the same.
  - **`createRemoteSource({ proxy = '', fetch })`.** Returns a source of kind `remote`, built
    on the HTTP source, with no `indexPath`. Its `attach(library)` must be called before any
    read.
    - **Deriving the base.** `attach` derives the upstream `base` from the rows: the prefix
      `P` for which the most rows satisfy `row.sourceUrl === P + row.sourcePath`, or
      `P + per-segment-encoded sourcePath`.
    - **Counts.** It returns `{ base, streamable, unmatched }`.
    - **No usable URLs.** With no `source_url` values, or no consistent base, it throws an
      `Error` that says so.
    - **Reads.** Reads fetch `(proxy || base) + encodedPath`. Reading a row that doesn't
      follow `base` throws `<path>: source_url does not follow <base>`. The source therefore
      keeps the set of matching `sourcePath`s.
    - **Size.** `size()` returns `null`, so no HEAD request is made per track. The resolver's
      post-read cap still applies.
    - **Label.** `label` names the host, plus the proxy host when one is set, for example
      `hornet.org via starplayer-hornet-proxy.example.workers.dev`.
  - **Network errors.** A `fetch` that rejects with a `TypeError` (a CORS or network failure)
    becomes `Error("<path>: could not fetch from <host>; it may not allow cross-origin
    requests — set a Proxy URL")`. `AbortError` passes through unchanged.
- **`www/remote-sources.js` (new).** `export const REMOTE_SOURCES = [{ upstream:
  'https://hornet.org/music/', proxy: '' }]` and `defaultProxyFor(base)`. The coordinator
  fills in the deployed Worker URL later. Leave `proxy: ''` and a comment pointing to
  `remote-proxy/README.md`.
- **`library-controls.js` and `index.html`.**
  - **Stream button.** When an index CSV is pending, whether chosen with **Open index CSV…**
    or restored, show a **Stream from source URLs** button next to the existing prompt.
    - **When it appears.** Only when the pending CSV's header has a `source_url` column.
      Sniff the header with `parseCsv` on the first line; don't parse the whole CSV twice.
    - **What it does.** Clicking it goes through `selectSource` with a remote source. The
      existing revision and staleness rules apply.
  - **Proxy URL field.** Show `<input id="library-proxy" type="url">`, labelled **Proxy URL
    (blank: fetch directly)**, while streaming is offered or active.
    - **Prefill.** It is prefilled from `localStorage` (key `starplayer.libraryProxy`, and
      every access is wrapped in `try`), otherwise from `defaultProxyFor(base)` once the
      base is known.
    - **Format.** The value must be an absolute `http(s)` URL; a trailing `/` is added if
      missing.
    - **Changes.** Editing it while streaming updates the source's proxy for later reads,
      and clears the resolver cache (`state.library.resolver.clear()`).
  - **Loading.** `loadIndex` calls `source.attach(library)` for remote sources before
    building the resolver. The summary adds, for example, "streaming 9,585 tracks from
    hornet.org via <proxy host>". When some rows can't be streamed, it adds "; 12 rows have
    no matching source_url".
  - **Comments.** For the remote source, comments come only from **Open comments CSV…** or
    the stored copy (below); automatic discovery is skipped.
- **Reopen after reload.**
  - **Storage.**
    - **Upgrade.** IndexedDB `starplayer-library` goes to **v2**. `onupgradeneeded` creates
      `handles` if missing and adds an `indexes` store, so v1 data survives.
    - **What is saved.** When a remote library loads, save `{ kind: 'remote', name, bytes,
      proxy }` under the key `last`. Attaching comments to a remote library updates the
      record with `commentsName` and `commentsBytes`.
    - **Failures.** A storage failure or quota error is caught. The summary notes that this
      library won't be offered after a reload, and nothing else breaks.
  - **Reopen button.** On start-up, if `last` exists, show a **Reopen <name> (streaming)**
    button, `#library-reopen`. Clicking it rebuilds the remote source with the stored
    proxy, unless `localStorage` has an override, and loads the stored bytes under the stored
    name. The identity is unchanged, so playlist entries bound to it resolve. It then
    attaches the stored comments, if any.
- **Unchanged.** The server library, folder sources, uploads, the playlist, comments search,
  the tree and prefetch must all behave exactly as before. All existing tests stay green.

### 3. Documentation

- **`LIBRARY-CSV.md`.** A "Streaming from `source_url`" section:
  - `source_url` is optional;
  - the base-plus-path rule and how the base is derived;
  - direct mode versus proxy mode, and why a proxy is needed for servers without CORS;
  - the reopen behaviour, and that the CSV bytes are stored locally in the browser.
- **`apps/starplayer-web/README.md`.** How to stream the Hornet archive on the published
  site: open `hornet-modules.csv` (and optionally `hornet-comments.csv`), click **Stream
  from source URLs**, and use **Reopen** after a reload. Link to `remote-proxy/README.md`.

## Research points

- Under COEP `require-corp`, a cross-origin `fetch()` in its default `cors` mode succeeds
  only when the response passes CORS. `no-cors` would give an opaque response, which is
  useless. Keep the default mode and no credentials.
- `coi-serviceworker.js` intercepts fetches on Pages. Confirm by reading it that CORS-mode
  cross-origin requests pass through unchanged. The headless test runs with real headers
  (`--no-isolation` covers the plain mode), so note any shim-specific risk in the final
  message rather than guessing.
- The Worker's per-segment decoding should match `confinedPath` in `dev-server.mjs`. Reuse
  the same rules, but not the code, because the Worker must stay self-contained.

## Verification

- **`node --test apps/starplayer-web/test/*.test.mjs`**, run from the repo root.
  - **New `test/remote-proxy.test.mjs`.** It imports `../remote-proxy/worker.js` and stubs
    `globalThis.fetch` to record upstream calls. It checks:
    - path mapping and re-encoding (a segment with a space and a `#`);
    - 400 for `..`, `.`, an empty segment, an encoded `%2e%2e`, an encoded `%2F` that
      decodes to `/` inside a segment, a backslash, malformed `%zz`, and the root path;
    - 405 for POST, and the 204 preflight headers;
    - 403 for a disallowed origin, with **no** upstream call;
    - no CORS headers when there is no `Origin`;
    - the headers passed through, plus CORP and `Cache-Control`;
    - an upstream 404 passed through;
    - 413 for a large `Content-Length`;
    - HEAD returning no body;
    - `env` overrides of `UPSTREAM` and `ALLOWED_ORIGINS`.
  - **Extended source tests.**
    - The HTTP source builds URLs for both the default base and a custom one.
    - `createRemoteSource.attach`: base derivation, including encoded `source_url` values,
      a minority of mismatched rows (counted, and reading one throws), no `source_url` at
      all (throws), proxy versus direct URLs, the `TypeError` → helpful-message mapping, and
      `AbortError` passthrough.
    - The IndexedDB v2 upgrade keeps a v1 `handles` record. Use the in-repo IndexedDB shim
      approach from `playlist-store.test.mjs`, or cover it in the headless test.
- **`cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`**
  passes in all three modes. Add a `runRemoteLibrary` scenario, run in every mode, and make
  `--library-only` include it:
  - **The upstream.** A second local HTTP server on a different port, so it is a different
    origin from the page. It serves synthetic `loose/REFLEX.S3M` and `disks/disk1.zip`
    (PETRI and REFLEX members, built the way `runLibrary` builds them). It has a switch for
    whether it sends `Access-Control-Allow-Origin: *` and
    `Cross-Origin-Resource-Policy: cross-origin`, and it counts requests.
  - **The proxy.** A third local server: a tiny adapter that turns Node `http` requests into
    `Request`, calls the real `worker.js` `fetch` with `env = { UPSTREAM: <upstream base>,
    ALLOWED_ORIGINS: <page origin> }`, and pipes the `Response` back.
  - **The index.** A synthetic index CSV whose `source_url` values point at the upstream. It
    is loaded through `#library-index` by setting `input.files` from a `DataTransfer`, as the
    upload tests do.
  - **Direct mode** (upstream with CORS):
    - **Streaming.** Click **Stream from source URLs**; the row count and the summary
      appear, double-clicking the loose REFLEX row plays it (asserted by its title), and a
      ZIP member plays.
    - **Reload.** Add a row to the playlist and reload the page. **Reopen** appears; click
      it, then play the playlist entry.
  - **Proxy mode** (upstream without CORS, Proxy URL set to the adapter):
    - **Playback.** A track plays.
    - **Routing.** The upstream request counter shows the bytes came through the adapter,
      with its `User-Agent` header seen upstream.
  - **Failure.** With the proxy cleared and the upstream still without CORS, playing a row
    shows the "may not allow cross-origin requests — set a Proxy URL" message, and the
    previously playing track keeps playing.
- **`cargo xtask ci`** passes.
- **Final message.** Give the files changed and why, the test results, the shim analysis
  from the research points, and anything done differently from this spec, with the reason.

## Out of scope

- Deploying the Worker; the owner does it.
- Hosting the CSVs on Pages.
- Range requests, custom rate limiting, and more than one upstream per Worker.
- Fetching per-row `source_url`s that don't follow the derived base. They are reported as
  not streamable.
- Changes to the dev server or xtask.
