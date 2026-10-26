# Library and playlist CSV contract

A local library folder can be opened without an index. Directory handles discover
files progressively; loose MOD/S3M/MTM/XM/IT entries appear by filename without
reading module contents. ZIP files are catalogued automatically in a background
worker, including one inner ZIP level. Results are usable while discovery continues.
Selecting tracks or adding them to a playlist reads no module contents.

A library index locates modules beneath a library root. Put it at the root as
`index.csv` for automatic discovery, or open it separately with **Open index CSV…**. ModDNA's `hornet_index.py` output loads unchanged.
The index never requires a download URL or an absolute filesystem path.

Use UTF-8 text, optionally starting with a UTF-8 BOM. CSV follows RFC 4180: a header
row, comma-separated fields, LF or CRLF record endings, and optional final newline.
Quote fields containing commas, quotes or newlines with `"`; double a quote inside
a quoted field. Embedded newlines are preserved. Malformed quoting is an error.
Header names are trimmed and case-insensitive, may appear in any order, and unknown
columns are ignored. A repeated known header is an error, as is a missing
`source_path` header. Issues on individual rows report their starting physical line
number, including newlines inside earlier quoted fields.

## Paths and locators

`source_path` is the only required column. It names a file relative to the library
root using `/` separators. It must be non-blank and relative: absolute paths,
Windows drive prefixes such as `C:`, backslashes, and segments equal to `.` or `..`
as well as empty segments and NUL characters are rejected. Invalid rows are skipped and reported in `issues`. `member_path` and
`container_member` are names inside an archive, not filesystem paths, so they are
not restricted this way (old DOS archives store `\` separators). Blank lines are
ignored. Path spelling and case are preserved.

Optional locator columns:

| Column | Meaning |
| --- | --- |
| `member_path` | Member inside `source_path` when that file is a ZIP; blank for a loose module. |
| `container_member` | An inner ZIP inside `source_path`; `member_path` then names a member in that inner ZIP. One nesting level is supported. |
| `member_path_raw_hex` | Exact ZIP member name bytes in hexadecimal, for `member_path` only. Supply this for non-ASCII names, especially legacy CP437 names, to avoid decoding ambiguity. Both upper- and lower-case hex are accepted; odd length or non-hex digits fail extraction. |

Archive resolution first tries the supplied raw bytes, then exact UTF-8 or CP437
encodings of the path, then case-insensitive decoded names with slash/backslash
normalisation. That final fallback must identify exactly one entry. Ambiguity is
an error naming the candidates. Directories and `__MACOSX` resources are skipped.
Inner ZIPs and modules use the archive extractor's 64 MiB per-entry bound.

## Metadata and filtering

All metadata columns are optional:

| Column | Use |
| --- | --- |
| `title` | Display title and text search. |
| `format_detected` | Preferred format label, from inspecting content. |
| `format_by_extension` | Format label used when `format_detected` is blank. |
| `extension` | Accepted compatibility metadata; fallback derives the extension from the innermost path itself. |
| `channels` | Numeric channel count for display, sorting and `ch:` queries; blank or non-numeric becomes null. |
| `tracker` | Display metadata and text search. |
| `size` | Uncompressed module size in bytes for display/sorting; blank or non-numeric becomes null. |
| `timestamp` | Original timestamp text, sorted lexically; no timezone is inferred. |
| `instruments`, `samples` | Accepted compatibility metadata, currently not retained in compact browse records. |
| `sha256` | Module checksum used to join comments; trimmed and lower-cased if 64 hex digits, otherwise empty. It does not set library identity. |

The format is the first non-blank of `format_detected`, `format_by_extension`,
then the lower-case extension of the innermost path (`member_path`, otherwise
`container_member`, otherwise `source_path`). Labels are lower-cased. MOD, S3M,
MTM, XM and IT are playable. Other formats remain indexed; search hides them
unless `showUnplayable` is enabled.

Rows with `entry_kind` equal to `directory` are skipped. If either `status` or
`extended_status` is present and non-blank, it must be `ok` for the row to be kept.
These checks ignore case and surrounding whitespace. Filtered rows are not issues.
The row key is `(source_path, container_member, member_path)`, case-sensitive.
The first duplicate wins; later duplicates produce issues. Kept rows receive
contiguous zero-based ids. A member's folder combines its outer path, inner ZIP
path if any, and member directories, treating each ZIP as another folder level.

Library identity is `{sha256, name}`: SHA-256 of the exact CSV bytes followed by
the UTF-8 bytes of its identity name, plus that name. BOM and line endings count.
Editing the CSV changes identity; playlists offer explicit rebinding.

## Minimal example

```csv
source_path,container_member,member_path
loose/song.mod,,
packs/collection.zip,,music/song.s3m
packs/collection.zip,bonus/inner.zip,music/song.it
```

## Queries and tree

Queries AND whitespace-separated, case-insensitive substring tokens; double quotes
keep phrases together. Prefixes are `format:`, `title:`, `name:`, `path:` and
`tracker:`. `path:` includes archive and member paths. Examples: `blue sky`,
`title:"night flight"`, `format:xm ch:>8`, `ch:<=4`. `ch:8` tests exact equality;
`<`, `>`, `<=`, `>=` and `=` are supported. Missing channels never match `ch:`.
A leading `-` excludes rows that the rest of the token matches, under the same rules
(prefix, folding, compact matching, and the comments checkbox): `oxygen -oxygene`,
`-text:remix`, `-format:mod`, `-ch:>8`, `-"some phrase"`. A dash inside quotes is
literal (`"-=FC=-"` searches for that text), and an empty exclusion such as `-text:`
is ignored.
Folder scope includes that folder and its descendants. Sort columns are `name`,
`title`, `format`, `channels`, `size`, `timestamp`, `folder`. Text sort keys are
lower-case lexical strings, computed at parse time; ties use folder, then name,
then the case-sensitive full locator tuple. Null numeric values sort first in
ascending order.
`buildTree` returns a root node at path `''`, with nested `children`; each node has
`path`, `label`, `kind` (`dir`, `zip`, `nested`) and recursive playable `trackCount`.

## Playlist CSV

Playlist CSV uses the same syntax, locator validation, header and metadata rules,
but preserves order and duplicate locators. An optional `library` column contains
the originating library's identity name. The writer emits this header:

```csv
source_path,container_member,member_path,member_path_raw_hex,title,format_detected,library
```

An A–B–A list stays A–B–A on export/import. Non-ASCII names and raw hex survive,
as do titles containing quotes, commas and newlines. The CSV records locators and
library names; identity rebinding and persistence are described below.

## Opening a library in the web player

Use **Open library folder** to select the root. The player prefers a root
`index.csv` (case-insensitive), otherwise the only root index candidate (excluding comments CSVs), or offers a choice
when several exist. Candidates require a `source_path` header; playlist-shaped CSVs
with a `library` column are excluded from automatic selection, even if named
`index.csv`. A folder without an index enters discovery directly. **Browse folder
without an index** bypasses a valid or pending index; a malformed index reports its
error and offers **Browse folder instead** rather than silently switching modes.
**Open index CSV…** selects an index stored elsewhere; then
click **Open library folder** separately to grant access to its files.

Chromium's directory picker requires a secure context (localhost or HTTPS). Its
read-only handle is stored in IndexedDB when available; after reloading, click
**Reconnect <name>** to request read permission again. Other browsers use a folder
file input: this selection cannot be restored, so choose the folder again after
every reload. Its picker finishes enumerating the folder before delivering the
FileList; that initial browser-controlled wait cannot be removed. Application
processing afterward yields to keep controls responsive. No library files are
uploaded to a server.

**Stop discovery** pauses walking and archive jobs; **Resume discovery** continues
without duplicating rows. Partial folder-tree **+** actions say **Add discovered
tracks in …** and add the click-time snapshot. Later discoveries are never appended
to the playlist automatically. Source replacement cancels old work; cancelling the
picker preserves the current library.

Filename results initially have unknown title, channel count and comments. Playback
reads only its selected module and enriches its matching row and playlist entries.
**Scan metadata** explicitly reads eligible modules in a worker, computes hashes and
updates search/filter results without replacing the playing module. **Cancel metadata scan**
stops that optional work. Known-row inspection continues while discovery is paused;
scan completion waits
until discovery finishes and the inspection queue is empty.
Inspection failures remain playable, show error details/tooltips and offer **Retry
metadata**. Worker failures have a separate **Retry** control. Known unextractable
archive members remain visible through **Show unplayable** and its live count.
Each source file read is limited to 128 MiB. ZIP member extraction is limited to
64 MiB; unsupported/encrypted members and
container failures retain explanations. Invalid paths and unreadable folders are
skipped with a path-based issue list; a root failure offers discovery retry.

For a local HTTP source, run `cargo xtask serve --library <directory>` and select
**Server library**. The default index is `<directory>/index.csv`; supply
`--library-index <file>` when it lives elsewhere. Both options also work with
`cargo xtask wasm --serve`. Without `--library`, the server source is hidden.

## Saving and exchanging playlists

**Export playlist** downloads the library entries in their current order, including
repeats. `library` contains each entry's original library identity name. Uploaded
files are omitted, and the status message reports the number skipped. The CSV
contains locators, not module bytes. It can also be opened as a small library index.

**Import playlist** appends rows bound to the currently open library. Missing keys
are reported and retained. For a matching discovered folder, **not yet discovered**
and, after completion, **not found in scan** are informational: explicit playback
and advance still try the saved locator. Actual read/extraction errors decide whether
it plays. Indexed-library entries absent from their index remain unavailable.
Opening another index or folder offers **Rebind N
entries to …** when its keys match; clicking explicitly replaces those entries'
identities. Rebinding never happens automatically. For a discovered folder it stays disabled
while discovery is running, paused or failed and becomes available only after a
complete scan, so one click rebinds the complete matching set.

Discovered folders keep an opaque random identity alongside the directory handle in
IndexedDB, reused when `isSameEntry` confirms a reconnect or reselection. Identical
display names do not identify folders, and adding/removing files does not change the
identity. Reconnect re-enumerates the folder; there is no persistent catalog or
folder metadata cache and no filesystem watcher. FileList selections and unavailable
storage use session identities; after
reselection, explicitly rebind relative locators. If a folder gains an index, opening
it chooses the CSV identity; bypassing that index restores its folder identity.

Playlist export leaves `format_detected` blank for unknown or failed discovered
metadata and writes the actual parsed format after enrichment. Legacy entries keep
their previous behavior until enriched. Provisional imported titles/formats can be
displayed, but do not count as verified metadata.

The browser saves ordered entries, current entry id, loop setting, known library
identities and uploaded files in IndexedDB. Libraries must be reopened after reload;
entries show **library not connected** with their identity name until then. Missing
uploaded bytes show **missing — re-add**. Playback skips unavailable entries.

Use **Add files…** or drop files onto the Playlist panel to keep modules and ZIP
members across reloads. ZIP members use numeric directory indices, including one
nested ZIP level. A shared ZIP is stored once and removed when its last entry is
removed. Stored files are capped at 256 MiB, with only unreferenced files eligible
for eviction. A refused file can still be opened once using the main file picker.
Storage failure switches to session-only use with a notice. Browser storage may be
cleared or evicted; CSV export does not back up uploaded bytes.

## Comments CSV (optional)

A comments sidecar joins author text to library rows by `sha256`. Discovery does
not hash loose modules: sidecar counts show awaiting metadata/hashes until playback
or an explicit scan supplies them. Sidecars take precedence over extracted names.
Module inspection extracts instrument/sample names; song messages are available
only from a sidecar or inline CSV, not from module inspection. Use UTF-8, RFC 4180
quoting and LF record endings, including quoted multiline fields. The usual header
is `sha256,format,instrument_names,sample_names,song_message`. Header names are
trimmed and case-insensitive and can be reordered; unknown columns are ignored,
duplicate headers are errors. Require `sha256` and at least one text column.

Use one row per distinct module checksum, sorted by checksum. A checksum must be
64 hexadecimal digits (case is ignored); invalid or duplicate checksums produce
issues and the first occurrence wins. Modules without text may be omitted. Each
checksum should occur in the index; several tracks may share it. `format` is
informational. Text columns are:

- `instrument_names`: one line per declared slot, preserving empty slots so line N
  is slot N. Drop trailing empty lines. IT sample-mode names belong in `sample_names`.
- `sample_names`: XM and IT sample names, with the same slot numbering convention.
- `song_message`: IT song message or MTM comment, preserving line breaks.

Decode legacy module text from CP437 before writing UTF-8. Convert CR/CRLF to LF,
replace NUL and other control characters with spaces, and trim trailing spaces
on each line. ASCII art is welcome. All text is untrusted and displayed literally.

For a small hand-made index, put optional `instrument_names`, `sample_names` and
`song_message` columns directly in the index instead. A sidecar entry overrides all
three inline fields for a matching checksum, including empty fields. Without an
entry, inline text remains available. Replacing the sidecar replaces its attachment;
it never changes the library identity or playlist bindings.

Folder sources sniff root CSV headers, separating comments files (checksum plus a
text column, without `source_path`) from index candidates. They prefer `index.csv`,
otherwise the only index candidate, otherwise offer a choice. Exactly one comments
candidate loads automatically; with several, use **Open comments CSV…**. Failed
header reads are ignored. That button can also attach an external sidecar to the
open library; the choice lasts until reload. The table appears before comments load,
and errors in comments do not prevent browsing.

For HTTP, `cargo xtask serve` and `cargo xtask wasm --serve` accept
`--library-comments FILE`, serving it at `/library-comments.csv`. The page probes
that route after loading the index. There is no default comments location.

Plain search includes comments by default; **Search comments** turns that off.
`text:"purple motion"` searches comments only regardless of the checkbox. All text
search lowercases and uses Unicode NFKD with combining marks removed, so `cafe`
finds `Café`; box-drawing symbols survive. Plain and `text:` tokens also match a
compact form with punctuation and spacing removed, when the compact token has at
least four letters/digits: `purplemotion`, `purple-motion` and `"purple motion"`
match `Purple Motion`; `p.motion` matches `P. Motion`. `fc` does not compact-match.
Other field prefixes use folded substring matching without compaction. The details
pane, the **Comment match** column and the hover popover highlight what matched under
these same rules (excluded tokens are never highlighted), and the pane preserves
numbered empty slots.

Three synthetic module rows (the first has a multiline field):

```csv
sha256,format,instrument_names,sample_names,song_message
1111111111111111111111111111111111111111111111111111111111111111,mod,"Demo Group

Lead",,
2222222222222222222222222222222222222222222222222222222222222222,xm,Composer,Kick,
3333333333333333333333333333333333333333333333333333333333333333,it,,,Greetings!
```

## Streaming from `source_url`

The optional `source_url` column lets an index stream files without a local mirror.
Open the index and click **Stream from source URLs**. The player chooses the base
prefix for which the most rows equal `base + source_path`, accepting either literal
paths or paths encoded one segment at a time. Rows outside that base are reported
as unmatched and cannot be streamed. Archive members still use the same ZIP resolver.

Leave **Proxy URL (blank: fetch directly)** blank for direct mode when the archive
server supports CORS. Servers without CORS, including Hornet, need the
[remote proxy](remote-proxy/README.md); enter its absolute HTTP(S) URL. Requests
remain in CORS mode, including on the cross-origin isolated published player.
Changing the proxy clears cached archives for later reads.

The browser stores the exact index CSV bytes and name, proxy setting, and any
manually attached comments CSV in local IndexedDB. After a reload, **Reopen <name>
(streaming)** reconnects it with the same library identity and saved playlist
bindings. A saved Proxy URL overrides the record's proxy. Storage failure leaves
streaming usable but reports that reopening will be unavailable after reload.
