//! The three kinds of state, as three separate types (architecture §2).
//!
//! * **Timeline events** — [`TimedEvent`], the only thing carrying a timestamp.
//! * **Voice parameters** — [`VoiceParams`] plus [`DirtyBits`]; written directly, never
//!   queued. At 32 channels × 50 ticks/s with vibrato, tremolo and envelopes running,
//!   encoding these as events would be 5,000–10,000 messages a second describing field
//!   writes to a struct the writer already holds. The 80386 original did the right thing:
//!   **write field, set dirty bit.**
//! * **Control commands** — [`Command`], sparse, cross-thread, never on the timeline.
//!
//! Everything here is declarations and trivial constructors. The behaviour lands in M1.

use bitflags::bitflags;

use crate::fixed::{I1F15, Step, U0F16};
use crate::frame::Frame;
use crate::note::Note;

// ── identifiers ─────────────────────────────────────────────────────────────────────

/// A logical control lane: a tracker pattern column, or a MIDI channel.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelId(pub u16);

/// A handle to one sounding voice in the global pool (architecture §5.2).
///
/// Generational, because a voice **can** be stolen out from under its owner: a stale
/// handle must resolve to `None`, not to somebody else's voice. The fields are private so
/// a handle can only come from the pool that minted it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VoiceId {
    index: u16,
    generation: u16,
}

impl VoiceId {
    /// Mint a handle. Only the voice pool should call this.
    pub const fn new(index: u16, generation: u16) -> VoiceId { VoiceId { index, generation } }

    /// Slot index within the pool.
    pub const fn index(self) -> u16 { self.index }

    /// Generation counter, bumped every time the slot is reused.
    pub const fn generation(self) -> u16 { self.generation }
}

/// An instrument slot within the loaded module.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstrumentId(pub u16);

/// A sample slot within the loaded module.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SampleId(pub u16);

// ── (1) the timeline ────────────────────────────────────────────────────────────────

/// What an [`Event`] is addressed to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// A control lane. The usual case: the channel's instrument decides which voice, if
    /// any, the event reaches.
    Channel(ChannelId),
    /// One specific sounding voice — how an IT background voice is addressed after it has
    /// been detached from its channel.
    Voice(VoiceId),
    /// The engine as a whole: tempo, global volume, all-notes-off.
    Global,
}

/// A timeline event at an absolute output frame.
///
/// Architecture §2.1 writes the timestamp as a bare `u64`; [`Frame`] is the same
/// representation with the monotonicity invariant attached to it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TimedEvent {
    /// Absolute output frame at which this event takes effect.
    pub frame: Frame,
    /// Who it is addressed to.
    pub target: Target,
    /// What happens.
    pub event: Event,
}

impl TimedEvent {
    /// An event addressed to a channel.
    pub const fn on_channel(frame: Frame, channel: ChannelId, event: Event) -> TimedEvent {
        TimedEvent { frame, target: Target::Channel(channel), event }
    }

    /// An event addressed to one specific voice.
    pub const fn on_voice(frame: Frame, voice: VoiceId, event: Event) -> TimedEvent {
        TimedEvent { frame, target: Target::Voice(voice), event }
    }

    /// An event addressed to the engine as a whole.
    pub const fn global(frame: Frame, event: Event) -> TimedEvent {
        TimedEvent { frame, target: Target::Global, event }
    }
}

/// A musical event.
///
/// The MIDI-shaped variants are the *musical* layer; [`Event::Trigger`] and
/// [`Event::Param`] are the tracker-native peers of it, not a lowering of it
/// (architecture §2.4). MOD/S3M/MTM patterns never produce a `NoteOn`.
///
/// Velocities, pressures, controller values and volumes are [`U0F16`] rather than 7-bit
/// bytes, because seven bits cannot round-trip S3M volume (0–64), IT global volume
/// (0–128) or IT pan (0–64 plus surround) — architecture §2.3.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Start a note.
    NoteOn { note: Note, velocity: U0F16 },
    /// Release a note.
    NoteOff { note: Note, velocity: U0F16 },
    /// Release whatever is sounding, entering the release stage of its envelopes.
    KeyOff,
    /// Begin the fadeout ramp (IT).
    FadeOut,
    /// Stop immediately, without a release stage.
    Cut,
    /// Per-note pressure.
    PolyAftertouch { note: Note, pressure: U0F16 },
    /// Channel-wide pressure.
    ChannelAftertouch(U0F16),
    /// A continuous controller. `number` is wider than MIDI's 7 bits so that a
    /// high-resolution or non-MIDI controller does not have to be squeezed into a byte.
    Controller { number: u16, value: U0F16 },
    /// Pitch bend, bipolar.
    PitchBend(I1F15),
    /// Bind an instrument to the target channel.
    Program(InstrumentId),
    /// Release every sounding note on the target.
    AllNotesOff,
    /// Silence the target immediately.
    AllSoundOff,

    /// Tracker-native note-on: play this sample from this offset.
    Trigger(TriggerSpec),
    /// Set one voice parameter absolutely — the escape hatch for anything the musical
    /// vocabulary cannot say.
    Param(VoiceParam),
    /// Change tempo and/or ticks-per-row.
    Tempo { bpm: u16, speed: u8 },
    /// Set the global volume.
    GlobalVolume(U0F16),
}

