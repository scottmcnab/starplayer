//! The 80-byte XM file header, the tracker-name classification that produces a
//! [`FormatDialect`], and [`XmFormatExtra`] — the XM-owned bits that ride in
//! [`ModuleHeader::format_extra`](starplayer_model::ModuleHeader::format_extra).

use starplayer_core::Error;
use starplayer_model::FormatDialect;

/// The 17-byte signature every XM starts with.
pub const MAGIC: [u8; 17] = *b"Extended Module: ";

/// Offset of the DOS end-of-file marker FastTracker 2 writes after the module name.
pub const EOF_MARKER_OFFSET: usize = 37;

/// The byte at [`EOF_MARKER_OFFSET`]. `probe` requires it as well as [`MAGIC`], because
/// the signature alone is 17 printable characters that a text file could carry.
pub const EOF_MARKER: u8 = 0x1A;

/// Offset of the module title.
pub const TITLE_OFFSET: usize = 17;

/// Length of the module title field.
pub const TITLE_LENGTH: usize = 20;

/// Offset of the 20-byte tracker-name field.
pub const TRACKER_NAME_OFFSET: usize = 38;

/// Length of the tracker-name field.
pub const TRACKER_NAME_LENGTH: usize = 20;

/// Bytes of the header this loader parses as fixed fields: everything up to the order
/// table. The `header_size` field measures from [`HEADER_SIZE_ORIGIN`] instead.
pub const FIXED_HEADER_LENGTH: usize = 80;

/// Offset the `header_size` field is measured from — itself, at `0x3C`. So the order
/// table starts at [`FIXED_HEADER_LENGTH`] and the first pattern at
/// `HEADER_SIZE_ORIGIN + header_size`.
pub const HEADER_SIZE_ORIGIN: usize = 60;

/// Smallest `header_size` that still covers the fixed fields this loader reads: the
/// four-byte size itself plus the eight `u16`s after it.
pub const MINIMUM_HEADER_SIZE: u32 = (FIXED_HEADER_LENGTH - HEADER_SIZE_ORIGIN) as u32;

/// `header_size` FastTracker 2 writes: [`MINIMUM_HEADER_SIZE`] plus the full 256-byte
/// order table. Part of the FastTracker 2 dialect evidence.
pub const FT2_HEADER_SIZE: u32 = MINIMUM_HEADER_SIZE + ORDER_TABLE_LENGTH as u32;

/// Entries the order table can hold. A `song_length` above this is clamped to it.
pub const ORDER_TABLE_LENGTH: usize = 256;

/// Channels this engine plays, which is `ChannelTable::MAX_CHANNELS`. FastTracker 2 wrote
/// 2..=32; OpenMPT writes up to this.
pub const MAX_CHANNELS: u16 = 64;

/// Patterns the format allows. The `u16` field is wider than the format.
pub const MAX_PATTERNS: u16 = 256;

/// Instruments the format allows.
pub const MAX_INSTRUMENTS: u16 = 128;

/// Instruments this loader will still parse, which is OpenMPT's own ceiling
/// (`MAX_INSTRUMENTS - 1` in `Load_xm.cpp`). Between [`MAX_INSTRUMENTS`] and this the
/// file is out of specification but readable; above it, it is refused.
pub const INSTRUMENT_LIMIT: u16 = 255;

/// `flags` bit 0: pitch slides are linear in semitones rather than in Amiga periods.
pub const FLAG_LINEAR_SLIDES: u16 = 1 << 0;

/// The first format version whose patterns come **before** the instruments and whose
/// sample data follows each instrument's sample headers. See [`XmHeader::version`].
pub const VERSION_1_04: u16 = 0x0104;

/// The one older version whose pattern header stores its row count as `u8 + 1`.
pub const VERSION_1_02: u16 = 0x0102;

