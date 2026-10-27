# NO — Interactive note overlap and instrument audition

| Field | Value |
|---|---|
| Status | Implementation authorised 2026-10-27; NO-01–03 integrated; NO-04 in progress; reviewed instrument-only/onset-volume contract unchanged |
| Created | 2026-10-27 |
| Branch / worktree | `feature/note-overlap-audition` / `../starplayer-note-overlap-audition` |
| Goal | Optional instrument-only note overlap in MOD/S3M/MTM/XM/IT; faithful audition; sequencer-only Pause |
| Coordinator | `gpt-6.1-sol`, reasoning `xhigh` |
| Workers | `gpt-6.1-sol` for bounded tasks; `gpt-6-astra` for clock/articulation/NNA work; see task headers |
| External review | Claude CLI `claude-opus-5-5`, invoked with `--effort high`; instrument-only/onset-volume revision reviewed with corrections and final verdict: no further amendments required; see review log |
| Related | M4 live input, M5 XM, M6 IT, M10-K5 sample enhancement, M11 instrument library |

## Context for a fresh coordinator

Read `AGENTS.md`, `plans/README.md`, product architecture §§5 and 9, and the accuracy
policy. This is a reusable engine feature with a web UI, not a module-file editor.
Historical `STARPLAY/` and `STARPLAY-2.25s/` are read-only. Do not run cargo fmt.
The original checkout contains unrelated untracked files; never stage them.

The owner wants to hear truncated sample decays and optionally allow them to finish.
A snare should survive the next kick on its channel; piano C then G should overlap,
with another C at the same onset volume replacing earlier matching C voices on that
channel; quieter repeats may overlap as echoes. Defaults
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

### Instrument-only note overlap

Store one `let_notes_ring` boolean per instrument outside immutable Module data, plus
one global `note_overlap_enabled` boolean. Both default false. Settings are session-only,
reset on explicit new load, and retained across pause, seek and same-session renderer/
DSP/sample-enhancement rebuilds. Use separate session identity and engine-build generation
to reject stale commands, including explicit reloads of the same bytes.

There is no NoteOverlapMode enum, channel setting, inheritance, explicit Cut mode or
unrestricted Continue option. Unchecked instruments preserve native format behavior,
including authored IT NNA/DCT/DCA and runtime NNA changes; unchecked does not force Cut.
An instrument's checkbox applies everywhere it appears in the song. Channel identity
remains part of duplicate matching, not a configurable override.

When globally enabled, an eligible checked instrument lets outgoing notes finish, except
that a new musical note replaces **all** earlier matching foreground/background voices
with the same instrument, original trigger note, onset volume and originating channel.
Different notes or different onset volumes overlap. Notes on other channels remain
independent. The outgoing sounding instrument determines survival; the incoming identity
selects duplicates. Use existing bounded cut/release ramps for replacement.

Finalize onset identity on the actual start tick (including delayed starts), after all
of that channel’s processing for that tick, using its exact base, unmodulated note volume.
Include defaults, volume setters and any fine/fast slides executed on that onset tick.
MOD/MTM/S3M use `current_volume`, not `actual_volume`; IT uses `note_volume`; NO-05 must
name and document the corresponding XM base field before implementing its capture seam. Do not compare current audible loudness
or introduce rounding/tolerance buckets. Exclude tremolo/tremor, envelopes, later-tick slides, separate channel/global/master
gains, random gain modulation and DSP. Live identity uses the resolved base note gain
from event velocity and applicable note defaults **before CC7** or other separate gains,
using the same format mapping as playback. Preserve raw event velocity alongside the
current gain-scaled input where necessary; do not infer it by dividing the rendered gain.
Existing format volume resolution may map different velocities to the same base volume;
document/test these collisions, with no new identity-only quantization. NO-01 defines this identity field; format workers document and test the
capture point without changing canonical effect ordering. Later effects never rewrite
stored original note/onset volume. A volume change after a delayed note has actually
started does not retroactively change its identity.

