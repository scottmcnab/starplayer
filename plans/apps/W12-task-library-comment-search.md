# W12 — Search module comments: instrument names, sample names and song messages

| Field | Value |
|---|---|
| Milestone | Web player: music library and playlist (W7–W12, branch `library-playlist`) |
| Status | Specified 2026-10-23 |
| Depends on | [W7](W7-task-library-index-and-member-lookup.md), [W8](W8-task-library-sources-and-resolver.md), [W9](W9-task-library-view.md) |
| Blocks | Landing `library-playlist` on `main` |
| Recommended model | gpt-6.1-sol |
| Verified by | agent (`node --test apps/starplayer-web/test/*.test.mjs`, `cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`, `cargo xtask ci`), then the owner in a browser on the Hornet mirror |

## Context for a fresh agent

Read `CLAUDE.md` (full variable names, compact formatting, no `cargo fmt`; do **not**
commit). Then read, in this order:

- `apps/starplayer-web/LIBRARY-CSV.md`, the public CSV contract;
- `apps/starplayer-web/www/library-index.js` (`parseCsv`, `readRows`, `parseLibraryCsv`,
  `compileQuery`, `search`, `buildTree`);
- `apps/starplayer-web/www/library-source.js` (`discoverIndex` and the three sources);
- `apps/starplayer-web/www/library-controls.js` (how an index is chosen and loaded);
- `apps/starplayer-web/www/library-view.js`, `index.html` and `style.css` (the toolbar,
  the virtualised grid, the folder tree);
- `apps/starplayer-web/dev-server.mjs` (`--library`, `--library-index`, `confinedPath`) and
  `xtask/src/main.rs` (`serve` and `wasm --serve` pass `--library*` options through);
- the tests: `apps/starplayer-web/test/library-index.test.mjs`,
  `library-source.test.mjs`, `dev-server-library.test.mjs`, and `runLibrary` in
  `test/headless.mjs`.

Module authors hid text in their modules. The song title is one place, but greetings,
group names, handles and ASCII art mostly live in the **instrument and sample names**,
and IT files carry a **song message**. The owner wants to find, for example, every track
by "Purple Motion" or "Future Crew". Today the library searches only name, title, folder,
format and tracker. The index CSV has instrument *counts* but not names.

ModDNA, the tool that writes the index, now also writes a **comments sidecar CSV**. For the
Hornet mirror it is `/home/scott/projects/moddna/index/hornet/hornet-comments.csv`:
9,204 rows, 6.6 MB, beside `hornet-modules.csv` (9,585 rows). Its contract, fixed by
ModDNA:

- One row per distinct module `sha256`, sorted by `sha256`. Modules with no text are
  omitted. Every `sha256` in it occurs in `hornet-modules.csv`'s `sha256` column. Several
  index rows can share one `sha256`, because the same module appears in several places.
- Columns, in this order: `sha256, format, instrument_names, sample_names, song_message`.
- RFC 4180 quoting, UTF-8, LF line endings, multi-line quoted fields.
- `instrument_names`: one line per declared slot. Empty slots are kept as empty lines, so
  line N is slot N. Trailing empty lines are dropped. IT files in sample mode have empty
  `instrument_names` and their names in `sample_names`.
- `sample_names`: XM and IT only.
- `song_message`: IT song message and MTM comment.
- The text is decoded from CP437. CR and CRLF become `\n`; NUL and other control characters
  become spaces; trailing spaces are trimmed on each line. It contains ASCII art, including
  runs of `<`, `=` and `>`, and it is **untrusted**: it must only ever reach the page as
  text, never as HTML.

Rules for this task:

- **Never write into `/home/scott/projects/moddna` or `/mnt/d/hornet-music`**, and never
  copy rows from the real ModDNA CSVs into the repository. Test fixtures are synthetic.
- Never use `ARMANI.S3M` as a fixture.
- Keep the existing behaviour and tests green. In particular, the library's identity stays
  the SHA-256 of the *index* CSV plus its name. The comments file does not change it,
  because playlists are bound to that identity.

## Deliverables

### 1. Data layer (`library-index.js`)

- **Keep `sha256` on library rows.** `readRows` stores `row.sha256`: the trimmed,
  lowercased value when it is 64 hex digits, otherwise `''`.
