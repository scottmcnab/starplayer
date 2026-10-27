# NO — Master/task review record and implementation handoff

| Field | Value |
|---|---|
| Status | Master/task review complete; corrections applied; final approval condition satisfied |
| Requested reviewer | Claude CLI `claude-opus-5-5`; `--effort high` supplied; returned modelUsage confirms exact model |
| Earliest invocation | After 2026-10-27 16:20 Australia/Perth / 08:20 UTC, per owner |
| Master planning commit | c3bead3 |
| Task planning commit | 4755c94 |
| Implementation | Not started; owner expressly requested planning only |

## Original deferred-review procedure (historical)

1. Work from `../starplayer-note-overlap-audition`, branch
   `feature/note-overlap-audition`. Read AGENTS.md and NO-master-plan.md.
2. Check local time and CLI usage/model configuration without assuming a reset has
   actually restored quota. Inspect `claude --help` and available configuration;
   select the owner's requested Opus-5.5/high only if supported. Do not silently
   substitute a model, use an unrelated alias or bypass quota restrictions.
3. Use a read-only CLI permission mode, with no edits, shell writes, implementation,
   commits or nested workers. Provide the repository path and the prompt below through
   stdin/a prompt file using the documented CLI syntax. Do not guess flags/model IDs.
4. Capture master findings here, triage each with evidence, fix accepted findings and
   amend any affected foundation/task files. Record disagreement reasons. Request
   targeted follow-up review of material changes before marking master review complete.
5. Review tasks against the corrected master separately; capture/resolve findings and
   re-review affected interfaces/dependencies. Commit each coherent review resolution
   using explicit paths and no attribution or session URLs in the commit/PR text.
6. If quota/model is unavailable, record the actual error and tell the owner. Do not
   label either review complete. No automatic timer or scheduled invocation was created.
7. After review, hand off only on a subsequent implementation request to the Sol xhigh
   coordinator using task worker assignments. No current implementation authorization.

## Master review prompt

Review `plans/engine/NO-master-plan.md` and its planned foundation amendments in
`plans/product/01-technical-architecture.md` and `03-accuracy-policy.md` against actual
Rust/web source. You are reviewing a plan, not implementing. Do not modify files.
Prioritize correctness, scope, real-time/no_std constraints and missing decisions.
Check clock domains/resume articulation phase, source mux/live scheduling, Stop/seek,
XM articulation extraction, IT NNA/DCT/DCA mixed policies, loop/mapped-sample eligibility,
voice domain budgets and >256 slots, MIDI lanes above wide tracker modules, prepared
sample/DSP rebuild lifecycle, command-generation checks, disabled-feature overhead and
compatibility of legacy host transport. Identify repository evidence for each finding.
Return severity, affected section, concrete failure scenario, suggested resolution and
whether the finding blocks implementation. Do not invent existing APIs or assume M11's
aspirational instrument behavior is implemented. Do not rewrite the plan wholesale.

## Task review prompt

Review all `plans/engine/NO-task-*.md` and `plans/apps/NO-task-W1-web-controls.md`
against the corrected NO-master-plan.md and repository source. Do not modify files.
For each task assess fresh-agent executability, dependencies, least-capable worker,
shared-file concurrency, interface ownership, omitted acceptance tests and scope creep.
Check that the union implements the master without contradictory semantics or holes.
Pay special attention to the articulation/transport seam, feature forwarding and
format dependency cycles, IT extra-slot bounds and stealing eligibility, loop release
expectations, live note ownership and source collision, and web rebuild/playlist paths.
Return severity-ranked actionable findings with file/section and source evidence.
Distinguish a design defect from an implementation detail safely delegated to the worker.

## Evidence recorded during planning

- Repository inspected at base 267c559; master/task plans use source findings, not an
  external review. Claude binary exists at `/home/scott/.local/bin/claude`; not invoked.
- Original worktree's unrelated untracked files were left untouched.
- Planning-only checks: Markdown local links, task section/header coverage and
  `git diff --check` (results recorded in the task-planning commit handoff).
- No builds, playback tests, device tests or external review results claimed for docs.

## Findings and disposition

Initial review completed 2026-10-27 09:41:51 UTC (17:41 Australia/Perth).
The CLI returned success, 71 turns, no permission denials, and modelUsage identifying
`claude-opus-5-5`. Invocation explicitly used `--effort high`; the reviewer could not
independently observe effort. Read-only Read/Glob/Grep tools only; no nested agents.

Accepted A1–A15 and B1–B12, with these qualifications:
- A2: choose 17 live lanes (16 shared jam/selected + one button), admitting 64 tracker
  lanes rather than accepting an audition limitation on wide modules.
- A3/A10: separate N+C+L quotas; live NNA obeys live oldest-first exhaustion. The 64
  custom cap may produce fewer tails than native IT; document this instead of promising
  custom polyphony can never be lower.
- A4: retain the current fade attenuation while paused but freeze its elapsed time and
  settlement; interactive Stop cancels it. Do not claim a separate per-source fade bus.
- A7/B6: one rack-level runtime; its interface lands with two live implementations in
  NO-06. NO-02 supplies concrete format computations, not a speculative live trait.
- A9: new cleanup semantics apply to interactive seek only and preserve audition. The
  pre-existing legacy IT seek/reset hazard is explicitly a separate follow-up.