/// A tracker-native note-on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TriggerSpec {
    /// Which sample to play.
    pub sample: SampleId,
    /// Where in the sample to start, in source frames. `Oxx` sets this; a fresh trigger
    /// otherwise resets it to zero, exactly as `__UpdateTracker` clears `_SampleOffset`.
    pub offset_frames: u32,
    /// What the trigger should *not* reset.
    pub flags: TriggerFlags,
}

impl TriggerSpec {
    /// A plain trigger from the start of the sample.
    pub const fn new(sample: SampleId) -> TriggerSpec {
        TriggerSpec { sample, offset_frames: 0, flags: TriggerFlags::empty() }
    }

    /// A trigger from a sample offset (`Oxx`).
    pub const fn at_offset(sample: SampleId, offset_frames: u32) -> TriggerSpec {
        TriggerSpec { sample, offset_frames, flags: TriggerFlags::empty() }
    }
}

bitflags! {
    /// Modifiers on a [`TriggerSpec`], all of them "keep something the default trigger
    /// would reset".
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
    pub struct TriggerFlags: u8 {
        /// Do not retrigger: glide the current voice to the new pitch instead. This is
        /// what `Gxx` and `Lxy` produce, and it is why the original's tick-0 portamento
        /// detection checks `_ActiveFlag` before suppressing the retrigger
        /// (`__UpdateTracker`, `STARPLAY/S3MLIB.ASM` ~2544).
        const TONE_PORTAMENTO = 0b0000_0001;
        /// Do not reset the channel volume to the sample's default volume.
        const KEEP_VOLUME     = 0b0000_0010;
        /// Do not reset the sample playback position.
        const KEEP_POSITION   = 0b0000_0100;
    }
}

// ── (2) voice state ─────────────────────────────────────────────────────────────────

/// The parameters of one sounding voice. Written directly by whoever owns the voice,
/// never queued (architecture §2.1).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceParams {
    /// Q32.32 sample-position increment per output frame.
    pub step: Step,
    /// Linear voice volume.
    pub volume: U0F16,
    /// Pan: −1.0 hard left, 0 centre, +1.0 hard right.
    pub pan: I1F15,
    /// Voice-level resonant filter (IT). Not an insert effect — it belongs to the voice.
    pub filter: FilterParams,
    /// Which of the above the driver still has to act on.
    pub dirty: DirtyBits,
}

impl VoiceParams {
    /// A silent, centred, unfiltered voice with nothing pending.
    pub const SILENT: VoiceParams = VoiceParams {
        step: Step::ZERO,
        volume: U0F16::ZERO,
        pan: I1F15::ZERO,
        filter: FilterParams::BYPASS,
        dirty: DirtyBits::empty(),
    };

    /// Set the step and flag it.
    pub fn set_step(&mut self, step: Step) {
        self.step = step;
        self.dirty.insert(DirtyBits::PITCH);
    }

    /// Set the volume and flag it.
    pub fn set_volume(&mut self, volume: U0F16) {
        self.volume = volume;
        self.dirty.insert(DirtyBits::VOLUME);
    }

    /// Set the pan and flag it.
    pub fn set_pan(&mut self, pan: I1F15) {
        self.pan = pan;
        self.dirty.insert(DirtyBits::PAN);
    }

    /// Set the filter and flag it as a pitch/timbre change.
    pub fn set_filter(&mut self, filter: FilterParams) {
        self.filter = filter;
        self.dirty.insert(DirtyBits::PITCH);
    }