- **Inline comment columns.** The index CSV may itself carry optional `instrument_names`,
  `sample_names` and `song_message` columns, so a hand-made CSV can be searched without a
  sidecar. Store them on the row as `instrumentNames`, `sampleNames`, `songMessage`
  (`''` when absent).
- **`parseCommentsCsv(text) -> { comments: Map<sha256, { instrumentNames, sampleNames,
  songMessage }>, issues }`.**
  - Header names are matched as in the index: trimmed, case-insensitive, in any order.
    Unknown columns are ignored, and a duplicated header is an error.
  - Required: `sha256` and at least one of the three text columns. Otherwise throw an
    `Error` that names the missing columns.
  - A row whose `sha256` is not 64 hex digits is reported as an issue `{ line, reason }` and
    skipped. A repeated `sha256` is reported and the first occurrence wins. Blank records
    are skipped, as in `readRows`.
  - Reuse `parseCsv`. Do not write a second CSV parser.
- **`isCommentsCsvHeader(firstLine) -> boolean`** for discovery: true when the header has
  `sha256` and at least one text column and has **no** `source_path`. Parse the line with
  `parseCsv` so that quoted header names work.
- **`attachComments(library, comments | null)`** joins the sidecar to the library by
  `row.sha256`:
  - **Precedence.** Each row's comment text comes from the sidecar entry when there is one,
    otherwise from the inline columns.
  - **Storage.** It builds `library.commentHaystacks`, indexed by row id, from all three
    texts. Rows that share a `sha256` must share **one** string, not copies.
  - **Counts.** It returns `{ joined, unmatched }`:
    - `joined`: the number of library rows that got sidecar text;
    - `unmatched`: the number of sidecar entries with no library row.
  - **Lifecycle.** Calling it again replaces the previous attachment, and `null` detaches
    the sidecar but keeps the inline text.
  - **Without it.** `parseLibraryCsv` builds `commentHaystacks` from the inline columns, so
    `search` never needs to check whether a sidecar exists.
  - **Display.** `library.commentsFor(rowId)` returns the row's `{ instrumentNames,
    sampleNames, songMessage }` for display, unfolded.
- **Text folding**, one exported function, `foldText(text)`, used for every haystack
  (existing and new) and every query token:
  - lowercase;
  - Unicode NFKD, then strip combining marks (`\p{M}`), so `é` matches `e`, and box-drawing
    and other CP437 symbols pass through unchanged.

  The existing `haystacks` must be folded the same way, so accented titles match
  unaccented queries.
- **Compact matching** for handles written with varying spacing and punctuation:
  - **The compact form.** The compact form of a folded string removes every character that
    is not a Unicode letter or digit.
  - **Haystacks.** The library keeps a compact form of each haystack beside it, for both
    the existing text and comments.
  - **Matching.** A free-text token matches a row when the folded haystack contains it,
    **or** when the token's compact form is at least 4 characters and the compact haystack
    contains it.

  Examples: `purplemotion`, `purple-motion` and `"purple motion"` all match "Purple Motion";
  `p.motion` matches "P. Motion"; `fc` (compact length 2) does **not** compact-match.
- **Query syntax** (`compileQuery` / `search`):
  - **New `text:` prefix.** `text:value` matches only comment text, with the same folding
    and compact rules.
  - **Plain tokens.** Plain tokens search the existing fields and, when
    `options.includeComments` is true (the default), comment text as well. Each token may
    match either.
  - **Other prefixes.** `name:`, `title:`, `path:` and `tracker:` use folding too, but not
    compact matching. `format:` and `ch:` are unchanged.
- **Budget.** `test/library-index.test.mjs` already has a generated 37,674-row fixture.
  Give every row synthetic comment text, about 10 MB in all, then assert:
  - parse plus attach plus build stays under 1.5 s;
  - one search keystroke (a plain token and a `text:` token) stays under 60 ms.

  Measure on this machine. If a budget is breached, optimise; do not raise the budget.

### 2. Finding the sidecar (sources, controls, dev server, xtask)

- **Folder sources (directory handle and `FileList`).**
  - **New interface member.** Each source gains `readHead(path, bytes = 65536) ->
    Promise<string>`, which decodes the first `bytes` bytes via `File.slice`, without
    reading the whole file.
  - **Classifying root CSVs.** `discoverIndex` becomes async (or a second step runs after
    it). It reads each root `*.csv` header with `readHead` and splits the files into index
    candidates and comments candidates (`isCommentsCsvHeader`).
  - **Choosing.** The existing index rules then apply to the index candidates only:
    `index.csv` first, otherwise the only candidate, otherwise ask the user. Exactly one
    comments candidate is attached automatically. With several, none is attached and the
    summary says so; the user can then use **Open comments CSV…**.
  - **Example.** A root holding `hornet-modules.csv` and `hornet-comments.csv` must pick the
    first as the index without asking, and attach the second.
  - **Isolation.** A header read that fails, for example on an unreadable file, marks that
    file as neither kind. It must not fail the library.
- **"Open comments CSV…" button** in the library source controls, a hidden
  `<input type=file accept=".csv,text/csv">`.
  - It attaches a chosen file to the *open* library, replacing any earlier sidecar. With no
    library open it is disabled.
  - It is not persisted. The summary says the choice lasts until the page reloads.
- **HTTP source and dev server.**
  - **Server.** `dev-server.mjs --library-comments <file>` serves that file as an explicit
    mapping at `/library-comments.csv`, using the same GET/HEAD handling and 404 rules as
    `/library-index.csv`. The usage line documents it.
  - **No default location.** Without `--library-comments`, `/library-comments.csv` is a
    404. Folder sources find the sidecar by sniffing headers instead.
  - **Page.** The HTTP source probes `/library-comments.csv` with HEAD after the index
    loads, and attaches the file on 200.
  - **xtask.** `cargo xtask serve` and `cargo xtask wasm --serve` accept
    `--library-comments FILE` and pass it through. Update their help lines.
- **Loading and errors.**
  - **Order.** The sidecar loads after the index has loaded and the table is showing. A
    slow or failing sidecar never delays or breaks the library.
  - **Staleness.** It is tied to the same revision counter as the index, so a sidecar that
    finishes after another library was opened is discarded.
  - **Summary.** The summary line gets a second sentence: for example "Comments: 9,204
    modules from hornet-comments.csv, 9,286 tracks matched" or "Comments: not loaded
    (reason)". It also names the issue count, as the index summary does.

### 3. Library view (`library-view.js`, `index.html`, `style.css`)

- **Toolbar.**
  - **"Search comments" checkbox**, checked by default. It sets `includeComments` and
    re-runs the search. It is shown only when at least one row has comment text, whether
    from the sidecar or inline.
  - **Query hint.** Add `text:` and one example (`"purple motion"` or `purplemotion`).
- **Comments details pane.** Below the grid, a pane titled with the focused row's name and
  title shows that row's comment text. The focused row is `selection.focus`, so the pane
  follows the arrow keys and clicks.
  - **Instrument names.**
    - Numbered lines, `01` to `NN`, two digits, or three when there are more than 99
      slots.
    - A monospace, non-wrapping block that scrolls horizontally inside the pane, so
      ASCII art keeps its columns.
    - Empty lines are kept, so that slot numbers stay true.
  - **Sample names**, when present, in their own titled block in the same style.
  - **Song message** in its own block, preserving line breaks.
  - **Empty states.** The pane shows "No comment text" when the row has none, and is
    hidden when no row is focused or no comment text is loaded at all.
  - **Highlighting.** Case-insensitive *literal* occurrences of the current free-text and
    `text:` tokens are wrapped in `<mark>`. Folded or compact-only matches need no
    highlight.
  - **Rendering rule.** Build the pane with `textContent` and `createElement` only. **No
    `innerHTML` with module text**, anywhere.
  - **Size and refresh.** The pane is height-bounded (around 260 px, scrolling) and usable
    at 390 px width. Update it on focus changes without re-rendering the grid.
- **Accessibility.** The pane is a labelled region. The grid's `aria-describedby` is not
  changed.

### 4. Documentation

- **`LIBRARY-CSV.md`.** A new section, "Comments CSV (optional)":
  - the sidecar contract above, written for someone building their own file;
  - the inline-column alternative;
  - the precedence rule and how each source finds the file;
  - the `text:` syntax, folding and compact matching;
  - a three-row minimal example, with one multi-line quoted field.
- **`apps/starplayer-web/README.md`.** The library paragraph and the Hornet serve command:

  ```
  cargo xtask serve --library /mnt/d/hornet-music --library-index ../moddna/index/hornet/hornet-modules.csv --library-comments ../moddna/index/hornet/hornet-comments.csv
  ```

## Research points

- `String.prototype.normalize('NFKD')` cost over about 10 MB once at load is acceptable.
  Never fold per keystroke per row: fold once into the stored haystacks, and fold only the
  query per keystroke.
- **Sharing strings by `sha256`.** Build one folded and one compact string per sidecar
  entry. Rows point to them through an array of references (`commentHaystacks[rowId] =
  sharedString`). Check heap use roughly with `process.memoryUsage()` in the budget test,
  and report it in the final message. Do not assert on it.
- The `File.slice(0, 65536).text()` in `readHead` can cut a UTF-8 sequence. Only the first
  line matters, so decode with `TextDecoder` without `fatal` and split on the first `\n`.
- The directory-handle source must not request extra permissions: header reads use the
  same handle.

## Verification

- `node --test apps/starplayer-web/test/*.test.mjs`. All existing tests pass, plus new
  tests for:
  - `parseCommentsCsv`: shuffled and upper-case headers; a missing required column; a
    multi-line quoted field; an invalid `sha256`; a duplicate `sha256`; a blank record;
  - `isCommentsCsvHeader` on the index header, the sidecar header, and a quoted header;
  - `attachComments`:
    - precedence over inline text, and inline text alone with no sidecar;
    - two rows sharing one `sha256`, which must share one haystack string (assert identity
      with `===`);
    - the `joined` / `unmatched` counts, and detaching with `null`;
  - `foldText` (`Café` → `cafe`, a CP437 box-drawing character unchanged);
  - compact matching (`purplemotion`, `purple-motion`, `"purple motion"`, `p.motion` all
    match; `fc` does not compact-match);
  - `text:` versus plain tokens, with `includeComments` true and false;
  - discovery with fake `FileList` sources:
    - `index.csv` plus `comments.csv`;
    - `hornet-modules.csv` plus `hornet-comments.csv` (no prompt);
    - two comments files (none attached);
    - an unreadable header;
  - the dev server: `/library-comments.csv` with an explicit file, a 404 without one, HEAD,
    and an unreadable mapped file;
  - **a real-file check, skipped when the files are absent.** Load
    `/home/scott/projects/moddna/index/hornet/hornet-modules.csv` and
    `hornet-comments.csv`, then assert:
    - no comment issues and `unmatched === 0`;
    - `joined > 9000`;
    - `search(…, 'text:"purple motion"')` returns at least 100 rows.

    The test reads these files only and never writes beside them.
  - the 37,674-row budget above.
- `cargo xtask wasm && STARPLAYER_REQUIRE_CHROMIUM=1 node apps/starplayer-web/test/headless.mjs`
  passes in all three modes. Extend `runLibrary`'s synthetic library:
  - **Fixture.** Write a `comments.csv` next to `index.csv` with `sha256` values matching
    two of its rows. The text includes a group name split across punctuation, an
    instrument named `<b>bold</b>`, and an IT-style multi-line message.
  - **Plain and `text:` search.** `text:<group>` narrows the count, and so does the compact
    form of the group name.
  - **The checkbox.** Unticking **Search comments** removes the comment-only match.
  - **Details pane.** Focusing a matching row shows numbered instrument lines, including a
    blank slot, and a `<mark>` around the literal match.
  - **Untrusted text.** `<b>bold</b>` appears as literal text; assert that no `b` element
    exists inside the pane.
  - **Discovery.** Serve it with `--library` and `--library-comments`.
- `cargo xtask ci` is green, because the xtask change touches Rust.
- **The owner's check**, after the Hornet serve command above:
  - `purple motion` and `text:"future crew"` find tracks;
  - the details pane shows the instrument names of a focused track;
  - with the folder picker on `D:\hornet-music`, `hornet-comments.csv` (if copied there by
    the owner) is attached automatically.

## Out of scope

- Changing ModDNA or its CSVs, and producing comment text for 669, FAR, STM, ULT, MED and
  OKT.
- Comment text in the now-playing view or the channel table. The player already shows the
  loaded module's own instrument names there.
- Ranking, fuzzy or edit-distance matching, stemming, an inverted index, Web Workers, and
  gzip input.
- Persisting a manually chosen comments file across reloads.
