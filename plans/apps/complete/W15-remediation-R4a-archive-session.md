# W15 remediation R4a — Bounded Rust archive extraction sessions

| Field | Value |
|---|---|
| Status | **Landed on `main` via `ece531f` (2026-10-27) — reviewed/verified; manual feature acceptance pending** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | Baseline; parallel R1/R2/R3/R5 (pure Rust, disjoint files) |
| Ownership | apps/starplayer-web/src/lib.rs, narrow archive owned-buffer normalization helper, compatibility tests and related Rust tests |
| Historical branch / sibling worktree | `w15-remediation-r4a` / `../starplayer-w15-remediation-r4a` |
| Authorization now | Implementation authorized; worker must not stage, commit or merge |

## Context for a fresh agent

Read AGENTS.md, [remediation master](W15-remediation-master-plan.md), and the
[archived W15 master](W15-master-plan.md) with its linked implementation
evidence. The remediation contracts explicitly supersede only listed archived clauses.
Feature baseline is b3faf9e; execution starts from the integrated planning tip.
Task status and verification are updated by the coordinator.
Do not rely on chat history or temporary review scripts. JS paths are under
`apps/starplayer-web/www/`; tests under `apps/starplayer-web/test/`. During execution,
worker reports diff/results to coordinator, does not commit or edit another task's
files. Preserve engine/RT/no_std invariants.

Finding #4 requires source bytes and nested decoded containers to survive multiple
member requests, without repeated wasm-bindgen copies or nested decompression per
member. Finding #9 is intentional first-raw-duplicate behavior across old/new APIs.

Amendment (2026-10-26): the #1–#3 correctness gate now applies to R4b's integration,
not to this Rust-only task, which can start at the baseline.

## Deliverables

- Implement master D ArchiveInspectionSession constructor/extract_member/free binding.
  Own one outer <=128 MiB snapshot and at most one inner <=64 MiB cache. Resolve with
  current exact/raw/encoded precedence and first duplicate policy; no new locator wire
  fields or archive formats. Replace/free inner cache when container changes.
- Return one copied member Uint8Array, bounded by extraction limits before/during
  decompression. Do not retain previous members; free explicitly releases owned source/
  inner bytes. No global archive state or shared playback session. Session methods
  expose ordinary Rust Result errors as wasm-bindgen JS throws to be caught per member
  by R4b; success returns Uint8Array. Actual WebAssembly.RuntimeError traps remain
  distinguishable for R1 fatal policy.
- Keep existing catalog/extract/upload exports compatible. Add synthetic duplicate
  raw-name tests through legacy classic/ZIP64 listing/extraction as well as new session.
  Document this as intentional first-entry consistency, not new remediation behavior.
- Supply test-only extraction counters or injectable archive adapter sufficient to
  verify same-inner cache reuse, inner switch invalidation and source-memory bounds;
  production must not expose user data or require instrumentation to function.

## Research points

Check current archive helpers and Rust ownership to avoid creating a fresh owned outer
buffer per extract. Decide internal borrowed ZipArchive opening freely, but outward
session API and max one inner cache are fixed. Allocation high-water need not shrink
until worker retirement; record owned buffers versus WASM capacity separately.

## Verification

Run `cargo test -p starplayer-archive -p starplayer-web` and `cargo xtask wasm`.
Synthetic direct/nested/mixed session tests: multiple members same inner extracts it
once; switch containers releases/replaces cache; outer/no member extraction unaffected;
malformed/encrypted/oversized members are isolated. Verify cap checks, free lifecycle,
legacy raw duplicate classic/ZIP64 behavior, exact-name precedence and no behavior
change to uploaded archive extraction. Hand R4b session API and measured allocation/
inner-extraction test hooks. No new binary owner-library fixtures.

## Out of scope

JS worker/controller integration, app/RT changes, archive format expansion, caching
multiple inners or indefinite sources, changing first-duplicate compatibility policy.

## Coordinator ownership clarification (2026-10-27)

A narrow helper in `crates/starplayer-archive/src/lib.rs` may normalize duplicate
directory records in an already owned buffer once at session/inner construction,
avoiding a whole-source copy for every lookup/extraction. Preserve legacy APIs and
first-entry semantics. Record temporary construction allocation separately from
retained outer/inner buffers. No outward session contract change.

## Execution evidence (2026-10-27)

Integrated as `b0cf227` after coordinator diff review. Coordinator independently passed 31 archive tests (one pre-existing manual generator ignored), 22 web Rust tests and cargo xtask wasm. Worker actual Node/WASM ABI smoke passed. Native cfg(test) statistics establish inner-cache reuse, pointer stability, exact source cap and drop release; browser adapter counts are separate evidence.
`git diff --check` passed. Logs: `/tmp/starplayer-w15-remediation-evidence/`.

Final combined Node/Rust/WASM/CI, Chromium and Pages gates passed; see
[R7 final evidence](W15-remediation-R7-integration-evidence.md#final-execution-evidence-2026-10-27).
Native-picker, real-collection and device acceptance remain owner checks.
