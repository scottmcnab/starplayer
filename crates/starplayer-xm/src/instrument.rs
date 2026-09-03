//! The XM instrument header — its 29-byte fixed part, its extended part, and the
//! conversion of both envelopes into the model's [`Envelope`].
//!
//! # The layout this walks
//!
//! ```text
//! 0x00  4   size          the whole header, including the extended part
//! 0x04  22  name
//! 0x1A  1   type          FastTracker 2 writes junk here, the same junk every time
//! 0x1B  2   sample count
//! ── the extended part, present when the sample count is non-zero ──
//! 0x1D  4   sample header size
//! 0x21  96  note → sample map, one local sample index per note C-0..B-7
//! 0x81  48  volume envelope: 12 × (u16 tick, u16 value)
//! 0xB1  48  panning envelope, the same shape
//! 0xE1  1   volume points     0xE2  1  panning points
//! 0xE3  1   volume sustain    0xE4  1  volume loop start   0xE5  1  volume loop end
//! 0xE6  1   panning sustain   0xE7  1  panning loop start  0xE8  1  panning loop end
//! 0xE9  1   volume type       0xEA  1  panning type
//! 0xEB  1   vibrato type      0xEC  1  sweep  0xED  1  depth  0xEE  1  rate
//! 0xEF  2   volume fadeout
//! 0xF1  …   MIDI settings and reserved bytes, up to a 263-byte header
//! ```
//!
//! Every field past the end of the bytes actually present reads as zero, because the
//! `size` field is the file's own and trackers disagree about it: FastTracker 2 writes
//! 263 for an instrument with samples and 33 for an empty one, ModPlug Tracker 1.0 alpha
//! writes 245, its beta writes 263, and `4-mat`'s `eternity.xm` writes 29. OpenMPT reads
//! all of them with `ReadStructPartial`, and so does this.

use starplayer_model::{AutoVibrato, AutoVibratoWaveform, Envelope, EnvelopePoint, EnvelopeSpan};

use alloc::boxed::Box;
use alloc::vec::Vec;

/// Bytes in the largest instrument header any tracker writes — FastTracker 2's and
/// OpenMPT's. A `size` field of zero is read as this, the way OpenMPT does.
pub const MAX_HEADER_LENGTH: usize = 263;

/// Bytes in the fixed part, before the extended part begins.
pub const FIXED_LENGTH: usize = 29;

/// Offset of the 22-byte instrument name.
pub const NAME_OFFSET: usize = 4;

/// Length of the instrument-name field.
pub const NAME_LENGTH: usize = 22;

/// Notes the file's note→sample map covers: C-0..B-7.
pub const NOTE_MAP_ENTRIES: usize = 96;

/// Breakpoints one XM envelope may have.
pub const MAX_ENVELOPE_POINTS: usize = 12;

/// Envelope `type` bit 0: the envelope is on.
pub const ENVELOPE_ENABLED: u8 = 1 << 0;

/// Envelope `type` bit 1: the envelope holds at its sustain point.
pub const ENVELOPE_SUSTAIN: u8 = 1 << 1;

/// Envelope `type` bit 2: the envelope loops.
pub const ENVELOPE_LOOP: u8 = 1 << 2;

/// One envelope's raw fields, exactly as the instrument header spells them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct XmEnvelope {
    /// The 12 `(tick, value)` breakpoints, however many the count says are real.
    pub points: [(u16, u16); MAX_ENVELOPE_POINTS],
    /// How many of `points` the file uses.
    pub point_count: u8,
    /// Index of the sustain point.
    pub sustain: u8,
    /// Index of the first point of the loop.
    pub loop_start: u8,
    /// Index of the last point of the loop.
    pub loop_end: u8,
    /// [`ENVELOPE_ENABLED`] / [`ENVELOPE_SUSTAIN`] / [`ENVELOPE_LOOP`].
    pub flags: u8,
}

impl Default for XmEnvelope {
    fn default() -> XmEnvelope {
        XmEnvelope { points: [(0, 0); MAX_ENVELOPE_POINTS], point_count: 0, sustain: 0, loop_start: 0, loop_end: 0, flags: 0 }
    }
}

