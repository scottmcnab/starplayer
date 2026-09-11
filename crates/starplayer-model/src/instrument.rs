//! [`InstrumentDef`] and the envelope / new-note types it declares for XM and IT.
//!
//! # Declared, not implemented
//!
//! S3M has no instruments in the XM/IT sense: one "instrument" is one sample plus a
//! default volume, which is all the M1 engine reads. Everything else here —
//! [`Envelope`], [`InstrumentDef::note_sample_map`], the NNA triple and
//! [`InstrumentDef::fadeout`] — is **declared now and unused until M5 (XM) and M6 (IT)**,
//! so that adding those formats is a matter of filling fields in rather than reshaping
//! the model. Task B1 deliberately declares the data and invents no behaviour: an
//! envelope here is a list of points, not a player.

use alloc::boxed::Box;
use alloc::string::String;
use starplayer_core::{I1F15, SampleId, U0F16};

/// Notes an instrument's note→sample map covers: 10 octaves, C-0 to B-9, matching IT.
pub const NOTE_MAP_LENGTH: usize = 120;

/// One breakpoint of an [`Envelope`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct EnvelopePoint {
    /// Position in control ticks from the start of the envelope.
    pub tick: u16,
    /// Envelope value at that tick, in the envelope's own units: XM's volume envelope and
    /// IT's volume envelope are `0..64`; IT's panning and pitch/filter envelopes are
    /// signed, `-32..32`. Signed so one field serves every envelope kind without a second,
    /// format-specific representation.
    pub value: i16,
}

/// A point-index span of an envelope: `start ..= end`, inclusive, as both XM and IT
/// spell their sustain and loop points.
///
/// XM has a single sustain **point** rather than IT's sustain span; a loaded XM instrument
/// represents it as a span with `start == end`, so `Envelope` needs no separate field for
/// it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct EnvelopeSpan {
    /// Index of the first point of the span.
    pub start: u8,
    /// Index of the last point of the span.
    pub end: u8,
}

/// A volume, panning or pitch envelope. **M5/M6 — declared, never evaluated in M1.**
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Envelope {
    /// The breakpoints, in ascending tick order.
    pub points: Box<[EnvelopePoint]>,
    /// The sustain span, held while the note is on.
    pub sustain: Option<EnvelopeSpan>,
    /// The loop span, repeated for as long as the envelope runs.
    pub loop_span: Option<EnvelopeSpan>,
    /// IT's "carry": a new note resumes the envelope where the previous one left off
    /// rather than restarting it.
    pub carry: bool,
}

/// What happens to the voice already sounding on a channel when a new note arrives.
/// **M6 — declared, never acted on in M1.**
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum NewNoteAction {
    /// Stop the old voice immediately. MOD, S3M and XM behaviour, and IT's default.
    #[default]
    Cut,
    /// Let the old voice keep playing, unowned by the channel.
    Continue,
    /// Release the old voice: envelopes move to their release stage.
    NoteOff,
    /// Fade the old voice out at [`InstrumentDef::fadeout`].
    NoteFade,
}

/// Which property IT compares to decide two voices are duplicates. **M6.**
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum DuplicateCheck {
    /// Never treat a voice as a duplicate.
    #[default]
    Off,
    /// Same note.
    Note,
    /// Same sample.
    Sample,
    /// Same instrument.
    Instrument,
}

/// What IT does to a voice that [`DuplicateCheck`] found. **M6.**
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum DuplicateAction {
    /// Stop it immediately.
    #[default]
    Cut,
    /// Release it.
    NoteOff,
    /// Fade it out.
    NoteFade,
}

