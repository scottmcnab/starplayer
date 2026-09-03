//! The Impulse Tracker song header, the `format_extra` bitfield and the `format_data`
//! block the effect processor reads its `Zxx` / `Sxx` inputs out of.
//!
//! Everything here is the file's own spelling of its header — the numbers exactly as the
//! bytes hold them. Turning them into a [`ModuleHeader`](starplayer_model::ModuleHeader)
//! is [`crate::loader`]'s job.

use alloc::vec::Vec;
use starplayer_core::fixed::bipolar_from_ratio;
use starplayer_core::quirks::FormatDialect;
use starplayer_core::{Error, I1F15};
use starplayer_model::ModuleHeader;

/// The signature every IT carries at offset zero.
pub const MAGIC: [u8; 4] = *b"IMPM";

/// Offset of [`MAGIC`].
pub const MAGIC_OFFSET: usize = 0;

/// Bytes of fixed header before the order list — the order list starts at `0xC0`.
pub const HEADER_LENGTH: usize = 0xC0;

/// Channels an IT file describes: `ChnPan` and `ChnVol` are 64 bytes each, and the packed
/// pattern format's channel byte is masked to six bits by Impulse Tracker's own unpacker.
pub const MAX_CHANNELS: usize = 64;

/// `ChnPan` value meaning "surround", rather than a position on the 0..64 scale.
pub const PAN_SURROUND: u8 = 100;

/// `ChnPan` bit meaning "this channel is disabled": notes are not played, but effects in
/// it are still processed (ITTECH.TXT, *Chnl Pan*).
pub const PAN_DISABLED: u8 = 0x80;

/// `ChnPan` value that is dead centre.
pub const PAN_CENTRE: u8 = 32;

/// Highest `ChnPan` position, hard right.
pub const PAN_RIGHT: u8 = 64;

// ── Flags (header 0x2C) ─────────────────────────────────────────────────────────────

/// `Flags` bit 0 — stereo playback.
pub const FLAG_STEREO: u16 = 0x0001;
/// `Flags` bit 1 — the redundant "no mixing at volume zero" optimisation.
pub const FLAG_VOL0_OPTIMISATIONS: u16 = 0x0002;
/// `Flags` bit 2 — instrument mode. Clear means the instrument column names a sample.
pub const FLAG_INSTRUMENT_MODE: u16 = 0x0004;
/// `Flags` bit 3 — linear slides rather than Amiga slides.
pub const FLAG_LINEAR_SLIDES: u16 = 0x0008;
/// `Flags` bit 4 — old effects: deeper vibrato updated every tick, `Oxx` clamping to the
/// end of the sample rather than being ignored.
pub const FLAG_OLD_EFFECTS: u16 = 0x0010;
/// `Flags` bit 5 — link `Gxx`'s memory with `Exx`/`Fxx`.
pub const FLAG_COMPATIBLE_GXX: u16 = 0x0020;
/// `Flags` bit 6 — use the MIDI pitch controller, at the depth `PWD` gives.
pub const FLAG_MIDI_PITCH_CONTROLLER: u16 = 0x0040;
/// `Flags` bit 7 — the file asks for its embedded MIDI configuration to be used.
pub const FLAG_REQUEST_EMBEDDED_MIDI: u16 = 0x0080;
/// `Flags` bit 12 — OpenMPT's extended filter range. Not in ITTECH.TXT; `ITTools.h`
/// `extendedFilterRange`.
pub const FLAG_EXTENDED_FILTER_RANGE: u16 = 0x1000;

// ── Special (header 0x2E) ───────────────────────────────────────────────────────────

/// `Special` bit 0 — a song message is attached at `message_offset`.
pub const SPECIAL_SONG_MESSAGE: u16 = 0x0001;
/// `Special` bit 1 — an edit-history block follows the parapointer tables.
pub const SPECIAL_EDIT_HISTORY: u16 = 0x0002;
/// `Special` bit 2 — pattern row highlights are embedded.
pub const SPECIAL_PATTERN_HIGHLIGHTS: u16 = 0x0004;
/// `Special` bit 3 — the MIDI configuration block is embedded.
pub const SPECIAL_MIDI_CONFIGURATION: u16 = 0x0008;

/// Bytes in one edit-history entry (`ITHistoryStruct`).
pub const EDIT_HISTORY_ENTRY_BYTES: usize = 8;

// ── the embedded MIDI configuration ─────────────────────────────────────────────────