Record identity passively for **every** actual new song/live note whenever note-overlap
support is compiled and the host has configured its runtime capability, including notes
started unchecked, globally bypassed or sample-ineligible and native IT backgrounds.
The global enable switch is not a recording gate. Defaults/bypass still produce canonical
PCM. Keep identity for the whole life of the generational voice — through key-off, fadeout,
NNA note-off/fade and live key-up release — until its slot is freed or reused, independent
of later toggles. Voices already retiring (bypass/Stop/eviction ramps) are not duplicate
candidates.

IT's native NNA/DCT seam precedes instrument defaults and onset volume effects. Keep
native candidate processing at that seam. For custom candidates, preserve/detach the
outgoing voice there without native duplicate action, then finalize incoming identity and
run the custom replacement scan after that channel's onset-tick effects, before mixing
any samples at that frame. Skip the new foreground by VoiceId/generation. Never match
against the previous note's volume, replay effects, or reorder native candidates' actions.
Resolve each onset synchronously at its event frame; delayed starts follow the same rule.
NO-05 must test this sequence under capacity pressure as well as ordinary playback; an
allocation fallback may not leave an unresolved incoming identity or run a duplicate scan
against a reused slot. Retain the bounded allocation/retirement rules already specified.

Examples: same-volume C→G→C replaces earlier C while G continues; repeated snare at the
same note/volume replaces the earlier hit; quieter repeats of the same note can ring as
echoes. This is a heuristic: equal-volume echoes are replaced, and differently pitched or
velocity-varied drum/piano hits overlap. No automatic echo or physical-instrument detection.

For IT, effective checked/eligible behavior substitutes this duplicate rule and continuing
NNA for the affected candidate's authored NNA/DCT/DCA; do not run both scans on that voice.
Unchecked, bypassed or ineligible candidates retain native matching/actions. Native
foreground processing still applies to outgoing native voices when incoming/outgoing
instruments differ. Keep native background-control commands. Track detachment provenance
separately from the current checkbox so bypass clears only custom-created tails.

For each IT candidate use the current global switch, its sounding instrument's current
checkbox and its actual sample's loop eligibility. Incoming sample eligibility does not
force native DCT on an otherwise eligible candidate. Ownership/provenance records are
orthogonal to this decision: existing native backgrounds keep their native budget and
survive bypass even if their instrument is now checked; custom-created tails keep their
custom provenance when unchecked and become subject to native DCT/DCA on subsequent
notes. Checkbox changes never retroactively migrate a voice between budgets. Song custom
tails use C; live tails use L even when custom-detached, with a separate custom-detachment
flag in the same authoritative ownership record. Bypass clears custom-detached voices
in both domains, never native-created backgrounds or live foregrounds merely because
an instrument is checked.

Only new musical note starts (including actual delayed starts) detach. Portamento,
effect retriggers, sample-only changes and queued swaps retain native semantics.
Detached voices keep their actual sample, pitch/volume/pan and format articulation;
subsequent channel effects target foreground only. Preserve originating channel routing,
mute and inserts. Explicit tracker cuts target foreground; native IT background-control
commands retain their authored semantics. A pending MOD swap must not make a custom
one-shot tail loop: detach the current non-looping region and discard its queued swap.

Let notes ring applies only to PCM with neither a normal loop nor a sustain loop.
Ineligible voices retain native behavior, including authored IT looping NNA. For mapped
XM/IT instruments check the actual sample; show a mixed-loop badge instead of disabling
all notes. Checkbox edits affect subsequent note events.

The global switch is labelled **Enable note overlap** and starts off. Off preserves all
instrument selections but restores native behavior for subsequent notes and immediately requests ramped clearing
of custom-created tails; authored IT backgrounds remain. On applies checked instruments
without recreating lost tails. This switch affects overlap in song playback and live audition/jam, but never disables
audition input/output, transport/Pause or other DSP settings. Bypass is a live comparison,
not reconstruction of the voices that an uninterrupted native render would have retained.

### Audition and UI

Instrument rows gain a Preview button, selection, a **Let notes ring** checkbox and loop status.
Use infinity with accessible text, and distinguish all-looping from mixed-map instruments.
Disable Preview for empty/unmapped/non-PCM slots. Do not implement S3M FM synthesis here.
Show why overlap is unavailable; disable the checkbox for entirely ineligible instruments,
not mixed maps. No channel overlap controls. Keep checkboxes editable while the global
switch is off so selections can be prepared. Checkbox help: “Let previous notes finish.
An identical note played at the same starting volume on this instrument and channel
replaces its earlier sound. Applies only to non-looping samples.” Explain that unchecked
means native behavior and may still overlap in IT.

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

