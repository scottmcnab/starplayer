//! [`Instrument`] — the musical layer's half of the engine (architecture §5.3), the
//! [`InstrumentRack`] that binds sixteen MIDI channels to a module's instruments, and the
//! [`EventFeed`]/[`MidiSource`] pair that carries `Event`s to it.
//!
//! # Two peers, one voice pool
//!
//! Architecture §2.4 draws the engine as two peers feeding one voice pool: a tracker
//! pattern writes voice parameters directly, and a **musical** source — an SMF, a live
//! MIDI keyboard, a computer keyboard — sends `NoteOn`/`NoteOff`/controller events to an
//! [`Instrument`], which turns them into voices. The tracker half has five format
//! implementations; this module is the musical half.
//!
//! ```text
//! PatternSequencer ──Trigger + direct VoiceParams writes──┐
//! MidiSource<Feed> ──Event──> InstrumentRack ──Instrument─┼──> VoicePool ──> master bus
//! ```
//!
//! # Why the trait is committed here
//!
//! Design goal 8: no trait until its second real implementation exists. [`Instrument`] has
//! two here — [`SampleInstrument`] for MOD, S3M and MTM, and [`MappedInstrument`] for XM
//! and IT — and neither is a tracker processor. The five `TrackerProcessor`s deliberately
//! do **not** implement it: they own their formats' per-voice articulation and write voice
//! parameters through `TickContext`, which is what the M4-lite decision made them.
//!
//! # What these instruments do not do
//!
//! No volume, panning or pitch envelopes, no fadeout, no New Note Actions, no
//! auto-vibrato and no resonant filter — a MIDI-driven note starts, sounds and stops.
//! Giving a MIDI-driven XM or IT instrument its format's own articulation is **M11**'s
//! deliverable (`plans/engine/M11-master-plan.md`), and architecture open question Q4 —
//! whether the trait survives contact with a non-sample instrument — stays open for M10.
//!
//! # The two shapes E5 and E6 build on
//!
//! [`EventFeed`] is what a feed implements for [`MidiSource<Feed>`]: E5's `SmfSequencer`
//! is a cursor over a sorted event list, and [`ExternalEventQueue`] is the SPSC ring live
//! MIDI and the computer keyboard push into.
//!
//! ```
//! use starplayer_core::{Frame, TimedEvent};
//! use starplayer_engine::instrument::EventFeed;
//!
//! /// A fixed list of events at known frames — the shape E5's `SmfSequencer` has.
//! struct ScriptedFeed { events: Vec<TimedEvent>, next: usize }
//!
//! impl EventFeed for ScriptedFeed {
//!     fn next_frame(&self) -> Option<Frame> { self.events.get(self.next).map(|event| event.frame) }
//!     fn pop_due(&mut self, frame: Frame) -> Option<TimedEvent> {
//!         let event = *self.events.get(self.next).filter(|event| event.frame <= frame)?;
//!         self.next += 1;
//!         Some(event)
//!     }
//! }
//! ```
//!
//! The host's half is [`ExternalEventProducer`], which E6's `Player::send_event` will
//! call from whatever thread its MIDI input runs on:
//!
//! ```
//! use starplayer_core::{Event, Frame, Note, TimedEvent, U0F16};
//! use starplayer_engine::instrument::{external_event_channel, midi_channel};
//!
//! let (mut producer, queue) = external_event_channel(256);
//! // The host stamps an absolute output frame, far enough ahead that the event is still
//! // in the future when the audio thread reaches it — two render quanta by default.
//! let due = Frame(48_000 + 256);
//! let note_on = Event::NoteOn { note: Note::MIDDLE_C, velocity: U0F16::MAX };
//! producer.send(TimedEvent::on_channel(due, midi_channel(0), note_on)).expect("room in the ring");
//! # let _ = queue;
//! ```
//!
//! An event whose frame has already passed is **not** dropped: it dispatches at the
//! current frame and raises [`EngineWarnings::late_events`](crate::EngineWarnings).

use alloc::boxed::Box;
use alloc::vec::Vec;

use starplayer_core::fixed::bipolar_from_ratio;
use starplayer_core::tables::{LINEAR_UNITS_PER_SEMITONE, scale_frequency};
use starplayer_core::{
    ChannelId, Event, Frame, I1F15, InstrumentId, Note, SampleId, Step, Target, TimedEvent, U0F16, VoiceId, VoiceParam,
    VoiceParams,
};
use starplayer_mixer::{LoopSpan, SampleRegion, VoiceTag};
use starplayer_model::{InstrumentDef, LoopMode, Module, ModuleFormat, SampleIndex};
use starplayer_rt::{Arc, Consumer, Producer};

use crate::control::{ControlClock, DEFAULT_CONTROL_INTERVAL_MICROS};
use crate::source::{EngineContext, EventSource};

// ── the conventions ─────────────────────────────────────────────────────────────────

/// The engine channel MIDI channel 0 occupies (master-plan decision 3).
///
/// MIDI channel `n` is `ChannelId(48 + n)`, so a module of up to 48 channels and sixteen
/// MIDI channels coexist in the 64-lane [`ChannelTable`](crate::ChannelTable) and the
/// telemetry snapshot shows both. An IT module wider than 48 channels shares its top lanes
/// with MIDI: documented, not prevented, because forbidding it would cost every narrower
/// module a second table.
pub const MIDI_CHANNEL_BASE: u16 = 48;

/// MIDI channels in one rack.
pub const MIDI_CHANNEL_COUNT: usize = 16;

/// The note that plays a sample at its reference rate (master-plan decision 4).
///
/// MIDI 60 — middle C — is Scream Tracker's C-4 and Impulse Tracker's C-5. XM's
/// `relative_note`/`finetune` and IT's note maps apply on top of it.
pub const REFERENCE_NOTE: u8 = 60;

/// Notes one MIDI channel can hold down at once (research point 4).
///
/// Sixteen is past what ten fingers and a sustain pedal produce, and a fixed array is what
/// keeps the rack allocation-free. A seventeenth note **steals the oldest** entry: the
/// stolen note is forgotten rather than stopped, because a channel sounds only its newest
/// note, so nothing audible changes — the stolen note's own `NoteOff` simply finds nothing
/// to release.
pub const MAX_HELD_NOTES: usize = 16;

/// Cents a full-scale [`Event::PitchBend`] moves the note (master-plan decision 4): ±2
/// semitones, the General MIDI default bend range.
pub const DEFAULT_BEND_RANGE_CENTS: i16 = 200;

/// Control ticks one [`MidiSource::dispatch`] will take before it resynchronises its clock
/// to the current frame. A source installed at a frame far past its clock's next tick would
/// otherwise have to catch up one interval at a time, inside the audio callback.
const MAX_CONTROL_TICKS_PER_DISPATCH: u32 = 8;

/// The engine channel MIDI channel `channel` plays on. Channels past
/// [`MIDI_CHANNEL_COUNT`] wrap, so a decoder cannot address a tracker lane by accident.
pub const fn midi_channel(channel: u8) -> ChannelId {
    ChannelId(MIDI_CHANNEL_BASE + (channel as u16 % MIDI_CHANNEL_COUNT as u16))
}

/// The MIDI channel an engine channel is, or `None` for a tracker lane.
pub const fn midi_channel_index(channel: ChannelId) -> Option<usize> {
    let index = channel.0 as usize;
    let base = MIDI_CHANNEL_BASE as usize;
    if index < base || index >= base + MIDI_CHANNEL_COUNT { None } else { Some(index - base) }
}

// ── the trait ───────────────────────────────────────────────────────────────────────