- B7: metadata instance supplies descriptive metadata, audio instance confirms installed
  capabilities; negotiated atomic records carry build generation.
- B13 is not accepted as a tier blocker: the owner explicitly requested Sol workers or
  Astra for complex tasks and Sol xhigh coordination, which takes precedence over the
  general repo delegation default. No speculative ranking of models is necessary.

Source checks confirmed fixed 64 lanes/272 existing pool, IT ownership-based quota and
victim scan, host fade-to-Stop/seek behavior, absolute song_origin, FrameClock reset dropping
fractional remainder, and the existing 1 ms MIDI clock architecture. The master and task
amendments assign every accepted issue to an owner and verification cases. No code or
playback behavior changed; focused re-review is still required.

### Initial reviewer report (verbatim)

This was a read-only review. I made no edits, wrote no plan file, spawned no agents and made no commits. **Reviewer:** `claude-opus-5-5`. I can't see the effort setting from inside the session, so record that as "not observable" in the review log.

**Verdict:** the master's eight "verified starting points" (lines 26–37) all check out against base `267c559`. Four master findings block implementation:
- how live voices learn the tracker's tick rate (A1);
- where live input lanes go when a module uses all 64 channels (A2);
- whether custom tails count against IT's 256-voice limit (A3);
- the host-side transport layer, which the plan never mentions (A4).

Three task findings also block (B1–B3). Everything else can be fixed in the task text or safely left to the implementer.

---

## Part A — Master plan and foundation amendments

### A1 · High · BLOCKING — Nothing says how live/audition voices get the tracker's tick rate
**Section:** Transport and clocks; Audition and UI.

**Evidence:**
- `MidiSource` runs its own 1 ms clock and calls `rack.control_tick` on it (`crates/starplayer-engine/src/instrument.rs:1089-1124`, `:1159-1173`).
- `Instrument::control_tick(&self, channel, …)` is called once per MIDI channel (`instrument.rs:189`, `:927-933`).
- Architecture §5.4 says this explicitly needs revisiting "when M11 gives MIDI instruments envelopes and the two rates have to agree" (`01-technical-architecture.md:673-683`). The NO amendment doesn't touch §5.4.
- The master only defines the cadence for song voices during Pause, plus "before playback use initial cadence".

**Failure scenario:** an XM piano is previewed while the song plays at 125 BPM (50 Hz ticks).
- If it ticks on `MidiSource`'s 1 ms clock, the envelope runs about 20× too fast.
- If the worker improvises a coupling instead, the live and song envelopes rejoin at different phases after Resume, and the order of same-frame ticks between mux slots is undefined.

**Fix:** have the master name one engine-owned articulation cadence authority:
- When sequencing, it is driven by the tracker tick.
- When paused, stopped or with no tracker, it is synthesised at the retained (or initial) cadence using the existing integer remainder arithmetic.
- Live sources step on it, after the tracker slot at equal frames.
- Amend §5.4 to match. NO-03 owns the clock; NO-06 consumes it.

### A2 · High · BLOCKING — "Live lanes after module lanes" can't fit the fixed 64-lane limits
**Section:** Audition and UI (live lanes).

**Evidence — the 64-lane limit:**
- `ChannelTable::MAX_CHANNELS = 64`, and `trigger` returns `None` for a lane that doesn't exist (`channel.rs:67`, `:148`).
- Mixer `CHANNEL_MAJOR_GROUPS = 65`, i.e. 64 lanes plus a spill group (`crates/starplayer-mixer/src/voice.rs:491-493`).
- `starplayer_telemetry::MAX_CHANNELS = 64` (`snapshot.rs:15`) and web `TELEMETRY_CHANNELS = 64` (`host-wasm/src/lib.rs:166`).
- Insert targets are packed into 8 bits (`lib.rs:143-152`).

**Evidence — modules can already use all 64:** the XM loader accepts up to 64 channels and IT allows 64.

**Evidence — 48 is public and baked in:**
- `MIDI_CHANNEL_BASE = 48` and `midi_channel` / `midi_channel_index` are public API (`instrument.rs:101-141`).
- Hardcoded consumers: `sequencer.rs:1374-1384`, `telemetry.rs:89-97`, `app.js:1014-1016`, and host-wasm tests asserting `channels[48]` (`lib.rs:1585-1594`).

**Failure scenarios:**
- A 64-channel XM or a dense IT puts its live lanes at 64 and above, so every live note-on silently returns `None`. If NO-01 "rejects unsupported layouts", audition is simply unavailable on the owner's "dense IT" acceptance case.
- The lane count itself is never stated. 16 jam MIDI lanes, plus a distinct selected-instrument destination that must "preserve hardware MIDI channel identity", plus a preview lane, overflows 64 even with a 32-channel S3M.

**Fix (needs a product decision):** either
- (a) under `interactive-audition` only, widen the channel table to 64 tracker lanes plus K live lanes. Live lanes sit at or above `bus_count`, so they route to spill: no tracker inserts, which matches the master. Keep telemetry at 64 tracker lanes plus a separate live summary, and keep the 48 layout when the feature is off; or
- (b) explicitly accept that audition is unavailable above N channels.

Either way, state K and how jam and the selected instrument share lanes.