impl XmEnvelope {
    /// The model's [`Envelope`], or `None` when this envelope is off.
    ///
    /// `Option<Envelope>` in the model *is* the enabled bit: an envelope that the file
    /// switched off, or that has no points, contributes nothing to playback, and there is
    /// nowhere else in [`InstrumentDef`](starplayer_model::InstrumentDef) to record that
    /// it was present but disabled. OpenMPT's `ConvertEnvelopeToMPT` makes the same two
    /// tests before it sets `ENV_ENABLED`.
    ///
    /// The sustain and loop spans are dropped when their indices are out of range, again
    /// exactly as `ConvertEnvelopeToMPT` does: a sustain point at or past
    /// [`MAX_ENVELOPE_POINTS`] is no sustain, and a loop whose end is out of range or
    /// before its start is no loop. XM has a sustain *point* rather than a span, so it
    /// becomes an [`EnvelopeSpan`] with `start == end`.
    pub fn to_model(&self) -> Option<Envelope> {
        let count = core::cmp::min(self.point_count as usize, MAX_ENVELOPE_POINTS);
        if self.flags & ENVELOPE_ENABLED == 0 || count == 0 {
            return None;
        }

        let points: Vec<EnvelopePoint> = self.points.iter()
            .take(count)
            .map(|(tick, value)| EnvelopePoint { tick: *tick, value: *value as i16 })
            .collect();

        let sustain_index = self.sustain;
        let sustain = match self.flags & ENVELOPE_SUSTAIN != 0 && (sustain_index as usize) < MAX_ENVELOPE_POINTS {
            true => Some(EnvelopeSpan { start: sustain_index, end: sustain_index }),
            false => None,
        };
        let loops = self.flags & ENVELOPE_LOOP != 0
            && (self.loop_end as usize) < MAX_ENVELOPE_POINTS
            && self.loop_end >= self.loop_start;
        let loop_span = match loops {
            true => Some(EnvelopeSpan { start: self.loop_start, end: self.loop_end }),
            false => None,
        };

        Some(Envelope { points: points.into_boxed_slice(), sustain, loop_span, carry: false })
    }
}

/// One XM instrument header, parsed.
///
/// `name` is not here — it is a string and this type is `Copy`; the loader reads it out of
/// bytes [`NAME_OFFSET`]`..`[`NAME_OFFSET`]`+`[`NAME_LENGTH`] itself.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct XmInstrumentHeader {
    /// `size` (`0x00`), the whole header. Zero is read as [`MAX_HEADER_LENGTH`].
    pub header_size: u32,
    /// `type` (`0x1A`). Meaningless — FastTracker 2 writes uninitialised memory here —
    /// and kept only because OpenMPT's tracker detection reads it.
    pub instrument_type: u8,
    /// `numSamples` (`0x1B`).
    pub sample_count: u16,
    /// `sampleHeaderSize` (`0x1D`), the stride between this instrument's sample headers.
    /// Zero, or anything else absurd, is read as [`crate::sample::HEADER_LENGTH`].
    pub sample_header_size: u32,
    /// The note→sample map, one **local** sample index per note C-0..B-7.
    pub note_sample_map: [u8; NOTE_MAP_ENTRIES],
    /// The volume envelope's raw fields.
    pub volume_envelope: XmEnvelope,
    /// The panning envelope's raw fields.
    pub panning_envelope: XmEnvelope,
    /// Auto-vibrato, which FastTracker 2 stores per instrument and applies to every sample
    /// the instrument owns.
    pub auto_vibrato: AutoVibrato,
    /// `volFade` (`0xEF`), raw. The units are FastTracker 2's own.
    pub fadeout: u16,
}

impl XmInstrumentHeader {
    /// Parse an instrument header out of however many of its bytes are present.
    ///
    /// Never fails: a short slice reads its missing fields as zero. See the module
    /// documentation for why that is the right reading rather than an error.
    pub fn parse(bytes: &[u8]) -> XmInstrumentHeader {
        let mut note_sample_map = [0u8; NOTE_MAP_ENTRIES];
        for (note, entry) in note_sample_map.iter_mut().enumerate() {
            *entry = read_u8(bytes, 0x21 + note);
        }

        let header_size = match read_u32(bytes, 0x00) {
            0 => MAX_HEADER_LENGTH as u32,
            size => size,
        };
        let sample_header_size = match read_u32(bytes, 0x1D) {
            size if size == 0 || size > MAX_HEADER_LENGTH as u32 => crate::sample::HEADER_LENGTH as u32,
            size => size,
        };

        XmInstrumentHeader {
            header_size,
            instrument_type: read_u8(bytes, 0x1A),
            sample_count: read_u16(bytes, 0x1B),
            sample_header_size,
            note_sample_map,
            volume_envelope: read_envelope(bytes, 0x81, 0xE1, 0xE3, 0xE9),
            panning_envelope: read_envelope(bytes, 0xB1, 0xE2, 0xE6, 0xEA),
            auto_vibrato: AutoVibrato {
                waveform: vibrato_waveform(read_u8(bytes, 0xEB)),
                sweep: read_u8(bytes, 0xEC),
                depth: read_u8(bytes, 0xED),
                rate: read_u8(bytes, 0xEE),
            },
            fadeout: read_u16(bytes, 0xEF),
        }
    }
}

