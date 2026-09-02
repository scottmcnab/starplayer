# M2-task-C5 — QuirkSet, FormatDialect and TempoModel, wired end to end

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Status | Outstanding |
| Depends on | C3 (MOD), C3b (ProTracker fidelity repairs), C2a (harness repairs) |
| Blocks | M2 exit |
| Parallel with | C6a, C7 |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests per quirk + conformance corpus deltas) |

## Context for a fresh agent

`TempoModel` was declared in M0-A2 with three implementations. `QuirkSet` has been
referenced by every accuracy decision so far but does not exist yet. This task makes both
real, and threads them from the host API down to the effect processors.

Read `plans/product/03-accuracy-policy.md` — §1 lists quirks that are always on, §2 the
ones gated behind `quirks-starplayer`, §3 the defects we never reproduce. This task
implements §2's gate.

The philosophy: quirks are **data, not branches scattered through the code**. A quirk
that cannot be expressed as a `QuirkSet` field probably is not a quirk — it is a format
difference and belongs in that format's processor.

**What M2's conformance work added to this task.** Running libxmp's `test-dev/` corpus
showed that eleven of its cases do not test MOD or S3M as such: they test **other
trackers' dialects of those formats**. libxmp itself selects a behaviour profile from the
file header — for S3M from the `cwtv` field at `src/loaders/s3m_load.c:390-432` — and its
expected dumps then encode Imago Orpheus, ModPlug 1.16 or Scream Tracker 3.01 behaviour
rather than ST3.21's. The owner's decision (2026-09-02) is that these dialects are to be
**supported eventually and captured here as data**, not excluded as foreign formats. So
`QuirkSet` grows a companion: a `FormatDialect` detected at load time from the file
header, stored in the module header, and mapped to a `QuirkSet` by a named constructor.

Until this task lands, those eleven exclusions cite this task file as their tracking
reference.

## Deliverables

1. **`QuirkSet`** — a plain struct of named booleans and small enums, `Copy`, with named
   constructors:
   ```rust
   impl QuirkSet {
       pub fn canonical() -> Self;          // the default
       pub fn starplayer_classic() -> Self; // accuracy policy §2
   }
   ```
   Every field carries a doc comment naming the accuracy-policy entry it implements. A
   field without a policy entry is not allowed.

2. **`FormatDialect`**, detected from the file header by each loader and stored in the
   module header, with a named `QuirkSet` constructor per dialect.

   **S3M**, from the `cwtv` field, mirroring libxmp `src/loaders/s3m_load.c:390-432`:

   | Condition on `cwtv` | Dialect |
   |---|---|
   | `cwtv >> 12 == 1` and `cwtv < 0x1303` | Scream Tracker 3.01 |
   | `cwtv == 0x1320` with the ModPlug marker (zero `special` word and the accompanying flag pattern libxmp tests) | ModPlug Tracker 1.16 |
   | `cwtv >> 12 == 2` | Imago Orpheus |
   | otherwise | Scream Tracker 3.21 (the default) |

   Reproduce libxmp's exact predicates from the pinned source rather than from this table;
   the table is the shape, not the specification. Add a unit test per branch with the
   `cwtv` value that selects it.

   **MOD**, from the 4-byte tag at offset 1080 plus the sample count:

   | Tag | Dialect |
   |---|---|
   | `M.K.`, `M!K!` | ProTracker 1.x/2.x |
   | `M&K!`, `LARD`, `PATT` and the other PT 3.x tags libxmp lists | ProTracker 3.x |
   | `CD61`, `CD81` | Octalyser |
   | `FA04`, `FA06`, `FA08` | Digital Tracker |
   | `FLT4`, `FLT8` | Startrekker |
   | `xCHN`, `xxCH`, `TDZx` | FastTracker and friends |
   | no recognised tag, 15 sample headers | Soundtracker family — **research point 1** |

   The `CD61`, `FA04` and `FA06` tags are currently **rejected at the loader signature
   gate**, which is why their five corpus cases are excluded. Accepting the tag without
   implementing its pattern-loop dialect would turn five honest exclusions into silent
   wrong playback, so accept the tag and set the dialect **in the same change**.