/// Bytes in one MIDI macro string.
pub const MIDI_MACRO_BYTES: usize = 32;
/// Global MIDI macros (`MIDI Out` — start, stop, tick, note on/off, volume, pan, bank,
/// program).
pub const MIDI_GLOBAL_MACROS: usize = 9;
/// Parametered macros, selected by `SF0`..`SFF`.
pub const MIDI_PARAMETERED_MACROS: usize = 16;
/// Fixed macros, selected by `Z80`..`ZFF`.
pub const MIDI_FIXED_MACROS: usize = 128;
/// Bytes in the whole embedded MIDI configuration block.
pub const MIDI_CONFIGURATION_BYTES: usize =
    (MIDI_GLOBAL_MACROS + MIDI_PARAMETERED_MACROS + MIDI_FIXED_MACROS) * MIDI_MACRO_BYTES;

/// The four `reserved` bytes OpenMPT writes when a file is *not* a compatibility export.
pub const OPENMPT_RESERVED_MARKER: [u8; 4] = *b"OMPT";

/// The IT file header, field for field. Raw: nothing here is clamped or scaled.
///
/// `title` is not here because it is a string and this type is `Copy`; the loader reads it
/// out of bytes `0x04..0x1E` itself.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItHeader {
    /// `PHiligt` low byte (`0x1E`): rows per beat.
    pub highlight_minor: u8,
    /// `PHiligt` high byte (`0x1F`): rows per measure.
    pub highlight_major: u8,
    /// `OrdNum` (`0x20`).
    pub order_count: u16,
    /// `InsNum` (`0x22`).
    pub instrument_count: u16,
    /// `SmpNum` (`0x24`).
    pub sample_count: u16,
    /// `PatNum` (`0x26`).
    pub pattern_count: u16,
    /// `Cwt/v` (`0x28`): created with tracker.
    pub tracker_version: u16,
    /// `Cmwt` (`0x2A`): compatible with tracker — the format version.
    pub format_version: u16,
    /// `Flags` (`0x2C`).
    pub flags: u16,
    /// `Special` (`0x2E`).
    pub special: u16,
    /// `GV` (`0x30`), 0..=128.
    pub global_volume: u8,
    /// `MV` (`0x31`), 0..=128.
    pub mix_volume: u8,
    /// `IS` (`0x32`).
    pub initial_speed: u8,
    /// `IT` (`0x33`).
    pub initial_tempo: u8,
    /// `Sep` (`0x34`), 0..=128.
    pub stereo_separation: u8,
    /// `PWD` (`0x35`), the MIDI pitch-wheel depth.
    pub pitch_wheel_depth: u8,
    /// `MsgLgth` (`0x36`).
    pub message_length: u16,
    /// `Message Offset` (`0x38`).
    pub message_offset: u32,
    /// `Reserved` (`0x3C`), raw. OpenMPT and Schism Tracker put version information here,
    /// which is why the dialect classifier reads it.
    pub reserved: [u8; 4],
    /// `ChnPan[64]` (`0x40`), raw: 0..=64 position, [`PAN_SURROUND`], `| PAN_DISABLED`.
    pub channel_pan: [u8; MAX_CHANNELS],
    /// `ChnVol[64]` (`0x80`), 0..=64.
    pub channel_volume: [u8; MAX_CHANNELS],
}

impl ItHeader {
    /// Parse the `0xC0`-byte fixed header.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if fewer than [`HEADER_LENGTH`] bytes were supplied, and
    /// [`Error::BadMagic`] if `IMPM` is not at offset zero.
    pub fn parse(bytes: &[u8]) -> Result<ItHeader, Error> {
        let bytes = bytes.get(..HEADER_LENGTH).ok_or(Error::Truncated { offset: 0, needed: HEADER_LENGTH })?;
        if bytes.get(..4) != Some(&MAGIC[..]) {
            return Err(Error::BadMagic);
        }

        let mut channel_pan = [0u8; MAX_CHANNELS];
        let mut channel_volume = [0u8; MAX_CHANNELS];
        for (channel, slot) in channel_pan.iter_mut().enumerate() {
            *slot = read_u8(bytes, 0x40 + channel)?;
        }
        for (channel, slot) in channel_volume.iter_mut().enumerate() {
            *slot = read_u8(bytes, 0x80 + channel)?;
        }

        Ok(ItHeader {
            highlight_minor: read_u8(bytes, 0x1E)?,
            highlight_major: read_u8(bytes, 0x1F)?,
            order_count: read_u16(bytes, 0x20)?,
            instrument_count: read_u16(bytes, 0x22)?,
            sample_count: read_u16(bytes, 0x24)?,
            pattern_count: read_u16(bytes, 0x26)?,
            tracker_version: read_u16(bytes, 0x28)?,
            format_version: read_u16(bytes, 0x2A)?,
            flags: read_u16(bytes, 0x2C)?,
            special: read_u16(bytes, 0x2E)?,
            global_volume: read_u8(bytes, 0x30)?,
            mix_volume: read_u8(bytes, 0x31)?,
            initial_speed: read_u8(bytes, 0x32)?,
            initial_tempo: read_u8(bytes, 0x33)?,
            stereo_separation: read_u8(bytes, 0x34)?,
            pitch_wheel_depth: read_u8(bytes, 0x35)?,
            message_length: read_u16(bytes, 0x36)?,
            message_offset: read_u32(bytes, 0x38)?,
            reserved: [read_u8(bytes, 0x3C)?, read_u8(bytes, 0x3D)?, read_u8(bytes, 0x3E)?, read_u8(bytes, 0x3F)?],
            channel_pan,
            channel_volume,
        })
    }

