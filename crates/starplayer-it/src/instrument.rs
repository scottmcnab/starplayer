//! The 554-byte `IMPI` instrument header — both the IT 2.xx layout and the pre-2.00 one —
//! and its conversion into the model's [`InstrumentDef`].

use alloc::string::String;
use alloc::vec::Vec;
use starplayer_core::fixed::unit_from_ratio;
use starplayer_core::{Error, U0F16};
use starplayer_model::{
    DuplicateAction, DuplicateCheck, Envelope, EnvelopePoint, EnvelopeSpan, InstrumentDef,
    NOTE_MAP_LENGTH, NewNoteAction,
};

use crate::header::pan_to_bipolar;

/// Bytes in one instrument header, in either layout.
///
/// ITTECH.TXT: "Total length of an instrument is 547 bytes, but 554 bytes are written,
/// just to simplify the loading of the old format."
pub const HEADER_LENGTH: usize = 554;

/// The signature at offset `0x00` of an instrument header.
pub const MAGIC: [u8; 4] = *b"IMPI";

/// Offset of the 240-byte note/sample keyboard table, in both layouts.
pub const KEYBOARD_OFFSET: usize = 0x40;

/// Bytes in one envelope structure: six header bytes, 25 three-byte nodes, one reserved.
pub const ENVELOPE_LENGTH: usize = 82;

/// Nodes an IT envelope may have.
pub const MAX_ENVELOPE_NODES: usize = 25;

/// Offset of the volume envelope in the IT 2.xx layout.
pub const VOLUME_ENVELOPE_OFFSET: usize = 0x130;
/// Offset of the panning envelope in the IT 2.xx layout.
pub const PANNING_ENVELOPE_OFFSET: usize = 0x182;
/// Offset of the pitch/filter envelope in the IT 2.xx layout.
pub const PITCH_ENVELOPE_OFFSET: usize = 0x1D4;

/// Offset of the pre-2.00 layout's 25 two-byte `(tick, value)` volume-envelope nodes.
///
/// The 200 bytes at `0x130` that precede them are a pre-interpolated copy of the same
/// envelope; OpenMPT ignores it and reads these nodes, and so does this loader.
pub const OLD_ENVELOPE_NODES_OFFSET: usize = 0x1F8;

/// Envelope flag bit 0 — the envelope is on.
pub const ENVELOPE_ENABLED: u8 = 1 << 0;
/// Envelope flag bit 1 — the loop is on.
pub const ENVELOPE_LOOP: u8 = 1 << 1;
/// Envelope flag bit 2 — the sustain loop is on.
pub const ENVELOPE_SUSTAIN: u8 = 1 << 2;
/// Envelope flag bit 3 — carry: a new note resumes the envelope rather than restarting it.
pub const ENVELOPE_CARRY: u8 = 1 << 3;
/// Envelope flag bit 7, on the **pitch** envelope only — it is a filter envelope instead.
pub const ENVELOPE_FILTER: u8 = 1 << 7;

/// Instrument `DfP` bit 7 — *ignore* the default pan. The opposite sense to a sample's.
pub const PAN_IGNORED: u8 = 0x80;

/// Instrument `IFC` / `IFR` bit 7 — the filter cutoff / resonance is enabled.
pub const FILTER_ENABLED: u8 = 0x80;

/// Highest `GbV` the format defines. `ITInstrument::ConvertToMPT` halves it into a 0..64
/// internal volume, which is where the clamp comes from.
pub const MAX_GLOBAL_VOLUME: u8 = 128;

/// Multiplier that puts a pre-2.00 `FadeOut` on the IT 2.xx scale.
///
/// ITTECH.TXT: the old format's fadeout ranges 0..64 against a count of 512, the new one's
/// 0..128 against a count of 1024, so the same raw value fades twice as fast in the old
/// layout. OpenMPT expresses the same ratio as `fadeout << 6` against `fadeout << 5`.
pub const OLD_FADEOUT_SCALE: u16 = 2;