/// An instrument: what a note number means for the channel that plays it.
///
/// For S3M — the only format M1 loads — [`sample`](InstrumentDef::sample) and
/// [`default_volume`](InstrumentDef::default_volume) are the whole story, and one
/// `InstrumentDef` is created per sample in the file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InstrumentDef {
    /// The instrument's name as the file spells it.
    pub name: Box<str>,
    /// The single sample this instrument plays, for the formats that have one.
    ///
    /// `None` means the instrument sounds nothing on its own — an XM/IT instrument that
    /// selects its sample through [`note_sample_map`](InstrumentDef::note_sample_map), or
    /// an empty slot the file declared but never filled.
    pub sample: Option<SampleId>,
    /// Instrument volume, applied on top of the sample's own.
    pub default_volume: U0F16,
    /// Sample for each of the 120 notes: a one-based **global** [`SampleId`], `0` meaning
    /// "no sample". `u16` because XM allows 128 instruments × 16 samples each, 2048 in
    /// total, which does not fit a `u8`. **M5/M6** — M1 formats leave this all zero and use
    /// [`sample`](InstrumentDef::sample).
    pub note_sample_map: [u16; NOTE_MAP_LENGTH],
    /// Note actually played for each of the 120 notes (IT's note transposition). The
    /// identity map for every other format, so a format that never transposes reads this
    /// exactly like "play the note you were given". **M6.**
    pub note_transpose_map: [u8; NOTE_MAP_LENGTH],
    /// Volume envelope. **M5/M6.**
    pub volume_envelope: Option<Envelope>,
    /// Panning envelope. **M5/M6.**
    pub panning_envelope: Option<Envelope>,
    /// Pitch/filter envelope. **M6.**
    pub pitch_envelope: Option<Envelope>,
    /// Fadeout rate applied after a note-off, in the source format's own units.
    /// **M5/M6.**
    pub fadeout: u16,
    /// What a new note does to the voice already sounding. **M6.**
    pub new_note_action: NewNoteAction,
    /// Which property marks a voice as a duplicate. **M6.**
    pub duplicate_check: DuplicateCheck,
    /// What happens to a duplicate voice. **M6.**
    pub duplicate_action: DuplicateAction,
    /// IT's instrument-level global volume, scaling every sample the instrument plays.
    /// `U0F16::MAX` — unity — for every other format. **M6.**
    pub global_volume: U0F16,
    /// IT's instrument default pan, or `None` when the file's "enabled" bit is off.
    /// **M6.**
    pub default_pan: Option<I1F15>,
    /// IT's pitch/pan separation: how far a note away from
    /// [`pitch_pan_centre`](InstrumentDef::pitch_pan_centre) pushes the pan, `-32..32`.
    /// **M6.**
    pub pitch_pan_separation: i8,
    /// IT's pitch/pan centre note, the note at which
    /// [`pitch_pan_separation`](InstrumentDef::pitch_pan_separation) contributes nothing.
    /// **M6.**
    pub pitch_pan_centre: u8,
    /// IT's random volume variation, `0..100` percent. **M6.**
    pub random_volume_variation: u8,
    /// IT's random pan variation, `0..64`. **M6.**
    pub random_pan_variation: u8,
    /// IT's initial filter cutoff, `0..127`, or `None` when the file's "enabled" bit is
    /// off. **M6.**
    pub initial_filter_cutoff: Option<u8>,
    /// IT's initial filter resonance, `0..127`, or `None` when the file's "enabled" bit is
    /// off. **M6.**
    pub initial_filter_resonance: Option<u8>,
    /// IT's flag making [`pitch_envelope`](InstrumentDef::pitch_envelope) act as a filter
    /// envelope instead of a pitch envelope. **M6.**
    pub pitch_envelope_is_filter: bool,
}

/// The identity note-transpose map: note `n` plays note `n`, for every format that never
/// transposes.
///
/// Crate-visible because the module image encodes it as a single byte rather than 120
/// (`crate::image`): every MOD, S3M and MTM instrument carries exactly this map, and on a
/// flash-resident module those bytes are real.
pub(crate) fn identity_transpose_map() -> [u8; NOTE_MAP_LENGTH] { core::array::from_fn(|note| note as u8) }

impl Default for InstrumentDef {
    fn default() -> InstrumentDef {
        InstrumentDef {
            name: String::new().into_boxed_str(),
            sample: None,
            default_volume: U0F16::MAX,
            note_sample_map: [0; NOTE_MAP_LENGTH],
            note_transpose_map: identity_transpose_map(),
            volume_envelope: None,
            panning_envelope: None,
            pitch_envelope: None,
            fadeout: 0,
            new_note_action: NewNoteAction::Cut,
            duplicate_check: DuplicateCheck::Off,
            duplicate_action: DuplicateAction::Cut,
            global_volume: U0F16::MAX,
            default_pan: None,
            pitch_pan_separation: 0,
            pitch_pan_centre: 0,
            random_volume_variation: 0,
            random_pan_variation: 0,
            initial_filter_cutoff: None,
            initial_filter_resonance: None,
            pitch_envelope_is_filter: false,
        }
    }
}

impl InstrumentDef {
    /// The one-sample instrument S3M, MOD and MTM all describe: a name, a sample and a
    /// volume, with every XM/IT field left at its default.
    pub fn from_sample(name: &str, sample: SampleId, default_volume: U0F16) -> InstrumentDef {
        InstrumentDef {
            name: String::from(name).into_boxed_str(),
            sample: Some(sample),
            default_volume,
            ..InstrumentDef::default()
        }
    }
}