    /// Clear every dirty bit. The driver calls this once it has consumed them, the way
    /// the original's `SB_ProcessTracks` ends with `mov [edi+_ChannelFlag],0`
    /// (`STARPLAY/S3MLIB.ASM` ~5828).
    pub fn clear_dirty(&mut self) { self.dirty = DirtyBits::empty(); }
}

/// Voice-level resonant filter parameters (IT).
///
/// Carried as unit scalars; turning them into biquad coefficients is M6's job, and will
/// be a table lookup rather than a `tan`/`exp` call — design goal 5 bans transcendental
/// functions from the RT path.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FilterParams {
    /// Normalised cutoff. [`U0F16::MAX`] is fully open.
    pub cutoff: U0F16,
    /// Normalised resonance. Zero is none.
    pub resonance: U0F16,
}

/// A default filter is a *bypassed* filter, not a closed one — a zeroed `FilterParams`
/// would silence every MOD and S3M voice in the engine.
impl Default for FilterParams {
    fn default() -> FilterParams { FilterParams::BYPASS }
}

impl FilterParams {
    /// Fully open, no resonance — the filter is not doing anything.
    pub const BYPASS: FilterParams = FilterParams { cutoff: U0F16::MAX, resonance: U0F16::ZERO };

    /// Whether these parameters leave the signal untouched, so the mixer can skip the
    /// filter entirely for MOD/S3M/MTM.
    pub const fn is_bypass(self) -> bool { self.cutoff.to_bits() == u16::MAX && self.resonance.to_bits() == 0 }

    /// The scale factor that puts Impulse Tracker's 0..=255 mixer domain onto the whole
    /// `U0F16` range: `255 · 257 == 65535`, so the encoding is lossless in both
    /// directions and no rounding rule has to be agreed twice.
    const IT_SCALE: u16 = 257;

    /// Voice filter parameters from Impulse Tracker's own **0..=127** cutoff and
    /// resonance — an instrument's `IFC`/`IFR` bytes, or a `Zxx` macro's parameter.
    ///
    /// IT's replayers work the filter in a 0..=255 domain that is twice the file's
    /// 0..=127 one (libxmp `player.c` `apply_midi_macro_effect`: `xc->filter.cutoff =
    /// val << 1`), and that is the domain the conformance oracle dumps and the C1 trace's
    /// `unit_to_scale(bits, 255)` reads back. Encoding `value · 2 · 257` therefore makes
    /// the trace field, the oracle column and [`FilterParams::to_it`] all agree exactly.
    ///
    /// Cutoff 127 with no resonance is not a filter at all in IT — "resonance is only
    /// ever applied if the cutoff is not full or the resonance is not zero" (OpenMPT
    /// `filter-7F.it`) — so it answers [`FilterParams::BYPASS`] rather than a
    /// nearly-open filter.
    pub const fn from_it(cutoff: u8, resonance: u8) -> FilterParams {
        if cutoff >= 127 && resonance == 0 { return FilterParams::BYPASS; }
        FilterParams::from_it_scaled(saturating_double(cutoff), resonance)
    }

    /// [`FilterParams::from_it`] with the cutoff already in the replayer's **0..=255**
    /// domain — what the filter envelope produces, since IT scales the instrument's
    /// cutoff by the envelope before the coefficients are derived.
    pub const fn from_it_scaled(cutoff: u8, resonance: u8) -> FilterParams {
        if cutoff >= 254 && resonance == 0 { return FilterParams::BYPASS; }
        FilterParams {
            cutoff: U0F16::from_bits(cutoff as u16 * FilterParams::IT_SCALE),
            resonance: U0F16::from_bits(saturating_double(resonance) as u16 * FilterParams::IT_SCALE),
        }
    }

    /// The Impulse Tracker **0..=127** cutoff and resonance these parameters encode —
    /// the pair the resonant-filter coefficients are derived from.
    ///
    /// The inverse of [`FilterParams::from_it`] for every value it can produce, and a
    /// rounded nearest answer for parameters some other format wrote.
    pub const fn to_it(self) -> (u8, u8) {
        (scale_to_it(self.cutoff.to_bits()), scale_to_it(self.resonance.to_bits()))
    }
}

/// `value · 2`, pinned at 255 — IT's 0..=127 domain widened to the replayer's 0..=255 one.
const fn saturating_double(value: u8) -> u8 {
    if value >= 128 { 255 } else { value * 2 }
}