/// One decoded IT envelope, before it becomes the model's [`Envelope`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItEnvelope {
    /// The raw flag byte.
    pub flags: u8,
    /// Node count, already clamped to [`MAX_ENVELOPE_NODES`].
    pub node_count: u8,
    /// Loop start node index.
    pub loop_start: u8,
    /// Loop end node index.
    pub loop_end: u8,
    /// Sustain-loop start node index.
    pub sustain_start: u8,
    /// Sustain-loop end node index.
    pub sustain_end: u8,
    /// `(value, tick)` for each node, in the file's own units.
    pub nodes: Vec<(i8, u16)>,
}

impl ItEnvelope {
    /// Parse the 82-byte structure at `bytes`.
    pub fn parse(bytes: &[u8]) -> ItEnvelope {
        let byte = |offset: usize| bytes.get(offset).copied().unwrap_or(0);
        // Bounded by the format's limit *and* by what the slice actually holds, so a
        // truncated structure reads the nodes that are there rather than inventing silent
        // ones behind them.
        let readable = (bytes.len().saturating_sub(6) / 3).min(MAX_ENVELOPE_NODES) as u8;
        let node_count = byte(1).min(readable);
        let mut nodes = Vec::with_capacity(node_count as usize);
        for node in 0..node_count as usize {
            let base = 6 + node * 3;
            let value = byte(base) as i8;
            let tick = u16::from_le_bytes([byte(base + 1), byte(base + 2)]);
            nodes.push((value, tick));
        }
        ItEnvelope {
            flags: byte(0),
            node_count,
            loop_start: byte(2),
            loop_end: byte(3),
            sustain_start: byte(4),
            sustain_end: byte(5),
            nodes,
        }
    }

    /// Whether the envelope is on.
    pub const fn is_enabled(&self) -> bool { self.flags & ENVELOPE_ENABLED != 0 }

    /// Whether the pitch envelope's filter bit is set.
    pub const fn is_filter(&self) -> bool { self.flags & ENVELOPE_FILTER != 0 }

    /// Convert to the model's [`Envelope`], clamping node values into `low ..= high`.
    ///
    /// `None` when the envelope is off: nothing downstream reads a disabled envelope's
    /// points, and `Option::None` is how the model spells "this instrument has none".
    pub fn to_model(&self, low: i16, high: i16) -> Option<Envelope> {
        if !self.is_enabled() {
            return None;
        }
        let points: Vec<EnvelopePoint> = self.nodes.iter()
            .map(|(value, tick)| EnvelopePoint { tick: *tick, value: (*value as i16).clamp(low, high) })
            .collect();
        // A span whose indices fall outside the points is dropped rather than clamped into
        // a different loop, which is what `ITEnvelope::ConvertToMPT`'s own guard does.
        let node_count = points.len();
        let span = |enabled: bool, start: u8, end: u8| match enabled && (start as usize) < node_count && (end as usize) < node_count && start <= end {
            true => Some(EnvelopeSpan { start, end }),
            false => None,
        };
        Some(Envelope {
            sustain: span(self.flags & ENVELOPE_SUSTAIN != 0, self.sustain_start, self.sustain_end),
            loop_span: span(self.flags & ENVELOPE_LOOP != 0, self.loop_start, self.loop_end),
            carry: self.flags & ENVELOPE_CARRY != 0,
            points: points.into_boxed_slice(),
        })
    }
}