/// The fixed part of an XM header, field for field.
///
/// `Copy`, so the title and the tracker name stay as raw bytes; the loader decodes the
/// title itself and [`XmHeader::dialect`] matches the tracker name without allocating.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct XmHeader {
    /// The 20-byte tracker-name field, raw and space-padded.
    pub tracker_name: [u8; TRACKER_NAME_LENGTH],
    /// Format version, high byte major: `0x0102`, `0x0103` or `0x0104` and later.
    pub version: u16,
    /// `header_size` (`0x3C`), measured from [`HEADER_SIZE_ORIGIN`].
    pub header_size: u32,
    /// Order-list entries the song uses, already clamped to [`ORDER_TABLE_LENGTH`].
    pub song_length: u16,
    /// Order index the song restarts from.
    pub restart_position: u16,
    /// Channels, 1..=[`MAX_CHANNELS`].
    pub channel_count: u16,
    /// Patterns stored in the file.
    pub pattern_count: u16,
    /// Instruments stored in the file.
    pub instrument_count: u16,
    /// Song flags; bit 0 is [`FLAG_LINEAR_SLIDES`].
    pub flags: u16,
    /// Ticks per row at the start of the song. FastTracker 2 calls this "default tempo".
    pub initial_speed: u16,
    /// Beats per minute at the start of the song. FastTracker 2 calls this "default BPM".
    pub initial_tempo: u16,
}

impl XmHeader {
    /// Parse the fixed [`FIXED_HEADER_LENGTH`] bytes at the start of the file.
    ///
    /// # Errors
    ///
    /// * [`Error::BadMagic`] — no [`MAGIC`], or no [`EOF_MARKER`] at
    ///   [`EOF_MARKER_OFFSET`].
    /// * [`Error::Invalid`] — a channel count of zero, which no tracker can play.
    /// * [`Error::TooLarge`] — a channel, pattern or instrument count past what the
    ///   format or this engine allows. See the [loader's](crate::loader) table.
    pub fn parse(bytes: &[u8; FIXED_HEADER_LENGTH]) -> Result<XmHeader, Error> {
        if bytes.get(..MAGIC.len()) != Some(&MAGIC[..]) || bytes.get(EOF_MARKER_OFFSET) != Some(&EOF_MARKER) {
            return Err(Error::BadMagic);
        }

        let mut tracker_name = [0u8; TRACKER_NAME_LENGTH];
        match bytes.get(TRACKER_NAME_OFFSET..TRACKER_NAME_OFFSET + TRACKER_NAME_LENGTH) {
            Some(field) => tracker_name.copy_from_slice(field),
            None => return Err(Error::Truncated { offset: TRACKER_NAME_OFFSET, needed: TRACKER_NAME_LENGTH }),
        }

        let channel_count = read_u16(bytes, 0x44);
        if channel_count == 0 {
            return Err(Error::Invalid("an XM with no channels"));
        }
        if channel_count > MAX_CHANNELS {
            return Err(Error::TooLarge("more XM channels than the engine's 64"));
        }
        let pattern_count = read_u16(bytes, 0x46);
        if pattern_count > MAX_PATTERNS {
            return Err(Error::TooLarge("more than 256 XM patterns"));
        }
        let instrument_count = read_u16(bytes, 0x48);
        if instrument_count > INSTRUMENT_LIMIT {
            return Err(Error::TooLarge("more than 255 XM instruments"));
        }

        Ok(XmHeader {
            tracker_name,
            version: read_u16(bytes, 0x3A),
            header_size: read_u32(bytes, 0x3C),
            song_length: core::cmp::min(read_u16(bytes, 0x40), ORDER_TABLE_LENGTH as u16),
            restart_position: read_u16(bytes, 0x42),
            channel_count,
            pattern_count,
            instrument_count,
            flags: read_u16(bytes, 0x4A),
            initial_speed: read_u16(bytes, 0x4C),
            initial_tempo: read_u16(bytes, 0x4E),
        })
    }

    /// Whether pitch slides are linear rather than Amiga-period based.
    pub const fn linear_slides(&self) -> bool { self.flags & FLAG_LINEAR_SLIDES != 0 }