/// A `U0F16` back to Impulse Tracker's 0..=127 domain, rounded to nearest.
const fn scale_to_it(bits: u16) -> u8 {
    ((bits as u32 * 127 + u16::MAX as u32 / 2) / u16::MAX as u32) as u8
}

/// One absolutely-set voice parameter, as carried by [`Event::Param`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VoiceParam {
    /// Set the resample increment.
    Step(Step),
    /// Set the voice volume.
    Volume(U0F16),
    /// Set the pan position.
    Pan(I1F15),
    /// Set the voice filter.
    Filter(FilterParams),
}

impl VoiceParam {
    /// Apply this parameter to a [`VoiceParams`], setting the matching dirty bit.
    pub fn apply(self, params: &mut VoiceParams) {
        match self {
            VoiceParam::Step(step) => params.set_step(step),
            VoiceParam::Volume(volume) => params.set_volume(volume),
            VoiceParam::Pan(pan) => params.set_pan(pan),
            VoiceParam::Filter(filter) => params.set_filter(filter),
        }
    }
}

bitflags! {
    /// What the driver still has to act on for a voice.
    ///
    /// A direct descendant of the original's `_CHN_New*` channel-update flags — the bit
    /// values are the same ones (`STARPLAY/S3MLIB.INC` lines 55–65 for `NewVol`…`NewBPM`,
    /// and `STARPLAY/S3MLIB.ASM` line 237 for `_CHN_StopVoice`, which lives with the GUS
    /// driver rather than in the shared include):
    ///
    /// ```text
    /// _CHN_NewVol     equ     00000001b       ;Change/New chan volume
    /// _CHN_NewSamp    equ     00000010b       ;New sample/sample point
    /// _CHN_NewPitch   equ     00000100b       ;Change/New sample pitch
    /// _CHN_NewPan     equ     00001000b       ;Set pan position for channel
    /// _CHN_NewBPM     equ     00010000b       ;Change song BPM setting
    /// _CHN_StopVoice  equ     10000000b       ;*Special: only for gus
    /// ```
    ///
    /// The design is thirty years old and still correct.
    #[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
    pub struct DirtyBits: u8 {
        /// Volume changed. Was `_CHN_NewVol`.
        const VOLUME = 0b0000_0001;
        /// A new sample, or a new position within one. Was `_CHN_NewSamp`.
        const SAMPLE = 0b0000_0010;
        /// Pitch changed. Was `_CHN_NewPitch`.
        const PITCH  = 0b0000_0100;
        /// Pan changed. Was `_CHN_NewPan`.
        const PAN    = 0b0000_1000;
        /// Song tempo changed. Was `_CHN_NewBPM`.
        const TEMPO  = 0b0001_0000;
        /// Stop this voice. Was `_CHN_StopVoice`.
        const STOP   = 0b1000_0000;
    }
}

// ── (3) the control plane ───────────────────────────────────────────────────────────

/// A control-plane command, delivered over an SPSC ring and never on the timeline.
///
/// # Why the module handle is a type parameter
///
/// Architecture §2.1 writes the first variant as `LoadModule(Arc<Module>)`. Neither half
/// of that can appear literally in `starplayer-core`:
///
/// * `Module` lives in `starplayer-model`, and core is the root of the dependency graph —
///   it must never depend on another StarPlayer crate.
/// * `alloc::sync::Arc` needs native compare-and-swap, which
///   `riscv32imc-unknown-none-elf` does not have. Core has to compile for that target on
///   every commit, so naming `Arc` here would break the CI matrix outright. (Architecture
///   §10 anticipates this: `portable-atomic` + `critical-section` are the fix, and they
///   belong in `starplayer-rt`, not here.)
///
/// Making the handle a type parameter satisfies both without weakening the shape: the
/// engine writes `Command<Arc<Module>>` on a target that has atomics, an embedded host
/// writes `Command<&'static Module>` or `Command<ModuleSlot>`, and core stays free of
/// both dependencies. The alternative — dropping `LoadModule` from core and having the
/// engine define its own superset enum — would fork the command vocabulary in two, which
/// is worse.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Command<ModuleHandle> {
    /// Swap in a module that was loaded off the audio thread. The retired handle goes
    /// back over the garbage channel to be dropped by the control thread (architecture
    /// §8) — the audio thread must never run a deallocator.
    LoadModule(ModuleHandle),
    /// Start playback.
    Play,
    /// Stop playback.
    Stop,
    /// Jump to a position in the order list.
    SeekOrder(u16),
    /// Jump to a row within the current pattern.
    SeekRow(u16),
    /// Jump to an elapsed position in the song, in frames from the song's start.
    ///
    /// Needs a scanned song timeline to resolve the frame to a row; a host without one
    /// has nothing to seek against.
    SeekFrame(u64),
    /// Choose what happens when the song reaches its detected loop point.
    SetAtEnd(AtEnd),
    /// Set the master volume.
    SetMasterVolume(U0F16),
    /// Mute or unmute one channel.
    MuteChannel { channel: ChannelId, muted: bool },
    /// Change the resampling interpolator.
    SetInterpolator(Interpolator),
    /// Change the tempo model.
    SetTempoModel(crate::tempo::TempoModelId),
}