### A3 · High · BLOCKING — IT custom tails vs the native 256-voice quota and stealing
**Section:** IT policy; Capacity.

**Evidence:**
- `within_voice_quota` counts every IT-owned voice against `VIRTUAL_CHANNELS = 256` (`crates/starplayer-it/src/processor.rs:1382-1392`).
- `choose_victim` will pick any owned non-foreground voice (`:1398-1419`).
- An IT custom tail needs IT articulation, so it will be IT-owned.

**Failure scenarios:**
- In a dense IT with Overlap on one instrument, custom tails eat into the 256 quota and trigger native stealing of authored backgrounds on other instruments. Native behaviour changes for instruments still set to Original, and the master's "64 tails *in addition to* native" isn't true.
- In the other direction: if tails are capped at 64, choosing "Overlap" on an instrument whose native NNA is Continue produces fewer tails than Original does.

**Fix:** decide explicitly. Recommendation: custom-provenance voices are excluded from the quota count and from `choose_victim`, and are governed only by the 64-voice oldest-first budget. Write this into Capacity and test it in NO-05.

### A4 · High · BLOCKING — The host transport layer is absent
**Section:** Transport and clocks.

**Evidence:**
- The native/web `Player` puts a whole-output `GainRamp` plus the song fade after the engine (`crates/starplayer-host/src/transport.rs:31-164`).
- Stop is implemented as "ramp to 0, then `Command::Stop`" (`player.rs:238`, `:283-295`).
- The fade advances on every output frame (`transport.rs:131-138`) and lands as Stop plus `seek(0)`.
- End-of-song arming reads `engine.is_playing()` (`player.rs:266-278`).

**Failure scenarios:**
- If Pause is built on the existing Stop path, output gain goes to 0, so audition and the "voices continue while paused" behaviour are both inaudible.
- If the user pauses during a 10-second end fade, the fade keeps running, lands, and sends a legacy Stop plus a rewind. That kills the audition and moves the song position.

**Fix:** the master should require that:
- Pause freezes the song fade and end-of-song arming, and never ramps output gain.
- Interactive Stop is click-safe without leaving the output gain at zero.
- New `HostCommand` variants live in `crates/starplayer-host` (currently unnamed in both the master and NO-03).

### A5 · Medium-High — The pause mechanism conflicts with absolute-frame song state
**Section:** Transport and clocks.

**Evidence:**
- `song_frame = now − song_origin`, where `song_origin` is an absolute frame (`sequencer.rs:897-900`).
- `FrameClock` holds an absolute `pending_tick_frame` plus a Q32.32 remainder (`starplayer-core/src/clock.rs:71-77`).
- `restart_clock_at` → `reset` **drops the remainder** (`clock.rs:84-87`).
- Live events are stamped against `engine.source_frame` (`player.rs:342`). That clock must keep running during Pause for live input to work.

**Failure scenarios:**
- Pause for 10 s and the progress bar jumps +10 s on resume, which also skews seek-by-frame and end detection.
- If a worker resumes with `restart_clock_at`, the fractional remainder is lost, breaking "preserve integer/fractional tempo timing".

**Fix:** specify that the tracker source shifts `pending_tick_frame` and `song_origin` by the integer paused frame count, which keeps the remainder intact. Keep the live stamping clock monotonic.

### A6 · Medium — Pause semantics for SMF sources are undefined
**Evidence:** `Player::load_smf` plays `.mid` files through `MidiSource` + `SmfSequencer` (`player.rs:678-695`).

**Failure scenario:** a `.mid` keeps playing through Pause.

**Fix:** define which sources are "sequenced" (tracker, SMF) and pause, versus "live" sources that keep running.

### A7 · Medium (blocks NO-06 only) — Live XM/IT runtime ownership vs the `&self` `Instrument` trait
**Evidence:**
- `Instrument` is `&self`, with per-channel state held in the rack, and there is one boxed instrument per program (`instrument.rs:163-194`, `:499-506`, `:644-655`).
- IT per-slot state is pool-sized (`it/processor.rs:596`).

**Failure scenario:** a worker builds one runtime per instrument (99 instruments × roughly 350-slot state arrays), or uses interior mutability, and `RefCell` panics are forbidden in the RT path.

**Fix:** decide on one live format runtime per rack, owning per-slot state and RNG. Two options:
- a new engine trait landed together with both its XM and IT implementations (goal 8 is satisfied because both implementations exist at once); or
- extend `Instrument` with rack-level hooks.

Amend architecture §5.3.

### A8 · Medium — Load generation vs session identity conflict
**Evidence:**
- host-wasm bumps `module_generation` on every load, including enhancement reloads of the same bytes (`host-wasm/src/lib.rs:401-414`; `app.js:1330-1400`).
- `Player::rebuild` replays the insert layout but would replay no overlap state (`player.rs:585-646`).

**Problem:** the master requires both "retain across enhancement rebuilds" and "reject a reload of the same bytes". The web currently implements enhancement changes *as* a reload of the same bytes, so the two rules contradict each other.

**Fix:** use two identifiers — a user session/load id and an engine build generation. Have `Player` shadow the overlap layout and replay it on rebuild, as it already does for inserts.