Live note lifecycle is unchanged by the checkbox: key-up releases the corresponding
held instance (foreground or detached) through native release, or the simple-sample ramp;
sustain defers that release until pedal-up. Let notes ring prevents replacement by a new
note, not explicit note-off. One-shot button Preview still runs to completion as specified
above. Jam programs use their own instrument checkbox, not the selected-preview instrument.
All live voices, including custom-detached tails, use L, not C. Stop preview clears its
live audition scope; global bypass clears custom-detached live tails as well as song tails.
Test C and G held together, key-up of detached C, pedal release, equal/different velocities,
CC7 changes and bypass without shutting down audition or killing native live backgrounds.

Allocate interactive live lanes at 64–80, beyond the maximum tracker width, with an
explicit address mapping; never hardcode 48 as universally safe. Preserve hardware MIDI channel identity and tracker VoiceTag
limits. Preview voices do not consume tracker foreground reservations. Live note-offs
must locate held note instances even after NNA detachment; sustain, repeated keys and
program changes must not leave stuck voices.

### Capacity, real-time behavior and feature flags

Preallocate web budgets of 64 custom tails and 16 audition voices, in addition to native
format foreground/background requirements and bounded retirement headroom specified below.
These are caps in the global pool, not new mixers. Validate IT's per-slot state sizing when total capacity exceeds 256. Never leave
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

## Reviewed implementation decisions

These decisions resolve the initial external review and apply to every task.

### One articulation cadence, separate sequencing time

NO-03 owns one engine-level cadence authority for loaded-format articulation, consumed
by song and live runtimes. While Playing, tracker ticks drive it; while Paused/Stopped,
after natural sequence exhaustion, or without a tracker, it synthesises steps at the
retained/module-initial cadence with fractional remainder preserved. A host without a
loaded tracker uses an explicit configured format cadence (default 50 Hz), never the
MidiSource 1 ms housekeeping clock. The latter may still service simple instruments or
input, but must not advance loaded XM/IT articulation a second time.

At equal frames: apply due quantum-boundary commands, then sequenced events (preserving
native per-channel effect/articulation ordering), then stamped live events in stable
source order, then one live articulation step. A control-only boundary advances held
song articulation before live events and live articulation. Original tracker processing
continues to step its own voices once; the authority must not step them again. NO-02
adds the provided tracker control-step hook with real XM/IT implementations; NO-03 owns
its non-sequencing dispatch; NO-06 consumes the cadence for live format runtimes.

Pausing translates the tracker's pending absolute tick deadline and song origin by the
held audio-frame duration on resume, without resetting FrameClock's fractional remainder.
While held, song progress uses the frozen song-time cursor. Stopped time is held likewise.
Keep the separate live timestamp clock monotonic. Explicit seek is a different operation
and may reset tick phase. Tracker and SMF playback sources are sequenced sources and
freeze; keyboard/MIDI feeds are live sources and continue. SMF scheduling needs the same
held-time translation, but does not invent tracker tempo effects. Interactive SMF installation and
same-session replacement retain parsed file data off-thread and restore its consumed-event
index, song time, pending event duration and channel program/volume/pan/bend/sustain state,
with no replay of consumed events and no restored held notes. `InstrumentRack::for_sequenced_module`
uses song lanes 0–15/N in slot 0; the permanent live rack uses lanes 64–80/L in slot 1.
Song custom-detached tails belong to C; live custom tails remain L. Song/live clears reach
only the corresponding rack's holds/sustain. Generic MIDI racks retain their legacy or
configured live mapping unless explicitly constructed as sequenced. The existing SMF
length-return/caller-owned completion policy is unchanged.

