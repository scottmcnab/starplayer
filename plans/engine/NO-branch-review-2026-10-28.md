# NO — Independent full-branch implementation reviews

| Field | Value |
|---|---|
| Reviewed feature head | `55479442f198f16be0492e8da9821f35a02762ec` |
| Main / merge base | `d0cdcc53dd320675edd64258aacf247f8cc57f0c` |
| Scope | All 120 changed paths against main, not only NO-07; current master/tasks authoritative |
| Status | Both independent full reviews and final corrective reviews complete; all eight accepted corrections integrated on feature/note-overlap-audition from task 20dbc07; automated verification passed; owner/device acceptance pending |
| Opus reviewer | Claude CLI `claude-opus-5-5`, `--effort high` supplied; exact returned model confirmed |
| Astra reviewer | `gpt-6-astra`, medium reasoning explicitly requested and accepted by agent orchestration |
| Coordinator | `gpt-6.1-sol`, xhigh |
| Remediation | [NO-08](NO-task-08-branch-review-fixes.md), isolated `task/no-08-review-fixes` |

## Method and evidence boundaries

The owner requested both independent full-branch reviews and resolution of findings the
coordinator concurs are worth fixing. Reviewers ran sequentially under the one-agent OOM
constraint. Both initial passes inspected the same frozen source tree and did not receive
the other's report. No branch source changed during review. Reviews were source-only:
no new passing/failing tests, browser, compilation or hardware evidence is implied.

Opus ran with only Read/Glob/Grep tools, exact `claude-opus-5-5`, high effort supplied,
110 turns, exit 0 and no permission denials. The model itself could not introspect effort.
Its raw report's statement about no tools is retained verbatim; CLI evidence confirms
read-only tools were used, with no shell/build/edit tools. The supplemental pass fills
its expressly declared first-pass coverage gaps without receiving Astra's findings.
Astra used a fresh conversation with the complete review assignment, requested model and
medium effort; it read neither Opus's report nor the coordinator's draft triage. Model/
effort assignment is distinguished from the reviewer's lack of runtime introspection.

Full prompts, per-file frozen diffs and raw CLI artifacts are in
`/tmp/no-branch-review-5547944`. The reports below are verbatim. Existing NO-07 verification
remains evidence for its tested source; it is not proof that these review findings have
been experimentally reproduced or fixed.

## Coordinator dispositions

| Finding | Disposition | Required correction / evidence |
|---|---|---|
| Opus NO-R1 (low) | Accept | Minimum one-frame Q32.32 cadence; valid fractional timing unchanged; direct/configured progress regressions |
| Opus NO-R2 (low) | Accept confirmed snapshot loss | Coalesce command transition publishes after bounded drain; final live/state/cursor snapshot; no generic ring or resource-budget change |
| Astra ASTRA-01 (P2) | Accept | Poll feed lookahead before segment scheduling, including initial and idle admissions; prove exact dispatch frames and no late events at all six sizes |
| Astra ASTRA-02 (P2) | Accept | First audio attachment rebases sequenced origin/deadline once; retain stopped/paused/playing cursor and fractional remainder; SMF and delayed-load pressure regressions |
| Astra ASTRA-03 (P2) | Accept | IT interactive-clear marker prevents instrument-only synthetic resurrection; native completion/restart and effect memories preserved |
| Opus NO-I1 / Astra ASTRA-04 (info/P3) | Accept bounded protocol hardening | Shared/fallback malformed members reject whole group with explicit identity-bearing ack, no partial valid member; retry and v1/63-event throughput regressions |
| Opus NO-R4 (medium) | Accept | Play/Resume honors natural-end armed rewind after Pause; no repeat end/playlist event; explicit user Stop still cancels rewind |
| Opus NO-R5 (low) | Accept for interactive contract | Musically invalid seeks preserve voices and DSP tails; valid seek clears only after authoritative validation; legacy API behavior remains scoped |
| Opus atomic-group design-risk observation | Retain reviewed design | Group atomicity and safe rejection cleanup are intentional; no independent contract defect demonstrated |

The eight accepted corrections implement existing timing/transport/telemetry/wire contracts;
no product switch, duplicate identity, native NNA policy or quota changes are approved.
Opus NO-R2's replacement-cursor extension remains an unproven hypothesis, not a reproduced
bug. Some raw Opus line numbers are patch offsets: actual frozen-source command locations
are Engine::drain_commands at 878, apply_interactive at 1012/1052 and publish_interactive
at 1070. Coordinator source inspection confirms the accepted mechanisms; the focused
failing regressions and final evidence will be recorded in NO-08.

A small independent Node/VM reproduction confirmed silent filtering before fixes:
shared consumed two records but forwarded only the valid one; fallback also forwarded
only the valid member. `/tmp/no-branch-review-5547944/malformed-before.json`. This proves
the drain mechanism; actual worklet/WASM atomicity regressions are still required.

### ASTRA-03 initial-state clarification — coordinator follow-up

The sound-clear suppression must distinguish actual previously accepted starts from
fresh setup. Initial Stopped has no cleared voice to resurrect, so IT's native first
instrument-only C0 onset remains canonical. Default C0 or a latched instrument alone is
not accepted-start history. NO-08 adds a failing fresh configured-load regression and
tracks never-started/remembered/cleared states within the audition-only marker. Existing
prior-note clear suppression, genuine-note recovery and natural-completion controls stay
strict. MOD/XM receive no speculative change without an actual reproduced initial-start
regression. The worker's first11-job CI pass precedes this clarification; final corrected
production must pass whole CI again. This is the existing faithful-native contract,
without a new product switch or semantics change.

### ASTRA-03 note-only memory clarification — coordinator follow-up

A missing instrument on a genuine post-clear note is remembered mapping, not an explicit
instrument change. Resolving it early as a raw instrument column replays sample default
volume and IFC/IFR, contradicting preserved effect memories. NO-08 must reproduce volume,
filter and onset-identity loss, then resolve mapping at note_change without replaying
instrument defaults. Prior CI/matrix passes are retained for their prior source; whole
CI and affected checks must pass again on the final correction.

## Opus initial report (verbatim)

# StarPlayer `feature/note-overlap-audition` — independent code review

**Reviewer:** Claude Opus 5.5 (`claude-opus-5-5`). I can't see my effort setting from here.
**Head reviewed:** `55479442`, compared against merge base `d0cdcc53`. I only read files and diffs; I made no edits and ran no tests, builds or tools.

## Verdict

I found **no confirmed high- or medium-severity correctness defects**. There are **two low-severity defects** I'm confident in, **one informational hardening item**, and **one design-risk observation**. These are listed below.

---

## Actionable findings

### NO-R1 — Low (confirmed in code): the articulation cadence has no minimum interval, so the "always advances" guarantee is lost

- **Where:**
  - `crates/starplayer-engine/src/control.rs:99-111` (`tick_synthesised`, cadence branch) and `:130-137` (`set_cadence`).
  - `crates/starplayer-engine/src/engine.rs:152`, `:414-418`, `:562-566`, `:590-594`.
- **Trigger:** a host sets `EngineSettings.articulation_interval_micros` to 0, or to anything shorter than one output frame (below about 22 µs at 44.1 kHz). This needs an interactive engine, or a source that uses the cadence while Paused, Stopped or with no tracker.
- **What goes wrong:**
  - The interval is `(rate·µs << 32) / 1e6` and is never validated.
  - In the cadence branch, `next_frame += (interval + remainder) >> 32`, which can be 0.
  - So `dispatch_due_events` keeps finding the control tick due. Each loop pass bumps `ticks`, runs `post_source_articulation` (an extra live XM/IT envelope/fadeout step) and `publish_interactive`. This repeats until `MAX_ZERO_ADVANCE` and sets `zero_advance_forced`.
  - Result: envelopes run far too fast, and CPU cost spikes on every quantum.
- **Evidence:**
  - The doc comment at `control.rs:99-100` still promises "Always moves … forward by at least one frame".
  - The legacy path keeps that promise: `interval_frames_for` clamps to at least 1 (`control.rs:74-77`).
  - The master plan expects a fixed configured cadence (default 50 Hz).
- **Why tests miss it:** every test uses the default 20 000 µs or a tracker-derived interval.
- **Fix:**
  - Reject 0 / sub-frame intervals in `try_with_settings` (a new `PlaybackConfigError` variant), or clamp the interval to at least `1 << 32` in `set_cadence`.
  - Correct the doc comment.
- **Regression check:** a unit test calling `set_cadence(.., 0, 0, true)` then `tick_synthesised` should show `next_frame` advances by at least 1. An engine test with an invalid interval should get `Err` back.

### NO-R2 — Low (mechanism confirmed; visible impact bounded): a burst of interactive commands can drop the newest telemetry snapshot