/// What a [`NoteOn`](Event::NoteOn) asks an [`Instrument`] for.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteParams {
    /// The pitch. [`Note::cents`] is honoured, so a microtonal source needs nothing extra.
    pub note: Note,
    /// How hard it was struck. Linear: velocity scales the voice volume directly.
    pub velocity: U0F16,
    /// Pan for this note, overriding the instrument's and the sample's.
    pub pan_override: Option<I1F15>,
}

impl NoteParams {
    /// A note at full velocity with no pan override.
    pub const fn new(note: Note, velocity: U0F16) -> NoteParams {
        NoteParams { note, velocity, pan_override: None }
    }
}

/// Turns channel events into voice behaviour (architecture §5.3).
///
/// `&self`, deliberately: an instrument is **shared, immutable knowledge** about a
/// module's samples, and everything that varies per channel — the held notes, the
/// controller state, the bend — belongs to the [`InstrumentRack`] that owns it. That is
/// what lets one `Box<dyn Instrument>` serve all sixteen channels at once, and it is why
/// the channel is a parameter rather than a field.
///
/// Trait objects at event rate are fine (architecture §8: "trait objects at block level
/// are fine"); nothing here is called per sample.
pub trait Instrument: Send {
    /// Start (or restart) `channel`'s voice for `params`. Returns the voice, or `None` if
    /// the instrument sounds nothing for that note or the pool is full.
    fn note_on(&self, channel: ChannelId, params: NoteParams, context: &mut EngineContext<'_>) -> Option<VoiceId>;

    /// Release `channel`'s voice: for a sample with no release stage, stop it.
    ///
    /// The mixer's 64-frame gain ramp is what makes that click-free, so "stop" here is a
    /// [`DirtyBits::STOP`](starplayer_core::DirtyBits) flag rather than a silenced voice.
    fn note_off(&self, channel: ChannelId, context: &mut EngineContext<'_>);

    /// Once per control tick: advance whatever articulation the instrument has.
    ///
    /// A no-op for both of this milestone's instruments — neither runs an envelope — but
    /// it is called, at the rate architecture §5.4 defines, so that M11's enveloped
    /// instruments need no new plumbing.
    fn control_tick(&self, channel: ChannelId, context: &mut EngineContext<'_>);

    /// Apply a pitch bend, in cents, to `channel`'s voice. The rack has already scaled the
    /// bend by its range, so this is an absolute offset from the sounding note.
    fn set_bend(&self, channel: ChannelId, cents: i16, context: &mut EngineContext<'_>);
}

// ── shared arithmetic ───────────────────────────────────────────────────────────────

/// The mixer region for one sample: its loop, in the mixer's own vocabulary.
///
/// The engine's copy of the projection every format crate makes from a
/// [`SampleIndex`] — `starplayer-model` cannot make it itself, because the model does not
/// depend on the mixer (architecture §6). IT's sustain-loop variant stays in
/// `starplayer-it`: choosing between the sustain loop and the normal one is key-off
/// semantics, which is format behaviour rather than a projection.
pub fn sample_region(sample: &SampleIndex) -> SampleRegion {
    let one_shot = || SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames());
    match sample.loop_mode() {
        LoopMode::Forward => LoopSpan::new(sample.loop_start(), sample.loop_end())
            .map(|span| SampleRegion::looping(sample.pcm_offset(), span))
            .unwrap_or_else(one_shot),
        LoopMode::PingPong => LoopSpan::ping_pong(sample.loop_start(), sample.loop_end())
            .map(|span| SampleRegion::looping(sample.pcm_offset(), span))
            .unwrap_or_else(one_shot),
        LoopMode::None => one_shot(),
    }
}

/// `left × right` for two unit scalars, rounded to nearest.
///
/// `U0F16::MAX` is unity, so the divisor is 65535 rather than 65536 and unity times unity
/// is exactly unity — which matters because velocity, instrument volume, instrument global
/// volume and sample volume are multiplied together and a `>> 16` would lose a bit at each
/// step.
fn scale_unit(left: U0F16, right: U0F16) -> U0F16 {
    let product = left.to_bits() as u32 * right.to_bits() as u32;
    U0F16::from_bits(((product + u16::MAX as u32 / 2) / u16::MAX as u32) as u16)
}

/// Cents to 1/64ths of a semitone, rounded to nearest — the resolution of
/// [`scale_frequency`] (research point 2).
///
/// One unit is 1.5625 cents, so a bend is quantised to 1/64 of a semitone: a ±2-semitone
/// range resolves to 256 steps rather than MIDI's 16,384, and a bend wheel moved by one
/// fourteenth-bit LSB usually changes nothing. That is FastTracker 2's and Impulse
/// Tracker's own pitch resolution, and using it here is what keeps the MIDI path on the
/// same table — and therefore bit-identical across x86, ARM and WASM — as the tracker path.
fn cents_to_units(cents: i32) -> i32 {
    let scaled = cents * LINEAR_UNITS_PER_SEMITONE;
    if scaled >= 0 { (scaled + 50) / 100 } else { (scaled - 50) / 100 }
}

/// Where `note` sits relative to [`REFERENCE_NOTE`], in 1/64ths of a semitone, with
/// `extra_cents` of pitch bend folded in.
fn units_for_note(note: Note, extra_cents: i16) -> i32 {
    let semitones = note.semitone as i32 - REFERENCE_NOTE as i32;
    semitones * LINEAR_UNITS_PER_SEMITONE + cents_to_units(note.cents as i32 + extra_cents as i32)
}

/// The resample step for a sample of `reference_rate_hz` played `units` from its reference
/// note at `sample_rate_hz` (research point 1).
///
/// **One integer pitch path**, the linear-frequency table, for every instrument here —
/// including MOD, S3M and MTM, which their own processors play through the ST3 period
/// table instead. The period path truncates twice (note → period → frequency), so
/// `8363 Hz` at MIDI 60 comes back as 8362 or 8364 depending on the sample; the linear
/// path returns the reference rate **exactly** at zero units, which is what makes
/// decision 4's convention — "MIDI note 60 plays the sample at its reference rate" — and
/// "a bend of zero cents changes nothing" true rather than nearly true. It is also the
/// table pitch bend has to use anyway, so the alternative would have been two tables
/// disagreeing by a few cents at the ends of the keyboard.
///
/// This is not an accuracy deviation: no tracker pattern reaches this function. The format
/// processors keep their own period arithmetic, which is what the goldens and the
/// conformance corpus pin.
fn step_for(reference_rate_hz: u32, units: i32, sample_rate_hz: u32) -> Step {
    let frequency_hz = scale_frequency(reference_rate_hz, units);
    Step::from_ratio(frequency_hz as u64, sample_rate_hz.max(1) as u64)
}

/// Everything one note needs from a module, resolved once.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct VoicePlan {
    region: SampleRegion,
    params: VoiceParams,
    tag: VoiceTag,
}

/// Everything an [`Instrument`] has resolved about one note before the sample is read.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct VoiceRecipe {
    sample_id: SampleId,
    instrument: InstrumentId,
    /// The note actually played — after an XM/IT transpose map, if there is one.
    note: Note,
    /// Velocity, already scaled by the channel's CC7 volume.
    velocity: U0F16,
    /// The instrument's own volume, and its global volume where the format has one.
    volume_scale: U0F16,
    pan: I1F15,
    sample_rate_hz: u32,
}

/// XM's per-sample tuning, in 1/64ths of a semitone: a relative note and a 1/128-semitone
/// finetune. Zero for every other format, which folds its tuning into the reference rate.
fn sample_tuning_units(sample: &SampleIndex) -> i32 {
    sample.relative_note() as i32 * LINEAR_UNITS_PER_SEMITONE + (sample.finetune() as i32) / 2
}

/// The voice a sample plays for one note, or `None` when the sample is unusable.
fn plan_voice(sample: &SampleIndex, recipe: VoiceRecipe) -> Option<VoicePlan> {
    if sample.length_frames() == 0 {
        return None;
    }
    // XM tunes its samples by a relative note and a 1/128-semitone finetune; IT tunes them
    // by C5Speed alone, which is already `reference_rate_hz`. Reading both from the one
    // `SampleIndex` is what lets a single formula serve both formats.
    let units = units_for_note(recipe.note, 0) + sample_tuning_units(sample);
    let params = VoiceParams {
        step: step_for(sample.reference_rate_hz(), units, recipe.sample_rate_hz),
        volume: scale_unit(scale_unit(recipe.velocity, recipe.volume_scale), sample.default_volume()),
        pan: recipe.pan,
        ..VoiceParams::SILENT
    };
    let tag = VoiceTag {
        // `ChannelTable::trigger` overwrites this with the real channel.
        channel: 0,
        // One-based, as every format processor tags its voices, so the telemetry and the
        // trace read a MIDI voice exactly as they read a tracker one.
        instrument: recipe.instrument.0.saturating_add(1).min(u8::MAX as u16) as u8,
        sample: recipe.sample_id.0.saturating_add(1),
        note: recipe.note.to_midi(),
    };
    Some(VoicePlan { region: sample_region(sample), params, tag })
}

/// Rewrite the sounding voice's step for a new bend, re-deriving the note from the voice's
/// own tag.
///
/// The tag carries a **MIDI note number**, so the cents of a microtonal `NoteOn` are not
/// preserved across a bend — the bend snaps such a note to its nearest semitone plus the
/// bend. Re-deriving beats scaling the current step, which would accumulate rounding over
/// a wheel sweep, and the alternative — a per-channel copy of the sounding `Note` — would
/// be state the *rack* owns being read by an instrument that must stay `&self`.
fn rebend(
    module: &Module,
    channel: ChannelId,
    cents: i16,
    tuning: bool,
    sample_rate_hz: u32,
    context: &mut EngineContext<'_>,
) {
    let Some(voice) = context.channels.foreground(channel) else { return };
    let Some(state) = context.voices.get(voice) else { return };
    let (sample_number, note) = (state.tag.sample, state.tag.note);
    if sample_number == 0 {
        return;
    }
    let Some(sample) = module.sample(SampleId(sample_number - 1)) else { return };
    let tuning_units = if tuning { sample_tuning_units(sample) } else { 0 };
    let units = units_for_note(Note::new(note), cents) + tuning_units;
    let step = step_for(sample.reference_rate_hz(), units, sample_rate_hz);
    context.write_voice_param(voice, VoiceParam::Step(step));
}

// ── the two implementations ─────────────────────────────────────────────────────────

/// One instrument, one sample: MOD, S3M and MTM (`InstrumentDef::sample`).
///
/// MIDI note 60 plays the sample at [`SampleIndex::reference_rate_hz`]; every other note
/// is that rate scaled through the linear-frequency table (see [`step_for`]). Volume is
/// velocity × the instrument's default volume × the sample's own; pan is the note's
/// override, else the sample's default, else centre.
pub struct SampleInstrument {
    module: Arc<Module>,
    instrument: InstrumentId,
    sample_rate_hz: u32,
}

impl SampleInstrument {
    /// Bind instrument `index` of `module`, playing at `sample_rate_hz`.
    pub fn new(module: &Arc<Module>, index: InstrumentId, sample_rate_hz: u32) -> SampleInstrument {
        SampleInstrument { module: Arc::clone(module), instrument: index, sample_rate_hz }
    }