The cadence is also an engine event boundary: NO-03 includes its next frame in event
splitting and adds a provided post-source articulation method to the existing EventSource
trait (default no-op, forwarded in SourceMux slot order). After due source dispatch at
the exact frame, call this method once when the cadence is due. NO-06 implements it for
MidiSource's format runtime. The MIDI 1 ms deadline must not defer this step. NO-03 tests
a scripted consumer before NO-06 lands the real one.

### Host transport, cleanup and telemetry

NO-03 covers `crates/starplayer-host/{src/player.rs,src/transport.rs,src/events.rs,
src/source.rs}`, not just engine commands. Add explicit interactive HostCommands.
Pause must never invoke the legacy ramp-to-zero Stop. Freeze song-fade elapsed time,
end-of-song arming and fade settlement during Pause. The existing fade gain is held
while paused (and can attenuate audition); resume continues the saved fade. Interactive
Stop cancels pending fades/rewinds and leaves the output path able to sound new live
notes at normal transport gain. Preserve the legacy host command path unchanged.

Interactive hosts install the sequenced source in mux slot 0 and a prepared live
MidiSource/rack supporting all 17 lanes in slot 1 at module installation, before any
Preview or destination selection. Jam/selected-input/Preview are bounded routing state
changes, never jam()/midi_only() source reconstruction or LoadModule. Feature-off or
non-opted-in hosts retain those existing APIs. NO-03 defines/enforces the Player source
invariant; NO-06 installs the rack/live slot and routing. Test first preview during pause
without changing song voices, effect state, row/tick or pending remainder.

Before first Play, interactive hosts start Stopped with a running audio/live path and
unity transport gain. Natural end/fade completion follows host playlist policy but clears
into interactive Stopped, restores unity gain after a bounded transition, and arms rewind
for next Play instead of immediately seeking or freezing live audio. User Stop cancels
pending rewind and holds position. Explicit Resume from Pause resumes the saved fade;
it must not take the legacy Play-during-fade restart branch. Preview is audible before
first Play and after natural end, with no false playlist event from user Pause/Stop.


Scoped clearing must reach foregrounds and all backgrounds; `ChannelTable::stop_all`
alone is insufficient and unramped `VoicePool::release_all` is not the audible solution.
Use bounded existing-duration release ramps, invalidate bindings/held input immediately,
and keep retiring slots owned until their ramps finish. They no longer count against
active admission, but use bounded physical retirement capacity as specified below. Stop clears DSP tails with a bounded click-safe transition, without a persistent
master mute or muting new preview input. Exact ramp mechanics belong to NO-03/06.
Bypass and Stop preview clear only their own scope. Global all-sound-off is a distinct
UI/host command; hardware MIDI CC120 stays lane-scoped.

Interactive seek clears song foregrounds, native/custom backgrounds and song articulation
before processor reset, retaining transport mode and audition voices. Invalid seeks do
not clear sound. Wire this at the seek dispatch that has engine context, and ensure seek
requests can dispatch while the sequencing gate is closed. Legacy seek behavior stays
unchanged in this feature; the pre-existing IT reset/voice ownership hazard is separate
follow-up work, not an excuse to change canonical seek silently. Jam/source replacement
and same-session rebuild must retain interactive mode and must not synthesize end events.
A valid seek while Paused/Stopped repositions without dispatching a sequence tick,
sets the frozen cursor to the target timeline mark, zeroes the held tick remainder, and
rebases the hold baseline to the seek frame S. Resume at R translates origin/deadline by
R−S, not by the time since the original pause. Test both held states and repeated seeks.
Legacy seek/restart with custom overlap enabled must retain ownership of sounding custom
tails until completion/bypass (or explicitly ramp-clear them); never reset bookkeeping
alone. Native legacy seek behavior remains outside this feature's cleanup change.

Rebuild may clear sounding voices (as today), but clears held input consistently and
restores logical cursor/mode/settings; sample-rate changes use time conversion, not an
assumption that old frame counts and fractional bits remain valid at a new rate.

Publish transport and live voice/meter telemetry on rate-limited control-only ticks,
and immediately on clears/state transitions at the next publication boundary. Keep the
64 tracker-lane telemetry layout; add a separate live summary. Existing `is_playing`
means sequencing advances (false for Paused/Stopped); a new explicit transport field
and natural-end indication govern host/UI behavior, never a falling boolean edge alone.