- **Where:**
  - `crates/starplayer-engine/src/engine.rs:444-445` (`apply_interactive` publishes after every command).
  - The ring keeps the oldest snapshots and drops new ones when full: `crates/starplayer-telemetry/src/publisher.rs:244-255`, with its own test at `:417-429` ("the ring kept the two oldest").
  - Depth is 3: `crates/starplayer-host/src/player.rs:77`.
- **Trigger:** four or more publishes inside one engine render quantum. For example, a v2 group of three or more interactive commands drained in one engine pass, plus a tracker/cadence publish in the same quantum. The host only reads once per quantum (`player.rs:422`, `:521`).
- **What goes wrong:**
  - The post-transition snapshot is dropped and the reader gets an intermediate one.
  - Stale fields: live summary (active/retiring), channel activity, and the sequencer cursor.
  - Transport state and `natural_end` are not affected, because the host overrides them (`player.rs:522-525`).
  - The staleness lasts until the next cadence publish, at most about one tick.
- **Uncertain extension:** `replacement_state` (`crates/starplayer-host/src/replacement.rs:12-20`) takes its cursor from this snapshot. A capture inside that window after Pause could carry a cursor from before the Pause, putting the restored deadline off by up to one tick. Reaching this needs tight timing; I haven't shown it happening.
- **Contract:** the master plan's "Host transport, cleanup and telemetry" section says to publish "immediately on clears/state transitions at the next publication boundary".
- **Why tests miss it:** they send one command per quantum, or read after several more quanta have rendered.
- **Fix:** in `apply_interactive`, set a dirty flag instead of publishing, then publish once after `drain_commands` (or once per quantum).
- **Regression check:** in one quantum, apply 4× `SetLetNotesRing` + `StopPreview` while a preview is sounding. The snapshot read right after that quantum should show the final live summary.

### NO-I1 — Info / hardening: the v2 ring drain silently skips invalid records

- **Where:** `apps/starplayer-web/www/ring.js:150-152`.
- **What happens:** a record that fails `validInteractiveArguments`, or has opcode 0 or above 24, is skipped and the read index still advances. `controlFailed` is not set, so the group's ack can come back `accepted`.
- **Reachability:** `pushCommand` already validates (`ring.js:120`), so only a malformed or foreign producer hits this.
- **Fix:** have the drain call `apply` with a sentinel (or set the failure flag) so the whole group is rejected.

---

## Design-risk observation (not a contract defect)

**Unrelated UI actions share one all-or-nothing group per audio quantum.**
- The worklet stages everything drained in a quantum, up to 63 records, as one group (`worklet-processor.js:61-71`, `:131-147`). `drain_interactive_batch` rejects the whole group if any member fails (`crates/starplayer-host-wasm/src/lib.rs:110-140`).
- On rejection, `app.js:1054-1057` sends `STOP_PREVIEW`, which stops held live notes, and reverts the checkbox UI.
- Because the ring is first-in-first-out, stale records come before newer ones, which limits the blast radius. But a headroom refusal (`can_admit_browser_batch`) can still take a current note-off or toggle down with it. The code comments say this is intentional; it's worth noting for owner acceptance.

---

## Checked and found consistent with the current contract

**Shared overlap and ownership**
- `overlap.rs`, `ownership.rs` and `channel.rs`: eligibility is decided by the actual sounding sample; the outgoing voice decides survival; matching uses the full identity (instrument, note, onset volume, channel); the incoming voice is skipped; retiring voices are excluded.
- Domain quotas and physical caps: retiring voices are hard-released only within their own domain. The 256+64+16 active / 339 physical arithmetic holds, and a victim is passed only when the domain is at its limit.

**Legacy formats (MOD/S3M/MTM)**
- Onset is captured at flush after that tick's effects, using `current_volume`, including delayed starts.
- Retriggers and sample-only changes inherit the existing identity rather than starting a new note.
- A pending ProTracker sample swap is discarded on custom detach, with the voice tag following the swap boundary.
- The `VoiceTag` widening is 1:1 for MOD, because the loader maps each instrument to the sample of the same index.

**XM**
- The articulation was moved into `XmVoiceState` without changing it.
- On the detaching tick the outgoing tail is stepped exactly once, from its previous-tick snapshot, after its channel's flush.
- `real_volume` capture includes `EDx` delayed notes.

**IT**
- Native duplicate check (DCT) → custom preserve → native new-note action (NNA) ordering is correct.
- DCT is skipped only for currently eligible custom candidates.
- `Oxx` runs after the trigger and resets offset to 0, so both `musical_start` checks agree.
- `pending_onset` is finalized after that channel's effects using `note_volume`.
- Victim selection, quota and background-control commands are limited to native voices that aren't retiring.
- Articulation and target arrays are sized to the actual capacity (339).

**Live**
- Identity is resolved before CC7 using `raw_velocity`.
- Held instances are tracked per voice, so key-up after detach works; pedal, program change and CC120 are lane-scoped.
- L admission is applied twice, but the arithmetic stays bounded.
- The XM and IT live runtimes match the tracker's DCT/NNA order.

**Clocks and transport**
- Pause/Stop move the deadline and song origin by the held duration; seeking while held moves the hold point.
- Control-only steps are skipped while Stopped; no double articulation on resume.
- SMF held-time translation and cursor restore are correct.
- Fade and natural-end are frozen during Pause; Stop cancels pending rewind; Resume does not take the legacy Play-during-fade restart branch.

**Real-time safety, features, wire format, replacement**
- Retirement reservations prevent frees on the audio thread for Load and RestoreSelections; there are no new allocations or locks in render.
- Cargo feature gating and forwarding look right.
- v2 SAB record publication is ordered correctly through Atomics.
- Replacement codec: 47 + N words, with matching encode/decode indices and validation.

---

## Coverage limits

- **Plans:** I read `AGENTS.md` and the master plan in full. I only searched the `NO-task-*` files and the W1 task, and didn't read the product foundation documents.
- **Host and web:** for `player.rs` I covered the render/apply/load/interactive/validation paths, not all 42 hunks. `app.js`, `index.html`, `style.css` and `media-session.js` were spot-checked only, and I didn't read the headless/Node tests or `xtask/no07-*`.
- **Tests:** I scanned for weakened assertions and found only generalizations (capacity-parameterized checks and heap formulas). I didn't audit the tests in depth, so fixture correctness and golden fidelity rest on the stated evidence plus my scan.
- **Not looked at:** documented pre-existing S3M fast-slide and IT note-only mapping issues; there was no branch evidence for them.


## Astra independent report (verbatim)

# Independent source review — note overlap and audition

Reviewed head: `55479442f198f16be0492e8da9821f35a02762ec`.
Comparison base: `d0cdcc53dd320675edd64258aacf247f8cc57f0c`.
Worktree: `/home/scott/projects/starplayer-note-overlap-audition`.

Requested reviewer configuration: GPT-6-astra, medium reasoning. I have no independent runtime model/effort introspection; this identifies the assignment, not an inferred runtime setting.

**Result: three substantive correctness findings and one lower-priority wire-validation finding.** These are source-derived findings; I did not execute tests, builds, browsers, formatters, or hardware actions. Repository status was clean at the frozen head when checked. No repository files were edited and no other new review report was read.

## ASTRA-01 — P2: prepared external input misses its first scheduled event boundary

**Location:** `crates/starplayer-engine/src/instrument.rs:1382–1391` (especially the new prepared-rack early return at 1384). Supporting paths: `ExternalEventQueue` initialization/refresh at 1132–1188, `EventSource::dispatch_interactive` in `engine/src/source.rs:204–207`, and `engine/src/engine.rs:758–775,814–820`.

**Trigger and wrong behavior:** Prequeue a NoteOn at frame 17 into an `external_event_channel`, construct a prepared live rack and `MidiSource`, install it in an interactive engine, and render from frame zero. The queue starts with `peeked: None`. Prepared `MidiSource::next_event_frame()` now reports only `feed.next_frame()`, so the engine cannot see frame 17. The control boundary at frame zero does not refresh it: the default interactive dispatch calls `dispatch` only when `next_event_frame()` is due. The first refresh occurs in `advance_to` *after* the first segment was mixed, normally at frame 128. The already-queued event is consequently dispatched at 128 and counted late. A NoteOff at 18 can be dispatched with the NoteOn at 128, collapsing the intended audible interval entirely for a simple sample.

This is a regression from the legacy source's initial housekeeping deadline at frame zero, which caused `dispatch`/`pop_due` to prime the feed before mixing. It violates the sample-exact timestamp contract even though output remains buffer-size independent. It also applies to an explicitly prepared song rack driven by an external feed. The same visibility problem recurs after an idle queue: if a producer admits an event before a later render quantum, due within that quantum, the previous end-of-segment refresh has already happened and the next refresh can again occur after its deadline. Normal web input stamped with a two-quantum lead masks this startup case; the reusable source API and prequeued schedules do not.

