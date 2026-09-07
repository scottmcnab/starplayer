//! Impulse Tracker's tick-zero and per-tick effect processor, its instrument
//! articulation, and its New Note Action policy.
//!
//! # The shape of an IT tick
//!
//! IT is the first format in scope where a *pattern channel* and a *sounding voice* are
//! different things: a New Note Action detaches the old voice and leaves it running its
//! own envelopes, fadeout and auto-vibrato while the channel goes on to the new note
//! (architecture §5.1). The split here follows OpenMPT's, with the names its `ModChannel`
//! uses in doc comments:
//!
//! * [`ItChannel`] is the pattern channel — the row's columns, every effect memory, the
//!   oscillators, the pattern-loop bookkeeping, and the values that must outlive a voice
//!   (channel volume, pan, filter cutoff, the last note and instrument).
//! * [`ItVoiceState`] is one *voice's* articulation, indexed by
//!   [`VoiceId::index`](starplayer_core::VoiceId::index) and validated by the stored
//!   [`VoiceId`] so a recycled pool slot can never be mistaken for the voice that used to
//!   own it. Foreground and background entries are the same type and are advanced by the
//!   same code, which is what makes an NNA voice keep sounding correctly.
//!
//! Effects run on the channel and are pushed into its foreground voice by
//! [`ItProcessor::sync_foreground`] at the end of the channel's tick; the per-voice pass
//! then advances every owned voice — foreground or background — and writes its volume,
//! step, pan and filter straight to `voice.params`.
//!
//! # Pitch is a frequency, not a period
//!
//! `kPeriodsAreHertz`: IT computes note frequency in hertz and slides multiply it, in both
//! linear-slide and Amiga-slide modes. Nothing here holds an Amiga period; the conformance
//! trace converts back to libxmp's period domain only so the two can be compared.

// Every channel index originates from a loop bounded by `channels.len()`, every voice
// index is bounds-checked through `VoicePool::get_mut` or against the parallel array's
// length before use, and every table index is masked to its declared domain.
#![allow(clippy::indexing_slicing)]

use alloc::boxed::Box;
use alloc::vec;

use starplayer_core::fixed::{bipolar_from_ratio, unit_from_ratio};
use starplayer_core::quirks::{QuirkSelection, QuirkSet};
use starplayer_core::random::Xorshift32;
use starplayer_core::tables::{
    LINEAR_FREQUENCY_TABLE_LEN, fine_linear_slide_down_q16, fine_linear_slide_up_q16, linear_slide_down_q16,
    linear_slide_up_q16, scale_frequency,
};
use starplayer_core::{
    ChannelId, DirtyBits, FilterParams, Frame, I1F15, InstrumentId, Note, SampleId, Step, TempoModel, TempoModelId,
    VoiceId, VoiceParam, VoiceParams,
};
use starplayer_engine::{
    EndOfSongPolicy, MAX_VOICE_CAPACITY, OrderEntry, PatternData, PatternFlowState, PatternSequencer, RowRef,
    SequencerSettings, TickContext, TickOutcome, TrackerProcessor,
};
#[cfg(feature = "trace")]
use starplayer_engine::TraceChannelState;
use starplayer_mixer::{LoopSpan, SampleRegion, VoiceTag};
use starplayer_model::{
    DuplicateAction, DuplicateCheck, EffectNames, Envelope, InstrumentDef, LoopMode, Module, NewNoteAction,
    OrderEntry as ModelOrderEntry, SampleIndex,
};
use starplayer_rt::Arc;

use crate::header::{ItFormatData, ItFormatExtra, PAN_SURROUND};
use crate::pattern::{
    CELL_BYTES, INSTRUMENT_NONE, ItCell, ItVolumeCommand, NOTE_CUT, NOTE_FADE, NOTE_NONE, NOTE_OFF,
};

/// How many voices an IT module may sound at once.
///
/// Impulse Tracker's own limit. The processor's parallel state is wider than this so a
/// MIDI voice may occupy a low global slot without pushing a tracked IT voice past the
/// array, but no more than this many slots may be IT-owned at once.
pub const VIRTUAL_CHANNELS: usize = 256;

/// The note at which a sample plays back at its own `C5Speed`: IT's `C-5`.
const REFERENCE_NOTE: u8 = 60;
/// The highest note an IT pattern can name (`B-9`).
const MAX_NOTE: u8 = 119;
/// 1/768ths of an octave per semitone — the step of [`LINEAR_FREQUENCY_TABLE`].
const UNITS_PER_SEMITONE: i32 = LINEAR_FREQUENCY_TABLE_LEN as i32 / 12;
/// IT's fadeout counter, full scale. ITTECH.TXT's `NFC` is this divided by 64.
const FADEOUT_FULL: i32 = 65536;
/// What one unit of an instrument's `FadeOut` takes off [`FADEOUT_FULL`] per tick.
const FADEOUT_STEP_SCALE: i32 = FADEOUT_FULL / 1024;
/// The widest note volume IT's mixer works in, four times the file's 0..64.
const MAX_NOTE_VOLUME: i32 = 256;
/// The widest pan position IT's mixer works in, four times the file's 0..64.
const MAX_PAN: i32 = 256;
/// IT's `GV` range.
const MAX_GLOBAL_VOLUME: u8 = 128;
/// IT's `CV` and sample/instrument global volume range.
const MAX_CHANNEL_VOLUME: u8 = 64;

/// Command codes, as the loader stores them: 1 for `A` through 26 for `Z`.
const COMMAND_SET_SPEED: u8 = 1;
const COMMAND_POSITION_JUMP: u8 = 2;
const COMMAND_PATTERN_BREAK: u8 = 3;
const COMMAND_VOLUME_SLIDE: u8 = 4;
const COMMAND_PORTAMENTO_DOWN: u8 = 5;
const COMMAND_PORTAMENTO_UP: u8 = 6;
const COMMAND_TONE_PORTAMENTO: u8 = 7;
const COMMAND_VIBRATO: u8 = 8;
const COMMAND_TREMOR: u8 = 9;
const COMMAND_ARPEGGIO: u8 = 10;
const COMMAND_VIBRATO_VOLUME_SLIDE: u8 = 11;
const COMMAND_PORTAMENTO_VOLUME_SLIDE: u8 = 12;
const COMMAND_CHANNEL_VOLUME: u8 = 13;
const COMMAND_CHANNEL_VOLUME_SLIDE: u8 = 14;
const COMMAND_OFFSET: u8 = 15;
const COMMAND_PAN_SLIDE: u8 = 16;
const COMMAND_RETRIGGER: u8 = 17;
const COMMAND_TREMOLO: u8 = 18;
const COMMAND_SPECIAL: u8 = 19;
const COMMAND_TEMPO: u8 = 20;
const COMMAND_FINE_VIBRATO: u8 = 21;
const COMMAND_GLOBAL_VOLUME: u8 = 22;
const COMMAND_GLOBAL_VOLUME_SLIDE: u8 = 23;
const COMMAND_PAN: u8 = 24;
const COMMAND_PANBRELLO: u8 = 25;
const COMMAND_MIDI_MACRO: u8 = 26;
const COMMAND_SMOOTH_MIDI_MACRO: u8 = 27;

/// ITTECH.TXT's `FineSineData`, 256 entries peaking at ±64 — the table `Hxy`, `Rxy`,
/// `Yxy` and per-sample auto-vibrato all read.
///
/// Transcribed from OpenMPT's `ITSinusTable` (`soundlib/Tables.cpp`), which is
/// ITTECH.TXT's table verbatim. Design goal 5 bans `sin` from the render path, so this is
/// a table and its shape is pinned by a test.
const IT_SINE_TABLE: [i8; 256] = [
      0,   2,   3,   5,   6,   8,   9,  11,  12,  14,  16,  17,  19,  20,  22,  23,
     24,  26,  27,  29,  30,  32,  33,  34,  36,  37,  38,  39,  41,  42,  43,  44,
     45,  46,  47,  48,  49,  50,  51,  52,  53,  54,  55,  56,  56,  57,  58,  59,
     59,  60,  60,  61,  61,  62,  62,  62,  63,  63,  63,  64,  64,  64,  64,  64,
     64,  64,  64,  64,  64,  64,  63,  63,  63,  62,  62,  62,  61,  61,  60,  60,
     59,  59,  58,  57,  56,  56,  55,  54,  53,  52,  51,  50,  49,  48,  47,  46,
     45,  44,  43,  42,  41,  39,  38,  37,  36,  34,  33,  32,  30,  29,  27,  26,
     24,  23,  22,  20,  19,  17,  16,  14,  12,  11,   9,   8,   6,   5,   3,   2,
      0,  -2,  -3,  -5,  -6,  -8,  -9, -11, -12, -14, -16, -17, -19, -20, -22, -23,
    -24, -26, -27, -29, -30, -32, -33, -34, -36, -37, -38, -39, -41, -42, -43, -44,
    -45, -46, -47, -48, -49, -50, -51, -52, -53, -54, -55, -56, -56, -57, -58, -59,
    -59, -60, -60, -61, -61, -62, -62, -62, -63, -63, -63, -64, -64, -64, -64, -64,
    -64, -64, -64, -64, -64, -64, -63, -63, -63, -62, -62, -62, -61, -61, -60, -60,
    -59, -59, -58, -57, -56, -56, -55, -54, -53, -52, -51, -50, -49, -48, -47, -46,
    -45, -44, -43, -42, -41, -39, -38, -37, -36, -34, -33, -32, -30, -29, -27, -26,
    -24, -23, -22, -20, -19, -17, -16, -14, -12, -11,  -9,  -8,  -6,  -5,  -3,  -2,
];

/// libxmp's IT period numerator: `C4_PERIOD · c4rate`, which is `428 · 8363`. Only the
/// conformance trace uses it — the engine itself never holds a period.
#[cfg(feature = "trace")]
const LIBXMP_PERIOD_NUMERATOR: u64 = 428 * 8363;

/// ITTECH.TXT's `SlideTable`, the volume column's tone-portamento parameters for `g01`
/// through `g09`.
const VOLUME_COLUMN_PORTAMENTO: [u8; 9] = [1, 4, 8, 16, 32, 64, 96, 128, 255];

/// One waveform sample for `Hxy`, `Rxy` or `Yxy`.
///
/// `waveform` is 0 sine, 1 ramp down, 2 square, 3 random — and IT's square is
/// **unipolar**, 0 to +64 rather than ±64 (`FineSquareWave DB 128 Dup (64), 128 Dup (0)`).
fn oscillator_sample(waveform: u8, position: u8, random: &mut Xorshift32) -> i32 {
    match waveform & 3 {
        0 => IT_SINE_TABLE[position as usize] as i32,
        1 => 64 - (position as i32 + 1) / 2,
        2 => if position < 128 { 64 } else { 0 },
        _ => (random.next_u32() & 127) as i32 - 64,
    }
}

/// The frequency at which `note` sounds for a sample whose reference rate is `c5speed`.
fn frequency_from_note(c5speed: u32, note: u8) -> u32 {
    scale_frequency(c5speed, (note as i32 - REFERENCE_NOTE as i32) * UNITS_PER_SEMITONE)
}

/// `value · numerator / denominator`, truncated — OpenMPT's `Util::muldiv`.
fn muldiv(value: i64, numerator: i64, denominator: i64) -> i64 {
    if denominator == 0 { 0 } else { value.saturating_mul(numerator) / denominator }
}

/// `value · numerator / 65536`, rounded — the shape every IT linear-slide table is used in.
fn apply_slide_ratio(value: u32, ratio_q16: u32) -> u32 {
    (((value as u64 * ratio_q16 as u64) + 32_768) >> 16).min(u32::MAX as u64) as u32
}

/// One envelope's playback state, which belongs to a **voice** rather than to a channel.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ItEnvelopeState {
    /// IT's own **one-based** tick position: zero means "not started yet", and a tick
    /// increments before it evaluates, reading the envelope at `position - 1`.
    /// `kITEnvelopePositionHandling`, test case `s77.it`.
    pub position: u16,
    /// The channel's copy of the instrument's enable flag. `S77`/`S79`/`S7B` clear it,
    /// which **pauses the counter** without stopping the envelope being applied.
    pub enabled: bool,
    /// The value the last processed tick produced, in the envelope's own output range.
    pub value: i32,
}

/// One voice's articulation — everything that keeps running after a New Note Action has
/// detached it from its channel.
///
/// OpenMPT keeps all of this in `ModChannel` and copies the whole struct into a background
/// channel; here the channel and the voice are separate objects and
/// [`ItProcessor::sync_foreground`] pushes the channel's live values into its foreground
/// voice every tick, so foreground and background entries can be advanced by one code
/// path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItVoiceState {
    /// The voice this entry describes. The generation is what makes a recycled pool slot
    /// safe: an entry whose id does not match the pool's is not ours.
    pub voice: Option<VoiceId>,
    /// The pattern channel the note started on — `VoiceTag::channel`, and what `S7x` and
    /// the duplicate check match against.
    pub root_channel: u8,
    /// Whether this voice is still the foreground of its root channel.
    pub foreground: bool,
    /// One-based IT instrument number, or zero in sample mode.
    pub instrument: u16,
    /// One-based sample number.
    pub sample: u16,
    /// The note the mixer plays — the pattern note through the instrument's note map.
    pub note: u8,
    /// The raw pattern note, which is what `DCT = Note` and pitch/pan separation compare
    /// (`kITRealNoteMapping`).
    pub trigger_note: u8,
    /// Playback frequency in hertz before auto-vibrato and the pitch envelope.
    pub frequency_hz: u32,
    /// The sample's reference rate, kept so the conformance trace can name a period.
    pub reference_rate_hz: u32,
    /// `Vol`, in IT's mixer scale of 0..=256.
    pub note_volume: i32,
    /// `SV · IV / 64` — the sample's global volume times the instrument's.
    pub instrument_volume: u8,
    /// `CV`, the channel volume the note started with.
    pub channel_volume: u8,
    /// Pan position, 0..=256 with 128 centre.
    pub pan: i16,
    /// Whether the voice is in surround. Rendered as centre — accuracy policy D66.
    pub surround: bool,
    pub volume_envelope: ItEnvelopeState,
    pub panning_envelope: ItEnvelopeState,
    pub pitch_envelope: ItEnvelopeState,
    /// The instrument's pitch envelope drives the filter cutoff rather than the pitch.
    pub pitch_envelope_is_filter: bool,
    /// `NFC · 64`: 65536 at note-on, decremented by `64 · fadeout` once note-fade is set —
    /// IT counts `NFC` down from 1024 by the instrument's raw `FadeOut` every tick.
    pub fadeout: i32,
    /// The key has been released (`===`, `S71`, NNA note-off, DCA note-off).
    pub key_off: bool,
    /// The previous tick's `key_off`. IT reads the envelope sustain flag one tick late —
    /// `EnvOffLength.it`.
    pub key_off_previous: bool,
    /// The fadeout is running.
    pub note_fade: bool,
    /// The voice still has a sample to play. OpenMPT's `nLength != 0`.
    pub playing: bool,
    /// `SCx` silenced this voice. Impulse Tracker zeroes the increment and the fadeout but
    /// leaves the note on the channel, where a `^^` note cut takes the channel away
    /// altogether (OpenMPT `Snd_fx.cpp` `NoteCut` under `kITSCxStopsSample`, against
    /// libxmp's `libxmp_virt_resetchannel` for `XMP_KEY_CUT`).
    pub cut_by_scx: bool,
    pub auto_vibrato_position: u16,
    pub auto_vibrato_depth: u32,
    /// The per-note random volume offset, applied to the instrument volume.
    pub volume_swing: i16,
    /// The per-note random pan offset, applied to the pan every tick.
    pub pan_swing: i16,
    /// IT's 0..=127 filter cutoff and resonance for this voice.
    pub cutoff: u8,
    pub resonance: u8,
    /// The filter is engaged. IT only *disengages* it on a note trigger, so a `Z7F`
    /// mid-note leaves the coefficients where they were.
    pub filter_active: bool,
    /// The New Note Action this voice will be detached with — the instrument's, or an
    /// `S73`..`S76` override.
    pub new_note_action: NewNoteAction,
    /// The last parameters written, so a tick only writes what changed.
    written: VoiceParams,
    has_written: bool,
    /// The volume the last tick computed, in OpenMPT's 14-bit mixing scale.
    real_volume: i32,
    /// The pan the last tick computed, 0..=256.
    real_pan: i16,
}

impl Default for ItVoiceState {
    fn default() -> ItVoiceState {
        ItVoiceState {
            voice: None,
            root_channel: 0,
            foreground: false,
            instrument: 0,
            sample: 0,
            note: REFERENCE_NOTE,
            trigger_note: REFERENCE_NOTE,
            frequency_hz: 0,
            reference_rate_hz: 8363,
            note_volume: 0,
            instrument_volume: MAX_CHANNEL_VOLUME,
            channel_volume: MAX_CHANNEL_VOLUME,
            pan: 128,
            surround: false,
            volume_envelope: ItEnvelopeState::default(),
            panning_envelope: ItEnvelopeState::default(),
            pitch_envelope: ItEnvelopeState::default(),
            pitch_envelope_is_filter: false,
            fadeout: FADEOUT_FULL,
            key_off: false,
            key_off_previous: false,
            note_fade: false,
            playing: false,
            cut_by_scx: false,
            auto_vibrato_position: 0,
            auto_vibrato_depth: 0,
            volume_swing: 0,
            pan_swing: 0,
            cutoff: 0x7F,
            resonance: 0,
            filter_active: false,
            new_note_action: NewNoteAction::Cut,
            written: VoiceParams::SILENT,
            has_written: false,
            real_volume: 0,
            real_pan: 128,
        }
    }
}

impl ItVoiceState {
    /// Whether this entry describes `voice` — the generation check that makes a recycled
    /// pool slot safe.
    fn owns(&self, voice: VoiceId) -> bool { self.voice == Some(voice) }

    fn release(&mut self) { *self = ItVoiceState::default(); }
}