    /// Whether patterns come before the instruments (`0x0104` and later) rather than
    /// after them.
    pub const fn patterns_precede_instruments(&self) -> bool { self.version >= VERSION_1_04 }

    /// Whether a pattern header stores its row count in one byte, biased by one. Only
    /// `0x0102` does; `0x0103` already uses the `u16` that `0x0104` does.
    pub const fn rows_are_a_biased_byte(&self) -> bool { self.version == VERSION_1_02 }

    /// Byte offset the data after the header starts at: `header_size` from
    /// [`HEADER_SIZE_ORIGIN`], with a `header_size` too small to cover the fields this
    /// loader has already read raised to [`MINIMUM_HEADER_SIZE`].
    pub const fn body_offset(&self) -> usize {
        let size = if self.header_size < MINIMUM_HEADER_SIZE { MINIMUM_HEADER_SIZE } else { self.header_size };
        HEADER_SIZE_ORIGIN + size as usize
    }

    /// Whether the tracker name is FastTracker 2's own — libxmp's `claims_ft2`
    /// (`xm_load.c:855-859`), the gate on its ModPlug Tracker 1.16 detection.
    pub fn claims_fast_tracker_2(&self) -> bool { self.tracker_name.starts_with(b"FastTracker v2.00") }

    /// Which tracker wrote this file, from the tracker name and header size alone.
    ///
    /// Reproduces the classifier at the head of OpenMPT's `CSoundFile::ReadXM` and
    /// libxmp's `xm_load.c:846-893`. OpenMPT's further split of the FastTracker 2 tag into
    /// "FT2 generic", "FT2 clone" and PlayerPRO rests on null-padding heuristics in the
    /// song title, and none of it selects a quirk StarPlayer has, so the one
    /// [`FormatDialect::FastTracker2`] covers all of them — as `quirks.rs` says it does.
    ///
    /// | Tracker name | Extra evidence | Dialect |
    /// |---|---|---|
    /// | `OpenMPT ` prefix | — | [`FormatDialect::OpenMptXm`] |
    /// | `MilkyTracker` prefix | — | [`FormatDialect::MilkyTracker`] |
    /// | `FastTracker v 2.00  ` exactly (note the extra space) | — | [`FormatDialect::ModPlugXm`] |
    /// | `FastTracker v2.00   ` exactly | `header_size == 276` | [`FormatDialect::FastTracker2`] |
    /// | `Fasttracker II clone` exactly | — | [`FormatDialect::FastTracker2`] |
    /// | `Skale Tracker` or `Sk@le Tracker`, NUL-terminated | — | [`FormatDialect::SkaleTracker`] |
    /// | anything else | — | [`FormatDialect::UnknownXm`] |
    ///
    /// [`FormatDialect::FastTracker2`] is the only answer here that carries FastTracker
    /// 2's replay bugs, so the fall-through is [`FormatDialect::UnknownXm`] rather than
    /// [`FormatDialect::Unknown`]: an XM whose tracker name says it was not written by
    /// FastTracker 2 is evidence, not the absence of it, and libxmp turns its whole
    /// `QUIRK_FT2BUGS` off on exactly this test.
    ///
    /// A ModPlug Tracker 1.16 file that signs itself `FastTracker v2.00   ` cannot be told
    /// apart here — it takes an instrument header or a trailing chunk to see — so
    /// [`crate::load_from`] revises that one case after it has read the body.
    pub fn dialect(&self) -> FormatDialect {
        let name = &self.tracker_name;
        if name.starts_with(b"OpenMPT ") {
            return FormatDialect::OpenMptXm;
        }
        if name.starts_with(b"MilkyTracker") {
            return FormatDialect::MilkyTracker;
        }
        if name == b"FastTracker v 2.00  " {
            return FormatDialect::ModPlugXm;
        }
        if name == b"Fasttracker II clone" {
            return FormatDialect::FastTracker2;
        }
        if name == b"FastTracker v2.00   " && self.header_size == FT2_HEADER_SIZE {
            return FormatDialect::FastTracker2;
        }
        // libxmp compares the whole 20-byte field with `strcmp`, so the name has to be
        // NUL-terminated rather than space-padded — which is what Skale writes.
        if name.starts_with(b"Skale Tracker ") || name.starts_with(b"Sk@le Tracker ") {
            return FormatDialect::SkaleTracker;
        }
        FormatDialect::UnknownXm
    }
}

