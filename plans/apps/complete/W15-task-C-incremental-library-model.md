# W15-C — Incremental library rows, search and comments

| Field | Value |
|---|---|
| Status | Complete — reviewed and integrated on `w15-folder-library`; final automated acceptance passed |
| Recommended model | gpt-6.1-sol |
| Recommended effort | **high** |
| Depends on | None; parallel with A and B |
| Branch / sibling worktree | `w15-c` / `../starplayer-w15-c` |
| Ownership | `library-index.js`, new pure library helpers, model unit tests |
| Coordinator | gpt-6.1-sol, xhigh; [master](W15-master-plan.md) |

## Context for a fresh agent

Read `AGENTS.md`, [W15 master](W15-master-plan.md), and the complete
[behavioral specification](W15-task-folder-library-discovery.md). The master defines
shared APIs; the behavioral specification defines edge cases and compatibility. Work
from this task file without relying on conversation history. Do not commit or start
another task. Report changes, verification results and handoff contracts to coordinator.
All paths below are repository-relative; web JS paths are under
`apps/starplayer-web/www/` unless stated otherwise.

parseLibraryCsv currently builds immutable-style row-indexed arrays; search indexes,
sortKeys and comments must stay consistent when rows are appended or enriched. CSV
rows use row.id as array position. playlist-transfer caches a locator set in a WeakMap;
F owns that consumer change, while C exposes revision and locatorIds.

## Deliverables

- Implement createDiscoveredLibrary, appendDiscoveredRows and updateDiscoveredRows
  with the master's signatures and DiscoveredRecord/RowUpdate schemas. Share normalization/search preparation with CSV parsing
  without serializing synthetic CSV. Stable append-only row IDs, locator deduplication,
  revision increments, changed ID results and locatorIds are required.
- Records use the existing source/member fields plus metadataState/error. Normalize
  provisional extension formats, unknown fields and row paths; never derive title or
  channels from guesses. Distinguish unextractable archive rows from metadata errors:
  inspection failure must leave otherwise eligible rows playable.
- Update affected search/field/comment haystacks, sorting, hasComments and membership
  consistently. Retain the latest sidecar map; rejoin when row SHA arrives, preserve
  sidecar precedence and remove old sidecar contributions on replacement. Keep current
  CSV attachComments results/API compatible.
- Provide full case-sensitive locator tie-breaking after existing sort keys. Metadata
  changes must affect filter/sort results without changing row IDs. Export the shared
  locator key helper; document row defaults and callback-ready changed IDs for D2/E/F.
- Keep core model operations independent of DOM, worker, storage and playback.

## Research points

Inspect library-index tests for quoted CSVs, numeric fields, comment search, tree
building and playlist CSV parsing. Specify batch revision behavior consistently;
update sort/search arrays only through shared helpers so CSV/discovered results agree.

## Verification

Run `node --test apps/starplayer-web/test/library-index.test.mjs` and new incremental
model tests. Cover append/enrich/search equivalence, stable IDs, duplicate locators,
case-sensitive tie order, sidecar before/after SHA and replacement, metadata error
visibility, unplayable archive members, empty libraries and unchanged CSV parsing.
Test 40,000 synthetic records in batches for correctness without per-row full-library
rebuilds. F later tests actual WeakMap consumer invalidation.

## Out of scope

Source enumeration, DOM controls/view, worker scheduling, playlist-transfer edits,
app bootstrapping and persistent catalog caching.

## Verification evidence (2026-10-26)

Worker: existing model tests 18/18, new incremental tests 8/8; full web Node
suite 86/86 passed with localhost access. Coordinator independently reran both
model test files successfully and reviewed the complete diff. `git diff --check`
passed. The 40,000-record synthetic test verifies batched append/enrichment without
whole-library traversal. The only old assertion update applies the required
case-sensitive locator tie order.

Handoff: `locatorIds` is a `Map<string, rowId>`; revision advances once per changed
batch. `comments`, `commentCounts`, `commentChangedRowIds`, `formats` and
`folderCounts` expose incremental state without changing `attachComments`’ return.

Final integrated automated acceptance passed in G, including all three browser
modes and full 11-job repository CI. Remaining manual owner acceptance is recorded
in the [master](W15-master-plan.md). This task is archived with the completed
integration deliverable; `main` was not merged or deployed.