/// One pattern channel: the row's columns, every effect memory, and the state that
/// outlives the voices the channel triggers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItChannel {
    pub channel_number: u8,
    /// The row's cell, kept for the whole row so a pattern-delay repeat and the per-tick
    /// handlers see the same columns.
    pub row: ItCell,
    /// The tick this channel's note lands on — `SDx`, and the tick every "first tick"
    /// effect is measured against (`kITFirstTickHandling`).
    pub start_tick: u16,
    /// The channel's foreground voice, mirroring [`ChannelTable::foreground`].
    pub voice: Option<VoiceId>,

    // ── effect memories, one per family (IT is not ST3: they are not shared) ──
    /// `Dxy`, `Kxy` and `Lxy` — one memory (`EfxMem_DKL`).
    pub volume_slide_memory: u8,
    /// `Exx`/`Fxx`, and `Gxx` too unless Compatible Gxx unlinks it (`EfxMem_EFG`).
    pub pitch_slide_memory: u8,
    /// `Gxx` under Compatible Gxx (`EfxMem_G_Compat`).
    pub tone_portamento_memory: u8,
    pub tremor_memory: u8,
    pub arpeggio_memory: u8,
    pub channel_volume_slide_memory: u8,
    pub offset_memory: u8,
    /// `SAy`, which has its own persistent memory and never repositions by itself.
    pub high_offset: u8,
    pub pan_slide_memory: u8,
    pub retrigger_memory: u8,
    /// The whole `Sxy` byte (`EfxMem_S`), so `S00` recalls the last sub-command too.
    pub special_memory: u8,
    pub tempo_memory: u8,
    /// `Wxy`. Per channel even though the effect is global (`kPerChannelGlobalVolSlide`).
    pub global_volume_slide_memory: u8,
    /// The volume column's `a`/`b`/`c`/`d` share one memory, separate from `Dxy`
    /// (`kITVolColNoSlidePropagation`).
    pub volume_column_memory: u8,

    // ── oscillators ──
    pub vibrato_speed: u8,
    pub vibrato_depth: u8,
    pub vibrato_position: u8,
    pub vibrato_waveform: u8,
    pub last_vibrato: i32,
    pub tremolo_speed: u8,
    pub tremolo_depth: u8,
    pub tremolo_position: u8,
    pub tremolo_waveform: u8,
    pub last_tremolo: i32,
    pub panbrello_speed: u8,
    pub panbrello_depth: u8,
    pub panbrello_position: u8,
    pub panbrello_waveform: u8,
    pub panbrello_random: i32,
    /// `kITPanbrelloHold`: the offset survives until the next note or panning command.
    pub panbrello_offset: i32,

    // ── per-row counters ──
    pub note_delay: u8,
    pub note_cut: u8,
    pub tremor_count: u8,
    pub tremor_on: bool,
    pub retrigger_count: u8,
    pub arpeggio_tick: u8,

    // ── live channel state ──
    /// The foreground note's frequency in hertz.
    pub frequency_hz: u32,
    /// `Gxx`'s destination frequency. Zero once reached (`kITPortaTargetReached`).
    pub portamento_target_hz: u32,
    pub glissando: bool,
    /// `Vol`, 0..=256.
    pub note_volume: i32,
    /// `Mxx`, 0..=64.
    pub channel_volume: u8,
    /// 0..=256, 128 centre.
    pub pan: i16,
    pub surround: bool,
    /// The channel pan an instrument's or sample's own panning displaced
    /// (`kITDoNotOverrideChannelPan`), restored by the next note.
    pub restore_pan: Option<i16>,
    /// IT's 0..=127 filter settings, which persist across notes unless the instrument
    /// enables its own.
    pub cutoff: u8,
    pub resonance: u8,
    pub filter_active: bool,
    /// Which of the sixteen parametered macros `Zxx` below `0x80` runs (`SFx`).
    pub active_macro: u8,
    /// The last MIDI macro parameter and a smooth macro's fixed-point interpolation.
    pub macro_value: i32,
    pub macro_target: i32,
    pub macro_slide: i32,
    /// The **previous** tick's computed volume and pan, which the `u` and `y` macro
    /// letters read. A macro runs at the top of the tick, before the tick's own volume
    /// and pan are computed, and the value it sees belongs to the channel rather than to
    /// the voice — it survives a note change and an idle row (libxmp
    /// `xc->macro.finalvol` / `xc->macro.notepan`, `src/player.c:1113,1374`; OpenMPT
    /// `chn.nCalcVolume` / `chn.nRealPan`, `MIDIMacroParser.cpp` letters `u` and `y`).
    pub macro_real_volume: i32,
    pub macro_real_pan: i16,
    /// `nNewNote` / `nLastNote`: the note a lone instrument number would play.
    /// `kITInitialNoteMemory` starts it at `C-0`, not "no note".
    pub last_note: u8,
    /// `nOldIns`: the last instrument number seen, valid or not.
    pub last_instrument: u8,
    /// `nNewIns`: an instrument number latched without a note.
    pub pending_instrument: u8,
    /// The channel's New Note Action, which `S73`..`S76` override until the next note.
    pub new_note_action: NewNoteAction,
    /// Set while the row's note is a tone portamento, so the per-tick handlers agree with
    /// tick zero about what the cell meant.
    pub tone_portamento: bool,
    /// The command the per-tick pass runs, and its parameter.
    pub command: u8,
    pub command_parameter: u8,
    /// Whether a note was triggered this tick, which is the only moment IT lets the filter
    /// be switched off.
    pub triggered: bool,
    /// `Oxx`'s resolved sample offset in frames.
    pub sample_offset: u32,
    /// The frequency this tick's oscillators produced, which is what the voice sounds at.
    /// Vibrato and arpeggio are output offsets and must not change `frequency_hz` itself,
    /// so they write here and set [`ItChannel::output_frequency_set`]; every other tick
    /// sounds `frequency_hz` as it stands once the effects have run.
    pub output_frequency_hz: u32,
    /// Whether an oscillator wrote `output_frequency_hz` this tick.
    pub output_frequency_set: bool,
    /// The note volume this tick's tremolo and tremor produced, 0..=256.
    pub output_volume: i32,
}

impl ItChannel {
    fn new(channel_number: u8, pan: i16, surround: bool, channel_volume: u8) -> ItChannel {
        ItChannel {
            channel_number,
            row: ItCell::EMPTY,
            start_tick: 0,
            voice: None,
            volume_slide_memory: 0,
            pitch_slide_memory: 0,
            tone_portamento_memory: 0,
            tremor_memory: 0,
            arpeggio_memory: 0,
            channel_volume_slide_memory: 0,
            offset_memory: 0,
            high_offset: 0,
            pan_slide_memory: 0,
            retrigger_memory: 0,
            special_memory: 0,
            tempo_memory: 0,
            global_volume_slide_memory: 0,
            volume_column_memory: 0,
            vibrato_speed: 0,
            vibrato_depth: 0,
            vibrato_position: 0,
            vibrato_waveform: 0,
            last_vibrato: 0,
            tremolo_speed: 0,
            tremolo_depth: 0,
            tremolo_position: 0,
            tremolo_waveform: 0,
            last_tremolo: 0,
            panbrello_speed: 0,
            panbrello_depth: 0,
            panbrello_position: 0,
            panbrello_waveform: 0,
            panbrello_random: 0,
            panbrello_offset: 0,
            note_delay: 0,
            note_cut: 0,
            tremor_count: 0,
            tremor_on: false,
            retrigger_count: 0,
            arpeggio_tick: 0,
            frequency_hz: 0,
            portamento_target_hz: 0,
            glissando: false,
            note_volume: 0,
            channel_volume,
            pan,
            surround,
            restore_pan: None,
            cutoff: 0x7F,
            resonance: 0,
            filter_active: false,
            active_macro: 0,
            macro_value: 0,
            macro_target: 0,
            macro_slide: 0,
            macro_real_volume: 0,
            macro_real_pan: 128,
            last_note: 0,
            last_instrument: 0,
            pending_instrument: 0,
            new_note_action: NewNoteAction::Cut,
            tone_portamento: false,
            command: 0,
            command_parameter: 0,
            triggered: false,
            sample_offset: 0,
            output_frequency_hz: 0,
            output_frequency_set: false,
            output_volume: 0,
        }
    }
}

/// Impulse Tracker's stateful effect processor.
pub struct ItProcessor {
    module: Arc<Module>,
    channels: Box<[ItChannel]>,
    /// One entry per persistent-host pool slot, allocated once. IT owns at most
    /// [`VIRTUAL_CHANNELS`] of them; the extra entries let live MIDI occupy arbitrary
    /// global voice IDs without making an IT voice untracked.
    voices: Box<[ItVoiceState]>,
    sample_rate_hz: u32,
    /// `GV`, 0..=128.
    global_volume: u8,
    /// The song speed, without any `S6x` tick delay this row added.
    song_speed: u8,
    /// `S6x`, summed across the row's channels and cleared when the row changes.
    frame_delay: u8,
    instrument_mode: bool,
    old_effects: bool,
    compatible_gxx: bool,
    linear_slides: bool,
    extended_filter_range: bool,
    quirks: QuirkSet,
    flow: PatternFlowState,
    last_order: Option<(u16, u16)>,
    /// The random stream every per-note swing and every random oscillator waveform draws
    /// from. Seeded from a constant so a render is reproducible.
    random: Xorshift32,
    /// `m_lastMovedChannel`: the voice the last New Note Action moved, which IT's
    /// envelope-carry quirk reads back from.
    last_moved_voice: Option<VoiceId>,
}

impl ItProcessor {
    /// An IT processor whose quirks come from the dialect the loader detected.
    pub fn new(module: Arc<Module>, sample_rate_hz: u32) -> ItProcessor {
        ItProcessor::with_quirks(module, sample_rate_hz, QuirkSelection::FromDialect)
    }

    /// An IT processor with an explicit [`QuirkSelection`].
    pub fn with_quirks(module: Arc<Module>, sample_rate_hz: u32, quirks: QuirkSelection) -> ItProcessor {
        let header = module.header();
        let quirks = quirks.resolve(header.dialect);
        let extra = ItFormatExtra::from_header(header);
        let global_volume = ((header.global_volume.to_bits() as u32 * MAX_GLOBAL_VOLUME as u32 + 32_767) / 65_535) as u8;
        let channel_count = header.channel_count as usize;
        let mut channels = alloc::vec::Vec::with_capacity(channel_count);
        for index in 0..channel_count {
            let (pan, surround) = default_pan(&module, index as u8);
            channels.push(ItChannel::new(index as u8, pan, surround, default_channel_volume(&module, index as u8)));
        }
        ItProcessor {
            channels: channels.into_boxed_slice(),
            voices: vec![ItVoiceState::default(); MAX_VOICE_CAPACITY].into_boxed_slice(),
            sample_rate_hz,
            global_volume,
            song_speed: header.initial_speed,
            frame_delay: 0,
            instrument_mode: extra.is_instrument_mode(),
            old_effects: extra.is_old_effects(),
            compatible_gxx: extra.is_compatible_gxx(),
            linear_slides: header.flags.linear_slides,
            extended_filter_range: extra.has_extended_filter_range(),
            quirks,
            flow: PatternFlowState::new(quirks.it_pattern_loop.flow(), channel_count),
            last_order: None,
            random: Xorshift32::new(0x1D_F0_5A_C7),
            module,
            last_moved_voice: None,
        }
    }

    /// The replay behaviour in force. Fixed for the lifetime of the loaded module.
    pub const fn quirks(&self) -> QuirkSet { self.quirks }

    /// Whether the module asked for OpenMPT's extended filter range, which widens the
    /// cutoff law from one octave per 24 units to one per 20. The coefficients themselves
    /// are the mixer's (task G2); this is where the flag is read from.
    pub const fn has_extended_filter_range(&self) -> bool { self.extended_filter_range }

    /// The pattern-loop and break/jump bookkeeping, for inspection by a host or a test.
    pub const fn pattern_flow(&self) -> &PatternFlowState { &self.flow }

    pub fn channels(&self) -> &[ItChannel] { &self.channels }
    pub fn channel(&self, channel: u8) -> Option<&ItChannel> { self.channels.get(channel as usize) }

    /// The articulation of one voice, if this processor owns it.
    pub fn voice(&self, voice: VoiceId) -> Option<&ItVoiceState> {
        self.voices.get(voice.index() as usize).filter(|state| state.owns(voice))
    }

    /// How many voices are sounding, foreground and background together — what the peak
    /// voice count on a dense module is measured from.
    pub fn active_voices(&self) -> usize { self.voices.iter().filter(|state| state.voice.is_some()).count() }

    // ── module lookups ───────────────────────────────────────────────────────────────

    fn instrument_def(&self, instrument: u16) -> Option<&InstrumentDef> {
        self.module.instrument(InstrumentId(instrument.checked_sub(1)?))
    }

    fn sample_index(&self, sample: u16) -> Option<&SampleIndex> {
        self.module.sample(SampleId(sample.checked_sub(1)?))
    }

    /// The sample number and mixer note an instrument's note map gives `note`.
    ///
    /// In sample mode the instrument column names a sample directly and there is no map.
    fn map_note(&self, instrument: u16, note: u8) -> (u16, u8) {
        if !self.instrument_mode {
            return (instrument, note);
        }
        let Some(definition) = self.instrument_def(instrument) else { return (0, note) };
        let Some(&sample) = definition.note_sample_map.get(note as usize) else { return (0, note) };
        let mapped = definition.note_transpose_map.get(note as usize).copied().unwrap_or(note);
        (sample, mapped.min(MAX_NOTE))
    }

    /// `SV · IV / 64`, the instrument volume the mixing chain multiplies by.
    fn instrument_volume(&self, instrument: u16, sample: u16) -> u8 {
        let sample_global = ItFormatData::from_header(self.module.header())
            .and_then(|data| data.sample_global_volume(sample.saturating_sub(1)))
            .unwrap_or(MAX_CHANNEL_VOLUME)
            .min(MAX_CHANNEL_VOLUME);
        let instrument_global = match self.instrument_mode {
            true => self
                .instrument_def(instrument)
                .map(|definition| ((definition.global_volume.to_bits() as u32 * MAX_CHANNEL_VOLUME as u32 + 32_767) / 65_535) as u8)
                .unwrap_or(MAX_CHANNEL_VOLUME),
            false => MAX_CHANNEL_VOLUME,
        };
        ((sample_global as u32 * instrument_global as u32) / MAX_CHANNEL_VOLUME as u32).min(MAX_CHANNEL_VOLUME as u32) as u8
    }

    /// The sample's own default volume, in IT's 0..=256 mixer scale.
    fn sample_volume(&self, sample: u16) -> i32 {
        self.sample_index(sample)
            .map(|index| ((index.default_volume().to_bits() as u32 * MAX_NOTE_VOLUME as u32 + 32_767) / 65_535) as i32)
            .unwrap_or(0)
            .min(MAX_NOTE_VOLUME)
    }
}

/// One channel's default pan, and whether the file put it in surround.
fn default_pan(module: &Module, channel: u8) -> (i16, bool) {
    let raw = ItFormatData::from_header(module.header()).and_then(|data| data.channel_pan_raw(channel));
    match raw {
        Some(raw) => {
            let position = raw & 0x7F;
            if position == PAN_SURROUND { (128, true) } else { ((position.min(64) as i16) * 4, false) }
        }
        None => (128, false),
    }
}

fn default_channel_volume(module: &Module, channel: u8) -> u8 {
    module
        .header()
        .default_channel_volume
        .get(channel as usize)
        .map(|volume| ((volume.to_bits() as u32 * MAX_CHANNEL_VOLUME as u32 + 32_767) / 65_535) as u8)
        .unwrap_or(MAX_CHANNEL_VOLUME)
        .min(MAX_CHANNEL_VOLUME)
}

/// The mixer region for one sample, choosing the sustain loop while the key is down.
fn sample_region(sample: &SampleIndex, sustain: bool) -> SampleRegion {
    if sustain && let Some(loop_span) = sample.sustain_loop() {
        let span = match loop_span.mode {
            LoopMode::PingPong => LoopSpan::ping_pong(loop_span.start, loop_span.end),
            _ => LoopSpan::new(loop_span.start, loop_span.end),
        };
        if let Some(span) = span {
            return SampleRegion::looping(sample.pcm_offset(), span);
        }
    }
    match sample.loop_mode() {
        LoopMode::Forward => LoopSpan::new(sample.loop_start(), sample.loop_end())
            .map(|span| SampleRegion::looping(sample.pcm_offset(), span))
            .unwrap_or_else(|| SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames())),
        LoopMode::PingPong => LoopSpan::ping_pong(sample.loop_start(), sample.loop_end())
            .map(|span| SampleRegion::looping(sample.pcm_offset(), span))
            .unwrap_or_else(|| SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames())),
        LoopMode::None => SampleRegion::one_shot(sample.pcm_offset(), sample.length_frames()),
    }
}

/// One envelope's interpolated value at `position`, already multiplied by `scale`.
///
/// OpenMPT interpolates in Q16.16 over the file's own `0..=64` node domain and rounds once
/// after scaling the output — `InstrumentEnvelope::GetValueFromPosition(position, rangeOut,
/// rangeIn = 64)` in `ModInstrument.cpp`, which its three callers ask for a `0..=256`
/// volume, a `0..=64` pan and a `0..=512` pitch/filter value from.
///
/// IT's volume envelope is stored in the model as the file's own `0..=64`, so its
/// `centre_offset` is zero. The pan and pitch/filter envelopes are stored as `-32..=32`,
/// so theirs is 32: the interpolation runs in the file's unsigned domain and the offset
/// comes back off the scaled result. That is not only bookkeeping — it keeps every
/// intermediate division non-negative, so a falling segment truncates the way OpenMPT's
/// does rather than toward zero.
fn envelope_value(envelope: &Envelope, position: u16, scale: i32, centre_offset: i32) -> i32 {
    let points = &envelope.points;
    let Some(&last) = points.last() else { return 0 };
    let mut index = points.len() - 1;
    for (candidate, point) in points.iter().enumerate().take(points.len() - 1) {
        if position <= point.tick {
            index = candidate;
            break;
        }
    }
    let point = points[index];
    if position >= point.tick || index == 0 {
        let _ = last;
        return point.value as i32 * scale;
    }
    let previous = points[index - 1];
    let span = point.tick as i32 - previous.tick as i32;
    if span <= 0 {
        return point.value as i32 * scale;
    }
    let output_range = 64 * scale;
    let mut value_q16 = (previous.value as i32 + centre_offset) * 65_536 / 64;
    let destination_q16 = (point.value as i32 + centre_offset) * 65_536 / 64;
    value_q16 += (position as i32 - previous.tick as i32) * (destination_q16 - value_q16) / span;
    (value_q16 * output_range + 32_768) / 65_536 - centre_offset * scale
}

/// The envelope tick a loop or sustain span sends the position back to, and the tick it
/// wraps at.
///
/// `end` is one past the loop-end node, because IT plays the loop-end node's tick and
/// wraps on the tick after it.
fn envelope_span(envelope: &Envelope, key_off_previous: bool) -> (u16, u16, bool) {
    let node_tick = |index: u8| envelope.points.get(index as usize).map(|point| point.tick).unwrap_or(0);
    if let Some(sustain) = envelope.sustain
        && !key_off_previous
    {
        return (node_tick(sustain.start), node_tick(sustain.end).saturating_add(1), false);
    }
    if let Some(span) = envelope.loop_span {
        return (node_tick(span.start), node_tick(span.end).saturating_add(1), false);
    }
    let end = envelope.points.last().map(|point| point.tick).unwrap_or(0);
    (end, end, true)
}

// ── the row, the note and the New Note Action ────────────────────────────────────────

impl ItProcessor {
    /// Parse the row's cells into the channels and settle everything that has to be known
    /// before the first tick runs: the note-delay start tick, IT's empty-note-map-slot
    /// veto, and its invalid-instrument rule.
    fn latch_row(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) {
        let channel_count = core::cmp::min(self.channels.len(), row.bytes.len() / CELL_BYTES);
        let row_length = (self.song_speed as u16).max(1);
        for channel_index in 0..self.channels.len() {
            let channel = &mut self.channels[channel_index];
            channel.row = ItCell::EMPTY;
            channel.start_tick = 0;
            channel.command = 0;
            channel.command_parameter = 0;
            channel.tone_portamento = false;
            channel.triggered = false;
            channel.arpeggio_tick = 0;
            if channel_index >= channel_count {
                continue;
            }
            let start = channel_index * CELL_BYTES;
            let Some(mut cell) = ItCell::from_bytes(row.bytes.get(start..start + CELL_BYTES).unwrap_or(&[])) else { continue };
            let channel_id = ChannelId(channel_index as u16);
            context.report_effect(channel_id, cell.command, cell.info, EffectNames::IT.name(cell.command, cell.info).unwrap_or(""));
            if cell.note != NOTE_NONE || cell.instrument != INSTRUMENT_NONE {
                context.report_note(channel_id, (cell.note <= MAX_NOTE).then(|| Note::new(cell.note)), (cell.instrument != INSTRUMENT_NONE).then_some(cell.instrument));
            }

            // IT compatibility (`kITEmptyNoteMapSlotIgnoreCell`, `NoMap.it`): in instrument
            // mode a note that maps to no sample makes IT discard the **whole cell**,
            // effect column included, remembering only the note and the instrument number.
            if self.instrument_mode && cell.instrument != INSTRUMENT_NONE {
                let note = if cell.note <= MAX_NOTE { cell.note } else { self.channels[channel_index].last_note };
                if cell.note <= MAX_NOTE || cell.note == NOTE_NONE {
                    let (sample, _) = self.map_note(cell.instrument as u16, note);
                    if self.instrument_def(cell.instrument as u16).is_some() && sample == 0 {
                        let channel = &mut self.channels[channel_index];
                        channel.last_note = note;
                        channel.pending_instrument = cell.instrument;
                        channel.row = ItCell::EMPTY;
                        continue;
                    }
                }
            }

            // IT compatibility (`InstrumentNumberChange.it`): an invalid instrument number
            // suppresses the note entirely and is remembered, so later notes on the
            // channel stay suppressed until a valid number arrives.
            if self.instrument_mode && (cell.note <= MAX_NOTE || cell.note == NOTE_NONE) {
                let to_check = if cell.instrument != INSTRUMENT_NONE { cell.instrument } else { self.channels[channel_index].last_instrument };
                if to_check != 0 && self.instrument_def(to_check as u16).is_none() {
                    cell.note = NOTE_NONE;
                    cell.instrument = INSTRUMENT_NONE;
                }
            }
            if cell.instrument != INSTRUMENT_NONE {
                self.channels[channel_index].last_instrument = cell.instrument;
            }

            // `SDx` note delay. `SD0` is `SD1` in IT, and a delay at or past the row's
            // length drops the cell while still latching its instrument number
            // (`kITOutOfRangeDelay`, `tickdelay.it`).
            if cell.command == COMMAND_SPECIAL {
                let parameter = if cell.info == 0 { self.channels[channel_index].special_memory } else { cell.info };
                if parameter >> 4 == 0xD {
                    let delay = (parameter & 0x0F).max(1) as u16;
                    if delay >= row_length {
                        if cell.instrument != INSTRUMENT_NONE {
                            self.channels[channel_index].pending_instrument = cell.instrument;
                        }
                        self.channels[channel_index].row = ItCell::EMPTY;
                        continue;
                    }
                    self.channels[channel_index].start_tick = delay;
                }
            }
            self.channels[channel_index].tone_portamento = matches!(cell.command, COMMAND_TONE_PORTAMENTO | COMMAND_PORTAMENTO_VOLUME_SLIDE)
                || matches!(cell.volume_command(), ItVolumeCommand::TonePortamento(_));
            self.channels[channel_index].row = cell;
        }
        self.init_portamento_memories();
    }