/// What a player does when the song **has been heard through once** — at its detected loop
/// point, or when its order list runs out.
///
/// This is not the same question as
/// [`EndOfSongPolicy`](https://docs.rs/starplayer-engine): that one says what the *end of
/// the order list* means, which is a property of the module and its format. This one says
/// what the *host* wants to happen once the song has been heard once through — the media
/// player's repeat button — and it is answered by the loop detector rather than by the
/// order list.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum AtEnd {
    /// Wrap and keep playing, from the loop point or from the restart order. Repeat on, and
    /// the default.
    #[default]
    Continue,
    /// Stop dead there. Voices ring out; no further ticks.
    Stop,
    /// Keep playing past the **loop point** and let the host fade the transport out.
    ///
    /// A song whose order list simply ran out has no second pass to fade into, so it stops
    /// there exactly as [`AtEnd::Stop`] does (task D2).
    ///
    /// The engine does not fade: it only reports that the point was passed, through
    /// [`TransportState::end_reached`](https://docs.rs/starplayer-telemetry).
    FadeOut,
}

/// Which resampling kernel the mixer uses (architecture §7.1).
///
/// Selected as a monomorphised parameter of the inner loop, never a `dyn` call per
/// sample; this enum is the control-plane tag that picks the monomorphisation.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Interpolator {
    /// Nearest-neighbour. Retro character, cheapest.
    None,
    /// Linear. The default, and the golden-hash reference.
    #[default]
    Linear,
    /// Cubic Hermite. Quality real-time.
    Cubic,
    /// Windowed sinc. Offline and high-quality rendering.
    Sinc,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixed::unit_from_ratio;
    use crate::tempo::TempoModelId;

    #[test]
    fn dirty_bits_match_the_original_flag_values() {
        assert_eq!(DirtyBits::VOLUME.bits(), 0b0000_0001, "_CHN_NewVol");
        assert_eq!(DirtyBits::SAMPLE.bits(), 0b0000_0010, "_CHN_NewSamp");
        assert_eq!(DirtyBits::PITCH.bits(), 0b0000_0100, "_CHN_NewPitch");
        assert_eq!(DirtyBits::PAN.bits(), 0b0000_1000, "_CHN_NewPan");
        assert_eq!(DirtyBits::TEMPO.bits(), 0b0001_0000, "_CHN_NewBPM");
        assert_eq!(DirtyBits::STOP.bits(), 0b1000_0000, "_CHN_StopVoice");
        assert_eq!(DirtyBits::all().bits(), 0b1001_1111, "bits 5 and 6 are unused, as in the original");
    }

    #[test]
    fn setters_flag_exactly_what_they_changed() {
        let mut params = VoiceParams::SILENT;
        assert!(params.dirty.is_empty());

        params.set_volume(unit_from_ratio(64, 64));
        assert_eq!(params.dirty, DirtyBits::VOLUME);
        assert_eq!(params.volume, U0F16::MAX);

        params.set_pan(I1F15::ZERO);
        assert_eq!(params.dirty, DirtyBits::VOLUME | DirtyBits::PAN);

        params.set_step(Step::ONE);
        assert_eq!(params.dirty, DirtyBits::VOLUME | DirtyBits::PAN | DirtyBits::PITCH);
        assert_eq!(params.step, Step::ONE);

        params.clear_dirty();
        assert!(params.dirty.is_empty(), "the driver clears the flags once it has consumed them");
    }

    #[test]
    fn voice_param_apply_sets_the_matching_bit() {
        let mut params = VoiceParams::SILENT;
        VoiceParam::Pan(I1F15::MAX).apply(&mut params);
        assert_eq!(params.dirty, DirtyBits::PAN);
        assert_eq!(params.pan, I1F15::MAX);

        let filter = FilterParams { cutoff: unit_from_ratio(1, 2), resonance: unit_from_ratio(1, 4) };
        VoiceParam::Filter(filter).apply(&mut params);
        assert_eq!(params.filter, filter);
        assert!(!filter.is_bypass());
        assert!(FilterParams::BYPASS.is_bypass());
    }

    /// The joint G2/G3 encoding: the C1 trace's `unit_to_scale(bits, 255)` must read back
    /// the oracle's 0..=255 column, and the filter's coefficient inputs must read back
    /// Impulse Tracker's own 0..=127 pair.
    #[test]
    fn the_it_filter_encoding_round_trips_through_both_domains() {
        fn trace_scale(bits: u16) -> u16 { ((bits as u32 * 255 + 32_767) / 65_535) as u16 }

        for cutoff in 0..=127u8 {
            for resonance in [0u8, 1, 63, 64, 127] {
                let params = FilterParams::from_it(cutoff, resonance);
                if cutoff == 127 && resonance == 0 {
                    assert!(params.is_bypass(), "IT's full cutoff with no resonance is no filter at all");
                    continue;
                }
                assert_eq!(params.to_it(), (cutoff, resonance), "cutoff {cutoff}, resonance {resonance}");
                assert_eq!(trace_scale(params.cutoff.to_bits()), cutoff as u16 * 2, "the trace reads libxmp's doubled cutoff");
                assert_eq!(trace_scale(params.resonance.to_bits()), resonance as u16 * 2, "the trace reads libxmp's doubled resonance");
            }
        }

        // An envelope-scaled cutoff keeps the 0..=255 domain exactly.
        for scaled in 0..=253u8 {
            let params = FilterParams::from_it_scaled(scaled, 0);
            assert_eq!(trace_scale(params.cutoff.to_bits()), scaled as u16, "scaled cutoff {scaled}");
        }
        assert!(FilterParams::from_it_scaled(254, 0).is_bypass());
        assert!(!FilterParams::from_it_scaled(254, 1).is_bypass());
    }

    #[test]
    fn timed_event_constructors_set_the_target() {
        let note_on = Event::NoteOn { note: Note::A440, velocity: U0F16::MAX };
        assert_eq!(TimedEvent::on_channel(Frame(128), ChannelId(3), note_on).target, Target::Channel(ChannelId(3)));
        assert_eq!(TimedEvent::on_voice(Frame(128), VoiceId::new(7, 2), note_on).target, Target::Voice(VoiceId::new(7, 2)));
        assert_eq!(TimedEvent::global(Frame(0), Event::AllSoundOff).target, Target::Global);
        assert_eq!(TimedEvent::global(Frame(64), Event::AllSoundOff).frame, Frame(64));
    }

    #[test]
    fn voice_ids_of_different_generations_are_distinct() {
        assert_ne!(VoiceId::new(4, 0), VoiceId::new(4, 1), "a stolen voice must not answer to its old handle");
        assert_eq!(VoiceId::new(4, 1).index(), 4);
        assert_eq!(VoiceId::new(4, 1).generation(), 1);
    }

    #[test]
    fn trigger_spec_defaults_to_the_start_of_the_sample() {
        let trigger = TriggerSpec::new(SampleId(9));
        assert_eq!(trigger.offset_frames, 0);
        assert!(trigger.flags.is_empty());
        assert_eq!(TriggerSpec::at_offset(SampleId(9), 4096).offset_frames, 4096);
        assert!(TriggerFlags::TONE_PORTAMENTO.contains(TriggerFlags::TONE_PORTAMENTO));
    }

    /// The engine instantiates the command enum over whatever module handle its target
    /// supports; core never names `Arc` or `Module`.
    #[test]
    fn commands_are_generic_over_the_module_handle() {
        let load: Command<u32> = Command::LoadModule(7);
        assert_eq!(load, Command::LoadModule(7));
        let tempo: Command<u32> = Command::SetTempoModel(TempoModelId::St3Truncating);
        assert_ne!(tempo, load);
        assert_eq!(Interpolator::default(), Interpolator::Linear);
        let mute: Command<u32> = Command::MuteChannel { channel: ChannelId(1), muted: true };
        assert_ne!(mute, Command::<u32>::Stop);
    }
}