### Lane layout and capacity accounting

With interactive-audition, admit 64 tracker lanes plus **17 live lanes** (maximum 81).
The first 16 live lanes preserve MIDI channel identity; jam and selected-instrument
input are mutually exclusive destinations sharing these lanes. A seventeenth lane is
for the Preview button, with independent held-instance ownership. Its voices share the
live budget. Live lane base is fixed at 64 for every module width, so live lanes 64–80 lie outside
the maximum 64 tracker insert buses and mix through spill/master DSP. Decouple insert
bus_count from ChannelTable length. Preserve legacy MAX_CHANNELS=64, MIDI_CHANNEL_BASE=48
and MAX_VOICE_CAPACITY=272; add separate interactive limits and explicitly configured
constructors. Features make this capability available, but do not alter legacy host
settings when Cargo unifies features. Web opts in through runtime EngineSettings/host
configuration; other constructors keep legacy defaults. Keep mixer spill grouping and
tracker telemetry unchanged. Audit VoiceTag bounds,
insert packing and every hardcoded-48 consumer; expose layout-aware mappings to interactive
hosts instead of silently changing public legacy helpers. NO-01 implements the layout.

Active capacity is **N + C + L**: N is native format capacity (channel count for MOD/S3M/MTM/XM,
256 for IT), C is the configured custom-tail cap (web 64 when enabled, otherwise zero),
and L is a shared live budget (web 16). L replaces/includes the existing 16 jam reserve;
it is not 16 jam plus another 16 preview. Existing feature-off hosts with jam retain their
existing reserve; offline hosts without live input need none. A persistent host that
prepares before load uses maximum N=256, yielding 336 active web slots with both features
plus retirement headroom below.
NO-02 threads the prepared actual pool size through host/facade/source/format constructors;
all indexed articulation arrays cover it, and allocation errors occur before playback.

IT tracker foregrounds and authored backgrounds count toward N. Custom-provenance tails
count only toward C, are excluded from native quota counts and native victim selection,
and use oldest-custom eviction. All live voices, including native IT live NNA tails,
count only toward L and use oldest-live eviction at exhaustion; native live note actions
are preserved but resource-pressure stealing follows the live budget. None may steal
another domain's foreground. An overridden IT instrument can have fewer tails under the
64 custom cap than under native 256-voice behavior; this is an explicit bounded creative
mode, not a guarantee of increased polyphony. Each generation has one engine-owned
ownership/provenance/age record, used by format runtimes rather than duplicated tables.

Physical pool size is **N+C+L+R**, with R split into one native retirement slot when
interactive transport is configured, one custom retirement slot when custom tails are
configured, and one live retirement slot when interactive live input is configured.
The default interactive web pool is therefore 339 slots. These reserve slots are not
additional active voices. Retiring voices retain domain/age/generation ownership and
cannot receive note/effect updates, but no longer consume active admission quota. Physical
domain capacity is its active cap plus its retirement slot, even if other domains are
empty. Before a new admission at that physical limit, hard-release that domain's oldest
already-retiring voice (stable slot tie-break); never another domain's voice. Then move
the active victim into retiring state and admit the new note at its scheduled frame.
This bounded pressure fallback can shorten a release ramp; it is preferred to delaying
or refusing new input and must be tested/documented. Normal native IT allocation/stealing
is unchanged; native headroom serves interactive clearing, not increased native polyphony.
Stop can initially leave all former voices retiring in the domain's ordinary slots; the
extra slot permits immediate new input and the same rule bounds subsequent admissions.
NO-02 plumbs physical capacity; NO-03 supplies retirement/clear contracts; NO-04/05/06
apply them. Ownership metadata compiles with either feature, with custom-only policy
fields gated by note-overlap. Runtime defaults leave all optional budgets zero.

### Live runtime ownership and session/wire identity

One mutable live format runtime per rack owns the pool-indexed articulation and RNG for
all its XM or IT programs; never allocate one pool-sized runtime per program. Immutable
Instrument definitions retain mapping/sample data. NO-06 introduces a rack-level mutable
runtime interface only in the same commit as its two real XM and IT implementations,
backed by NO-02 concrete runtimes. No engine-to-format dependencies or RefCell-based RT
mutation. The facade constructs the format runtime and host uses it at each rack-builder
call site. Song and live random state remain separate.