    /// IT reads every portamento parameter **once per row**, in a fixed order, before any
    /// of them slide (`kITDoublePortamentoSlides`, `DoubleSlide.it`): the effect column's
    /// `Gxx`/`Lxy` first, then the volume column's `g0x`, then the volume column's
    /// `e0x`/`f0x`, then the effect column's `Exx`/`Fxx`.
    fn init_portamento_memories(&mut self) {
        for channel_index in 0..self.channels.len() {
            let cell = self.channels[channel_index].row;
            let effect_column_porta = matches!(cell.command, COMMAND_TONE_PORTAMENTO | COMMAND_PORTAMENTO_VOLUME_SLIDE);
            if effect_column_porta {
                let parameter = if cell.command == COMMAND_PORTAMENTO_VOLUME_SLIDE { 0 } else { cell.info };
                self.init_tone_portamento(channel_index, parameter);
            }
            if let ItVolumeCommand::TonePortamento(value) = cell.volume_command() {
                let parameter = VOLUME_COLUMN_PORTAMENTO.get(value.wrapping_sub(1) as usize).copied().unwrap_or(0);
                self.init_tone_portamento(channel_index, parameter);
            }
            if let ItVolumeCommand::PitchSlideDown(value) | ItVolumeCommand::PitchSlideUp(value) = cell.volume_command()
                && value != 0
            {
                self.channels[channel_index].pitch_slide_memory = value << 2;
                if !effect_column_porta && self.shares_portamento_memory() {
                    self.channels[channel_index].tone_portamento_memory = value << 2;
                }
            }
            if matches!(cell.command, COMMAND_PORTAMENTO_UP | COMMAND_PORTAMENTO_DOWN) && cell.info != 0 {
                self.channels[channel_index].pitch_slide_memory = cell.info;
                if self.shares_portamento_memory() {
                    self.channels[channel_index].tone_portamento_memory = cell.info;
                }
            }
        }
    }

    /// Whether `Gxx` reads and writes the same memory as `Exx`/`Fxx` — true unless the
    /// module asks for Compatible Gxx.
    const fn shares_portamento_memory(&self) -> bool { !self.compatible_gxx }

    fn init_tone_portamento(&mut self, channel_index: usize, parameter: u8) {
        let mut parameter = parameter;
        if self.shares_portamento_memory() {
            if parameter == 0 {
                parameter = self.channels[channel_index].pitch_slide_memory;
            }
            self.channels[channel_index].pitch_slide_memory = parameter;
        }
        if parameter != 0 {
            self.channels[channel_index].tone_portamento_memory = parameter;
        }
    }

    /// OpenMPT's `InstrumentChange` followed by `NoteChange`, plus the New Note Action
    /// that has to run between the two.
    fn trigger_channel(&mut self, context: &mut TickContext<'_>, channel_index: usize) {
        let cell = self.channels[channel_index].row;
        let mut note = cell.note;
        let mut instrument = cell.instrument;
        let tone_portamento = self.channels[channel_index].tone_portamento;

        // IT compatibility (`NoteOffInstr.it`, `noteoff2.it`): a note cut, note off or note
        // fade ignores the instrument number, except that it still recalls the sample's
        // default volume — and, with Old Effects, still retriggers the envelopes.
        if note >= NOTE_FADE && note != NOTE_NONE {
            if instrument != INSTRUMENT_NONE {
                let last = self.channels[channel_index].last_note;
                let (sample, _) = self.map_note(instrument as u16, last);
                if sample != 0 {
                    self.channels[channel_index].note_volume = self.sample_volume(sample);
                }
            }
            if !self.old_effects {
                instrument = INSTRUMENT_NONE;
            }
        }

        // A lone instrument number: IT re-applies the sample's volume without retriggering,
        // unless the instrument actually changed or the sample already stopped, in which
        // case it synthesises the channel's remembered note and retriggers
        // (`kITInstrWithoutNote`, `it_instrument_memory_default.it`, `StoppedInstrSwap.it`).
        if note == NOTE_NONE && instrument != INSTRUMENT_NONE {
            let playing = self.foreground_playing(channel_index);
            // Sample mode compares the *sample* the number addresses, exactly as OpenMPT's
            // `kITInstrWithoutNote` branch does (`Snd_fx.cpp`: instrument mode compares
            // `chn.pModInstrument`, sample mode `chn.pModSample`), so a lone sample number
            // after `SCx` retriggers there too.
            let changed = match self.instrument_mode {
                true => self.foreground_instrument(channel_index) != instrument as u16,
                false => self.foreground_sample(channel_index) != instrument as u16,
            };
            if changed || !playing {
                note = self.channels[channel_index].last_note;
            }
        }

        if note <= MAX_NOTE {
            self.channels[channel_index].last_note = note;
            if !tone_portamento {
                self.check_new_note_action(context, channel_index, instrument, note);
                self.restore_channel_pan(channel_index);
            }
        }

        // A note with no instrument column still adopts an instrument number latched
        // earlier without a note.
        if note <= MAX_NOTE && instrument == INSTRUMENT_NONE && self.channels[channel_index].pending_instrument != 0 {
            instrument = self.channels[channel_index].pending_instrument;
            self.channels[channel_index].pending_instrument = 0;
        }
        if instrument != INSTRUMENT_NONE {
            self.instrument_change(channel_index, instrument, tone_portamento);
            if note == NOTE_NONE {
                self.channels[channel_index].pending_instrument = instrument;
            }
        }
        if note != NOTE_NONE {
            self.note_change(context, channel_index, note, instrument, tone_portamento);
        }
        self.volume_column_set(channel_index);
    }

    fn foreground_playing(&self, channel_index: usize) -> bool {
        self.channels[channel_index]
            .voice
            .and_then(|voice| self.voices.get(voice.index() as usize).filter(|state| state.owns(voice)))
            .is_some_and(|state| state.playing)
    }

    fn foreground_instrument(&self, channel_index: usize) -> u16 {
        self.channels[channel_index]
            .voice
            .and_then(|voice| self.voices.get(voice.index() as usize).filter(|state| state.owns(voice)))
            .map(|state| state.instrument)
            .unwrap_or(0)
    }

    fn foreground_state_mut(&mut self, channel_index: usize) -> Option<&mut ItVoiceState> {
        let voice = self.channels[channel_index].voice?;
        self.voices.get_mut(voice.index() as usize).filter(|state| state.owns(voice))
    }

    /// `RestorePanAndFilter`: a note puts back the channel pan an instrument's or sample's
    /// own panning displaced (`kITDoNotOverrideChannelPan`, `PanResetInstr.it`).
    fn restore_channel_pan(&mut self, channel_index: usize) {
        if let Some(pan) = self.channels[channel_index].restore_pan.take() {
            self.channels[channel_index].pan = pan;
        }
    }

    /// OpenMPT's `InstrumentChange`, minus the note: adopt the instrument's volume, pan,
    /// filter, NNA and envelope flags.
    fn instrument_change(&mut self, channel_index: usize, instrument: u8, tone_portamento: bool) {
        let note = self.channels[channel_index].last_note;
        let (sample, _) = self.map_note(instrument as u16, note);
        if sample == 0 {
            return;
        }
        self.channels[channel_index].note_volume = self.sample_volume(sample);
        // A lone instrument number leaves the playing voice's identity and sample data
        // alone, but immediately adopts the addressed sample/instrument volume product.
        // `it_channel_filter.it` varies instrument global volume this way while the
        // trace correctly continues to identify the original voice as instrument 1.
        let instrument_volume = self.instrument_volume(instrument as u16, sample);
        if let Some(state) = self.foreground_state_mut(channel_index) {
            state.instrument_volume = instrument_volume;
        }
        if !self.instrument_mode {
            return;
        }
        let Some(definition) = self.instrument_def(instrument as u16) else { return };
        // An enabled `IFC`/`IFR` overrides the channel's filter; a disabled one leaves a
        // previous `Zxx` in place. Not under portamento (`ins-flt-porta-reset.it`).
        let cutoff = definition.initial_filter_cutoff;
        let resonance = definition.initial_filter_resonance;
        if !tone_portamento {
            if let Some(cutoff) = cutoff {
                self.channels[channel_index].cutoff = cutoff.min(0x7F);
            }
            if let Some(resonance) = resonance {
                self.channels[channel_index].resonance = resonance.min(0x7F);
            }
        }
    }

    /// OpenMPT's `NoteChange`: the note-off family, then the note itself.
    fn note_change(&mut self, context: &mut TickContext<'_>, channel_index: usize, note: u8, instrument: u8, tone_portamento: bool) {
        match note {
            NOTE_OFF => {
                if let Some(voice) = self.channels[channel_index].voice {
                    self.key_off_voice(context, voice);
                }
                // IT compatibility (`noteoff3.it`): a note-off with an instrument number
                // under Old Effects releases the sustain loop without releasing the
                // envelopes or starting the fadeout.
                if !tone_portamento && self.old_effects && instrument != INSTRUMENT_NONE
                    && let Some(state) = self.foreground_state_mut(channel_index)
                {
                    state.note_fade = false;
                    state.key_off = false;
                }
                return;
            }
            NOTE_CUT => {
                self.cut_foreground(channel_index, false);
                return;
            }
            NOTE_FADE => {
                if self.instrument_mode && let Some(state) = self.foreground_state_mut(channel_index) {
                    state.note_fade = true;
                }
                return;
            }
            NOTE_NONE => return,
            _ => {}
        }

        let instrument_number = match self.instrument_mode {
            true => {
                if instrument != INSTRUMENT_NONE { instrument as u16 } else { self.foreground_instrument(channel_index) }
            }
            false => {
                if instrument != INSTRUMENT_NONE { instrument as u16 } else { self.channels[channel_index].last_instrument as u16 }
            }
        };
        let (sample, mapped_note) = self.map_note(instrument_number, note);
        if sample == 0 {
            // `kITEmptyNoteMapSlot`: an unmapped note plays nothing at all.
            return;
        }
        let Some(sample_index) = self.sample_index(sample) else { return };
        let reference_rate_hz = sample_index.reference_rate_hz();
        let frequency = frequency_from_note(reference_rate_hz, mapped_note);

        if tone_portamento && self.foreground_playing(channel_index) {
            // A portamento keeps the sounding sample and only moves the target — unless
            // Compatible Gxx is off and the sample actually changed, which restarts it
            // (`kITPortamentoSwapResetsPos`, `PortaSample.it`).
            self.channels[channel_index].portamento_target_hz = frequency;
            let current_sample = self.foreground_sample(channel_index);
            if !self.compatible_gxx && current_sample != sample {
                self.start_note(context, channel_index, instrument_number, sample, note, mapped_note, frequency, reference_rate_hz, true);
            }
            self.channels[channel_index].last_note = note;
            return;
        }

        // `kITClearPortaTarget`: a new non-portamento note resets the target outright.
        self.channels[channel_index].portamento_target_hz = 0;
        self.start_note(context, channel_index, instrument_number, sample, note, mapped_note, frequency, reference_rate_hz, false);
    }

    fn foreground_sample(&self, channel_index: usize) -> u16 {
        self.channels[channel_index]
            .voice
            .and_then(|voice| self.voices.get(voice.index() as usize).filter(|state| state.owns(voice)))
            .map(|state| state.sample)
            .unwrap_or(0)
    }
}

// ── triggering, New Note Actions and voice stealing ──────────────────────────────────

impl ItProcessor {
    /// Start a voice for `channel_index` and fill in its articulation.
    #[allow(clippy::too_many_arguments)]
    fn start_note(
        &mut self,
        context: &mut TickContext<'_>,
        channel_index: usize,
        instrument: u16,
        sample: u16,
        trigger_note: u8,
        note: u8,
        frequency: u32,
        reference_rate_hz: u32,
        keep_position: bool,
    ) {
        let Some(sample_index) = self.sample_index(sample) else { return };
        let carried_envelopes = self.carried_envelopes(channel_index, instrument);
        let region = sample_region(sample_index, sample_index.sustain_loop().is_some());
        let sample_default_pan = sample_index.default_pan();
        let channel_id = ChannelId(channel_index as u16);
        let offset = self.channels[channel_index].sample_offset;
        let tag = VoiceTag {
            channel: channel_index as u8,
            instrument: instrument.min(u8::MAX as u16) as u8,
            sample,
            note,
        };

        // Instrument and sample panning apply on a **note**, never on a lone instrument
        // number (`kITPanningReset`, `PanReset.it`), and they cancel surround
        // (`SmpInsPanSurround.it`).
        let instrument_pan = match self.instrument_mode {
            true => self.instrument_def(instrument).and_then(|definition| definition.default_pan),
            false => None,
        };
        if let Some(pan) = sample_default_pan.or(instrument_pan) {
            let channel = &mut self.channels[channel_index];
            if channel.restore_pan.is_none() {
                channel.restore_pan = Some(channel.pan);
            }
            channel.pan = bipolar_to_pan(pan);
            channel.surround = false;
        }

        // Pitch/pan separation moves the channel pan itself, from the **raw** pattern note
        // (`kITRealNoteMapping`, `kITPitchPanSeparation`).
        let pitch_pan = match self.instrument_mode {
            true => self.instrument_def(instrument).map(|definition| (definition.pitch_pan_separation, definition.pitch_pan_centre)),
            false => None,
        };
        if let Some((separation, centre)) = pitch_pan
            && separation != 0
        {
            let channel = &mut self.channels[channel_index];
            if channel.restore_pan.is_none() {
                channel.restore_pan = Some(channel.pan);
            }
            let delta = (trigger_note as i32 - centre as i32) * separation as i32 / 2;
            channel.pan = (channel.pan as i32 + delta).clamp(0, MAX_PAN) as i16;
        }

        let params = VoiceParams { dirty: DirtyBits::SAMPLE | DirtyBits::VOLUME | DirtyBits::PITCH | DirtyBits::PAN, ..VoiceParams::SILENT };
        let Some(voice) = self.allocate_voice(context, channel_id, tag, region, params, offset) else { return };
        let carry = match self.instrument_mode {
            true => self.instrument_def(instrument).map(|definition| {
                (definition.pitch_envelope_is_filter, definition.new_note_action)
            }),
            false => None,
        };
        let instrument_volume = self.instrument_volume(instrument, sample);
        let (note_volume, channel_volume, channel_pan, channel_surround, channel_cutoff, channel_resonance) = {
            let channel = &self.channels[channel_index];
            (channel.note_volume, channel.channel_volume, channel.pan, channel.surround, channel.cutoff, channel.resonance)
        };
        let Some(entry) = self.voices.get_mut(voice.index() as usize) else { return };
        *entry = ItVoiceState {
            voice: Some(voice),
            root_channel: channel_index as u8,
            foreground: true,
            instrument,
            sample,
            note,
            trigger_note,
            frequency_hz: frequency,
            reference_rate_hz,
            note_volume,
            instrument_volume,
            channel_volume,
            pan: channel_pan,
            surround: channel_surround,
            pitch_envelope_is_filter: carry.is_some_and(|(filter, _)| filter),
            playing: true,
            cutoff: channel_cutoff,
            resonance: channel_resonance,
            filter_active: false,
            new_note_action: carry.map(|(_, action)| action).unwrap_or(NewNoteAction::Cut),
            ..ItVoiceState::default()
        };
        if let Some((volume, panning, pitch)) = carried_envelopes {
            entry.volume_envelope = volume;
            entry.panning_envelope = panning;
            entry.pitch_envelope = pitch;
        }
        // `kITNNAReset`: the New Note Action is reset by a note, not by an instrument
        // number, so an `S73`..`S76` override survives a lone instrument (`s7xinsnum.it`).
        self.channels[channel_index].new_note_action = carry.map(|(_, action)| action).unwrap_or(NewNoteAction::Cut);
        if let Some(state) = context.voices.get_mut(voice) {
            // The cutoff law is a module-level property the mixer cannot read for itself
            // (task G2), so the processor that owns the voice sets it on every trigger.
            state.filter_mut().set_extended_range(self.has_extended_filter_range());
            if keep_position {
                state.set_position(0);
            }
        }
        self.channels[channel_index].voice = Some(voice);
        self.channels[channel_index].frequency_hz = frequency;
        self.channels[channel_index].triggered = true;
        self.channels[channel_index].sample_offset = 0;
        self.roll_swing(voice, instrument);
        self.apply_envelope_flags(voice, instrument);
    }

    /// Envelope Carry copies the preceding voice's counters into a newly allocated
    /// voice. Depending on its NNA, that voice is either still the channel foreground or
    /// was just detached and recorded as `m_lastMovedChannel`.
    fn carried_envelopes(&self, channel_index: usize, instrument: u16) -> Option<(ItEnvelopeState, ItEnvelopeState, ItEnvelopeState)> {
        let definition = self.instrument_def(instrument)?;
        let source_voice = self.channels[channel_index].voice.or(self.last_moved_voice)?;
        let source = self.voices.get(source_voice.index() as usize).filter(|state| state.owns(source_voice))?;
        let mut volume = ItEnvelopeState::default();
        let mut panning = ItEnvelopeState::default();
        let mut pitch = ItEnvelopeState::default();
        let mut any = false;
        if definition.volume_envelope.as_ref().is_some_and(|envelope| envelope.carry) {
            volume = source.volume_envelope;
            any = true;
        }
        if definition.panning_envelope.as_ref().is_some_and(|envelope| envelope.carry) {
            panning = source.panning_envelope;
            any = true;
        }
        if definition.pitch_envelope.as_ref().is_some_and(|envelope| envelope.carry) {
            pitch = source.pitch_envelope;
            any = true;
        }
        any.then_some((volume, panning, pitch))
    }

    /// Allocate a voice, stealing a background one if the pool is full or IT already owns
    /// its 256 virtual channels. The latter keeps the sixteen-slot jam reserve available
    /// to MIDI even though both sources share one global pool.
    fn allocate_voice(
        &mut self,
        context: &mut TickContext<'_>,
        channel: ChannelId,
        tag: VoiceTag,
        region: SampleRegion,
        params: VoiceParams,
        offset: u32,
    ) -> Option<VoiceId> {
        // Replacing this channel's own foreground voice is net-zero — `trigger_channel`
        // releases it before allocating — so it is always allowed and costs a lane read.
        let replaces_owned_foreground = context.channels.foreground(channel).is_some_and(|voice| {
            context.voices.get(voice).is_some()
                && self.voices.get(voice.index() as usize).is_some_and(|state| state.owns(voice) && state.foreground)
        });
        if (replaces_owned_foreground || self.within_voice_quota(context))
            && let Some(voice) = context.trigger_channel(channel, tag, region, params, offset)
        {
            return Some(voice);
        }
        let victim = self.choose_victim(context)?;
        if let Some(entry) = self.voices.get_mut(victim.index() as usize) {
            entry.release();
        }
        context.voices.release(victim);
        context.trigger_channel(channel, tag, region, params, offset)
    }

