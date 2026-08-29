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
use starplayer_core::{SampleId, U0F16};

/// Notes an instrument's note→sample map covers: 10 octaves, C-0 to B-9, matching IT.
pub const NOTE_MAP_LENGTH: usize = 120;

/// One breakpoint of an [`Envelope`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct EnvelopePoint {
    /// Position in control ticks from the start of the envelope.
    pub tick: u16,
    /// Envelope value at that tick, in the envelope's own units (XM 0..64, IT 0..64).
    pub value: u16,
}

/// A point-index span of an envelope: `start ..= end`, inclusive, as both XM and IT
/// spell their sustain and loop points.
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
    /// Sample played for each of the 120 notes, one-based, `0` meaning "no sample".
    /// **M5/M6** — M1 formats leave this all zero and use
    /// [`sample`](InstrumentDef::sample).
    pub note_sample_map: [u8; NOTE_MAP_LENGTH],
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
}

impl Default for InstrumentDef {
    fn default() -> InstrumentDef {
        InstrumentDef {
            name: String::new().into_boxed_str(),
            sample: None,
            default_volume: U0F16::MAX,
            note_sample_map: [0; NOTE_MAP_LENGTH],
            volume_envelope: None,
            panning_envelope: None,
            pitch_envelope: None,
            fadeout: 0,
            new_note_action: NewNoteAction::Cut,
            duplicate_check: DuplicateCheck::Off,
            duplicate_action: DuplicateAction::Cut,
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