    fn definition(&self) -> Option<&InstrumentDef> { self.module.instrument(self.instrument) }

    fn plan(&self, params: NoteParams) -> Option<VoicePlan> {
        let definition = self.definition()?;
        let sample_id = definition.sample?;
        let sample = self.module.sample(sample_id)?;
        let recipe = VoiceRecipe {
            sample_id,
            instrument: self.instrument,
            note: params.note,
            velocity: params.velocity,
            volume_scale: definition.default_volume,
            pan: params.pan_override.or(sample.default_pan()).unwrap_or(I1F15::ZERO),
            sample_rate_hz: self.sample_rate_hz,
        };
        plan_voice(sample, recipe)
    }
}

impl Instrument for SampleInstrument {
    fn note_on(&self, channel: ChannelId, params: NoteParams, context: &mut EngineContext<'_>) -> Option<VoiceId> {
        let plan = self.plan(params)?;
        context.channels.trigger(channel, context.voices, plan.tag, plan.region, plan.params, 0)
    }

    fn note_off(&self, channel: ChannelId, context: &mut EngineContext<'_>) {
        context.channels.stop(channel, context.voices);
    }

    fn control_tick(&self, _channel: ChannelId, _context: &mut EngineContext<'_>) {}

    fn set_bend(&self, channel: ChannelId, cents: i16, context: &mut EngineContext<'_>) {
        // MOD, S3M and MTM samples carry no relative note or finetune of their own — the
        // loaders fold both into `reference_rate_hz` — so there is no tuning term here.
        rebend(&self.module, channel, cents, false, self.sample_rate_hz, context);
    }
}

/// An instrument that selects its sample per note: XM and IT.
///
/// [`InstrumentDef::note_transpose_map`] chooses the note actually played (IT's note
/// transposition; the identity map everywhere else), then
/// [`InstrumentDef::note_sample_map`] chooses the sample. XM's per-sample
/// `relative_note`/`finetune` and IT's `C5Speed` — both of them fields of the sample —
/// apply on top, as does [`InstrumentDef::global_volume`].
///
/// An instrument whose map is empty falls back to [`InstrumentDef::sample`], so an XM
/// instrument with a single sample and no map still sounds.
pub struct MappedInstrument {
    module: Arc<Module>,
    instrument: InstrumentId,
    sample_rate_hz: u32,
}

impl MappedInstrument {
    /// Bind instrument `index` of `module`, playing at `sample_rate_hz`.
    pub fn new(module: &Arc<Module>, index: InstrumentId, sample_rate_hz: u32) -> MappedInstrument {
        MappedInstrument { module: Arc::clone(module), instrument: index, sample_rate_hz }
    }

    fn definition(&self) -> Option<&InstrumentDef> { self.module.instrument(self.instrument) }

    /// The played note and the sample it selects, or `None` when the map is silent there.
    fn select(&self, definition: &InstrumentDef, note: Note) -> Option<(Note, SampleId)> {
        let index = note.semitone as usize;
        let transposed = match definition.note_transpose_map.get(index) {
            Some(&played) => Note { semitone: played, cents: note.cents },
            None => note,
        };
        let mapped = definition
            .note_sample_map
            .get(transposed.semitone as usize)
            .copied()
            .filter(|number| *number > 0)
            .map(|number| SampleId(number - 1))
            .or(definition.sample)?;
        Some((transposed, mapped))
    }

    fn plan(&self, params: NoteParams) -> Option<VoicePlan> {
        let definition = self.definition()?;
        let (note, sample_id) = self.select(definition, params.note)?;
        let sample = self.module.sample(sample_id)?;
        // IT's instrument pan wins over the sample's when the file enables it; a note's
        // own override wins over both.
        let recipe = VoiceRecipe {
            sample_id,
            instrument: self.instrument,
            note,
            velocity: params.velocity,
            volume_scale: scale_unit(definition.default_volume, definition.global_volume),
            pan: params.pan_override.or(definition.default_pan).or(sample.default_pan()).unwrap_or(I1F15::ZERO),
            sample_rate_hz: self.sample_rate_hz,
        };
        plan_voice(sample, recipe)
    }
}

impl Instrument for MappedInstrument {
    fn note_on(&self, channel: ChannelId, params: NoteParams, context: &mut EngineContext<'_>) -> Option<VoiceId> {
        let plan = self.plan(params)?;
        context.channels.trigger(channel, context.voices, plan.tag, plan.region, plan.params, 0)
    }