### A9 · Medium — Seek behaviour change and the `reset()` hazard
**Evidence:**
- Seek never touches voices (`sequencer.rs:979-1004`; `starplayer-host/src/source.rs:182-205`).
- IT `reset()` releases its voice states while the pool voices keep sounding (`it/processor.rs:3122-3124`). Detached looping backgrounds become untracked, which looks like a latent bug today. After NO-02, XM's per-voice state would behave the same way.

**Gaps in the master:** "Seek clears voices" is a behaviour change for legacy hosts too, and it's unclear whether seek also clears audition.

**Fix:** state the scope (all hosts or interactive only), do the clear in `SeekableModuleSource::dispatch` (which has the context), and say whether audition survives a seek.

### A10 · Medium — Capacity accounting is ambiguous
**Evidence:**
- `MAX_VOICE_CAPACITY = 256 + 16` already reserves 16 slots for jam (`channel.rs:60`; arch §5 at `:655`).
- `ItProcessor` sizes its state from that constant, not from the actual pool (`processor.rs:594-597`, `:621`).
- The facade and host build sequencers without knowing the pool capacity (`starplayer-host/src/source.rs:281-293`).

**Open questions:**
- Are the 16 audition voices the existing jam reserve or another 16 on top?
- Does live IT NNA stealing use oldest-preview or IT's quietest-background rule?

**Fix:** give a per-format total formula, and either make the constant feature-dependent or thread capacity through the facade constructors.

### A11 · Medium — Global all-sound-off vs MIDI CC120
**Evidence:** CC120 is lane-scoped by design and is tested to leave the module playing (`instrument.rs:830-838`; `host-wasm/src/lib.rs:1598-1600`).

**Fix:** state that global all-sound-off is a new UI/host command, and that CC120 from hardware keeps its current lane-scoped meaning.

### A12 · Low-Medium — Telemetry freezes during Pause and Stop
**Evidence:** snapshots are published only per tracker tick, or when `MidiSource`'s fingerprint changes (`sequencer.rs:1345-1386`; `instrument.rs:1181-1200`).

**Failure scenario:** VU meters and the voice count freeze while tails ring on, or keep showing cleared voices after Stop.

**Fix:** publish on control-only ticks (rate-limited), or carry transport and voice state outside the snapshot.

### A13 · Low-Medium — There is no ramped pool-wide stop
**Evidence:** `ChannelTable::stop_all` only stops foregrounds (`channel.rs:193-198`). `VoicePool::release_all` is explicitly unramped (`voice.rs:607-627`).

**Failure scenario:** interactive Stop or bypass clears backgrounds and tails with a click.

**Fix:** require a ramped, pool-wide stop for each ownership scope.

### A14 · Low — Foundation documents are stubs
The amendments (`01-technical-architecture.md:1763-1782`, `03-accuracy-policy.md:712-725`) only point at the master. §§5.3, 5.4 and 9 aren't actually amended, but AGENTS.md says foundation docs are the source of truth for "what and why". Amend them alongside the decisions in A1–A4.

### A15 · Low — Small wording and verification issues
- "non-looping PCM, including sustain loops" can be read either way. NO-04 treats sustain loops as ineligible; the master should say so plainly.
- "Include mid-quantum commands" conflicts with the existing rule that commands apply at quantum boundaries (`engine.rs:566-571`; `player.rs:199-203`). Only stamped live events land mid-quantum, so say that.

---

## Part B — Task specs

### B1 · High · BLOCKING — The articulation seam has no owner (NO-02 / NO-03 / NO-06)
**Evidence:** articulation currently runs inside `TrackerProcessor::row` / `tick` (XM: `processor.rs:2038-2046`, `:2064-2068`). `PatternSequencer` is generic over that trait (`sequencer.rs:513-572`, `:1416-1446`).

**Gaps:**
- NO-02 offers "concrete-type" control steps, but the sequencer can only reach them through a trait method, such as a provided `TrackerProcessor::control_step` that does nothing by default. No task owns adding it.
- No task owns the live cadence from A1.

**Fix:**
- NO-02 adds the trait method plus the XM/IT implementations.
- NO-03 owns the cadence clock and dispatching ticks during pause.
- NO-06 consumes both.

### B2 · High · BLOCKING — NO-03 omits `crates/starplayer-host`
**Gap:** "facade/host interception" doesn't name `player.rs`, `transport.rs`, `events.rs` or `source.rs`.

**Missing tests:**
- pause during the end fade (A4);
- the paused offset for `song_origin` (A5);
- pause surviving `replace_source`, jam toggles and mixer-mode rebuilds. Jam toggles send `LoadModule`, which calls `release_all` (`player.rs:886-916`; `engine.rs:471-478`), and rebuild only replays Play (`player.rs:641-643`; `app.js:1272-1292`).

**Fix:** name the files and add these deliverables and tests.

### B3 · High · BLOCKING — NO-01's lane-layout deliverable can't be executed until A2 is decided
Its "no broad MIDI refactor" out-of-scope line also conflicts with changing the `MIDI_CHANNEL_BASE` consumers listed under A2. Resolve A2 first, then list every consumer that changes.

### B4 · Medium-High — Cleanup integration has gaps and double ownership
- NO-03 lands before custom tails and previews exist.
- NO-04 and NO-05 don't require interactive Stop, all-sound-off or Stop-preview to clear tail ownership metadata.
- NO-03 and NO-06 both claim the Stop-preview and all-sound-off plumbing.