**Why tests miss it:** `crates/starplayer/tests/live_audition.rs:143–174` already queues the relevant frame-17/18 pair, but checks only silence before frame 17 and *some* audio anywhere in frames 17–255. A frame-128 start passes that assertion. The six-size comparison reproduces the same lateness at every size. The engine transport test's scripted `Live` source exposes its deadlines directly and therefore does not exercise an unprimed `ExternalEventQueue`.

**Bounded fix:** Refresh feeds at the current source frame before calculating the first event boundary of each quantum, including initial installation and a previously idle queue. A one-time constructor prime alone does not cover subsequent producer admissions. Keep this polling separate from articulation; do not restore the old per-MIDI-source articulation cadence.

**Regression verification:** Assert the exact dispatch frame and zero late-event count for prequeued events at 0, 17 and 18; use a nonzero simple sample with a short held interval to prove frames 17–18 are actually rendered. Repeat for the six required block sizes and for installation at a nonzero source frame. Also render an idle quantum, enqueue a future event inside the next quantum before rendering it, and verify its exact dispatch frame.

**Confidence:** High; the entire initialization-to-dispatch path is visible in source.

## ASTRA-02 — P2: deferred installation ages a stopped source's cursor and consumes its saved tick remainder

**Location:** `crates/starplayer-host/src/player.rs:1006–1017` (preparation-time frame capture) and `274–291` (new load deferral); `crates/starplayer-engine/src/sequencer.rs:1516–1532` (first transport attachment). Supporting paths: `host/src/source.rs:375–386` and `engine/src/engine.rs:585–599`.

**Trigger and wrong behavior:** Prepare a fresh interactive load, or a same-session replacement with a saved pending tick, and let its HostCommand wait while rendering continues—for example, use the supported retirement-pressure path that defers prepared loads until garbage is collected. The source's origin/deadline are anchored to preparation frame P. Installation at I calls `set_transport` on a source with `transport=None` and `held_at=None`. If the engine is stopped/paused, that sets `held_at=I` without translating the prepared origin/deadline by I−P. If it is playing, it performs no translation either.

For a fresh stopped load, its reported song time is already I−P before the first Play. On Play at R, translation covers only R−I, leaving the first deadline R−(I−P), in the past. Ticks catch up at the resume frame rather than beginning at the held start. For a restored paused cursor with remainder D, installation delay silently consumes D; after a long enough delay, resume immediately dispatches ticks that were supposed to remain pending. The same preparation/attachment shape exists for interactive SMF feeds.

The old code also anchored ordinary source preparation to a sampled frame; this finding concerns the **new interactive stopped/replacement contract and explicitly introduced deferred-install path**, not a claim that every legacy load-timing problem originated here. Stopped interactive audio now continues advancing the source clock, making that old assumption invalid even while the user never plays.

**Why tests miss it:** The WASM-host test `applied_barrier_is_absent_initially_and_waits_for_deferred_loads` deliberately produces this delay but checks only the final identity/serial/acceptance. Cursor-preservation tests install promptly. The worklet's deferred-message test prepares Rust state after the outer deferral, so it does not exercise a Rust source that has already been prepared and then held in `pending_command`.

**Bounded fix:** Carry a preparation origin with the source and rebase its logical cursor/deadline at the actual first audio-side installation, preserving the saved whole/fractional remainder and song time. Treat first attachment separately from an ordinary Pause/Resume transition. Translate sequenced state only, not absolute live event timestamps; include the SMF installation path.

**Regression verification:** Extend the existing retirement-pressure test to assert a fresh load remains at song frame zero before Play and starts with exactly one initial tick. Restore a paused cursor with a known nonzero fractional remainder after a delay longer than several tick periods; assert its song frame/remainder remain unchanged and the first tick after Resume occurs at R+D. Repeat for stopped/paused/playing attachment and SMF where applicable.

**Confidence:** High for the stated delayed-attachment path; source-derived, not experimentally reproduced in this review.

## ASTRA-03 — P2: IT instrument-only rows can resurrect a voice cleared by interactive Stop

**Location:** `crates/starplayer-it/src/processor.rs:3279–3287` (new clear hook), interacting with `1351–1363` and `1380–1397` (native instrument-only restart).

**Trigger and wrong behavior:** Play an IT note, issue interactive Stop or global all-sound-off, then resume/continue into a row containing an instrument number but no musical note. `clear_sound` invalidates the voice but retains `last_note`, with no marker distinguishing an interactive sound clear from normal native sample completion. The instrument-only branch therefore sees `!playing` (and an instrument mismatch against the now-empty foreground), synthesizes `last_note`, and starts it again. Thus an old note returns without a new musical note.

The branch explicitly classifies instrument-only sample replacements as non-musical onsets elsewhere (`musical_start` checks the original row note). The master says Resume must not resurrect cleared voices and effects may remain silent until a subsequent note; NO-03 repeats that verification requirement. MOD/S3M/XM gained `sound_cleared` guards for this distinction; IT did not. Native instrument-only restart is correct baseline behavior and should remain unchanged outside an interactive clear.

**Why tests miss it:** `host/tests/interactive_transport.rs:49–56` tests non-resurrection using a MOD E91 retrigger fixture. XM/IT overlap tests cover instrument-only behavior without first performing an interactive clear. The combined format schedule does not isolate an instrument-only row after Stop.

**Bounded fix:** Retain an IT channel-level interactive-clear marker while preserving its effect/instrument memories, and suppress synthetic instrument-only/effect restarts until an actual musical note is accepted. Set it for the relevant clear scope, not for checkbox bypass or normal native completion, and reset it on real onsets/appropriate sequencer reset. Keep native IT behavior unchanged for unconfigured hosts.

**Regression verification:** Instrument-mode and sample-mode IT: real note → Stop → Play → instrument-only row must remain silent, followed by a genuine note that must sound. Repeat for AllSoundOff while playing; retain a control test proving canonical instrument-only restart after natural sample completion still works.

**Confidence:** High for the derived restart; the desired distinction is grounded in the explicit interactive-clear contract.

## ASTRA-04 — P3: JS validation silently removes malformed members before atomic v2 admission

**Location:** `apps/starplayer-web/www/ring.js:147–156,166–174`; consumer at `www/worklet-processor.js:352–358`. Rust's intended all-or-nothing validator is `crates/starplayer-host-wasm/src/lib.rs:513–544`.

**Trigger and wrong behavior:** Send a v2 fallback batch containing a valid current-identity Pause and an invalid current-identity overlap boolean (`opcode=22, argument=2, extra=0`). `drainFallbackCommands` drops the malformed record and passes only Pause onward. Rust validates and applies the reduced group, and the worklet can acknowledge it as accepted. A malformed member in a shared ring is likewise consumed without notifying `applyCommand` or setting `controlFailed`. This bypasses the advertised whole-group rejection; an entirely malformed group can disappear without a rejection acknowledgment.

This is a malformed-client/protocol case, not an assertion that normal controls currently generate the bad boolean. Producer-side `pushCommand` already prevents that particular value through the normal UI.

**Why tests miss it:** JS wire tests assert producer-side refusal or that malformed records do not invoke the callback; Rust group tests submit invalid records directly to Rust. Neither tests a mixed valid/malformed group through the actual JS consumer and verifies that the valid member is not applied.

**Bounded fix:** Propagate a group-invalid result from both drains to the worklet, or forward structurally representable records to Rust's validator. Reject the whole consumed group with its requested identity instead of silently filtering individual members. Keep bounded drain sizes and legacy-v1 behavior.

**Regression verification:** Actual-worklet tests on fallback and shared transports: valid Pause + invalid boolean, valid volume + malformed identity, and invalid-only groups. Assert no partial host/UI state change and an explicit rejected acknowledgment where identity is available.

**Confidence:** High for the filtering/application behavior; lower priority because it requires malformed wire input.

## Coverage and limits

I inspected the complete changed-path inventory and branch-wide diff by subsystem, followed relevant callers outside the diff, and read the current master/task contracts and relevant architecture/accuracy foundations. Coverage included:

- Core identities/capabilities/cursor conversion; ownership, admission, retirement, provenance and duplicate matching; channel/layout changes and wide voice indices.
- MOD/MTM/S3M actual-onset capture, delays/retriggers and pending PCM tags; XM runtime extraction/snapshot/background ordering; IT extraction, native/custom NNA/DCT/DCA and native-victim exclusion.
- Prepared simple/XM/IT racks, raw velocity/CC7 identity, held instances/sustain/release, runtime RNG separation, prepared PCM scaling and live-domain quotas.
- Engine clocks/event splitting and DSP quanta; host transport/fades/end behavior, deferred ownership disposal, seek/replacement codecs, SMF and permanent live-source installation.
- WASM command decoding/headroom/barriers/full-width identities; JS ring and fallback consumers, load/replay/capture/rebuild lifecycle, insert preparation, input cleanup, media/playlist controls and telemetry consumers.
- Independent Cargo forwarding/defaults, core-only and host feature-unification fixtures, optional telemetry/runtime storage, embedded resource assertions and allocation-guard additions.
- Integration fixtures and test assertions, six-size schedules, `xtask/no07-feature-matrix.py`, configured golden comparisons and owner-artifact generation. The matrix genuinely compares separately compiled feature configurations; its output comparisons do not by themselves prove exact event placement (ASTRA-01). The artifact helper's non-identical WAV assertion establishes a PCM difference, not subjective audible quality.

Test inspection was selective at assertion/fixture and changed-test level, not a fresh execution or independent exhaustive conformance run. I did not audit every historical assembly instruction or every pre-existing DSP algorithm. I make no claim of browser/device listening, physical MIDI, allocation measurements, target compilation or conformance results beyond what source inspection supports.

I did not classify the documented baseline S3M fast-slide omission or native IT note-only-after-NNA mapping defect as new findings. I also did not treat intentional rebuild loss of processor working state, next-note CC7/CC10 latching, bounded release truncation under domain pressure, or native loops that require Stop Preview as defects. The pre-existing Player seek path's unconditional DSP reset deserves separate consideration when checking invalid-seek promises, but is not included as a newly introduced branch defect here.


## Opus supplemental report (verbatim)

# Supplemental review — StarPlayer `feature/note-overlap-audition` @ `55479442` vs `d0cdcc53`

Model: Claude Opus 5.5 (`claude-opus-5-5`); I can't see my effort setting. I only read files and diffs: no edits, shell commands, tests, builds or plan files. I did not open any other review report or triage draft.

**Result:** two new actionable defects, NO-R4 and NO-R5. I also note two things I traced and decided were baseline or design, so they aren't findings.

---

## New actionable findings

### NO-R4 — Medium (high confidence): pressing Pause after a song ends loses the armed rewind, so the next Play ends the song again

**Where (current source):**
- `crates/starplayer-host/src/player.rs:374-387`: the Pause branch never touches the armed rewind or `natural_end`. The Play/Resume branch only rewinds when `self.interactive_state == InteractiveTransport::Stopped && self.transport.take_armed_rewind()`.
- `crates/starplayer-host/src/player.rs:421-439`: `arm_end_of_song` re-fires on the sticky `end_reached`.
- UI side: `apps/starplayer-web/www/app.js:2074` leaves Pause enabled while Stopped, and `app.js:2470` turns Play into Resume while paused.

**Trigger:**
1. A song ends naturally (AtEnd FadeOut/Stop). The host goes to interactive Stopped with `natural_end=true` and the rewind armed.
2. The user presses Pause, from the page or a media-session Pause. State becomes Paused; the rewind stays armed and `natural_end` stays true.
3. The user presses Play. That is sent as Resume, and since the state is Paused, not Stopped, no rewind happens.
4. One quantum later, `arm_end_of_song` sees Playing plus the sticky `end_reached` and calls `finish_interactive_song` again. The host is back in Stopped with `natural_end` set.

**What goes wrong:**
- Play doesn't restart the song; the user needs a second Play.
- Play first resets the controller's `emitted` flag (`playback-controller.js:72`). The repeated natural end then emits a second playlist `ended` event (`playback-controller.js:96-101`).
- That breaks "no false playlist event from user Pause/Stop".

**Contract:**
- Master plan, host transport: natural end "arms rewind for next Play".
- Architecture diff: "natural end clears into that state with rewind armed for the next Play".
- Nothing says Pause cancels the rewind. Only user Stop does, and `explicit_stop_cancels_natural_end_armed_rewind` in `crates/starplayer-host/tests/interactive_transport.rs:110` tests that Stop case on purpose.

**Why tests miss it:** no Rust, worklet, Node or headless test does natural end → Pause → Play. `playback-controller.test.mjs:349` fakes snapshots and only tests a Pause before the end. The codec test pauses during a fade, not after the end.

**Fix (pick one):**
- Have Play/Resume consume an armed rewind whatever the held state is (`if self.transport.take_armed_rewind()`), and set `awaiting_restart` the same way.
- Or make Pause a no-op while `natural_end` is set.

**Regression test:**
- Native host: natural end → `pause()` → `play()`. Expect `song_frame()==0`, `!natural_end()`, and no re-finish within 8 quanta.
- Node controller: ended is emitted exactly once.

### NO-R5 — Low (high confidence): an invalid interactive seek still resets the DSP state

**Where:**
- `crates/starplayer-host/src/player.rs:1208-1219`: `request_seek` calls `insert_control.reset_all()` straight away, for every seek.
- `crates/starplayer-host/src/source.rs:228-235`: validity is only checked later, on the audio side (`can_seek_*`), and that check leaves voices alone.
- `crates/starplayer-host-wasm/src/lib.rs:127`: the v2 preflight only requires a module and a scan. `lib.rs:149`: `valid_wire` only bounds the argument (≤ u16::MAX).

**Trigger:** an interactive seek that passes the wire checks but is musically invalid. Examples: an order past the order list (100 on a 10-order song), a row past the pattern, or a frame past the timeline when the UI didn't clamp it (a scripted or wire caller).

**What goes wrong:** the mailbox seek is ignored and voices keep playing. But every insert's delay and reverb state, including master DSP tails under audition voices, has already been cleared without any click-safe transition. Interactive Stop does get that transition (`engine.rs` `clear_transition`).

**Contract:**
- Master plan: "Invalid seeks do not clear sound."
- Also: interactive seek "retaining … audition voices".

**Why tests miss it:**
- The NO-07 invalid-seek case uses `argument=u32::MAX`, which `valid_wire` rejects before `request_seek` runs.
- The UI clamps frame and order seeks (`app.js:2473-2475`, `:2671`), so the browser tests never reach this.

**Fix:**
- Interactive hosts: drop the control-side `reset_all()` and let `apply_request` reset the inserts only after `valid` is true. That's already an audio-side context, which can use the existing `clear_transition`.
- Alternatively, validate the order/row/frame against the scan on the control side before queueing.

**Regression test:** with a delay insert ringing, seek to order 999. Expect the insert's state and output to be unchanged, and the voices too.

---

## Traced and not reported as defects

- **Rebuild drops `speedAdjust` from `activeLoadOptions`** (`app.js:1615-1618`, `:1673-1676`). Main has the same code at its lines 1453 and 1509, so this is pre-existing.
- **User Stop after a natural end, then Play, re-ends the song.** Same mechanism as NO-R4, but the contract explicitly says user Stop cancels the armed rewind, and it's tested. I'm flagging it only as a UX question for the owner.

---

## Coverage of this pass

**Task specs and foundations**
- Read in full: NO-W1, NO-06 (deliverables, interfaces, handoff), NO-07 (including evidence and resource proof).
- NO-01 to NO-05: section structure only.
- Product foundation diffs: searched for the transport, seek and rewind contract lines only.

**Web app and worklet**
- `app.js`:
  - startup, node preparation and graph install;
  - fresh activation and supersede/cancel guards;
  - `activateModuleOnNode` host-settings replay;
  - the whole worklet message handler (load, defer, replay, state capture, acks, inserts, MIDI errors);
  - live input (destination, keyboard, MIDI, focus, disconnect, recovery);
  - the transport, interactive and command queues and fallback flush;
  - capture/replacement retry;
  - load-option, sample-rate, channel and mixer rebuilds;
  - telemetry/progress rendering;
  - media session, keep-alive and seek clamping;
  - the instrument rows (from the first pass).
- `worklet-processor.js` (all), `ring.js` v2 encode/decode/fallback, the `playback-controller.js` and `media-session.js` diffs.

**Host and engine plumbing**
- `starplayer-host/src/player.rs`: render state, command apply, rebuild, SMF/tracker install, live-input APIs, seeks, admission/validation.
- `replacement.rs` codec, host `engine.rs` and `insert.rs` diffs.
- Telemetry `snapshot.rs`, `apps/starplayer-web/src/lib.rs` metadata exports, facade `live.rs`.

**Tests and xtask helpers**
- `crates/starplayer/tests/no_integration.rs` canonical/schedule fixtures. Commands land on 128-frame-aligned deadlines, the three passive states are compared with unprepared PCM, and the overlap is audible.
- `xtask/no07-feature-matrix.py`: the neither-feature graph is checked and written first, then compared across builds.
- `xtask/no07-artifacts/main.rs`: 3 passive states × the committed goldens. State 2 checks only instruments with no one-shot sample.
- Changed media-session/controller test hunks and host transport tests.
- No weakened assertions found.