    /// Whether IT may own one more voice: at most [`VIRTUAL_CHANNELS`] slots of a wider
    /// global pool may be IT-owned, so a persistent host's sixteen jam slots stay
    /// reachable by live MIDI.
    ///
    /// A pool holding fewer voices than the limit cannot be over it, and that integer
    /// compare is the answer on every module short of a saturated IT, so the walk over
    /// the pool is not on the ordinary trigger path.
    fn within_voice_quota(&self, context: &TickContext<'_>) -> bool {
        if context.voices.voices_active() < VIRTUAL_CHANNELS {
            return true;
        }
        let owned_voice_count = context
            .voices
            .iter()
            .filter(|(voice, _)| self.voices.get(voice.index() as usize).is_some_and(|state| state.owns(*voice)))
            .count();
        owned_voice_count < VIRTUAL_CHANNELS
    }

    /// Architecture open question **Q3**, settled by OpenMPT's `GetNNAChannel`
    /// (`soundlib/Snd_fx.cpp:2257`): the quietest **background** voice, with a looping
    /// sample counting half, and a fully faded one taken outright. A foreground voice of
    /// another channel is never a candidate.
    fn choose_victim(&self, context: &TickContext<'_>) -> Option<VoiceId> {
        let mut best: Option<(VoiceId, i64)> = None;
        for (voice, _) in context.voices.iter() {
            let Some(state) = self.voices.get(voice.index() as usize) else { continue };
            if !state.owns(voice) || state.foreground {
                continue;
            }
            if state.fadeout == 0 {
                return Some(voice);
            }
            // Schism folds the fadeout into the score explicitly because the 14-bit
            // mixing volume is recomputed rather than cached; the ordering is the same.
            let mut score = ((state.real_volume as i64) << 9) | state.note_volume.clamp(0, MAX_NOTE_VOLUME) as i64;
            if context.voices.get(voice).is_some_and(|slot| slot.region().loop_span().is_some()) {
                score /= 2;
            }
            if best.is_none_or(|(_, current)| score < current) {
                best = Some((voice, score));
            }
        }
        best.map(|(voice, _)| voice)
    }

    /// The per-note random volume and pan variation, rolled once at note-on.
    fn roll_swing(&mut self, voice: VoiceId, instrument: u16) {
        if !self.instrument_mode {
            return;
        }
        let Some(definition) = self.module.instrument(InstrumentId(instrument.wrapping_sub(1))) else { return };
        let volume_variation = definition.random_volume_variation;
        let pan_variation = definition.random_pan_variation;
        let (volume_random, pan_random) = (self.next_signed_byte(), self.next_signed_byte());
        let Some(entry) = self.voices.get_mut(voice.index() as usize) else { return };
        if volume_variation != 0 {
            // OpenMPT: `((rand * RV) / 64 + 1) * insVol / 199`, applied to the instrument
            // volume rather than to the note volume (`kITSwingBehaviour`).
            entry.volume_swing = ((volume_random as i32 * volume_variation as i32 / 64 + 1) * entry.instrument_volume as i32 / 199) as i16;
        }
        if pan_variation != 0 {
            entry.pan_swing = (pan_random as i32 * pan_variation as i32 * 4 / 128) as i16;
        }
    }

    fn next_signed_byte(&mut self) -> i8 { (self.random.next_u32() >> 16) as u8 as i8 }

    /// Copy the instrument's envelope enable flags onto the voice, which is what undoes an
    /// earlier `S77`-style pause.
    fn apply_envelope_flags(&mut self, voice: VoiceId, instrument: u16) {
        let enabled = match self.instrument_mode {
            true => self.module.instrument(InstrumentId(instrument.wrapping_sub(1))).map(|definition| {
                (
                    definition.volume_envelope.is_some(),
                    definition.panning_envelope.is_some(),
                    definition.pitch_envelope.is_some(),
                )
            }),
            false => None,
        };
        let Some((volume, panning, pitch)) = enabled else { return };
        let Some(entry) = self.voices.get_mut(voice.index() as usize) else { return };
        entry.volume_envelope.enabled = volume;
        entry.panning_envelope.enabled = panning;
        entry.pitch_envelope.enabled = pitch;
    }

    /// `CheckNNA`: the duplicate check first, then the New Note Action that detaches the
    /// channel's sounding voice.
    fn check_new_note_action(&mut self, context: &mut TickContext<'_>, channel_index: usize, instrument: u8, note: u8) {
        if !self.instrument_mode {
            // Sample mode has no New Note Action: the old voice is simply replaced, which
            // `trigger_channel` already does.
            return;
        }
        let instrument_number = match instrument != INSTRUMENT_NONE {
            true => instrument as u16,
            false => self.foreground_instrument(channel_index),
        };
        let (sample, _) = self.map_note(instrument_number, note);
        self.duplicate_check(context, channel_index, instrument_number, sample, note);

        let Some(voice) = self.channels[channel_index].voice else { return };
        let Some(state) = self.voices.get(voice.index() as usize) else { return };
        if !state.owns(voice) || !state.playing {
            return;
        }
        let action = self.channels[channel_index].new_note_action;
        // A New Note Action of `Cut` keeps no voice: `trigger_channel` releases whatever
        // foreground it finds, which is what IT and libxmp both do — libxmp's
        // `libxmp_virt_setvol` frees a background voice the moment its volume reaches
        // zero, so a cut note never occupies a virtual channel at all. Only the other
        // three actions leave something sounding.
        if matches!(action, NewNoteAction::Cut) {
            return;
        }
        let Some(detached) = context.detach_channel(ChannelId(channel_index as u16)) else { return };
        self.channels[channel_index].voice = None;
        let Some(entry) = self.voices.get_mut(detached.index() as usize) else { return };
        entry.foreground = false;
        match action {
            NewNoteAction::Continue => {}
            // `KeyOff` on the moved channel: the sustain loop goes, and the fadeout starts
            // unless a non-looping volume envelope is left to run out.
            NewNoteAction::NoteOff => self.key_off_voice(context, detached),
            NewNoteAction::NoteFade => entry.note_fade = true,
            NewNoteAction::Cut => {
                entry.fadeout = 0;
                entry.note_fade = true;
            }
        }
        self.last_moved_voice = Some(detached);
    }

    /// `DCT`/`DCA`: every voice rooted on this channel whose instrument's duplicate check
    /// matches is cut, released or faded before the new note starts.
    fn duplicate_check(&mut self, context: &mut TickContext<'_>, channel_index: usize, instrument: u16, sample: u16, note: u8) {
        let mut affected: [Option<VoiceId>; VIRTUAL_CHANNELS] = [None; VIRTUAL_CHANNELS];
        let mut count = 0usize;
        for (voice, _) in context.voices.iter() {
            let Some(state) = self.voices.get(voice.index() as usize) else { continue };
            if !state.owns(voice) || state.root_channel as usize != channel_index {
                continue;
            }
            let Some(definition) = self.instrument_def(state.instrument) else { continue };
            let duplicate = match definition.duplicate_check {
                DuplicateCheck::Off => false,
                // `kITDCTBehaviour`: the raw pattern notes are compared, and the
                // instrument must match too.
                DuplicateCheck::Note => state.trigger_note == note && state.instrument == instrument,
                DuplicateCheck::Sample => sample != 0 && state.sample == sample && state.instrument == instrument,
                DuplicateCheck::Instrument => state.instrument == instrument,
            };
            if duplicate && count < affected.len() {
                affected[count] = Some(voice);
                count += 1;
            }
        }
        for voice in affected.iter().flatten().copied() {
            let Some(state) = self.voices.get_mut(voice.index() as usize) else { continue };
            let Some(definition) = self.module.instrument(InstrumentId(state.instrument.wrapping_sub(1))) else { continue };
            match definition.duplicate_action {
                DuplicateAction::Cut => {
                    state.key_off = true;
                    state.note_volume = 0;
                    state.fadeout = 0;
                    state.note_fade = true;
                }
                DuplicateAction::NoteOff => self.key_off_voice(context, voice),
                DuplicateAction::NoteFade => state.note_fade = true,
            }
        }
    }

    /// `===`, `S71` and NNA note-off: release the sustain loop, and start the fadeout when
    /// there is no volume envelope or the envelope loops.
    fn key_off_voice(&mut self, context: &mut TickContext<'_>, voice: VoiceId) {
        let Some(state) = self.voices.get_mut(voice.index() as usize) else { return };
        if !state.owns(voice) {
            return;
        }
        let first_release = !state.key_off;
        state.key_off = true;
        let instrument = state.instrument;
        let sample = state.sample;
        let has_volume_envelope = self.instrument_mode
            && self
                .module
                .instrument(InstrumentId(instrument.wrapping_sub(1)))
                .is_some_and(|definition| definition.volume_envelope.is_some());
        let loops = self
            .module
            .instrument(InstrumentId(instrument.wrapping_sub(1)))
            .and_then(|definition| definition.volume_envelope.as_ref())
            .is_some_and(|envelope| envelope.loop_span.is_some());
        let fadeout = self
            .module
            .instrument(InstrumentId(instrument.wrapping_sub(1)))
            .map(|definition| definition.fadeout)
            .unwrap_or(0);
        if let Some(state) = self.voices.get_mut(voice.index() as usize)
            && (!has_volume_envelope || (loops && fadeout != 0))
        {
            state.note_fade = true;
        }
        // The sustain loop is released here: the sample's normal loop comes back, or the
        // sample stops looping altogether (`SusAfterLoop.it`).
        if first_release
            && let Some(sample_index) = self.sample_index(sample)
            && sample_index.sustain_loop().is_some()
            && let Some(slot) = context.voices.get_mut(voice)
        {
            let region = sample_region(sample_index, false);
            let position = slot.position() >> 32;
            slot.set_region(region);
            let loop_end = sample_index.loop_end() as u64;
            let loop_start = sample_index.loop_start() as u64;
            if sample_index.loop_mode().is_looping() && loop_end > loop_start && position > loop_end {
                let wrapped = loop_start + (position - loop_start) % (loop_end - loop_start);
                slot.set_position(wrapped << 32);
            } else {
                slot.set_position(position << 32);
            }
        }
    }

    /// `^^^` and `SCx`: IT really stops the sample rather than muting it
    /// (`kITSCxStopsSample`, `scx.it`).
    fn cut_foreground(&mut self, channel_index: usize, by_scx: bool) {
        // `kITNoteCutWithPorta`: picking the note back up with a lone instrument number
        // also forgets the portamento's pitch.
        self.channels[channel_index].portamento_target_hz = 0;
        if let Some(state) = self.foreground_state_mut(channel_index) {
            state.fadeout = 0;
            state.note_fade = true;
            state.playing = false;
            state.cut_by_scx = by_scx;
        }
    }
}

/// A model pan position as IT's 0..=256 channel pan.
fn bipolar_to_pan(pan: I1F15) -> i16 {
    (((pan.to_bits() as i32 + 32_768) * MAX_PAN + 32_768) / 65_536).clamp(0, MAX_PAN) as i16
}

/// IT's 0..=256 pan as the bipolar position a voice takes.
fn pan_to_bipolar_position(pan: i16) -> I1F15 {
    bipolar_from_ratio(pan as i32 - 128, 128)
}

// ── the effect set ───────────────────────────────────────────────────────────────────

impl ItProcessor {
    /// The volume column's `vxx` and `pxx`, which land on the note's own tick.
    fn volume_column_set(&mut self, channel_index: usize) {
        match self.channels[channel_index].row.volume_command() {
            ItVolumeCommand::Volume(volume) => {
                let volume = volume.min(64) as i32 * 4;
                self.channels[channel_index].note_volume = volume;
                if let Some(state) = self.foreground_state_mut(channel_index) {
                    state.note_volume = volume;
                }
            }
            ItVolumeCommand::Panning(pan) => self.set_pan(channel_index, pan.min(64) as i16 * 4),
            _ => {}
        }
    }

    /// Every panning command cancels surround and the panbrello offset (`kPanOverride`).
    fn set_pan(&mut self, channel_index: usize, pan: i16) {
        let channel = &mut self.channels[channel_index];
        channel.pan = pan.clamp(0, MAX_PAN as i16);
        channel.surround = false;
        channel.restore_pan = None;
        channel.panbrello_offset = 0;
        if let Some(state) = self.foreground_state_mut(channel_index) {
            state.pan = pan.clamp(0, MAX_PAN as i16);
            state.surround = false;
            state.pan_swing = 0;
        }
    }

    /// Tick-zero effects, which run again at the start of every pattern-delay repeat.
    fn static_effect(&mut self, context: &mut TickContext<'_>, channel_index: usize, outcome: &mut TickOutcome, song_first_tick: bool) {
        let cell = self.channels[channel_index].row;
        let parameter = cell.info;
        match cell.command {
            COMMAND_SET_SPEED => {
                if parameter != 0 && song_first_tick {
                    self.song_speed = parameter;
                }
            }
            COMMAND_POSITION_JUMP => {
                if song_first_tick {
                    self.flow.pattern_jump(parameter as u16);
                }
            }
            COMMAND_PATTERN_BREAK => {
                // IT reads the break row in hexadecimal, unlike Scream Tracker 3.
                if song_first_tick {
                    self.flow.pattern_break(parameter as u16);
                }
            }
            COMMAND_VOLUME_SLIDE => self.volume_slide(channel_index, parameter, true),
            COMMAND_PORTAMENTO_DOWN => self.pitch_slide_command(channel_index, parameter, true, true),
            COMMAND_PORTAMENTO_UP => self.pitch_slide_command(channel_index, parameter, false, true),
            COMMAND_TONE_PORTAMENTO => {}
            COMMAND_VIBRATO => self.init_vibrato(channel_index, parameter, false),
            COMMAND_FINE_VIBRATO => self.init_vibrato(channel_index, parameter, true),
            COMMAND_TREMOR => self.init_tremor(channel_index, parameter),
            COMMAND_ARPEGGIO => {
                if parameter != 0 {
                    self.channels[channel_index].arpeggio_memory = parameter;
                }
                self.channels[channel_index].arpeggio_tick = 0;
            }
            COMMAND_VIBRATO_VOLUME_SLIDE => {
                if parameter != 0 {
                    self.channels[channel_index].volume_slide_memory = parameter;
                }
                self.init_vibrato(channel_index, 0, false);
                self.volume_slide(channel_index, parameter, true);
            }
            COMMAND_PORTAMENTO_VOLUME_SLIDE => {
                if parameter != 0 {
                    self.channels[channel_index].volume_slide_memory = parameter;
                }
                self.volume_slide(channel_index, parameter, true);
            }
            COMMAND_CHANNEL_VOLUME => {
                // Out-of-range values are ignored rather than clamped.
                if parameter <= MAX_CHANNEL_VOLUME {
                    self.channels[channel_index].channel_volume = parameter;
                    if let Some(state) = self.foreground_state_mut(channel_index) {
                        state.channel_volume = parameter;
                    }
                }
            }
            COMMAND_CHANNEL_VOLUME_SLIDE => self.channel_volume_slide(channel_index, parameter, true),
            COMMAND_OFFSET => self.sample_offset(context, channel_index, parameter),
            COMMAND_PAN_SLIDE => self.pan_slide(channel_index, parameter, true),
            COMMAND_RETRIGGER => self.init_retrigger(context, channel_index, parameter),
            COMMAND_TREMOLO => self.init_tremolo(channel_index, parameter),
            COMMAND_SPECIAL => self.special(context, channel_index, parameter, outcome, song_first_tick),
            COMMAND_TEMPO => {
                if parameter != 0 {
                    self.channels[channel_index].tempo_memory = parameter;
                }
                let value = self.channels[channel_index].tempo_memory;
                if value >= 0x20 && song_first_tick {
                    outcome.tempo_bpm = value as u16;
                }
            }
            COMMAND_GLOBAL_VOLUME => {
                // `globalvol-invalid.it`: `V81`..`VFF` change nothing at all.
                if parameter <= MAX_GLOBAL_VOLUME {
                    self.global_volume = parameter;
                    context.report_global_volume(unit_from_ratio(self.global_volume as u32, MAX_GLOBAL_VOLUME as u32));
                }
            }
            COMMAND_GLOBAL_VOLUME_SLIDE => self.global_volume_slide(channel_index, parameter, true),
            COMMAND_PAN => {
                // IT rounds the full byte onto its own 0..64 scale, unlike OpenMPT's 0..255.
                self.set_pan(channel_index, ((parameter as i32 + 2) >> 2).min(64) as i16 * 4);
            }
            COMMAND_PANBRELLO => self.init_panbrello(channel_index, parameter),
            COMMAND_MIDI_MACRO => {
                self.channels[channel_index].macro_value = (parameter as i32) << 16;
                self.channels[channel_index].macro_target = (parameter as i32) << 16;
                self.channels[channel_index].macro_slide = 0;
                self.midi_macro(context, channel_index, parameter);
            }
            COMMAND_SMOOTH_MIDI_MACRO => {
                let channel = &mut self.channels[channel_index];
                channel.macro_target = (parameter as i32) << 16;
                channel.macro_slide = (channel.macro_target - channel.macro_value) / self.song_speed.max(1) as i32;
            }
            _ => {}
        }
        self.volume_column_effect(channel_index, true);
    }

    /// Per-tick effects, for every tick of the row past the channel's own first.
    fn tick_effect(&mut self, context: &TickContext<'_>, channel_index: usize, tick_in_repeat: u16, outcome: &mut TickOutcome) {
        let cell = self.channels[channel_index].row;
        let parameter = cell.info;
        match cell.command {
            COMMAND_VOLUME_SLIDE => self.volume_slide(channel_index, parameter, false),
            COMMAND_PORTAMENTO_DOWN => self.pitch_slide_command(channel_index, parameter, true, false),
            COMMAND_PORTAMENTO_UP => self.pitch_slide_command(channel_index, parameter, false, false),
            COMMAND_TONE_PORTAMENTO => self.tone_portamento(channel_index),
            COMMAND_VIBRATO | COMMAND_FINE_VIBRATO => self.run_vibrato(channel_index),
            COMMAND_TREMOR => self.run_tremor(channel_index),
            COMMAND_ARPEGGIO => self.run_arpeggio(channel_index, tick_in_repeat),
            COMMAND_VIBRATO_VOLUME_SLIDE => {
                self.run_vibrato(channel_index);
                self.volume_slide(channel_index, parameter, false);
            }
            COMMAND_PORTAMENTO_VOLUME_SLIDE => {
                self.tone_portamento(channel_index);
                self.volume_slide(channel_index, parameter, false);
            }
            COMMAND_CHANNEL_VOLUME_SLIDE => self.channel_volume_slide(channel_index, parameter, false),
            COMMAND_PAN_SLIDE => self.pan_slide(channel_index, parameter, false),
            COMMAND_RETRIGGER => self.run_retrigger(channel_index),
            COMMAND_TREMOLO => self.run_tremolo(channel_index),
            COMMAND_TEMPO => outcome.tempo_bpm = self.tempo_slide(channel_index, outcome.tempo_bpm),
            COMMAND_GLOBAL_VOLUME_SLIDE => self.global_volume_slide(channel_index, parameter, false),
            COMMAND_PANBRELLO => self.run_panbrello(channel_index),
            COMMAND_SMOOTH_MIDI_MACRO => self.smooth_midi_macro(context, channel_index),
            _ => {}
        }
        self.volume_column_effect(channel_index, false);
    }