**Fix:** NO-03 provides scoped clear hooks. NO-04, NO-05 and NO-06 each register their cleanup and add tests for "Stop/seek/bypass with tails and previews active". Give Stop-preview to NO-06 alone.

### B5 · Medium — Capacity plumbing has no owner
**Gap:** NO-01 "specifies", NO-02 "sizes IT storage", but no task wires the pool size into `HostEngine::build`, `MAX_VOICE_CAPACITY`, `NativeSequencer::new` / `build_source`, or XM's new per-slot storage.

**Fix:** assign this to NO-02, together with the A10 decision.

### B6 · Medium — NO-06's research point leaves a design decision to the worker
"Inspect trait object/context limitations… no speculative new abstraction" may push the worker towards per-instrument runtimes.

**Fix:**
- Embed the A7 decision in the task.
- List the `InstrumentRack::for_module` call sites the facade must replace (`player.rs:683`, `:733`, `:861`).
- Add a test that live audition doesn't advance the song's RNG.

### B7 · Medium — NO-W1 misses files and wire constraints
- Instrument metadata comes from the metadata wasm instance `apps/starplayer-web/src/lib.rs:462-480` (see `app.js:1827-1829`). That's the natural source for loop and mixed-map badges, but the task routes them through the worklet.
- `WireCommand` has only `opcode: u8, argument: u32, extra: u32` (`host-wasm/src/command.rs:17`), with no room for a generation number. Packing needs a decision.
- Telemetry header word 15 (`is_playing`) and `app.js:2052` (`endReachedStopped`) need a defined meaning for Paused.

### B8 · Medium — Feature-matrix evidence
In NO-01 and NO-07, a workspace-wide build or `cargo xtask ci` merges host-wasm's enabled features into every crate, so it can't prove anything about the feature-off build.

**Fix:**
- Require per-package `-p … --no-default-features --features …` builds plus `cargo tree -e features`.
- Add `cargo test -p starplayer-firmware-common` (host-run, no Xtensa needed) in the embedded workspace as a cheap check that nothing optional reaches embedded.

### B9 · Low-Medium — NO-02 under-specifies XM ordering for background voices
**Evidence:** XM interleaves effects → articulation → flush per channel, and reads `global_volume` inside the articulation step (`xm/processor.rs:1556-1560`, `:2038-2046`). A mid-row `Gxx` therefore affects later channels in the same tick.

**Fix:** state where background-voice steps run, and test `Gxx` with tails active (that belongs in NO-05).

### B10 · Low-Medium — Two provenance tables risk
NO-04's "reusable engine helper" and NO-05's IT/XM per-voice state could each keep their own provenance and age table. Name one source of truth per format.

### B11 · Low — Fresh-agent executability
The tasks are terse and mostly shared boilerplate. Several lack:
- concrete file lists (NO-03, NO-06, NO-W1);
- "interfaces provided/consumed" sections (for example, which types and commands NO-01 hands to later tasks).

Add both. This is a quality issue, not a blocker.

### B12 · Low — NO-07's verification assigns owner listening to a worker
"Manual browser listening matrix" is owner acceptance. A worker can produce the fixtures and renders; the owner does the listening. As written, NO-07 can't be completed.

### B13 · Low — Worker tier can't be verified
AGENTS.md step 3 says delegate to GPT-6.1-sol, while four task headers name `gpt-6-astra`. That's allowed by the header rule if astra is at least sol's tier, but nothing in the plan states their relative capability.

---

## Dependencies and completeness

- **No dependency cycles.** The table order 01 → 02 → 03 → 04 → 05 → 06 → W1 → 07 is consistent.
- **Concurrency risk:** NO-03 and NO-04 both edit `sequencer.rs` and `channel.rs`. The coordinator must serialise them, which the plan already acknowledges.
- **Holes in the union of all tasks** (nothing owns these):
  - the live cadence (A1/B1);
  - the host transport layer (A4/B2);
  - the lane layout (A2/B3);
  - IT quota membership (A3);
  - SMF pause (A6);
  - telemetry while paused (A12);
  - pause surviving rebuild and jam toggle (B2);
  - capacity plumbing (B5);
  - cross-feature cleanup (B4).

## Safely left to implementers

Exact type and opcode names and values, diagnostics counters, trace line formats, fixture construction, UI layout and badge styling, ramp mechanics (once A13's direction is set), and per-sample eligibility evaluation at detach time (once A15's wording is settled).


### Follow-up review and second corrections

Recorded 2026-10-27 09:59 UTC. Exact model `claude-opus-5-5`, invocation
`--effort high`; CLI success, no implementation. The report below found remaining
integration gaps. Accepted 1–11. Changes: permanent live mux slot and routing-only input
selection; audible initial/natural-end interactive states; correctly rebased held seeks;
fixed live base 64 with separate insert bus count; runtime opt-in preserving constants;
N+C+L+R physical sizing with one retirement slot per domain and explicit oldest-retiring
pressure fallback; exact-frame post-source cadence hook; independently gated ownership;
retained ownership on legacy seek; clarified runtime interface; NO-05 depends on NO-03.
The pressure fallback is an explicit bounded compromise: it may shorten an already
retiring ramp but does not delay new notes or steal another domain's voices.
Focused re-review of these changes pending. No approval claimed from this report.