/// FastTracker 2's `vibType` numbering, which is not IT's: `0` sine, `1` square, `2` ramp
/// down, `3` ramp up. A value past 3 is undefined; FastTracker 2 masks it down to a sine.
pub const fn vibrato_waveform(vibrato_type: u8) -> AutoVibratoWaveform {
    match vibrato_type {
        1 => AutoVibratoWaveform::Square,
        2 => AutoVibratoWaveform::RampDown,
        3 => AutoVibratoWaveform::RampUp,
        _ => AutoVibratoWaveform::Sine,
    }
}

/// Read one envelope's twelve points and its five control bytes.
///
/// `points_offset` is the 48-byte point block; `count_offset` the point count;
/// `sustain_offset` the first of the sustain / loop-start / loop-end triple; `flags_offset`
/// the type byte.
fn read_envelope(bytes: &[u8], points_offset: usize, count_offset: usize, sustain_offset: usize, flags_offset: usize) -> XmEnvelope {
    let mut points = [(0u16, 0u16); MAX_ENVELOPE_POINTS];
    for (index, point) in points.iter_mut().enumerate() {
        *point = (read_u16(bytes, points_offset + index * 4), read_u16(bytes, points_offset + index * 4 + 2));
    }
    XmEnvelope {
        points,
        point_count: read_u8(bytes, count_offset),
        sustain: read_u8(bytes, sustain_offset),
        loop_start: read_u8(bytes, sustain_offset + 1),
        loop_end: read_u8(bytes, sustain_offset + 2),
        flags: read_u8(bytes, flags_offset),
    }
}

/// The note→sample map as the model wants it: one-based **global** sample ids over the
/// model's 120-note map, with a note whose local index names no sample left at zero.
pub fn global_note_sample_map(local_map: &[u8; NOTE_MAP_ENTRIES], sample_ids: &[u16]) -> [u16; starplayer_model::NOTE_MAP_LENGTH] {
    let mut map = [0u16; starplayer_model::NOTE_MAP_LENGTH];
    for (note, local) in local_map.iter().enumerate() {
        let Some(global) = sample_ids.get(*local as usize) else { continue };
        let Some(entry) = map.get_mut(note) else { break };
        *entry = global.saturating_add(1);
    }
    map
}

/// A name field, trimmed of its padding. Kept here so the loader has one place to ask.
pub fn name(bytes: &[u8], offset: usize, length: usize) -> Box<str> {
    let field = bytes.get(offset..offset + length).unwrap_or_default();
    starplayer_model::decode_cp437(field).into_boxed_str()
}