    /// The volume column's own slides and portamentos.
    fn volume_column_effect(&mut self, channel_index: usize, first_tick: bool) {
        let command = self.channels[channel_index].row.volume_command();
        let recall = |channel: &mut ItChannel, value: u8| -> u8 {
            if value != 0 {
                channel.volume_column_memory = value;
                value
            } else {
                channel.volume_column_memory
            }
        };
        match command {
            // Volume-column fine slides run on the channel's first tick only — and,
            // unlike `DxF`, **not** on a pattern-delay repeat (`FineVolColSlide.it`).
            ItVolumeCommand::FineVolumeUp(value) => {
                if first_tick {
                    let value = recall(&mut self.channels[channel_index], value);
                    self.slide_note_volume(channel_index, value as i32 * 4);
                }
            }
            ItVolumeCommand::FineVolumeDown(value) => {
                if first_tick {
                    let value = recall(&mut self.channels[channel_index], value);
                    self.slide_note_volume(channel_index, -(value as i32) * 4);
                }
            }
            ItVolumeCommand::VolumeSlideUp(value) => {
                if !first_tick {
                    let value = recall(&mut self.channels[channel_index], value);
                    self.slide_note_volume(channel_index, value as i32 * 4);
                }
            }
            ItVolumeCommand::VolumeSlideDown(value) => {
                if !first_tick {
                    let value = recall(&mut self.channels[channel_index], value);
                    self.slide_note_volume(channel_index, -(value as i32) * 4);
                }
            }
            // `kITVolColFinePortamento`: the volume column never does a fine portamento.
            ItVolumeCommand::PitchSlideDown(_) => {
                if !first_tick {
                    let amount = self.channels[channel_index].pitch_slide_memory as i32 * 4;
                    self.slide_frequency(channel_index, -amount);
                }
            }
            ItVolumeCommand::PitchSlideUp(_) => {
                if !first_tick {
                    let amount = self.channels[channel_index].pitch_slide_memory as i32 * 4;
                    self.slide_frequency(channel_index, amount);
                }
            }
            ItVolumeCommand::TonePortamento(_) => {
                if !first_tick {
                    self.tone_portamento(channel_index);
                }
            }
            ItVolumeCommand::VibratoDepth(value) => {
                if first_tick {
                    if value != 0 {
                        self.channels[channel_index].vibrato_depth = value << 2;
                    }
                    self.init_vibrato(channel_index, 0, false);
                } else {
                    self.run_vibrato(channel_index);
                }
            }
            _ => {}
        }
    }

    /// `Dxy`, and the `Kxy`/`Lxy` half that shares its memory.
    ///
    /// ITTECH.TXT's order of testing is `Dx0`, `D0x`, `DxF`, `DFx`, which is what resolves
    /// the `D0F` / `DF0` ambiguity: `DF0` is a per-tick slide **up** by 15 and `D0F` a
    /// per-tick slide **down** by 15, each with one extra application on the first tick.
    fn volume_slide(&mut self, channel_index: usize, parameter: u8, first_tick: bool) {
        let value = {
            let channel = &mut self.channels[channel_index];
            if parameter != 0 {
                channel.volume_slide_memory = parameter;
            }
            channel.volume_slide_memory
        };
        let (high, low) = (value >> 4, value & 0x0F);
        if low == 0 {
            // `Dx0`, including `DF0`.
            if !first_tick || high == 0x0F {
                self.slide_note_volume(channel_index, high as i32 * 4);
            }
        } else if high == 0 {
            // `D0x`, including `D0F`.
            if !first_tick || low == 0x0F {
                self.slide_note_volume(channel_index, -(low as i32) * 4);
            }
        } else if low == 0x0F {
            if first_tick {
                self.slide_note_volume(channel_index, high as i32 * 4);
            }
        } else if high == 0x0F && first_tick {
            self.slide_note_volume(channel_index, -(low as i32) * 4);
        }
    }

    fn slide_note_volume(&mut self, channel_index: usize, delta: i32) {
        let volume = (self.channels[channel_index].note_volume + delta).clamp(0, MAX_NOTE_VOLUME);
        self.channels[channel_index].note_volume = volume;
        if let Some(state) = self.foreground_state_mut(channel_index) {
            state.note_volume = volume;
        }
    }

    /// `Exx` / `Fxx`, with `EFx`/`FFx` fine and `EEx`/`FEx` extra-fine.
    fn pitch_slide_command(&mut self, channel_index: usize, parameter: u8, down: bool, first_tick: bool) {
        let value = {
            let channel = &mut self.channels[channel_index];
            if parameter != 0 {
                channel.pitch_slide_memory = parameter;
            }
            channel.pitch_slide_memory
        };
        let high = value & 0xF0;
        let low = (value & 0x0F) as i32;
        let amount = if high < 0xE0 {
            if first_tick {
                return;
            }
            value as i32 * 4
        } else {
            if !first_tick || low == 0 {
                return;
            }
            if high == 0xE0 { low } else { low * 4 }
        };
        self.slide_frequency(channel_index, if down { -amount } else { amount });
    }

    /// One IT pitch slide, in 1/64ths of a semitone.
    ///
    /// Linear slides multiply the frequency through IT's two lookup-table domains: fine
    /// amounts below 16 index the fine table directly, while larger amounts index the
    /// coarse table at `|amount| / 4` and discard the low two bits. Amiga slides use IT's
    /// own reciprocal form, which is still a frequency operation — IT never holds a period.
    fn slide_frequency(&mut self, channel_index: usize, amount: i32) {
        let frequency = self.channels[channel_index].frequency_hz;
        let updated = self.slide_value(frequency, amount);
        self.channels[channel_index].frequency_hz = updated;
    }

    fn slide_value(&self, frequency: u32, amount: i32) -> u32 {
        if frequency == 0 || amount == 0 {
            return frequency;
        }
        if !self.linear_slides {
            // `freq' = 1712 · 8363 · freq / (1712 · 8363 ∓ freq · amount)`, which is a
            // slide of `amount` in Impulse Tracker's own period domain — the amount is
            // already the command parameter times four.
            const AMIGA_NUMERATOR: i64 = 1712 * 8363;
            let denominator = AMIGA_NUMERATOR - frequency as i64 * amount as i64;
            if denominator <= 0 {
                return u32::MAX;
            }
            return (frequency as i64 * AMIGA_NUMERATOR / denominator).clamp(1, u32::MAX as i64) as u32;
        }
        let magnitude = amount.unsigned_abs().min(255 * 4 + 3);
        let ratio = if magnitude < 16 {
            match amount > 0 {
                true => fine_linear_slide_up_q16(magnitude as u8),
                false => fine_linear_slide_down_q16(magnitude as u8),
            }
        } else {
            let coarse = (magnitude / 4).min(255) as u8;
            match amount > 0 {
                true => linear_slide_up_q16(coarse),
                false => linear_slide_down_q16(coarse),
            }
        };
        let mut value = apply_slide_ratio(frequency, ratio);
        if value == frequency {
            value = match amount > 0 {
                true => value.saturating_add(1),
                false => value.saturating_sub(1),
            };
        }
        value.max(1)
    }

    /// `Gxx`, and the portamento half of `Lxy`.
    fn tone_portamento(&mut self, channel_index: usize) {
        let target = self.channels[channel_index].portamento_target_hz;
        if target == 0 {
            return;
        }
        let memory = match self.compatible_gxx {
            true => self.channels[channel_index].tone_portamento_memory,
            false => self.channels[channel_index].tone_portamento_memory,
        };
        let amount = memory as i32 * 4;
        if amount == 0 {
            return;
        }
        let frequency = self.channels[channel_index].frequency_hz;
        let updated = if frequency < target {
            self.slide_value(frequency, amount).min(target)
        } else {
            self.slide_value(frequency, -amount).max(target)
        };
        self.channels[channel_index].frequency_hz = updated;
        if updated == target {
            // `kITPortaTargetReached`: the target is consumed once reached.
            self.channels[channel_index].portamento_target_hz = 0;
        }
    }
}

impl ItProcessor {
    /// `Hxy` and `Uxy`. The speed nibble is scaled the same for both; only the depth
    /// differs, and Old Effects doubles it and inverts the sign.
    fn init_vibrato(&mut self, channel_index: usize, parameter: u8, fine: bool) {
        let cell = self.channels[channel_index].row;
        {
            let channel = &mut self.channels[channel_index];
            if parameter >> 4 != 0 {
                channel.vibrato_speed = (parameter >> 4) << 2;
            }
            if parameter & 0x0F != 0 {
                channel.vibrato_depth = if fine { parameter & 0x0F } else { (parameter & 0x0F) << 2 };
            }
        }
        // `Hxy` resets the phase only for a real note; `Uxy` resets it for a note-off too.
        let note_present = cell.note != NOTE_NONE;
        let reset = if fine { note_present } else { note_present && cell.note <= MAX_NOTE };
        if reset {
            self.channels[channel_index].vibrato_position = 0;
            self.channels[channel_index].last_vibrato = 0;
        }
        if self.old_effects {
            let last = self.channels[channel_index].last_vibrato;
            self.apply_vibrato(channel_index, last);
        } else {
            self.run_vibrato(channel_index);
        }
    }

    fn run_vibrato(&mut self, channel_index: usize) {
        let (waveform, speed) = {
            let channel = &self.channels[channel_index];
            (channel.vibrato_waveform, channel.vibrato_speed)
        };
        let position = self.channels[channel_index].vibrato_position.wrapping_add(speed);
        self.channels[channel_index].vibrato_position = position;
        let sample = oscillator_sample(waveform, position, &mut self.random);
        self.channels[channel_index].last_vibrato = sample;
        self.apply_vibrato(channel_index, sample);
    }

    fn apply_vibrato(&mut self, channel_index: usize, sample: i32) {
        let depth = self.channels[channel_index].vibrato_depth as i32;
        let mut delta = ((sample * depth) * 4 + 128) >> 8;
        if self.old_effects {
            // "Yes, vibrato goes backwards with old effects enabled" — and twice as deep.
            delta = -delta * 2;
        }
        // Vibrato is an output offset, not a change to the channel's own pitch.
        let frequency = self.channels[channel_index].frequency_hz;
        self.channels[channel_index].output_frequency_hz = self.slide_value(frequency, delta);
        self.channels[channel_index].output_frequency_set = true;
    }

    /// `Ixy`: `x` ticks on then `y` ticks off, one more of each under Old Effects.
    fn init_tremor(&mut self, channel_index: usize, parameter: u8) {
        if parameter != 0 {
            self.channels[channel_index].tremor_memory = parameter;
        }
        self.run_tremor(channel_index);
    }

    fn run_tremor(&mut self, channel_index: usize) {
        let memory = self.channels[channel_index].tremor_memory;
        let extra = u8::from(self.old_effects);
        let on_time = (memory >> 4) + extra;
        let off_time = (memory & 0x0F) + extra;
        let channel = &mut self.channels[channel_index];
        if channel.tremor_count == 0 {
            channel.tremor_on = !channel.tremor_on;
            channel.tremor_count = if channel.tremor_on { on_time.max(1) } else { off_time.max(1) };
        }
        channel.tremor_count = channel.tremor_count.saturating_sub(1);
    }

    /// `Jxy`: base note, `+x`, `+y`, repeating every three ticks.
    fn run_arpeggio(&mut self, channel_index: usize, tick_in_repeat: u16) {
        let memory = self.channels[channel_index].arpeggio_memory;
        let semitones = match tick_in_repeat % 3 {
            0 => 0,
            1 => memory >> 4,
            _ => memory & 0x0F,
        };
        if semitones == 0 {
            return;
        }
        let frequency = self.channels[channel_index].frequency_hz;
        self.channels[channel_index].output_frequency_hz = scale_frequency(frequency, semitones as i32 * UNITS_PER_SEMITONE);
        self.channels[channel_index].output_frequency_set = true;
    }

    /// `Nxy`, the channel volume slide. Range 0..=64, no `D0F`/`DF0` immediate hack.
    fn channel_volume_slide(&mut self, channel_index: usize, parameter: u8, first_tick: bool) {
        let value = {
            let channel = &mut self.channels[channel_index];
            if parameter != 0 {
                channel.channel_volume_slide_memory = parameter;
            }
            channel.channel_volume_slide_memory
        };
        let (high, low) = (value >> 4, value & 0x0F);
        let delta = if low == 0 {
            if first_tick { return } else { high as i32 }
        } else if high == 0 {
            if first_tick { return } else { -(low as i32) }
        } else if low == 0x0F {
            if !first_tick { return } else { high as i32 }
        } else if high == 0x0F {
            if !first_tick { return } else { -(low as i32) }
        } else {
            return;
        };
        let volume = (self.channels[channel_index].channel_volume as i32 + delta).clamp(0, MAX_CHANNEL_VOLUME as i32) as u8;
        self.channels[channel_index].channel_volume = volume;
        if let Some(state) = self.foreground_state_mut(channel_index) {
            state.channel_volume = volume;
        }
    }

    /// `Pxy`, the panning slide. A no-op on a surround channel, as IT2 has it.
    fn pan_slide(&mut self, channel_index: usize, parameter: u8, first_tick: bool) {
        if self.channels[channel_index].surround {
            return;
        }
        let value = {
            let channel = &mut self.channels[channel_index];
            if parameter != 0 {
                channel.pan_slide_memory = parameter;
            }
            channel.pan_slide_memory
        };
        let (high, low) = (value >> 4, value & 0x0F);
        let delta = if low == 0 {
            if first_tick { return } else { -(high as i32) * 4 }
        } else if high == 0 {
            if first_tick { return } else { low as i32 * 4 }
        } else if low == 0x0F {
            if !first_tick { return } else { -(high as i32) * 4 }
        } else if high == 0x0F {
            if !first_tick { return } else { low as i32 * 4 }
        } else {
            return;
        };
        let pan = (self.channels[channel_index].pan as i32 + delta).clamp(0, MAX_PAN) as i16;
        self.channels[channel_index].pan = pan;
        if let Some(state) = self.foreground_state_mut(channel_index) {
            state.pan = pan;
        }
    }

    /// `Wxy`, the global volume slide. Range 0..=128, and its memory is per channel.
    fn global_volume_slide(&mut self, channel_index: usize, parameter: u8, first_tick: bool) {
        let value = {
            let channel = &mut self.channels[channel_index];
            if parameter != 0 {
                channel.global_volume_slide_memory = parameter;
            }
            channel.global_volume_slide_memory
        };
        let (high, low) = (value >> 4, value & 0x0F);
        let delta = if low == 0 {
            if first_tick { return } else { high as i32 * 2 }
        } else if high == 0 {
            if first_tick { return } else { -(low as i32) * 2 }
        } else if low == 0x0F {
            if !first_tick { return } else { high as i32 * 2 }
        } else if high == 0x0F {
            if !first_tick { return } else { -(low as i32) * 2 }
        } else {
            return;
        };
        self.global_volume = (self.global_volume as i32 + delta).clamp(0, MAX_GLOBAL_VOLUME as i32) as u8;
    }

    /// `Oxx`, with `SAy`'s persistent high byte.
    fn sample_offset(&mut self, context: &mut TickContext<'_>, channel_index: usize, parameter: u8) {
        let value = {
            let channel = &mut self.channels[channel_index];
            if parameter != 0 {
                channel.offset_memory = parameter;
            }
            channel.offset_memory
        };
        let cell = self.channels[channel_index].row;
        // `kITOffsetWithInstrNumber`: an offset next to a lone instrument number applies to
        // the note the channel remembers.
        let note = if cell.note <= MAX_NOTE { cell.note } else if cell.instrument != INSTRUMENT_NONE { self.channels[channel_index].last_note } else { return };
        let offset = ((self.channels[channel_index].high_offset as u32) << 16) | ((value as u32) << 8);
        let sample = self.foreground_sample(channel_index);
        let end = self.sample_index(sample).map(|index| index.length_frames()).unwrap_or(0);
        let _ = note;
        if offset >= end && end != 0 {
            // Past the end: ignored, unless Old Effects makes it play from the end.
            if !self.old_effects {
                return;
            }
            self.channels[channel_index].sample_offset = end.saturating_sub(1);
        } else {
            self.channels[channel_index].sample_offset = offset;
        }
        // A note on the same row is triggered before the effect column runs, so the offset
        // has to reposition the voice that just started.
        if self.channels[channel_index].triggered
            && let Some(voice) = self.channels[channel_index].voice
            && let Some(slot) = context.voices.get_mut(voice)
        {
            slot.retrigger(self.channels[channel_index].sample_offset);
            self.channels[channel_index].sample_offset = 0;
        }
    }

    /// `Qxy`: `y` is the tick interval, `x` selects one of sixteen volume changes.
    fn init_retrigger(&mut self, context: &mut TickContext<'_>, channel_index: usize, parameter: u8) {
        if parameter != 0 {
            self.channels[channel_index].retrigger_memory = parameter;
        }
        if self.channels[channel_index].row.note != NOTE_NONE {
            self.channels[channel_index].retrigger_count = self.channels[channel_index].retrigger_memory & 0x0F;
        } else {
            self.run_retrigger(channel_index);
        }
        let _ = context;
    }

    fn run_retrigger(&mut self, channel_index: usize) {
        let memory = self.channels[channel_index].retrigger_memory;
        let channel = &mut self.channels[channel_index];
        channel.retrigger_count = channel.retrigger_count.wrapping_sub(1);
        if (channel.retrigger_count as i8) > 0 {
            return;
        }
        channel.retrigger_count = memory & 0x0F;
        // `kITShortSampleRetrig`: a sample that has already stopped is not retriggered.
        if !self.foreground_playing(channel_index) {
            return;
        }
        let volume = self.channels[channel_index].note_volume / 4;
        let updated = match memory >> 4 {
            1 => volume - 1,
            2 => volume - 2,
            3 => volume - 4,
            4 => volume - 8,
            5 => volume - 16,
            6 => (volume << 1) / 3,
            7 => volume >> 1,
            9 => volume + 1,
            0xA => volume + 2,
            0xB => volume + 4,
            0xC => volume + 8,
            0xD => volume + 16,
            0xE => (volume * 3) >> 1,
            0xF => volume << 1,
            _ => volume,
        }
        .clamp(0, 64);
        self.channels[channel_index].note_volume = updated * 4;
        self.channels[channel_index].sample_offset = 0;
        if let Some(state) = self.foreground_state_mut(channel_index) {
            state.note_volume = updated * 4;
        }
    }

    /// `Rxy`, the tremolo. Its depth is halved against vibrato's, and Old Effects does not
    /// invert it.
    fn init_tremolo(&mut self, channel_index: usize, parameter: u8) {
        let channel = &mut self.channels[channel_index];
        if parameter >> 4 != 0 {
            channel.tremolo_speed = (parameter >> 4) << 2;
        }
        if parameter & 0x0F != 0 {
            channel.tremolo_depth = (parameter & 0x0F) << 1;
        }
        if self.old_effects {
            return;
        }
        self.run_tremolo(channel_index);
    }

    fn run_tremolo(&mut self, channel_index: usize) {
        let (waveform, speed) = {
            let channel = &self.channels[channel_index];
            (channel.tremolo_waveform, channel.tremolo_speed)
        };
        let position = self.channels[channel_index].tremolo_position.wrapping_add(speed);
        self.channels[channel_index].tremolo_position = position;
        let sample = oscillator_sample(waveform, position, &mut self.random);
        self.channels[channel_index].last_tremolo = sample;
    }

    /// `Yxy`, the panbrello. Its table is read four times more slowly than vibrato's, and
    /// its random waveform is a sample-and-hold rather than a fresh value each tick.
    fn init_panbrello(&mut self, channel_index: usize, parameter: u8) {
        let channel = &mut self.channels[channel_index];
        if parameter >> 4 != 0 {
            channel.panbrello_speed = parameter >> 4;
        }
        if parameter & 0x0F != 0 {
            channel.panbrello_depth = (parameter & 0x0F) << 1;
        }
        self.run_panbrello(channel_index);
    }

    fn run_panbrello(&mut self, channel_index: usize) {
        if self.channels[channel_index].surround {
            return;
        }
        let (waveform, speed) = {
            let channel = &self.channels[channel_index];
            (channel.panbrello_waveform, channel.panbrello_speed)
        };
        let sample = if waveform >= 3 {
            let channel = &mut self.channels[channel_index];
            channel.panbrello_position = channel.panbrello_position.wrapping_sub(1);
            if (channel.panbrello_position as i8) <= 0 {
                channel.panbrello_position = speed;
                let value = (self.random.next_u32() & 127) as i32 - 64;
                self.channels[channel_index].panbrello_random = value;
                value
            } else {
                channel.panbrello_random
            }
        } else {
            let position = self.channels[channel_index].panbrello_position.wrapping_add(speed);
            self.channels[channel_index].panbrello_position = position;
            oscillator_sample(waveform, position, &mut self.random)
        };
        let depth = self.channels[channel_index].panbrello_depth as i32;
        self.channels[channel_index].panbrello_offset = ((sample * depth) * 4 + 128) >> 8;
    }