I found 11 remaining defects, so the master and task plans are **not yet ready for implementation-planning approval**. Seven of them (1–7) block that approval; the other four (8–11) can be fixed alongside. I made no edits and ran no commands. The plan file couldn't be written because the Write tool is disabled in this session, so the full report is here.

Resolved, and not repeated below:
- A3, A4, A6–A8, A10–A15;
- A5 for plain pause;
- B1, B2, B4 (except item 9), B5, B7–B12;
- the atomic wire-generation record and the XM per-channel step ordering.

## Blocking defects

**1. High — Audition needs a live input source, and the only way to get one today disrupts the song.**
- **Where:** master "Audition and UI" and "Host transport"; NO-03 Interfaces ¶4; NO-06 Interfaces ¶1.
- **Evidence:** live input exists only through `Player::jam` or `midi_only` (`starplayer-host/src/player.rs:859-916`). Both rebuild the sequencer at the telemetry song frame, which restarts the clock and resets effect state (`sequencer.rs:952-956`). Both then send `LoadModule`, whose `release_all` cuts every voice without a ramp (`engine.rs:774-784`).
- **Failure:** the first preview while paused, or switching to the selected-instrument target, loses the pause remainder and effect memories, snaps the cursor to a timeline mark, and cuts all song voices and tails.
- **Fix:** with `interactive-audition` on, `Player::install` always builds a two-slot mux: the sequencer in slot 0 and one live `MidiSource` with all 17 lanes in slot 1. Jam, selected-instrument and Preview become routing state set by bounded commands, never a source replacement or `LoadModule`. Keep the legacy paths when the feature is off.
- **Owners and test:** NO-06 owns the rack and slot. NO-03 owns the Player invariant and a test that a first preview while paused leaves row, tick, remainder and song voices untouched.

**2. Medium-High — Before the first Play and after a natural end, output is silenced.**
- **Where:** master "Transport and clocks" table and "Host transport"; NO-03 Deliverables.
- **Evidence:**
  - `Player` opens with `Transport::stopped()` (gain 0) and the engine not playing (`player.rs:410`, `:857-858`).
  - At natural end, `arm_end_of_song` calls `begin_stop(true)` and `settle_transport` sends the legacy `Command::Stop` plus `seek(0)`, leaving gain at 0 (`player.rs:266-295`, `transport.rs:53,86-90`).
- **Failure:** audition is inaudible in both states. That breaks the master's "before playback use the module's initial cadence" and "after natural sequence exhaustion … synthesises steps".
- **Fix:** in interactive mode, start in interactive Stopped at unity gain. On natural end (Stop policy or a landed fade), run the clear-and-hold path into interactive Stopped with a rewind armed for the next Play, and restore unity gain after the clear. Legacy paths stay unchanged.
- **Owners and test:** NO-03 owns this. W1 adds a test that preview is audible before the first Play and after the song ends.

**3. Medium-High — Seeking while Paused or Stopped breaks the held-time translation.**
- **Where:** master "One articulation cadence" ¶3 and "Interactive seek"; NO-03 Interfaces ¶2–3.
- **Evidence:** `SeekableModuleSource::dispatch` restarts the clock at the seek frame and immediately dispatches a due tick (`starplayer-host/src/source.rs:189-204`). The seek sets `song_origin = now − mark.frame` (`sequencer.rs:962`).
- **Failure:** pause at P, seek at S, resume at R:
  - one row plays while held;
  - translating by R−P on resume delays the first tick by S−P;
  - progress reads S−P too early.

  Seeking while Stopped is the web's common case.
- **Fix:**
  - A held seek repositions without dispatching a tick and zeroes the held remainder.
  - It rebases the held baseline to S, so resume translates by R−S.
  - It sets the frozen progress cursor to `mark.frame`.
- **Owner and test:** NO-03 adds tests for seek-then-resume from both Paused and Stopped.

**4. Medium-High — Basing live lanes on the module's channel count puts them on insert buses.**
- **Where:** master "Lane layout" ¶1; NO-01 Deliverables ("dynamic live-lane layout"); arch §9 amendment.
- **Evidence:** the persistent host builds a 64-lane table (`starplayer-host/src/engine.rs:180-181`), and the engine sets `bus_count = channels.len()` (`engine.rs:313-317`).
- **Failure:** a 4-channel MOD's live lanes 4–20 get buses and insert chains. Widening the table to 81 would create 81 buses.
- **Fix:** use a fixed interactive live base of 64 (lanes 64–80) for every module width. Decouple the bus count from the table length, capped at 64. That change is in `engine.rs`, so name NO-01 or NO-03 as its owner. "Spill only" then holds by construction, and W1 never has to renegotiate the layout per load.

**5. Medium — Compile-time widening would leak into legacy hosts and CI.**
- **Where:** master "Lane layout" ("widen only the feature-enabled ChannelTable limit") and "Capacity"; NO-02 Interfaces ¶2.
- **Evidence:** `cargo xtask ci` runs `cargo test --workspace` (`xtask/src/main.rs:1245`) with resolver 2, and `apps/*` and host-wasm are workspace members. The web features are therefore switched on for every crate in that run, including native hosts and tests that assert 272, 48 or 64.
- **Fix:** features gate capability only. The lane count and the N+C+L budgets become runtime `EngineSettings`/host configuration that defaults to legacy values even when compiled in. Keep `MAX_VOICE_CAPACITY`, `ChannelTable::MAX_CHANNELS` and `MIDI_CHANNEL_BASE` unchanged and add separate interactive constants. Only the web host opts in at runtime.