    /// Whether the module asks for stereo playback.
    pub const fn is_stereo(&self) -> bool { self.flags & FLAG_STEREO != 0 }

    /// Whether the instrument column names an instrument rather than a sample.
    pub const fn is_instrument_mode(&self) -> bool { self.flags & FLAG_INSTRUMENT_MODE != 0 }

    /// Whether pitch slides are linear in semitones.
    pub const fn is_linear_slides(&self) -> bool { self.flags & FLAG_LINEAR_SLIDES != 0 }

    /// Whether instruments are in the pre-2.00 layout (`Cmwt < 0x200`).
    pub const fn has_old_instruments(&self) -> bool { self.format_version < 0x0200 }

    /// Whether the file carries an embedded MIDI configuration block.
    ///
    /// OpenMPT reads it when *either* the `Flags` request bit or the `Special` embed bit
    /// is set (`Load_it.cpp`, `hasMidiConfig`), not only the documented `Special` bit.
    pub const fn has_midi_configuration(&self) -> bool {
        self.flags & FLAG_REQUEST_EMBEDDED_MIDI != 0 || self.special & SPECIAL_MIDI_CONFIGURATION != 0
    }

    /// The `reserved` field as the little-endian `u32` the dialect rules compare.
    pub const fn reserved_word(&self) -> u32 { u32::from_le_bytes(self.reserved) }

    /// Which tracker wrote this file, from the header alone.
    ///
    /// This reproduces the IT half of `Load_it.cpp`'s classifier at the granularity task
    /// E2's [`FormatDialect`] declares: the `0x5000` OpenMPT nibble split by the `OMPT`
    /// reserved marker, the `0x0888` markers OpenMPT 1.17 wrote before it, the three
    /// hand-recognised early ModPlug / OpenMPT `cwtv` / `cmwt` / `reserved` combinations,
    /// and then the high nibble: `0` is Impulse Tracker (and the clones E2 folds into it),
    /// `1` is Schism Tracker.
    pub const fn dialect(&self) -> FormatDialect {
        let reserved = self.reserved_word();
        if self.tracker_version & 0xF000 == 0x5000 {
            return match reserved == u32::from_le_bytes(OPENMPT_RESERVED_MARKER) {
                true => FormatDialect::OpenMptIt,
                false => FormatDialect::ModPlugIt,
            };
        }
        if self.tracker_version == 0x0888 || self.format_version == 0x0888 {
            // OpenMPT 1.17.02.26 (r122) to 1.18.
            return FormatDialect::OpenMptIt;
        }
        if self.tracker_version == 0x0214 && self.format_version == 0x0202 && reserved == 0 {
            // ModPlug Tracker b3.2 - 1.09.
            return FormatDialect::ModPlugIt;
        }
        if self.tracker_version == 0x0300
            && self.format_version == 0x0300
            && reserved == 0
            && self.order_count == 256
            && self.stereo_separation == 128
            && self.pitch_wheel_depth == 0
        {
            // The rare OpenMPT 1.17.02.20 - 1.17.02.25 variant.
            return FormatDialect::OpenMptIt;
        }
        if self.tracker_version == 0x0217 && self.format_version == 0x0200 && reserved == 0 {
            // ModPlug Tracker 1.09 - 1.16, or OpenMPT 1.17's compatibility export. The two
            // are told apart by evidence outside the header, which E2's granularity does
            // not ask for.
            return FormatDialect::ModPlugIt;
        }
        match self.tracker_version >> 12 {
            0 => FormatDialect::ImpulseTracker,
            1 => FormatDialect::SchismTracker,
            _ => FormatDialect::Unknown,
        }
    }
}