**Remaining limits**
- I didn't read in detail `headless.mjs`, `no-headless.mjs`, `worklet-harness.mjs`, `interactive-ui.test.mjs` / `interactive-wire.test.mjs` (beyond searches), `style.css`, `index.html` (beyond searches), the web README, or the full bodies of NO-01 to NO-05.
- Browser behaviour, real MIDI devices and embedded execution can't be checked from source alone.


## Supplemental coordinator disposition

The exact Opus supplemental invocation completed successfully (36 turns, high effort
supplied, returned exact model confirmed, no permission denials). Its web/worklet/host,
changed-test and artifact/matrix coverage supplements the first pass; remaining selective
inspection limits are retained in the raw report above. It did not receive Astra findings.

NO-R4 is source-confirmed in Host::apply_interactive: Pause changes the held transport
state while the Play/Resume rewind condition incorrectly requires Stopped. Honor armed
natural-end rewind across held-state transitions; explicit user Stop remains the approved
rewind-cancel action. NO-R5's unconditional insert reset predates the feature, but the
new explicit interactive invalid-seek contract requires preserving sound on rejection.
Scope the correction to interactive behavior; do not call unrelated canonical baseline
seek behavior a new regression. Actual musical validity and DSP reset must stay coherent
across the control/audio boundary; UI-only clamping or rejecting u32::MAX is insufficient.
Focused regressions will establish the before/after behavior.


## Corrective reviewer follow-up status

The coordinator verified the frozen18-file source/test manifest and independently passed
18 targeted Rust tests (`/tmp/starplayer-no08-coordinator.log`). Opus5.5/high corrective
review was attempted on that diff with read-only tools, but was interrupted after13 reads
by the provider's five-hour quota: **"You've hit your 5-hour limit · resets 12:20pm
(Australia/Perth)"**. Exit1 / is_error=true; no closing verdict or acceptance was returned.
The full and supplemental Opus initial reviews above had completed successfully; this
interrupted follow-up is not represented as a completed review. Exact model/high request
were retained with no substitution. Attempt artifacts are preserved under
`/tmp/no-branch-review-5547944/opus-fix-review-quota-1*`.

Astra's requested medium-effort corrective review now runs independently of that failed
attempt. Source/test files remain frozen; review/landing status will be updated after
the available follow-ups finish.


## Astra corrective follow-up 1 (verbatim)

# NO-08 focused corrective source review

Reviewed worktree: `/home/scott/projects/starplayer-no-08-review-fixes`, branch `task/no-08-review-fixes`, based on `55479442f198f16be0492e8da9821f35a02762ec`, with the supplied uncommitted corrective diff. Requested reviewer: GPT-6-astra, medium; I cannot independently introspect runtime model/effort.

I verified the supplied 18-file source/test manifest: its SHA-256 is `7fe9c3dba6cb861d2125413e6ad359938e511a5a740931d3b231798b429ac9bb`, and all 18 file hashes match. Read the NO-08 assignment/task, current master and relevant architecture/accuracy foundations, both initial reports and dispositions, all corrective production/test changes, and surrounding scheduling, transport, IT and worklet paths. This was read-only source review. No tests, builds, formatting, commits or device actions were run; the only write was this report.

**Result: seven corrections resolved; malformed-v2 correction substantially resolves the original filtering defect but remains incomplete for malformed fallback opcode types. One concrete remaining P3 issue is described below. No other new corrective defect found.**

## Remaining finding — NO08-A1 / NO-I1 / ASTRA-04 (P3, high source confidence)

**Require an integer opcode before forwarding a fallback v2 member.**

Location: `apps/starplayer-web/www/ring.js:177–180`, with the called predicate at `85–90`. Downstream: `www/worklet-processor.js:61–64`; Rust numeric boundary `crates/starplayer-host-wasm/src/lib.rs:859` (`opcode: u32`). The generated packaged wrapper at `dist/starplayer-worklet.js:302–303` forwards the argument directly to the WASM export.

The new fallback drain correctly emits opcode-zero sentinels for invalid members, but its `valid` predicate only requires `Number.isInteger(record.opcode)` in the legacy-opcode arm. The interactive arm calls `validInteractive`, which validates argument/extra/identity but not opcode type. `validInteractiveArguments` accepts the relational range `opcode >= 13 && opcode <= 18`.

Consequently, a structured-cloned fallback record with current valid identity, argument/extra zero and `opcode: 14.5`, `opcode: "14"` or `opcode: 14n` is classified valid and forwarded unchanged:

- `14.5` or `"14"` crosses the WASM `u32` boundary as 14 and can execute Pause. A group containing a valid overlap change plus this malformed member can therefore apply, instead of being rejected atomically.
- `14n` passes the relational predicate but cannot be converted to the WASM numeric `u32` parameter; it throws at that call rather than producing the explicit rejection acknowledgement. An earlier group member may already have been staged when this occurs.

These are source-derived consequences; I did not execute the cases. Ordinary `Ring.pushCommand` already rejects such opcodes (`ring.js:120`), so this remains the accepted malformed/foreign-producer hardening scope, not a normal UI regression. The permissive predicate predates NO-08; this is an **incomplete accepted correction**, not a claim that NO-08 introduced the predicate.

The new test `interactive-wire.test.mjs:96` and actual-worklet cases cover BigInt argument/extra, malformed identities, opcode 99, invalid boolean, mixed order and invalid-only groups. They do not cover malformed opcode scalar types in the accepted 13–18 range.

**Bounded fix:** require `Number.isInteger(record.opcode)` and the supported opcode range before either fallback validity arm (or enforce this in the common interactive predicate and retain the legacy guard). Ensure the failed member follows the existing opcode-zero sentinel path with representable identity preserved. Do not numerically coerce malformed opcodes into valid ones.

**Regression:** fallback direct-drain and actual-worklet/WASM tests with 14.5, `"14"`, and 14n, individually and before/after a valid state-changing member. Assert no exception/fault, exactly one rejected acknowledgement, current/requested identity retained, no partial state change, and successful valid retry. Existing SAB integer, v1 and 63-record tests should remain unchanged.

## Per-finding disposition

### ASTRA-01 — Resolved

`engine/src/engine.rs:755–757` polls source lookahead at the current source frame before the first scheduling/dispatch iteration of every active quantum. This covers initial installation, nonzero installation and later admissions into an idle queue; it is not merely constructor priming. `MidiSource::advance_to` only refreshes the feed; tracker/scripted/silent sources are no-ops and mux/wrappers forward. The polling adds no articulation step or periodic source deadline. Existing source-order and post-source cadence processing remain intact.

`starplayer/tests/live_audition.rs:433` observes actual dispatch frames for 0/17/18 at base 0/1024, checks zero late warnings and a short nonzero PCM window, and separately checks newly admitted idle input. Each phase renders 8192 frames in chunks of each required block size, so 8191 is an actual render call. The existing simultaneous NoteOn/NoteOff test and integration schedules remain. Source evidence resolves the mechanism; I did not rerun these assertions.

### ASTRA-02 — Resolved

Tracker preparation anchor is feature-gated at `engine/src/sequencer.rs:723`, initialized and refreshed during preparation/restart/restore (`788`, `1061`, `1112`). First transport attachment (`1525–1545`) translates deadline and song origin exactly once while leaving the fractional clock remainder intact; later held transitions retain the existing duration translation. SMF uses a consumed optional preparation anchor and lazy event translation (`midi/src/sequencer.rs:71,110–121`), avoiding event-vector work on audio attachment. Absolute live feeds receive no preparation translation.

`engine/tests/interactive_transport.rs:236` checks all three target states, saved fractional remainder, exact resumed tracker deadline and unchanged absolute live timestamp. `host/src/player.rs:1624,1647` deliberately reaches actual retirement-queue deferral and checks tracker/SMF logical position and pending remainder across Playing/Paused/Stopped. The tracker assertion checks its exact first resumed tick. `host/tests/interactive_transport.rs:354` covers fresh stopped installation at song frame zero; the direct SMF test (`midi/src/sequencer.rs:166`) checks exact translated deadlines. No constructor-only or control-thread validity workaround remains.

### ASTRA-03 — Resolved, including both IT refinements

The audition-only `InteractiveSoundState` at `it/src/processor.rs:352–355` distinguishes NeverStarted, Remembered and Cleared. `start_note` records Remembered only after allocation and articulation start succeed (`1687–1688`). Relevant All/Song/lane clears suppress previously started channel memory (`3307`); Live/custom-only cleanup does not. The instrument-only synthesis branch consults Cleared (`1371–1375`), preventing the original resurrection while leaving fresh native C0 starts possible.