    fn note_off(&self, channel: ChannelId, context: &mut EngineContext<'_>) {
        // No envelope release and no fadeout: M11 gives a MIDI-driven XM or IT instrument
        // its format's own release. Until then a note-off stops the voice, and the mixer's
        // ramp keeps it click-free.
        context.channels.stop(channel, context.voices);
    }

    fn control_tick(&self, _channel: ChannelId, _context: &mut EngineContext<'_>) {}

    fn set_bend(&self, channel: ChannelId, cents: i16, context: &mut EngineContext<'_>) {
        rebend(&self.module, channel, cents, true, self.sample_rate_hz, context);
    }
}

/// The instrument a module's format calls for, boxed.
///
/// The one place the format decides which implementation plays: MOD, S3M and MTM get
/// [`SampleInstrument`], XM and IT get [`MappedInstrument`]. Off the audio thread — it
/// allocates.
pub fn instrument_for(module: &Arc<Module>, index: InstrumentId, sample_rate_hz: u32) -> Box<dyn Instrument> {
    match module.header().format {
        ModuleFormat::Mod | ModuleFormat::S3m | ModuleFormat::Mtm => {
            Box::new(SampleInstrument::new(module, index, sample_rate_hz))
        }
        ModuleFormat::Xm | ModuleFormat::It => Box::new(MappedInstrument::new(module, index, sample_rate_hz)),
    }
}

// ── the rack ────────────────────────────────────────────────────────────────────────

/// What the rack did with one event.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum EventOutcome {
    /// It changed something.
    #[default]
    Applied,
    /// It was addressed to the rack and understood, but there was nothing to do — a
    /// note-off for a note that is not held, a program change to the program already
    /// bound, a controller the rack does not map.
    Ignored,
    /// It is not the rack's vocabulary: [`Event::Trigger`] and [`Event::Param`] are the
    /// tracker's, and [`Target::Voice`] addresses a voice rather than a channel.
    Unsupported,
}

/// The MIDI controller numbers the rack acts on.
///
/// [`Event::Controller`] carries a `u16` number rather than MIDI's seven bits
/// (architecture §2.3), so these are the *MIDI* numbers a codec will produce; a
/// high-resolution or non-MIDI controller simply has a number of its own.
pub mod controller {
    /// Channel volume (CC7). Scales the next note struck on the channel.
    pub const VOLUME: u16 = 7;
    /// Pan position (CC10). Overrides the instrument's and the sample's pan.
    pub const PAN: u16 = 10;
    /// Sustain pedal (CC64). At or above half scale it holds note-offs back.
    pub const SUSTAIN: u16 = 64;
    /// All sound off (CC120). Stops every voice on the channel immediately.
    pub const ALL_SOUND_OFF: u16 = 120;
    /// All notes off (CC123). Releases every held note on the channel.
    pub const ALL_NOTES_OFF: u16 = 123;
}

/// The notes one channel is holding down, newest last.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct HeldNotes {
    notes: [u8; MAX_HELD_NOTES],
    /// Whether each held note has had its note-off held back by the sustain pedal.
    releasing: [bool; MAX_HELD_NOTES],
    len: usize,
}

impl Default for HeldNotes {
    fn default() -> HeldNotes { HeldNotes { notes: [0; MAX_HELD_NOTES], releasing: [false; MAX_HELD_NOTES], len: 0 } }
}

impl HeldNotes {
    fn as_slice(&self) -> &[u8] { self.notes.get(..self.len).unwrap_or(&[]) }

    fn position(&self, note: u8) -> Option<usize> { self.as_slice().iter().position(|held| *held == note) }

    /// Add `note`, stealing the oldest entry when the array is full (research point 4).
    fn push(&mut self, note: u8) {
        if let Some(index) = self.position(note) {
            if let Some(flag) = self.releasing.get_mut(index) {
                *flag = false;
            }
            return;
        }
        if self.len >= MAX_HELD_NOTES {
            self.remove_at(0);
        }
        if let (Some(slot), Some(flag)) = (self.notes.get_mut(self.len), self.releasing.get_mut(self.len)) {
            *slot = note;
            *flag = false;
            self.len = self.len.saturating_add(1);
        }
    }

    fn remove_at(&mut self, index: usize) {
        if index >= self.len {
            return;
        }
        for slot in index..self.len.saturating_sub(1) {
            let (next_note, next_flag) = match (self.notes.get(slot + 1), self.releasing.get(slot + 1)) {
                (Some(note), Some(flag)) => (*note, *flag),
                _ => break,
            };
            if let (Some(note), Some(flag)) = (self.notes.get_mut(slot), self.releasing.get_mut(slot)) {
                *note = next_note;
                *flag = next_flag;
            }
        }
        self.len = self.len.saturating_sub(1);
    }

    fn clear(&mut self) { self.len = 0; }
}

/// One MIDI channel's state.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct RackChannel {
    program: InstrumentId,
    volume: U0F16,
    pan: Option<I1F15>,
    sustain: bool,
    bend: I1F15,
    held: HeldNotes,
}

impl Default for RackChannel {
    fn default() -> RackChannel {
        RackChannel {
            program: InstrumentId(0),
            volume: U0F16::MAX,
            pan: None,
            sustain: false,
            bend: I1F15::ZERO,
            held: HeldNotes::default(),
        }
    }
}

/// Sixteen MIDI channels bound to one module's instruments.
///
/// Built once, off the audio thread ([`InstrumentRack::for_module`] allocates its
/// `Box<[Box<dyn Instrument>]>`), and swapped wholesale when the module changes — the
/// retired rack travels back over the same garbage channel a retired `Arc<Module>` does,
/// because dropping it would `free()` inside the audio callback.
///
/// Everything the rack does after that is allocation-free: fixed arrays, no locks, and
/// every lookup answers `None` rather than indexing.
pub struct InstrumentRack {
    instruments: Box<[Box<dyn Instrument>]>,
    channels: [RackChannel; MIDI_CHANNEL_COUNT],
    bend_range_cents: i16,
    unsupported_events: u32,
}

impl InstrumentRack {
    /// One instrument per slot in `module`, playing at `sample_rate_hz`.
    ///
    /// Every channel starts bound to program 0 with unity volume, no pan override, the
    /// sustain pedal up and the bend centred.
    pub fn for_module(module: &Arc<Module>, sample_rate_hz: u32) -> InstrumentRack {
        let mut instruments = Vec::with_capacity(module.instruments().len());
        for index in 0..module.instruments().len() {
            instruments.push(instrument_for(module, InstrumentId(index as u16), sample_rate_hz));
        }
        InstrumentRack {
            instruments: instruments.into_boxed_slice(),
            channels: [RackChannel::default(); MIDI_CHANNEL_COUNT],
            bend_range_cents: DEFAULT_BEND_RANGE_CENTS,
            unsupported_events: 0,
        }
    }

    /// A rack with no instruments at all — every note is silent. What a host builds before
    /// a module is loaded.
    pub fn empty() -> InstrumentRack {
        InstrumentRack {
            instruments: Vec::new().into_boxed_slice(),
            channels: [RackChannel::default(); MIDI_CHANNEL_COUNT],
            bend_range_cents: DEFAULT_BEND_RANGE_CENTS,
            unsupported_events: 0,
        }
    }

    /// How many instruments a program number may address.
    pub fn instrument_count(&self) -> usize { self.instruments.len() }

    /// Cents a full-scale pitch bend moves a note.
    pub const fn bend_range_cents(&self) -> i16 { self.bend_range_cents }

    /// Change the bend range. Off the audio thread, or from a control event.
    pub const fn set_bend_range_cents(&mut self, cents: i16) { self.bend_range_cents = cents; }