    /// `T0x` subtracts and `T1x` adds `x` BPM on every tick but the first, clamped to
    /// IT's own 32..255 range. The sequencer owns the tempo, so the slide is recorded here
    /// and reported through the tick's [`TickOutcome`].
    fn tempo_slide(&mut self, channel_index: usize, tempo_bpm: u16) -> u16 {
        let memory = self.channels[channel_index].tempo_memory;
        if memory >= 0x20 {
            return tempo_bpm;
        }
        let delta = match memory & 0xF0 {
            0 => -((memory & 0x0F) as i32),
            _ => (memory & 0x0F) as i32,
        };
        (tempo_bpm as i32 + delta).clamp(32, 255) as u16
    }

    /// `Zxx`, and the `SFx` that selects which parametered macro it runs.
    ///
    /// Only IT's two internal macros exist in a sample-only engine: `F0 F0 00 z` sets the
    /// filter cutoff and `F0 F0 01 z` the resonance. Everything else is parsed so message
    /// boundaries stay right and then reported rather than emitted.
    fn midi_macro(&mut self, context: &TickContext<'_>, channel_index: usize, parameter: u8) {
        let data = ItFormatData::from_header(self.module.header());
        let macro_bytes = match parameter >= 0x80 {
            true => data.and_then(|data| data.fixed_macro((parameter & 0x7F) as usize)),
            false => data.and_then(|data| data.parametered_macro(self.channels[channel_index].active_macro as usize)),
        };
        let channel = &self.channels[channel_index];
        let voice = channel.voice.and_then(|voice| self.voices.get(voice.index() as usize).filter(|state| state.owns(voice)));
        let substitutions = MacroSubstitutions {
            parameter,
            note: channel.last_note & 0x7F,
            channel: channel_index.min(127) as u8,
            offset: channel.offset_memory,
            reverse: channel.voice.and_then(|voice| context.voices.get(voice)).is_some_and(|voice| voice.is_reversed()) as u8,
            // `v`: `muldiv((volume + swing) · global, channel · instrument, 1 << 20) / 2`,
            // read live from the channel's own columns rather than from the voice.
            note_velocity: {
                let instrument_volume = voice.map(|state| state.instrument_volume).unwrap_or(MAX_CHANNEL_VOLUME);
                let volume_swing = voice.map(|state| state.volume_swing as i64).unwrap_or(0);
                let product = (channel.note_volume.max(0) as i64 + volume_swing) * (self.global_volume as i64 * 2) * channel.channel_volume as i64 * instrument_volume as i64;
                ((product >> 20) / 2).clamp(1, 127) as u8
            },
            computed_velocity: (channel.macro_real_volume >> 7).clamp(1, 127) as u8,
            note_pan: ((channel.pan as i32 + channel.panbrello_offset).clamp(0, 256) / 2).min(127) as u8,
            computed_pan: (channel.macro_real_pan.clamp(0, 255) / 2) as u8,
        };
        let (cutoff, resonance) = match macro_bytes {
            Some(bytes) => parse_filter_macro(bytes, substitutions),
            // The default configuration: `Z00`..`Z7F` is `F0F000z`, and `Z80`..`Z8F` is
            // `F0F001` with the parameter in steps of eight.
            None => match parameter {
                0x00..=0x7F => (Some(parameter), None),
                0x80..=0x8F => (None, Some((parameter & 0x0F) * 8)),
                _ => (None, None),
            },
        };
        if let Some(cutoff) = cutoff {
            self.channels[channel_index].cutoff = cutoff.min(0x7F);
            if let Some(state) = self.foreground_state_mut(channel_index) {
                state.cutoff = cutoff.min(0x7F);
            }
        }
        if let Some(resonance) = resonance {
            self.channels[channel_index].resonance = resonance.min(0x7F);
            if let Some(state) = self.foreground_state_mut(channel_index) {
                state.resonance = resonance.min(0x7F);
            }
        }
    }

    fn smooth_midi_macro(&mut self, context: &TickContext<'_>, channel_index: usize) {
        let parameter = {
            let channel = &mut self.channels[channel_index];
            channel.macro_value += channel.macro_slide;
            if (channel.macro_slide > 0 && channel.macro_value > channel.macro_target)
                || (channel.macro_slide < 0 && channel.macro_value < channel.macro_target)
            {
                channel.macro_value = channel.macro_target;
                channel.macro_slide = 0;
            }
            (channel.macro_value >> 16).clamp(0, 255) as u8
        };
        self.midi_macro(context, channel_index, parameter);
    }

    /// The whole `Sxy` family. One memory covers the entire byte, so `S00` recalls the
    /// last sub-command as well as its parameter.
    fn special(&mut self, context: &mut TickContext<'_>, channel_index: usize, parameter: u8, outcome: &mut TickOutcome, song_first_tick: bool) {
        let value = {
            let channel = &mut self.channels[channel_index];
            if parameter != 0 {
                channel.special_memory = parameter;
            }
            channel.special_memory
        };
        let subcommand = value >> 4;
        let argument = value & 0x0F;
        match subcommand {
            0x1 => self.channels[channel_index].glissando = argument != 0,
            // `S2x` set finetune was removed in Impulse Tracker 2 and does nothing.
            0x2 => {}
            0x3 => {
                if argument <= 3 {
                    self.channels[channel_index].vibrato_waveform = argument;
                }
            }
            0x4 => {
                if argument <= 3 {
                    self.channels[channel_index].tremolo_waveform = argument;
                }
            }
            0x5 => {
                if argument <= 3 {
                    self.channels[channel_index].panbrello_waveform = argument;
                    self.channels[channel_index].panbrello_position = 0;
                }
            }
            // `S6x` adds to the row's tick budget and is summed across the row's channels.
            0x6 => {
                if song_first_tick {
                    self.frame_delay = self.frame_delay.saturating_add(argument);
                }
            }
            0x7 => self.note_and_envelope_control(context, channel_index, argument),
            0x8 => self.set_pan(channel_index, (((argument as i32) << 4 | argument as i32) + 2) as i16 >> 2 << 2),
            0x9 => match argument {
                0x0 => {
                    self.channels[channel_index].surround = false;
                    if let Some(state) = self.foreground_state_mut(channel_index) {
                        state.surround = false;
                    }
                }
                0x1 => {
                    self.channels[channel_index].surround = true;
                    if let Some(state) = self.foreground_state_mut(channel_index) {
                        state.surround = true;
                    }
                }
                // OpenMPT's `kITReverseSample`: S9E resumes forward playback; S9F
                // reverses it and, on a fresh note or a non-looping sample at position
                // zero, starts at the final fractional position of the sample.
                0xE | 0xF => {
                    let reverse = argument == 0xF;
                    let triggered = self.channels[channel_index].triggered;
                    if let Some(voice) = self.channels[channel_index].voice
                        && let Some(slot) = context.voices.get_mut(voice)
                    {
                        if reverse && slot.position() == 0 && (triggered || slot.region().loop_span().is_none()) {
                            slot.set_position(((slot.region().length_frames() as u64) << 32).saturating_sub(1));
                        }
                        slot.set_reversed(reverse);
                    }
                }
                _ => {}
            },
            0xA => self.channels[channel_index].high_offset = argument,
            0xB => {
                if song_first_tick {
                    self.flow.pattern_loop(channel_index, context.position.row, argument);
                }
            }
            0xC => self.channels[channel_index].note_cut = argument.max(1),
            0xD => {}
            0xE => {
                // Only the first `SEx` on a row counts, `SE0` included.
                if song_first_tick && outcome.pattern_delay == 0 {
                    outcome.pattern_delay = argument;
                }
            }
            0xF => self.channels[channel_index].active_macro = argument,
            _ => {}
        }
    }

    /// `S70`..`S7C`: the past-note commands act on this channel's **background** voices;
    /// the New Note Action overrides and the envelope pauses act on the channel itself.
    fn note_and_envelope_control(&mut self, context: &mut TickContext<'_>, channel_index: usize, argument: u8) {
        match argument {
            0x0..=0x2 => {
                let mut targets: [Option<VoiceId>; VIRTUAL_CHANNELS] = [None; VIRTUAL_CHANNELS];
                let mut count = 0usize;
                for (voice, _) in context.voices.iter() {
                    let Some(state) = self.voices.get(voice.index() as usize) else { continue };
                    if state.owns(voice) && !state.foreground && state.root_channel as usize == channel_index && count < targets.len() {
                        targets[count] = Some(voice);
                        count += 1;
                    }
                }
                for voice in targets.iter().flatten().copied() {
                    match argument {
                        0x1 => self.key_off_voice(context, voice),
                        0x2 => {
                            if let Some(state) = self.voices.get_mut(voice.index() as usize) {
                                state.note_fade = true;
                            }
                        }
                        _ => {
                            if let Some(state) = self.voices.get_mut(voice.index() as usize) {
                                state.note_fade = true;
                                state.fadeout = 0;
                            }
                        }
                    }
                }
            }
            0x3 => self.set_new_note_action(channel_index, NewNoteAction::Cut),
            0x4 => self.set_new_note_action(channel_index, NewNoteAction::Continue),
            0x5 => self.set_new_note_action(channel_index, NewNoteAction::NoteOff),
            0x6 => self.set_new_note_action(channel_index, NewNoteAction::NoteFade),
            0x7..=0xC => {
                let enable = argument.is_multiple_of(2);
                if let Some(state) = self.foreground_state_mut(channel_index) {
                    match argument {
                        0x7 | 0x8 => state.volume_envelope.enabled = enable,
                        0x9 | 0xA => state.panning_envelope.enabled = enable,
                        _ => state.pitch_envelope.enabled = enable,
                    }
                }
            }
            _ => {}
        }
    }

    fn set_new_note_action(&mut self, channel_index: usize, action: NewNoteAction) {
        self.channels[channel_index].new_note_action = action;
        if let Some(state) = self.foreground_state_mut(channel_index) {
            state.new_note_action = action;
        }
    }
}

#[derive(Copy, Clone)]
struct MacroSubstitutions {
    parameter: u8,
    note: u8,
    channel: u8,
    offset: u8,
    reverse: u8,
    note_velocity: u8,
    computed_velocity: u8,
    note_pan: u8,
    computed_pan: u8,
}

struct MacroNibbleStream<'a> {
    bytes: &'a [u8],
    index: usize,
    buffered: Option<u8>,
    substitutions: MacroSubstitutions,
}

impl MacroNibbleStream<'_> {
    fn next_nibble(&mut self) -> Option<u8> {
        if let Some(nibble) = self.buffered.take() {
            return Some(nibble);
        }
        while let Some(&byte) = self.bytes.get(self.index) {
            self.index += 1;
            let value = match byte {
                b'0'..=b'9' => return Some(byte - b'0'),
                b'A'..=b'F' => return Some(byte - b'A' + 10),
                b'z' => self.substitutions.parameter,
                b'n' => self.substitutions.note,
                b'h' => self.substitutions.channel,
                b'o' => self.substitutions.offset,
                b'm' => self.substitutions.reverse,
                b'v' => self.substitutions.note_velocity,
                b'u' => self.substitutions.computed_velocity,
                b'x' => self.substitutions.note_pan,
                b'y' => self.substitutions.computed_pan,
                b'a' | b'b' | b'p' | b's' | b'c' => 0,
                0 => return None,
                _ => continue,
            };
            self.buffered = Some(value & 0x0F);
            return Some(value >> 4);
        }
        None
    }

    fn next_byte(&mut self) -> Option<u8> {
        let high = self.next_nibble()?;
        let low = self.next_nibble()?;
        Some(high << 4 | low)
    }
}

/// Parse every internal message in one macro. Real-time stop/status bytes reset the
/// filter, and the last internal cutoff or resonance write wins.
fn parse_filter_macro(bytes: &[u8], substitutions: MacroSubstitutions) -> (Option<u8>, Option<u8>) {
    let mut stream = MacroNibbleStream { bytes, index: 0, buffered: None, substitutions };
    let mut cutoff = None;
    let mut resonance = None;
    while let Some(first) = stream.next_byte() {
        if matches!(first, 0xFA | 0xFC | 0xFF) {
            cutoff = Some(0x7F);
            resonance = Some(0);
            continue;
        }
        if first != 0xF0 {
            continue;
        }
        let Some(second) = stream.next_byte() else { break };
        if !matches!(second, 0xF0 | 0xF1) {
            continue;
        }
        let (Some(command), Some(value)) = (stream.next_byte(), stream.next_byte()) else { break };
        if command >= 0x80 || value >= 0x80 {
            continue;
        }
        match command {
            0 => cutoff = Some(value),
            1 => resonance = Some(value),
            _ => {}
        }
    }
    (cutoff, resonance)
}

// ── the tick ─────────────────────────────────────────────────────────────────────────

impl ItProcessor {
    /// One tick of every channel, then one pass over every voice.
    fn run_tick(&mut self, context: &mut TickContext<'_>, outcome: &mut TickOutcome, row_start: bool) {
        let row_length = (self.song_speed as u16 + self.frame_delay as u16).max(1);
        let absolute_tick = context.row_clock.tick_in_row;
        let tick_in_repeat = absolute_tick % row_length;
        let song_first_tick = absolute_tick == 0;
        let _ = row_start;

        for channel_index in 0..self.channels.len() {
            {
                let channel = &mut self.channels[channel_index];
                channel.output_frequency_set = false;
                channel.output_volume = channel.note_volume;
                channel.triggered = false;
            }
            let start_tick = self.channels[channel_index].start_tick;
            // A plain note lands at the row's own first tick; a delayed one lands on its
            // delay tick in **every** pattern-delay repeat (`kRowDelayWithNoteDelay`).
            let trigger = match start_tick {
                0 => absolute_tick == 0,
                delay => tick_in_repeat == delay,
            };
            if trigger {
                self.trigger_channel(context, channel_index);
            }
            let channel_first_tick = tick_in_repeat == start_tick;
            if tick_in_repeat >= start_tick {
                if channel_first_tick {
                    self.static_effect(context, channel_index, outcome, song_first_tick);
                } else {
                    self.tick_effect(context, channel_index, tick_in_repeat, outcome);
                }
            }
            self.apply_tremolo_and_tremor(channel_index);
            self.run_note_cut(channel_index, tick_in_repeat);
            self.sync_foreground(channel_index);
        }

        outcome.speed = self.song_speed.saturating_add(self.frame_delay);
        outcome.jump = self.flow.jump();
        self.advance_voices(context);
        #[cfg(feature = "trace")]
        self.report_trace(context);
    }

    /// The tremolo and tremor deltas, which modify the tick's output volume without
    /// touching the channel's own.
    fn apply_tremolo_and_tremor(&mut self, channel_index: usize) {
        let channel = &self.channels[channel_index];
        let mut volume = channel.note_volume;
        if matches!(channel.row.command, COMMAND_TREMOLO) {
            volume += ((channel.last_tremolo * channel.tremolo_depth as i32) * 4 + 128) >> 8;
        }
        if matches!(channel.row.command, COMMAND_TREMOR) && !channel.tremor_on {
            volume = 0;
        }
        self.channels[channel_index].output_volume = volume.clamp(0, MAX_NOTE_VOLUME);
    }

    /// `SCx`, which counts down on the ticks after the channel's first.
    fn run_note_cut(&mut self, channel_index: usize, tick_in_repeat: u16) {
        if self.channels[channel_index].note_cut == 0 {
            return;
        }
        if tick_in_repeat as u32 == self.channels[channel_index].note_cut as u32 {
            self.channels[channel_index].note_cut = 0;
            self.cut_foreground(channel_index, true);
        }
    }

    /// Push the channel's live values into its foreground voice.
    fn sync_foreground(&mut self, channel_index: usize) {
        let channel = &self.channels[channel_index];
        let (volume, frequency, pan, surround, channel_volume, cutoff, resonance, panbrello) = (
            channel.output_volume,
            if channel.output_frequency_set { channel.output_frequency_hz } else { channel.frequency_hz },
            channel.pan,
            channel.surround,
            channel.channel_volume,
            channel.cutoff,
            channel.resonance,
            channel.panbrello_offset,
        );
        if let Some(state) = self.foreground_state_mut(channel_index) {
            state.note_volume = volume;
            state.frequency_hz = frequency;
            state.pan = (pan as i32 + panbrello).clamp(0, MAX_PAN) as i16;
            state.surround = surround;
            state.channel_volume = channel_volume;
            state.cutoff = cutoff;
            state.resonance = resonance;
        }
    }

