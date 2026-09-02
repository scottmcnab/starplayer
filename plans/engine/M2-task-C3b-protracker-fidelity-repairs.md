# M2-task-C3b — ProTracker fidelity repairs, MTM corrections, and the voice-boundary sample swap

| Field | Value |
|---|---|
| Milestone | M2 ([master plan](M2-master-plan.md)) |
| Status | Outstanding |
| Depends on | C3 (MOD), C4 (MTM) |
| Blocks | C5, M2 exit |
| Parallel with | C9, C6a |
| Recommended model | GPT-5.6-sol |
| Verified by | agent (unit tests per fix + conformance re-run) |

## Context for a fresh agent

`starplayer-mod` holds the MOD loader and its ProTracker effect processor;
`starplayer-mtm` reuses that processor under `EffectSemantics::MultiTracker`. Both landed
in M2 (C3, C4) and are careful, well-researched code. A branch review nonetheless found a
set of genuine fidelity defects against ProTracker 2.3D replayer semantics — verified
against the PT replayer sources and against the pinned libxmp — that the conformance
corpus could not surface, because at review time every one of the 27 MOD cases was
excluded for an unrelated reason.

**The reference is ProTracker 2.3D**, not the original DOS assembly and not libxmp. Where
libxmp differs from PT (it does, in the delayed-note phase path, and it does not model
PT's tremolo ramp bug at all), PT governs and the disagreement is documented, not fixed
towards the oracle. For MTM the reference is libxmp's `fx_s3m_*` handlers and the format's
own semantics, because MTM is not a ProTracker dialect even though it shares this
processor.

Each numbered deliverable below states the current wrong behaviour with a file and line,
the reference behaviour, and the unit test that proves the fix. **Write the named test for
every fix**; a green existing suite is not evidence, because the existing suite passed
while all of these defects were present.

Deliverable 11 is separable and larger than the rest; it may be split into an `M2-task-C3c`
if you prefer, but it must not be dropped.

## Deliverables

1. **Vibrato and tremolo must not round toward negative infinity on the negative half.**
   `crates/starplayer-mod/src/processor.rs:608` computes
   `(waveform as i32 * depth as i32) >> 7` and `:622` the same with `>> 6`, on a **signed**
   product. An arithmetic right shift of a negative value rounds toward minus infinity, so
   the negative half of every LFO is one unit deeper than the positive half. ProTracker
   computes `mulu`/`lsr` on the **unsigned magnitude** and then adds or subtracts according
   to the sign of `n_vibratopos`; libxmp uses ordinary division. Example: depth 15,
   phase 132 gives PT a delta of 2 (period 428 becomes 426) and StarPlayer -3 (425).

   Fix: take the magnitude with `waveform.unsigned_abs()`, shift, then negate — or divide
   by 128 and 64 respectively.
   *Test:* vibrato with phase 132 and depth 15 on period 428 yields **426**.

2. **Vibrato writes its result straight to Paula; do not clamp it.**
   `processor.rs:611` runs the vibrato result through `clamp_period(self.amiga_limits, …)`.
   PT's `mt_Vibrato3` writes `n_period +/- delta` directly to the hardware period, and
   libxmp adds vibrato **after** its MODRNG clamp. Under Amiga limits, a `4FF` on C-1
   (period 856) currently loses its whole downward excursion.

   Fix: remove the clamp from `vibrato()`; keep it in the slides and in tone portamento,
   which is where PT applies it. Apply the hardware floor at step derivation instead
   (deliverable 3).
   *Test:* `4FF` on period 856 with Amiga limits on reaches period **885** at the bottom of
   the excursion.

3. **Add the Paula hardware period floor and the period-0 rule.**
   `processor.rs:849-850` returns `period.max(1)` when Amiga limits are off, and
   `:879-881` (`step_from_period`) maps period 0 to `Step::ZERO`, which holds the voice at
   its last sample value as DC. Real hardware — as modelled by pt2-clone's
   `paulaSetPeriod` — clamps any period below 113 up to 113 and treats a written 0 as
   65536. Extended-range MODs sliding below 113 currently produce a runaway step, and the
   D16 arpeggio overflow produces a DC hold where the hardware produces a near-silent
   54 Hz crawl.

   Fix: in `step_from_period`, map `0` to `65536` and `1..113` to `113`.
   *Test:* period 1 derives the same `Step` as period 113; period 0 derives the step for
   period 65536, not `Step::ZERO`.

4. **The sample loop gate is off by one word.**
   `crates/starplayer-mod/src/loader.rs:117` enables looping only when
   `loop_length > 4` bytes. ProTracker loops whenever `n_replen > 1` word, that is
   **`loop_length >= 4` bytes**; libxmp uses `loop_size > 1` at `mod_load.c:656`. A 2-word
   (4-byte) loop is a real, audible loop in PT and a one-shot here. The `> 4` rule came
   from the original DOS converter, and the C3 task file repeated it as "a universal MOD
   convention" — it is not.

   Fix: `loop_length >= 4`. Correct the C3 task file's deliverable 2 wording and the format
   notes in the same commit.
   *Test:* a sample with a 4-byte loop length loads with `LoopMode::Forward` and its voice
   wraps rather than ending.

5. **The pattern-count scan must ignore garbage order entries at or above 0x80.**
   `loader.rs:67-68` filters only the value 255. ProTracker's `mt_init` uses a **signed**
   byte compare (`cmp.b` / `bgt`), so any entry with the high bit set never raises the
   maximum; libxmp breaks at the first byte above 0x7f (the "dragnet.mod" fix). A file with
   0x80 in its unused order tail is currently rejected as truncated, or loads with every
   sample offset shifted. `plans/reference/format-notes-mod.md:30` misattributes the
   current rule to ProTracker.

   Fix: `.filter(|order| *order < 0x80)`. Keep the existing `FLT8` stored-pair accounting.
   *Test:* a module whose order tail contains 0x80 loads, and its pattern count and first
   sample bytes match the same module with 0x00 in that slot.

6. **Accept the `M!K!` and single-digit `?CHN` tags.**
   `loader.rs:155-167` accepts only the DOS original's tag set. `M!K!` is what ProTracker
   itself writes for a module with more than 64 patterns and is very common; libxmp lists
   it first. Single-digit `[1-9]CHN` is likewise standard.

   Fix: add `M!K!` as 4 channels and `[1-9]CHN` alongside the existing `[0-9][0-9]CH`
   form. List the complete accepted tag set in `plans/reference/format-notes-mod.md`. Do
   **not** add `CD61`, `FA04`, `FA06` here — those dialects are C5's, and adding the tag
   without the dialect behaviour would turn five documented exclusions into silent wrong
   playback.
   *Test:* an `M!K!` module loads with 4 channels; a `5CHN` module loads with 5.

7. **`pattern_loop_start` must persist across a pattern change.**
   `processor.rs:761-763` clears every channel's `pattern_loop_start` whenever the pattern
   number changes. PT's `n_pattpos` is per channel and **persists**; only `E60` sets it. A
   pattern whose first `E6x` has no preceding `E60` loops back to the previous pattern's
   mark in PT and to row 0 here.

   Fix: remove the reset. If you conclude PT's behaviour is undesirable, that is a
   deviation and needs an accuracy-policy entry — not a silent difference.
   *Test:* `E60` on pattern A row 8, then `E61` on pattern B row 4, loops to the mark, and
   the resulting row sequence is asserted.

8. **Reset processor state on seek.**
   `TrackerProcessor` (`crates/starplayer-engine/src/sequencer.rs:432`) has no reset hook,
   and `PatternSequencer::seek_order` (`:604`) and `seek_row` (`:613`) move only the
   cursor. After a seek, `pending_tempo` (the deferred CIA BPM), the LFO random state,
   every effect memory and every pattern-loop counter survive — so a seeked render differs
   from a fresh render of the same order, and a stale `Fxx` can fire on the first tick
   after the seek. This applies to S3M as well as MOD and MTM.

   Fix: add `fn reset(&mut self)` to `TrackerProcessor` (a provided empty default is
   acceptable only if every current implementation overrides it — prefer a required
   method so a future format cannot forget). Call it from both seek entry points. Reseed
   the xorshift deterministically, clear pending tempo, effect memories and loop counters.
   C3a's panning toggle builds a fresh processor and is unaffected.
   *Test:* render order 3 from a fresh engine; separately load, seek to order 3, render;
   the two renders are byte-identical.

9. **Small MOD items.** Fix the first two; document the third.
   - `processor.rs:326-330` (`8xx`) and `:410-414` (`E8x`) map pan through `pan_byte()` in
     a way that cannot reach hard right or exact centre. Cosmetic in audio, visible in
     every trace and therefore in every conformance comparison.
   - `loader.rs:62` rejects `song_length == 0`; libxmp loads such a module and plays
     nothing. Load it and produce silence.
   - `reset_row` restores the un-modulated period and volume one tick earlier than PT's
     `mt_PerNop` path on a row that carries a command but no note. **Document only** — add
     it to `plans/product/03-accuracy-policy.md` §3 as a deviation with this description.

10. **PT's tremolo ramp-waveform bug (document or implement).**
    `mt_Tremolo2` tests `n_vibratopos` — not `n_tremolopos` — when choosing which half of
    the ramp waveform to use, so with `E71` the *shape* comes from the vibrato phase while
    the *sign* comes from the tremolo phase. libxmp does not model this, so the oracle
    cannot see it either way.

    Decide and record: implement it behind the quirk that C5 will own, or add it to
    `plans/product/03-accuracy-policy.md` §3 as a documented deviation. **Document-only is
    an acceptable outcome for this item**; a silent omission is not.

11. **MTM corrections** (`EffectSemantics::MultiTracker`).
    1. **`F00` must not stop the song.** `crates/starplayer-mod/src/processor.rs:356-359`
       matches `param == 0` **before** the semantics check, so MTM inherits ProTracker's
       "F00 means stop". libxmp's `fx_s3m_speed` ignores `F00`, and
       `plans/reference/format-notes-mtm.md` does not claim this difference. Gate the stop
       arm on `EffectSemantics::ProTracker`.
       *Test:* MTM `F00` is a no-op; MOD `F00` still stops.
    2. **`E8x` and `8xx` must use the loader's pan curve.** `processor.rs:326-330` and
       `:410-411` map through `pan_byte(nibble << 4)` on a 0..255 domain, while
       `crates/starplayer-mtm/src/loader.rs:219` maps the header nibble on 0..15. `E8F`
       therefore gives 240 where the header's 15 gives 255, and `E88` gives 128 where the
       header's 8 gives 136. libxmp is self-consistent (`fxp << 4` in both directions) and
       the conformance adapter projects with the loader's curve, so any MTM case carrying
       `E8x` would mismatch. Under `MultiTracker` semantics, route `E8x` through the
       loader's nibble mapping.
       *Test:* MTM `E8F` produces the same pan value as header pan 15.
    3. **The MTM loader is stricter than every reference.**
       `crates/starplayer-mtm/src/loader.rs:138` returns `Err(OutOfRange)` for an order
       byte at or above `pattern_count`, `:49` rejects `last_order >= 128`, and `:53`
       rejects a non-zero attribute byte. libxmp and the original DOS player tolerate all
       three. Clamp or skip instead of erroring, consistent with the loader's existing
       out-of-range track recovery.
       *Test:* one module per case loads and plays rather than erroring.

12. **The mixer voice-boundary sample swap (D12).** This is the largest single accuracy
    gap in MOD playback and the reason six corpus cases fail.

    In ProTracker, an instrument number **without** a note on a channel whose sample is
    looping, and a tone-portamento row that names a different sample, do **not** restart
    the voice: the new sample takes effect when the currently playing sample reaches its
    loop point (or, for a one-shot, its end). This idiom is everywhere in real MODs —
    timbre changes over a sustained loop. The accuracy policy currently records it as a
    deviation "until an RT-safe boundary event exists". The event is cheap and this task
    builds it.

    Design:
    - Add **one `Option<SampleRegion>` pending-swap slot per voice** in the mixer voice
      state. No allocation, no lock, no branch in the inner accumulation loop beyond the
      wrap/end path that already exists.
    - The voice kernel applies the pending region **at the boundary it already detects**:
      when a looping voice wraps past `loop_end`, and when a one-shot voice reaches its
      end. On application the region is replaced and the slot cleared; the position is
      rebased into the new region per PT (a wrap restarts at the new region's loop start; a
      one-shot end starts the new region at its beginning).
    - The MOD processor sets the slot for instrument-without-note on a looping voice and
      for a tone-portamento sample change, instead of its current behaviour. MTM uses it
      per its own profile — decide from `format-notes-mtm.md` and libxmp whether MTM shares
      PT's rule, and state the decision in the task's closing note.
    - Keep the swap observable in the C1 trace so the conformance harness can compare it.

    Targets — these six cases must be re-evaluated and reported after the change:
    `openmpt-mod-portamento-sample-change-pt`, `openmpt-mod-portamento-swap-pt`,
    `openmpt-mod-instrument-swap`, `openmpt-mod-stopped-swap`, `openmpt-mod-swap-empty`,
    `openmpt-mod-swap-no-loop`. Note that `openmpt-mod-portamento-sample-change`
    (without the `-pt` suffix) requests the **non**-ProTracker sample-change profile and
    stays a deviation (D17) — it is not a target.

    When this lands, D12 moves from "accepted deviation" to "implemented" in
    `plans/product/03-accuracy-policy.md`, and the on/off quirk becomes a C5 `QuirkSet`
    field.

## Research points

1. Whether `reset()` on `TrackerProcessor` should be required or provided-with-default.
   Prefer required; a format that genuinely has nothing to reset writes an empty body and
   says so in a comment.
2. Whether MTM shares ProTracker's sample-swap-at-boundary rule or restarts the voice.
   Check `plans/reference/format-notes-mtm.md`, libxmp's `mtm_load.c` plus its shared
   `virt_setpatch` path, and the original DOS `ConvertMTM`. Record the answer either way.
3. Whether removing the vibrato clamp (deliverable 2) changes any currently passing S3M or
   MTM result. It must not — the clamp stays in the slide paths — but confirm with the
   corpus rather than by reading.
4. Whether the pending-swap slot can reuse an existing voice field rather than growing
   `Voice`. Size matters on the embedded target (M8).

## Verification

Unit tests, one per fix, with these exact expectations:

- Vibrato phase 132, depth 15, period 428 gives **426**.
- `4FF` on period 856 under Amiga limits reaches **885**.
- Period 1 derives the step for period 113; period 0 derives the step for **65536**.
- A sample with a **4-byte** loop length loops.
- An order tail containing **0x80** is ignored by the pattern-count scan.
- An **`M!K!`** module loads (4 channels); a `5CHN` module loads (5 channels).
- A cross-pattern **`E6x`** loops to the previous pattern's `E60` mark.
- **Seek then render equals a fresh render** of the same order, byte for byte.
- MTM **`F00` is a no-op**; MOD `F00` still stops.
- MTM **`E8F` equals header pan 15**.

Then:

- `cargo test --workspace`, `cargo xtask ci --job clippy`, `--job no-std-check`,
  `--job wasm-build`.
- `cargo xtask conformance` re-run; report the new per-format counts and hand any case
  that still fails to the task that owns it, with the first divergence quoted.
- `cargo xtask goldens --check` still passes — none of these fixes touches the S3M path.
  If a golden moves, stop and explain why before regenerating it.
- The buffer-size-independence test (block sizes 1, 3, 64, 128, 4096, 8191) still holds
  after the voice-boundary swap; this is the invariant most at risk from deliverable 12.
- No allocation inside `render()` — assert it for the new pending-swap path specifically.

## Out of scope

Tracker dialects and their quirk flags (C5): `CD61`, `FA04`/`FA06`, `FLT8` beyond what
already loads, the PAL/NTSC clock choice, `Dxx` BCD versus hex. Harness repairs (C2a) —
if a case fails because of the comparator, say so and leave it. S3M effect bugs (C9). The
15-sample Soundtracker layout (C5 research point). `EF` invert loop.