/// One IT instrument header, in either layout, decoded but not yet clamped against the
/// module's sample count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItInstrument {
    /// Whether `IMPI` was present.
    pub has_magic: bool,
    /// The instrument's name, already trimmed.
    pub name: String,
    /// New note action.
    pub new_note_action: u8,
    /// Duplicate check type.
    pub duplicate_check: u8,
    /// Duplicate check action.
    pub duplicate_action: u8,
    /// `FadeOut`, normalised to the IT 2.xx scale (see [`OLD_FADEOUT_SCALE`]).
    pub fadeout: u16,
    /// `PPS`, pitch/pan separation, `-32..=32`.
    pub pitch_pan_separation: i8,
    /// `PPC`, pitch/pan centre note, 0..=119.
    pub pitch_pan_centre: u8,
    /// `GbV`, 0..=128.
    pub global_volume: u8,
    /// `DfP`, raw: bits 0..=6 the position, bit 7 to *ignore* it.
    pub default_pan: u8,
    /// `RV`, random volume variation, a percentage.
    pub random_volume_variation: u8,
    /// `RP`, random pan variation, 0..=64.
    pub random_pan_variation: u8,
    /// `TrkVers`, only meaningful in a standalone `.iti`.
    pub tracker_version: u16,
    /// `NoS`, only meaningful in a standalone `.iti`.
    pub sample_count: u8,
    /// `IFC`, raw: bits 0..=6 the cutoff, bit 7 to use it.
    pub filter_cutoff: u8,
    /// `IFR`, raw: bits 0..=6 the resonance, bit 7 to use it.
    pub filter_resonance: u8,
    /// The note each of the 120 keys plays, 0..=119.
    pub note_map: [u8; NOTE_MAP_LENGTH],
    /// The sample each of the 120 keys uses, one-based, `0` meaning none.
    pub sample_map: [u8; NOTE_MAP_LENGTH],
    /// The volume envelope.
    pub volume_envelope: ItEnvelope,
    /// The panning envelope. Always off in the pre-2.00 layout, which has none.
    pub panning_envelope: ItEnvelope,
    /// The pitch/filter envelope. Always off in the pre-2.00 layout.
    pub pitch_envelope: ItEnvelope,
}

impl ItInstrument {
    /// Parse the IT 2.xx layout (`Cmwt >= 0x200`).
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if fewer than [`HEADER_LENGTH`] bytes were supplied.
    pub fn parse(bytes: &[u8], name: String) -> Result<ItInstrument, Error> {
        let bytes = bytes.get(..HEADER_LENGTH).ok_or(Error::Truncated { offset: 0, needed: HEADER_LENGTH })?;
        let byte = |offset: usize| bytes.get(offset).copied().unwrap_or(0);
        let (note_map, sample_map) = keyboard_maps(bytes);

        Ok(ItInstrument {
            has_magic: bytes.get(..4) == Some(&MAGIC[..]),
            name,
            new_note_action: byte(0x11),
            duplicate_check: byte(0x12),
            duplicate_action: byte(0x13),
            fadeout: u16::from_le_bytes([byte(0x14), byte(0x15)]),
            pitch_pan_separation: byte(0x16) as i8,
            pitch_pan_centre: byte(0x17),
            global_volume: byte(0x18),
            default_pan: byte(0x19),
            random_volume_variation: byte(0x1A),
            random_pan_variation: byte(0x1B),
            tracker_version: u16::from_le_bytes([byte(0x1C), byte(0x1D)]),
            sample_count: byte(0x1E),
            filter_cutoff: byte(0x3A),
            filter_resonance: byte(0x3B),
            note_map,
            sample_map,
            volume_envelope: ItEnvelope::parse(bytes.get(VOLUME_ENVELOPE_OFFSET..VOLUME_ENVELOPE_OFFSET + ENVELOPE_LENGTH).unwrap_or_default()),
            panning_envelope: ItEnvelope::parse(bytes.get(PANNING_ENVELOPE_OFFSET..PANNING_ENVELOPE_OFFSET + ENVELOPE_LENGTH).unwrap_or_default()),
            pitch_envelope: ItEnvelope::parse(bytes.get(PITCH_ENVELOPE_OFFSET..PITCH_ENVELOPE_OFFSET + ENVELOPE_LENGTH).unwrap_or_default()),
        })
    }