    /// Events addressed to the rack that it does not implement — the tracker-native
    /// [`Event::Trigger`] and [`Event::Param`], and voice-addressed events.
    pub const fn unsupported_events(&self) -> u32 { self.unsupported_events }

    /// The instrument MIDI channel `channel` is bound to.
    pub fn program(&self, channel: u8) -> InstrumentId {
        self.channels.get(channel as usize % MIDI_CHANNEL_COUNT).map(|state| state.program).unwrap_or(InstrumentId(0))
    }

    /// The notes MIDI channel `channel` is holding down, oldest first.
    pub fn held_notes(&self, channel: u8) -> &[u8] {
        match self.channels.get(channel as usize % MIDI_CHANNEL_COUNT) {
            Some(state) => state.held.as_slice(),
            None => &[],
        }
    }

    /// Whether MIDI channel `channel`'s sustain pedal is down.
    pub fn sustain(&self, channel: u8) -> bool {
        self.channels.get(channel as usize % MIDI_CHANNEL_COUNT).is_some_and(|state| state.sustain)
    }

    fn instrument(&self, program: InstrumentId) -> Option<&dyn Instrument> {
        self.instruments.get(program.0 as usize).map(alloc::boxed::Box::as_ref)
    }

    /// Apply one timeline event.
    ///
    /// `target` selects the channel: [`Target::Channel`] within the MIDI range addresses
    /// that channel, [`Target::Global`] addresses all sixteen, and anything else — a
    /// tracker lane, or a specific voice — is [`EventOutcome::Unsupported`].
    pub fn apply(&mut self, target: Target, event: Event, context: &mut EngineContext<'_>) -> EventOutcome {
        let outcome = match target {
            Target::Channel(channel) => match midi_channel_index(channel) {
                Some(index) => self.apply_to_channel(index, event, context),
                None => EventOutcome::Unsupported,
            },
            Target::Global => self.apply_globally(event, context),
            Target::Voice(_) => EventOutcome::Unsupported,
        };
        if outcome == EventOutcome::Unsupported {
            self.unsupported_events = self.unsupported_events.saturating_add(1);
        }
        outcome
    }

    /// A global event, applied to every MIDI channel. Only the three that mean "stop"
    /// have a whole-rack meaning; a global note-on would have no instrument to choose.
    fn apply_globally(&mut self, event: Event, context: &mut EngineContext<'_>) -> EventOutcome {
        match event {
            Event::AllNotesOff | Event::AllSoundOff | Event::Cut | Event::KeyOff | Event::FadeOut => {
                for index in 0..MIDI_CHANNEL_COUNT {
                    self.apply_to_channel(index, event, context);
                }
                EventOutcome::Applied
            }
            _ => EventOutcome::Unsupported,
        }
    }

    fn apply_to_channel(&mut self, index: usize, event: Event, context: &mut EngineContext<'_>) -> EventOutcome {
        match event {
            // MIDI's "note-on with velocity zero is a note-off" is a wire-format
            // convention, but a decoder is not the only thing that produces `NoteOn`, so
            // it is honoured here rather than only in `starplayer-midi`.
            Event::NoteOn { note, velocity } if velocity == U0F16::ZERO => self.note_off(index, note, context),
            Event::NoteOn { note, velocity } => self.note_on(index, note, velocity, context),
            Event::NoteOff { note, .. } => self.note_off(index, note, context),
            Event::KeyOff | Event::FadeOut | Event::AllNotesOff => self.all_notes_off(index, context),
            Event::Cut | Event::AllSoundOff => self.all_sound_off(index, context),
            Event::Program(program) => self.set_program(index, program),
            Event::Controller { number, value } => self.set_controller(index, number, value, context),
            Event::PitchBend(bend) => self.set_bend(index, bend, context),
            // Pressure has nowhere to go until an instrument has an articulation to point
            // it at (M11); ignored rather than unsupported, because it is squarely the
            // rack's vocabulary.
            Event::PolyAftertouch { .. } | Event::ChannelAftertouch(_) => EventOutcome::Ignored,
            // Tempo and global volume belong to the engine and the transport, not to a
            // MIDI channel's instrument.
            Event::Tempo { .. } | Event::GlobalVolume(_) => EventOutcome::Unsupported,
            // The tracker's own vocabulary (architecture §2.4): a rack must never be a
            // lowering of pattern data into MIDI.
            Event::Trigger(_) | Event::Param(_) => EventOutcome::Unsupported,
        }
    }

    fn note_on(&mut self, index: usize, note: Note, velocity: U0F16, context: &mut EngineContext<'_>) -> EventOutcome {
        let Some(state) = self.channels.get_mut(index) else { return EventOutcome::Ignored };
        state.held.push(note.to_midi());
        let (program, volume, pan, bend) = (state.program, state.volume, state.pan, state.bend);
        let Some(instrument) = self.instrument(program) else { return EventOutcome::Ignored };
        let channel = ChannelId(MIDI_CHANNEL_BASE + index as u16);
        let params = NoteParams { note, velocity: scale_unit(velocity, volume), pan_override: pan };
        if instrument.note_on(channel, params, context).is_none() {
            return EventOutcome::Ignored;
        }
        // The wheel is not re-centred by a new note, so a note struck mid-bend has to be
        // bent to where the wheel already is.
        if bend != I1F15::ZERO {
            instrument.set_bend(channel, self.bend_cents(bend), context);
        }
        EventOutcome::Applied
    }

    fn note_off(&mut self, index: usize, note: Note, context: &mut EngineContext<'_>) -> EventOutcome {
        let midi_note = note.to_midi();
        let Some(state) = self.channels.get_mut(index) else { return EventOutcome::Ignored };
        let Some(position) = state.held.position(midi_note) else { return EventOutcome::Ignored };
        if state.sustain {
            // The pedal is down: remember that this note wants to stop, and stop nothing.
            if let Some(flag) = state.held.releasing.get_mut(position) {
                *flag = true;
            }
            return EventOutcome::Applied;
        }
        state.held.remove_at(position);
        self.release_if_sounding(index, midi_note, context);
        EventOutcome::Applied
    }

    /// Stop the channel's voice if it is the one playing `note`.
    ///
    /// A channel sounds its **newest** note, so a note-off for anything else only updates
    /// the held-note list. The sounding note is read from the voice's own tag rather than
    /// remembered, for the same reason `ChannelTable::is_sounding` asks the pool: a voice
    /// that ended on its own must not leave the rack thinking a note is still down.
    fn release_if_sounding(&mut self, index: usize, note: u8, context: &mut EngineContext<'_>) -> bool {
        let channel = ChannelId(MIDI_CHANNEL_BASE + index as u16);
        let sounding = context
            .channels
            .foreground(channel)
            .and_then(|voice| context.voices.get(voice))
            .map(|state| state.tag.note);
        if sounding != Some(note) {
            return false;
        }
        let Some(program) = self.channels.get(index).map(|state| state.program) else { return false };
        let Some(instrument) = self.instrument(program) else { return false };
        instrument.note_off(channel, context);
        true
    }

    fn all_notes_off(&mut self, index: usize, context: &mut EngineContext<'_>) -> EventOutcome {
        let Some(state) = self.channels.get_mut(index) else { return EventOutcome::Ignored };
        state.held.clear();
        state.sustain = false;
        let program = state.program;
        let channel = ChannelId(MIDI_CHANNEL_BASE + index as u16);
        let Some(instrument) = self.instrument(program) else { return EventOutcome::Ignored };
        instrument.note_off(channel, context);
        EventOutcome::Applied
    }

    fn all_sound_off(&mut self, index: usize, context: &mut EngineContext<'_>) -> EventOutcome {
        let Some(state) = self.channels.get_mut(index) else { return EventOutcome::Ignored };
        state.held.clear();
        state.sustain = false;
        // Straight to the channel table rather than through the instrument: "all sound
        // off" means *now*, whatever release an instrument would otherwise run.
        context.channels.stop(ChannelId(MIDI_CHANNEL_BASE + index as u16), context.voices);
        EventOutcome::Applied
    }