**6. Medium — Counting retiring voices against the budget contradicts eviction at exhaustion.**
- **Where:** master "Host transport" ¶2, "Capacity" ¶2 and "Lane layout" ¶2–3; NO-04 Interfaces; NO-06 Interfaces ¶2.
- **Evidence:** the pool is exactly N+C+L with no headroom, and a stopping voice holds its slot for `RAMP_FRAMES = 64` (`starplayer-mixer/src/gain.rs:72`; `voice.rs:393-407,485`).
- **Failure:** evicting the oldest voice at a full cap can't admit the new note immediately. With live IT NNA tails filling L=16, every other key press is refused. After an interactive Stop every slot is retiring, so notes played in the next 64 frames are refused.
- **Fix:** count admission against non-retiring voices only. Size the pool N+C+L+R with a small per-domain retire reserve; when R runs out, hard-release that domain's oldest already-retiring voice.
- **Tests:** a saturated domain admits every note, and a note immediately after Stop sounds.

**7. Medium — Nothing defines how cadence steps reach the live source.**
- **Where:** master "One articulation cadence" ¶2; NO-03 Interfaces ¶1; NO-06 Interfaces ¶2; arch §5.4 amendment.
- **Evidence:** `MidiSource` is a separate mux slot whose `next_event_frame(&self)` sees only its feed and its 1 ms clock (`instrument.rs:1204-1209`). The engine loop has no step after all sources have dispatched (`engine.rs:841-872`).
- **Failure:** live articulation steps would land on the next 1 ms dispatch rather than on the tick frame, which violates the equal-frame ordering.
- **Fix:** NO-03 feeds the engine cadence's next step frame into `frames_until_next_event`. At a step frame, after all sources have dispatched, the engine calls a new provided method on `EventSource` (no-op by default, forwarded by `SourceMux` in slot order). This extends an existing trait rather than adding a new one.
- **Owners and test:** NO-03 adds a scripted-consumer test; NO-06 implements the method in `MidiSource`.

## Smaller defects

**8. Low-Medium — The ownership/age record is gated only by `note-overlap` (NO-01 Deliverables bullet 2).** Audition-only builds need the live domain and age records for oldest-live eviction and IT quota isolation. Compile the record when either feature is on, and gate only the custom-provenance fields under `note-overlap`.