A user-session ID changes for explicit new loads/reloads, even identical bytes. An engine
build generation changes for every replacement/rebuild, including sample enhancement.
Same-session rebuilds retain overrides/cursor/mode via Player shadow state, but reject
old-generation commands/replies. Renderer replacement still clears voices and reconstructs
format processor working state, as the existing host rebuild path does; it does not
transfer a suspended processor object or promise continuation of its effect-memory
internals. This boundary does not relax Pause/Stop in the same renderer: those preserve
the processor and effect memories exactly. NO-03's rebuild cursor retains RowClock tick,
repeat and delay budgets, pending jump/requested speed, row arrival, tempo, sounding
position, remaining deadline and fractional phase, song time and end flags. Restore runs
no musical callbacks. Loop-detector visited rows are reconstructed from the scanned
timeline through the saved song frame; its loop/budget/armed scalars are retained.
The coherent host snapshot also retains fade gain/elapsed and pending natural-end rewind.
Sample-rate changes convert durations and the whole-plus-fraction pending deadline; they
do not copy old-rate frame counts. The web labels rebuild vs explicit reload intentionally;
no byte-equality heuristic. Replay the full state before accepting input in the new build.

NO-W1 uses a versioned atomic wire record for interactive commands carrying the engine
build generation alongside opcode/argument/extra. Do not squeeze it into fields already
needed by the command, or send a non-atomic generation prefix. Negotiate ring version
before enabling interactive commands; old records retain existing opcode values and
legacy behavior, while missing capability disables interactive UI. postMessage uses the
same logical envelope/validation. Obtain loop/maps metadata from the metadata WASM
instance (`apps/starplayer-web/src/lib.rs`), and correlate it with session/build identity;
worklet capability/ack state confirms what is actually installed.

### XM background update ordering

Preserve foreground effects → articulation → flush ordering per channel. Custom XM
backgrounds step after that originating channel's effects/foreground processing, before
the next channel; they see that channel's current module global-volume value. A detached
voice already stepped at that boundary must not step twice. Control-only steps use
stable channel/slot order and the frozen global-volume state. NO-05 tests same-tick Gxx
and detachment to make the ordering executable rather than inferred.

## Delivery and review workflow

No implementation starts until explicitly requested. Planning commits are not evidence
of external approval. First commit this master, index and planned foundation amendments;
then commit executable task specs and the review handoff separately.

External review of the prior multi-mode design is complete: an initial source-backed
master/task pass, a source-backed
follow-up, and a final focused consistency pass using the corrected documents. All used
Claude CLI `claude-opus-5-5` with `--effort high`; no fallback model or implementation.
The final pass's remaining wording condition has been applied, and findings/dispositions
are preserved in the review log. Future material contract changes need focused review;
the subsequent instrument-only/onset-volume revision has now completed its own source-backed
review and focused follow-ups, ending with “No further amendments required.” Corrections
and dispositions are in the review log. Planning approval does not authorize implementation.
The owner authorised implementation, delegation, testing and feature/task-branch commits on 2026-10-27, after planning commit `f844ba7`. No merge into main or deployment is authorised.
Never include session URLs or attribution in commits/PRs.

Implementation coordinator is gpt-6.1-sol (xhigh). It reads this master and task headers,
creates task branches/sibling worktrees, delegates to the named worker tier, reviews
results and runs Verification before explicit-path staging. Tasks sharing engine state
or format runtime code land sequentially. Use repository merge/squash rules, not empty
merge commits. Keep plans outstanding until deliverables land; then archive with owner
listening items clearly distinguished.

## Verification and acceptance

Canonical goldens must remain byte-identical with runtime bypass and with features off.
For all five formats compare canonical/feature-off PCM against three independently reset
feature-on configurations with passive identity recording active: globally enabled with
all instruments unchecked; globally bypassed with all checked; globally enabled with only
wholly ineligible/looping instruments checked (including authored IT looping NNA).
Do not confuse these from-start equivalence tests with mid-song bypass, which clears
custom tails and cannot recreate an uninterrupted canonical voice history.
Enabled behavior must be byte-identical at host block sizes 1, 3, 64, 128, 4096 and 8191
for identical absolute-frame input/control schedules. Include mid-quantum stamped live
events; host commands retain their existing quantum-boundary application contract.