Genuine note-only recovery resolves remembered instrument mapping inside `note_change` (`1503–1515`), without manufacturing an authored instrument column. Thus it avoids `instrument_change` default-volume and IFC/IFR replay while retaining normal explicit/pending instrument semantics. The resolved state feeds `start_note`, whose voice state takes the channel's current note volume and filter values.

Tests include sample/instrument modes and All/Song/lane suppression, effect-retrigger silence and genuine note-only recovery; a delayed/latched instrument that never started does not arm suppression. The retained-memory regression (`processor.rs:3576` vicinity) requires channel and voice volume 68, cutoff/resonance 24/37 and passive onset identity 68, with no-clear controls. Facade tests (`xm_it_overlap.rs:551,572`) use actual mixer completion for native restart controls, unrelated clear scopes, and exact fresh configured/native PCM equality with an audible IT C0 control and silent XM control. These are meaningful refinements rather than weakened original assertions. No change to the documented unrelated native IT NNA mapping baseline is inferred.

### NO-R1 — Resolved

`engine/src/control.rs:135–137` clamps installed Q32.32 cadence to `1 << 32`, retaining valid intervals and the supplied fractional phase. Synthesized ticks therefore make at least one frame of progress. The direct clock regression exercises zero, tiny, just-subframe and one-frame intervals; the configured-engine regression (`engine/tests/interactive_transport.rs:206`) requires 256 steps over 256 frames and no zero-advance/event-limit warnings. This preserves infallible construction and adds no public error or heap state.

### NO-R2 — Resolved for the accepted burst defect

`engine/src/engine.rs:883–893` coalesces interactive publication using a drain-local boolean and publishes the final state after the bounded command drain. Per-command publication was removed. There is no new Engine field or generic telemetry-ring policy change. The direct engine test (`interactive_transport.rs:219`) sends more than ring capacity, then AllSoundOff/Pause, and checks final live count, Paused state and exact captured cursor immediately. The packaged worklet regression similarly checks the first quantum's final live/held state. The initial report's speculative replacement-cursor extension remains unproven and is not claimed independently reproduced.

### NO-I1 / ASTRA-04 — Incomplete (NO08-A1 above)

The original invalid-member filtering is fixed for the covered numeric wire records. Shared drain forwards every consumed record; fallback emits invalid sentinels while retaining representable identity words. Rust validates the complete staged group before mutation, and the worklet produces an identity-bearing rejection. The actual-worklet tests exercise both transport orders, invalid-only rejection and retry. However, the fallback opcode-type hole prevents full closure of the accepted malformed-opcode/BigInt scope.

### NO-R4 — Resolved

`host/src/player.rs:379` consumes an armed natural rewind on Play/Resume regardless of the intermediate held state. Explicit Stop still cancels it through the existing `interactive_stop(false)` path. `host/tests/interactive_transport.rs:307` covers AtEnd Stop/FadeOut, Pause, both Play and Resume, song frame zero and continued playback beyond eight quanta. The packaged worklet/controller test counts exactly one ended notification across the same Pause/Resume rewind. No new user-Stop semantics were introduced.

### NO-R5 — Resolved

Interactive `Player::request_seek` no longer queues an unconditional insert reset (`host/src/player.rs:1220`); legacy behavior remains. `SeekableModuleSource::apply_request` validates musical bounds before clearing Song and requesting DSP reset (`host/src/source.rs:225–237`). A dispatch-local context flag is consumed by Engine (`engine/src/engine.rs:1140–1159`) before normal whole-128-frame insert processing. The flag is forwarded through context reborrowing and compiles out without audition. Live voices are retained; the existing additive clear transition bridges output.

Real host and actual-worklet regressions compare complete ringing channel/master-delay PCM for invalid order/row/frame requests against a no-seek reference. A reset-probe test (`host/tests/note_overlap.rs:159`) requires whole-128-frame insert calls, zero resets for invalid seeks, exactly one reset per chain for a valid seek and the same live VoiceId. WASM reservation tests verify seek-only admission with a full insert ring while a group containing an actual insert mutation still rejects atomically. This couples mutation to authoritative audio-side validity rather than a stale control-side/UI check.

## Invariants, evidence and remaining limits

The corrective production changes introduce no observed audio-path allocation, lock, transcendental operation or panic path in Rust. New preparation state and IT clear history are feature-gated; the DSP request is a borrowed stack-local flag, not persistent Engine storage. Source polling remains bounded by existing source/queue capacity. DSP processing remains on complete 128-frame quanta. No manifests or native NNA policy were changed. The fallback opcode issue above is the remaining JS exception path identified in this review.

The NO-08 task records failing-before evidence for all eight accepted mechanisms, separate failures for the two IT refinements, and final settled CI/matrix/browser results. Those are worker/coordinator runtime evidence, not tests I ran. I inspected changed fixture/assertion logic and verified the frozen source hashes, but did not independently audit every log, execute conformance/portable/resource checks or repeat the original whole-branch review of unchanged code.

The ordinary browser removal check now waits for the exact accepted request and current identity, rather than treating optimistic UI removal as completed audio work. Separate actual-worklet and browser remove→Stop tests still issue Stop without awaiting removal and retain strict transport/frozen-row checks, including the six browser delay phases. This is reasonable precondition/coverage strengthening, not a diagnosis of the retained original SAB timeout. The timeout's cause remains uncertain; no concrete new production lifecycle defect follows from the available evidence. Final browser passes do not prove that diagnostic can never recur.

Subjective listening, physical MIDI, phone/device behavior and firmware execution remain outside this review. No main merge, deployment or owner acceptance is implied. The Opus closing pass was quota-interrupted and is not represented here as reviewer acceptance.


## Coordinator disposition of NO08-A1

Accept as incomplete NO-I1/ASTRA-04 hardening. Fallback opcode validity must reject
non-integer/wrong-type opcodes before any coercion or WASM call. Add failing direct-drain
and actual-worklet cases for14.5, "14" and14n alone and beside a valid current-identity
member, then require explicit whole-group rejection, no exception/partial change and
valid retry. Reuse the common validator where appropriate; no valid-command/v1 behavior
change is approved. This is JavaScript protocol validation only; settled Rust CI and
resource/PCM evidence remain applicable if Rust is unchanged. Final JS/package/wire and
relevant browser evidence must be rerun. Preserve the prior review manifest/report as
historical evidence, refresh the final manifest and obtain Astra's focused closure.


## NO08-A1 worker verification handoff (reviewer closure pending)

The fallback opcode coercion/exception cases were experimentally reproduced in direct
drain and actual packaged worklet/WASM tests, including invalid-only and both mixed
orders. A common Number integer/supported-range guard now sends all three malformed
opcode forms through explicit whole-group rejection. Post-guard 347 JS, 22 web tests,
packaged wire/worklet, strict pending Stop stress and ordinary/interactive Chromium
SAB/fallback/plain all pass; exact logs, commands and refreshed hashes are in NO-08.
Rust files are unchanged from the settled 11-job CI and matrices. These are worker runtime
results, not a claim that Astra has closed NO08-A1 or Opus returned a corrective verdict.


## Astra final corrective closure (verbatim)

# NO-08 final focused closure

**NO08-A1 is resolved. All eight accepted corrective findings are now resolved in the reviewed source. No new concrete defect found in this bounded follow-up.**

Reviewed `/home/scott/projects/starplayer-no-08-review-fixes`, based on `55479442f198f16be0492e8da9821f35a02762ec`, with the frozen uncommitted corrective changes. Requested reviewer configuration remains GPT-6-astra/medium; runtime model/effort is not independently introspectable.

Verified the manifest SHA-256 `ae781b908dd3fc82bff38cb1c152d715229e5547d239f235a0e922879ff99f83`, all 18 listed file hashes, and supplied `fixes.diff` SHA-256 `09b03327d6c72fbec225251d4df4357168529a0b4b4e0ff03026b668319cd923`. Compared the previous and current manifests: only `www/ring.js`, `test/interactive-wire.test.mjs` and `test/worklet-harness.mjs` changed. All Rust files and the browser headless tests match the previous corrective review.

## NO08-A1 / NO-I1 / ASTRA-04 — Resolved

`apps/starplayer-web/www/ring.js:90` now starts the shared interactive argument validator with:

```js
if (!Number.isInteger(opcode) || opcode < 13 || opcode > 24) return false;
```

This rejects fractional and non-Number opcodes before the permissive 13–18 relational arm. In particular, 14.5, `"14"` and 14n can no longer be forwarded unchanged by the fallback drain. They follow its existing opcode-zero sentinel path (`ring.js:178–181`), preserving representable session/build words. The worklet forwards that explicit invalid member to the staged group (`worklet-processor.js:61–72`); Rust validates the complete group before applying any member (`host-wasm/src/lib.rs:517–546`). The correction therefore closes both numeric coercion into Pause and the BigInt-to-WASM conversion exception from my prior report.