/// The XM-owned header bits, packed into
/// [`ModuleHeader::format_extra`](starplayer_model::ModuleHeader::format_extra).
///
/// Both fields are things the *sequencer* needs and the format-neutral header has no room
/// for: the restart position becomes `SequencerSettings::restart_order`, and the raw flags
/// word carries bit 12, ModPlug's extended filter range, which
/// [`ModuleFlags`](starplayer_model::ModuleFlags) does not model.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct XmFormatExtra {
    /// `restart_position` (`0x42`), the order the song loops back to.
    pub restart_position: u16,
    /// `flags` (`0x4A`) raw, linear-slides bit included.
    pub flags: u16,
}

impl XmFormatExtra {
    /// Pack into the `u32` the module header carries.
    pub const fn encode(&self) -> u32 { self.restart_position as u32 | ((self.flags as u32) << 16) }

    /// Unpack the `u32` a module built by this crate carries.
    pub const fn decode(bits: u32) -> XmFormatExtra {
        XmFormatExtra { restart_position: bits as u16, flags: (bits >> 16) as u16 }
    }

    /// Extract the XM-owned bits from a header this crate produced.
    pub fn from_header(header: &starplayer_model::ModuleHeader) -> XmFormatExtra {
        XmFormatExtra::decode(header.format_extra)
    }
}

/// A little-endian `u16` at `offset`, or zero past the end. The callers all read inside a
/// fixed-size array, so "past the end" is unreachable and needs no error path.
fn read_u16(bytes: &[u8; FIXED_HEADER_LENGTH], offset: usize) -> u16 {
    match (bytes.get(offset), bytes.get(offset + 1)) {
        (Some(low), Some(high)) => u16::from_le_bytes([*low, *high]),
        _ => 0,
    }
}