Cover snare→kick, same-volume C→G→C and snare repeats, different-volume echoes, onset
volume commands/delays, later slides/envelopes/gain changes, cross-channel identity,
mixed maps/loops, checked/unchecked IT candidates and runtime
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

The instrument-only/onset-volume revision and all task amendments have completed external
review. Implementation is authorised by the 2026-10-27 owner handoff. The coordinator is
`gpt-6.1-sol` (xhigh); worker floors and effort are explicit in each task header.

| Task | Deliverable | Worker | Prerequisites |
|---|---|---|---|
| [NO-01](NO-task-01-contracts-and-features.md) | Contracts, feature gates and ownership | gpt-6.1-sol (high) | Reviewed master; implementation explicitly requested |
| [NO-02](NO-task-02-format-articulation.md) | Format-owned per-voice articulation | gpt-6-astra (medium) | NO-01 |
| [NO-03](NO-task-03-interactive-transport.md) | Sequencer-only pause and clear-and-hold Stop | gpt-6-astra (medium) | NO-02 |
| [NO-04](NO-task-04-legacy-overlap.md) | Legacy-format overlap and bounded tails | gpt-6.1-sol (high) | NO-01 |
| [NO-05](NO-task-05-xm-it-overlap.md) | XM and IT overlap integration | gpt-6-astra (medium) | NO-02, NO-03 and NO-04 |
| [NO-06](NO-task-06-faithful-audition.md) | Faithful loaded-module audition | gpt-6-astra (medium) | NO-03 and NO-05 |
| [NO-W1](../apps/NO-task-W1-web-controls.md) | Web transport, Instruments panel and command bridge | gpt-6.1-sol (high) | NO-06 (and all its dependencies) |
| [NO-07](NO-task-07-integration-and-acceptance.md) | Integration, determinism and feature exclusion | gpt-6.1-sol (high) | NO-W1 and NO-01–06 |

Recommended landing order: 01 → 02 → 03 → 04 → 05 → 06 → W1 → 07.
04 can be prepared independently after 01, with NO-03 cleanup integrated before landing;
05 begins after 02/03/04. Shared engine edits land sequentially.
Do not split multiple agents across the same checkout. No workers are started by this
planning handoff. External review findings, corrections and final disposition are in
[the review log](NO-review-log.md).

## Implementation progress

- NO-01 integrated as a reviewed single-change squash (task commit `5d191ac`).
  Independent feature matrices, no_std, WASM, firmware-common host tests and all eight
  goldens passed; coordinator final engine/facade regression passed. Exact evidence and
  dependent capture/behavior obligations are in NO-01. Plans stay here pending main landing.
- NO-02 starts after NO-01; worker Astra medium per explicit owner assignment. Only one
  task agent and one heavy Cargo workload run at a time (owner OOM constraint).

- NO-02 integrated as a reviewed single-change squash (task commit `bbf57f4`): concrete
  XM/IT runtimes, prepared capacity plumbing and generation-safe control operations.
  Canonical conformance baseline and all eight goldens unchanged; prepared PCM agrees
  at all six required sizes. Allocation hooks, optional combinations, no_std and Clippy
  passed; coordinator affected-package regression passed. See NO-02 for exact evidence.
- NO-03 starts after NO-02, using Astra medium as the sole active task worker.

- NO-03 integrated by reviewed single-change squash (task commit `adaa84e`): opt-in
  sequencer-only Pause, clear-and-hold Stop, exact cadence/source ordering, scoped
  retirement, DSP transition, held seeks and tracker/SMF rebuild cursors. Root final
  affected-package regression passed, including 13 host and 8 engine transport tests.
  Canonical conformance/goldens unchanged; allocation, no_std/WASM, unification and
  default embedded checks passed. See NO-03 for exact commands and reconstruction limits.
- NO-04 starts after NO-03, using Sol high as the sole active task worker.
