# Library and playlist CSV contract

A library index locates modules beneath a library root. Put it at the root as
`index.csv` for automatic discovery, or open it separately (source selection and
opening are implemented in W8). ModDNA's `hornet_index.py` output loads unchanged.
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
are rejected. Invalid rows are skipped and reported in `issues`. `member_path` and
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
| `sha256` | Accepted module checksum metadata, currently not used for lookup or library identity. |

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
Editing the CSV changes identity; playlists will offer to rebind in W11.

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
Folder scope includes that folder and its descendants. Sort columns are `name`,
`title`, `format`, `channels`, `size`, `timestamp`, `folder`. Text sort keys are
lower-case lexical strings, computed at parse time; ties use folder, then name,
then original row order. Null numeric values sort first in ascending order.
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
library names; identity rebinding and persistence belong to W11.

## Opening a library in the web player

Use **Open library folder** to select the root. The player prefers a root
`index.csv` (case-insensitive), otherwise the only root CSV, or offers a choice
when several exist. **Open index CSV…** selects an index stored elsewhere; then
click **Open library folder** separately to grant access to its files.

Chromium's directory picker requires a secure context (localhost or HTTPS). Its
read-only handle is stored in IndexedDB when available; after reloading, click
**Reconnect <name>** to request read permission again. Other browsers use a folder
file input: this selection cannot be restored, so choose the folder again after
every reload. No library files are uploaded to a server.

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
are reported and retained as unavailable. Opening another index offers **Rebind N
entries to …** when its keys match; clicking explicitly replaces those entries'
identities. Rebinding never happens automatically.

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
