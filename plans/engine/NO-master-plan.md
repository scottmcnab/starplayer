# NO — Interactive note overlap and instrument audition

| Field | Value |
|---|---|
| Status | Planning complete enough for external review; implementation NOT started or authorised by this document |
| Created | 2026-10-27 |
| Branch / worktree | `feature/note-overlap-audition` / `../starplayer-note-overlap-audition` |
| Goal | Optional per-instrument/per-channel note overlap in MOD/S3M/MTM/XM/IT; faithful audition; sequencer-only Pause |
| Coordinator | `gpt-6.1-sol`, reasoning `xhigh` |
| Workers | `gpt-6.1-sol` for bounded tasks; `gpt-6-astra` for clock/articulation/NNA work; see task headers |
| External review | Claude CLI, requested Opus-5.5, high effort; master and task reviews deferred until after 2026-10-27 16:20 Australia/Perth (08:20 UTC), at owner's request |
| Related | M4 live input, M5 XM, M6 IT, M10-K5 sample enhancement, M11 instrument library |

## Context for a fresh coordinator

Read `AGENTS.md`, `plans/README.md`, product architecture §§5 and 9, and the accuracy
policy. This is a reusable engine feature with a web UI, not a module-file editor.
Historical `STARPLAY/` and `STARPLAY-2.25s/` are read-only. Do not run cargo fmt.
The original checkout contains unrelated untracked files; never stage them.

The owner wants to hear truncated sample decays and optionally allow them to finish.
A snare should survive the next kick on its channel; piano C then G should overlap,
with an optional rule that another C cuts earlier C voices on that channel. Defaults
must preserve canonical output. This cannot reconstruct audio absent from the sample.

Verified starting points (2026-10-27, base `267c559`):
- `engine/src/channel.rs`: trigger releases the old foreground; detach preserves it.
- `engine/src/sequencer.rs`: TickContext wraps triggers; new notes, retriggers and
  sample replacement currently share paths. Do not treat every trigger as a new note.
- MOD/S3M/MTM reserve channel-count voices; XM articulation is channel-owned and never
  detaches; IT owns per-voice articulation, NNA/DCT/DCA and bounded stealing.
- `engine/src/engine.rs`: playing gates both mixing and source dispatch. ControlClock's
  synthesised tick counter alone does not implement format articulation callbacks.
- `engine/src/instrument.rs`: MappedInstrument maps samples but does not provide native
  XM/IT envelopes, release or fadeout. M11's aspirational language is not implemented fact.
- Web app has keyboard/MIDI jam mode and a read-only Instruments list. MIDI lanes start
  at 48 today; do not assume that is disjoint from all wide XM/IT tracker channels.

## Product contract

### Transport and clocks

Web transport is Play, Pause, Stop, in that order. Pause has an accessible pause icon,
pressed state and a Resume action. Stop holds position, never implicitly rewinds.

| State | Sequencing | Existing voices / DSP | New live input |
|---|---|---|---|
| Playing | Advances normally | Advances | Accepted |
| Paused | Frozen, including remainder to next tick | Advances | Accepted |
| Stopped | Held at same sequence position | Stop clears voices and DSP tails | Accepted after Stop |

Keep audio/event time monotonic. Preserve tracker position, row/tick effect memories,
and remaining sample-frame distance to the next sequence event. Pause does not run
row effects, slides, retriggers or pattern flow. Voice envelopes, fadeout and auto-vibrato
continue at the last active tracker cadence, with the existing integer timing/remainder
arithmetic (including non-integer tick lengths); before playback use the module's initial
cadence. Preserve modulation state owned by the voice, not continuing tracker effects.
Control-only ticks and sequencing ticks must not advance articulation twice on resume.
Resume executes the next pending sequence tick after its saved remainder, and joins
articulation to that tick once; it does not replay missed sequencing events. This
intentional phase rejoin can shorten/lengthen one articulation interval after pause.

Stop clears bindings, native and enhanced backgrounds, audition held keys/pedals and
DSP delays without resetting sequence/effect position. While stopped, only newly played
live voices advance. Resume does not resurrect cleared voices; effects may operate on
silent channels until a subsequent note. Global all-sound-off clears all sound without
changing transport state. Pause is not sample-state freezing: loops can sustain and
one-shots can finish before resume. Do not represent Pause as natural song completion.
Keep legacy `Command::Stop` behavior for unmodified hosts; add explicit interactive
transport commands so the web Stop can select the new clear-and-hold behavior.

### Overlap policies

Store overrides outside immutable Module data; session-only, reset on different module,
retained across pause, seek and renderer/DSP/sample-enhancement rebuilds of the same load.
Use module load generation to reject stale commands (including a reload of the same bytes).

Instrument: Follow channel (default), Original, Cut, Overlap, Overlap different notes.
Channel: Original (default), Cut, Overlap, Overlap different notes.
Explicit instrument policy wins. Original means native format behavior, including IT
runtime NNA changes. It can exempt an instrument from a channel override.