    /// Advance every owned voice — foreground and background alike — and write what the
    /// mixer needs.
    fn advance_voices(&mut self, context: &mut TickContext<'_>) {
        let module = Arc::clone(&self.module);
        let instrument_mode = self.instrument_mode;
        let global_volume = self.global_volume;
        let sample_rate_hz = self.sample_rate_hz;
        for index in 0..self.voices.len() {
            let Some(voice) = self.voices[index].voice else { continue };
            if context.voices.get(voice).is_none() {
                self.voices[index].release();
                continue;
            }
            // `chn.triggerNote`: the note that started this voice landed on this very
            // tick, which is the only moment IT is allowed to *disengage* a filter.
            let trigger_note = {
                let state = &self.voices[index];
                state.foreground && self.channels.get(state.root_channel as usize).is_some_and(|channel| channel.triggered)
            };
            let state = &mut self.voices[index];
            let definition = match instrument_mode {
                true => module.instrument(InstrumentId(state.instrument.wrapping_sub(1))),
                false => None,
            };

            let mut volume = state.note_volume.clamp(0, MAX_NOTE_VOLUME) << 6;
            let mut pan = (state.pan as i32 + state.pan_swing as i32).clamp(0, MAX_PAN);
            let mut filter_modifier = 256i32;

            if let Some(definition) = definition {
                // IT increments the position before it evaluates, so an envelope disabled
                // on the very tick its note triggers is not processed at all.
                let key_off_previous = state.key_off_previous;
                let mut fade = false;
                if let Some(envelope) = definition.volume_envelope.as_ref() {
                    fade |= advance_envelope(&mut state.volume_envelope, envelope, key_off_previous);
                    let position = state.volume_envelope.position.saturating_sub(1);
                    if state.volume_envelope.position != 0 {
                        state.volume_envelope.value = envelope_value(envelope, position, 4, 0).clamp(0, MAX_NOTE_VOLUME);
                        volume = volume * state.volume_envelope.value / 256;
                    }
                    if fade {
                        state.note_fade = true;
                        if envelope.points.last().is_some_and(|point| point.value == 0) {
                            state.fadeout = 0;
                            volume = 0;
                        }
                    }
                }
                if let Some(envelope) = definition.panning_envelope.as_ref() {
                    advance_envelope(&mut state.panning_envelope, envelope, key_off_previous);
                    if state.panning_envelope.position != 0 {
                        let position = state.panning_envelope.position.saturating_sub(1);
                        let value = envelope_value(envelope, position, 1, 32).clamp(-32, 32);
                        state.panning_envelope.value = value;
                        // The envelope's reach is proportional to the distance to the
                        // nearer edge, so a hard-panned voice barely moves outward.
                        pan += if pan >= 128 { value * (MAX_PAN - pan) / 32 } else { value * pan / 32 };
                    }
                }
                if let Some(envelope) = definition.pitch_envelope.as_ref() {
                    advance_envelope(&mut state.pitch_envelope, envelope, key_off_previous);
                    if state.pitch_envelope.position != 0 {
                        let position = state.pitch_envelope.position.saturating_sub(1);
                        let value = envelope_value(envelope, position, 8, 32).clamp(-256, 256);
                        state.pitch_envelope.value = value;
                        if state.pitch_envelope_is_filter {
                            filter_modifier = value;
                        }
                    }
                }
                // The fadeout, once note-fade is set: ITTECH.TXT's `NFC` loses the raw
                // `FadeOut` from a count of 1024 every tick, so a fadeout of 10 is silent
                // after 103 ticks. OpenMPT stores `fadeout << 5` and subtracts twice that
                // from the same 65536 scale.
                if state.note_fade {
                    let fadeout = definition.fadeout as i32;
                    if fadeout != 0 {
                        state.fadeout = (state.fadeout - fadeout * FADEOUT_STEP_SCALE).max(0);
                    }
                    volume = (volume as i64 * state.fadeout as i64 / FADEOUT_FULL as i64) as i32;
                }
            } else if state.note_fade {
                state.fadeout = 0;
                volume = 0;
            }

            // The mixing chain: `Vol · VEV · NFC · CV · SV · IV · GV`.
            let instrument_volume = (state.instrument_volume as i32 + state.volume_swing as i32).clamp(0, MAX_CHANNEL_VOLUME as i32);
            let real_volume = muldiv(
                volume as i64 * (global_volume as i64 * 2),
                state.channel_volume as i64 * instrument_volume as i64,
                1 << 20,
            )
            .clamp(0, 1 << 14) as i32;
            state.real_volume = real_volume;

            // Pitch: the channel's frequency, the pitch envelope, then auto-vibrato.
            let mut frequency = state.frequency_hz;
            if !state.pitch_envelope_is_filter && state.pitch_envelope.position != 0 {
                let value = state.pitch_envelope.value;
                let magnitude = value.unsigned_abs().min(255) as u8;
                frequency = match value >= 0 {
                    true => apply_slide_ratio(frequency, linear_slide_up_q16(magnitude)),
                    false => apply_slide_ratio(frequency, linear_slide_down_q16(magnitude)),
                };
            }
            frequency = advance_auto_vibrato(state, module.sample(SampleId(state.sample.wrapping_sub(1))), frequency, &mut self.random);

            if state.surround {
                // Accuracy policy D66: StarPlayer has no rear bus, so surround is centre.
                pan = 128;
            }
            state.real_pan = pan.clamp(0, MAX_PAN) as i16;

            // The filter. `SetupChannelFilter` filters whenever the cutoff is not fully
            // open or the resonance is set; otherwise it returns `-1` and *leaves the
            // coefficients alone*, and only a note trigger on the same tick clears
            // `CHN_FILTER` and so silences the filter (OpenMPT `Snd_flt.cpp`
            // `SetupChannelFilter`, the `kITFilterBehaviour` branch; libxmp spells the
            // same rule as `cutoff < 0xfe || resonance > 0 || xc->filter.can_disable`,
            // `src/player.c:1330`). Test cases `filter-reset.it`, `filter-reset-carry.it`,
            // `filter-nna.it`.
            let computed_cutoff = (state.cutoff as i32 * (filter_modifier + 256) / 256).clamp(0, 255) as u8;
            let filter = if state.resonance == 0 && computed_cutoff >= 254 {
                match state.filter_active && !trigger_note {
                    // Still filtering with the coefficients it already has.
                    true => None,
                    false => {
                        state.filter_active = false;
                        Some(FilterParams::BYPASS)
                    }
                }
            } else {
                state.filter_active = true;
                Some(FilterParams::from_it_scaled(computed_cutoff, state.resonance))
            };

            // `SCx` zeroes the increment outright (OpenMPT `Snd_fx.cpp` `NoteCut` under
            // `kITSCxStopsSample`); the note itself stays on the channel.
            let step = match state.playing {
                true => Step::from_ratio(frequency as u64, sample_rate_hz.max(1) as u64),
                false => Step::ZERO,
            };
            let volume_unit = unit_from_ratio(real_volume as u32, 1 << 14);
            let pan_unit = pan_to_bipolar_position(state.real_pan);
            let written = state.written;
            let has_written = state.has_written;
            let held_filter = match has_written {
                true => written.filter,
                false => FilterParams::BYPASS,
            };
            state.written = VoiceParams { step, volume: volume_unit, pan: pan_unit, filter: filter.unwrap_or(held_filter), dirty: DirtyBits::empty() };
            state.has_written = true;
            // A silent voice — the fadeout reached zero, or `SCx` zeroed the increment.
            // Impulse Tracker frees the pool slot only for a *background* voice: libxmp
            // reclaims a zero-volume voice only when its channel index is past the
            // module's own tracks (`libxmp_virt_setvol`, `src/virtual.c:325`), and
            // OpenMPT's `NoteCut` leaves the note, the instrument and the sample on the
            // channel with `nFadeOutVol` at zero. A foreground channel therefore keeps
            // reporting its silent note until another note replaces it.
            let silent = state.note_fade && state.fadeout == 0 && real_volume == 0;
            if silent && !(state.foreground && state.cut_by_scx) {
                if state.foreground {
                    if let Some(slot) = context.voices.get_mut(voice) {
                        slot.stop();
                    }
                } else {
                    context.voices.release(voice);
                }
                let root = self.voices[index].root_channel as usize;
                if self.voices[index].foreground && self.channels.get(root).is_some_and(|channel| channel.voice == Some(voice)) {
                    self.channels[root].voice = None;
                }
                self.voices[index].release();
                continue;
            }
            // The channel's macro inputs, for the *next* tick's `u` and `y` letters. Only
            // a voice that survives the tick writes them, exactly as libxmp only reaches
            // `process_volume` for a channel whose voice is still active
            // (`src/player.c:1633` returns early otherwise), so a cut or faded-out note
            // leaves the last sounding values in place.
            if self.voices[index].foreground {
                let (real_volume, real_pan) = (self.voices[index].real_volume, self.voices[index].real_pan);
                let root = self.voices[index].root_channel as usize;
                if let Some(channel) = self.channels.get_mut(root) {
                    channel.macro_real_volume = real_volume;
                    channel.macro_real_pan = real_pan;
                }
            }
            if !has_written || written.step != step {
                context.write_voice_param(voice, VoiceParam::Step(step));
            }
            if !has_written || written.volume != volume_unit {
                context.write_voice_param(voice, VoiceParam::Volume(volume_unit));
            }
            if !has_written || written.pan != pan_unit {
                context.write_voice_param(voice, VoiceParam::Pan(pan_unit));
            }
            if let Some(filter) = filter
                && (!has_written || written.filter != filter)
            {
                context.write_voice_param(voice, VoiceParam::Filter(filter));
            }
            self.voices[index].key_off_previous = self.voices[index].key_off;
        }
    }

    /// Feed the diagnostic per-tick trace: one row per channel, and one per background
    /// voice.
    #[cfg(feature = "trace")]
    fn report_trace(&self, context: &mut TickContext<'_>) {
        for channel_index in 0..self.channels.len() {
            let state = self.channels[channel_index]
                .voice
                .and_then(|voice| self.voices.get(voice.index() as usize).filter(|state| state.owns(voice)));
            context.report_trace_channel(ChannelId(channel_index as u16), self.trace_state(state));
        }
        for index in 0..self.voices.len() {
            let state = &self.voices[index];
            let Some(voice) = state.voice else { continue };
            if state.foreground {
                continue;
            }
            context.report_trace_voice(voice, self.trace_state(Some(state)));
        }
    }

    #[cfg(feature = "trace")]
    fn trace_state(&self, state: Option<&ItVoiceState>) -> TraceChannelState {
        let Some(state) = state else { return TraceChannelState::default() };
        TraceChannelState {
            note: Some(state.note),
            instrument: state.instrument,
            sample: state.sample,
            volume: ((state.real_volume + 128) / 256).clamp(0, 64) as u16,
            // libxmp's own comparison axis. Its IT loader converts every sample's
            // `C5Speed` into a relative note plus a finetune
            // (`libxmp_c2spd_to_note`, `src/loaders/it_load.c:906`) and then mixes at the
            // fixed `m->c4rate`, so its period is `C4_PERIOD · 8363 / frequency` with the
            // sample's own rate already folded into the note — which is the whole period
            // its `info_period` column carries once the Q12 scale is removed.
            period: match state.frequency_hz {
                0 => 0,
                frequency => ((LIBXMP_PERIOD_NUMERATOR + frequency as u64 / 2) / frequency as u64).min(u32::MAX as u64) as u32,
            },
            pan: state.real_pan.clamp(0, 255) as u16,
        }
    }
}

/// Advance one envelope by a tick, answering whether a non-looping envelope has run past
/// its last node.
fn advance_envelope(state: &mut ItEnvelopeState, envelope: &Envelope, key_off_previous: bool) -> bool {
    if !state.enabled {
        // `S77`/`S79`/`S7B` pause the counter; the value already reached keeps applying.
        return false;
    }
    let (start, end, no_loop) = envelope_span(envelope, key_off_previous);
    let mut position = state.position;
    let end_reached = no_loop && position > end;
    if position >= end {
        position = start;
    }
    state.position = position.saturating_add(1);
    end_reached
}

/// Per-sample auto-vibrato: the sweep ramps the depth in, the rate advances the phase, and
/// the result is a frequency multiplier of `2^(delta/768)`.
fn advance_auto_vibrato(state: &mut ItVoiceState, sample: Option<&SampleIndex>, frequency: u32, random: &mut Xorshift32) -> u32 {
    let Some(sample) = sample else { return frequency };
    let vibrato = sample.auto_vibrato();
    if vibrato.rate == 0 {
        // `VibratoSweep0.it`: a sample whose IT `ViS` is zero has no auto-vibrato at all.
        return frequency;
    }
    let position = (state.auto_vibrato_position & 0xFF) as u8;
    let depth = (state.auto_vibrato_depth + vibrato.sweep as u32).min(vibrato.depth as u32 * 256);
    state.auto_vibrato_depth = depth;
    state.auto_vibrato_position = state.auto_vibrato_position.wrapping_add(vibrato.rate as u16);
    let waveform = match vibrato.waveform {
        starplayer_model::AutoVibratoWaveform::Sine => 0,
        starplayer_model::AutoVibratoWaveform::RampDown => 1,
        starplayer_model::AutoVibratoWaveform::Square => 2,
        starplayer_model::AutoVibratoWaveform::Random => 3,
        // IT has no ramp-up auto-vibrato; the model carries XM's fourth waveform and IT's
        // loader never produces it. Ramp down is the nearest shape.
        starplayer_model::AutoVibratoWaveform::RampUp => 1,
    };
    let sample_value = oscillator_sample(waveform, position, random);
    let delta = sample_value * (depth / 256) as i32 / 64;
    if delta == 0 {
        return frequency;
    }
    let magnitude = delta.unsigned_abs().min(255 * 4 + 3);
    let ratio = if magnitude < 16 {
        match delta > 0 {
            true => fine_linear_slide_up_q16(magnitude as u8),
            false => fine_linear_slide_down_q16(magnitude as u8),
        }
    } else {
        let coarse = (magnitude / 4).min(255) as u8;
        match delta > 0 {
            true => linear_slide_up_q16(coarse),
            false => linear_slide_down_q16(coarse),
        }
    };
    let value = apply_slide_ratio(frequency, ratio);
    value.max(1)
}

// ── the TrackerProcessor seam ────────────────────────────────────────────────────────

impl TrackerProcessor for ItProcessor {
    fn row(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) -> TickOutcome {
        context.report_global_volume(unit_from_ratio(self.global_volume as u32, MAX_GLOBAL_VOLUME as u32));
        let mut outcome = context.outcome();
        if self.last_order.is_some_and(|position| position != (row.order, row.pattern)) {
            self.flow.position_changed();
        }
        self.last_order = Some((row.order, row.pattern));
        self.flow.begin_row();
        self.frame_delay = 0;
        outcome.pattern_delay = 0;
        self.latch_row(context, row);
        self.run_tick(context, &mut outcome, true);
        outcome
    }

    /// Every `SEx` repeat is another first tick in IT, so the row's tick-zero effects run
    /// again — without re-latching its notes, which keep sounding from the first pass.
    fn row_repeat(&mut self, context: &mut TickContext<'_>, row: RowRef<'_>) -> TickOutcome {
        let _ = row;
        context.report_global_volume(unit_from_ratio(self.global_volume as u32, MAX_GLOBAL_VOLUME as u32));
        let mut outcome = context.outcome();
        self.flow.begin_row();
        self.run_tick(context, &mut outcome, true);
        outcome
    }

    fn tick(&mut self, context: &mut TickContext<'_>) -> TickOutcome {
        context.report_global_volume(unit_from_ratio(self.global_volume as u32, MAX_GLOBAL_VOLUME as u32));
        let mut outcome = context.outcome();
        self.run_tick(context, &mut outcome, false);
        outcome
    }

    /// Restore the state a fresh processor would have. Nothing here allocates: both boxed
    /// arrays keep their storage and every entry is overwritten in place.
    fn reset(&mut self) {
        let header = self.module.header();
        self.global_volume = ((header.global_volume.to_bits() as u32 * MAX_GLOBAL_VOLUME as u32 + 32_767) / 65_535) as u8;
        self.song_speed = header.initial_speed;
        self.frame_delay = 0;
        for channel_index in 0..self.channels.len() {
            let (pan, surround) = default_pan(&self.module, channel_index as u8);
            self.channels[channel_index] = ItChannel::new(channel_index as u8, pan, surround, default_channel_volume(&self.module, channel_index as u8));
        }
        for state in self.voices.iter_mut() {
            state.release();
        }
        self.flow.reset();
        self.last_order = None;
        self.last_moved_voice = None;
        self.random = Xorshift32::new(0x1D_F0_5A_C7);
    }

    /// IT sounds several voices per channel, so the pool is the format's own virtual
    /// channel count rather than the module's.
    fn recommended_voice_capacity(&self, _channel_count: usize) -> usize { VIRTUAL_CHANNELS }
}

/// How many voices an IT module wants in the pool, without building a processor.
pub const fn recommended_voice_capacity(_channel_count: usize) -> usize { VIRTUAL_CHANNELS }

/// Build the public IT sequencer with the module's own speed, tempo and pan state.
pub fn sequencer_for<Tempo: TempoModel>(module: Arc<Module>, sample_rate_hz: u32, tempo_model: Tempo) -> PatternSequencer<Tempo, ItProcessor, ItPatternData> {
    let settings = sequencer_settings(&module, sample_rate_hz);
    PatternSequencer::new(tempo_model, ItPatternData(Arc::clone(&module)), ItProcessor::new(module, sample_rate_hz), settings)
}

/// Build the public IT sequencer under an explicit [`QuirkSelection`].
pub fn sequencer_with_quirks(module: Arc<Module>, sample_rate_hz: u32, quirks: QuirkSelection) -> PatternSequencer<TempoModelId, ItProcessor, ItPatternData> {
    let resolved = quirks.resolve(module.header().dialect);
    let settings = sequencer_settings(&module, sample_rate_hz);
    let processor = ItProcessor::with_quirks(Arc::clone(&module), sample_rate_hz, QuirkSelection::Override(resolved));
    PatternSequencer::new(resolved.tempo_model, ItPatternData(module), processor, settings)
}

fn sequencer_settings(module: &Module, sample_rate_hz: u32) -> SequencerSettings {
    SequencerSettings {
        sample_rate_hz,
        first_tick_frame: Frame::ZERO,
        initial_speed: module.header().initial_speed,
        initial_tempo_bpm: module.header().initial_tempo,
        restart_order: 0,
        // An IT order list ends with its own `0xFF` marker, which the loader turns into
        // `OrderEntry::End`; the policy is what happens when the list simply runs out,
        // and IT wraps to the restart position exactly as MOD, MTM and S3M do.
        end_of_song: EndOfSongPolicy::Loop,
    }
}

/// The [`PatternData`] seam, re-exported so a host names one type per format crate.
pub use crate::pattern::ItPatternData;