    /// Parse the pre-2.00 layout (`Cmwt < 0x200`), research point 1.
    ///
    /// The old header is the same 554 bytes and the same keyboard table, and differs in
    /// everything else: there is one envelope rather than three, it is a volume envelope
    /// only, its loop points sit in the header rather than in the envelope structure, its
    /// nodes are `(tick, value)` pairs rather than `(value, tick)` triples, and its fadeout
    /// is on half the scale. There is no panning, no pitch/pan separation, no filter, no
    /// duplicate-check *action* and no instrument global volume: the fields simply do not
    /// exist, so they take the values `ITOldInstrument::ConvertToMPT` gives them — full
    /// global volume, centre pan, `Cut` on a duplicate.
    ///
    /// The 200-byte pre-interpolated envelope table at `0x130` is not read; it is derived
    /// from the nodes that follow it, and OpenMPT ignores it too.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if fewer than [`HEADER_LENGTH`] bytes were supplied.
    pub fn parse_old(bytes: &[u8], name: String) -> Result<ItInstrument, Error> {
        let bytes = bytes.get(..HEADER_LENGTH).ok_or(Error::Truncated { offset: 0, needed: HEADER_LENGTH })?;
        let byte = |offset: usize| bytes.get(offset).copied().unwrap_or(0);
        let (note_map, sample_map) = keyboard_maps(bytes);

        // The old flag byte uses the same low three bits as the new envelope flag byte, so
        // it transfers unchanged; carry and filter did not exist yet.
        let flags = byte(0x11) & (ENVELOPE_ENABLED | ENVELOPE_LOOP | ENVELOPE_SUSTAIN);
        let mut nodes = Vec::with_capacity(MAX_ENVELOPE_NODES);
        for node in 0..MAX_ENVELOPE_NODES {
            let tick = byte(OLD_ENVELOPE_NODES_OFFSET + node * 2);
            if tick == 0xFF {
                break;
            }
            nodes.push((byte(OLD_ENVELOPE_NODES_OFFSET + node * 2 + 1) as i8, tick as u16));
        }

        Ok(ItInstrument {
            has_magic: bytes.get(..4) == Some(&MAGIC[..]),
            name,
            new_note_action: byte(0x1A),
            duplicate_check: byte(0x1B),
            duplicate_action: 0,
            fadeout: u16::from_le_bytes([byte(0x18), byte(0x19)]).saturating_mul(OLD_FADEOUT_SCALE),
            pitch_pan_separation: 0,
            pitch_pan_centre: 60,
            global_volume: MAX_GLOBAL_VOLUME,
            default_pan: PAN_IGNORED,
            random_volume_variation: 0,
            random_pan_variation: 0,
            tracker_version: u16::from_le_bytes([byte(0x1C), byte(0x1D)]),
            sample_count: byte(0x1E),
            filter_cutoff: 0,
            filter_resonance: 0,
            note_map,
            sample_map,
            volume_envelope: ItEnvelope {
                flags,
                node_count: nodes.len() as u8,
                loop_start: byte(0x12),
                loop_end: byte(0x13),
                sustain_start: byte(0x14),
                sustain_end: byte(0x15),
                nodes,
            },
            panning_envelope: ItEnvelope::default(),
            pitch_envelope: ItEnvelope::default(),
        })
    }