The change is at consumer validation, not merely the normal producer gate. Valid interactive numeric opcodes retain their prior argument rules. Legacy fallback records retain their existing separate integer check, shared records remain bounded integer wire words, and the v1 branch is unchanged. No new allocation, storage or protocol field was added by this guard.

The added direct-drain tests (`interactive-wire.test.mjs:104` onward) cover each malformed opcode alone and on either side of a valid overlap command. They require an invalid sentinel, unchanged record count, no exception and preserved identity words.

The actual packaged-worklet/WASM test (`worklet-harness.mjs:611–651`) uses a fresh installed processor for each of the nine type/order cases. It checks no exception or fault, exactly one rejected acknowledgement, full-width current/requested identity, unchanged Stopped state and overlap setting, and a subsequent accepted valid retry. Errors are collected but cause the final assertion to fail; the diagnostic loop does not swallow failures. These assertions directly address the previously observed conversion/partial-staging mechanisms rather than testing producer refusal alone.

## Earlier dispositions remain valid

The first corrective report remains preserved at `/tmp/no-branch-review-5547944/astra-fix-review.md`; its detailed source reasoning and limitations continue to apply except for its now-closed NO08-A1 finding.

| Accepted correction | Final disposition | Basis retained from first corrective review |
|---|---|---|
| ASTRA-01 external event visibility | Resolved | Quantum-start feed polling; exact initial/idle/nonzero dispatch and real 8191-frame call coverage |
| ASTRA-02 first attachment | Resolved | One-time tracker/SMF preparation translation, preserved logical/fractional remainder, absolute live timestamps unchanged |
| ASTRA-03 IT resurrection | Resolved | Accepted-start history distinguishes fresh setup from explicit clear; mapping-only genuine-note recovery preserves volume/filter/onset memory |
| NO-R1 minimum cadence | Resolved | One-frame minimum Q32.32 cadence, retained valid phase and progress regressions |
| NO-R2 burst telemetry | Resolved | One publication after bounded interactive command drain, final live/transport/cursor assertions |
| NO-I1 / ASTRA-04 malformed groups | Resolved | Original raw-member/sentinel forwarding plus the final opcode integer/type/range guard and nine actual-WASM cases |
| NO-R4 natural-end Pause/Play | Resolved | Armed rewind consumed across held states; explicit Stop cancellation and exactly-once ended controls retained |
| NO-R5 invalid seek | Resolved | Audio-authoritative validation precedes Song clear/DSP reset; real delay PCM and whole-quantum/live-voice tests retained |

No timing, DSP, Rust feature gating, native fallback or IT source changed in this final guard revision. The previous seven resolutions therefore remain applicable, including the two IT refinements. The exact-removal acknowledgement precondition and separate no-wait pending-Stop browser coverage are also unchanged. The original browser timeout remains an undiagnosed retained diagnostic, not a production defect claimed fixed by this guard.

## Evidence limits

I read the appended NO-08 failing-before/post-guard evidence, exact changed guard/test hunks, and relevant validator/forwarder/Rust admission paths. The task records all nine malformed opcode cases failing before the guard and passing after, 347 JS tests, 22 web tests, package/ring/worklet checks, pending-Stop checks and all six Chromium modes across 13 completed post-guard steps. The coordinator separately reports its 10-test wire regression. These are recorded worker/coordinator executions, not tests I ran or independently replayed logs.

The earlier 11-job Rust CI, 963 affected Rust tests and feature/resource/portable/PCM matrices precede the JavaScript guard; unchanged Rust hashes support their continued applicability. This report does not mislabel them as a newly run full CI.

This was source review only: no tests/builds, source edits, formatting, commits, hardware actions or nested agents. The sole write was this final report; review1 is preserved. No main merge, deployment or subjective/device acceptance is implied. Opus's quota-interrupted closing pass remains unaccepted and is not represented as completed by this review.

The sole review stream is released.


## Opus final corrective closure (verbatim)

# NO-08 corrective review — closing disposition

**Snapshot reviewed:** `task/no-08-review-fixes`, base `55479442`, the 18-file corrective diff, with the current `ring.js` (including the NO08-A1 opcode guard).
**Reviewer:** Claude Opus 5.5 (`claude-opus-5-5`). The effort setting isn't visible to me.
**Method:** I only read files. I didn't run any tests, builds, browsers or devices, so the pass logs are the worker's and coordinator's evidence, not my own checks. I read Astra's final report only to compare conclusions, after forming my own.

## Verdict

All eight accepted corrections are **resolved** in source, and their regression tests make meaningful assertions. I found **no new concrete defect** in NO-08 and have no further actionable finding. One out-of-scope behaviour that already existed is noted at the end.

---

## Per-finding disposition

### ASTRA-01 — external event visibility: **Resolved**
- **Fix:** `engine.rs:755-757` calls `self.sources.advance_to(self.source_frame)` once per quantum, after the command drain and before the first segment is chosen.
- **Same-frame safety:** I checked every `advance_to` implementation:
  - the tracker `PatternSequencer` (`sequencer.rs:1575`), scripted and silent sources are no-ops;
  - the wrappers (`starplayer/src/sequencer.rs:408`, host `source.rs:202`, host-embedded `source.rs:224`, `SourceMux` `source.rs:531`) only forward;
  - `MidiSource` (`instrument.rs:1391`) only calls `feed.refresh`, which fills `peeked` or runs the existing recovery/epoch logic (`instrument.rs:1170-1190`).
- So the extra call is idempotent at the same frame, runs no articulation, and leaves splitting, source order and DSP128 alone.
- **Test:** `live_audition.rs` `no08_prepared_external_feed…` records the actual dispatch frame through an observing feed. It covers bases 0 and 1024, offsets 0/17/18, all six block sizes, and a second event that arrives while the queue is idle. It asserts no late warning, silence before onset and nonzero PCM within 16 frames. This proves exact placement independently of the six-size output-equality tests.

### ASTRA-02 — deferred first attachment: **Resolved**
- **Tracker:**
  - Anchor `prepared_at` (`sequencer.rs:723`, `:788`). It is updated during preparation by `restart_clock_at` and `restore_cursor`, only while `transport.is_none()` (`:1061`, `:1112`).
  - On the first `set_transport`, both the clock and the song origin move by the attachment delay, exactly once (`:1525-1536`). This happens before the existing held/Playing logic, so later Pause/Resume behaves as before.
  - The whole-plus-fractional remainder is kept, because only an integer frame translation is applied.
- **SMF:** `prepared_at: Option<Frame>` is set only by `interactive_at` and is consumed (`take()`) on the first `set_transport`, which adds the delay to `translation` and `song_origin` (`midi/sequencer.rs:24`, `:71`, `:110-114`).
- **Live input:** queues have no `set_transport` override, so absolute live timestamps are untouched. The engine test asserts that directly ("must not translate live timestamps").
- **Unaffected paths:** legacy and overlap-only paths never call `set_transport`.
- **Tests:**
  - Host tests (`player.rs` `no08_attachment_tests`) use real retirement-queue deferral of at least 4096 frames for Playing, Paused and Stopped. They check exact `song_frame`, `remaining_frames` and fractional remainder, a next tick at exactly `resume + remaining`, and SMF event-index progression.
  - A fresh deferred load starts at frame 0, tick 0.

### ASTRA-03 — IT no resurrection after a clear: **Resolved**, including the first-C0 and retained-memory refinements

**Marker:** a three-state `InteractiveSoundState` (NeverStarted / Remembered / Cleared) at `it/processor.rs:354`, `:462`, `:555`. It exists only when the `interactive-audition` feature is compiled.

**How the marker moves:**
- **Remembered** is set only after a voice is actually admitted (`:1688`, after `allocate_voice` and `articulation.start`). A merely latched or pending instrument doesn't count.
- **Cleared** is set only from Remembered, and only for All, Song or this channel's Lane (`:3307`). Live and CustomDetached scopes leave it alone.

**Behaviour:**
- **Fresh session:** the native first instrument-only C0 start is kept. With NeverStarted, the synthetic note path still runs (`:1366-1373`).
- **After a clear:** the synthetic instrument-only restart is suppressed (`suppressed` at `:1372`).
- **Note-only recovery:** a genuine note with no instrument column, after a clear, maps through `last_instrument` inside `note_change` (`:1503-1515`). `instrument_change` is not replayed, so base volume, the instrument's initial filter cutoff/resonance (IFC/IFR) and the passive onset identity survive. Sample mode already used `last_instrument`.
- **No-clear path:** unchanged, including the documented native note-only mapping baseline.