fn read_u8(bytes: &[u8], offset: usize) -> u8 { bytes.get(offset).copied().unwrap_or(0) }

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([read_u8(bytes, offset), read_u8(bytes, offset + 1)])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        read_u8(bytes, offset),
        read_u8(bytes, offset + 1),
        read_u8(bytes, offset + 2),
        read_u8(bytes, offset + 3),
    ])
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A 263-byte instrument header with two samples, both envelopes on, and a
    /// recognisable value in every field the loader reads.
    fn synthetic_instrument_header() -> [u8; MAX_HEADER_LENGTH] {
        let mut bytes = [0u8; MAX_HEADER_LENGTH];
        bytes[0x00..0x04].copy_from_slice(&(MAX_HEADER_LENGTH as u32).to_le_bytes());
        bytes[NAME_OFFSET..NAME_OFFSET + 5].copy_from_slice(b"piano");
        bytes[0x1A] = 0x42;
        bytes[0x1B..0x1D].copy_from_slice(&2u16.to_le_bytes());
        bytes[0x1D..0x21].copy_from_slice(&40u32.to_le_bytes());
        for note in 0..NOTE_MAP_ENTRIES {
            bytes[0x21 + note] = (note % 2) as u8;
        }
        // Volume envelope: three points at (0, 64), (10, 32), (20, 0).
        for (index, (tick, value)) in [(0u16, 64u16), (10, 32), (20, 0)].into_iter().enumerate() {
            bytes[0x81 + index * 4..0x81 + index * 4 + 2].copy_from_slice(&tick.to_le_bytes());
            bytes[0x81 + index * 4 + 2..0x81 + index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        // Panning envelope: two points at (0, 32), (16, 63).
        for (index, (tick, value)) in [(0u16, 32u16), (16, 63)].into_iter().enumerate() {
            bytes[0xB1 + index * 4..0xB1 + index * 4 + 2].copy_from_slice(&tick.to_le_bytes());
            bytes[0xB1 + index * 4 + 2..0xB1 + index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[0xE1] = 3; // volume points
        bytes[0xE2] = 2; // panning points
        bytes[0xE3] = 1; // volume sustain
        bytes[0xE4] = 0; // volume loop start
        bytes[0xE5] = 2; // volume loop end
        bytes[0xE6] = 0; // panning sustain
        bytes[0xE7] = 0; // panning loop start
        bytes[0xE8] = 1; // panning loop end
        bytes[0xE9] = ENVELOPE_ENABLED | ENVELOPE_SUSTAIN | ENVELOPE_LOOP;
        bytes[0xEA] = ENVELOPE_ENABLED;
        bytes[0xEB] = 3; // vibrato type: ramp up
        bytes[0xEC] = 20; // sweep
        bytes[0xED] = 8; // depth
        bytes[0xEE] = 4; // rate
        bytes[0xEF..0xF1].copy_from_slice(&512u16.to_le_bytes());
        bytes
    }

    #[test]
    fn every_instrument_field_is_read_at_the_offset_the_specification_gives() {
        let bytes = synthetic_instrument_header();
        let header = XmInstrumentHeader::parse(&bytes);

        assert_eq!(header.header_size, MAX_HEADER_LENGTH as u32);
        assert_eq!(header.instrument_type, 0x42);
        assert_eq!(header.sample_count, 2);
        assert_eq!(header.sample_header_size, 40);
        assert_eq!(header.note_sample_map[0], 0);
        assert_eq!(header.note_sample_map[1], 1);
        assert_eq!(header.note_sample_map[95], 1);
        assert_eq!(header.fadeout, 512);
        assert_eq!(header.auto_vibrato, AutoVibrato { waveform: AutoVibratoWaveform::RampUp, sweep: 20, depth: 8, rate: 4 });
        assert_eq!(name(&bytes, NAME_OFFSET, NAME_LENGTH).as_ref(), "piano");
    }

    #[test]
    fn an_enabled_envelope_becomes_the_models_envelope_with_its_spans() {
        let header = XmInstrumentHeader::parse(&synthetic_instrument_header());
        let volume = header.volume_envelope.to_model().expect("the volume envelope is on");

        assert_eq!(volume.points.len(), 3);
        assert_eq!(volume.points[0], EnvelopePoint { tick: 0, value: 64 });
        assert_eq!(volume.points[2], EnvelopePoint { tick: 20, value: 0 });
        assert_eq!(volume.sustain, Some(EnvelopeSpan { start: 1, end: 1 }), "XM's sustain point is a one-point span");
        assert_eq!(volume.loop_span, Some(EnvelopeSpan { start: 0, end: 2 }));
        assert!(!volume.carry, "carry is IT's, and XM never sets it");

        let panning = header.panning_envelope.to_model().expect("the panning envelope is on");
        assert_eq!(panning.points.len(), 2);
        assert_eq!(panning.sustain, None, "the sustain bit is off");
        assert_eq!(panning.loop_span, None, "and so is the loop bit");
    }

    #[test]
    fn an_envelope_that_is_off_or_empty_is_no_envelope_at_all() {
        let mut bytes = synthetic_instrument_header();
        bytes[0xE9] = ENVELOPE_SUSTAIN | ENVELOPE_LOOP; // spans set, but the enable bit is not
        assert_eq!(XmInstrumentHeader::parse(&bytes).volume_envelope.to_model(), None);

        let mut bytes = synthetic_instrument_header();
        bytes[0xE1] = 0;
        assert_eq!(XmInstrumentHeader::parse(&bytes).volume_envelope.to_model(), None, "an enabled envelope with no points is off");
    }

    #[test]
    fn a_span_whose_indices_are_out_of_range_is_dropped_rather_than_clamped() {
        let mut bytes = synthetic_instrument_header();
        bytes[0xE3] = 12; // sustain at the point past the last
        bytes[0xE5] = 200; // loop end likewise
        let volume = XmInstrumentHeader::parse(&bytes).volume_envelope.to_model().expect("still enabled");
        assert_eq!(volume.sustain, None);
        assert_eq!(volume.loop_span, None);

        let mut bytes = synthetic_instrument_header();
        bytes[0xE4] = 3; // loop start after loop end (2)
        let volume = XmInstrumentHeader::parse(&bytes).volume_envelope.to_model().expect("still enabled");
        assert_eq!(volume.loop_span, None, "a backwards loop is no loop");
    }

    #[test]
    fn a_point_count_past_twelve_is_cut_to_twelve() {
        let mut bytes = synthetic_instrument_header();
        bytes[0xE1] = 255;
        let volume = XmInstrumentHeader::parse(&bytes).volume_envelope.to_model().expect("still enabled");
        assert_eq!(volume.points.len(), MAX_ENVELOPE_POINTS);
    }

    #[test]
    fn a_header_shorter_than_the_extended_part_reads_zeroes_rather_than_failing() {
        // `4-mat`'s eternity.xm writes a 29-byte header for an empty instrument.
        let mut bytes = [0u8; FIXED_LENGTH];
        bytes[0x00..0x04].copy_from_slice(&(FIXED_LENGTH as u32).to_le_bytes());
        let header = XmInstrumentHeader::parse(&bytes);

        assert_eq!(header.header_size, FIXED_LENGTH as u32);
        assert_eq!(header.sample_count, 0);
        assert_eq!(header.volume_envelope.to_model(), None);
        assert_eq!(header.auto_vibrato, AutoVibrato::default());
        assert_eq!(header.sample_header_size, crate::sample::HEADER_LENGTH as u32);
    }

    #[test]
    fn a_size_of_zero_is_read_as_the_largest_header_any_tracker_writes() {
        let mut bytes = synthetic_instrument_header();
        bytes[0x00..0x04].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(XmInstrumentHeader::parse(&bytes).header_size, MAX_HEADER_LENGTH as u32);
    }

    #[test]
    fn an_absurd_sample_header_size_falls_back_to_the_formats_own_forty() {
        // Early Sk@le Tracker writes 0 (IFULOVE.XM); PlayerPRO writes junk.
        let mut bytes = synthetic_instrument_header();
        bytes[0x1D..0x21].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(XmInstrumentHeader::parse(&bytes).sample_header_size, 40);

        let mut bytes = synthetic_instrument_header();
        bytes[0x1D..0x21].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        assert_eq!(XmInstrumentHeader::parse(&bytes).sample_header_size, 40);

        let mut bytes = synthetic_instrument_header();
        bytes[0x1D..0x21].copy_from_slice(&0x12u32.to_le_bytes());
        assert_eq!(XmInstrumentHeader::parse(&bytes).sample_header_size, 0x12, "cybernostra weekend's short stride is honoured");
    }

    #[test]
    fn the_vibrato_numbering_is_fast_tracker_twos_and_not_impulse_trackers() {
        assert_eq!(vibrato_waveform(0), AutoVibratoWaveform::Sine);
        assert_eq!(vibrato_waveform(1), AutoVibratoWaveform::Square);
        assert_eq!(vibrato_waveform(2), AutoVibratoWaveform::RampDown);
        assert_eq!(vibrato_waveform(3), AutoVibratoWaveform::RampUp);
        assert_eq!(vibrato_waveform(200), AutoVibratoWaveform::Sine, "anything else is a sine");
    }

    #[test]
    fn the_note_map_becomes_one_based_global_ids_and_drops_what_names_no_sample() {
        let mut local = [0u8; NOTE_MAP_ENTRIES];
        local[0] = 0;
        local[1] = 1;
        local[2] = 9; // this instrument has only two samples
        let map = global_note_sample_map(&local, &[7, 8]);

        assert_eq!(map[0], 8, "local 0 is global sample 7, one-based");
        assert_eq!(map[1], 9);
        assert_eq!(map[2], 0, "a local index the instrument does not have maps to nothing");
        assert_eq!(map[NOTE_MAP_ENTRIES], 0, "notes above B-7 are never mapped");
        assert_eq!(map.len(), starplayer_model::NOTE_MAP_LENGTH);

        let empty = global_note_sample_map(&local, &[]);
        assert!(empty.iter().all(|entry| *entry == 0), "an instrument with no samples maps nothing");
        let _ = vec![0u8; 1];
    }
}