    /// Turn this header into the model's [`InstrumentDef`].
    ///
    /// `sample_count` is the module's, so a keyboard entry naming a sample the file does
    /// not have becomes "no sample" rather than a dangling id the builder would reject.
    pub fn to_model(&self, sample_count: usize) -> InstrumentDef {
        let mut note_sample_map = [0u16; NOTE_MAP_LENGTH];
        let mut note_transpose_map = [0u8; NOTE_MAP_LENGTH];
        for (note, (mapped_sample, mapped_note)) in note_sample_map.iter_mut().zip(note_transpose_map.iter_mut()).enumerate() {
            let sample = self.sample_map.get(note).copied().unwrap_or(0);
            *mapped_sample = match sample as usize <= sample_count {
                true => sample as u16,
                false => 0,
            };
            *mapped_note = self.note_map.get(note).copied().unwrap_or(note as u8);
        }

        InstrumentDef {
            name: self.name.clone().into_boxed_str(),
            sample: None,
            default_volume: U0F16::MAX,
            note_sample_map,
            note_transpose_map,
            volume_envelope: self.volume_envelope.to_model(0, 64),
            panning_envelope: self.panning_envelope.to_model(-32, 32),
            pitch_envelope: self.pitch_envelope.to_model(-32, 32),
            fadeout: self.fadeout,
            new_note_action: match self.new_note_action {
                1 => NewNoteAction::Continue,
                2 => NewNoteAction::NoteOff,
                3 => NewNoteAction::NoteFade,
                _ => NewNoteAction::Cut,
            },
            duplicate_check: match self.duplicate_check {
                1 => DuplicateCheck::Note,
                2 => DuplicateCheck::Sample,
                3 => DuplicateCheck::Instrument,
                _ => DuplicateCheck::Off,
            },
            duplicate_action: match self.duplicate_action {
                1 => DuplicateAction::NoteOff,
                2 => DuplicateAction::NoteFade,
                _ => DuplicateAction::Cut,
            },
            global_volume: unit_from_ratio(core::cmp::min(self.global_volume, MAX_GLOBAL_VOLUME) as u32, MAX_GLOBAL_VOLUME as u32),
            default_pan: match self.default_pan & PAN_IGNORED == 0 {
                true => Some(pan_to_bipolar(self.default_pan & 0x7F)),
                false => None,
            },
            pitch_pan_separation: self.pitch_pan_separation.clamp(-32, 32),
            pitch_pan_centre: core::cmp::min(self.pitch_pan_centre, (NOTE_MAP_LENGTH - 1) as u8),
            random_volume_variation: core::cmp::min(self.random_volume_variation, 100),
            random_pan_variation: core::cmp::min(self.random_pan_variation, 64),
            initial_filter_cutoff: match self.filter_cutoff & FILTER_ENABLED != 0 {
                true => Some(self.filter_cutoff & 0x7F),
                false => None,
            },
            initial_filter_resonance: match self.filter_resonance & FILTER_ENABLED != 0 {
                true => Some(self.filter_resonance & 0x7F),
                false => None,
            },
            pitch_envelope_is_filter: self.pitch_envelope.is_filter(),
        }
    }
}