/// One channel's `ChnPan` byte as a bipolar position, `-1` hard left to `+1` hard right.
///
/// A surround channel and a disabled channel both answer their *position*: surround is
/// centre, and a disabled channel keeps the position its low seven bits give, so a host
/// that un-mutes it pans it where the file says. Which channels are surround and which are
/// disabled is in [`ItFormatData::channel_pan_raw`].
pub fn pan_to_bipolar(raw: u8) -> I1F15 {
    let value = raw & !PAN_DISABLED;
    if value == PAN_SURROUND {
        return I1F15::ZERO;
    }
    let position = if value > PAN_RIGHT { PAN_CENTRE } else { value };
    bipolar_from_ratio(position as i32 - PAN_CENTRE as i32, PAN_CENTRE as i32)
}

// ── format_extra ────────────────────────────────────────────────────────────────────

/// The IT decoder for
/// [`ModuleHeader::format_extra`](starplayer_model::ModuleHeader::format_extra).
///
/// # Bit layout
///
/// | Bits | Field |
/// |---|---|
/// | 0..=15 | the file's `Flags` word, verbatim — [`FLAG_INSTRUMENT_MODE`], [`FLAG_OLD_EFFECTS`], [`FLAG_COMPATIBLE_GXX`], [`FLAG_MIDI_PITCH_CONTROLLER`], [`FLAG_REQUEST_EMBEDDED_MIDI`], [`FLAG_EXTENDED_FILTER_RANGE`] and the rest |
/// | 16..=19 | the file's `Special` low nibble — [`SPECIAL_SONG_MESSAGE`] … [`SPECIAL_MIDI_CONFIGURATION`] |
/// | 20 | instruments are in the pre-2.00 layout ([`ItHeader::has_old_instruments`]) |
/// | 21 | a MIDI configuration block is present in [`ModuleHeader::format_data`] |
/// | 22..=31 | reserved, zero |
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ItFormatExtra {
    /// The `Flags` word, verbatim.
    pub flags: u16,
    /// The `Special` low nibble, verbatim.
    pub special: u8,
    /// Instruments were read in the pre-2.00 layout.
    pub old_instruments: bool,
    /// [`ModuleHeader::format_data`] carries a MIDI configuration block.
    pub has_midi_configuration: bool,
}

/// `format_extra` bit 20 — pre-2.00 instrument layout.
const EXTRA_OLD_INSTRUMENTS: u32 = 1 << 20;
/// `format_extra` bit 21 — a MIDI configuration block is present.
const EXTRA_MIDI_CONFIGURATION: u32 = 1 << 21;

impl ItFormatExtra {
    /// Pack into the header's `format_extra` word.
    pub const fn encode(&self) -> u32 {
        self.flags as u32
            | ((self.special & 0x0F) as u32) << 16
            | if self.old_instruments { EXTRA_OLD_INSTRUMENTS } else { 0 }
            | if self.has_midi_configuration { EXTRA_MIDI_CONFIGURATION } else { 0 }
    }

    /// Unpack a `format_extra` word.
    pub const fn decode(word: u32) -> ItFormatExtra {
        ItFormatExtra {
            flags: word as u16,
            special: ((word >> 16) & 0x0F) as u8,
            old_instruments: word & EXTRA_OLD_INSTRUMENTS != 0,
            has_midi_configuration: word & EXTRA_MIDI_CONFIGURATION != 0,
        }
    }

    /// Read it straight out of a loaded module's header.
    pub const fn from_header(header: &ModuleHeader) -> ItFormatExtra { ItFormatExtra::decode(header.format_extra) }

    /// Whether the instrument column names an instrument rather than a sample.
    pub const fn is_instrument_mode(&self) -> bool { self.flags & FLAG_INSTRUMENT_MODE != 0 }

    /// Whether the file asks for old-effects behaviour.
    pub const fn is_old_effects(&self) -> bool { self.flags & FLAG_OLD_EFFECTS != 0 }

    /// Whether `Gxx` shares its memory with `Exx`/`Fxx`.
    pub const fn is_compatible_gxx(&self) -> bool { self.flags & FLAG_COMPATIBLE_GXX != 0 }

    /// Whether the MIDI pitch controller is requested.
    pub const fn uses_midi_pitch_controller(&self) -> bool { self.flags & FLAG_MIDI_PITCH_CONTROLLER != 0 }

    /// Whether OpenMPT's extended filter range applies.
    pub const fn has_extended_filter_range(&self) -> bool { self.flags & FLAG_EXTENDED_FILTER_RANGE != 0 }
}

// ── format_data ─────────────────────────────────────────────────────────────────────