    fn set_program(&mut self, index: usize, program: InstrumentId) -> EventOutcome {
        let Some(state) = self.channels.get_mut(index) else { return EventOutcome::Ignored };
        if state.program == program {
            return EventOutcome::Ignored;
        }
        // A program past the module's instrument count is stored rather than clamped: it
        // simply sounds nothing until a program that exists arrives. What a General MIDI
        // file should get instead is E5's research point 3.
        state.program = program;
        EventOutcome::Applied
    }

    fn set_controller(&mut self, index: usize, number: u16, value: U0F16, context: &mut EngineContext<'_>) -> EventOutcome {
        match number {
            controller::VOLUME => {
                let Some(state) = self.channels.get_mut(index) else { return EventOutcome::Ignored };
                // Channel volume scales the *next* note. Rewriting the sounding voice's
                // volume would fight the instrument's own articulation the moment M11
                // gives it one.
                state.volume = value;
                EventOutcome::Applied
            }
            controller::PAN => {
                let Some(state) = self.channels.get_mut(index) else { return EventOutcome::Ignored };
                state.pan = Some(pan_from_unit(value));
                EventOutcome::Applied
            }
            controller::SUSTAIN => self.set_sustain(index, value.to_bits() >= u16::MAX / 2, context),
            controller::ALL_SOUND_OFF => self.all_sound_off(index, context),
            controller::ALL_NOTES_OFF => self.all_notes_off(index, context),
            _ => EventOutcome::Ignored,
        }
    }

    /// Raise or lower the sustain pedal. Lowering it releases every note whose note-off
    /// arrived while it was down.
    fn set_sustain(&mut self, index: usize, down: bool, context: &mut EngineContext<'_>) -> EventOutcome {
        let Some(state) = self.channels.get_mut(index) else { return EventOutcome::Ignored };
        if state.sustain == down {
            return EventOutcome::Ignored;
        }
        state.sustain = down;
        if down {
            return EventOutcome::Applied;
        }
        // Newest first, so removing an entry never shifts one that has not been looked at
        // yet. The notes come out of the array before anything is released, because
        // releasing borrows the rack's instruments while the channel state is borrowed.
        let mut released = [0u8; MAX_HELD_NOTES];
        let mut count = 0usize;
        let mut position = state.held.len;
        while position > 0 {
            position -= 1;
            if !state.held.releasing.get(position).copied().unwrap_or(false) {
                continue;
            }
            let Some(note) = state.held.as_slice().get(position).copied() else { continue };
            state.held.remove_at(position);
            if let Some(slot) = released.get_mut(count) {
                *slot = note;
                count = count.saturating_add(1);
            }
        }
        for note in released.get(..count).unwrap_or(&[]) {
            self.release_if_sounding(index, *note, context);
        }
        EventOutcome::Applied
    }

    fn bend_cents(&self, bend: I1F15) -> i16 {
        ((bend.to_bits() as i32 * self.bend_range_cents as i32) / i16::MAX as i32) as i16
    }

    fn set_bend(&mut self, index: usize, bend: I1F15, context: &mut EngineContext<'_>) -> EventOutcome {
        let Some(state) = self.channels.get_mut(index) else { return EventOutcome::Ignored };
        if state.bend == bend {
            return EventOutcome::Ignored;
        }
        state.bend = bend;
        let program = state.program;
        let cents = self.bend_cents(bend);
        let Some(instrument) = self.instrument(program) else { return EventOutcome::Ignored };
        instrument.set_bend(ChannelId(MIDI_CHANNEL_BASE + index as u16), cents, context);
        EventOutcome::Applied
    }

    /// One control tick for every channel (architecture §5.4).
    pub fn control_tick(&mut self, context: &mut EngineContext<'_>) {
        for index in 0..MIDI_CHANNEL_COUNT {
            let Some(state) = self.channels.get(index) else { continue };
            let Some(instrument) = self.instrument(state.program) else { continue };
            instrument.control_tick(ChannelId(MIDI_CHANNEL_BASE + index as u16), context);
        }
    }

    /// Stop every voice on every MIDI channel and forget every held note.
    pub fn all_sound_off_everywhere(&mut self, context: &mut EngineContext<'_>) {
        for index in 0..MIDI_CHANNEL_COUNT {
            self.all_sound_off(index, context);
        }
    }
}

/// A unit controller value as a pan position: 0 hard left, half centre, full hard right.
fn pan_from_unit(value: U0F16) -> I1F15 { bipolar_from_ratio(value.to_bits() as i32 - 32_768, 32_768) }

impl core::fmt::Debug for InstrumentRack {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("InstrumentRack")
            .field("instruments", &self.instruments.len())
            .field("bend_range_cents", &self.bend_range_cents)
            .field("unsupported_events", &self.unsupported_events)
            .finish()
    }
}

// ── feeds ───────────────────────────────────────────────────────────────────────────

/// Where a [`MidiSource`] gets its events.
///
/// Two implementations by the end of M4: [`ExternalEventQueue`] over an SPSC ring, which
/// live MIDI and the computer keyboard push into (E6), and E5's `SmfSequencer`, a cursor
/// over a MIDI file's sorted event list. They differ in exactly one respect — whether the
/// next event is already known — which is what [`EventFeed::refresh`] exists for.
pub trait EventFeed: Send {
    /// Absolute frame of the next event, or `None` when the feed is idle.
    ///
    /// `&self`, because [`EventSource::next_event_frame`] is: a feed that has to *look* for
    /// its next event does that in [`EventFeed::refresh`] and answers from the lookahead.
    fn next_frame(&self) -> Option<Frame>;

    /// Take the next event if it is due at or before `frame`, else `None`.
    ///
    /// An event whose frame is already **past** `frame` is still due: the source dispatches
    /// it at the current frame and counts it late.
    fn pop_due(&mut self, frame: Frame) -> Option<TimedEvent>;

    /// Refresh whatever lookahead the feed keeps. Called once per render segment, from
    /// [`EventSource::advance_to`].
    ///
    /// The default is a no-op, which is right for any feed that already holds its events —
    /// E5's `SmfSequencer` among them. [`ExternalEventQueue`] overrides it to peek the
    /// ring, because [`EventFeed::next_frame`] cannot pop.
    fn refresh(&mut self, frame: Frame) { let _ = frame; }
}

/// The host's end of an [`ExternalEventQueue`].
///
/// Send timestamped events from any one thread: a MIDI input callback, a key handler, a
/// plugin host's per-block event list (architecture §3.2). The frame is **absolute** and
/// the host stamps it — `output_frame + lead`, two render quanta by default (E6) — so that
/// an event lands on an exact frame rather than at the top of whatever block it arrives in.
pub struct ExternalEventProducer {
    producer: Producer<TimedEvent>,
}

impl ExternalEventProducer {
    /// Queue one event, or hand it back when the ring is full.
    ///
    /// Never blocks and never allocates: a full ring is a host that is sending faster than
    /// the audio thread consumes, and the honest answer is to say so.
    pub fn send(&mut self, event: TimedEvent) -> Result<(), TimedEvent> { self.producer.push(event) }

    /// Events queued but not yet consumed.
    pub fn len(&self) -> usize { self.producer.len() }

    /// Whether nothing is queued.
    pub fn is_empty(&self) -> bool { self.producer.is_empty() }

    /// Whether the next [`ExternalEventProducer::send`] will be rejected.
    pub fn is_full(&self) -> bool { self.producer.is_full() }

    /// How many events the ring holds.
    pub fn capacity(&self) -> usize { self.producer.capacity() }
}

