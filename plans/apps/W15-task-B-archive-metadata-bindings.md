# W15-B — Structured archive catalog and metadata bindings

| Field | Value |
|---|---|
| Status | Specified — not started |
| Recommended model | gpt-6.1-sol |
| Recommended effort | **high** |
| Depends on | None; parallel with A and C |
| Branch / sibling worktree | `w15-b` / `../starplayer-w15-b` |
| Ownership | `crates/starplayer-archive`, `apps/starplayer-web/src/lib.rs`, Rust tests |
| Coordinator | gpt-6.1-sol, xhigh; [master](W15-master-plan.md) |

## Context for a fresh agent

Read `AGENTS.md`, [W15 master](W15-master-plan.md), and the complete
[behavioral specification](W15-task-folder-library-discovery.md). The master defines
shared APIs; the behavioral specification defines edge cases and compatibility. Work
from this task file without relying on conversation history. Do not commit or start
another task. Report changes, verification results and handoff contracts to coordinator.
All paths below are repository-relative; web JS paths are under
`apps/starplayer-web/www/` unless stated otherwise.

Current list_modules skips unsupported entries and has no raw name field. Existing
archive_modules is a tab-delimited upload API and must remain compatible. Module
inspection stores one module per WASM instance; only instrument name getters exist.
The model does not retain IT/MTM song messages, so this task does not extract them.

## Deliverables

- Add reusable structured catalog support to the archive crate with per-entry decoded
  and raw names, size, format/status; preserve current APIs. The new WASM archive_catalog
  JSON export implements the master's record schema and nested traversal policy.
- Enumerate module members without decompressing them; extract at most one inner ZIP
  level to enumerate it. Discover inner modules when no direct outer modules exist.
  Enforce 128 MiB source and 64 MiB extracted-entry limits; skip/report deeper containers.
- Corrupt/non-ZIP outer input throws a binding error. An inner-container failure is
  an issue at that inner path and does not discard valid outer/other-inner entries.
- Include known unextractable module members as unplayable records with errors, and
  container problems as issues. Deduplicate decoded locator keys by first directory
  entry. Preserve rawHex for members and validate inner-container path lookup against
  its intended central-directory index; report and skip unresolvable containers.
- Add module_metadata JSON export for the already-inspected module, with real format,
  title, channels and newline-joined instrument/sample names in slot order; songMessage
  stays empty. No new core fields or reparsing. Worker instances isolate mutable state.
- Preserve existing upload picker/extraction APIs and all existing legacy ZIP support.
  Use a safe JSON serializer already available or add a narrowly scoped dependency
  only if needed; no hand-built unsafe escaping. Return useful errors when no module
  is inspected, rather than claiming successful empty metadata.

## Research points

Inspect archive find_entry precedence (exact/raw/encoded before normalized fallback),
legacy encodings, module sample accessors, crate features/dependencies and current
Rust fixtures. Confirm size limits before allocation/extraction, not only afterward.

## Verification

Run `cargo test -p starplayer-archive` and `cargo test -p starplayer-web`.
Add synthetic tests for direct/nested/inner-only ZIPs; raw/CP437 names, quotes/newlines;
case variants, duplicate names and ambiguous inner paths; encrypted/unsupported/
oversized entries; corrupt inner archive without losing valid outer results; deeper
nesting diagnostics. Verify metadata across all five module formats: existing REFLEX for S3M and synthetic minimal MOD/MTM/XM/IT
builders in the web Rust tests, with named instruments/samples and empty slots. Reuse
local MOD/MTM builders and adapt the XM/IT loader test-builder patterns; add no binary
fixtures. Also test no prior inspected module. Run `cargo xtask
wasm` to verify bindings; hand D1 the exact JSON field contract and failure behavior.

## Out of scope

Worker JS, app/view wiring, metadata hashing, engine DSP/timing, new song-message
model fields, changed ZIP upload APIs, unrelated Rust formatting, historical assembly.