/// Offset of the raw `ChnPan` copy inside [`ModuleHeader::format_data`].
pub const DATA_CHANNEL_PAN_OFFSET: usize = 0x08;
/// Offset of the sample global-volume table's length word.
pub const DATA_SAMPLE_VOLUME_OFFSET: usize = DATA_CHANNEL_PAN_OFFSET + MAX_CHANNELS;
/// Bytes in the fixed part of [`ModuleHeader::format_data`], before the variable tables.
pub const DATA_FIXED_BYTES: usize = DATA_SAMPLE_VOLUME_OFFSET + 2;

/// `format_data` block-flags bit 0 — a MIDI configuration block follows the sample table.
const DATA_FLAG_MIDI_CONFIGURATION: u8 = 1 << 0;

/// The IT reader for
/// [`ModuleHeader::format_data`](starplayer_model::ModuleHeader::format_data) — everything
/// the effect processor needs that neither the model nor `format_extra`'s 32 bits have
/// room for.
///
/// # The layout this reads
///
/// ```text
/// 0x00  u16  Cwt/v
/// 0x02  u16  Cmwt
/// 0x04  u8   Sep, the stereo separation (0..=128)
/// 0x05  u8   PWD, the MIDI pitch-wheel depth
/// 0x06  u8   block flags: bit 0 = a MIDI configuration block is present
/// 0x07  u8   reserved, zero
/// 0x08  [u8; 64]  ChnPan, verbatim — 0..=64 position, 100 surround, +128 disabled
/// 0x48  u16  N, the number of samples
/// 0x4A  [u8; N]   each sample's GvL, its 0..=64 global volume
/// 0x4A+N [u8; 4896]  the MIDI configuration, iff block flags bit 0:
///                    9x32 global macros, then 16x32 SFx, then 128x32 Zxx
/// ```
///
/// The sample global volumes live here rather than on
/// [`SampleSpec`](starplayer_model::SampleSpec) because IT's `Vol` and `GvL` are two
/// different multiplicands of the final-volume formula — `Vol` is the volume a note starts
/// the channel at, `GvL` scales every note the sample sounds — and the model has one field
/// for the first.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItFormatData<'header> {
    bytes: &'header [u8],
}

impl<'header> ItFormatData<'header> {
    /// Borrow the block out of a loaded module's header, or `None` if it is not an IT's.
    pub fn from_header(header: &'header ModuleHeader) -> Option<ItFormatData<'header>> {
        ItFormatData::new(&header.format_data)
    }

    /// Borrow the block out of raw bytes.
    pub fn new(bytes: &'header [u8]) -> Option<ItFormatData<'header>> {
        if bytes.len() < DATA_FIXED_BYTES {
            return None;
        }
        Some(ItFormatData { bytes })
    }

    fn byte(&self, offset: usize) -> u8 { self.bytes.get(offset).copied().unwrap_or(0) }

    fn word(&self, offset: usize) -> u16 {
        match self.bytes.get(offset..offset + 2) {
            Some([low, high]) => u16::from_le_bytes([*low, *high]),
            _ => 0,
        }
    }

    /// `Cwt/v`, the tracker that created the file.
    pub fn tracker_version(&self) -> u16 { self.word(0x00) }

    /// `Cmwt`, the format version.
    pub fn format_version(&self) -> u16 { self.word(0x02) }

    /// `Sep`, the stereo separation, 0..=128.
    pub fn stereo_separation(&self) -> u8 { self.byte(0x04) }

    /// `PWD`, the MIDI pitch-wheel depth.
    pub fn pitch_wheel_depth(&self) -> u8 { self.byte(0x05) }

    /// One channel's raw `ChnPan` byte, or `None` past channel 63.
    pub fn channel_pan_raw(&self, channel: u8) -> Option<u8> {
        if channel as usize >= MAX_CHANNELS {
            return None;
        }
        self.bytes.get(DATA_CHANNEL_PAN_OFFSET + channel as usize).copied()
    }

    /// Whether a channel is panned to surround.
    pub fn is_surround(&self, channel: u8) -> bool {
        matches!(self.channel_pan_raw(channel), Some(raw) if raw & !PAN_DISABLED == PAN_SURROUND)
    }

    /// Whether a channel is disabled: its notes are silent, its effects still run.
    pub fn is_disabled(&self, channel: u8) -> bool {
        matches!(self.channel_pan_raw(channel), Some(raw) if raw & PAN_DISABLED != 0)
    }

    /// Samples the global-volume table covers.
    pub fn sample_count(&self) -> u16 { self.word(DATA_SAMPLE_VOLUME_OFFSET) }

    /// One sample's `GvL`, its 0..=64 global volume, or `None` past the table.
    pub fn sample_global_volume(&self, sample: u16) -> Option<u8> {
        if sample >= self.sample_count() {
            return None;
        }
        self.bytes.get(DATA_FIXED_BYTES + sample as usize).copied()
    }

    /// The whole embedded MIDI configuration, or `None` when the file had none.
    pub fn midi_configuration(&self) -> Option<&'header [u8]> {
        if self.byte(0x06) & DATA_FLAG_MIDI_CONFIGURATION == 0 {
            return None;
        }
        let start = DATA_FIXED_BYTES + self.sample_count() as usize;
        self.bytes.get(start..start + MIDI_CONFIGURATION_BYTES)
    }

    fn macro_at(&self, index: usize) -> Option<&'header [u8]> {
        let configuration = self.midi_configuration()?;
        configuration.get(index * MIDI_MACRO_BYTES..(index + 1) * MIDI_MACRO_BYTES)
    }