/// The audio thread's end: an [`EventFeed`] over an SPSC ring of [`TimedEvent`]s
/// (architecture §3.2).
///
/// # One-event lookahead
///
/// [`EventFeed::next_frame`] cannot pop, so the queue keeps one peeked event and refills it
/// in [`EventFeed::refresh`] — which the engine calls at the end of **every render
/// segment**, not once per block. An event already in the ring at a segment boundary is
/// therefore visible before the next segment is planned, and the engine splits the segment
/// on its exact frame. The host's two-quantum stamping lead is what keeps that true for an
/// event pushed while a segment is being rendered.
pub struct ExternalEventQueue {
    consumer: Consumer<TimedEvent>,
    peeked: Option<TimedEvent>,
}

/// A queue and the producer that feeds it, sized for `capacity` events.
///
/// Allocates once, off the audio thread.
pub fn external_event_channel(capacity: usize) -> (ExternalEventProducer, ExternalEventQueue) {
    let (producer, consumer) = starplayer_rt::channel(capacity);
    (ExternalEventProducer { producer }, ExternalEventQueue { consumer, peeked: None })
}

impl ExternalEventQueue {
    /// Events waiting in the ring, not counting the peeked one.
    pub fn len(&self) -> usize { self.consumer.len() }

    /// Whether the ring and the lookahead are both empty.
    pub fn is_empty(&self) -> bool { self.peeked.is_none() && self.consumer.is_empty() }
}

impl EventFeed for ExternalEventQueue {
    fn next_frame(&self) -> Option<Frame> { self.peeked.map(|event| event.frame) }

    fn pop_due(&mut self, frame: Frame) -> Option<TimedEvent> {
        let due = self.peeked.filter(|event| event.frame <= frame)?;
        self.peeked = self.consumer.pop();
        Some(due)
    }

    fn refresh(&mut self, _frame: Frame) {
        if self.peeked.is_none() {
            self.peeked = self.consumer.pop();
        }
    }
}

impl core::fmt::Debug for ExternalEventQueue {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_struct("ExternalEventQueue").field("queued", &self.len()).field("peeked", &self.peeked).finish()
    }
}

// ── the source ──────────────────────────────────────────────────────────────────────

/// An [`EventSource`] that plays a feed's events through an [`InstrumentRack`].
///
/// # Its own control tick
///
/// Master-plan decision 2: a MIDI-driven source carries its own control tick. Architecture
/// §5.4 wants a tracker's tick to *be* the control tick when one is playing, but that rate
/// belongs to the module's tempo, and a MIDI channel jammed over a stopped module would
/// then never tick at all. So the source keeps a [`ControlClock`] of its own at the engine
/// rate (~1 ms), reports `min(feed, control)` as its next frame, and ticks it from inside
/// [`EventSource::dispatch`]. Nothing in the engine had to change: it already splits its
/// render segments at whatever frame a source reports.
///
/// The engine's own [`ControlClock`] keeps running alongside, unread by this source. That
/// is deliberate — a tracker sharing the mux takes that clock over and a MIDI source must
/// not be silenced by it — and it costs a counter, because
/// [`ControlClock::tick_synthesised`] does nothing else.
pub struct MidiSource<Feed> {
    feed: Feed,
    rack: InstrumentRack,
    control: ControlClock,
    control_interval_frames: u32,
    late_events: u32,
    /// What the MIDI lanes were sounding at the last telemetry publish, so a publish only
    /// happens when the picture changed.
    #[cfg(feature = "telemetry")]
    published_sounding: u64,
}

impl<Feed: EventFeed> MidiSource<Feed> {
    /// A source playing `feed` through `rack`, ticking at the default control interval.
    pub fn new(feed: Feed, rack: InstrumentRack, sample_rate_hz: u32) -> MidiSource<Feed> {
        MidiSource::with_control_interval_micros(feed, rack, sample_rate_hz, DEFAULT_CONTROL_INTERVAL_MICROS)
    }

    /// A source whose control tick runs at a chosen interval.
    pub fn with_control_interval_micros(
        feed: Feed,
        rack: InstrumentRack,
        sample_rate_hz: u32,
        interval_micros: u32,
    ) -> MidiSource<Feed> {
        let control = ControlClock::with_interval_micros(sample_rate_hz, Frame::ZERO, interval_micros);
        MidiSource {
            feed,
            rack,
            control_interval_frames: control.interval_frames(),
            control,
            late_events: 0,
            #[cfg(feature = "telemetry")]
            published_sounding: 0,
        }
    }

    /// The feed.
    pub const fn feed(&self) -> &Feed { &self.feed }

    /// The feed, mutably — for a host that seeks an SMF or drains a queue.
    pub const fn feed_mut(&mut self) -> &mut Feed { &mut self.feed }

    /// The rack.
    pub const fn rack(&self) -> &InstrumentRack { &self.rack }

    /// The rack, mutably.
    pub const fn rack_mut(&mut self) -> &mut InstrumentRack { &mut self.rack }

    /// Replace the rack — what a module swap does. The retired rack is handed back rather
    /// than dropped, because dropping it inside the audio callback is a `free()`.
    pub fn replace_rack(&mut self, rack: InstrumentRack) -> InstrumentRack {
        core::mem::replace(&mut self.rack, rack)
    }

    /// Events that arrived at a frame already past. They were dispatched at the current
    /// frame; the count is what
    /// [`EngineWarnings::late_events`](crate::EngineWarnings) reports.
    pub const fn late_events(&self) -> u32 { self.late_events }

    /// Control ticks this source has taken.
    pub const fn control_ticks(&self) -> u64 { self.control.ticks() }

    /// Frames between control ticks.
    pub const fn control_interval_frames(&self) -> u32 { self.control_interval_frames }

    /// Start the control clock at `frame`, for a source installed mid-render.
    pub fn start_control_at(&mut self, frame: Frame) { self.control.resume_synthesis(frame); }

    /// Take one control tick and let the rack advance whatever it has.
    fn run_control_ticks(&mut self, frame: Frame, context: &mut EngineContext<'_>) {
        let mut taken = 0u32;
        while self.control.next_synthesised_frame().is_some_and(|next| next <= frame) {
            self.control.tick_synthesised();
            self.rack.control_tick(context);
            taken = taken.saturating_add(1);
            if taken >= MAX_CONTROL_TICKS_PER_DISPATCH {
                // Installed long after its clock was started, or resumed after a seek.
                // Resynchronise rather than catching up one interval at a time inside the
                // audio callback.
                self.control.resume_synthesis(frame.saturating_add(self.control_interval_frames as u64));
                break;
            }
        }
    }

    /// Publish the sixteen MIDI lanes when what they are sounding has changed.
    ///
    /// Not once per control tick: at ~1 kHz that would copy a whole snapshot a thousand
    /// times a second to say nothing. The fingerprint is the sounding note per lane, so a
    /// note starting, ending, changing or being stolen publishes and a held chord does not.
    #[cfg(feature = "telemetry")]
    fn publish_telemetry(&mut self, context: &mut EngineContext<'_>) {
        let mut fingerprint = 0u64;
        for index in 0..MIDI_CHANNEL_COUNT {
            let channel = ChannelId(MIDI_CHANNEL_BASE + index as u16);
            let note = context
                .channels
                .foreground(channel)
                .and_then(|voice| context.voices.get(voice))
                .map(|state| state.tag.note as u64 + 1)
                .unwrap_or(0);
            fingerprint = fingerprint.rotate_left(4) ^ (note << (index % 8));
        }
        if fingerprint == self.published_sounding {
            return;
        }
        self.published_sounding = fingerprint;
        let Some(telemetry) = context.telemetry.as_deref_mut() else { return };
        crate::telemetry::capture_midi_channels(telemetry, context.channels, context.voices);
        telemetry.publish();
    }
}