/// Split the 240-byte keyboard table into the note map and the sample map.
///
/// A note byte of 120 or more is out of range and becomes the identity — the key plays its
/// own note — which is what `ITInstrument::ConvertToMPT` does with it.
fn keyboard_maps(bytes: &[u8]) -> ([u8; NOTE_MAP_LENGTH], [u8; NOTE_MAP_LENGTH]) {
    let mut note_map = [0u8; NOTE_MAP_LENGTH];
    let mut sample_map = [0u8; NOTE_MAP_LENGTH];
    for (note, (mapped_note, mapped_sample)) in note_map.iter_mut().zip(sample_map.iter_mut()).enumerate() {
        let raw_note = bytes.get(KEYBOARD_OFFSET + note * 2).copied().unwrap_or(0);
        *mapped_note = match (raw_note as usize) < NOTE_MAP_LENGTH {
            true => raw_note,
            false => note as u8,
        };
        *mapped_sample = bytes.get(KEYBOARD_OFFSET + note * 2 + 1).copied().unwrap_or(0);
    }
    (note_map, sample_map)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn write_envelope(bytes: &mut [u8], offset: usize, flags: u8, nodes: &[(i8, u16)], loop_span: (u8, u8), sustain: (u8, u8)) {
        bytes[offset] = flags;
        bytes[offset + 1] = nodes.len() as u8;
        bytes[offset + 2] = loop_span.0;
        bytes[offset + 3] = loop_span.1;
        bytes[offset + 4] = sustain.0;
        bytes[offset + 5] = sustain.1;
        for (index, (value, tick)) in nodes.iter().enumerate() {
            let base = offset + 6 + index * 3;
            bytes[base] = *value as u8;
            bytes[base + 1..base + 3].copy_from_slice(&tick.to_le_bytes());
        }
    }

    fn synthetic_instrument() -> [u8; HEADER_LENGTH] {
        let mut bytes = [0u8; HEADER_LENGTH];
        bytes[..4].copy_from_slice(&MAGIC);
        bytes[0x11] = 3; // NNA: note fade
        bytes[0x12] = 2; // DCT: sample
        bytes[0x13] = 1; // DCA: note off
        bytes[0x14..0x16].copy_from_slice(&512u16.to_le_bytes());
        bytes[0x16] = (-8i8) as u8;
        bytes[0x17] = 60;
        bytes[0x18] = 96;
        bytes[0x19] = 48;
        bytes[0x1A] = 25;
        bytes[0x1B] = 12;
        bytes[0x1C..0x1E].copy_from_slice(&0x0214u16.to_le_bytes());
        bytes[0x1E] = 2;
        bytes[0x3A] = FILTER_ENABLED | 100;
        bytes[0x3B] = FILTER_ENABLED | 40;
        for note in 0..NOTE_MAP_LENGTH {
            bytes[KEYBOARD_OFFSET + note * 2] = note as u8;
            bytes[KEYBOARD_OFFSET + note * 2 + 1] = 1;
        }
        write_envelope(&mut bytes, VOLUME_ENVELOPE_OFFSET, ENVELOPE_ENABLED | ENVELOPE_LOOP | ENVELOPE_CARRY, &[(64, 0), (32, 20), (0, 40)], (0, 2), (0, 0));
        write_envelope(&mut bytes, PANNING_ENVELOPE_OFFSET, ENVELOPE_ENABLED | ENVELOPE_SUSTAIN, &[(-32, 0), (32, 10)], (0, 0), (0, 1));
        write_envelope(&mut bytes, PITCH_ENVELOPE_OFFSET, ENVELOPE_ENABLED | ENVELOPE_FILTER, &[(0, 0), (-32, 8)], (0, 0), (0, 0));
        bytes
    }

    #[test]
    fn a_new_format_instrument_reads_every_field_the_model_declares() {
        let header = ItInstrument::parse(&synthetic_instrument(), "lead".to_string()).expect("a valid header");
        let instrument = header.to_model(4);

        assert!(header.has_magic);
        assert_eq!(&*instrument.name, "lead");
        assert_eq!(instrument.new_note_action, NewNoteAction::NoteFade);
        assert_eq!(instrument.duplicate_check, DuplicateCheck::Sample);
        assert_eq!(instrument.duplicate_action, DuplicateAction::NoteOff);
        assert_eq!(instrument.fadeout, 512);
        assert_eq!(instrument.pitch_pan_separation, -8);
        assert_eq!(instrument.pitch_pan_centre, 60);
        assert_eq!(instrument.global_volume, unit_from_ratio(96, 128));
        assert_eq!(instrument.default_pan, Some(pan_to_bipolar(48)));
        assert_eq!(instrument.random_volume_variation, 25);
        assert_eq!(instrument.random_pan_variation, 12);
        assert_eq!(instrument.initial_filter_cutoff, Some(100));
        assert_eq!(instrument.initial_filter_resonance, Some(40));
        assert!(instrument.pitch_envelope_is_filter);
        assert_eq!(instrument.sample, None, "an instrument selects its sample through the keyboard table");
        assert_eq!(instrument.note_sample_map[60], 1);
        assert_eq!(instrument.note_transpose_map[60], 60);
    }

    #[test]
    fn the_three_envelopes_keep_their_own_value_ranges() {
        let header = ItInstrument::parse(&synthetic_instrument(), String::new()).expect("a valid header");
        let instrument = header.to_model(4);

        let volume = instrument.volume_envelope.expect("the volume envelope is on");
        assert_eq!(volume.points.len(), 3);
        assert_eq!(volume.points[0], EnvelopePoint { tick: 0, value: 64 });
        assert_eq!(volume.points[2], EnvelopePoint { tick: 40, value: 0 });
        assert_eq!(volume.loop_span, Some(EnvelopeSpan { start: 0, end: 2 }));
        assert_eq!(volume.sustain, None);
        assert!(volume.carry);

        let panning = instrument.panning_envelope.expect("the panning envelope is on");
        assert_eq!(panning.points[0].value, -32, "panning envelope values stay signed");
        assert_eq!(panning.points[1].value, 32);
        assert_eq!(panning.sustain, Some(EnvelopeSpan { start: 0, end: 1 }));

        let pitch = instrument.pitch_envelope.expect("the pitch envelope is on");
        assert_eq!(pitch.points[1].value, -32);
    }

    #[test]
    fn a_disabled_envelope_is_absent_rather_than_empty() {
        let mut bytes = synthetic_instrument();
        bytes[VOLUME_ENVELOPE_OFFSET] = 0;
        bytes[PANNING_ENVELOPE_OFFSET] = 0;
        bytes[PITCH_ENVELOPE_OFFSET] = 0;
        let instrument = ItInstrument::parse(&bytes, String::new()).expect("valid").to_model(4);

        assert_eq!(instrument.volume_envelope, None);
        assert_eq!(instrument.panning_envelope, None);
        assert_eq!(instrument.pitch_envelope, None);
        assert!(!instrument.pitch_envelope_is_filter, "the filter bit needs the envelope to be on");
    }

    #[test]
    fn an_envelope_span_outside_its_points_is_dropped_rather_than_clamped() {
        let mut bytes = synthetic_instrument();
        write_envelope(&mut bytes, VOLUME_ENVELOPE_OFFSET, ENVELOPE_ENABLED | ENVELOPE_LOOP | ENVELOPE_SUSTAIN, &[(64, 0), (0, 10)], (1, 9), (7, 3));
        let instrument = ItInstrument::parse(&bytes, String::new()).expect("valid").to_model(4);
        let volume = instrument.volume_envelope.expect("the envelope is on");

        assert_eq!(volume.loop_span, None, "node 9 does not exist");
        assert_eq!(volume.sustain, None, "the sustain span is both out of range and inverted");
    }

    #[test]
    fn more_than_twenty_five_nodes_are_clamped_to_the_formats_limit() {
        let mut bytes = synthetic_instrument();
        bytes[VOLUME_ENVELOPE_OFFSET + 1] = 200;
        let envelope = ItEnvelope::parse(&bytes[VOLUME_ENVELOPE_OFFSET..VOLUME_ENVELOPE_OFFSET + ENVELOPE_LENGTH]);

        assert_eq!(envelope.node_count, MAX_ENVELOPE_NODES as u8);
        assert_eq!(envelope.nodes.len(), MAX_ENVELOPE_NODES);
    }

    #[test]
    fn a_keyboard_entry_naming_a_sample_the_module_lacks_becomes_no_sample() {
        let mut bytes = synthetic_instrument();
        bytes[KEYBOARD_OFFSET + 60 * 2 + 1] = 99;
        bytes[KEYBOARD_OFFSET + 61 * 2] = 200;
        let instrument = ItInstrument::parse(&bytes, String::new()).expect("valid").to_model(4);

        assert_eq!(instrument.note_sample_map[60], 0, "sample 99 does not exist in a four-sample module");
        assert_eq!(instrument.note_transpose_map[61], 61, "an out-of-range note byte is the identity");
    }

    #[test]
    fn an_instrument_with_the_ignore_pan_bit_has_no_default_pan() {
        let mut bytes = synthetic_instrument();
        bytes[0x19] = PAN_IGNORED | 48;
        let instrument = ItInstrument::parse(&bytes, String::new()).expect("valid").to_model(4);

        assert_eq!(instrument.default_pan, None, "an instrument's bit 7 means *don't* use the pan");
    }

    #[test]
    fn an_old_format_instrument_converts_its_one_volume_envelope() {
        let mut bytes = [0u8; HEADER_LENGTH];
        bytes[..4].copy_from_slice(&MAGIC);
        bytes[0x11] = ENVELOPE_ENABLED | ENVELOPE_LOOP;
        bytes[0x12] = 0; // volume loop start
        bytes[0x13] = 2; // volume loop end
        bytes[0x14] = 0; // sustain start
        bytes[0x15] = 1; // sustain end
        bytes[0x18..0x1A].copy_from_slice(&64u16.to_le_bytes());
        bytes[0x1A] = 2; // NNA: note off
        bytes[0x1B] = 1; // DNC on
        for note in 0..NOTE_MAP_LENGTH {
            bytes[KEYBOARD_OFFSET + note * 2] = note as u8;
            bytes[KEYBOARD_OFFSET + note * 2 + 1] = 2;
        }
        // The old nodes are `(tick, value)` pairs and `0xFF` ends the list.
        let old_nodes: [(u8, u8); 3] = [(0, 64), (16, 32), (32, 0)];
        for (index, (tick, value)) in old_nodes.iter().enumerate() {
            bytes[OLD_ENVELOPE_NODES_OFFSET + index * 2] = *tick;
            bytes[OLD_ENVELOPE_NODES_OFFSET + index * 2 + 1] = *value;
        }
        bytes[OLD_ENVELOPE_NODES_OFFSET + 3 * 2] = 0xFF;

        let header = ItInstrument::parse_old(&bytes, "old".to_string()).expect("a valid header");
        let instrument = header.to_model(4);

        assert_eq!(instrument.fadeout, 128, "the old scale is half the new one");
        assert_eq!(instrument.new_note_action, NewNoteAction::NoteOff);
        assert_eq!(instrument.duplicate_check, DuplicateCheck::Note);
        assert_eq!(instrument.duplicate_action, DuplicateAction::Cut);
        assert_eq!(instrument.default_pan, None, "the old layout has no instrument pan");
        assert_eq!(instrument.global_volume, U0F16::MAX);
        assert_eq!(instrument.initial_filter_cutoff, None);
        assert_eq!(instrument.note_sample_map[60], 2);

        let volume = instrument.volume_envelope.expect("the volume envelope is on");
        assert_eq!(volume.points.len(), 3, "the 0xFF terminator ends the list");
        assert_eq!(volume.points[0], EnvelopePoint { tick: 0, value: 64 });
        assert_eq!(volume.points[1], EnvelopePoint { tick: 16, value: 32 });
        assert_eq!(volume.loop_span, Some(EnvelopeSpan { start: 0, end: 2 }));
        assert_eq!(volume.sustain, None, "the old flag byte did not ask for a sustain loop");
        assert_eq!(instrument.panning_envelope, None);
        assert_eq!(instrument.pitch_envelope, None);
    }

    #[test]
    fn a_short_instrument_header_is_truncated_not_a_panic() {
        assert_eq!(ItInstrument::parse(&[0u8; HEADER_LENGTH - 1], String::new()).err(), Some(Error::Truncated { offset: 0, needed: HEADER_LENGTH }));
        assert_eq!(ItInstrument::parse_old(&[], String::new()).err(), Some(Error::Truncated { offset: 0, needed: HEADER_LENGTH }));
        assert_eq!(ItEnvelope::parse(&[]), ItEnvelope::default(), "an empty envelope slice reads as an off envelope");
        assert_eq!(ItEnvelope::parse(&[]).to_model(0, 64), None);
        assert_eq!(ItEnvelope::parse(&[0xFFu8; 4]).nodes.len(), 0, "a truncated envelope reads no nodes it cannot see");
    }
}