// Keep the `PatternData`, `OrderEntry` and `ModelOrderEntry` imports honest: the seam is
// implemented in `pattern.rs`, and naming them here is what documents that.
const _: fn() = || {
    fn assert_pattern_data<T: PatternData>() {}
    assert_pattern_data::<ItPatternData>();
    let _ = OrderEntry::End;
    let _ = ModelOrderEntry::End;
};

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use starplayer_core::{Frame, RowClock, U0F16};
    use starplayer_engine::{ChannelTable, SongPosition};
    use starplayer_mixer::VoicePool;
    use starplayer_model::{InstrumentDef, ModuleBuilder, ModuleFormat, ModuleHeader, SampleSpec};

    const ROWS: u16 = 4;
    const CHANNELS: u8 = 2;

    /// A two-channel instrument-mode IT with one sample and one instrument, and a pattern
    /// of `cells` — one `ItCell` per channel per row.
    fn module_with(cells: &[ItCell], new_note_action: NewNoteAction) -> Arc<Module> {
        module_with_instrument(cells, new_note_action, |_| {})
    }

    /// [`module_with`], with `customise` applied to the instrument before it is added.
    fn module_with_instrument(cells: &[ItCell], new_note_action: NewNoteAction, customise: impl FnOnce(&mut InstrumentDef)) -> Arc<Module> {
        let mut builder = ModuleBuilder::new();
        let sample = builder.add_sample(&[0i16; 64], SampleSpec::one_shot("pcm").with_forward_loop(0, 64)).unwrap();
        let mut instrument = InstrumentDef::from_sample("ins", sample, U0F16::MAX);
        instrument.note_sample_map = [1u16; starplayer_model::NOTE_MAP_LENGTH];
        for (note, mapped) in instrument.note_transpose_map.iter_mut().enumerate() {
            *mapped = note as u8;
        }
        instrument.new_note_action = new_note_action;
        instrument.fadeout = 0;
        customise(&mut instrument);
        builder.add_instrument(instrument).unwrap();
        let mut bytes = alloc::vec::Vec::new();
        for cell in cells {
            bytes.extend_from_slice(&cell.to_bytes());
        }
        bytes.resize(ROWS as usize * CHANNELS as usize * CELL_BYTES, 0);
        for (index, byte) in bytes.iter_mut().enumerate() {
            // Any cell the caller did not supply is an empty one.
            if index >= cells.len() * CELL_BYTES {
                *byte = ItCell::EMPTY.to_bytes()[index % CELL_BYTES];
            }
        }
        builder.add_pattern(&bytes, ROWS, CHANNELS).unwrap();
        builder.set_orders(&[0, starplayer_model::ORDER_END]);
        let mut header = ModuleHeader::new(ModuleFormat::It, CHANNELS);
        // Instrument mode and linear slides, which is what every modern IT is.
        header.format_extra = ItFormatExtra { flags: 0x000C, special: 0, old_instruments: false, has_midi_configuration: false }.encode();
        header.flags.linear_slides = true;
        builder.set_header(header);
        Arc::new(builder.build().unwrap())
    }

    fn note_cell(note: u8, instrument: u8) -> ItCell { ItCell { note, instrument, ..ItCell::EMPTY } }

    /// Drive `rows` rows of the module through a processor, returning it with its pool.
    fn play(module: Arc<Module>, rows: u16, ticks_per_row: u8) -> (ItProcessor, VoicePool, ChannelTable) {
        let mut processor = ItProcessor::new(Arc::clone(&module), 44_100);
        let mut voices = VoicePool::new(VIRTUAL_CHANNELS);
        let mut channels = ChannelTable::new(CHANNELS as usize);
        let data = ItPatternData(Arc::clone(&module));
        let mut tick = 0u64;
        for row in 0..rows {
            let bytes = data.row_bytes(0, row).unwrap().to_vec();
            for tick_in_row in 0..ticks_per_row as u16 {
                let clock = RowClock { speed: ticks_per_row, pattern_delay: 0, tick_in_row, repeat_index: 0 };
                let position = SongPosition { order: 0, pattern: 0, row };
                let mut context = TickContext::new(Frame(tick * 882), &mut voices, &mut channels, clock, position, 125);
                let outcome = match tick_in_row {
                    0 => processor.row(&mut context, RowRef { order: 0, pattern: 0, row, bytes: &bytes }),
                    _ => processor.tick(&mut context),
                };
                let _ = outcome;
                tick += 1;
            }
        }
        (processor, voices, channels)
    }

    #[test]
    fn a_continue_new_note_action_leaves_the_old_voice_sounding_behind_its_channel() {
        let module = module_with(&[note_cell(60, 1), ItCell::EMPTY, note_cell(64, 1), ItCell::EMPTY], NewNoteAction::Continue);
        let (processor, voices, channels) = play(module, 2, 6);
        assert_eq!(voices.voices_active(), 2, "the second note kept the first one sounding");
        let foreground = channels.foreground(ChannelId(0)).expect("channel zero has a foreground voice");
        let background: alloc::vec::Vec<VoiceId> = voices.iter().map(|(voice, _)| voice).filter(|voice| *voice != foreground).collect();
        assert_eq!(background.len(), 1);
        let detached = processor.voice(background[0]).expect("the processor owns the detached voice");
        assert!(!detached.foreground, "the detached voice is nobody's foreground");
        assert_eq!(detached.root_channel, 0, "it keeps the channel that triggered it, for S7x and the duplicate check");
        assert_eq!(detached.note, 60, "the detached voice keeps its own note");
        assert_eq!(processor.voice(foreground).unwrap().note, 64);
    }

    /// A flat volume envelope that loops over its whole length and never sustains, so a
    /// key-off can only end the note through the fadeout.
    fn looping_volume_envelope() -> Envelope {
        Envelope {
            points: alloc::vec![
                starplayer_model::EnvelopePoint { tick: 0, value: 64 },
                starplayer_model::EnvelopePoint { tick: 10, value: 64 },
            ]
            .into_boxed_slice(),
            sustain: None,
            loop_span: Some(starplayer_model::EnvelopeSpan { start: 0, end: 1 }),
            carry: false,
        }
    }

    /// The ticks a fadeout of `fadeout` takes to reach silence: ITTECH.TXT's `NFC` counts
    /// down from 1024 by the raw value every tick.
    fn fadeout_ticks(fadeout: u16) -> u16 { 1024u16.div_ceil(fadeout) }

    /// The background voice a New Note Action of `Fade` detaches takes `1024 / FadeOut`
    /// ticks to reach silence and be released — not thirty-two times that, which is what
    /// left M4V-UNKN.IT's strings ringing across a dozen orders.
    #[test]
    fn a_fading_background_voice_is_released_after_1024_over_fadeout_ticks() {
        const FADEOUT: u16 = 10;
        const TICKS_PER_ROW: u8 = 60;
        let cells = [note_cell(60, 1), ItCell::EMPTY, note_cell(64, 1), ItCell::EMPTY];
        let module = module_with_instrument(&cells, NewNoteAction::NoteFade, |instrument| instrument.fadeout = FADEOUT);

        // Sixty ticks into the fade the voice is still there, and its counter says exactly
        // how far along it is.
        let (processor, voices, _) = play(Arc::clone(&module), 2, TICKS_PER_ROW);
        assert_eq!(voices.voices_active(), 2, "the faded note is still sounding sixty ticks in");
        let fading = voices.iter().map(|(voice, _)| voice).find(|voice| !processor.voice(*voice).unwrap().foreground).unwrap();
        assert_eq!(processor.voice(fading).unwrap().fadeout, FADEOUT_FULL - TICKS_PER_ROW as i32 * FADEOUT as i32 * FADEOUT_STEP_SCALE);

        // One more row is past the 103 ticks the fade takes, and the slot has been freed.
        assert!(fadeout_ticks(FADEOUT) < TICKS_PER_ROW as u16 * 2);
        let (processor, voices, _) = play(module, 3, TICKS_PER_ROW);
        assert_eq!(voices.voices_active(), 1, "the faded voice is released once its counter reaches zero");
        assert_eq!(processor.active_voices(), 1);
    }

    /// `===` on an instrument whose volume envelope loops (or which has none) starts the
    /// fadeout, exactly as `S71` does; the key-off flag alone would leave the note
    /// sounding forever at its looped envelope level.
    #[test]
    fn a_note_off_starts_the_fadeout_when_the_volume_envelope_cannot_end_the_note() {
        const FADEOUT: u16 = 10;
        const TICKS_PER_ROW: u8 = 60;
        let cells = [note_cell(60, 1), ItCell::EMPTY, note_cell(NOTE_OFF, 0), ItCell::EMPTY];
        for (label, customise) in [
            ("a looping volume envelope", (|instrument: &mut InstrumentDef| instrument.volume_envelope = Some(looping_volume_envelope())) as fn(&mut InstrumentDef)),
            ("no volume envelope", |instrument: &mut InstrumentDef| instrument.volume_envelope = None),
        ] {
            let module = module_with_instrument(&cells, NewNoteAction::Cut, |instrument| {
                instrument.fadeout = FADEOUT;
                customise(instrument);
            });
            let (processor, voices, _) = play(Arc::clone(&module), 2, TICKS_PER_ROW);
            assert_eq!(voices.voices_active(), 1, "{label}: the released note is still sounding sixty ticks in");
            let foreground = voices.iter().map(|(voice, _)| voice).next().unwrap();
            let state = processor.voice(foreground).unwrap();
            assert!(state.key_off && state.note_fade, "{label}: the note-off released the key and started the fade");
            // A silent foreground voice is stopped rather than released — the mixer frees
            // the slot when it honours the stop — and the processor forgets it at once.
            let (processor, voices, _) = play(module, 3, TICKS_PER_ROW);
            assert!(voices.get(foreground).is_some_and(|slot| slot.wants_stop()), "{label}: the faded note has been told to stop");
            assert_eq!(processor.active_voices(), 0, "{label}: the processor no longer tracks the faded voice");
            assert!(processor.channel(0).unwrap().voice.is_none(), "{label}: the channel no longer owns a voice");
        }
    }

    /// A New Note Action of `NoteOff` is a key-off on the detached voice, and so also
    /// starts the fadeout on a looping volume envelope.
    #[test]
    fn a_note_off_new_note_action_fades_the_detached_voice() {
        const FADEOUT: u16 = 10;
        const TICKS_PER_ROW: u8 = 60;
        let cells = [note_cell(60, 1), ItCell::EMPTY, note_cell(64, 1), ItCell::EMPTY];
        let module = module_with_instrument(&cells, NewNoteAction::NoteOff, |instrument| {
            instrument.fadeout = FADEOUT;
            instrument.volume_envelope = Some(looping_volume_envelope());
        });
        let (processor, voices, _) = play(Arc::clone(&module), 2, TICKS_PER_ROW);
        assert_eq!(voices.voices_active(), 2);
        let detached = voices.iter().map(|(voice, _)| voice).find(|voice| !processor.voice(*voice).unwrap().foreground).unwrap();
        assert!(processor.voice(detached).unwrap().note_fade, "the note-off action started the detached voice's fade");
        let (_, voices, _) = play(module, 3, TICKS_PER_ROW);
        assert_eq!(voices.voices_active(), 1, "only the new note is left once the detached voice has faded");
    }

    /// A New Note Action of `Cut` allocates nothing: `trigger_channel` replaces the old
    /// voice, exactly as libxmp frees a background voice whose volume reaches zero.
    #[test]
    fn a_cut_new_note_action_allocates_no_background_voice() {
        let module = module_with(&[note_cell(60, 1), ItCell::EMPTY, note_cell(64, 1), ItCell::EMPTY], NewNoteAction::Cut);
        let (_, voices, _) = play(module, 2, 6);
        assert_eq!(voices.voices_active(), 1, "a cut note never occupies a virtual channel");
    }

    /// Architecture Q3: a free background slot wins outright and takes the lowest index;
    /// otherwise the quietest background voice loses, with a looped sample counting half.
    #[test]
    fn voice_stealing_takes_the_quietest_background_voice() {
        let module = module_with(&[note_cell(60, 1)], NewNoteAction::Continue);
        let mut processor = ItProcessor::new(Arc::clone(&module), 44_100);
        let mut voices = VoicePool::new(4);
        let mut channels = ChannelTable::new(CHANNELS as usize);
        let region = SampleRegion::one_shot(0, 64);
        let tag = VoiceTag { channel: 0, instrument: 1, sample: 1, note: 60 };
        // Three background voices with descending volume, and a foreground one.
        for (index, volume) in [4096i32, 1024, 8192].into_iter().enumerate() {
            let voice = voices.allocate(tag, region, VoiceParams::SILENT, 0).expect("the pool has room");
            let entry = &mut processor.voices[voice.index() as usize];
            *entry = ItVoiceState { voice: Some(voice), foreground: false, real_volume: volume, note_volume: 0, fadeout: FADEOUT_FULL, ..ItVoiceState::default() };
            let _ = index;
        }
        let foreground = voices.allocate(tag, region, VoiceParams::SILENT, 0).expect("the pool has room");
        processor.voices[foreground.index() as usize] = ItVoiceState { voice: Some(foreground), foreground: true, real_volume: 1, fadeout: FADEOUT_FULL, ..ItVoiceState::default() };

        let context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(6), SongPosition::default(), 125);
        let victim = processor.choose_victim(&context).expect("a full pool still yields a victim");
        assert_eq!(processor.voice(victim).unwrap().real_volume, 1024, "the quietest background voice is stolen");
        assert!(!processor.voice(victim).unwrap().foreground, "a foreground voice is never a candidate, however quiet");
    }

    /// `kITEnvelopePositionHandling`: IT increments the position **before** it evaluates,
    /// so the first tick reads node zero and a paused envelope holds its value.
    #[test]
    fn the_envelope_position_is_incremented_before_it_is_evaluated() {
        let envelope = Envelope {
            points: alloc::vec![
                starplayer_model::EnvelopePoint { tick: 0, value: 64 },
                starplayer_model::EnvelopePoint { tick: 4, value: 0 },
            ]
            .into_boxed_slice(),
            sustain: None,
            loop_span: None,
            carry: false,
        };
        let mut state = ItEnvelopeState { position: 0, enabled: true, value: 0 };
        assert!(!advance_envelope(&mut state, &envelope, false));
        assert_eq!(state.position, 1, "the stored position is one-based");
        assert_eq!(envelope_value(&envelope, state.position - 1, 1, 32), 64, "the first tick evaluates node zero");
        advance_envelope(&mut state, &envelope, false);
        assert_eq!(envelope_value(&envelope, state.position - 1, 1, 32), 48, "the second tick is one envelope tick in");

        // `S77` pauses the counter rather than stopping the envelope.
        state.enabled = false;
        let paused = state.position;
        advance_envelope(&mut state, &envelope, false);
        assert_eq!(state.position, paused, "a paused envelope does not advance");
    }

    /// ITTECH.TXT's order of testing — `Dx0`, `D0x`, `DxF`, `DFx` — is what resolves the
    /// `D0F` / `DF0` ambiguity.
    #[test]
    fn the_volume_slide_classification_is_ittechs_order_of_testing() {
        let module = module_with(&[note_cell(60, 1)], NewNoteAction::Cut);
        let mut processor = ItProcessor::new(module, 44_100);
        let mut check = |parameter: u8, first_tick: bool, from: i32| -> i32 {
            processor.channels[0].note_volume = from;
            processor.channels[0].volume_slide_memory = 0;
            processor.volume_slide(0, parameter, first_tick);
            processor.channels[0].note_volume
        };
        assert_eq!(check(0xF0, true, 128), 188, "DF0 is a slide up by 15, applied on the first tick as well");
        assert_eq!(check(0xF0, false, 128), 188, "DF0 slides up on every tick");
        assert_eq!(check(0x0F, true, 128), 68, "D0F is a slide down by 15, applied on the first tick as well");
        assert_eq!(check(0x2F, true, 128), 136, "DxF is a fine slide up");
        assert_eq!(check(0x2F, false, 128), 128, "a fine slide is the first tick only");
        assert_eq!(check(0xF2, true, 128), 120, "DFx is a fine slide down");
        assert_eq!(check(0x34, false, 128), 128, "both nibbles set is ignored in IT");
        assert_eq!(check(0x04, false, 128), 112, "D0x slides down by x on every tick but the first");
        assert_eq!(check(0x04, true, 128), 128);
        assert_eq!(check(0x40, false, 128), 144, "Dx0 slides up by x on every tick but the first");
    }

    /// IT's square auto-vibrato and oscillator waveform is **unipolar** — 0 to +64, not
    /// ±64 — and its ramp runs down.
    #[test]
    fn the_oscillator_waveforms_are_its_own() {
        let mut random = Xorshift32::new(1);
        assert_eq!(oscillator_sample(0, 0, &mut random), 0);
        assert_eq!(oscillator_sample(0, 64, &mut random), 64, "the sine table peaks at +64 a quarter of the way through");
        assert_eq!(oscillator_sample(1, 0, &mut random), 64, "the ramp runs down from +64");
        assert_eq!(oscillator_sample(1, 255, &mut random), -64);
        assert_eq!(oscillator_sample(2, 0, &mut random), 64, "IT's square wave is unipolar");
        assert_eq!(oscillator_sample(2, 128, &mut random), 0);
        for position in 0..=255u8 {
            let value = oscillator_sample(3, position, &mut random);
            assert!((-64..=63).contains(&value), "the random waveform stays in IT's range");
        }
    }

    /// The pitch domain: a note sounds at its sample's `C5Speed` at `C-5`, and one octave
    /// up is exactly twice the frequency.
    #[test]
    fn a_note_sounds_at_the_samples_reference_rate_at_c5() {
        assert_eq!(frequency_from_note(8_363, REFERENCE_NOTE), 8_363);
        assert_eq!(frequency_from_note(8_363, REFERENCE_NOTE + 12), 16_726);
        assert_eq!(frequency_from_note(8_363, REFERENCE_NOTE - 12), 4_182, "rounded to the nearest whole hertz");
        assert_eq!(frequency_from_note(44_100, REFERENCE_NOTE), 44_100);
    }

    /// The default MIDI macro set: `Z00`..`Z7F` is the cutoff and `Z80`..`Z8F` the
    /// resonance in sixteen steps of eight.
    #[test]
    fn the_default_midi_macros_set_the_cutoff_and_the_resonance() {
        let values = |parameter| MacroSubstitutions {
            parameter,
            note: 60,
            channel: 1,
            offset: 2,
            reverse: 0,
            note_velocity: 127,
            computed_velocity: 64,
            note_pan: 32,
            computed_pan: 96,
        };
        assert_eq!(parse_filter_macro(b"F0F000z", values(0x40)), (Some(0x40), None));
        assert_eq!(parse_filter_macro(b"F0F001z", values(0x20)), (None, Some(0x20)));
        assert_eq!(parse_filter_macro(b"F0F00120", values(0x7F)), (None, Some(0x20)), "a fixed macro spells its own value");
        assert_eq!(parse_filter_macro(b"F0F002z", values(0x10)), (None, None), "the filter-mode macro is a ModPlug extension");
        assert_eq!(parse_filter_macro(b"9c n v", values(0x10)), (None, None), "a real MIDI message reaches no filter");
        assert_eq!(parse_filter_macro(b"F0F000v F0F001u", values(0x10)), (Some(127), Some(64)), "one macro may contain multiple internal messages");
        assert_eq!(parse_filter_macro(b"FA", values(0x10)), (Some(127), Some(0)), "MIDI start resets both filter parameters");
    }

    /// IT still recommends 256 owned voices, while a persistent jam host carries sixteen
    /// extra global slots and enough parallel state to track IT voices at high IDs.
    #[test]
    fn the_recommended_capacity_and_wider_parallel_state_have_distinct_limits() {
        let module = module_with(&[note_cell(60, 1)], NewNoteAction::Cut);
        let processor = ItProcessor::new(module, 44_100);
        assert_eq!(processor.recommended_voice_capacity(CHANNELS as usize), VIRTUAL_CHANNELS);
        assert_eq!(recommended_voice_capacity(CHANNELS as usize), VIRTUAL_CHANNELS);
        assert_eq!(processor.voices.len(), MAX_VOICE_CAPACITY);
        assert!(processor.voice(VoiceId::new(MAX_VOICE_CAPACITY as u16, 1)).is_none(), "a voice id past the array is skipped");
    }

    #[test]
    fn midi_low_slots_do_not_push_it_past_its_state_or_let_it_consume_the_reserve() {
        let module = module_with(&[note_cell(60, 1)], NewNoteAction::Continue);
        let mut processor = ItProcessor::new(module, 44_100);
        let mut voices = VoicePool::new(MAX_VOICE_CAPACITY);
        let mut channels = ChannelTable::new(ChannelTable::MAX_CHANNELS);
        let region = SampleRegion::one_shot(0, 64);
        let midi_tag = VoiceTag { channel: 48, instrument: 1, sample: 1, note: 60 };
        for _ in 0..16 {
            voices.allocate(midi_tag, region, VoiceParams::SILENT, 0).expect("the MIDI reserve has room");
        }
        let tracker_tag = VoiceTag { channel: 0, instrument: 1, sample: 1, note: 60 };
        let mut highest = None;
        for index in 0..VIRTUAL_CHANNELS {
            let voice = voices.allocate(tracker_tag, region, VoiceParams::SILENT, 0).expect("all IT virtual channels fit");
            processor.voices[voice.index() as usize] = ItVoiceState {
                voice: Some(voice),
                foreground: false,
                real_volume: 1_024,
                note_volume: 64,
                fadeout: if index + 1 == VIRTUAL_CHANNELS { 0 } else { FADEOUT_FULL },
                playing: true,
                ..ItVoiceState::default()
            };
            highest = Some(voice);
        }
        let highest = highest.expect("there is a highest IT voice");
        assert_eq!(highest.index() as usize, MAX_VOICE_CAPACITY - 1, "sixteen low MIDI IDs push the last IT state to slot 271");

        let first_midi = voices.iter().next().map(|(voice, _)| voice).expect("a MIDI voice exists");
        voices.release(first_midi);
        assert_eq!(voices.voices_active(), MAX_VOICE_CAPACITY - 1, "one global reserve slot is free");
        let allocated = {
            let mut context = TickContext::new(Frame::ZERO, &mut voices, &mut channels, RowClock::new(6), SongPosition::default(), 125);
            processor
                .allocate_voice(&mut context, ChannelId(0), tracker_tag, region, VoiceParams::SILENT, 0)
                .expect("IT steals at its own limit")
        };
        assert_eq!(allocated.index(), highest.index(), "the fully faded high-ID IT background slot is reused first");
        assert_ne!(allocated.generation(), highest.generation(), "the reused slot has a fresh generation");
        assert_eq!(voices.voices_active(), MAX_VOICE_CAPACITY - 1, "quota replacement keeps the pool at 271 before the caller records the new IT state");
        processor.voices[allocated.index() as usize] = ItVoiceState {
            voice: Some(allocated),
            foreground: false,
            fadeout: FADEOUT_FULL,
            playing: true,
            ..ItVoiceState::default()
        };
        let owned_voice_count = voices
            .iter()
            .filter(|(voice, _)| processor.voices.get(voice.index() as usize).is_some_and(|state| state.owns(*voice)))
            .count();
        assert_eq!(owned_voice_count, VIRTUAL_CHANNELS, "the replacement restores exactly 256 IT-owned voices");
        let replacement_midi = voices.allocate(midi_tag, region, VoiceParams::SILENT, 0).expect("the freed MIDI reserve slot remains available");
        assert_eq!(replacement_midi.index(), first_midi.index());
        assert_eq!(voices.voices_active(), MAX_VOICE_CAPACITY, "sixteen MIDI and 256 IT voices fit together");
        for (voice, state) in voices.iter().take(16) {
            assert_eq!(state.tag.channel, midi_tag.channel, "low slot {} remains MIDI-owned", voice.index());
        }
    }

    /// A wider pool than the processor's array is legal, and so is a narrower one.
    #[test]
    fn a_pool_narrower_than_the_virtual_channel_count_simply_sounds_fewer_voices() {
        let module = module_with(&[note_cell(60, 1), ItCell::EMPTY, note_cell(64, 1), ItCell::EMPTY], NewNoteAction::Continue);
        let mut processor = ItProcessor::new(Arc::clone(&module), 44_100);
        let mut voices = VoicePool::new(1);
        let mut channels = ChannelTable::new(CHANNELS as usize);
        let data = ItPatternData(Arc::clone(&module));
        for row in 0..2u16 {
            let bytes = data.row_bytes(0, row).unwrap().to_vec();
            let clock = RowClock::new(6);
            let position = SongPosition { order: 0, pattern: 0, row };
            let mut context = TickContext::new(Frame(row as u64 * 882), &mut voices, &mut channels, clock, position, 125);
            let _ = processor.row(&mut context, RowRef { order: 0, pattern: 0, row, bytes: &bytes });
        }
        assert_eq!(voices.voices_active(), 1, "one slot holds one voice; the New Note Action's is stolen back");
    }
}
