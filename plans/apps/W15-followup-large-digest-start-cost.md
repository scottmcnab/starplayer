# W15 follow-up — Profile large playback-digest invocation cost

| Field | Value |
|---|---|
| Status | **Specified follow-up — measurement-triggered profiling; no production policy change authorized** |
| Worker | GPT-6.1-sol, **high** effort |
| Coordinator | GPT-6.1-sol, **xhigh** effort |
| Depends on | Reviewed W15 remediation and its R7 native measurements |
| Ownership | Separate profiling harness/evidence; production changes require a reviewed contract decision |
| Future branch / sibling worktree | `w15-digest-start-cost` / `../starplayer-w15-digest-start-cost` (not created) |

## Context for a fresh agent

Read AGENTS.md, [remediation master](W15-remediation-master-plan.md), its R2/R7 tasks,
and apps/starplayer-web/test/w15-remediation-headless.mjs. Remediation deliberately
hashes playback bytes on the page, calls WebCrypto synchronously before any await,
and publishes metadata/SHA atomically when the accepted context/version is current.
Digests remain independent of worker cancellation/Retry. Do not restore the withdrawn
hash/recovery queues or assume that activation always limits outstanding digest work.

R7 measured a material synchronous invocation cost despite asynchronous digest
settlement. Environment: Chromium 156.0.8078.4, Linux WSL2 x86_64, AMD Ryzen 9 5950X,
32 logical CPUs, approximately 25.4 GiB RAM. Each fixture had three repeats, each
issuing four synchronous controller digest calls, awaiting their settlement, then
measuring one real application activation separately. Inputs were synthetic MODs with
loader-accepted trailing padding. These are controlled fixtures, not natural user
concurrency or measured total browser memory.

| Input | Invocation p50 / p95=max (ms) | Settlement p50 / p95=max (ms) | Activation p50 / p95=max (ms) | Peak outstanding / input-byte estimate |
|---|---|---|---|---|
| Loose 1 MiB | 1.105 / 1.540 | 3.445 / 5.665 | 33.190 / 57.100 | 4 / 4 MiB |
| Loose 32 MiB | 37.710 / 39.085 | 111.345 / 152.560 | 117.810 / 118.285 | 4 / 128 MiB |
| Loose 64 MiB | 76.880 / 81.560 | 230.870 / 312.550 | 206.505 / 217.785 | 4 / 256 MiB |
| Loose 128 MiB | 150.020 / 181.165 | 445.170 / 626.375 | 400.380 / 403.560 | 4 / 512 MiB |
| Extracted 64 MiB (65,416-byte ZIP) | 75.395 / 105.700 | 225.200 / 335.830 | 203.835 / 220.180 | 4 / 256 MiB |

The input-byte sum is a conservative input-snapshot budgeting estimate, not total
RAM or an upper bound on native overhead/copies. Invocation includes controller work
and native WebCrypto entry/snapshot cost; settlement is observed at the JS callback
and includes delivery delay. Twelve digest samples and three activation samples per
size do not establish statistical confidence or a universal real-device budget.
Evidence: /tmp/starplayer-w15-remediation-evidence/r7-digest.log. Normative snapshot
and parallel processing steps: [WebCrypto digest](https://w3c.github.io/webcrypto/#SubtleCrypto-method-digest).

## Deliverables

- Reproduce the invocation cost with the native provider alone and through the
  controller. Profile allocation/input snapshotting, native provider work, GC and
  delivery scheduling using traces and primary implementation evidence for the
  measured browser version. Do not label all invocation time as copying without proof.
- Measure bounded successful library-origin playback sequences as well as direct
  digest requests, at most eight per run, then await cleanup. Actual activation and
  hashing must share the production library path in this follow-up so observed
  natural concurrency is distinguished from R7's deliberately synchronous burst.
- Record peak physical promises, scalar input-byte estimates, start/settlement/
  activation distributions and tracing overhead. Measure native/browser memory only
  where supported; distinguish ownership, heap capacity and process memory. Never
  retain module bytes merely for instrumentation.
- Compare practical options and a platform-specific responsiveness/memory budget,
  preserving borrowed bytes, call-time snapshots, version guards, atomic publication,
  unchanged playback and worker-failure independence. Return a concrete recommendation
  for owner/coordinator review before any production design or policy change.

## Research points

WebCrypto being asynchronous does not make entry cost zero. Determine what the
actual provider does before deciding whether execution location helps. Loose files
may be 128 MiB; 64 MiB is the extracted-entry limit. Do not infer a concurrency bound or
native-memory bound from this fixture's four promises. Consider realistic browsers
and owner devices only with explicit fixture/device access; no owner collection scan.

## Verification

Re-run the native measurement case with explicit browser/flags/versions and source/
package hashes, bounded fixture repeats and complete promise cleanup. Keep all current
controller snapshot/version/atomicity/failure tests, full browser regressions and
engine/RT/no_std checks intact for any later approved implementation. Profile captures
must state their own overhead; do not silently substitute traced timings for baseline.

## Out of scope

Implementing a queue, concurrency cap, recovery FIFO, worker-hash scheduler or weaker
checksum freshness; changing source limits or native formats; deployment/main merge;
cargo fmt, historical sources or attribution. This is a separate profiling follow-up,
not an unpassed W15 remediation acceptance gate.