**Regressions guarded:**
- Volume 68 is kept (the early bug gave 256), cutoff/resonance stay 24/37, and the voice's onset identity is 68 (`no08_note_only_recovery…`), tested in both sample and instrument modes, with and without a clear.
- A latched instrument without an accepted start doesn't arm suppression.
- Suppression holds across All/Song/Lane in both modes, retrigger stays silent, and a real note recovers.
- `xm_it_overlap.rs`: a real mixer sample completion followed by an instrument-only row still restarts, including under Live/CustomDetached clears. The fresh configured Stop→Play run is PCM-identical to native for IT (audible C0) and XM (silent).

**MOD/S3M/XM by inspection:** their clear flags only suppress paths that need a prior note or SAMPLE dirtiness, so a fresh Stop has no audible effect. That matches "checked without production changes".

### NO-R1 — zero / sub-frame cadence: **Resolved**
- `control.rs:132-139` applies `interval.max(1 << 32)` and passes the remainder through. Valid intervals are bit-identical.
- Tests: direct clock checks for 0, 1, (1<<32)-1 and 1<<32 show one frame of progress per tick. Engine checks for 0/1/22 µs show 256 ticks in 256 frames with no zero-advance or event-limit warning. The six-size schedules come from the matrix evidence, not from my own run.

### NO-R2 — burst telemetry: **Resolved**
- The per-command publish was removed from `apply_interactive`. `drain_commands` keeps one stack-local `interactive_changed` flag and publishes once after the bounded drain (`engine.rs:880-894`).
- No new Engine field, heap storage or change to ring policy; the gating is unchanged (`interactive-audition` + `telemetry`).
- Tests: 5×Resume + AllSoundOff + Pause in one drain gives a snapshot that is Paused, with `live.active == 0` and the correct cursor frame. The actual-worklet burst test asserts the same.

### NO-I1 / ASTRA-04 — malformed groups (including NO08-A1): **Resolved**
- **Shared ring:** the v2 drain forwards every consumed member (`ring.js:152-154`). Rust then rejects the whole group, either through `enqueue_interactive_words` (opcode outside 1..=24 → `controlFailed`) or through `valid_wire`.
- **Fallback:** v2 always takes the object path. A malformed member becomes opcode 0 with every identity word that can be represented, and BigInt, NaN or null arguments become 0 (`ring.js:169-184`).
- **Common validation:** `validInteractiveArguments` now rejects non-integer or out-of-range opcodes (`ring.js:90`), which closes 14.5, "14" and 14n.
- **Unchanged:** v1 triples and the 63-record limit.
- **Tests** (actual worklet + WASM, SAB and fallback):
  - valid+malformed in both orders, and malformed alone;
  - bad identity, BigInt arguments, and the nine opcode type/order cases;
  - each gives exactly one rejection carrying the current identity, no exception, no partial overlap change, and a valid retry that applies.

### NO-R4 — natural end → Pause → Play: **Resolved**
- `player.rs:379` consumes the armed rewind whatever the held state is. Explicit Stop still cancels it through `interactive_stop(false)`, and the strict existing test is unchanged.
- Tests:
  - The native host test covers Stop (F00 ending) and FadeOut × Play/Resume, and asserts frame 0, no `natural_end`, and still playing eight quanta later.
  - The wire/controller harness asserts `ended` is emitted exactly once.

### NO-R5 — invalid musical seeks: **Resolved**
- **Control side:** in interactive mode `Player::request_seek` no longer queues ResetAll (`player.rs:1220`). Legacy mode still does.
- **Audio side:**
  - The source requests the reset only after validating the seek, inside `transport.is_some()` (`host/source.rs:235-237`).
  - The engine passes a dispatch-local `&mut bool` and runs `reset_all_inserts()` plus `clear_transition` after that dispatch (`engine.rs` dispatch block; `source.rs:124-132`).
  - The DSP still processes whole quanta afterwards, so state resets at a DSP-quantum boundary. There's no cross-thread validity race and no insert-ring reservation; the WASM preflight drops the seek's insert count (`lib.rs:532-534`).
- **Tests:**
  - `ResetProbe` asserts 128-frame `process` calls, zero resets for invalid order/row/frame, exactly two resets (master + channel) for a valid seek, and the same live `VoiceId` throughout.
  - The host and actual-worklet delay-PCM checks show identical output for invalid seeks and changed output for a valid one.
- **Test change, by design:** `versioned_groups…` case 5 now tests seek + full-ring insert mutation, because a seek no longer reserves an insert slot. The assertion that the seek mailbox stays unwritten is kept, so this isn't a weakening.

---

## Cross-cutting checks

| Area | What I checked | Status |
|---|---|---|
| Exact timing | Feed poll is idempotent | No regression |
| DSP128 | Reset lands at a quantum boundary; probe asserts 128 frames | No regression |
| Allocation-free real-time | Only a stack bool / reference and enum fields | No regression |
| no_std | No std usage added | No regression |
| Feature-off | New fields and the context reference are cfg-gated | No regression |
| Native/legacy fallback | Legacy `reset_all` kept; tracker/SMF anchors are never consumed outside interactive `set_transport`; v1 wire unchanged | No regression |

---

## Observation (not caused by NO-08, not actionable here)
- After a natural end, a user seek doesn't cancel the armed rewind, so the next Play still jumps to frame 0. This already happened from Stopped before NO-08; NO-08 extends the same rule to Paused.
- The UI already shows position 0 while `naturalEnd` is set, so this looks consistent with what the user sees. Whether it's intended is a product question for the coordinator; it isn't a NO-08 defect.

## Browser and verification evidence
- Settled acceptance rests on `/tmp/no08-ci-settled-final.log` (11 jobs on the final Rust hashes), the settled affected-suite run (963 tests), the final matrix, and Astra's JS-only NO08-A1 runs (JS 347 / web 22 / ring / worklet / six browser modes).
- The earlier CI logs (`no08-ci.log`, `-refined-final`) are correctly kept as evidence for earlier trees only.
- The single ordinary-SAB Stop timeout is honestly kept as an undiagnosed diagnostic. The source-level controls for it are the forced remove→Stop worklet check on both transports and the correlated None round-trip; the Chromium stress runs at 0–50 ms of pending preparation are browser evidence I didn't run.

## Remaining limits
- No dynamic verification by me.
- I didn't read the `headless.mjs` or `interactive-wire.test.mjs` diffs in depth.
- Firmware compile-only results, listening, physical MIDI and device behaviour are still owner-pending.


## Final coordinator disposition — 2026-10-28

Both independent full-branch source reviews completed on frozen5547944 vs main d0cdcc5.
Their eight accepted deduplicated corrections are implemented and verified; both final
corrective source reviews now mark all eight resolved with no new concrete defect.
Astra's opcode follow-up NO08-A1 was reproduced, fixed and explicitly closed. The two IT
refinements preserve native first C0 and retained volume/filter/onset memory. No reviewed
product decision was changed.

The first Opus closing attempt was genuinely quota-interrupted; its artifacts/error remain
preserved. After the reported12:20Perth reset, exact claude-opus-5-5/high retry succeeded
(exit0,19 turns, no permission denials), forming its own source conclusions before
comparing Astra's final report. Earlier pending-status statements describe those checkpoints
and are superseded by this final disposition; the interrupted attempt is not counted as
acceptance. Both closing reports are source reviews, not independent runtime executions.

Coordinator verification:18 targeted Rust tests and10 focused wire tests pass; final18
source/test hashes match. Worker evidence covers settled Rust CI11/affected963, isolated
feature/resource/portable/fixed-float PCM, unchanged8 committed/24 prepared golden hashes,
and post-opcode JS347/web22/actual wire plus all six browser runs. Fresh coordinator
firmware-common96 and A1S production/C5 bench release links also pass with neither optional
flag in exact board graphs. Opus did not perform/check those fresh firmware results; its
raw owner-pending firmware-compile limit above is superseded by the recorded coordinator
evidence. Physical execution/timing, listening, MIDI and phone/device acceptance remain
pending.

The original single SAB Stop timeout remains an undiagnosed retained diagnostic; strict
accepted-removal preconditions and separate forced pending-Stop/stress controls all pass.
No production bug is claimed diagnosed from that timeout. The natural-end seek/armed-
rewind observation is baseline behavior under the existing explicit rewind rule; only
user Stop is the reviewed cancellation action. No unrequested transport-policy change
is made for that observation.

The verified corrective change is accepted for single-change task commit and feature
squash. Main merge/deployment and archival remain outside this task.


## Coordinator feature landing

Task commit `20dbc07` is integrated by reviewed single-change squash from the feature
checkout. All18 source/test hashes and the exact09b03327… corrective patch remain
unchanged; only plan status/landing records differ from that task commit. The initial
independent source reviews, final corrective acceptances, quota retry, all diagnostics
and current verification limits are preserved. Main and deployment are untouched.