impl<Feed: EventFeed> EventSource for MidiSource<Feed> {
    fn next_event_frame(&self) -> Option<Frame> {
        match (self.feed.next_frame(), self.control.next_synthesised_frame()) {
            (Some(feed), Some(control)) => Some(feed.min(control)),
            (feed, control) => feed.or(control),
        }
    }

    fn advance_to(&mut self, frame: Frame) { self.feed.refresh(frame); }

    fn dispatch(&mut self, frame: Frame, context: &mut EngineContext<'_>) {
        while let Some(event) = self.feed.pop_due(frame) {
            if event.frame < frame {
                // Architecture §3.1: the engine tolerates a source reporting a past frame
                // and treats it as due now. Dropping the event instead would make a
                // slightly-late keystroke silently vanish.
                self.late_events = self.late_events.saturating_add(1);
                context.report_late_event();
            }
            self.rack.apply(event.target, event.event, context);
        }
        self.run_control_ticks(frame, context);
        #[cfg(feature = "telemetry")]
        self.publish_telemetry(context);
    }
}

impl<Feed> core::fmt::Debug for MidiSource<Feed> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("MidiSource")
            .field("rack", &self.rack)
            .field("control_ticks", &self.control.ticks())
            .field("late_events", &self.late_events)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starplayer_core::fixed::unit_from_ratio;

    /// Research point 1, and master-plan decision 4: the pitch convention is exact, not
    /// nearly exact, at the reference note — for every reference rate a file can carry.
    #[test]
    fn midi_note_60_plays_a_sample_at_its_reference_rate() {
        for reference_rate_hz in [8_363u32, 8_000, 11_025, 16_726, 22_050, 44_100, 1] {
            let step = step_for(reference_rate_hz, 0, 44_100);
            assert_eq!(step, Step::from_ratio(reference_rate_hz as u64, 44_100), "reference rate {reference_rate_hz}");
        }
        assert_eq!(scale_frequency(8_363, 0), 8_363);
        assert_eq!(scale_frequency(8_363, 12 * LINEAR_UNITS_PER_SEMITONE), 16_726, "an octave up doubles");
        assert_eq!(scale_frequency(8_363, -12 * LINEAR_UNITS_PER_SEMITONE), 4_182, "an octave down halves, rounded");
    }

    /// Research point 2: one unit is 1/64 of a semitone, so a bend quantises to 1.5625
    /// cents and a bend of zero is exactly no bend at all.
    #[test]
    fn a_bend_quantises_to_one_sixty_fourth_of_a_semitone() {
        assert_eq!(cents_to_units(0), 0);
        assert_eq!(cents_to_units(100), LINEAR_UNITS_PER_SEMITONE, "a semitone is 64 units");
        assert_eq!(cents_to_units(-100), -LINEAR_UNITS_PER_SEMITONE);
        assert_eq!(cents_to_units(200), 2 * LINEAR_UNITS_PER_SEMITONE, "the default bend range is two semitones");
        assert_eq!(cents_to_units(1), 1, "1 cent rounds up to one unit");
        assert_eq!(cents_to_units(2), 1, "and so does 2 — 1.28 units");
        assert_eq!(cents_to_units(-2), -1);
        assert_eq!(units_for_note(Note::new(REFERENCE_NOTE), 0), 0, "a bend of zero changes nothing");
        assert_eq!(units_for_note(Note::new(REFERENCE_NOTE + 12), 0), 12 * LINEAR_UNITS_PER_SEMITONE);
        assert_eq!(units_for_note(Note::with_cents(REFERENCE_NOTE, 50), 0), 32, "half a semitone of cents");
        assert_eq!(units_for_note(Note::new(REFERENCE_NOTE), 100), LINEAR_UNITS_PER_SEMITONE, "bend adds to the note");
    }

    /// Unity times unity is unity: the four volume terms multiply without losing a bit at
    /// each step, which a `>> 16` would.
    #[test]
    fn unit_scaling_keeps_full_scale_full() {
        assert_eq!(scale_unit(U0F16::MAX, U0F16::MAX), U0F16::MAX);
        assert_eq!(scale_unit(U0F16::MAX, U0F16::ZERO), U0F16::ZERO);
        let half = unit_from_ratio(1, 2);
        assert_eq!(scale_unit(U0F16::MAX, half), half, "unity is the identity");
        assert_eq!(scale_unit(half, half).to_bits(), 16_384, "a quarter, rounded");
    }

    #[test]
    fn a_midi_channel_maps_onto_the_engines_top_lanes() {
        assert_eq!(midi_channel(0), ChannelId(48));
        assert_eq!(midi_channel(15), ChannelId(63));
        assert_eq!(midi_channel(16), ChannelId(48), "a channel past sixteen wraps rather than reaching a tracker lane");
        assert_eq!(midi_channel_index(ChannelId(48)), Some(0));
        assert_eq!(midi_channel_index(ChannelId(63)), Some(15));
        assert_eq!(midi_channel_index(ChannelId(47)), None, "a tracker lane is not a MIDI channel");
        assert_eq!(midi_channel_index(ChannelId(64)), None);
    }

    /// Research point 4: the seventeenth held note steals the oldest, and the stolen note
    /// is forgotten rather than stopped.
    #[test]
    fn holding_more_notes_than_the_array_steals_the_oldest() {
        let mut held = HeldNotes::default();
        for note in 0..MAX_HELD_NOTES as u8 {
            held.push(60 + note);
        }
        assert_eq!(held.len, MAX_HELD_NOTES);
        assert_eq!(held.as_slice().first(), Some(&60));

        held.push(90);
        assert_eq!(held.len, MAX_HELD_NOTES, "the array never grows");
        assert_eq!(held.as_slice().first(), Some(&61), "the oldest went");
        assert_eq!(held.as_slice().last(), Some(&90), "and the newest is last");
        assert_eq!(held.position(60), None, "a stolen note's own note-off finds nothing to release");

        held.push(90);
        assert_eq!(held.len, MAX_HELD_NOTES, "re-striking a held note does not hold it twice");

        assert!(held.position(75).is_some());
        held.remove_at(held.position(75).expect("held"));
        assert_eq!(held.position(75), None);
        assert_eq!(held.len, MAX_HELD_NOTES - 1);
        held.clear();
        assert!(held.as_slice().is_empty());
    }

    #[test]
    fn the_external_queue_shows_an_event_only_once_it_has_been_refreshed() {
        let (mut producer, mut queue) = external_event_channel(4);
        assert!(queue.is_empty());
        assert_eq!(queue.next_frame(), None);

        let note_on = Event::NoteOn { note: Note::MIDDLE_C, velocity: U0F16::MAX };
        assert!(producer.send(TimedEvent::on_channel(Frame(500), midi_channel(0), note_on)).is_ok());
        assert_eq!(queue.next_frame(), None, "the lookahead is filled by `refresh`, which the engine calls per segment");

        queue.refresh(Frame(0));
        assert_eq!(queue.next_frame(), Some(Frame(500)));
        assert_eq!(queue.pop_due(Frame(499)), None, "nothing is due before its frame");
        assert!(queue.pop_due(Frame(500)).is_some());
        assert_eq!(queue.next_frame(), None);
        assert!(queue.is_empty());
    }

    #[test]
    fn a_full_external_queue_hands_the_event_back_rather_than_blocking() {
        let (mut producer, _queue) = external_event_channel(2);
        let event = TimedEvent::global(Frame(0), Event::AllSoundOff);
        assert!(producer.send(event).is_ok());
        assert!(producer.send(event).is_ok());
        assert!(producer.is_full());
        assert_eq!(producer.send(event), Err(event), "a full ring rejects rather than growing");
        assert_eq!(producer.capacity(), 2);
        assert_eq!(producer.len(), 2);
    }
}