The outgoing voice's instrument determines survival. The incoming instrument/note drives
duplicate matching. Different-notes mode cuts all matching voices for that instrument
and original trigger note in the same originating channel, not just its foreground.
Notes on other channels remain independent; slides do not change trigger identity.

For IT, custom policy replaces the affected voice's authored NNA/DCT/DCA behavior; do
not run both. Native-policy voices retain native matching/action behavior. Evaluate
policy per candidate during duplicate scans; custom Overlap has no native duplicate
scan, Different-notes uses the explicit match, Cut has no background-duplicate action.
Native foreground processing still applies to the outgoing voice when incoming and
outgoing instruments differ. Tag detachment provenance separately from current policy
so bypass can clear custom-created tails without deleting authored IT background voices.

Only new musical note starts (including actual delayed starts) detach. Portamento,
effect retriggers, sample-only changes and queued swaps retain native semantics.
Detached voices keep their actual sample, pitch/volume/pan and format articulation;
subsequent channel effects target foreground only. Preserve originating channel routing,
mute and inserts. Explicit tracker cuts target foreground; native IT background-control
commands retain their authored semantics. A pending MOD swap must not make a custom
one-shot tail loop: detach the current non-looping region and discard its queued swap.

Custom Overlap / Different-notes applies only to actually non-looping PCM, including
sustain loops. Ineligible voices use native behavior, NOT forced Cut. Cut/Original remain
available for loops. IT's authored looping NNA remains intact. For mapped XM/IT instruments
check the note's actual sample; show a mixed-loop badge rather than disabling all notes.
User changes affect subsequent note events. Global overlap bypass clears custom-created
tails immediately and restores native policy; enabling does not recreate lost tails.

### Audition and UI

Instrument rows gain a Preview button, selection, Note overlap dropdown and loop status.
Use infinity with accessible text, and distinguish all-looping from mixed-map instruments.
Disable Preview for empty/unmapped/non-PCM slots. Do not implement S3M FM synthesis here.
Show why overlap is unavailable; use explicit labels, never a blank default.
Channel rows gain Note overlap beside mute. A global enable switch starts disabled.

Selecting an instrument sets an explicitly armed keyboard/MIDI target. Keep jam and
selected-instrument destinations distinct; no duplicated input. Preview sounds over
playback and while paused/stopped. One-shot button previews run to completion, whereas
looping button previews release on pointer/key release; stop/release also on pointer
cancel, focus loss, input disconnect, destination or instrument change and module load.
Offer Stop preview distinct from global all-sound-off.

Audition uses currently prepared PCM (including enabled sample enhancement), tuning,
format note maps/envelopes/filter/NNA and shared master DSP. Dedicated live-input lanes
have no implicit tracker-channel inserts: an instrument has no unique tracker channel.
Use existing reference-pitch convention (MIDI 60 for simple samples); mapped instruments
use their native mapping at that note. No raw/DSP-bypass switch in v1. Keyboard/MIDI
note-off invokes native release; simple samples use the existing click-safe release.
For a looped XM/IT voice with no terminating release, Stop preview is always available;
never claim every native note-off guarantees silence.

Allocate live lanes after module lanes with an explicit address mapping; never hardcode
48 as universally safe. Preserve hardware MIDI channel identity and tracker VoiceTag
limits. Preview voices do not consume tracker foreground reservations. Live note-offs
must locate held note instances even after NNA detachment; sustain, repeated keys and
program changes must not leave stuck voices.

### Capacity, real-time behavior and feature flags

Preallocate web budgets of 64 custom tails and 16 audition voices, in addition to native
format foreground/background requirements. These are caps in the global pool, not new
mixers. Validate IT's per-slot state sizing when total capacity exceeds 256. Never leave
an allocated voice outside the format state's addressable range. Host budgets are
configurable before playback; no allocations on toggle, dispatch or render.

Steal oldest custom tail within its budget, stable slot-order tie-break; fallback to
ordinary foreground replacement if custom allocation cannot succeed. Steal oldest
preview within preview budget; never unrelated tracker foreground. Native IT stealing
keeps its native policy over its eligible voice domain. Generation-check ownership.
Count steals/refused events for diagnostics. Use existing release ramp; do not add RT
transcendentals. Retire module/rack ownership off the audio callback.

Cargo features `note-overlap` and `interactive-audition` are off by default in reusable
crates, forwarded through facade/format crates and explicitly enabled by web host defaults.
Neither enables std. Each builds independently; audition without overlap uses native
instrument behavior, overlap without audition has no input-lane reservations. Runtime
controls advertise compiled capabilities. Embedded omits both: no override tables,
custom-tail/audition reservations or optional dispatch branches. Existing native IT NNA
must NOT be gated by these features. Small transport types may be unconditional; gate
interactive machinery under `interactive-audition` and keep existing host semantics.
Do not introduce a new trait before two real implementations exist.

## Delivery and review workflow

No implementation starts until explicitly requested. Planning commits are not evidence
of external approval. First commit this master, index and planned foundation amendments;
then commit executable task specs and the review handoff separately.