3. **The new `QuirkSet` fields**, each with the policy entry it implements and the corpus
   case it must flip:

   | Field | Policy | Corpus cases it must flip |
   |---|---|---|
   | S3M pattern-loop dialect (`St321` / `St301` / `ModPlug116` / `ImagoOrpheus`) | §2 dialect note | `libxmp-s3m-pattern-loop-imf`, `-imf-breakjump`, `-mpt`, `-mpt-breakjump`, `-st301`, `-st301-breakjump` |
   | MOD Paula clock (PAL / NTSC) | D14 | none directly; it must **not** move any currently passing case under `canonical()` |
   | MOD `F00` stop | §1 (ProTracker) versus MTM's no-op, delivered by C3b | none directly; asserted by unit test |
   | MOD pattern-loop dialect (ProTracker / Octalyser / Digital Tracker) | §2 dialect note | `libxmp-mod-pattern-jump-octalyser-break`, `libxmp-mod-pattern-loop-octalyser`, `-octalyser-breakjump`, `libxmp-mod-pattern-loop-dt`, `-dt-breakjump` |
   | MOD `Dxx` parameter encoding (BCD / hex) | C3 research point 3 | none directly; asserted by unit test |
   | PT sample-swap-at-boundary on/off | D12, once C3b lands | the six D12 cases stay passing with it **on** |
   | PT tremolo-ramp `n_vibratopos` bug on/off | D20 (added by C3b deliverable 10) | none — libxmp does not model it, so only a unit test can see it |

   A field whose corpus column says "none directly" still needs a unit test that observes
   it; a quirk nothing can see is a dead quirk (see the last verification bullet).

4. **Threading.** `QuirkSet` reaches the effect processors from the engine configuration,
   not from a global. It is fixed for the lifetime of a loaded module — changing it
   mid-playback is not supported, and the API should make that obvious. The loader-detected
   `FormatDialect` supplies the **default** `QuirkSet`; an explicit host-supplied
   `QuirkSet` overrides it. Make the precedence explicit in the type, not in prose.

5. **`TempoModel` selection**, likewise, with `ExactFixedPoint` the default and
   `St3Truncating` available. Make the drift difference observable: a test that renders
   the same module under both and asserts the frame counts diverge by the expected amount
   is worth more than a comment.

6. **The `quirks-starplayer` feature flag** gating the classic profile's *code* where it
   would otherwise cost anything at runtime. Where a quirk is a single branch on a
   `QuirkSet` field, no feature gate is needed — prefer the simpler form.

7. **Documentation** in the facade crate's docs explaining what a quirk profile is, what a
   dialect is, when to use each, and pointing at the accuracy policy. Update
   `plans/product/03-accuracy-policy.md` and `conformance/exclusions.tsv` in the same
   commits: every case this task flips loses its exclusion row.

## Research points

1. Whether to support the 15-sample Soundtracker layout as a dialect. It is a different
   header layout, not merely a different tag, so it touches the loader's structure. C3
   deferred the decision; decide it here, implement or explicitly reject, and record it.
2. Whether any quirk needs to vary *per format* rather than per session. If so,
   `QuirkSet` should be per-format-processor rather than per-engine; decide before
   threading it. The dialect fields above are per-module by construction, which is
   evidence for the per-format shape.
3. Whether `ItModern`'s tempo semantics (tempo slides) can be stubbed usefully now or
   should stay a `todo` until M6. Prefer an honest stub that returns the exact model's
   answer and is documented as incomplete.
4. Whether the Octalyser and Digital Tracker pattern-loop dialects differ from ProTracker
   only in the break/jump interaction, or also in the loop counter itself. Derive it from
   the five corpus cases' dumps, not from memory.

## Verification

- `QuirkSet::canonical()` and `::starplayer_classic()` differ in exactly the fields the
  accuracy policy §2 lists — assert field by field, so adding a quirk without updating
  the policy fails the test.
- **Each dialect field flips exactly the corpus cases that name it in deliverable 3's
  table**, and no others. Run the corpus before and after enabling each field and diff the
  case list; a field that moves an unlisted case is either mis-scoped or has found a bug
  that belongs to C3b or C9.
- **Under `canonical()`, every other conformance result is unchanged** from before this
  task. This is the invariant that keeps dialect support from becoming a licence to change
  default behaviour.
- The S3M `cwtv` detection has one test per branch of libxmp's predicate, using the
  `cwtv` value that selects it.
- The MOD tag table has one loading test per accepted tag, asserting channel count and
  dialect.
- Rendering a module under both tempo models produces the expected frame-count divergence
  over a long render.
- Every `QuirkSet` field is referenced somewhere in the codebase and observed by at least
  one test (a dead quirk is a bug).
- `cargo test --workspace`, `cargo xtask ci --job clippy`, `--job no-std-check`,
  `cargo xtask goldens --check`.

## Out of scope

Any new quirk discovered during M2 that is not yet in the accuracy policy — add it to the
policy first, then here. The ST3.21 pattern-loop bugs themselves (C9 owns those; this task
adds the *dialect selection* around a correct ST3.21 baseline, and depends on C9 having
made that baseline correct for the two `st321` cases). ProTracker fidelity repairs (C3b).
XM and IT dialects — they arrive with their formats.

## A note for C7

C7's fuzz seed corpus should include the loader cases this milestone found: `M!K!`,
garbage order tails containing bytes at or above 0x80, a zero song length, 4-byte sample
loops, and one file per dialect tag accepted above.