    /// One of the nine global MIDI macros, 0..=8.
    pub fn global_macro(&self, index: usize) -> Option<&'header [u8]> {
        if index >= MIDI_GLOBAL_MACROS {
            return None;
        }
        self.macro_at(index)
    }

    /// One of the sixteen `SFx` parametered macros, 0..=15.
    pub fn parametered_macro(&self, index: usize) -> Option<&'header [u8]> {
        if index >= MIDI_PARAMETERED_MACROS {
            return None;
        }
        self.macro_at(MIDI_GLOBAL_MACROS + index)
    }

    /// One of the 128 `Zxx` fixed macros, 0..=127 for `Z80`..`ZFF`.
    pub fn fixed_macro(&self, index: usize) -> Option<&'header [u8]> {
        if index >= MIDI_FIXED_MACROS {
            return None;
        }
        self.macro_at(MIDI_GLOBAL_MACROS + MIDI_PARAMETERED_MACROS + index)
    }
}

/// Build the `format_data` block [`ItFormatData`] reads.
pub fn encode_format_data(header: &ItHeader, sample_global_volumes: &[u8], midi_configuration: Option<&[u8]>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(DATA_FIXED_BYTES + sample_global_volumes.len() + midi_configuration.map_or(0, <[u8]>::len));
    bytes.extend_from_slice(&header.tracker_version.to_le_bytes());
    bytes.extend_from_slice(&header.format_version.to_le_bytes());
    bytes.push(header.stereo_separation);
    bytes.push(header.pitch_wheel_depth);
    bytes.push(if midi_configuration.is_some() { DATA_FLAG_MIDI_CONFIGURATION } else { 0 });
    bytes.push(0);
    bytes.extend_from_slice(&header.channel_pan);
    let sample_count = u16::try_from(sample_global_volumes.len()).unwrap_or(u16::MAX);
    bytes.extend_from_slice(&sample_count.to_le_bytes());
    bytes.extend_from_slice(sample_global_volumes.get(..sample_count as usize).unwrap_or(sample_global_volumes));
    if let Some(configuration) = midi_configuration {
        bytes.extend_from_slice(configuration);
        // A short block is padded so every accessor's slice is either whole or absent.
        bytes.resize(DATA_FIXED_BYTES + sample_count as usize + MIDI_CONFIGURATION_BYTES, 0);
    }
    bytes
}

fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, Error> {
    bytes.get(offset).copied().ok_or(Error::Truncated { offset, needed: 1 })
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, Error> {
    match bytes.get(offset..offset + 2) {
        Some([low, high]) => Ok(u16::from_le_bytes([*low, *high])),
        _ => Err(Error::Truncated { offset, needed: 2 }),
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, Error> {
    match bytes.get(offset..offset + 4) {
        Some([a, b, c, d]) => Ok(u32::from_le_bytes([*a, *b, *c, *d])),
        _ => Err(Error::Truncated { offset, needed: 4 }),
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A `0xC0`-byte header with `IMPM` in place and everything else zero.
    fn synthetic_header() -> [u8; HEADER_LENGTH] {
        let mut bytes = [0u8; HEADER_LENGTH];
        bytes[..4].copy_from_slice(&MAGIC);
        bytes
    }

    #[test]
    fn a_header_reads_every_field_the_loader_uses() {
        let mut bytes = synthetic_header();
        bytes[0x1E] = 4;
        bytes[0x1F] = 16;
        bytes[0x20..0x22].copy_from_slice(&12u16.to_le_bytes());
        bytes[0x22..0x24].copy_from_slice(&3u16.to_le_bytes());
        bytes[0x24..0x26].copy_from_slice(&5u16.to_le_bytes());
        bytes[0x26..0x28].copy_from_slice(&7u16.to_le_bytes());
        bytes[0x28..0x2A].copy_from_slice(&0x0214u16.to_le_bytes());
        bytes[0x2A..0x2C].copy_from_slice(&0x0214u16.to_le_bytes());
        bytes[0x2C..0x2E].copy_from_slice(&(FLAG_STEREO | FLAG_INSTRUMENT_MODE | FLAG_LINEAR_SLIDES).to_le_bytes());
        bytes[0x2E..0x30].copy_from_slice(&(SPECIAL_SONG_MESSAGE | SPECIAL_MIDI_CONFIGURATION).to_le_bytes());
        bytes[0x30] = 128;
        bytes[0x31] = 48;
        bytes[0x32] = 6;
        bytes[0x33] = 125;
        bytes[0x34] = 128;
        bytes[0x35] = 2;
        bytes[0x36..0x38].copy_from_slice(&40u16.to_le_bytes());
        bytes[0x38..0x3C].copy_from_slice(&0x1234u32.to_le_bytes());
        bytes[0x40] = 0;
        bytes[0x41] = PAN_SURROUND;
        bytes[0x42] = 32 | PAN_DISABLED;
        bytes[0x80] = 64;

        let header = ItHeader::parse(&bytes).expect("a valid header");

        assert_eq!(header.highlight_minor, 4);
        assert_eq!(header.highlight_major, 16);
        assert_eq!(header.order_count, 12);
        assert_eq!(header.instrument_count, 3);
        assert_eq!(header.sample_count, 5);
        assert_eq!(header.pattern_count, 7);
        assert_eq!(header.tracker_version, 0x0214);
        assert_eq!(header.format_version, 0x0214);
        assert!(header.is_stereo() && header.is_instrument_mode() && header.is_linear_slides());
        assert!(!header.has_old_instruments());
        assert!(header.has_midi_configuration());
        assert_eq!(header.global_volume, 128);
        assert_eq!(header.mix_volume, 48);
        assert_eq!(header.initial_speed, 6);
        assert_eq!(header.initial_tempo, 125);
        assert_eq!(header.stereo_separation, 128);
        assert_eq!(header.pitch_wheel_depth, 2);
        assert_eq!(header.message_length, 40);
        assert_eq!(header.message_offset, 0x1234);
        assert_eq!(header.channel_pan[1], PAN_SURROUND);
        assert_eq!(header.channel_volume[0], 64);
    }

    #[test]
    fn a_short_or_unsigned_header_is_an_error_not_a_panic() {
        assert_eq!(ItHeader::parse(&[]), Err(Error::Truncated { offset: 0, needed: HEADER_LENGTH }));
        assert_eq!(ItHeader::parse(&[0u8; HEADER_LENGTH - 1]), Err(Error::Truncated { offset: 0, needed: HEADER_LENGTH }));
        assert_eq!(ItHeader::parse(&[0u8; HEADER_LENGTH]), Err(Error::BadMagic));
    }

    #[test]
    fn pan_maps_the_zero_to_sixty_four_scale_onto_the_full_bipolar_range() {
        assert_eq!(pan_to_bipolar(0), I1F15::MIN + I1F15::DELTA, "hard left is full scale");
        assert_eq!(pan_to_bipolar(PAN_CENTRE), I1F15::ZERO);
        assert_eq!(pan_to_bipolar(PAN_RIGHT), I1F15::MAX, "hard right is full scale");
        assert_eq!(pan_to_bipolar(16), -pan_to_bipolar(48));
        assert_eq!(pan_to_bipolar(PAN_SURROUND), I1F15::ZERO, "surround plays from the centre until G4 renders it");
        assert_eq!(pan_to_bipolar(16 | PAN_DISABLED), pan_to_bipolar(16), "a disabled channel keeps its position");
        assert_eq!(pan_to_bipolar(200), I1F15::ZERO, "an out-of-range value is centred rather than wrapped");
    }

    #[test]
    fn the_dialect_follows_the_tracker_version_nibble_and_the_hand_recognised_combinations() {
        let with = |tracker_version, format_version, reserved: [u8; 4]| ItHeader {
            tracker_version,
            format_version,
            reserved,
            ..ItHeader::parse(&synthetic_header()).expect("a valid header")
        };

        assert_eq!(with(0x0214, 0x0214, [0; 4]).dialect(), FormatDialect::ImpulseTracker);
        assert_eq!(with(0x1234, 0x0214, [0; 4]).dialect(), FormatDialect::SchismTracker);
        assert_eq!(with(0x5129, 0x0214, OPENMPT_RESERVED_MARKER).dialect(), FormatDialect::OpenMptIt);
        assert_eq!(with(0x5129, 0x0214, [0; 4]).dialect(), FormatDialect::ModPlugIt, "the 0x5000 nibble without OMPT is a compatibility export");
        assert_eq!(with(0x0888, 0x0214, [0; 4]).dialect(), FormatDialect::OpenMptIt);
        assert_eq!(with(0x0214, 0x0888, [0; 4]).dialect(), FormatDialect::OpenMptIt);
        assert_eq!(with(0x0214, 0x0202, [0; 4]).dialect(), FormatDialect::ModPlugIt);
        assert_eq!(with(0x0217, 0x0200, [0; 4]).dialect(), FormatDialect::ModPlugIt);
        assert_eq!(with(0x2000, 0x0214, [0; 4]).dialect(), FormatDialect::Unknown);
    }

    #[test]
    fn the_format_extra_word_round_trips_every_field() {
        let extra = ItFormatExtra {
            flags: FLAG_STEREO | FLAG_INSTRUMENT_MODE | FLAG_OLD_EFFECTS | FLAG_COMPATIBLE_GXX | FLAG_EXTENDED_FILTER_RANGE,
            special: 0x09,
            old_instruments: true,
            has_midi_configuration: true,
        };
        assert_eq!(ItFormatExtra::decode(extra.encode()), extra);
        assert!(extra.is_instrument_mode() && extra.is_old_effects() && extra.is_compatible_gxx());
        assert!(extra.has_extended_filter_range() && !extra.uses_midi_pitch_controller());
        assert_eq!(ItFormatExtra::decode(0), ItFormatExtra::default());
    }

    #[test]
    fn the_format_data_block_round_trips_every_field() {
        let mut file_header = ItHeader::parse(&synthetic_header()).expect("a valid header");
        file_header.tracker_version = 0x0217;
        file_header.format_version = 0x0215;
        file_header.stereo_separation = 96;
        file_header.pitch_wheel_depth = 12;
        file_header.channel_pan[0] = 0;
        file_header.channel_pan[1] = PAN_SURROUND;
        file_header.channel_pan[2] = 32 | PAN_DISABLED;

        let mut configuration = vec![0u8; MIDI_CONFIGURATION_BYTES];
        configuration[0] = b'F';
        configuration[MIDI_GLOBAL_MACROS * MIDI_MACRO_BYTES] = b'S';
        configuration[(MIDI_GLOBAL_MACROS + MIDI_PARAMETERED_MACROS) * MIDI_MACRO_BYTES] = b'Z';

        let bytes = encode_format_data(&file_header, &[64, 32, 0], Some(&configuration));
        let data = ItFormatData::new(&bytes).expect("the block is long enough");

        assert_eq!(data.tracker_version(), 0x0217);
        assert_eq!(data.format_version(), 0x0215);
        assert_eq!(data.stereo_separation(), 96);
        assert_eq!(data.pitch_wheel_depth(), 12);
        assert_eq!(data.channel_pan_raw(1), Some(PAN_SURROUND));
        assert!(data.is_surround(1) && !data.is_disabled(1));
        assert!(data.is_disabled(2) && !data.is_surround(2));
        assert_eq!(data.channel_pan_raw(64), None);
        assert_eq!(data.sample_count(), 3);
        assert_eq!(data.sample_global_volume(0), Some(64));
        assert_eq!(data.sample_global_volume(2), Some(0));
        assert_eq!(data.sample_global_volume(3), None);
        assert_eq!(data.global_macro(0).map(|bytes| bytes[0]), Some(b'F'));
        assert_eq!(data.parametered_macro(0).map(|bytes| bytes[0]), Some(b'S'));
        assert_eq!(data.fixed_macro(0).map(|bytes| bytes[0]), Some(b'Z'));
        assert_eq!(data.global_macro(MIDI_GLOBAL_MACROS), None);
        assert_eq!(data.parametered_macro(MIDI_PARAMETERED_MACROS), None);
        assert_eq!(data.fixed_macro(MIDI_FIXED_MACROS), None);
    }

    #[test]
    fn a_file_with_no_midi_block_answers_none_for_every_macro() {
        let file_header = ItHeader::parse(&synthetic_header()).expect("a valid header");
        let bytes = encode_format_data(&file_header, &[], None);
        let data = ItFormatData::new(&bytes).expect("the block is long enough");

        assert_eq!(bytes.len(), DATA_FIXED_BYTES);
        assert_eq!(data.midi_configuration(), None);
        assert_eq!(data.global_macro(0), None);
        assert_eq!(data.sample_count(), 0);
        assert_eq!(ItFormatData::new(&[]), None, "a block shorter than the fixed part is not one");
    }
}