Deferred review has two passes: master first, then tasks against the corrected master.
After the specified reset, use the installed `claude` CLI in this sibling worktree in
read-only review mode. Inspect `claude --help` and locally supported model selection;
request exactly Opus-5.5 with high effort. Do not silently substitute an alias/model or
claim it is available. If rejected/unavailable, record the exact limitation for the owner.
Give it repo context and file paths, request severity-ranked findings with evidence and
proposed fixes, and prohibit edits/implementation. Capture review text in the review log,
resolve findings agreed with, explain rejected findings, and seek focused re-review of
material amendments. Never include session URLs or attribution in commits/PRs.
The owner deferred invocation; do not probe the paid model before the reset. No automatic
scheduled invocation is installed by this plan.

Implementation coordinator is gpt-6.1-sol (xhigh). It reads this master and task headers,
creates task branches/sibling worktrees, delegates to the named worker tier, reviews
results and runs Verification before explicit-path staging. Tasks sharing engine state
or format runtime code land sequentially. Use repository merge/squash rules, not empty
merge commits. Keep plans outstanding until deliverables land; then archive with owner
listening items clearly distinguished.

## Verification and acceptance

Canonical goldens must remain byte-identical with runtime bypass and with features off.
Run paired feature-on/original and feature-off output comparisons for all five formats.
Enabled behavior must be byte-identical at host block sizes 1, 3, 64, 128, 4096 and 8191
for identical absolute-frame input/control schedules. Include mid-quantum commands.

Cover snare→kick, C→G→C, cross-channel identity, mixed maps/loops, IT policies and runtime
commands, XM envelope sustain/release/carry, pending MOD swaps, offsets, delayed notes,
retrigger/portamento, resource exhaustion and stale generational handles. Test pause
mid-tick across tempo models, fractional clocks, repeated toggles, stop/seek/rebuild,
native versus custom cleanup and transport-aware playlist/media behavior. Check live
input using shared-memory ring and postMessage, focus/disconnect and wide modules.

Run targeted Rust/web tests, `cargo xtask goldens --check`, `cargo xtask ci`, WASM and
bare-metal feature matrices plus allocation hooks. Root CI does not build embedded:
inspect its feature graph and use its documented toolchain/check commands separately
when installed; report unavailable hardware/toolchains, never claim device validation.
No golden regeneration to conceal canonical differences. No cargo fmt.

Owner acceptance: percussion, piano, XM envelope and dense IT listening; demonstrate
instant bypass and audition while paused. Extra voices can increase loudness/muddiness;
shared master dynamics mean audition can affect the song mix. Retain existing gain and
limiter controls rather than silently normalising enhanced playback.

## Out of scope

Loop-tail synthesis/release overrides, saved presets, sample replacement catalogues,
ModDNA, an instrument library/extractor, native UI controls, new file formats, changing
historical sources, and broad M11 synthesis/MIDI-controller expansion. This delivers the
format articulation subset needed for faithful loaded-module audition, not all of M11.

## Task breakdown and landing order

All files below are pending external review and implementation. The coordinator is
`gpt-6.1-sol` (xhigh); worker floors and effort are explicit in each task header.

| Task | Deliverable | Worker | Prerequisites |
|---|---|---|---|
| [NO-01](NO-task-01-contracts-and-features.md) | Contracts, feature gates and ownership | gpt-6.1-sol (high) | Reviewed master; implementation explicitly requested |
| [NO-02](NO-task-02-format-articulation.md) | Format-owned per-voice articulation | gpt-6-astra (high) | NO-01 |
| [NO-03](NO-task-03-interactive-transport.md) | Sequencer-only pause and clear-and-hold Stop | gpt-6-astra (high) | NO-02 |
| [NO-04](NO-task-04-legacy-overlap.md) | Legacy-format overlap and bounded tails | gpt-6.1-sol (high) | NO-01 |
| [NO-05](NO-task-05-xm-it-overlap.md) | XM and IT overlap integration | gpt-6-astra (high) | NO-02 and NO-04 |
| [NO-06](NO-task-06-faithful-audition.md) | Faithful loaded-module audition | gpt-6-astra (high) | NO-03 and NO-05 |
| [NO-W1](../apps/NO-task-W1-web-controls.md) | Web transport, Instruments panel and command bridge | gpt-6.1-sol (high) | NO-06 (and all its dependencies) |
| [NO-07](NO-task-07-integration-and-acceptance.md) | Integration, determinism and feature exclusion | gpt-6.1-sol (high) | NO-W1 and NO-01–06 |

Recommended landing order: 01 → 02 → 03 → 04 → 05 → 06 → W1 → 07.
04 can be prepared independently after 01; 05 can be prepared after 02/04 while 03
finishes, but shared engine edits must be integrated by the coordinator sequentially.
Do not split multiple agents across the same checkout. No workers are started by this
planning handoff. External review procedure and pending evidence are in
[the review log](NO-review-log.md).
