# W15-A — Local enumeration, CSV candidates and folder identity

| Field | Value |
|---|---|
| Status | Complete — reviewed and integrated on `w15-folder-library`; final automated acceptance passed |
| Recommended model | gpt-6.1-sol |
| Recommended effort | **high** |
| Depends on | None; parallel with B and C |
| Branch / sibling worktree | `w15-a` / `../starplayer-w15-a` |
| Ownership | `library-source.js`, new `library-identity.js`, source/identity unit tests |
| Coordinator | gpt-6.1-sol, xhigh; [master](W15-master-plan.md) |

## Context for a fresh agent

Read `AGENTS.md`, [W15 master](W15-master-plan.md), and the complete
[behavioral specification](W15-task-folder-library-discovery.md). The master defines
shared APIs; the behavioral specification defines edge cases and compatibility. Work
from this task file without relying on conversation history. Do not commit or start
another task. Report changes, verification results and handoff contracts to coordinator.
All paths below are repository-relative; web JS paths are under
`apps/starplayer-web/www/` unless stated otherwise.

The current directory source lists only root files for CSV discovery. FileList builds
its map synchronously. IndexedDB `starplayer-library` v2 holds one bare directory
handle at `handles/root` and remote indexes in `indexes`. Inspect source tests first.

## Deliverables

- Implement the master's abortable `enumerate` contract for both local source types.
  Iteratively walk the tree (avoid recursive call-stack growth), yield to the event loop
  in bounded batches, and catch per-path errors. No loose file content reads. Retain
  the iterator between pause/resume; abort checks bracket awaited operations.
- Yield while building large FileList maps, and retain source path access for saved
  locators even before discovery reaches them. Share validation with readFile; report
  invalid/unreadable entries as path issues instead of aborting the entire walk.
- Validate root CSV headers with the existing parser: source_path required, comments
  recognized separately, playlist library-column excluded from discovery candidates.
  Prefer non-playlist index.csv, otherwise sole candidate. Preserve manual CSV opening
  compatibility. D2 implements error UI and explicit index bypass, not this task.
- Implement v3 identity registry and the master's folderIdentity API, preserving last
  reconnect handle and remote index records. Same-entry comparison must preserve A→B→A
  identity, session fallback and same-source token reuse. Use random 64-hex tokens.
  Legacy bare handles migrate on first discovered use; no automatic registry eviction.
- Keep browser picker activation before awaits and handle storage failures without
  losing the selected source. Export documented producer APIs for D2.

## Research points

Check existing readHead CSV BOM/quoting behavior, permission errors and IndexedDB
upgrade blocked/versionchange handling. Keep the initial browser-controlled FileList
picker delay distinct from application yields. Coordinate API changes before D2 starts.

## Verification

Run `node --test apps/starplayer-web/test/library-source.test.mjs` plus new source/
identity test files. Cover no content reads, abort during walk, backpressure/pause,
invalid names, empty folders, missing children, root denial, mixed-case extensions and
CSV/playlist/comments headers. Use in-memory fake handles/IndexedDB for unit tests;
G owns real OPFS/reload tests. Assert v2→v3 preserves remote indexes and root handle,
A→B→A reuse, different same-named roots, comparison/storage failure and concurrent
same-page lookups. Provide the exact new test commands in handoff.

## Out of scope

UI, app wiring, background archive or metadata jobs, remote directory discovery,
playlist schema, changes to the user's AGENTS.md edit or any other task's files.

## Verification evidence (2026-10-26)

Worker: source/enumeration/identity tests 25/25 and full web Node suite 96/96
passed with localhost access; `git diff --check` passed. Coordinator reviewed all
production and new test files and independently reran the three focused test files
successfully. Real OPFS/IndexedDB reload verification passed in G; native picker prompts remain manual.

Handoff: abort throws `AbortError`; retain the async iterator for pause/resume.
An issue at path `''` is a retryable root-walk failure. Directory enumeration
uses `size:null`, `timestamp:''` without metadata/content I/O. FileList mapping
and enumeration yield every 128 entries; its complete map resolves saved locators
before discovery. `folderIdentity` is exported by both source and identity modules.
Valid-header/malformed-body CSVs remain candidates for the explicit D2 error UI.

Final integrated automated acceptance passed in G, including all three browser
modes and full 11-job repository CI. Remaining manual owner acceptance is recorded
in the [master](W15-master-plan.md). This task is archived with the completed
integration deliverable; `main` was not merged or deployed.