/// A little-endian `u32` at `offset`, or zero past the end.
fn read_u32(bytes: &[u8; FIXED_HEADER_LENGTH], offset: usize) -> u32 {
    (read_u16(bytes, offset) as u32) | ((read_u16(bytes, offset + 2) as u32) << 16)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    /// A well-formed fixed header a test can then break one field of.
    pub(crate) fn synthetic_header(tracker: &[u8; TRACKER_NAME_LENGTH], version: u16) -> [u8; FIXED_HEADER_LENGTH] {
        let mut bytes = [0u8; FIXED_HEADER_LENGTH];
        bytes[..MAGIC.len()].copy_from_slice(&MAGIC);
        bytes[TITLE_OFFSET..TITLE_OFFSET + 5].copy_from_slice(b"title");
        bytes[EOF_MARKER_OFFSET] = EOF_MARKER;
        bytes[TRACKER_NAME_OFFSET..TRACKER_NAME_OFFSET + TRACKER_NAME_LENGTH].copy_from_slice(tracker);
        bytes[0x3A..0x3C].copy_from_slice(&version.to_le_bytes());
        bytes[0x3C..0x40].copy_from_slice(&FT2_HEADER_SIZE.to_le_bytes());
        bytes[0x40..0x42].copy_from_slice(&1u16.to_le_bytes());
        bytes[0x42..0x44].copy_from_slice(&0u16.to_le_bytes());
        bytes[0x44..0x46].copy_from_slice(&4u16.to_le_bytes());
        bytes[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
        bytes[0x48..0x4A].copy_from_slice(&1u16.to_le_bytes());
        bytes[0x4A..0x4C].copy_from_slice(&FLAG_LINEAR_SLIDES.to_le_bytes());
        bytes[0x4C..0x4E].copy_from_slice(&6u16.to_le_bytes());
        bytes[0x4E..0x50].copy_from_slice(&125u16.to_le_bytes());
        bytes
    }

    #[test]
    fn every_fixed_field_is_read_at_the_offset_the_specification_gives() {
        let header = XmHeader::parse(&synthetic_header(b"FastTracker v2.00   ", 0x0104)).expect("a valid header");

        assert_eq!(header.version, 0x0104);
        assert_eq!(header.header_size, 276);
        assert_eq!(header.song_length, 1);
        assert_eq!(header.restart_position, 0);
        assert_eq!(header.channel_count, 4);
        assert_eq!(header.pattern_count, 1);
        assert_eq!(header.instrument_count, 1);
        assert_eq!(header.initial_speed, 6);
        assert_eq!(header.initial_tempo, 125);
        assert!(header.linear_slides());
        assert!(header.patterns_precede_instruments());
        assert!(!header.rows_are_a_biased_byte());
        assert_eq!(header.body_offset(), 60 + 276);
    }

    #[test]
    fn the_signature_and_the_end_of_file_marker_are_both_required() {
        let mut bytes = synthetic_header(b"FastTracker v2.00   ", 0x0104);
        bytes[0] = b'x';
        assert_eq!(XmHeader::parse(&bytes), Err(Error::BadMagic));

        let mut bytes = synthetic_header(b"FastTracker v2.00   ", 0x0104);
        bytes[EOF_MARKER_OFFSET] = 0;
        assert_eq!(XmHeader::parse(&bytes), Err(Error::BadMagic), "the 0x1A marker is part of the signature");
    }

    #[test]
    fn counts_past_the_format_or_the_engine_are_refused_rather_than_clamped() {
        let mut bytes = synthetic_header(b"FastTracker v2.00   ", 0x0104);
        bytes[0x44..0x46].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(XmHeader::parse(&bytes), Err(Error::Invalid("an XM with no channels")));

        let mut bytes = synthetic_header(b"FastTracker v2.00   ", 0x0104);
        bytes[0x44..0x46].copy_from_slice(&65u16.to_le_bytes());
        assert_eq!(XmHeader::parse(&bytes), Err(Error::TooLarge("more XM channels than the engine's 64")));

        let mut bytes = synthetic_header(b"FastTracker v2.00   ", 0x0104);
        bytes[0x44..0x46].copy_from_slice(&64u16.to_le_bytes());
        assert_eq!(XmHeader::parse(&bytes).map(|header| header.channel_count), Ok(64), "64 is the last accepted count");

        let mut bytes = synthetic_header(b"FastTracker v2.00   ", 0x0104);
        bytes[0x46..0x48].copy_from_slice(&257u16.to_le_bytes());
        assert_eq!(XmHeader::parse(&bytes), Err(Error::TooLarge("more than 256 XM patterns")));

        let mut bytes = synthetic_header(b"FastTracker v2.00   ", 0x0104);
        bytes[0x48..0x4A].copy_from_slice(&256u16.to_le_bytes());
        assert_eq!(XmHeader::parse(&bytes), Err(Error::TooLarge("more than 255 XM instruments")));
    }

    #[test]
    fn a_song_length_past_the_order_table_is_clamped_to_it() {
        let mut bytes = synthetic_header(b"FastTracker v2.00   ", 0x0104);
        bytes[0x40..0x42].copy_from_slice(&1000u16.to_le_bytes());
        assert_eq!(XmHeader::parse(&bytes).map(|header| header.song_length), Ok(256));
    }

    #[test]
    fn a_header_size_below_the_fields_it_covers_still_lands_after_them() {
        let mut bytes = synthetic_header(b"FastTracker v2.00   ", 0x0104);
        bytes[0x3C..0x40].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(XmHeader::parse(&bytes).map(|header| header.body_offset()), Ok(FIXED_HEADER_LENGTH));

        // Real files do write a short one: 56 files under `openmpt/xm` have a 21-byte
        // header, which is the fields plus a one-entry order table.
        let mut bytes = synthetic_header(b"OpenMPT 1.29.13.00  ", 0x0104);
        bytes[0x3C..0x40].copy_from_slice(&21u32.to_le_bytes());
        assert_eq!(XmHeader::parse(&bytes).map(|header| header.body_offset()), Ok(81));
    }

    #[test]
    fn the_tracker_name_selects_the_dialect_openmpt_selects() {
        let dialect_of = |tracker: &[u8; TRACKER_NAME_LENGTH]| {
            XmHeader::parse(&synthetic_header(tracker, 0x0104)).expect("a valid header").dialect()
        };

        assert_eq!(dialect_of(b"FastTracker v2.00   "), FormatDialect::FastTracker2);
        assert_eq!(dialect_of(b"Fasttracker II clone"), FormatDialect::FastTracker2);
        assert_eq!(dialect_of(b"FastTracker v 2.00  "), FormatDialect::ModPlugXm, "the extra space is ModPlug 1.0");
        assert_eq!(dialect_of(b"OpenMPT 1.29.13.00  "), FormatDialect::OpenMptXm);
        assert_eq!(dialect_of(b"MilkyTracker        "), FormatDialect::MilkyTracker);
        assert_eq!(dialect_of(b"MilkyTracker 1.02.00"), FormatDialect::MilkyTracker);
        assert_eq!(dialect_of(b"MadTracker 2.0\0\0\0\0\0\0"), FormatDialect::UnknownXm);
        assert_eq!(dialect_of(b"rst's SoundTracker  "), FormatDialect::UnknownXm);
        assert_eq!(dialect_of(b"Skale Tracker\0\0\0\0\0\0\0"), FormatDialect::SkaleTracker);
        assert_eq!(dialect_of(b"Sk@le Tracker\0\0\0\0\0\0\0"), FormatDialect::SkaleTracker);
        assert_eq!(dialect_of(b"Skale Tracker       "), FormatDialect::UnknownXm, "libxmp compares the whole NUL-terminated name");
        assert_eq!(dialect_of(b"XpenMPT 1.20.00.39  "), FormatDialect::UnknownXm, "one letter out is not OpenMPT");
    }

    #[test]
    fn the_fast_tracker_tag_needs_the_header_size_that_goes_with_it() {
        let mut bytes = synthetic_header(b"FastTracker v2.00   ", 0x0104);
        bytes[0x3C..0x40].copy_from_slice(&21u32.to_le_bytes());
        assert_eq!(XmHeader::parse(&bytes).expect("a valid header").dialect(), FormatDialect::UnknownXm);
    }

    #[test]
    fn the_two_older_versions_change_only_the_two_layout_predicates() {
        let old = XmHeader::parse(&synthetic_header(b"FastTracker v2.00   ", 0x0102)).expect("a valid header");
        assert!(!old.patterns_precede_instruments());
        assert!(old.rows_are_a_biased_byte());

        let middle = XmHeader::parse(&synthetic_header(b"FastTracker v2.00   ", 0x0103)).expect("a valid header");
        assert!(!middle.patterns_precede_instruments());
        assert!(!middle.rows_are_a_biased_byte(), "only 1.02 writes the row count as a byte");
    }

    #[test]
    fn the_format_extra_word_round_trips_both_fields() {
        let extra = XmFormatExtra { restart_position: 0x1234, flags: 0x1001 };
        assert_eq!(XmFormatExtra::decode(extra.encode()), extra);
        assert_eq!(XmFormatExtra::decode(0), XmFormatExtra::default());
    }

    #[test]
    fn a_short_read_of_the_fixed_header_is_not_this_functions_problem() {
        // `parse` takes a fixed-size array, so truncation is the caller's error path; this
        // pins that the array form has no hidden panic for an all-zero buffer.
        let bytes = [0u8; FIXED_HEADER_LENGTH];
        assert_eq!(XmHeader::parse(&bytes), Err(Error::BadMagic));
        let _: Vec<u8> = vec![0u8; FIXED_HEADER_LENGTH];
    }
}