**9. Low-Medium — NO-04's "Seek/restart … clear ownership" re-creates orphan tails (Deliverables bullet 6).** It contradicts the master's "legacy seek unchanged", and dropping ownership while tails still sound leaves the custom-tail budget (C) under-counted and puts those tails beyond bypass's reach.
- On legacy seek or restart with overlap enabled, either ramp-clear the custom tails (they aren't canonical) or keep their ownership.
- Never forget ownership of a voice that is still sounding.

**10. Low — NO-06 Research point 1 contradicts NO-06's own Interfaces section.** It still says to avoid "a speculative new abstraction", but the decided design is a rack-level runtime interface. Reword it to: land that interface with both implementations, no per-program runtime, no engine→format dependency.

**11. Low — NO-05's "Depends on" omits NO-03,** although NO-05 consumes NO-03's clear hooks. Add NO-03, or copy NO-04's coordinator-integration clause.

## Next step

Amend the master, NO-01 to NO-06 and the arch §5.4/§9 text for these items. Record this report and its dispositions in `NO-review-log.md`, then get a focused re-review of items 1–7. After that, the plans can be put forward for implementation-planning approval. That approval would still not authorise implementation.


### Final focused review and disposition

2026-10-27 10:02 UTC. Exact model `claude-opus-5-5`; `--effort high` supplied,
CLI success. This pass had no tools and checked the current master/tasks against the
previous source-backed findings; it did not repeat a whole-repository source review.

The reviewer resolved all eleven substantive follow-up findings and made approval
conditional on replacing NO-06 Deliverables bullet 3. Its exact semantic replacement is
now applied: fixed lanes 64–80 and one shared live domain of 16 active plus one retirement
slot, including jam, selection, button preview and live NNA tails. Applied all three
optional wording clarifications too: natural-end armed rewind is in scope, slot-0 held
song dispatch precedes live events while the post-source hook advances live articulation,
and NO-01 includes R. Confirmed architecture §§5.4/9 already state fixed base 64,
decoupled bus_count and the post-source hook. No additional design change was made after
this conditional verdict; no fourth model pass was necessary to verify the text replacement.

**Outcome:** master and all eight tasks are ready for the requested Sol xhigh coordinator
handoff, pending explicit owner authorization to implement. Worker assignments remain
Sol/Astra as requested. No implementation, builds, playback tests or hardware claims.
Documentation checks: local Markdown links/task sections and `git diff --check` pass.
Planning-only commits preserve the complete findings and accepted/rejected dispositions.

### Final reviewer report (verbatim)

# Focused re-review: dispositions for findings 1–11

1. **Live input source: Resolved.**
   - The master's Host transport ¶2 defines the permanent two-slot mux: slot 0 sequenced, slot 1 with all 17 lanes, installed at module installation.
   - Routing changes never call `jam()`/`midi_only()` or `LoadModule`.
   - Ownership is split cleanly: NO-03 owns the invariant and NO-06 installs the rack.
   - Tests for first preview while paused, before first Play and after natural end are in NO-03, NO-06 and W1.
2. **Silent before first Play and after natural end: Resolved.**
   - Interactive Stopped starts at unity gain with the live path running.
   - Natural end clears into Stopped, restores unity gain and arms rewind for the next Play.
   - User Stop cancels the pending rewind.
   - Resume does not take the legacy fade-restart branch.
3. **Held-time seek: Resolved.**
   - A held seek dispatches no tick, zeroes the held remainder, rebases the hold to S, translates by R−S and freezes the cursor at the mark.
   - This is consistent with "Resume executes the next pending sequence tick after its saved remainder".
   - Tests cover Paused, Stopped and repeated seeks.
4. **Live lane base: Resolved in the master and NO-01.**
   - Live lanes are fixed at 64–80.
   - NO-01 owns decoupling `bus_count` from table length, capped at 64.
   - NO-01 tests both a 4-channel and a 64-channel module.
   - One stale line remains in NO-06 (see the blocker below).
5. **Compile-time widening: Resolved.**
   - Features gate capability only; the web host opts in at runtime.
   - The legacy constants 64, 48 and 272 and the legacy constructors are preserved.
   - "Runtime defaults leave all optional budgets zero."
   - This is restated consistently in NO-01, NO-02, NO-03, NO-07 and W1.
6. **Retiring voices vs eviction: Resolved.**
   - The pool is N+C+L+R with one retirement slot per configured domain, 339 slots for web.
   - Retiring voices hold ownership but not active quota.
   - The admission sequence is implementable: hard-release the domain's oldest retiring voice, retire the active victim, then admit. A physically full domain with no retiring voice cannot occur.
   - Tests are assigned in NO-03, NO-05 and NO-06.
7. **Cadence reaching the live source: Resolved.**
   - The cadence frame takes part in event splitting.
   - A provided post-source method on the existing `EventSource` trait is forwarded by `SourceMux` in slot order. This is a trait extension, so it does not conflict with design goal 8.
   - NO-03 has a scripted-consumer test and NO-06 implements the method.
8. **Ownership record gating: Resolved.** NO-01 bullet 2 and the master's capacity section compile the record when either feature is on, with only the custom-policy fields gated by `note-overlap`.
9. **Legacy seek orphan tails: Resolved.**
   - The master, NO-04 bullet 6 and NO-05 now retain ownership of sounding custom tails.
   - NO-04 picks the "retain" option the master allows, so the two do not conflict.
10. **NO-06 Research point 1: Resolved.** It now states the decided rack-level interface.
11. **NO-05 dependencies: Resolved.** NO-05 now depends on NO-02, NO-03 and NO-04, and the master's table matches.

## Remaining blocking contradiction (one, NO-06 wording residue from items 4 and 6)

NO-06 Deliverables bullet 3 still says lanes are allocated "after module channels". That reintroduces the dynamic lane base removed by finding 4.

The same bullet gives preview "its own ownership domain and 16-voice cap". That contradicts the master's "L replaces/includes the existing 16 jam reserve; it is not 16 jam plus another 16 preview". It also contradicts NO-06's own Interfaces section, which says "The 17 lanes share 16 live voice slots".

A worker following that bullet could implement either the dynamic base or a 16 + 16 budget.

**Replace that bullet with:**

> - Use NO-01's fixed interactive live lanes 64–80 for every module width (16 MIDI-identity lanes shared by jam/selected input, plus lane 80 for the Preview button). All live input, including the Preview lane and live NNA tails, shares the single live domain L (web 16 active slots plus one retirement slot), separate from tracker/native and custom-tail domains. Oldest-live stealing is deterministic; format states cover the actual pool slots.

## Non-blocking clarifications (optional, can be fixed during task kickoff)

- **NO-03 Out of scope says "No automatic rewind".** This could be read as conflicting with "arms rewind for next Play" after natural end. Suggested wording: "No automatic rewind on user Pause/Stop (the natural-end armed rewind is in scope)".
- **Held song control steps and the post-source hook.** The master requires held song articulation to step *before* live events at equal frames. The post-source hook runs *after* source dispatch. Add to NO-03 follow-up bullet 1: "Held song control steps run in slot 0's own dispatch, before live events; the post-source hook is for live articulation only."
- **NO-01 Deliverables bullet 4** lists only "native + 64 + 16". Append "+ per-domain retirement reserve R (master)" so it matches NO-01's own Interfaces section.
- **Architecture §5.4/§9 text was not supplied in this pass.** The coordinator should confirm it states the fixed 64-base layout, the decoupled `bus_count` and the post-source hook, matching the master.

## Verdict

Once the NO-06 bullet 3 replacement above is applied, no blockers remain. At that point, the master and task plans are ready to hand to the implementation coordinator, pending owner authorization to implement. The task headers' "external task review pending" status and the review log should then be updated to record this disposition.
